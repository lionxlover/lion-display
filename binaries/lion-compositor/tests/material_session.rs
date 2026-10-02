//! Phase 40's exit criterion (the native half): the material depth
//! served end to end through the real protocol — a popup wears the
//! **menu glass** (the vibrant frost: the backdrop's chroma boosted
//! past itself, the tint veil, and the luminous hairline tracing the
//! rounded silhouette), byte-pinned against the hand-computed integer
//! rules, with the wallpaper's material untouched beneath it.
//!
//! The expectations, worked out here from the crate's named rules:
//!
//! * the backdrop under the menu is the parent's cherry `(200, 60,
//!   60)` — BT.601 luma `(15400 + 9000 + 1740 + 128) >> 8` = 102;
//! * the menu material at `High` is `vibrant_light`: blur 22 × 3
//!   (a uniform input stays uniform under Clamp), saturation 383 —
//!   boost 128 — so each channel extends by `128/255` of its
//!   luma-distance rounded half away from zero: R `200 → 249`, G and
//!   B `60 → 39` (the boost arm, away from luma);
//! * the tint veil `[0xFA, 0xFA, 0xFF]` at 76 over that: the veiled
//!   material lands at `(250, 102, 103)` (R = `mul255(250,76)` +
//!   `mul255(249,179)` = 75 + 175) — the menu's interior;
//! * the hairline (white at 90) over the material at the straight
//!   edge: `(252, 156, 157)` (R = 90 + `mul255(250,165)` = 90 + 162)
//!   — the light catch;
//! * the parent's own interior keeps `(200, 60, 60)` — the panel
//!   material never disturbs the ink.

mod testbench;

use ldp_client::Proxy;
use ldp_core::geometry::Rect;
use ldp_core::wire::Value;
use testbench::*;

use lion_compositor::server::CompositorConfig;
use lion_compositor::shell::{DockMode, ShellConfig};

/// The XRGB8888 fourcc ("XR24").
const XR24: u32 = 0x3432_5258;
/// The ARGB8888 fourcc ("AR24") — the menu's translucent ink.
const AR24: u32 = 0x3432_5241;
/// The mock output: 1920x1080@60.
const OUT_W: usize = 1920;

/// One XRGB word.
fn xrgb(r: u8, g: u8, b: u8) -> u32 {
    u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b)
}

/// A memfd carrying `pixels` (the pool backing).
fn pool_bytes(pixels: &[u8]) -> std::os::fd::OwnedFd {
    use std::os::unix::fs::FileExt as _;
    let fd = lion_compositor::sys::memfd("material-pool").expect("memfd");
    {
        let file = std::fs::File::from(fd.try_clone().expect("clone"));
        file.write_at(pixels, 0).expect("write pool");
    }
    fd
}

/// One scanout pixel as an RGBA quadruple.
fn px(words: &[u32], x: usize, y: usize) -> [u32; 4] {
    let v = words[y * OUT_W + x];
    [
        (v >> 16) & 0xFF,
        (v >> 8) & 0xFF,
        v & 0xFF,
        (v >> 24) & 0xFF,
    ]
}

/// The material-depth config: the full Liquid tier (the GPU tier's
/// language — the vibrant frost and the hairline serve), the dock off
/// (a clean desktop: placement still runs, no chrome in the way).
fn material_config() -> CompositorConfig {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    CompositorConfig {
        shell: ShellConfig {
            dock: DockMode::Off,
            ..ShellConfig::default()
        },
        effects: ldp_renderer::EffectChoice::Tier(ldp_renderer::EffectTier::High),
        socket: format!("lion-material-{n}-{}", std::process::id()),
        ..CompositorConfig::default()
    }
}

/// `shell.get_toplevel(surface, client-decorations)`.
fn get_toplevel(client: &mut TestClient, shell: &Proxy, surface: &Proxy) -> Proxy {
    client
        .conn
        .create_object(
            shell,
            "get_toplevel",
            vec![Value::Object(Some(surface.id())), Value::Enum(2)],
        )
        .expect("get_toplevel")
}

/// `shell.get_popup(surface, parent, anchor_rect, anchor, gravity,
/// offset, constraints)` — the menu shape: anchored at the rect's
/// bottom-left, growing down-right, every strategy granted.
fn get_menu_popup(
    client: &mut TestClient,
    shell: &Proxy,
    surface: &Proxy,
    parent: &Proxy,
    anchor_rect: Rect,
) -> Proxy {
    client
        .conn
        .create_object(
            shell,
            "get_popup",
            vec![
                Value::Object(Some(surface.id())),
                Value::Object(Some(parent.id())),
                Value::Rect(anchor_rect),
                Value::Enum(7), // anchor: bottom_left
                Value::Enum(5), // gravity: bottom_right
                Value::Int32(0),
                Value::Int32(0),
                Value::Bitset(
                    ldp_core::bitset::Bitset128::single(0)
                        .with(1)
                        .with(2)
                        .with(3)
                        .with(4)
                        .with(5),
                ),
            ],
        )
        .expect("get_popup")
}

/// Attach + damage + commit, then pump until the compositor rendered.
fn present(tb: &Testbench, client: &mut TestClient, surface: &Proxy, buffer: &Proxy, rect: Rect) {
    let before = tb.frames();
    attach(client, surface, buffer);
    damage(client, surface, &[rect]);
    commit(client, surface, 1);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while tb.frames() <= before {
        assert!(
            deadline > std::time::Instant::now(),
            "the compositor never rendered the buffer"
        );
        client.sync();
    }
}

/// THE exit criterion: the menu glass through the real wire — the
/// popup's frame carries the vibrant frost and the hairline, the
/// wallpaper's material untouched beneath.
#[test]
#[allow(clippy::too_many_lines)] // one session, one narrative
fn the_menu_glass_serves_through_the_real_wire() {
    let tb = Testbench::start_with("material-depth", material_config());
    let mut client = TestClient::connect(&tb.addr);

    // ---- the parent: a 200x100 cherry window at cascade step 0 ----
    let shm = client.bind("ldp.core.shm");
    let compositor = client.bind("ldp.core.compositor");
    let shell = client.bind("ldp.shell.shell");
    client.bind("ldp.core.output");

    let parent_word = xrgb(200, 60, 60);
    let parent_pixels: Vec<u8> = std::iter::repeat(parent_word.to_le_bytes())
        .take(200 * 100)
        .flatten()
        .collect();
    let parent_pool = create_pool(
        &mut client,
        &shm,
        pool_bytes(&parent_pixels),
        (200 * 100 * 4) as i64,
    );
    let parent_buffer = create_buffer(&mut client, &parent_pool, 0, 200, 100, 800, XR24);
    let parent_surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("parent surface");
    let _parent_toplevel = get_toplevel(&mut client, &shell, &parent_surface);
    present(
        &tb,
        &mut client,
        &parent_surface,
        &parent_buffer,
        Rect::new(0, 0, 200, 100),
    );

    // ---- the menu: 60x28 of fully translucent ink, anchored at the
    // parent's (100, 40) +8x4 → the top-left at (100, 44) ----
    //
    // The ink is transparent so the frame's every visible word is the
    // *system's* material — the frost and the hairline alone.
    let menu_pixels: Vec<u8> = vec![0; 60 * 28 * 4];
    let menu_pool = create_pool(
        &mut client,
        &shm,
        pool_bytes(&menu_pixels),
        (60 * 28 * 4) as i64,
    );
    let menu_buffer = create_buffer(&mut client, &menu_pool, 0, 60, 28, 240, AR24);
    let menu_surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("menu surface");
    let menu = get_menu_popup(
        &mut client,
        &shell,
        &menu_surface,
        &parent_surface,
        Rect::new(100, 40, 8, 4),
    );

    // The attach solves and proposes; the ack references the serial.
    attach(&mut client, &menu_surface, &menu_buffer);
    client.sync();
    client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "configure" && r.target == menu.id().as_u32())
    });
    {
        let records = client
            .events
            .records
            .iter()
            .filter(|r| r.event == "configure" && r.target == menu.id().as_u32())
            .collect::<Vec<_>>();
        let last = records.last().expect("the configure arrived");
        match &last.args[..] {
            [Value::Uint32(serial), Value::Int32(x), Value::Int32(y), Value::Uint32(w), Value::Uint32(h)] =>
            {
                assert_eq!((*x, *y), (100, 44), "the anchor solve");
                assert_eq!((*w, *h), (60, 28), "the buffer's size");
                client
                    .conn
                    .send_request(&menu, "ack_configure", vec![Value::Uint32(*serial)])
                    .expect("ack_configure");
            }
            other => panic!("the configure carries serial + placement (got {other:?})"),
        }
    }

    // The commit lands the menu placed — and dressed in the glass.
    damage(&mut client, &menu_surface, &[Rect::new(0, 0, 60, 28)]);
    commit(&mut client, &menu_surface, 2);
    client.sync();
    client.wait_until(|_| tb.frames() >= 2);
    {
        let scanout = tb.scanout();
        // The menu's interior: the vibrant frost over cherry — the
        // boost arm (channels away from luma) under the veil.
        assert_eq!(
            px(&scanout, 130, 58),
            [250, 102, 103, 255],
            "the interior: the vibrant material, hand-computed"
        );
        // The straight left edge (row 58 sits between the corner
        // bands): the luminous hairline over the material.
        assert_eq!(
            px(&scanout, 100, 58),
            [252, 156, 157, 255],
            "the edge: the hairline's light catch, hand-computed"
        );
        // The straight bottom edge (column 130 sits between the
        // corner bands): the hairline again.
        assert_eq!(
            px(&scanout, 130, 71),
            [252, 156, 157, 255],
            "the bottom edge traces too"
        );
        // The parent's interior: the panel material never disturbs
        // the ink (far from every shadow's reach).
        assert_eq!(
            px(&scanout, 50, 30),
            [200, 60, 60, 255],
            "the wallpaper keeps its own pixels"
        );
    }

    // The steady state: re-presenting the same scene keeps the bytes
    // (the memos — frost, edge, shadow — are transparent).
    present(
        &tb,
        &mut client,
        &menu_surface,
        &menu_buffer,
        Rect::new(0, 0, 60, 28),
    );
    {
        let scanout = tb.scanout();
        assert_eq!(
            px(&scanout, 130, 58),
            [250, 102, 103, 255],
            "the steady state keeps the interior"
        );
        assert_eq!(
            px(&scanout, 100, 58),
            [252, 156, 157, 255],
            "the steady state keeps the hairline"
        );
    }

    destroy(&mut client, &menu_surface);
    destroy(&mut client, &parent_surface);
    let _ = &menu;
}

/// Phase 45's exit criterion (the per-surface material half): the
/// `toplevel.set_material` request served end to end through the real
/// wire — a translucent window that defaults to the sheet material
/// requests **vibrant dark** (the family's first call site for the
/// control-center glass), the repaint of the changed chrome is the
/// *server's* (no client commit involved), and `default` clears the
/// request restoring the sheet's bytes **exactly** — the A/B/A proof,
/// the byte-equality discipline the material family's own tests pin.
#[test]
#[allow(clippy::too_many_lines)] // one session, one narrative
fn the_requested_material_serves_through_the_real_wire() {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tb = Testbench::start_with(
        "material-request",
        CompositorConfig {
            shell: ShellConfig {
                dock: DockMode::Off,
                ..ShellConfig::default()
            },
            effects: ldp_renderer::EffectChoice::Tier(ldp_renderer::EffectTier::High),
            socket: format!("lion-material-req-{n}-{}", std::process::id()),
            ..CompositorConfig::default()
        },
    );
    let mut client = TestClient::connect(&tb.addr);

    // ---- the window: 200x100 of half-translucent cherry ink --------
    //
    // Translucent by construction (AR24 at alpha 200, premultiplied):
    // the server's own resolution dresses it in the *sheet* — the
    // frosted_light backdrop with the light veil. The frost shows
    // through the 78%-alpha ink, so the material is visible ink.
    let shm = client.bind("ldp.core.shm");
    let compositor = client.bind("ldp.core.compositor");
    let shell = client.bind("ldp.shell.shell");
    client.bind("ldp.core.output");

    let ink: u32 = (200u32 << 24) | premul(200, 60, 60, 200);
    let pixels: Vec<u8> = std::iter::repeat(ink.to_le_bytes())
        .take(200 * 100)
        .flatten()
        .collect();
    let pool = create_pool(
        &mut client,
        &shm,
        pool_bytes(&pixels),
        (200 * 100 * 4) as i64,
    );
    let buffer = create_buffer(&mut client, &pool, 0, 200, 100, 800, AR24);
    let surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("surface");
    let toplevel = get_toplevel(&mut client, &shell, &surface);
    present(
        &tb,
        &mut client,
        &surface,
        &buffer,
        Rect::new(0, 0, 200, 100),
    );

    // The baseline: the sheet material (the translucent default —
    // the server's own resolution rules).
    let baseline = tb.scanout();
    tb.world(|w| {
        assert!(
            w.scene.material_requests.is_empty(),
            "no material requested yet (the sheet is the default)"
        );
    });

    // ---- the request: vibrant_dark (wire value 5) ------------------
    client
        .conn
        .send_request(&toplevel, "set_material", vec![Value::Enum(5)])
        .expect("set_material");
    // The repaint of the changed chrome is the server's: the request
    // dirtied the scene, the next wake renders it — no client commit
    // rides this frame.
    let before = tb.frames();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while tb.frames() <= before {
        assert!(
            deadline > std::time::Instant::now(),
            "the material change never repainted"
        );
        client.sync();
    }
    let requested = tb.scanout();
    assert_ne!(
        requested, baseline,
        "the requested material changes the pixels"
    );
    // The dark veil's signature: the interior is *darker* under the
    // vibrant-dark tint ([0x25,0x25,0x2B] at 170) than the sheet's
    // light veil — the interior pixel's luma drops measurably.
    let interior = px(&requested, 100, 50);
    let baseline_px = px(&baseline, 100, 50);
    assert!(
        interior[0] < baseline_px[0] && interior[1] < baseline_px[1],
        "the vibrant-dark veil darkens the interior (got {interior:?} vs {baseline_px:?})"
    );
    // The scene carries the request (the style resolution's truth).
    tb.world(|w| {
        assert_eq!(
            w.scene.material_requests.values().collect::<Vec<_>>(),
            vec![&ldp_renderer::Material::VibrantDark],
            "the scene carries the requested material"
        );
    });

    // ---- the clear: default (wire value 1) --------------------------
    client
        .conn
        .send_request(&toplevel, "set_material", vec![Value::Enum(1)])
        .expect("set_material default");
    let before = tb.frames();
    while tb.frames() <= before {
        assert!(
            deadline > std::time::Instant::now(),
            "the clear never repainted"
        );
        client.sync();
    }
    // The A/B/A: the cleared state is byte-identical to the baseline
    // — the sheet's pixels restored exactly, the request leaving no
    // residue (the memoized materials transparent, the resolution
    // deterministic).
    let cleared = tb.scanout();
    assert_eq!(
        cleared, baseline,
        "the clear restores the sheet's bytes exactly"
    );
    tb.world(|w| {
        assert!(
            w.scene.material_requests.is_empty(),
            "the clear removed the request"
        );
    });

    destroy(&mut client, &surface);
    let _ = toplevel;
}

/// Premultiply one color at `alpha` (the AR24 wire convention).
fn premul(r: u8, g: u8, b: u8, alpha: u8) -> u32 {
    let a = u32::from(alpha);
    let (r, g, b) = (u32::from(r), u32::from(g), u32::from(b));
    (((r * a + 127) / 255) << 16) | (((g * a + 127) / 255) << 8) | ((b * a + 127) / 255)
}
