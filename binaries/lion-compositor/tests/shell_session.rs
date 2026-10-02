//! Phase 28's exit criterion: the positioning shell, end to end
//! through the real protocol — windows land **placed** (the desktop
//! cascade, the phone's usable-origin anchor), the **system dock**
//! renders at the screen edge (frosted by the material language over
//! whatever sits beneath, rising on the spring and settling), the
//! legacy default keeps the Phase 26/27 pixels byte-exact, a
//! portrait swap re-resolves the whole doctrine mid-session, and the
//! whole shell session renders byte-equal under the injected
//! reference GL backend.

mod testbench;

use std::io::Write;

use ldp_core::geometry::Rect;
use ldp_core::wire::{Primitive, Value};
use ldp_renderer::EffectTier;
use testbench::*;

use lion_compositor::rearrange::Rearrange;
use lion_compositor::shell::{DockMode, ShellConfig};
use lion_compositor::World;

/// The XRGB8888 fourcc ("XR24").
const XR24: u32 = 0x3432_5258;
/// The ARGB8888 fourcc ("AR24").
const AR24: u32 = 0x3432_5241;
/// The mock output: 1920x1080@60.
const OUT_W: usize = 1920;
const OUT_H: usize = 1080;
/// The system dock's thickness (the phone home bar).
const DOCK: u32 = 84;
/// The portrait phone output: 540x960@60.
const PHONE_W: usize = 540;
const PHONE_H: usize = 960;

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

/// The dock's haze ink (premultiplied white at alpha 60 — the word
/// 0x3C3C3C3C).
const HAZE_INK: [u32; 4] = [60, 60, 60, 60];

/// The dock's pill ink (premultiplied [225, 228, 235] at alpha 235).
fn pill_ink() -> [u32; 4] {
    [mul255(225, 235), mul255(228, 235), mul255(235, 235), 235]
}

/// One pixel of a 1920-wide scanout as a straight RGBA quadruple.
fn px(words: &[u32], x: usize, y: usize) -> [u32; 4] {
    let v = words[y * OUT_W + x];
    [
        (v >> 16) & 0xFF,
        (v >> 8) & 0xFF,
        v & 0xFF,
        (v >> 24) & 0xFF,
    ]
}

/// One pixel of a 540-wide (phone) scanout.
fn ppx(words: &[u32], x: usize, y: usize) -> [u32; 4] {
    let v = words[y * PHONE_W + x];
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
    let fd = lion_compositor::sys::memfd("shell-pixels").expect("memfd");
    let mut file = std::fs::File::from(fd);
    file.write_all(pixels).expect("write pixels");
    file.into()
}

/// An XRGB word (X garbage in the high byte — the sampler's trap).
fn xrgb(r: u8, g: u8, b: u8) -> u32 {
    u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b)
}

/// Unique socket names for explicit-config benches.
static CFG_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// A config with a unique socket, an effects tier, and the shell's
/// dock mode.
fn shell_config(
    tier: ldp_renderer::EffectChoice,
    dock: DockMode,
) -> lion_compositor::server::CompositorConfig {
    let n = CFG_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    lion_compositor::server::CompositorConfig {
        effects: tier,
        shell: ShellConfig {
            dock,
            dock_thickness: DOCK,
        },
        socket: format!("lion-shell-{n}-{}", std::process::id()),
        ..lion_compositor::server::CompositorConfig::default()
    }
}

/// Bring up a compositor at an explicit tier with the shell active.
fn start_shell(tag: &str, tier: EffectTier) -> Testbench {
    Testbench::start_with(
        tag,
        shell_config(ldp_renderer::EffectChoice::Tier(tier), DockMode::Auto),
    )
}

/// Attach + damage + commit a buffer to a surface, then pump until the
/// compositor has rendered one more frame (the wake-point doctrine).
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

/// Drive the session until the dock's rise settles (~16 frames of
/// steps clear the strict velocity threshold; 24 is margin).
fn settle_the_rise(
    tb: &Testbench,
    client: &mut TestClient,
    surface: &ldp_client::Proxy,
    buffer: &ldp_client::Proxy,
    rect: Rect,
) {
    for _ in 0..24 {
        present(tb, client, surface, buffer, rect);
    }
    tb.world(|w| {
        assert!(
            w.shell.dock.as_ref().is_some_and(|d| d.rise.settled()),
            "the rise settled after 24 frames"
        );
    });
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

/// THE exit criterion (the desktop half): the second window lands one
/// cascade step in — never at the origin — and the system dock frosts
/// at the bottom edge over the wallpaper, pixel-exact against the
/// reference math, its haze and its pills, after the intro rise
/// settles.
#[test]
#[allow(clippy::too_many_lines)] // one session, one narrative: the rig reads best unsplit
fn the_desktop_cascades_and_the_dock_frosts_at_the_screen_edge() {
    let tb = start_shell("shell-desktop", EffectTier::Low);
    let mut client = TestClient::connect(&tb.addr);

    // The shell resolved for the 1920x1080 mock: a desktop with the
    // dock carved out of the bottom.
    tb.world(|w| {
        assert_eq!(
            w.shell_decision_report(),
            "desktop layout, dock 84 px at the bottom"
        );
        assert_eq!(w.shell.layout.usable, Rect::new(0, 0, 1920, 996));
    });

    // The wallpaper: fullscreen solid (0, 120, 240), opaque — the
    // first root, cascade step 0: the origin.
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
        .expect("wallpaper surface");
    client.bind("ldp.core.output");
    set_opaque(&mut client, &bg_surface, Rect::new(0, 0, 1920, 1080));
    present(
        &tb,
        &mut client,
        &bg_surface,
        &bg_buffer,
        Rect::new(0, 0, 1920, 1080),
    );

    // The window: 32x16 opaque white — the second root, cascade step
    // 1: (24, 24), never the origin.
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

    // Mid-rise: the dock has stepped once (offset ~37 px) — its top
    // edge sits ~1033, so (960, 1000) is still the wallpaper and the
    // strip near the bottom edge already shows the haze.
    {
        let scanout = tb.scanout();
        assert_eq!(
            px(&scanout, 960, 1000),
            [0, 120, 240, 255],
            "mid-rise: the dock has not reached y=1000 yet"
        );
        assert_eq!(
            px(&scanout, 960, 1070),
            over(HAZE_INK, low_frost_material([0, 120, 240, 255])),
            "mid-rise: the visible bottom strip is already the dock"
        );
    }

    // Let the rise settle, then the resting truth.
    settle_the_rise(
        &tb,
        &mut client,
        &win_surface,
        &win_buffer,
        Rect::new(0, 0, 32, 16),
    );
    let scanout = tb.scanout();
    let backdrop = [0, 120, 240, 255];

    // The cascade: the window's center is white at (24+16, 24+8) —
    // and the ORIGIN shows the wallpaper (legacy placement would have
    // put the window there).
    assert_eq!(
        px(&scanout, 40, 32),
        [250, 250, 250, 255],
        "the window's center"
    );
    assert_eq!(
        px(&scanout, 1, 1),
        backdrop,
        "the origin stays the wallpaper"
    );

    // The hard shadow strip below the cascaded window: (40, 42) sits
    // inside the silhouette (offset four down), outside the ink.
    let shadow = over([0, 0, 0, 80], backdrop);
    assert_eq!(
        px(&scanout, 40, 42),
        shadow,
        "the shadow strip below the window"
    );
    assert_eq!(shadow, [0, 82, 165, 255], "the shadow's hand value");

    // The dock at rest: the frost material over the wallpaper, then
    // the haze. x=960 falls in the pill row's gap.
    let material = low_frost_material(backdrop);
    let expected_haze = over(HAZE_INK, material);
    assert_eq!(
        px(&scanout, 960, 1038),
        expected_haze,
        "the dock's haze over the frost"
    );
    assert_eq!(
        material,
        [153, 180, 209, 255],
        "the frost material's hand value"
    );
    assert_eq!(
        expected_haze,
        [177, 198, 220, 255],
        "the haze pixel's hand value"
    );

    // The pill: [225, 228, 235] at alpha 235 over the frost. The
    // pill row at 1920 wide: pills of 456 px, the third spans
    // x∈[968..1424]; its center is (1196, 1038).
    let expected_pill = over(pill_ink(), material);
    assert_eq!(
        px(&scanout, 1196, 1038),
        expected_pill,
        "the dock's pill over the frost"
    );
    assert_eq!(
        expected_pill,
        [219, 224, 233, 255],
        "the pill pixel's hand value"
    );

    // The dock's rounded corner: (1, 997) — inside the 12 px corner
    // radius — folds coverage into the source alpha, so it is NOT
    // the full haze pixel.
    let corner = px(&scanout, 1, 997);
    assert_ne!(corner, expected_haze, "the corner coverage folds");
    // Away from everything: the wallpaper survives.
    assert_eq!(px(&scanout, 100, 500), backdrop);
}

/// The legacy regression gate: the default config (dock off) keeps the
/// Phase 27 pixels — the second root lands at its creation position
/// (the origin, no cascade), the client-side dock frosts exactly as
/// Phase 27 pinned it, and nothing renders at the screen's bottom
/// edge.
#[test]
fn the_dock_off_config_keeps_the_legacy_pixels() {
    let tb = Testbench::start_with(
        "shell-legacy",
        shell_config(
            ldp_renderer::EffectChoice::Tier(EffectTier::Low),
            DockMode::Off,
        ),
    );
    tb.world(|w| {
        assert_eq!(
            w.shell_decision_report(),
            "legacy (origin placement, no dock)"
        );
    });
    let mut client = TestClient::connect(&tb.addr);

    // The Phase 27 session verbatim: a fullscreen backdrop and a
    // translucent 4x2 client dock.
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
    let backdrop = [0, 120, 240, 255];
    // The Phase 27 truth, unchanged: the client dock frosts at the
    // ORIGIN (no cascade moved it).
    let material = low_frost_material(backdrop);
    let expected_center = over([90, 90, 90, 90], material);
    assert_eq!(
        px(&scanout, 1, 1),
        expected_center,
        "the frosted client dock center"
    );
    assert_eq!(expected_center[0], 189, "the Phase 27 hand value holds");
    let shadow = over([0, 0, 0, 80], backdrop);
    assert_eq!(px(&scanout, 1, 5), shadow, "the client dock's shadow strip");
    // No system dock at the screen edge: the wallpaper survives.
    assert_eq!(px(&scanout, 960, 1038), backdrop, "no system dock renders");
}

/// The dock renders plain at Minimal (no frost — the haze and the
/// pills blend directly) while the placement still cascades: the
/// shell and the effects tier are orthogonal doctrines.
#[test]
fn the_dock_renders_plain_at_minimal() {
    let tb = start_shell("shell-minimal", EffectTier::Minimal);
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
    settle_the_rise(
        &tb,
        &mut client,
        &win_surface,
        &win_buffer,
        Rect::new(0, 0, 32, 16),
    );

    let scanout = tb.scanout();
    let backdrop = [0, 120, 240, 255];
    // The cascade still runs at Minimal: the window at (24, 24), the
    // origin stays the wallpaper — and no corners, no shadow (the
    // plain path).
    assert_eq!(
        px(&scanout, 25, 25),
        [250, 250, 250, 255],
        "the window, placed"
    );
    assert_eq!(
        px(&scanout, 1, 1),
        backdrop,
        "the origin stays the wallpaper"
    );
    assert_eq!(px(&scanout, 40, 42), backdrop, "no shadow at Minimal");
    // The dock, plain: the haze and the pills blend directly over the
    // wallpaper (no frost material).
    assert_eq!(
        px(&scanout, 960, 1038),
        over(HAZE_INK, backdrop),
        "the plain haze over the wallpaper"
    );
    assert_eq!(
        px(&scanout, 1196, 1038),
        over(pill_ink(), backdrop),
        "the plain pill over the wallpaper"
    );
}

/// The phone doctrine, live: a portrait connector swaps in mid-session
/// and the whole shell re-resolves — the layout flips to the phone
/// class, the dock re-forms at the new width, the mapped roots
/// re-anchor at the new usable origin, and the wallpaper runs *under*
/// the frosted dock (chrome above client content, overflow occluded
/// never destroyed).
#[test]
#[allow(clippy::too_many_lines)] // the migration choreography is one narrative
fn the_phone_re_anchors_on_a_portrait_swap() {
    const EDP: u32 = 91;
    const HDMI: u32 = 93;
    let tb = start_shell("shell-phone", EffectTier::Low);
    let mut client = TestClient::connect(&tb.addr);
    let shm = client.bind("ldp.core.shm");

    // The desktop session: a fullscreen wallpaper and a window.
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
        .expect("wallpaper surface");
    client.bind("ldp.core.output");
    set_opaque(&mut client, &bg_surface, Rect::new(0, 0, 1920, 1080));
    present(
        &tb,
        &mut client,
        &bg_surface,
        &bg_buffer,
        Rect::new(0, 0, 1920, 1080),
    );

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

    // The swap: a portrait phone panel arrives, the laptop panel
    // dies. The pipeline migrates; the shell re-resolves.
    {
        let mut world = tb.shared.world.lock().expect("world lock");
        world
            .device
            .as_mock_mut()
            .expect("the mock driver")
            .hotplug_connect(
                ldp_display::ids::ConnectorId::new(HDMI).expect("preset connector id"),
                vec![phone_mode()],
                vec![0xA5u8; 16],
            );
        world
            .device
            .as_mock_mut()
            .expect("the mock driver")
            .hotplug_disconnect(
                ldp_display::ids::ConnectorId::new(EDP).expect("preset connector id"),
            );
    }
    {
        let mut world = tb.shared.world.lock().expect("world lock");
        let summary = world.service_device_events().expect("the service pass");
        match summary.expect("a hotplug event was queued") {
            Rearrange::Migrated { to, .. } => {
                assert_eq!((to.width, to.height), (540, 960));
            }
            other => panic!("expected a migration, got {other:?}"),
        }
    }
    // The shell flipped to the phone doctrine.
    tb.world(|w| {
        assert_eq!(w.shell.layout.class, ldp_shell::DeviceClass::Phone);
        assert_eq!(w.shell.layout.usable, Rect::new(0, 0, 540, 876));
        assert_eq!(w.shell.dock_rect(), Some(Rect::new(0, 876, 540, 84)));
        assert_eq!(
            w.shell_decision_report(),
            "phone layout, dock 84 px at the bottom"
        );
    });

    // The client learned (revoked) and re-binds.
    client.wait_until(|c| c.records.iter().any(|r| r.event == "revoked"));
    let _output2 = client.bind("ldp.core.output");
    client.wait_until(|c| c.records.iter().filter(|r| r.event == "name").count() >= 2);

    // The desktop paints again on the phone panel, and the fresh
    // dock's rise settles.
    settle_the_rise(
        &tb,
        &mut client,
        &win_surface,
        &win_buffer,
        Rect::new(0, 0, 32, 16),
    );
    let scanout = tb.scanout();
    assert_eq!(scanout.len(), PHONE_W * PHONE_H, "the phone-shaped scanout");
    let backdrop = [0, 120, 240, 255];

    // The window re-anchored at the phone's usable origin (0, 0) —
    // not the desktop cascade step it held before the swap.
    assert_eq!(
        ppx(&scanout, 16, 8),
        [250, 250, 250, 255],
        "the re-anchored window"
    );
    assert_eq!(
        ppx(&scanout, 25, 25),
        backdrop,
        "the old cascade step is wallpaper"
    );
    // Its shadow still casts below (the Low-tier doctrine).
    let shadow = over([0, 0, 0, 80], backdrop);
    assert_eq!(
        ppx(&scanout, 16, 18),
        shadow,
        "the shadow below the re-anchored window"
    );

    // The dock at the phone's bottom edge: the frost over the
    // wallpaper, the haze, the pills. The pill row at 540 wide: pills
    // of 111 px, the first spans x∈[24..135]; its center is (79, 918).
    let material = low_frost_material(backdrop);
    assert_eq!(
        ppx(&scanout, 270, 918),
        over(HAZE_INK, material),
        "the phone dock's haze over the frost"
    );
    assert_eq!(
        ppx(&scanout, 79, 918),
        over(pill_ink(), material),
        "the phone dock's pill over the frost"
    );
    // The wallpaper ran UNDER the dock (the z-order doctrine): the
    // strip is chrome, and above it the wallpaper survives.
    assert_eq!(
        ppx(&scanout, 300, 800),
        backdrop,
        "the wallpaper above the dock"
    );
}

/// A 540x960@60 portrait mode (the phone panel).
fn phone_mode() -> ldp_display::mode::Mode {
    ldp_display::mode::Mode::new(
        42_000,
        540,
        560,
        580,
        700,
        960,
        970,
        975,
        1000,
        ldp_display::mode::ModeFlags::PVSYNC | ldp_display::mode::ModeFlags::NHSYNC,
        ldp_display::mode::ModeType::PREFERRED | ldp_display::mode::ModeType::DRIVER,
    )
}

/// The cross-backend proof: the whole shell session — the cascaded
/// window with its shadow, the risen frosted dock with its pills —
/// renders byte-equal under the software backend and the injected
/// reference GL backend.
#[test]
#[allow(clippy::too_many_lines)] // one session, one narrative
fn the_shell_session_is_byte_equal_under_the_reference_gl_backend() {
    fn run(gl: bool) -> Vec<u32> {
        let config = lion_compositor::server::CompositorConfig {
            renderer: ldp_renderer::gles::RendererChoice::Software,
            renderer_api: gl.then(|| {
                Box::new(ldp_renderer::gles::RefGles::new())
                    as Box<dyn ldp_renderer::gles::api::GlesApi>
            }),
            ..shell_config(
                ldp_renderer::EffectChoice::Tier(EffectTier::High),
                DockMode::Auto,
            )
        };
        let tb = Testbench::start_with("shell-gl", config);
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

        // An opaque 32x16 window: the cascade's second step, corners
        // and a soft shadow at High tier.
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
        settle_the_rise(
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
        "the shell compositor diverged: {diffs} pixels, first at {:?} (x={}, y={})",
        first,
        first.map_or(0, |(i, _, _)| i % 1920),
        first.map_or(0, |(i, _, _)| i / 1920)
    );
    // And the frame is really the shell's: the dock's haze differs
    // from the plain backdrop beneath it.
    let software_px = px(&software, 960, 1038);
    let backdrop = [120, 40, 200, 255];
    assert_ne!(software_px, backdrop, "the dock renders at High tier");
    // The window's rounded corner clipped its ink.
    assert_ne!(
        px(&software, 24, 24),
        [250, 250, 250, 255],
        "the corner cutout"
    );
    // The soft shadow below the window is really there.
    assert_ne!(px(&software, 40, 44), px(&software, 400, 600), "the shadow");
}

/// The report lines are honest at both settings.
#[test]
fn the_shell_reports_from_bring_up() {
    let tb = start_shell("shell-report", EffectTier::High);
    let report = tb.world(World::shell_decision_report);
    assert_eq!(report, "desktop layout, dock 84 px at the bottom");
    let tb = Testbench::start_with(
        "shell-report-off",
        shell_config(ldp_renderer::EffectChoice::Auto, DockMode::Off),
    );
    let report = tb.world(World::shell_decision_report);
    assert_eq!(report, "legacy (origin placement, no dock)");
}
