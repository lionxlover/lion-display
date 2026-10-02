//! The Phase 19 benchmark suites (performance + security).
//!
//! Every benchmark drives a *real* project surface — the actual codec,
//! the actual framing writer/reader over a real socketpair, the actual
//! damage algebra, the actual 60 Hz frame scheduler, the actual color
//! pipeline, the actual clipboard pipe, and the actual constant-time
//! token walk and hash-chained audit verification. Inputs are
//! deterministic ([`SplitMix64`]-seeded) so runs are comparable across
//! machines to the extent the hardware allows, and every result
//! carries its unit.
//!
//! `quick` mode shrinks run counts and corpus sizes so the suite can
//! run as a dev-profile smoke test; the committed
//! `docs/benchmarks.md` numbers come from a release run at full size.

use ldp_clipboard::pipe;
use ldp_color::matrix::{apply3, primaries_matrix};
use ldp_color::transfer::{decode_transfer, encode_transfer};
use ldp_compositor::sched_types::SchedulerConfig;
use ldp_compositor::scheduler::FrameScheduler;
use ldp_core::caps::{Scope, ScopeSet};
use ldp_core::color::Primaries;
use ldp_core::geometry::{Rect, Region};
use ldp_core::ids::ObjectId;
use ldp_core::limits::Limits;
use ldp_core::time::{Mono, RefreshInterval};
use ldp_core::wire::{ArgType, Primitive, Value};
use ldp_protocol::{decode, Message, ValidationMode};
use ldp_security::chain::{AuditAction, AuditChain, AuditEvent};
use ldp_security::grant::{GrantTable, LcgSeed, SubmitOutcome};
use ldp_security::matrix::{decide, ALL_OPERATIONS};
use ldp_transport::{FdList, FramedReader, FramedWriter, TransportStream};

use crate::bench::{bench, bench_bytes, BenchSuite};
use crate::rng::SplitMix64;

/// 60 Hz frame interval in ns.
const INTERVAL_60HZ: u64 = 16_666_666;

/// Build the performance suite.
///
/// `quick` shrinks every corpus and run count (dev-profile smoke);
/// `false` is the full release-profile report shape.
#[must_use]
pub fn performance_suite(quick: bool) -> BenchSuite {
    let mut suite = BenchSuite::new("performance");
    codec_benchmarks(&mut suite, quick);
    transport_benchmarks(&mut suite, quick);
    geometry_benchmarks(&mut suite, quick);
    damage_benchmarks(&mut suite, quick);
    scheduler_benchmarks(&mut suite, quick);
    color_benchmarks(&mut suite, quick);
    clipboard_benchmarks(&mut suite, quick);
    renderer_benchmarks(&mut suite, quick);
    remote_benchmarks(&mut suite, quick);
    suite
}

/// Build the security suite: the constant-time token walk (valid and
/// forged), the audit chain append and full verification, and the
/// permission matrix decision sweep.
#[must_use]
pub fn security_suite(quick: bool) -> BenchSuite {
    let mut suite = BenchSuite::new("security");
    token_benchmarks(&mut suite, quick);
    audit_benchmarks(&mut suite, quick);
    matrix_benchmarks(&mut suite, quick);
    suite
}

// ---- codec ----------------------------------------------------------

fn bind_message() -> Message {
    Message::new(1, 1)
        .arg(Value::String("ldp.core.output".into()))
        .arg(Value::Uint32(1))
        .arg(Value::NewId(ObjectId::client(9).expect("client range")))
}

fn damage_message() -> Message {
    let rects: Vec<Primitive> = (0..8)
        .map(|i| Primitive::Rect(Rect::new(i * 40, (i % 4) * 90, 320, 240)))
        .collect();
    let damage = Value::array(ArgType::Rect, rects).expect("rect array");
    Message::new(0x21, 3).arg(damage)
}

fn codec_benchmarks(suite: &mut BenchSuite, quick: bool) {
    let (ops, runs) = if quick { (64, 3) } else { (512, 7) };
    let limits = Limits::DEFAULT;

    let bind = bind_message();
    let bind_bytes = bind.encode(&limits).expect("bind encodes");
    suite.add(bench(
        "codec: encode registry.bind",
        1,
        runs,
        ops,
        move || {
            for _ in 0..ops {
                // black_box: the encode must not be elided.
                let _ = std::hint::black_box(bind.encode(&limits));
            }
        },
    ));

    let damage = damage_message();
    let bytes = damage.encode(&limits).expect("damage encodes");
    suite.add(bench(
        "codec: encode surface.damage (8 rects)",
        1,
        runs,
        ops,
        move || {
            for _ in 0..ops {
                let _ = std::hint::black_box(damage.encode(&limits));
            }
        },
    ));

    suite.add(bench(
        "codec: decode surface.damage (8 rects)",
        1,
        runs,
        ops,
        move || {
            for _ in 0..ops {
                let _ = std::hint::black_box(decode(&bytes, 0, &limits, ValidationMode::Strict));
            }
        },
    ));

    suite.add(bench(
        "codec: decode + strict signature check (registry.bind)",
        1,
        runs,
        ops,
        move || {
            for _ in 0..ops {
                let msg = decode(&bind_bytes, 0, &limits, ValidationMode::Strict).expect("valid");
                let _ = std::hint::black_box(ldp_protocol::check_signature(
                    &ldp_protocol::REGISTRY,
                    &msg,
                    "ldp.core.registry",
                    ldp_protocol::Direction::Request,
                    1,
                    ValidationMode::Strict,
                ));
            }
        },
    ));
}

// ---- transport ------------------------------------------------------

fn transport_benchmarks(suite: &mut BenchSuite, quick: bool) {
    let (small_ops, runs) = if quick { (32, 3) } else { (256, 7) };
    let (big_ops, big_runs) = if quick { (8, 3) } else { (64, 7) };

    // 1 KiB framed message (128 words).
    let small = framed_message(128);
    let (mut tx, mut rx) = TransportStream::pair().expect("socketpair");
    let mut writer = FramedWriter::without_hooks(Limits::DEFAULT);
    let mut reader = FramedReader::new(Limits::DEFAULT);
    suite.add(bench(
        "transport: 1 KiB frame round-trip (sendmsg + recvmsg)",
        1,
        runs,
        small_ops,
        || {
            for _ in 0..small_ops {
                writer
                    .send_msg(&mut tx, &small, &mut FdList::new())
                    .expect("send");
                let frame = reader.recv_msg(&mut rx).expect("recv");
                assert_eq!(frame.message_bytes(), &small[..]);
            }
        },
    ));

    // 64 KiB framed message (8192 words) — bandwidth shape.
    let big = framed_message(8192);
    suite.add(bench_bytes(
        "transport: 64 KiB frame round-trip",
        1,
        big_runs,
        big_ops,
        big.len() as u64,
        || {
            for _ in 0..big_ops {
                writer
                    .send_msg(&mut tx, &big, &mut FdList::new())
                    .expect("send");
                let frame = reader.recv_msg(&mut rx).expect("recv");
                assert_eq!(frame.payload_words(), 8192);
            }
        },
    ));
}

/// A deterministic framed message of `words` payload words.
fn framed_message(words: u32) -> Vec<u8> {
    let mut msg = vec![0u8; 16 + words as usize * 8];
    msg[0..4].copy_from_slice(&words.to_le_bytes());
    msg[14..16].copy_from_slice(&0u16.to_le_bytes());
    let mut r = SplitMix64::new(0xC0DE);
    for (i, b) in msg[16..].iter_mut().enumerate() {
        let _ = i;
        *b = r.byte();
    }
    msg
}

// ---- geometry / damage ----------------------------------------------

fn geometry_benchmarks(suite: &mut BenchSuite, quick: bool) {
    let (rects, runs) = if quick { (32, 3) } else { (256, 7) };
    let mut rng = SplitMix64::new(0xD4A4_67A9);
    let boxes: Vec<Rect> = (0..rects)
        .map(|_| {
            let px = rng.below(3840 - 320) as i32;
            let py = rng.below(2160 - 240) as i32;
            let pw = 64 + rng.below(256) as u32;
            let ph = 64 + rng.below(192) as u32;
            Rect::new(px, py, pw, ph)
        })
        .collect();

    suite.add(bench(
        "geometry: Region.add (256 seeded rects)",
        1,
        runs,
        rects,
        || {
            let mut region = Region::from_rect(boxes[0]);
            for rect in &boxes[1..] {
                region.add(*rect);
            }
        },
    ));

    let base = {
        let mut region = Region::from_rect(boxes[0]);
        for rect in &boxes[1..] {
            region.add(*rect);
        }
        region
    };
    let cutters: Vec<Rect> = boxes.iter().step_by(4).copied().collect();
    suite.add(bench(
        "geometry: Region.subtract (64 cutters)",
        1,
        runs,
        cutters.len() as u64,
        || {
            let mut acc = base.clone();
            for cutter in &cutters {
                acc = acc.subtract(&Region::from_rect(*cutter));
            }
            assert!(!acc.is_empty());
        },
    ));
}

// ---- damage engine ---------------------------------------------------

/// A deterministic desktop tree: `windows` cascading opaque root
/// surfaces, committed and primed through one damage pass so the
/// steady-state passes below measure exactly the quiet-frame tax —
/// the cost the compositor pays every commit batch while nothing
/// moves (an animation-driven desktop runs this between every pair of
/// moving frames).
fn quiet_desktop_tree(windows: usize) -> ldp_compositor::SurfaceTree {
    let mut tree = ldp_compositor::SurfaceTree::new();
    for i in 0..windows as u64 {
        let id = ldp_compositor::SurfaceId::from_raw(i + 1);
        let x = 48 * (i % 16) as i32;
        let y = 32 * (i % 8) as i32;
        tree.create_root(id, x, y).expect("create");
        tree.pending_mut(id)
            .expect("pending")
            .attach(Some(ldp_compositor::BufferAttachment {
                width: 240,
                height: 160,
                generation: 1,
            }));
        tree.pending_mut(id)
            .expect("pending")
            .set_opaque_region(Region::from_rect(Rect::new(0, 0, 240, 160)));
        tree.commit(id).expect("commit");
    }
    // Prime the pass records: one compute so every surface carries its
    // "old" side; the benchmark body then runs on quiescent frames.
    let changes = tree.take_changes();
    let damage =
        ldp_compositor::DamageEngine::compute(&mut tree, &changes, Rect::new(0, 0, 1920, 1080));
    assert!(!damage.repaint.is_empty(), "the first map must damage");
    tree
}

fn damage_benchmarks(suite: &mut BenchSuite, quick: bool) {
    let (windows, frames, runs) = if quick { (16, 64, 3) } else { (64, 200, 7) };
    let output = Rect::new(0, 0, 1920, 1080);

    suite.add(bench(
        "damage: quiet 64-window desktop pass (no changes)",
        1,
        runs,
        frames,
        move || {
            let mut tree = quiet_desktop_tree(windows);
            for _ in 0..frames {
                let changes = tree.take_changes();
                let damage = ldp_compositor::DamageEngine::compute(&mut tree, &changes, output);
                // The quiet frame's contract: nothing to repaint, the
                // whole point of the pass records.
                assert!(damage.repaint.is_empty());
                std::hint::black_box(&damage);
            }
        },
    ));

    // One window slides one pixel per frame (the moving-cursor class:
    // a single surface's coverage rule fires, everything else stays
    // quiet) — the damage engine's steady working load.
    suite.add(bench(
        "damage: one moving window among 64 (1 px/frame)",
        1,
        runs,
        frames,
        move || {
            let mut tree = quiet_desktop_tree(windows);
            let mover = ldp_compositor::SurfaceId::from_raw(1);
            for f in 0..frames {
                // A shell-arm move: position changes without a client
                // commit (the re-layout doctrine) — every frame, so
                // each measured pass carries real coverage work.
                tree.set_position_now(mover, 48 + f as i32, 0)
                    .expect("move");
                let changes = tree.take_changes();
                let damage = ldp_compositor::DamageEngine::compute(&mut tree, &changes, output);
                assert!(!damage.repaint.is_empty());
                std::hint::black_box(&damage);
            }
        },
    ));
}

// ---- scheduler ------------------------------------------------------

fn scheduler_benchmarks(suite: &mut BenchSuite, quick: bool) {
    let (frames, runs) = if quick { (120, 3) } else { (1000, 5) };
    let interval = RefreshInterval::from_millihz(60_000).expect("60 Hz");
    let surface = ldp_compositor::surface::SurfaceId::from_raw(1);

    suite.add(bench(
        "scheduler: 60 Hz decision cadence (frame_request + commit + drain + flip)",
        1,
        runs,
        frames,
        || {
            let mut sched =
                FrameScheduler::new(interval, SchedulerConfig::default()).expect("config");
            for f in 0..frames {
                let ts = Mono::from_ns(f * INTERVAL_60HZ);
                sched.frame_request(surface, f, ts);
                sched.commit(surface, ts);
                let events = sched.drain();
                assert!(!events.is_empty() || f == 0);
                sched.observe_flip(Mono::from_ns(f * INTERVAL_60HZ + 600_000));
            }
        },
    ));
}

// ---- color ----------------------------------------------------------

fn color_benchmarks(suite: &mut BenchSuite, quick: bool) {
    let (n, runs) = if quick {
        (1usize << 14, 3)
    } else {
        (1usize << 20, 5)
    };
    // Deterministic [0, 1] ramp — the classic sampling grid.
    let input: Vec<f32> = (0..n).map(|i| i as f32 / n as f32).collect();

    suite.add(bench(
        "color: sRGB transfer decode (1 Mi samples)",
        1,
        runs,
        n as u64,
        || {
            let mut sink = 0f32;
            for v in &input {
                sink += decode_transfer(ldp_core::color::TransferFunction::Srgb, *v);
            }
            assert!(sink.is_finite());
        },
    ));

    suite.add(bench(
        "color: PQ transfer encode (1 Mi samples)",
        1,
        runs,
        n as u64,
        || {
            let mut sink = 0f32;
            for v in &input {
                sink += encode_transfer(ldp_core::color::TransferFunction::Pq, *v);
            }
            assert!(sink.is_finite());
        },
    ));

    let m = primaries_matrix(Primaries::Bt709, Primaries::Bt2020);
    suite.add(bench(
        "color: matrix apply3 (1 Mi RGB triples, BT.709 -> BT.2020)",
        1,
        runs,
        n as u64,
        || {
            let mut sink = 0f32;
            for (i, v) in input.iter().enumerate() {
                let out = apply3(&m, [*v, input[(i + 7) % n], input[(i + 13) % n]]);
                sink += out[0] + out[1] + out[2];
            }
            assert!(sink.is_finite());
        },
    ));
}

// ---- clipboard pipe -------------------------------------------------

fn clipboard_benchmarks(suite: &mut BenchSuite, quick: bool) {
    const PAGE: usize = 4096;
    let (total, runs) = if quick {
        (2u64 << 20, 2)
    } else {
        (16u64 << 20, 3)
    };
    let payload: Vec<u8> = {
        let mut rng = SplitMix64::new(0xBEEF_5EED);
        (0..total).map(|_| rng.byte()).collect()
    };

    suite.add(bench_bytes(
        "clipboard: 16 MiB stream through a real pipe (4 KiB pages)",
        1,
        runs,
        1,
        total,
        || {
            let mut pair = pipe::pipe().expect("pipe");
            pair.set_blocking().expect("blocking pipe");
            let mut reader = pair.read;
            let reader = std::thread::spawn(move || {
                let mut buf = vec![0u8; PAGE];
                let mut seen = 0usize;
                loop {
                    match reader.read_blocking(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => seen += n,
                        Err(e) => panic!("pipe read failed: {e}"),
                    }
                }
                seen
            });
            let mut writer = pair.write;
            for page in payload.chunks(PAGE) {
                writer.write_all_blocking(page).expect("pipe write");
            }
            drop(writer);
            let seen = reader.join().expect("reader thread");
            assert_eq!(seen, total as usize);
        },
    ));
}

// ---- renderer -------------------------------------------------------

/// The Phase 8 performance shape as a permanent suite row: one full
/// 3840×2160 XRGB composition through the word-copy path (the hottest
/// per-frame cost a display server has).
fn renderer_benchmarks(suite: &mut BenchSuite, quick: bool) {
    renderer_copy_row(suite, quick);
    renderer_argb_row(suite, quick);
    renderer_desktop_row(suite, quick);
    renderer_styled_rows(suite, quick);
    renderer_desktop_matrix_rows(suite, quick);
}

/// The Liquid desktop-matrix shape (Phase 30): the same material
/// language as the phone frame, laid out the way a desktop actually
/// sits — a plain wallpaper, four rounded-and-shadowed windows in the
/// four quadrants, and one frosted dock bar along the bottom — every
/// frame at full damage.
///
/// The matrix this phase owes the operator: **every desktop size** —
/// 1920×1080 (FHD, 2.1 MP), 2560×1440 (QHD, 3.7 MP), 3440×1440 (the
/// 21:9 ultrawide, 5.0 MP), 3840×2160 (4K, 8.3 MP) — at the High
/// tier (what a GPU serves), plus the tier the no-GPU doctrine picks
/// for each size (what a machine without a graphics card actually
/// runs, automatically). One scene, proportional geometry, the
/// steady state and (at the two ends) the cold first frame.
#[allow(clippy::too_many_lines)]
fn renderer_desktop_matrix_rows(suite: &mut BenchSuite, quick: bool) {
    use ldp_renderer::EffectTier;

    // (runs, the size matrix, first-frame rows)
    let (runs, sizes, first_frames) = if quick {
        (3, vec![(1920u32, 1080u32), (2560u32, 1440u32)], false)
    } else {
        (
            5,
            vec![
                (1920u32, 1080u32),
                (2560u32, 1440u32),
                (3440u32, 1440u32),
                (3840u32, 2160u32),
            ],
            true,
        )
    };

    for (w, h) in sizes {
        // The scene at this size: proportional geometry from the FHD
        // reference (the 1920×1080 layout scaled by exact integer
        // ratios — no float drift between matrix cells).
        let (paper_data, paper_geometry) =
            ldp_renderer::testkit::pattern_buffer(w, h, ldp_core::buffer::FourCC::XRGB8888);
        let paper_view =
            ldp_renderer::BufferView::new(4, &paper_data, paper_geometry).expect("view");
        let mut paper = ldp_renderer::SurfaceLayer::new(
            paper_view,
            Rect::new(0, 0, w, h),
            ldp_core::geometry::Transform::Normal,
            ldp_core::color::ColorDescription::srgb_sdr(),
            1.0,
            Region::from_rect(Rect::new(0, 0, w, h)),
        );
        paper.style = ldp_renderer::LayerStyle::default();

        // Four windows in the four quadrants, inset 6% x 12%: the
        // desktop cascade's honest footprint.
        let card_w = w * 2 / 5;
        let card_h = h * 2 / 5;
        let inset_x = w * 3 / 50;
        let inset_y = h * 3 / 25;
        let card_positions = [
            (inset_x as i32, inset_y as i32),
            (w as i32 - inset_x as i32 - card_w as i32, inset_y as i32),
            (inset_x as i32, h as i32 - inset_y as i32 - card_h as i32),
            (
                w as i32 - inset_x as i32 - card_w as i32,
                h as i32 - inset_y as i32 - card_h as i32,
            ),
        ];

        let output = ldp_renderer::OutputDesc::new(
            w,
            h,
            ldp_core::buffer::FourCC::ARGB8888,
            ldp_core::color::ColorDescription::srgb_sdr(),
        )
        .expect("output");
        let damage = ldp_renderer::testkit::full_damage(&output);

        // The two tier shapes this matrix owes: High (the GPU's tier)
        // and the no-GPU doctrine's pick for this size.
        let pixels = u64::from(w) * u64::from(h);
        let no_gpu_tier = ldp_renderer::EffectChoice::Auto.resolve(false, pixels, false);

        for (tier, tier_name) in [
            (EffectTier::High, "High tier"),
            (no_gpu_tier, "the no-GPU doctrine's tier"),
        ] {
            let card_style = tier.opaque_style();
            let dock_style = tier.translucent_style();

            // The frosted dock bar: 75% of the width, 6.7% of the
            // height, centered at the bottom.
            let dock_w = w * 3 / 4;
            let dock_h = h / 15 + 1;
            let (card_data, card_geometry) = opaque_argb_buffer(card_w, card_h);
            let mut cards: Vec<ldp_renderer::SurfaceLayer<'_>> = Vec::with_capacity(4);
            for (cx, cy) in card_positions {
                let mut layer = ldp_renderer::SurfaceLayer::new(
                    ldp_renderer::BufferView::new(6, &card_data, card_geometry.clone())
                        .expect("view"),
                    Rect::new(cx, cy, card_w, card_h),
                    ldp_core::geometry::Transform::Normal,
                    ldp_core::color::ColorDescription::srgb_sdr(),
                    1.0,
                    Region::from_rect(Rect::new(0, 0, card_w, card_h)),
                );
                layer.style = card_style;
                cards.push(layer);
            }
            let (dock_data, dock_geometry) = translucent_argb_buffer(dock_w, dock_h, 190);
            let dock_view =
                ldp_renderer::BufferView::new(7, &dock_data, dock_geometry).expect("view");
            let mut dock = ldp_renderer::SurfaceLayer::new(
                dock_view,
                Rect::new(
                    (w - dock_w) as i32 / 2,
                    h as i32 - dock_h as i32 - (h as i32 / 45),
                    dock_w,
                    dock_h,
                ),
                ldp_core::geometry::Transform::Normal,
                ldp_core::color::ColorDescription::srgb_sdr(),
                1.0,
                Region::from_rect(Rect::new(0, 0, dock_w, dock_h)),
            );
            dock.style = dock_style;

            let mut layers: Vec<ldp_renderer::SurfaceLayer<'_>> = Vec::with_capacity(6);
            layers.push(paper.clone());
            layers.extend(cards.iter().cloned());
            layers.push(dock);

            // Steady state: one persistent renderer — the 60 Hz
            // reality after the first frame.
            let mut steady = ldp_renderer::SoftwareRenderer::new();
            suite.add(bench(
                &format!(
                    "renderer: {w}x{h} Liquid desktop frame (wallpaper + 4 rounded windows + frosted dock, steady state, {tier_name})"
                ),
                1,
                runs,
                1,
                || {
                    use ldp_renderer::Renderer as _;
                    steady.begin_frame(&output, &damage).expect("begin");
                    let stats = steady.submit(&layers).expect("submit");
                    steady.end_frame().expect("end");
                    assert_eq!(stats.layers, 6);
                    assert_eq!(stats.layers_rendered, 6);
                },
            ));

            // First frame at the matrix's two ends (FHD and 4K): the
            // cold material generation a modeset or a window map pays.
            if first_frames && (w == 1920 || w == 3840) && tier == EffectTier::High {
                suite.add(bench(
                    &format!(
                        "renderer: {w}x{h} Liquid desktop frame (same scene, first frame, High tier)"
                    ),
                    1,
                    runs,
                    1,
                    || {
                        use ldp_renderer::Renderer as _;
                        let mut renderer = ldp_renderer::SoftwareRenderer::new();
                        renderer.begin_frame(&output, &damage).expect("begin");
                        let stats = renderer.submit(&layers).expect("submit");
                        renderer.end_frame().expect("end");
                        assert_eq!(stats.layers, 6);
                        assert_eq!(stats.layers_rendered, 6);
                    },
                ));
            }
        }
    }
}

/// The Phase 8 shape: one full 4K XRGB composition through the
/// word-copy path.
fn renderer_copy_row(suite: &mut BenchSuite, quick: bool) {
    let (runs, w, h) = if quick {
        (3, 1920u32, 1080u32)
    } else {
        (5, 3840u32, 2160u32)
    };
    let (data, geometry) =
        ldp_renderer::testkit::pattern_buffer(w, h, ldp_core::buffer::FourCC::XRGB8888);
    let output = ldp_renderer::OutputDesc::new(
        w,
        h,
        ldp_core::buffer::FourCC::XRGB8888,
        ldp_core::color::ColorDescription::srgb_sdr(),
    )
    .expect("output");
    let view = ldp_renderer::BufferView::new(1, &data, geometry.clone()).expect("view");
    let layer = ldp_renderer::SurfaceLayer::new(
        view,
        Rect::new(0, 0, w, h),
        ldp_core::geometry::Transform::Normal,
        ldp_core::color::ColorDescription::srgb_sdr(),
        1.0,
        Region::from_rect(Rect::new(0, 0, w, h)),
    );
    let damage = ldp_renderer::testkit::full_damage(&output);

    suite.add(bench(
        &format!("renderer: {w}x{h} opaque composite (full damage)"),
        1,
        runs,
        1,
        || {
            use ldp_renderer::Renderer as _;
            let mut renderer = ldp_renderer::SoftwareRenderer::new();
            renderer.begin_frame(&output, &damage).expect("begin");
            let stats = renderer
                .submit(std::slice::from_ref(&layer))
                .expect("submit");
            renderer.end_frame().expect("end");
            // Every run must be real work: the whole frame is opaque.
            assert_eq!(
                stats.pixels_opaque,
                u64::from(w) * u64::from(h),
                "the copy path must carry every pixel"
            );
        },
    ));

    // (a) The most common real compositing case: an ARGB8888 window whose
    // pixels are fully opaque (window interiors). Before Phase 22 this
    // took the per-pixel Alpha path; after, the premultiplied word copy.
}

fn renderer_argb_row(suite: &mut BenchSuite, quick: bool) {
    let (runs, w, h) = if quick {
        (3, 1920u32, 1080u32)
    } else {
        (5, 3840u32, 2160u32)
    };
    {
        let (argb_data, argb_geometry) = opaque_argb_buffer(w, h);
        let argb_output = ldp_renderer::OutputDesc::new(
            w,
            h,
            ldp_core::buffer::FourCC::ARGB8888,
            ldp_core::color::ColorDescription::srgb_sdr(),
        )
        .expect("output");
        let argb_view = ldp_renderer::BufferView::new(2, &argb_data, argb_geometry).expect("view");
        let argb_layer = ldp_renderer::SurfaceLayer::new(
            argb_view,
            Rect::new(0, 0, w, h),
            ldp_core::geometry::Transform::Normal,
            ldp_core::color::ColorDescription::srgb_sdr(),
            1.0,
            Region::from_rect(Rect::new(0, 0, w, h)),
        );
        let argb_damage = ldp_renderer::testkit::full_damage(&argb_output);
        suite.add(bench(
            &format!("renderer: {w}x{h} ARGB8888 composite (full damage, opaque pixels)"),
            1,
            runs,
            1,
            || {
                use ldp_renderer::Renderer as _;
                let mut renderer = ldp_renderer::SoftwareRenderer::new();
                renderer
                    .begin_frame(&argb_output, &argb_damage)
                    .expect("begin");
                let stats = renderer
                    .submit(std::slice::from_ref(&argb_layer))
                    .expect("submit");
                renderer.end_frame().expect("end");
                assert_eq!(
                    stats.pixels_opaque,
                    u64::from(w) * u64::from(h),
                    "every pixel of the opaque ARGB layer must be written"
                );
            },
        ));
    }
}

/// The desktop frame shape: twelve window-sized layers, thirty-two
/// scattered damage rectangles. Before Phase 22 every layer rescanned
/// every damage rect per covered row; after, a per-frame row index
/// serves each row in O(intervals).
fn renderer_desktop_row(suite: &mut BenchSuite, quick: bool) {
    let runs = if quick { 3 } else { 5 };
    {
        let (dw, dh) = if quick {
            (960u32, 540u32)
        } else {
            (1920u32, 1080u32)
        };
        let cols = 4u32;
        let rows_grid = 3u32;
        let (win_w, win_h) = (dw / (cols * 2), dh / (rows_grid * 2));
        let (data, geometry) =
            ldp_renderer::testkit::pattern_buffer(win_w, win_h, ldp_core::buffer::FourCC::XRGB8888);
        let desktop_output = ldp_renderer::OutputDesc::new(
            dw,
            dh,
            ldp_core::buffer::FourCC::XRGB8888,
            ldp_core::color::ColorDescription::srgb_sdr(),
        )
        .expect("output");
        let mut layers: Vec<ldp_renderer::SurfaceLayer<'_>> = Vec::with_capacity(12);
        for cy in 0..rows_grid {
            for cx in 0..cols {
                let view = ldp_renderer::BufferView::new(3, &data, geometry.clone()).expect("view");
                layers.push(ldp_renderer::SurfaceLayer::new(
                    view,
                    Rect::new(
                        (cx * dw / cols) as i32 + 16,
                        (cy * dh / rows_grid) as i32 + 16,
                        win_w,
                        win_h,
                    ),
                    ldp_core::geometry::Transform::Normal,
                    ldp_core::color::ColorDescription::srgb_sdr(),
                    1.0,
                    Region::from_rect(Rect::new(0, 0, win_w, win_h)),
                ));
            }
        }
        // 32 deterministic scattered damage rects (64x64), wrapped into
        // the output bounds.
        let mut rng = SplitMix64::new(0x5EED_0022);
        let mut damage = Region::new();
        for _ in 0..32 {
            let x = (rng.next_u64() % u64::from(dw - 64)) as u32;
            let y = (rng.next_u64() % u64::from(dh - 64)) as u32;
            damage.add(Rect::new(x as i32, y as i32, 64, 64));
        }
        suite.add(bench(
            &format!("renderer: {dw}x{dh} composite (12 windows, 32 damage rects)"),
            1,
            runs,
            1,
            || {
                use ldp_renderer::Renderer as _;
                let mut renderer = ldp_renderer::SoftwareRenderer::new();
                renderer
                    .begin_frame(&desktop_output, &damage)
                    .expect("begin");
                renderer.submit(&layers).expect("submit");
                renderer.end_frame().expect("end");
            },
        ));
    }
}

/// A deterministic translucent premultiplied ARGB8888 buffer (the
/// frosted-panel shape: content at partial alpha over whatever the
/// compositor puts beneath it).
// w/h/a/r/g/b are the pixel domain's names (the sibling helper's
// precedent, restated for this buffer's alpha dial).
#[allow(clippy::many_single_char_names)]
fn translucent_argb_buffer(
    w: u32,
    h: u32,
    alpha: u8,
) -> (Vec<u8>, ldp_core::buffer::BufferGeometry) {
    let geometry = ldp_renderer::testkit::geometry_for(w, h, ldp_core::buffer::FourCC::ARGB8888);
    let mut data = vec![0u8; geometry.spanned_bytes() as usize];
    let layout = geometry.planes()[0];
    let a = u32::from(alpha);
    for y in 0..h {
        for x in 0..w {
            let [r, g, b, _a] = ldp_renderer::testkit::pattern_rgba(x, y);
            let addr =
                layout.offset as usize + layout.stride as usize * y as usize + x as usize * 4;
            let prem = |c: u8| ((u32::from(c) * a + 127) / 255) as u8;
            data[addr] = prem(b);
            data[addr + 1] = prem(g);
            data[addr + 2] = prem(r);
            data[addr + 3] = alpha;
        }
    }
    (data, geometry)
}

/// The Liquid phone-frame shape (Phase 29): the low-end device this
/// project exists for — a portrait output, a plain wallpaper, four
/// rounded-and-shadowed app cards, and one frosted translucent panel —
/// every frame at full damage (the animated-wallpaper steady state,
/// where the styled path's per-frame cost is paid sixty times a
/// second).
///
/// Two rows pin the two ends: *steady state* (one persistent renderer —
/// the 60 Hz reality after the first frame) and *first frame* (a fresh
/// renderer per run — the cold material generation).
// The scene assembly is one honest block (the layers borrow the three
// backing buffers — splitting it would trade clarity for indirection).
#[allow(clippy::too_many_lines)]
fn renderer_styled_rows(suite: &mut BenchSuite, quick: bool) {
    use ldp_renderer::EffectTier;

    let (runs, w, h, card_w, card_h, panel_w, panel_h) = if quick {
        (3, 540u32, 1170u32, 240, 380, 480, 120)
    } else {
        (5, 1080u32, 2340u32, 480, 760, 960, 240)
    };

    // The scene: wallpaper (plain, X-family), four app cards (the
    // High opaque style — corners + panel shadow), one frosted panel
    // (the High translucent style — frost + corners + shadow).
    let (paper_data, paper_geometry) =
        ldp_renderer::testkit::pattern_buffer(w, h, ldp_core::buffer::FourCC::XRGB8888);
    let paper_view = ldp_renderer::BufferView::new(4, &paper_data, paper_geometry).expect("view");
    let mut paper = ldp_renderer::SurfaceLayer::new(
        paper_view,
        Rect::new(0, 0, w, h),
        ldp_core::geometry::Transform::Normal,
        ldp_core::color::ColorDescription::srgb_sdr(),
        1.0,
        Region::from_rect(Rect::new(0, 0, w, h)),
    );
    paper.style = ldp_renderer::LayerStyle::default();

    let card_style = EffectTier::High.opaque_style();
    let panel_style = EffectTier::High.translucent_style();

    let (card_data, card_geometry) = opaque_argb_buffer(card_w, card_h);
    let card_positions = [
        (48, 180),
        (w as i32 - 48 - card_w as i32, 180),
        (48, h as i32 - 180 - card_h as i32),
        (
            w as i32 - 48 - card_w as i32,
            h as i32 - 180 - card_h as i32,
        ),
    ];
    let mut cards: Vec<ldp_renderer::SurfaceLayer<'_>> = Vec::with_capacity(4);
    for (cx, cy) in card_positions {
        let mut layer = ldp_renderer::SurfaceLayer::new(
            ldp_renderer::BufferView::new(6, &card_data, card_geometry.clone()).expect("view"),
            Rect::new(cx, cy, card_w, card_h),
            ldp_core::geometry::Transform::Normal,
            ldp_core::color::ColorDescription::srgb_sdr(),
            1.0,
            Region::from_rect(Rect::new(0, 0, card_w, card_h)),
        );
        layer.style = card_style;
        cards.push(layer);
    }

    let (panel_data, panel_geometry) = translucent_argb_buffer(panel_w, panel_h, 190);
    let panel_view = ldp_renderer::BufferView::new(7, &panel_data, panel_geometry).expect("view");
    let mut panel = ldp_renderer::SurfaceLayer::new(
        panel_view,
        Rect::new(
            (w as i32 - panel_w as i32) / 2,
            h as i32 - panel_h as i32 - 96,
            panel_w,
            panel_h,
        ),
        ldp_core::geometry::Transform::Normal,
        ldp_core::color::ColorDescription::srgb_sdr(),
        1.0,
        Region::from_rect(Rect::new(0, 0, panel_w, panel_h)),
    );
    panel.style = panel_style;

    let mut layers: Vec<ldp_renderer::SurfaceLayer<'_>> = Vec::with_capacity(6);
    layers.push(paper);
    layers.extend(cards);
    layers.push(panel);

    let output = ldp_renderer::OutputDesc::new(
        w,
        h,
        ldp_core::buffer::FourCC::ARGB8888,
        ldp_core::color::ColorDescription::srgb_sdr(),
    )
    .expect("output");
    let damage = ldp_renderer::testkit::full_damage(&output);

    // Steady state: one renderer across every timed run — the 60 Hz
    // reality (materials warm, framebuffer persistent).
    let mut steady = ldp_renderer::SoftwareRenderer::new();
    suite.add(bench(
        &format!(
            "renderer: {w}x{h} Liquid phone frame (wallpaper + 4 rounded cards + frosted panel, steady state)"
        ),
        1,
        runs,
        1,
        || {
            use ldp_renderer::Renderer as _;
            steady.begin_frame(&output, &damage).expect("begin");
            let stats = steady.submit(&layers).expect("submit");
            steady.end_frame().expect("end");
            // Six layers of real work, every frame.
            assert_eq!(stats.layers, 6);
            assert_eq!(stats.layers_rendered, 6);
        },
    ));

    // First frame: a fresh renderer per run — the cold material
    // generation a window map or a modeset pays.
    suite.add(bench(
        &format!("renderer: {w}x{h} Liquid phone frame (same scene, first frame)"),
        1,
        runs,
        1,
        || {
            use ldp_renderer::Renderer as _;
            let mut renderer = ldp_renderer::SoftwareRenderer::new();
            renderer.begin_frame(&output, &damage).expect("begin");
            let stats = renderer.submit(&layers).expect("submit");
            renderer.end_frame().expect("end");
            assert_eq!(stats.layers, 6);
            assert_eq!(stats.layers_rendered, 6);
        },
    ));
}

/// A deterministic fully-opaque ARGB8888 buffer (premultiplied == straight
/// at alpha 255): the benchmark shape of a real window interior.
#[allow(clippy::many_single_char_names)] // w/h/r/g/b are the pixel domain's names
fn opaque_argb_buffer(w: u32, h: u32) -> (Vec<u8>, ldp_core::buffer::BufferGeometry) {
    let geometry = ldp_renderer::testkit::geometry_for(w, h, ldp_core::buffer::FourCC::ARGB8888);
    let mut data = vec![0u8; geometry.spanned_bytes() as usize];
    let planes = geometry.planes();
    let layout = planes[0];
    for y in 0..h {
        for x in 0..w {
            let [r, g, b, _a] = ldp_renderer::testkit::pattern_rgba(x, y);
            let addr =
                layout.offset as usize + layout.stride as usize * y as usize + x as usize * 4;
            data[addr] = b;
            data[addr + 1] = g;
            data[addr + 2] = r;
            data[addr + 3] = 0xFF;
        }
    }
    (data, geometry)
}

// ---- remote relay ---------------------------------------------------

/// The Phase 21 surfaces: envelope codec cost and pool-update shipping
/// bandwidth — the two numbers a remote deployment feels first.
fn remote_benchmarks(suite: &mut BenchSuite, quick: bool) {
    let (ops, runs) = if quick { (64, 3) } else { (512, 7) };
    let cap = ldp_remote::RemoteConfig::default().envelope_cap();

    // Envelope codec round-trip: encode + write + read + parse, the
    // per-envelope fixed cost on both gateway CPUs.
    let message = vec![0xA5u8; 1024];
    let env = ldp_remote::Envelope::data(&message);
    suite.add(bench(
        "remote: envelope codec round-trip (1 KiB DATA)",
        1,
        runs,
        ops,
        || {
            for _ in 0..ops {
                let mut wire = Vec::with_capacity(1024 + 16);
                env.write_to(&mut wire, cap).expect("encode");
                let mut cursor = std::io::Cursor::new(&wire);
                let back = ldp_remote::wire::read_envelope(&mut cursor, cap).expect("decode");
                assert_eq!(back.body, message);
            }
        },
    ));

    // Pool-update codec round-trip: the in-memory encode + parse cost
    // of one 64 KiB commit window — the CPU share of a remote frame,
    // before any socket touches it. (End-to-end network throughput is
    // the transport suite's domain.)
    let window = vec![0x5Au8; 64 * 1024];
    suite.add(bench_bytes(
        "remote: pool-update codec round-trip (64 KiB window, in-memory)",
        1,
        runs,
        ops,
        ops * (64 * 1024) as u64,
        || {
            for _ in 0..ops {
                let env = ldp_remote::Envelope::pool_update(7, 4096, &window);
                let mut wire = Vec::with_capacity(window.len() + 36);
                env.write_to(&mut wire, cap).expect("encode");
                let mut cursor = std::io::Cursor::new(&wire);
                let back = ldp_remote::wire::read_envelope(&mut cursor, cap).expect("decode");
                let (id, offset, bytes) =
                    ldp_remote::Envelope::parse_pool_update(&back.body).expect("parse");
                assert_eq!((id, offset), (7, 4096));
                assert_eq!(bytes.len(), window.len());
            }
        },
    ));
}

// ---- security: tokens -----------------------------------------------

fn token_benchmarks(suite: &mut BenchSuite, quick: bool) {
    let (ops, runs) = if quick { (64, 3) } else { (512, 7) };
    let grants = 64usize;
    let mut table = GrantTable::new();
    let mut seed = LcgSeed::new(0x600D);
    let scope = ScopeSet::single(Scope::Screenshot);
    let mut valid = [0u32; 8];
    for i in 0..grants {
        let words = table.mint(&mut seed, "app.bench", scope, None);
        if i + 1 == grants {
            valid = words;
        }
    }
    assert_eq!(table.len(), grants);
    let now = Mono::from_ns(123_456_789);

    suite.add(bench(
        "security: token submit — valid (64-entry table, constant-time walk)",
        1,
        runs,
        ops,
        || {
            for _ in 0..ops {
                let outcome = table.submit("app.bench", 7, valid, now);
                assert!(matches!(outcome, SubmitOutcome::Granted(_)));
            }
        },
    ));

    // A forged token: bytewise-unmatched words — the full walk with no
    // early exit, then denial.
    let forged = [0xDEAD_BEEFu32; 8];
    suite.add(bench(
        "security: token submit — forged (64-entry table)",
        1,
        runs,
        ops,
        || {
            for _ in 0..ops {
                let outcome = table.submit("app.bench", 7, forged, now);
                assert!(matches!(outcome, SubmitOutcome::Denied { .. }));
            }
        },
    ));

    // Replay across applications: a real token presented under the
    // wrong app id — matched bytes, denied anyway.
    suite.add(bench(
        "security: token submit — cross-app (real token, wrong app)",
        1,
        runs,
        ops,
        || {
            for _ in 0..ops {
                let outcome = table.submit("app.other", 7, valid, now);
                assert!(matches!(outcome, SubmitOutcome::Denied { .. }));
            }
        },
    ));
}

// ---- security: audit chain ------------------------------------------

fn audit_event(i: u64) -> AuditEvent {
    AuditEvent {
        ts_ns: i * 1_000_000,
        client: u32::try_from(i % 32).expect("small"),
        app_id: format!("app.{}", i % 8),
        scope: Some(Scope::AuditRead),
        action: if i % 2 == 0 {
            AuditAction::Grant
        } else {
            AuditAction::Deny
        },
        detail: format!("{{\"seq\":{i}}}"),
    }
}

fn audit_benchmarks(suite: &mut BenchSuite, quick: bool) {
    let (events, runs) = if quick { (128u64, 2) } else { (1024u64, 5) };
    suite.add(bench(
        "security: audit chain append (1024 events, fresh chain)",
        1,
        runs,
        events,
        || {
            let mut chain = AuditChain::new(4096);
            for i in 0..events {
                chain.append(audit_event(i));
            }
            assert_eq!(chain.records().len(), events as usize);
        },
    ));

    let (records, vruns) = if quick { (512u64, 2) } else { (4096u64, 5) };
    let mut full = AuditChain::new(8192);
    for i in 0..records {
        full.append(audit_event(i));
    }
    suite.add(bench(
        "security: audit chain verify (4096 records, full hash walk)",
        1,
        vruns,
        records,
        || {
            let result = full.verify();
            assert!(result.ok);
            assert_eq!(result.records, records);
        },
    ));
}

// ---- security: permission matrix ------------------------------------

fn matrix_benchmarks(suite: &mut BenchSuite, quick: bool) {
    let (ops, runs) = if quick { (16, 3) } else { (256, 7) };
    // A client holding half the scopes through grants, none in its
    // manifest: every decision must actually walk escalate semantics.
    let manifest = ScopeSet::NONE;
    let granted = {
        let mut set = ScopeSet::NONE;
        for scope in Scope::ALL.iter().step_by(2) {
            set = set.with(*scope);
        }
        set
    };
    suite.add(bench(
        "security: permission matrix decide (all 18 operations)",
        1,
        runs,
        ops * ALL_OPERATIONS.len() as u64,
        || {
            for _ in 0..ops {
                for op in ALL_OPERATIONS {
                    // black_box: the pure decision must not be elided.
                    let _ = std::hint::black_box(decide(*op, manifest, granted));
                }
            }
        },
    ));
}
