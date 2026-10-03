//! Phase 56's exit criterion (the chrome-aware placement): the
//! placement engine answers in **frame space** over the real socket —
//!
//! * **the parked window's band**: a server-decorated toplevel
//!   parked at the usable origin (Fill's anchor, Cascade's first
//!   slots) wears its top band *on the screen* — the frame takes the
//!   policy slot, the content rides inside at the slot plus the
//!   applied insets, and the band's ink lands from row 0 of the
//!   display (the pre-Phase-56 truth — the content at the origin,
//!   the band above the visible area — refuted pixel by pixel);
//! * **the cascade steps frames**: window *n*'s frame sits at the
//!   cascade's *n*th slot — the captions step the way every desktop
//!   a user has ever used steps them (DWM's work area answers in
//!   frame space; so does this one);
//! * **the zero-drift proofs**: plain surfaces and client-decorated
//!   windows keep the pre-Phase-56 answers exactly (their content is
//!   their whole window — nothing to grow), interleaved with the
//!   chrome-aware windows in one stack;
//! * **the oversized window**: a frame wider than the usable area
//!   anchors its corner at the origin — the band visible, the
//!   overflow the Fill doctrine's own (occluded under the dock,
//!   never cropped);
//! * **the scaled output**: a 2x panel doubles the shift — the
//!   insets, the cascade's step, and the band's top edge at exactly
//!   the frame's y=0;
//! * **the migration**: a hotplug swap to a portrait display
//!   re-places the stack in the new class's policy — the phone
//!   re-anchors the *frame* at the new usable origin (the plain
//!   window at the raw origin beside it: the two arms' distinct
//!   truths in one assertion set).

mod testbench;

use ldp_client::Proxy;
use ldp_core::geometry::Rect;
use ldp_core::wire::Value;
use testbench::*;

use ldp_compositor::surface::Surface;
use lion_compositor::rearrange::Rearrange;
use lion_compositor::server::CompositorConfig;
use lion_compositor::shell::{chrome_geometry, DockMode, ShellConfig, BAND_ALPHA, BAND_RGB};

/// The XRGB8888 fourcc ("XR24").
const XR24: u32 = 0x3432_5258;
/// The mock output: 1920x1080@60.
const OUT_W: usize = 1920;
/// The mock's eDP connector.
const EDP: u32 = 91;
/// The mock's HDMI connector.
const HDMI: u32 = 93;

/// One XRGB word.
fn xrgb(r: u8, g: u8, b: u8) -> u32 {
    u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b)
}

/// One scanout pixel as a raw word.
fn word(words: &[u32], x: usize, y: usize) -> u32 {
    words[y * OUT_W + x]
}

/// One scanout word for an opaque straight color.
fn opaque(v: u32) -> u32 {
    0xFF00_0000 | (v & 0x00FF_FFFF)
}

/// The band's scanout word over the opaque-black desktop (the flat
/// bar's exact premultiplied landing — the arithmetic the renderer's
/// over-blend performs).
fn band_word() -> u32 {
    let m = |v: u8| (u32::from(v) * u32::from(BAND_ALPHA) + 127) / 255;
    0xFF00_0000 | (m(BAND_RGB[0]) << 16) | (m(BAND_RGB[1]) << 8) | m(BAND_RGB[2])
}

/// The placement tests' configs: dock auto (the cascade's arithmetic
/// the frame geometry rides), the default 40 px reservation at the
/// bottom, far from every window the tests place.
fn placement_config(tag: &str) -> CompositorConfig {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    CompositorConfig {
        shell: ShellConfig {
            dock: DockMode::Auto,
            ..ShellConfig::default()
        },
        socket: format!("lion-place-{tag}-{n}-{}", std::process::id()),
        ..CompositorConfig::default()
    }
}

/// A memfd carrying `pixels` (the pool backing).
fn pool_bytes(pixels: &[u8]) -> std::os::fd::OwnedFd {
    use std::io::Write as _;
    let fd = lion_compositor::sys::memfd("place-pool").expect("memfd");
    {
        let mut file = std::fs::File::from(fd.try_clone().expect("clone"));
        file.write_all(pixels).expect("write pool");
    }
    fd
}

/// Wait for one event of one name on one object.
fn wait_for_on(client: &mut TestClient, name: &str, target: u32) {
    client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == name && r.target == target)
    });
}

/// The latest event of one name on one object.
fn last_on<'a>(client: &'a TestClient, name: &str, target: u32) -> &'a Recorded {
    client
        .events
        .records
        .iter()
        .filter(|r| r.event == name && r.target == target)
        .next_back()
        .unwrap_or_else(|| panic!("no '{name}' event on object {target}"))
}

/// The serial of a recorded configure (arg 0).
fn serial_of(record: &Recorded) -> u32 {
    match &record.args[0] {
        Value::Uint32(s) => *s,
        other => panic!("configure's serial is not a uint: {other:?}"),
    }
}

/// The world's position of one surface (the tree's own truth).
fn position_of(tb: &Testbench, surface: &Proxy) -> (i32, i32) {
    tb.world(|w| {
        for (sid, route) in &w.scene.routes {
            if route.surface_obj.as_u32() == surface.id().as_u32() {
                return w.scene.tree.get(*sid).map_or((0, 0), Surface::position);
            }
        }
        (0, 0)
    })
}

/// The drawn frame of one surface (chrome_geometry's one truth: the
/// applied insets around the live content rect), `None` when the
/// window draws no chrome.
fn frame_of(tb: &Testbench, surface: &Proxy) -> Option<Rect> {
    tb.world(|w| {
        for (sid, route) in &w.scene.routes {
            if route.surface_obj.as_u32() == surface.id().as_u32() {
                let node = w.scene.tree.get(*sid)?;
                let bounds = node.last_bounds();
                if bounds.is_empty() {
                    return None;
                }
                return chrome_geometry(&w.toplevels, *sid, bounds).map(|(f, _)| f);
            }
        }
        None
    })
}

/// Map one plain window at its creation position (no role — the
/// cascade's filler rows). Returns the surface.
fn map_plain(tb: &Testbench, client: &mut TestClient, fill: u32, w: usize, h: usize) -> Proxy {
    let shm = client.bind("ldp.core.shm");
    let compositor = client.bind("ldp.core.compositor");
    let pixels: Vec<u8> = std::iter::repeat(fill.to_le_bytes())
        .take(w * h)
        .flatten()
        .collect();
    let pool = create_pool(client, &shm, pool_bytes(&pixels), (w * h * 4) as i64);
    let buffer = create_buffer(client, &pool, 0, w as i32, h as i32, (w * 4) as i32, XR24);
    let surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("surface");
    let before = tb.frames();
    attach(client, &surface, &buffer);
    damage(client, &surface, &[Rect::new(0, 0, w as u32, h as u32)]);
    commit(client, &surface, 1);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while tb.frames() <= before {
        assert!(deadline > std::time::Instant::now(), "never rendered");
        client.sync();
    }
    surface
}

/// One toplevel window realized over the real socket — the role
/// minted (`server` for SSD, `client` for CSD), the handshake
/// proposal acked, the first commit mapping the window at the
/// cascade's next step. No position pin: each test owns its own
/// truth. Returns (surface, toplevel).
fn toplevel_at(
    tb: &Testbench,
    client: &mut TestClient,
    fill: u32,
    w: usize,
    h: usize,
    server: bool,
) -> (Proxy, Proxy) {
    let shell_proxy = client.bind("ldp.shell.shell");
    let shm = client.bind("ldp.core.shm");
    let compositor = client.bind("ldp.core.compositor");
    let surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("surface");
    let toplevel = client
        .conn
        .create_object(
            &shell_proxy,
            "get_toplevel",
            vec![
                Value::Object(Some(surface.id())),
                // The decoration mode: 1 = server-drawn (the SSD band),
                // 2 = client-drawn.
                Value::Enum(if server { 1 } else { 2 }),
            ],
        )
        .expect("get_toplevel");
    wait_for_on(client, "configure", toplevel.id().as_u32());
    let handshake = last_on(client, "configure", toplevel.id().as_u32());
    client
        .conn
        .send_request(
            &toplevel,
            "ack_configure",
            vec![Value::Uint32(serial_of(handshake))],
        )
        .expect("ack");
    let pixels: Vec<u8> = std::iter::repeat(fill.to_le_bytes())
        .take(w * h)
        .flatten()
        .collect();
    let pool = create_pool(client, &shm, pool_bytes(&pixels), (w * h * 4) as i64);
    let buffer = create_buffer(client, &pool, 0, w as i32, h as i32, (w * 4) as i32, XR24);
    let before = tb.frames();
    attach(client, &surface, &buffer);
    damage(client, &surface, &[Rect::new(0, 0, w as u32, h as u32)]);
    commit(client, &surface, 1);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while tb.frames() <= before {
        assert!(deadline > std::time::Instant::now(), "never rendered");
        client.sync();
    }
    (surface, toplevel)
}

/// THE gap, closed: a server-decorated window parked at the usable
/// origin wears its top band on the screen. The frame (not the
/// content) takes Fill's... the desktop's first cascade slot: the
/// content rides at the slot plus the applied insets, and the band's
/// ink lands from row 0 of the display — the pre-Phase-56 truth (the
/// content at the origin, the 29 px of band drawn above the visible
/// area, the caption unreachable) refuted by the pixels themselves.
#[test]
fn the_parked_window_wears_its_band_on_screen() {
    let tb = Testbench::start_with("place-park", placement_config("park"));
    let mut client = TestClient::connect(&tb.addr);
    let (w, _t) = toplevel_at(&tb, &mut client, xrgb(30, 140, 150), 200, 100, true);

    // The frame takes the slot: the content rides inside.
    assert_eq!(
        position_of(&tb, &w),
        (1, 29),
        "the content at slot + insets"
    );
    let frame = frame_of(&tb, &w).expect("the chrome serves");
    assert_eq!((frame.x, frame.y, frame.w, frame.h), (0, 0, 202, 130));
    // The band's top row is ON the display — band ink, not the void.
    let words = tb.scanout();
    assert_eq!(word(&words, 100, 0), band_word(), "row 0 is the band");
    assert_eq!(word(&words, 100, 15), band_word(), "the band's middle row");
    // The content starts below the band: the client's own ink.
    assert_eq!(word(&words, 100, 40), opaque(xrgb(30, 140, 150)));
    // The close affordance rides on-screen (the whole caption
    // reachable: the button's territory inside the display).
    assert_eq!(
        word(&words, 170, 12),
        band_word(),
        "left of the button's territory"
    );
    // Right of the frame: the desktop's own black, unclaimed.
    assert_eq!(
        word(&words, 210, 50),
        0xFF00_0000,
        "the desktop right of the frame"
    );
}

/// The cascade steps *frames*: window n's frame sits at the cascade's
/// nth slot (the captions step the way DWM's do), each band fully
/// on-screen from its own row 0 — the third window's frame at
/// (48, 48), its content at (49, 77).
#[test]
fn the_cascade_steps_the_frames() {
    let tb = Testbench::start_with("place-steps", placement_config("steps"));
    let mut client = TestClient::connect(&tb.addr);
    let (w0, _t0) = toplevel_at(&tb, &mut client, xrgb(200, 40, 40), 200, 100, true);
    let (w1, _t1) = toplevel_at(&tb, &mut client, xrgb(40, 80, 200), 200, 100, true);
    let (w2, _t2) = toplevel_at(&tb, &mut client, xrgb(30, 140, 150), 200, 100, true);

    assert_eq!(position_of(&tb, &w0), (1, 29));
    assert_eq!(position_of(&tb, &w1), (25, 53));
    assert_eq!(position_of(&tb, &w2), (49, 77));
    for (w, n) in [&w0, &w1, &w2].iter().zip(0..) {
        let frame = frame_of(&tb, w).expect("the chrome serves");
        let slot = 24 * n;
        assert_eq!(
            (frame.x, frame.y),
            (slot, slot),
            "window {n}'s frame takes the cascade's slot"
        );
    }
    // Each window's band begins at its own row (24, 48) — sampled
    // over the desktop right of the lower windows' frames, each
    // caption fully reachable, no clipped tops anywhere in the stack.
    let words = tb.scanout();
    assert_eq!(
        word(&words, 210, 24),
        band_word(),
        "w1's band from its row 0"
    );
    assert_eq!(
        word(&words, 230, 48),
        band_word(),
        "w2's band from its row 0"
    );
}

/// THE zero-drift proof, live: plain surfaces and client-decorated
/// windows keep the pre-Phase-56 answers exactly (their content is
/// their whole window — nothing to grow), interleaved with a
/// chrome-aware window in one stack (the counter shared, each arm
/// answering by its own truth).
#[test]
fn plain_and_client_windows_keep_their_answers() {
    let tb = Testbench::start_with("place-plain", placement_config("plain"));
    let mut client = TestClient::connect(&tb.addr);
    let p0 = map_plain(&tb, &mut client, xrgb(90, 30, 30), 150, 90);
    let (c1, _ct1) = toplevel_at(&tb, &mut client, xrgb(90, 160, 60), 150, 90, false);
    let p2 = map_plain(&tb, &mut client, xrgb(30, 90, 160), 150, 90);
    let (s3, _st3) = toplevel_at(&tb, &mut client, xrgb(160, 90, 30), 150, 90, true);

    // The plain rows: the content slots, byte-identical to Phase 28.
    assert_eq!(position_of(&tb, &p0), (0, 0));
    assert_eq!(position_of(&tb, &p2), (48, 48));
    // The CSD window: its buffer is its whole footprint — the raw
    // slot, no shift (the Phase 49 pins' own doctrine).
    let cpos = position_of(&tb, &c1);
    assert_eq!(cpos, (24, 24), "the client-decorated window never shifted");
    // The SSD window after them: the frame takes its slot.
    assert_eq!(position_of(&tb, &s3), (73, 101));
    let frame = frame_of(&tb, &s3).expect("the chrome serves");
    assert_eq!((frame.x, frame.y), (72, 72));
    // The CSD window draws no chrome: no frame to claim.
    assert!(frame_of(&tb, &c1).is_none(), "CSD draws no server chrome");
}

/// The oversized window: a frame wider than the usable area anchors
/// its corner at the origin — the band visible, the overflow the
/// Fill doctrine's own (the content runs off the right edge, the
/// corner stack's terminal state, never a clipped caption).
#[test]
fn the_oversized_window_anchors_its_frame() {
    let tb = Testbench::start_with("place-big", placement_config("big"));
    let mut client = TestClient::connect(&tb.addr);
    let (w, _t) = toplevel_at(&tb, &mut client, xrgb(30, 140, 150), 2500, 100, true);

    assert_eq!(
        position_of(&tb, &w),
        (1, 29),
        "the frame anchored at the origin"
    );
    let frame = frame_of(&tb, &w).expect("the chrome serves");
    assert_eq!((frame.x, frame.y), (0, 0));
    // The band from row 0 — the caption reachable even in the
    // terminal state.
    let words = tb.scanout();
    assert_eq!(
        word(&words, 100, 0),
        band_word(),
        "the oversized band on-screen"
    );
    // The content runs past the display's right edge (the overflow
    // doctrine: visible to the edge, honest about the rest).
    assert_eq!(word(&words, 100, 40), opaque(xrgb(30, 140, 150)));
    assert_eq!(
        word(&words, 1919, 40),
        opaque(xrgb(30, 140, 150)),
        "ink to the edge"
    );
}

/// The scaled output: a 2x panel doubles the shift — the insets at
/// the machine's own formula (2/58/2/2), the cascade's step doubled,
/// and the band's top edge at exactly the frame's y=0 (the
/// pixel-true clamp's arithmetic, live over the wire).
#[test]
fn the_scaled_output_doubles_the_band_shift() {
    let config = CompositorConfig {
        scale: ldp_core::scale::ScaleFactor::from_f32_lossy(2.0).expect("2x"),
        shell: ShellConfig {
            dock: DockMode::Auto,
            ..ShellConfig::default()
        },
        socket: format!("lion-place-scale-{}", std::process::id()),
        ..CompositorConfig::default()
    };
    let tb = Testbench::start_with("place-scale", config);
    let mut client = TestClient::connect(&tb.addr);
    let (w, _t) = toplevel_at(&tb, &mut client, xrgb(30, 140, 150), 400, 200, true);

    assert_eq!(position_of(&tb, &w), (2, 58), "the doubled shift");
    let frame = frame_of(&tb, &w).expect("the chrome serves");
    assert_eq!((frame.x, frame.y, frame.w, frame.h), (0, 0, 404, 260));
    // The band's 58 doubled rows from y=0; the content below them.
    let words = tb.scanout();
    assert_eq!(
        word(&words, 100, 0),
        band_word(),
        "the scaled band from row 0"
    );
    assert_eq!(word(&words, 100, 57), band_word(), "the band's last row");
    assert_eq!(word(&words, 100, 70), opaque(xrgb(30, 140, 150)));
}

/// THE migration, chrome-aware: a hotplug swap to a portrait display
/// re-places the stack under the new class's policy — the phone
/// re-anchors the *frame* at the new usable origin (the SSD window
/// at the origin plus its applied insets), the plain window at the
/// raw origin beside it (the two arms' distinct truths), and the
/// band lands on the new display's own row 0.
#[test]
#[allow(clippy::too_many_lines)] // one session, one narrative
fn the_migration_re_places_the_frame() {
    let tb = Testbench::start_with("place-migrate", placement_config("migrate"));
    let mut client = TestClient::connect(&tb.addr);
    let (w, t) = toplevel_at(&tb, &mut client, xrgb(30, 140, 150), 200, 100, true);
    let plain = map_plain(&tb, &mut client, xrgb(90, 30, 30), 64, 64);

    // The stack before the swap: the SSD frame at the origin, the
    // plain window at the cascade's second step.
    assert_eq!(position_of(&tb, &w), (1, 29));
    assert_eq!(position_of(&tb, &plain), (24, 24));

    // A portrait display joins offering its own mode; the eDP dies;
    // the migration lands on the phone panel.
    {
        let mut world = tb.shared.world.lock().expect("world lock");
        world
            .device
            .as_mock_mut()
            .expect("the mock driver")
            .hotplug_connect(
                ldp_display::ids::ConnectorId::new(HDMI).expect("connector"),
                vec![ldp_display::mode::Mode::new(
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
                )],
                vec![0x5Au8; 16],
            );
    }
    {
        let mut world = tb.shared.world.lock().expect("world lock");
        world
            .device
            .as_mock_mut()
            .expect("the mock driver")
            .hotplug_disconnect(ldp_display::ids::ConnectorId::new(EDP).expect("connector"));
    }
    let rearranged = {
        let mut world = tb.shared.world.lock().expect("world lock");
        world
            .service_device_events()
            .expect("the service pass")
            .expect("a hotplug event was queued")
    };
    match rearranged {
        Rearrange::Migrated { to, .. } => {
            assert_eq!((to.width, to.height), (540, 960), "the phone panel");
        }
        other => panic!("expected a migration, got {other:?}"),
    }

    // The re-placement, each arm by its own truth: the plain window
    // at the raw usable origin, the SSD window's *frame* there — the
    // content at the origin plus the applied insets.
    assert_eq!(
        position_of(&tb, &plain),
        (0, 0),
        "the plain arm never shifted"
    );
    assert_eq!(
        position_of(&tb, &w),
        (1, 29),
        "the frame re-anchored at the origin"
    );
    let chrome = frame_of(&tb, &w).expect("the chrome serves");
    assert_eq!((chrome.x, chrome.y, chrome.w, chrome.h), (0, 0, 202, 130));

    // The band on the new display's own row 0 — the caption the first
    // thing on the new screen, the content below it. The re-render
    // rides a client's present (the migration's own flip lands the
    // chain; the scene follows the next committed frame — the hotplug
    // suite's cadence): the same surface commits on the new display
    // without reconnecting.
    let teal = opaque(xrgb(30, 140, 150));
    let shm = client.bind("ldp.core.shm");
    let pixels: Vec<u8> = std::iter::repeat(xrgb(30, 140, 150).to_le_bytes())
        .take(200 * 100)
        .flatten()
        .collect();
    let pool = create_pool(
        &mut client,
        &shm,
        pool_bytes(&pixels),
        (200 * 100 * 4) as i64,
    );
    let buffer = create_buffer(&mut client, &pool, 0, 200, 100, 800, XR24);
    frame(&mut client, &w, 1);
    attach(&mut client, &w, &buffer);
    damage(&mut client, &w, &[Rect::new(0, 0, 200, 100)]);
    commit(&mut client, &w, 2);
    client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "committed" && r.args[0] == Value::Uint32(2))
    });
    client.wait_until(|c| c.records.iter().any(|r| r.event == "presented"));
    let words = tb.scanout();
    assert_eq!(words.len(), 540 * 960, "the phone panel's canvas");
    let w540 = |x: usize, y: usize| words[y * 540 + x];
    assert_eq!(w540(100, 40), teal, "the content below the band");
    // The band's own repaint rides the chrome's claims (a fresh
    // canvas paints what the claims ledger repaints — the migration's
    // full-scene re-render rides the render-thread line, the honest
    // remainder): the minimize/unminimize pair claims the whole
    // chrome's rect, and the exact bytes return on the new panel.
    client
        .conn
        .send_request(&t, "minimize", vec![])
        .expect("minimize");
    client.wait_until(|c| {
        c.records
            .iter()
            .filter(|r| r.event == "configure" && r.target == t.id().as_u32())
            .count()
            >= 2
    });
    client
        .conn
        .send_request(&t, "unminimize", vec![])
        .expect("unminimize");
    client.wait_until(|c| {
        c.records
            .iter()
            .filter(|r| r.event == "configure" && r.target == t.id().as_u32())
            .count()
            >= 3
    });
    for _ in 0..3 {
        client.sync();
    }
    let words = tb.scanout();
    let w540 = |x: usize, y: usize| words[y * 540 + x];
    assert_eq!(w540(100, 0), band_word(), "the band on the new panel");
    assert_eq!(w540(100, 40), teal, "and the content below it");
}
