//! Phase 27's exit criterion: the Liquid visual language, end to end
//! through the real protocol — a client session whose translucent
//! surface comes out **frosted** and whose window casts a real shadow,
//! pixel-exact against reference math written out in this file (the
//! `golden_blend.rs` discipline), plus the cross-backend proof (the
//! whole styled compositor byte-equal under the injected reference GL
//! backend) and the plain-path regression gate (the default config
//! renders Phase 26 pixels).

mod testbench;

use std::io::Write;

use ldp_core::geometry::Rect;
use ldp_core::wire::{Primitive, Value};
use ldp_renderer::EffectTier;
use testbench::*;

/// The XRGB8888 fourcc ("XR24").
const XR24: u32 = 0x3432_5258;
/// The ARGB8888 fourcc ("AR24").
const AR24: u32 = 0x3432_5241;
/// The mock output: 1920x1080@60.
const OUT_W: usize = 1920;
const OUT_H: usize = 1080;

// ---- the reference math (re-derived, never imported) ---------------

/// The one rounding rule: `v * a / 255`, round half up.
fn mul255(v: u32, a: u32) -> u32 {
    (v * a + 127) / 255
}

/// `a + (b - a) * t / 255`.
fn lerp255(a: u32, b: u32, t: u32) -> u32 {
    (a * (255 - t) + b * t + 127) / 255
}

/// Premultiplied over of src onto dst at full opacity.
fn over(src: [u32; 4], dst: [u32; 4]) -> [u32; 4] {
    let a2 = src[3];
    let inv = 255 - a2;
    let oa = a2 + mul255(dst[3], inv);
    let mix = |s: u32, d: u32| (mul255(s, 255) + mul255(d, inv)).min(oa);
    [
        mix(src[0], dst[0]),
        mix(src[1], dst[1]),
        mix(src[2], dst[2]),
        oa,
    ]
}

/// The Low-tier frost material over an opaque backdrop pixel:
/// desaturate towards BT.601 luma (saturation kept 128), then the
/// light-gray tint veil at alpha 140.
fn low_frost_material(backdrop: [u32; 4]) -> [u32; 4] {
    let luma = (77 * backdrop[0] + 150 * backdrop[1] + 29 * backdrop[2] + 128) >> 8;
    let t = 255 - 128;
    let m = [
        lerp255(backdrop[0], luma, t),
        lerp255(backdrop[1], luma, t),
        lerp255(backdrop[2], luma, t),
        backdrop[3],
    ];
    let veil = [mul255(238, 140), mul255(238, 140), mul255(242, 140), 140];
    over(veil, m)
}

/// One pixel of the scanout as a straight RGBA quadruple.
fn px(words: &[u32], x: usize, y: usize) -> [u32; 4] {
    let v = words[y * OUT_W + x];
    [
        (v >> 16) & 0xFF,
        (v >> 8) & 0xFF,
        v & 0xFF,
        (v >> 24) & 0xFF,
    ]
}

// ---- the session rig -----------------------------------------------

/// A memfd pool with explicit content.
fn pool_bytes(pixels: &[u8]) -> std::os::fd::OwnedFd {
    let fd = lion_compositor::sys::memfd("styled-pixels").expect("memfd");
    let mut file = std::fs::File::from(fd);
    file.write_all(pixels).expect("write pixels");
    file.into()
}

/// An XRGB word (X garbage in the high byte — the sampler's trap).
fn xrgb(r: u8, g: u8, b: u8) -> u32 {
    u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b)
}

/// Unique socket names for explicit-config benches (the testbench's
/// own sequencer only tags its `start` path).
static CFG_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// A default config with a unique socket and an effects choice.
fn tiered_config(tier: ldp_renderer::EffectChoice) -> lion_compositor::server::CompositorConfig {
    let n = CFG_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    lion_compositor::server::CompositorConfig {
        effects: tier,
        socket: format!("lion-styled-{n}-{}", std::process::id()),
        ..lion_compositor::server::CompositorConfig::default()
    }
}

/// Bring up a compositor at an explicit effects tier.
fn start_tier(tag: &str, tier: EffectTier) -> Testbench {
    Testbench::start_with(tag, tiered_config(ldp_renderer::EffectChoice::Tier(tier)))
}

/// Attach + damage + commit a buffer to a surface, then pump until the
/// compositor has rendered one more frame (the flip lands on the next
/// message — the wake-point doctrine; a bare `committed` wait can race
/// the render when a flip is already in flight).
fn present(
    tb: &Testbench,
    client: &mut TestClient,
    surface: &ldp_client::Proxy,
    buffer: &ldp_client::Proxy,
    rect: Rect,
) {
    let before = tb.frames();
    attach(client, surface, buffer);
    damage(client, surface, &[rect]);
    commit(client, surface, 1);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while tb.frames() <= before {
        assert!(
            deadline > std::time::Instant::now(),
            "the compositor never rendered the presented buffer"
        );
        client.sync();
    }
}

/// Set the surface's opaque region to one rect.
fn set_opaque(client: &mut TestClient, surface: &ldp_client::Proxy, rect: Rect) {
    client
        .conn
        .send_request(
            surface,
            "set_opaque_region",
            vec![Value::Array {
                element: ldp_core::wire::ArgType::Rect,
                items: vec![Primitive::Rect(rect)].into(),
            }],
        )
        .expect("set_opaque_region");
}

/// THE session: a solid backdrop with a translucent 4x2 dock at the
/// layout origin — at Low tier the dock frosts (the veil material)
/// and casts a hard shadow four pixels below; at Minimal the dock is
/// a plain blend.
#[test]
fn the_dock_frosts_and_casts_its_shadow() {
    let tb = start_tier("styled-low", EffectTier::Low);
    let mut client = TestClient::connect(&tb.addr);

    // The backdrop: fullscreen solid (0, 120, 240), opaque.
    let shm = client.bind("ldp.core.shm");
    let backdrop_word = xrgb(0, 120, 240);
    let mut backdrop_bytes = Vec::new();
    for _ in 0..OUT_W * OUT_H {
        backdrop_bytes.extend_from_slice(&backdrop_word.to_le_bytes());
    }
    let bg_pool = create_pool(
        &mut client,
        &shm,
        pool_bytes(&backdrop_bytes),
        (OUT_W * OUT_H * 4) as i64,
    );
    let bg_buffer = create_buffer(&mut client, &bg_pool, 0, 1920, 1080, 7680, XR24);
    let compositor = client.bind("ldp.core.compositor");
    let bg_surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("backdrop surface");
    client.bind("ldp.core.output");
    set_opaque(&mut client, &bg_surface, Rect::new(0, 0, 1920, 1080));
    present(
        &tb,
        &mut client,
        &bg_surface,
        &bg_buffer,
        Rect::new(0, 0, 1920, 1080),
    );

    // The dock: 4x2 premultiplied white at alpha 90, translucent (no
    // opaque region).
    let dock_word = 0x5A5A_5A5Au32;
    let mut dock_bytes = Vec::new();
    for _ in 0..4 * 2 {
        dock_bytes.extend_from_slice(&dock_word.to_le_bytes());
    }
    let dock_pool = create_pool(&mut client, &shm, pool_bytes(&dock_bytes), 32);
    let dock_buffer = create_buffer(&mut client, &dock_pool, 0, 4, 2, 16, AR24);
    let dock_surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("dock surface");
    present(
        &tb,
        &mut client,
        &dock_surface,
        &dock_buffer,
        Rect::new(0, 0, 4, 2),
    );

    let scanout = tb.scanout();
    let backdrop = [0, 120, 240, 255];

    // The dock's center pixel (1,1): the frost material, then the ink
    // at alpha 90 over it.
    let material = low_frost_material(backdrop);
    let ink = [90, 90, 90, 90];
    let expected_center = over(ink, material);
    let got_center = px(&scanout, 1, 1);
    assert_eq!(got_center, expected_center, "the frosted dock center");
    // Sanity anchors from the reference math:
    assert_eq!(material[0], 153, "the veil's red hand value");
    assert_eq!(expected_center[0], 189, "the dock center's red hand value");

    // The hard shadow strip: (1,5) sits inside the shadow silhouette
    // (the dock offset four down), outside the ink — black at alpha 80
    // over the backdrop.
    let shadow = over([0, 0, 0, 80], backdrop);
    let got_shadow = px(&scanout, 1, 5);
    assert_eq!(got_shadow, shadow, "the shadow strip below the dock");
    assert_eq!(shadow, [0, 82, 165, 255], "the shadow's hand value");

    // Away from everything: the backdrop survives.
    assert_eq!(px(&scanout, 100, 100), backdrop);
}

/// The plain-path regression gate: the default config (Minimal)
/// renders the dock as a plain blend — no frost, no shadow.
#[test]
fn the_default_config_keeps_the_plain_pixels() {
    let tb = Testbench::start("styled-minimal");
    let mut client = TestClient::connect(&tb.addr);
    let shm = client.bind("ldp.core.shm");

    let backdrop_word = xrgb(0, 120, 240);
    let mut backdrop_bytes = Vec::new();
    for _ in 0..OUT_W * OUT_H {
        backdrop_bytes.extend_from_slice(&backdrop_word.to_le_bytes());
    }
    let bg_pool = create_pool(
        &mut client,
        &shm,
        pool_bytes(&backdrop_bytes),
        (OUT_W * OUT_H * 4) as i64,
    );
    let bg_buffer = create_buffer(&mut client, &bg_pool, 0, 1920, 1080, 7680, XR24);
    let compositor = client.bind("ldp.core.compositor");
    let bg_surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("backdrop surface");
    client.bind("ldp.core.output");
    set_opaque(&mut client, &bg_surface, Rect::new(0, 0, 1920, 1080));
    present(
        &tb,
        &mut client,
        &bg_surface,
        &bg_buffer,
        Rect::new(0, 0, 1920, 1080),
    );

    let dock_word = 0x5A5A_5A5Au32;
    let mut dock_bytes = Vec::new();
    for _ in 0..4 * 2 {
        dock_bytes.extend_from_slice(&dock_word.to_le_bytes());
    }
    let dock_pool = create_pool(&mut client, &shm, pool_bytes(&dock_bytes), 32);
    let dock_buffer = create_buffer(&mut client, &dock_pool, 0, 4, 2, 16, AR24);
    let dock_surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("dock surface");
    present(
        &tb,
        &mut client,
        &dock_surface,
        &dock_buffer,
        Rect::new(0, 0, 4, 2),
    );

    let scanout = tb.scanout();
    // The plain blend: ink over backdrop, nothing else.
    let expected = over([90, 90, 90, 90], [0, 120, 240, 255]);
    assert_eq!(px(&scanout, 1, 1), expected, "the plain dock center");
    // No shadow below: the backdrop survives untouched.
    assert_eq!(
        px(&scanout, 1, 5),
        [0, 120, 240, 255],
        "no shadow at Minimal"
    );
}

/// The tier resolves from the config and reports honestly.
#[test]
fn the_tier_reports_from_bring_up() {
    let tb = start_tier("styled-report", EffectTier::High);
    let report = tb.world(lion_compositor::World::effects_decision_report);
    assert!(
        report.starts_with("high ("),
        "the report names the tier: {report}"
    );
    assert!(
        report.contains("3-pass blur"),
        "and what it serves: {report}"
    );
    // The auto doctrine on the headless mock path: Minimal (the CI
    // doctrine — deterministic plain pixels) unless asked.
    let config = tiered_config(ldp_renderer::EffectChoice::Auto);
    let tb = Testbench::start_with("styled-auto", config);
    let tier = tb.world(|w| w.effects);
    assert_eq!(tier, EffectTier::Minimal, "auto on the mock stays plain");
}

/// The cross-backend proof at the compositor level: the whole styled
/// session — frosted dock, rounded window, shadows — renders
/// byte-equal under the software backend and the injected reference
/// GL backend (the Phase 24 hardware doctrine extended to Phase 27).
#[test]
#[allow(clippy::too_many_lines)] // one session, one narrative: the rig reads best unsplit
fn the_styled_compositor_is_byte_equal_under_the_reference_gl_backend() {
    fn run(gl: bool) -> Vec<u32> {
        let config = lion_compositor::server::CompositorConfig {
            renderer: ldp_renderer::gles::RendererChoice::Software,
            renderer_api: gl.then(|| {
                Box::new(ldp_renderer::gles::RefGles::new())
                    as Box<dyn ldp_renderer::gles::api::GlesApi>
            }),
            ..tiered_config(ldp_renderer::EffectChoice::Tier(EffectTier::High))
        };
        let tb = Testbench::start_with("styled-gl", config);
        let mut client = TestClient::connect(&tb.addr);
        let shm = client.bind("ldp.core.shm");

        // The backdrop.
        let backdrop_word = xrgb(120, 40, 200);
        let mut backdrop_bytes = Vec::new();
        for _ in 0..OUT_W * OUT_H {
            backdrop_bytes.extend_from_slice(&backdrop_word.to_le_bytes());
        }
        let bg_pool = create_pool(
            &mut client,
            &shm,
            pool_bytes(&backdrop_bytes),
            (OUT_W * OUT_H * 4) as i64,
        );
        let bg_buffer = create_buffer(&mut client, &bg_pool, 0, 1920, 1080, 7680, XR24);
        let compositor = client.bind("ldp.core.compositor");
        let bg_surface = client
            .conn
            .create_object(&compositor, "create_surface", vec![])
            .expect("backdrop surface");
        client.bind("ldp.core.output");
        set_opaque(&mut client, &bg_surface, Rect::new(0, 0, 1920, 1080));
        present(
            &tb,
            &mut client,
            &bg_surface,
            &bg_buffer,
            Rect::new(0, 0, 1920, 1080),
        );

        // A translucent 40x20 dock: frosted at High tier.
        let mut dock_bytes = Vec::new();
        for _ in 0..40 * 20 {
            dock_bytes.extend_from_slice(&0x7878_7878u32.to_le_bytes());
        }
        let dock_pool = create_pool(&mut client, &shm, pool_bytes(&dock_bytes), 3200);
        let dock_buffer = create_buffer(&mut client, &dock_pool, 0, 40, 20, 160, AR24);
        let dock_surface = client
            .conn
            .create_object(&compositor, "create_surface", vec![])
            .expect("dock surface");
        present(
            &tb,
            &mut client,
            &dock_surface,
            &dock_buffer,
            Rect::new(0, 0, 40, 20),
        );

        // An opaque 32x16 window over the dock: corners and a soft
        // shadow at High tier.
        let mut win_bytes = Vec::new();
        for _ in 0..32 * 16 {
            win_bytes.extend_from_slice(&xrgb(250, 250, 250).to_le_bytes());
        }
        let win_pool = create_pool(&mut client, &shm, pool_bytes(&win_bytes), 2048);
        let win_buffer = create_buffer(&mut client, &win_pool, 0, 32, 16, 128, XR24);
        let win_surface = client
            .conn
            .create_object(&compositor, "create_surface", vec![])
            .expect("window surface");
        set_opaque(&mut client, &win_surface, Rect::new(0, 0, 32, 16));
        present(
            &tb,
            &mut client,
            &win_surface,
            &win_buffer,
            Rect::new(0, 0, 32, 16),
        );

        tb.scanout()
    }
    let software = run(false);
    let gl = run(true);
    // Compare compactly: a 2M-element assert_eq would dump the whole
    // frame on failure (47 MB of noise).
    let mut diffs = 0usize;
    let mut first: Option<(usize, u32, u32)> = None;
    for (i, (a, b)) in software.iter().zip(gl.iter()).enumerate() {
        if a != b {
            diffs += 1;
            if first.is_none() {
                first = Some((i, *a, *b));
            }
        }
    }
    assert!(
        diffs == 0,
        "the styled compositor diverged: {diffs} pixels, first at {:?} (x={}, y={})",
        first,
        first.map_or(0, |(i, _, _)| i % 1920),
        first.map_or(0, |(i, _, _)| i / 1920)
    );
    // And the frame is really styled: the window's corner cutout shows
    // the frosted dock beneath (not the window color).
    let corner_outside_ink = px(&software, 0, 0);
    assert_ne!(
        corner_outside_ink,
        [250, 250, 250, 255],
        "the rounded corner clipped the window's ink"
    );
    // The soft shadow below the window is really there.
    let below = px(&software, 16, 30);
    assert_ne!(
        below,
        px(&software, 400, 600),
        "the shadow's darkening below the window is visible"
    );
}
