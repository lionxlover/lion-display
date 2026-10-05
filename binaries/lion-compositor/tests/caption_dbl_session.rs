//! Phase 58's exit criterion (the caption's double-click grammar):
//! two presses on the same window's band — inside the double-click
//! window (500 ms) and the double-click radius (8 px) — toggle the
//! geometry verb, end to end over the real protocol —
//!
//! * **the maximize arm**: a floating SSD window's pair fires on the
//!   *second press* (the `WM_NCLBUTTONDBLCLK` doctrine) — the states
//!   arm's own machinery answers (the maximized flag, the workspace
//!   fill 1918 x 966, the serial continuing the domain), the presses
//!   never route to the client, the pair's release is consumed, and
//!   the client's ack+commit realizes the fill at the usable origin
//!   (the frame at (0, 0), the content at (1, 29) — the drawn band
//!   still on-screen, the pixel oracle pinning both the band's ink
//!   and the content's);
//! * **the restore arm**: a maximized window's pair (the client's own
//!   verb engaged it first — the two doors' equivalence) releases the
//!   flag and proposes the client's own size (0x0), the realizing
//!   commit returning the window to its restore point;
//! * **the two gates**: a second press outside the 500 ms window
//!   never pairs (the clock restarted — a later quick pair still
//!   fires), and a second press outside the radius never pairs (two
//!   *gestures*, never one — the slot re-records at the new point);
//! * **the close territory**: the close affordance's own clicks never
//!   toggle (each fires its arm-fire ask — the grammar's territory is
//!   the caption, never the button), and a close press *voids* the
//!   pending caption click (a click that targets the button never
//!   targets the caption);
//! * **the drag void**: a press that converts into a drag voids the
//!   pending pair (the hand moved the window — the next click is a
//!   fresh first click; the grammar is click-click, never
//!   drag-click), and the clock restarts from the drag's end;
//! * **the no-chrome controls**: plain windows and client-decorated
//!   windows never toggle (no band, no pair — the presses route to
//!   the client exactly as they always have, zero drift);
//! * **the one-slot clock**: a click on another window's band resets
//!   the clock (the pair never forms across windows — the grammar is
//!   per-gesture, one hand, one caption);
//! * **the death sweep**: the window's destroy takes the pending
//!   click with it (a dead window's band can never pair), and the
//!   world stays healthy.

mod testbench;

use ldp_client::Proxy;
use ldp_core::geometry::Rect;
use ldp_core::wire::Value;
use ldp_display::DisplayDriver;
use ldp_input::device::DeviceClass;
use ldp_input::normalizer::InputEvent;
use testbench::*;

use ldp_compositor::surface::Surface;
use lion_compositor::server::CompositorConfig;
use lion_compositor::shell::{DockMode, ShellConfig, BAND_ALPHA, BAND_RGB};

/// The XRGB8888 fourcc ("XR24").
const XR24: u32 = 0x3432_5258;
/// The mock output: 1920x1080@60 (the usable area 1920x996, the dock
/// at the bottom — the caption suite's own grid).
const OUT_W: usize = 1920;
/// BTN_LEFT (the evdev button code).
const BTN_LEFT: u32 = 0x110;

/// One XRGB word.
fn xrgb(r: u8, g: u8, b: u8) -> u32 {
    u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b)
}

/// One scanout pixel as a raw word (the premultiplied truth the
/// painter's words land in, uninterpreted).
fn word(words: &[u32], x: usize, y: usize) -> u32 {
    words[y * OUT_W + x]
}

/// The band's scanout word (the near-opaque bar over the opaque-black
/// desktop — the exact arithmetic the renderer's over-blend performs).
fn band_word() -> u32 {
    let m = |v: u8| (u32::from(v) * u32::from(BAND_ALPHA) + 127) / 255;
    0xFF00_0000 | (m(BAND_RGB[0]) << 16) | (m(BAND_RGB[1]) << 8) | m(BAND_RGB[2])
}

/// Wait until `n` events of one name have arrived on one object (the
/// count-based doctrine — the wait must fire on the *new* arrival,
/// never one already collected).
fn wait_for_on_with(client: &mut TestClient, name: &str, target: u32, n: usize) {
    client.wait_until(|c| {
        c.records
            .iter()
            .filter(|r| r.event == name && r.target == target)
            .count()
            >= n
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

/// The client's recorded count of one event on one object.
fn count_on(client: &TestClient, name: &str, target: u32) -> usize {
    client
        .events
        .records
        .iter()
        .filter(|r| r.event == name && r.target == target)
        .count()
}

/// The serial of a recorded configure (arg 0).
fn serial_of(record: &Recorded) -> u32 {
    match &record.args[0] {
        Value::Uint32(s) => *s,
        other => panic!("configure's serial is not a uint: {other:?}"),
    }
}

/// The states bitset of a recorded configure (arg 1).
fn states_of(record: &Recorded) -> ldp_core::bitset::Bitset128 {
    match &record.args[1] {
        Value::Bitset(bits) => *bits,
        other => panic!("configure's states argument is not a bitset: {other:?}"),
    }
}

/// A memfd carrying `pixels` (the pool backing).
fn pool_bytes(pixels: &[u8]) -> std::os::fd::OwnedFd {
    use std::io::Write as _;
    let fd = lion_compositor::sys::memfd("caption-dbl-pool").expect("memfd");
    {
        let mut file = std::fs::File::from(fd.try_clone().expect("clone"));
        file.write_all(pixels).expect("write pool");
    }
    fd
}

/// The caption-dbl tests' configs: dock auto (the caption suite's own
/// grid — the usable area 1920x996, the maximized frame filling it).
fn dbl_config(tag: &str) -> CompositorConfig {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    CompositorConfig {
        shell: ShellConfig {
            dock: DockMode::Auto,
            ..ShellConfig::default()
        },
        socket: format!("lion-caption-dbl-{tag}-{n}-{}", std::process::id()),
        ..CompositorConfig::default()
    }
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

/// `shell.get_toplevel(surface, server-decorations)` — the SSD mint.
fn get_ssd_toplevel(client: &mut TestClient, shell: &Proxy, surface: &Proxy) -> Proxy {
    client
        .conn
        .create_object(
            shell,
            "get_toplevel",
            vec![Value::Object(Some(surface.id())), Value::Enum(1)],
        )
        .expect("get_toplevel")
}

/// `shell.get_toplevel(surface, client-decorations)` — the CSD mint.
fn get_csd_toplevel(client: &mut TestClient, shell: &Proxy, surface: &Proxy) -> Proxy {
    client
        .conn
        .create_object(
            shell,
            "get_toplevel",
            vec![Value::Object(Some(surface.id())), Value::Enum(2)],
        )
        .expect("get_toplevel")
}

/// One SSD window realized: the toplevel minted *before* the mapping
/// commit (the well-behaved client's order), the handshake acked, the
/// first commit mapping the surface at the cascade's `nth` step with
/// the insets already applied (the band serving from its first
/// frame). Returns (surface, toplevel, buffer). The window is 200x100
/// with `fill`.
fn ssd_window(
    tb: &Testbench,
    client: &mut TestClient,
    fill: u32,
    nth: usize,
) -> (Proxy, Proxy, Proxy) {
    let shell = client.bind("ldp.shell.shell");
    let shm = client.bind("ldp.core.shm");
    let compositor = client.bind("ldp.core.compositor");
    let surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("surface");
    let toplevel = get_ssd_toplevel(client, &shell, &surface);
    // The handshake: serial 1, the SSD insets (the Lion metrics at
    // identity scale — 1 / 29 / 1 / 1).
    wait_for_on_with(client, "configure", toplevel.id().as_u32(), 1);
    let handshake = last_on(client, "configure", toplevel.id().as_u32());
    assert_eq!(serial_of(handshake), 1);
    assert_eq!(handshake.args[4], Value::Int32(1), "the left border");
    assert_eq!(handshake.args[5], Value::Int32(29), "the title band");
    assert_eq!(handshake.args[6], Value::Int32(1), "the right border");
    assert_eq!(handshake.args[7], Value::Int32(1), "the bottom border");
    // The ack, then the mapping commit (the applied insets land with
    // it — the chrome serves from this first frame).
    client
        .conn
        .send_request(&toplevel, "ack_configure", vec![Value::Uint32(1)])
        .expect("ack");
    let (w, h) = (200usize, 100usize);
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
    // The cascade's `nth` step (24 px per step, the first at the
    // origin): the content at (step+1, step+29), the frame around it.
    let step = 24 * nth as i32;
    assert_eq!(position_of(tb, &surface), (step + 1, step + 29));
    (surface, toplevel, buffer)
}

/// The world's position of one surface (the tree's own truth).
fn position_of(tb: &Testbench, surface: &Proxy) -> (i32, i32) {
    tb.world(|w| {
        for (sid, route) in &w.scene.routes {
            if route.surface_obj.as_u32() == surface.id().as_u32() {
                return w.scene.tree.get(*sid).map_or((0, 0), Surface::position);
            }
        }
        panic!("the surface has no route");
    })
}

/// One scripted input batch through the same pump the real devices
/// take (the caption grip's own heartbeat).
fn batch(tb: &Testbench, events: Vec<InputEvent>) {
    tb.world_mut(|w| {
        w.queue_input(DeviceClass::Mouse, events);
        w.pump_input();
    });
}

/// One button press/release.
fn button(tb: &Testbench, pressed: bool) {
    batch(
        tb,
        vec![InputEvent::Button {
            button: BTN_LEFT,
            pressed,
        }],
    );
}

/// One pointer motion by `(dx, dy)`.
fn motion(tb: &Testbench, dx: i32, dy: i32) {
    batch(tb, vec![InputEvent::PointerMotion { dx, dy }]);
}

/// Drive the pointer to an absolute position in accelerator-neutral
/// steps (each batch under the curve's 200-count threshold — the
/// pointer lands exactly where the arithmetic says; the drag suite's
/// own hard lesson, respected here).
fn move_to(tb: &Testbench, x: i32, y: i32) {
    loop {
        let (px, py) = tb.world(lion_compositor::World::input_pointer_position);
        let (dx, dy) = (x - px as i32, y - py as i32);
        if dx == 0 && dy == 0 {
            return;
        }
        // 100 per axis keeps the batch's magnitude at 141 — under
        // the threshold, the gain exactly 1.
        motion(tb, dx.clamp(-100, 100), dy.clamp(-100, 100));
    }
}

/// Advance the mock's injected clock (the double-click window's own
/// time source — the honest gate measurement, no wall-clock sleeps).
fn advance_ms(tb: &Testbench, ms: u64) {
    tb.world_mut(|w| {
        w.device
            .as_mock_mut()
            .expect("the mock driver")
            .advance_ns(ms * 1_000_000);
    });
}

/// The pointer device's mint (the routed events the presses would
/// deliver — the caption's presses never become one).
fn mint_pointer(client: &mut TestClient) -> Proxy {
    let seat = client.bind("ldp.input.seat");
    client
        .conn
        .create_object(&seat, "get_pointer", vec![])
        .expect("pointer")
}

/// The suite's stage: two fillers, the SSD window at the cascade's
/// third step (content (49, 77), frame (48, 48, 202, 130), the band's
/// rows [48, 77)), the pointer minted. Returns (filler, surface,
/// toplevel, buffer, pointer).
fn dbl_stage(
    tb: &Testbench,
    client: &mut TestClient,
) -> (Proxy, Proxy, Proxy, Proxy, Proxy) {
    let _w1 = map_plain(tb, client, xrgb(200, 40, 40), 150, 90);
    let _w2 = map_plain(tb, client, xrgb(40, 80, 200), 200, 100);
    let (w3, t3, b3) = ssd_window(tb, client, xrgb(30, 140, 150), 2);
    let pointer = mint_pointer(client);
    (_w2, w3, t3, b3, pointer)
}

/// THE maximize arm: the pair fires on the second press — the states
/// arm's own proposal (the maximized flag, the workspace fill
/// 1918 x 966, the serial continuing the domain after the
/// activation), the presses never route to the client, the pair's
/// release is consumed like every band button, and the client's
/// ack+commit realizes the fill at the usable origin (the frame at
/// (0, 0), the content at (1, 29) — the pixel oracle pinning both
/// the band's ink and the content's, the first filler covered).
#[test]
#[allow(clippy::too_many_lines)] // one session, one narrative
fn the_pair_maximizes_the_floating_window() {
    let tb = Testbench::start_with("dbl-max", dbl_config("mx"));
    let mut client = TestClient::connect(&tb.addr);
    let (filler, w3, t3, _b3, pointer) = dbl_stage(&tb, &mut client);

    // The first press on the band: (201, 51) — the band's middle row,
    // far from the close affordance at (225, 53, 20, 20). The press
    // arms and focuses (the Phase 52 doctrine holds under the new
    // grammar — the activation proposal, nothing else).
    move_to(&tb, 201, 51);
    button(&tb, true);
    client.sync();
    wait_for_on_with(&mut client, "configure", t3.id().as_u32(), 2);
    assert!(
        states_of(last_on(&client, "configure", t3.id().as_u32())).test(3),
        "the first press activated the window (bit 3)"
    );
    assert_eq!(
        count_on(&client, "button", pointer.id().as_u32()),
        0,
        "the first press never reaches the client"
    );
    button(&tb, false);
    client.sync();
    assert_eq!(
        count_on(&client, "configure", t3.id().as_u32()),
        2,
        "the click alone never toggles — the press, not the release, is the pair's second half"
    );

    // The second press, inside the window and the radius: the pair
    // fires *now* (on the press — the WM_NCLBUTTONDBLCLK doctrine).
    button(&tb, true);
    client.sync();
    wait_for_on_with(&mut client, "configure", t3.id().as_u32(), 3);
    let proposal = last_on(&client, "configure", t3.id().as_u32());
    assert_eq!(serial_of(proposal), 3, "the serial continues the domain");
    assert!(
        states_of(proposal).test(0),
        "the maximized flag (bit 0) — the toggle's own word"
    );
    assert!(
        states_of(proposal).test(3),
        "still the key window (the pair's press kept its activation)"
    );
    // The workspace fill: usable 1920x996 minus the SSD insets
    // (1 / 29 / 1 / 1) — 1918 x 966, the same answer the client's own
    // verb serves (the two doors, one room).
    assert_eq!(proposal.args[2], Value::Uint32(1918), "the fill's width");
    assert_eq!(proposal.args[3], Value::Uint32(966), "the fill's height");
    assert_eq!(
        count_on(&client, "button", pointer.id().as_u32()),
        0,
        "the pair's second press never reaches the client either"
    );

    // The pair's release: consumed like every band button, nothing
    // more fired, no drag ever minted.
    button(&tb, false);
    client.sync();
    assert_eq!(
        count_on(&client, "configure", t3.id().as_u32()),
        3,
        "the release resolves the grip — no second proposal"
    );
    assert_eq!(
        count_on(&client, "button", pointer.id().as_u32()),
        0,
        "the pair's release never routes"
    );

    // The two-phase commit: ack, then the buffer that answers the
    // fill, the position completing the placement.
    client
        .conn
        .send_request(&t3, "ack_configure", vec![Value::Uint32(3)])
        .expect("ack");
    let shm = client.bind("ldp.core.shm");
    let big: Vec<u8> = std::iter::repeat(xrgb(30, 140, 150).to_le_bytes())
        .take(1918 * 966)
        .flatten()
        .collect();
    let big_pool = create_pool(&mut client, &shm, pool_bytes(&big), (1918 * 966 * 4) as i64);
    let big_buffer = create_buffer(&mut client, &big_pool, 0, 1918, 966, 1918 * 4, XR24);
    let before = tb.frames();
    attach(&mut client, &w3, &big_buffer);
    damage(&mut client, &w3, &[Rect::new(0, 0, 1918, 966)]);
    commit(&mut client, &w3, 2);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while tb.frames() <= before {
        assert!(deadline > std::time::Instant::now(), "never rendered");
        client.sync();
    }
    assert_eq!(
        position_of(&tb, &w3),
        (1, 29),
        "the fill realized: the frame at the usable origin, the content inset by the band"
    );

    // The pixel truths: the drawn band still on-screen at the fill's
    // top (the near-opaque bar over the desktop), the content's ink
    // inside, the first filler (the red window at the origin)
    // covered beneath the maximized frame's band.
    let words = tb.scanout();
    assert_eq!(word(&words, 960, 10), band_word(), "the band on the fill's top");
    assert_eq!(
        word(&words, 960, 500),
        0xFF00_0000 | xrgb(30, 140, 150),
        "the content's ink"
    );
    assert_eq!(
        word(&words, 10, 10),
        band_word(),
        "the origin's filler covered by the maximized frame's band"
    );
    let _ = filler;
}

/// THE restore arm: the client's own verb engaged the geometry (the
/// two doors' equivalence), and the pair releases it — the flag gone,
/// the client's own size (0x0) proposed, the realizing commit
/// returning the window to its restore point (the position it held
/// when server geometry began).
#[test]
#[allow(clippy::too_many_lines)] // one session, one narrative
fn the_pair_restores_the_maximized_window() {
    let tb = Testbench::start_with("dbl-unmax", dbl_config("um"));
    let mut client = TestClient::connect(&tb.addr);
    let (_filler, w3, t3, b3, _pointer) = dbl_stage(&tb, &mut client);

    // The prelude: the client's own maximize verb — the engagement,
    // the fill realized (the pair's own door proven by the verb's).
    client
        .conn
        .send_request(&t3, "maximize", vec![])
        .expect("maximize");
    wait_for_on_with(&mut client, "configure", t3.id().as_u32(), 2);
    let max = last_on(&client, "configure", t3.id().as_u32());
    let max_serial = serial_of(max);
    assert!(states_of(max).test(0), "the verb engaged the flag");
    let shm = client.bind("ldp.core.shm");
    let big: Vec<u8> = std::iter::repeat(xrgb(30, 140, 150).to_le_bytes())
        .take(1918 * 966)
        .flatten()
        .collect();
    let big_pool = create_pool(&mut client, &shm, pool_bytes(&big), (1918 * 966 * 4) as i64);
    let big_buffer = create_buffer(&mut client, &big_pool, 0, 1918, 966, 1918 * 4, XR24);
    let before = tb.frames();
    client
        .conn
        .send_request(&t3, "ack_configure", vec![Value::Uint32(max_serial)])
        .expect("ack");
    attach(&mut client, &w3, &big_buffer);
    damage(&mut client, &w3, &[Rect::new(0, 0, 1918, 966)]);
    commit(&mut client, &w3, 2);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while tb.frames() <= before {
        assert!(deadline > std::time::Instant::now(), "never rendered");
        client.sync();
    }
    assert_eq!(position_of(&tb, &w3), (1, 29), "maximized, the frame at origin");

    // The pair on the maximized band: (400, 15) — the maximized
    // window's band rows [0, 29). The first click arms and focuses.
    move_to(&tb, 400, 15);
    button(&tb, true);
    client.sync();
    button(&tb, false);
    client.sync();

    // The second press: the restore — the flag released, the client's
    // own size (0x0 — the client's answer), the serial continuing.
    button(&tb, true);
    client.sync();
    wait_for_on_with(&mut client, "configure", t3.id().as_u32(), 4);
    let release = last_on(&client, "configure", t3.id().as_u32());
    assert_eq!(serial_of(release), 4, "the serial continues the domain");
    assert!(
        !states_of(release).test(0),
        "the maximized flag released — the pair's own word"
    );
    assert_eq!(release.args[2], Value::Uint32(0), "the client's own size");
    assert_eq!(release.args[3], Value::Uint32(0), "the client's own size");
    button(&tb, false);
    client.sync();

    // The realizing commit: the original buffer, the original
    // position (the restore point the engagement captured).
    client
        .conn
        .send_request(&t3, "ack_configure", vec![Value::Uint32(4)])
        .expect("ack");
    let before = tb.frames();
    attach(&mut client, &w3, &b3);
    damage(&mut client, &w3, &[Rect::new(0, 0, 200, 100)]);
    commit(&mut client, &w3, 3);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while tb.frames() <= before {
        assert!(deadline > std::time::Instant::now(), "never rendered");
        client.sync();
    }
    assert_eq!(position_of(&tb, &w3), (49, 77), "the restore point");
}

/// THE time gate: a second press outside the 500 ms window never
/// pairs — the press arms its grip as Phase 52 served it (the
/// activation beyond the handshake, nothing else), and the *clock
/// restarts* (a later quick pair still fires — the gate measured
/// honestly against the device's injected clock, no wall-clock
/// sleeps).
#[test]
fn a_slow_second_press_never_pairs() {
    let tb = Testbench::start_with("dbl-slow", dbl_config("sl"));
    let mut client = TestClient::connect(&tb.addr);
    let (_filler, _w3, t3, _b3, _pointer) = dbl_stage(&tb, &mut client);

    // The first press and release (the click recorded at the clock's
    // own word).
    move_to(&tb, 201, 51);
    button(&tb, true);
    client.sync();
    button(&tb, false);
    client.sync();
    assert_eq!(
        count_on(&client, "configure", t3.id().as_u32()),
        2,
        "the handshake and the activation — nothing else"
    );

    // 600 ms pass (the injected clock — the honest gate).
    advance_ms(&tb, 600);

    // The slow second press: two clicks, never a pair. The press
    // still arms (consumed, focusing — the Phase 52 doctrine holds).
    button(&tb, true);
    client.sync();
    button(&tb, false);
    client.sync();
    assert_eq!(
        count_on(&client, "configure", t3.id().as_u32()),
        2,
        "the slow press never toggles"
    );

    // The clock restarted at the slow press: the very next press —
    // quick, on the same spot — pairs with IT (the gate measured
    // against the fresh record, never the stale first click).
    button(&tb, true);
    client.sync();
    wait_for_on_with(&mut client, "configure", t3.id().as_u32(), 3);
    assert!(
        states_of(last_on(&client, "configure", t3.id().as_u32())).test(0),
        "the quick press after the slow one pairs — the clock restarted"
    );
}

/// THE radius gate: a second press outside the 8 px radius never
/// pairs — two *gestures* of one hand, never one; the slot re-records
/// at the new point (the next press there pairs).
#[test]
fn a_far_second_press_never_pairs() {
    let tb = Testbench::start_with("dbl-far", dbl_config("fr"));
    let mut client = TestClient::connect(&tb.addr);
    let (_filler, _w3, t3, _b3, _pointer) = dbl_stage(&tb, &mut client);

    // The first press at (201, 51), the release.
    move_to(&tb, 201, 51);
    button(&tb, true);
    client.sync();
    button(&tb, false);
    client.sync();

    // The far second press: (225, 51) — 24 px away, the same band,
    // never a pair.
    move_to(&tb, 225, 51);
    button(&tb, true);
    client.sync();
    button(&tb, false);
    client.sync();
    assert_eq!(
        count_on(&client, "configure", t3.id().as_u32()),
        2,
        "the far press never toggles"
    );

    // The slot re-records at the far point: the next press there
    // pairs (the radius gate was the only blocker).
    button(&tb, true);
    client.sync();
    wait_for_on_with(&mut client, "configure", t3.id().as_u32(), 3);
    assert!(
        states_of(last_on(&client, "configure", t3.id().as_u32())).test(0),
        "the pair at the recorded point toggles"
    );
}

/// THE close territory: the close affordance's own clicks never
/// toggle — each fires its arm-fire ask (the grammar's territory is
/// the caption, never the button), and a close press *voids* the
/// pending caption click (a click that targets the button never
/// targets the caption — the caption press after a close click is a
/// fresh first click, never a pair).
#[test]
fn the_close_territory_never_toggles() {
    let tb = Testbench::start_with("dbl-close", dbl_config("cl"));
    let mut client = TestClient::connect(&tb.addr);
    let (_filler, _w3, t3, _b3, _pointer) = dbl_stage(&tb, &mut client);

    // Two clicks on the drawn close button (its center — (235, 63),
    // the same affordance the Phase 52 narrative fires): each fires
    // its own close ask, no configure ever carries the flag.
    move_to(&tb, 235, 63);
    button(&tb, true);
    client.sync();
    button(&tb, false);
    client.sync();
    button(&tb, true);
    client.sync();
    button(&tb, false);
    client.sync();
    assert_eq!(
        count_on(&client, "close", t3.id().as_u32()),
        2,
        "each close click fires its own ask — the pair grammar never touches the button"
    );
    assert_eq!(
        count_on(&client, "configure", t3.id().as_u32()),
        2,
        "the handshake and the activation — the close territory never toggles"
    );

    // The void: a caption click recorded, a close click between, then
    // the caption click again *quickly* — no pair (the pending click
    // died with the close press).
    move_to(&tb, 201, 51);
    button(&tb, true);
    client.sync();
    button(&tb, false);
    client.sync();
    move_to(&tb, 235, 63);
    button(&tb, true);
    client.sync();
    button(&tb, false);
    client.sync();
    assert_eq!(
        count_on(&client, "close", t3.id().as_u32()),
        3,
        "the between click fired its ask"
    );
    move_to(&tb, 201, 51);
    button(&tb, true);
    client.sync();
    button(&tb, false);
    client.sync();
    assert_eq!(
        count_on(&client, "configure", t3.id().as_u32()),
        2,
        "the caption press after the close click is a fresh first click — no pair"
    );
}

/// THE drag void: a press that converts into a drag voids the pending
/// pair (the hand moved the window — the grammar is click-click,
/// never drag-click), the clock restarting from the drag's own
/// presses (the next quick pair on the moved window's band fires).
#[test]
fn a_drag_between_the_presses_voids_the_pair() {
    let tb = Testbench::start_with("dbl-drag", dbl_config("dg"));
    let mut client = TestClient::connect(&tb.addr);
    let (_filler, w3, t3, _b3, _pointer) = dbl_stage(&tb, &mut client);

    // The press on the band, the conversion (the caption drag the
    // Phase 53 suite pins), the release — the window moved to
    // (169, 137), the grip resolved.
    move_to(&tb, 201, 51);
    button(&tb, true);
    client.sync();
    wait_for_on_with(&mut client, "configure", t3.id().as_u32(), 2);
    motion(&tb, 120, 60);
    client.sync();
    button(&tb, false);
    client.sync();
    assert_eq!(position_of(&tb, &w3), (169, 137), "the drag moved the window");

    // The press on the moved band — the pointer (321, 111) sits on
    // the moved frame's band rows [108, 137). The conversion voided
    // the pending pair: this press is a fresh first click.
    button(&tb, true);
    client.sync();
    button(&tb, false);
    client.sync();
    assert_eq!(
        count_on(&client, "configure", t3.id().as_u32()),
        2,
        "the press after the drag never pairs with the drag's own press"
    );

    // The quick pair on the moved band: the toggle fires (the clock
    // restarted at the drag's end).
    button(&tb, true);
    client.sync();
    wait_for_on_with(&mut client, "configure", t3.id().as_u32(), 3);
    assert!(
        states_of(last_on(&client, "configure", t3.id().as_u32())).test(0),
        "the quick pair after the drag toggles"
    );
}

/// THE no-chrome controls: plain windows and client-decorated windows
/// never toggle — no band, no pair; the presses route to the client
/// exactly as they always have (zero drift), no configure ever
/// carries the flag (the grammar is the *drawn* chrome's own).
#[test]
fn plain_and_csd_windows_never_toggle() {
    let tb = Testbench::start_with("dbl-plain", dbl_config("pl"));
    let mut client = TestClient::connect(&tb.addr);

    // A plain window (no role) and a CSD toplevel side by side.
    let plain = map_plain(&tb, &mut client, xrgb(200, 40, 40), 150, 90);
    let csd = map_plain(&tb, &mut client, xrgb(40, 80, 200), 200, 100);
    let shell = client.bind("ldp.shell.shell");
    let toplevel = get_csd_toplevel(&mut client, &shell, &csd);
    let pointer = mint_pointer(&mut client);
    client.sync();
    // The CSD handshake (the inset reservation's own proposal).
    wait_for_on_with(&mut client, "configure", toplevel.id().as_u32(), 1);

    // Two quick presses on the plain window's content: both route
    // (the client's own pixels), no toggle, no configure.
    move_to(&tb, 75, 45);
    button(&tb, true);
    client.sync();
    button(&tb, false);
    client.sync();
    button(&tb, true);
    client.sync();
    button(&tb, false);
    client.sync();
    assert_eq!(
        count_on(&client, "button", pointer.id().as_u32()),
        4,
        "the plain window's presses route — no chrome, no grammar"
    );

    // Two quick presses on the CSD window's content (its reserved
    // title region is client-owned ink — no band to claim): the
    // presses route, the toplevel sees the activation and never a
    // flag proposal.
    move_to(&tb, 124, 34);
    button(&tb, true);
    client.sync();
    button(&tb, false);
    client.sync();
    button(&tb, true);
    client.sync();
    button(&tb, false);
    client.sync();
    assert_eq!(
        count_on(&client, "configure", toplevel.id().as_u32()),
        2,
        "the CSD window never toggles — the handshake and the activation, nothing else"
    );
    assert_eq!(
        count_on(&client, "button", pointer.id().as_u32()),
        8,
        "both windows' presses route — no chrome, no grammar"
    );
    let _ = plain;
}

/// The client's recorded count of configure events on one object
/// that carry the maximized flag (bit 0) — the toggle's own oracle,
/// immune to the activation dance's honest proposals.
fn flag_proposals(client: &TestClient, target: u32) -> usize {
    client
        .events
        .records
        .iter()
        .filter(|r| r.event == "configure" && r.target == target)
        .filter(|r| states_of(r).test(0))
        .count()
}

/// THE one-slot clock: a click on another window's band resets the
/// clock — the pair never forms across windows (the grammar is
/// per-gesture: one hand, one caption, one window).
#[test]
fn the_cross_window_press_resets_the_clock() {
    let tb = Testbench::start_with("dbl-xw", dbl_config("xw"));
    let mut client = TestClient::connect(&tb.addr);

    // Two SSD windows at the cascade's second and third steps.
    let _filler = map_plain(&tb, &mut client, xrgb(200, 40, 40), 150, 90);
    let (w2, t2, _b2) = ssd_window(&tb, &mut client, xrgb(40, 80, 200), 1);
    let (w3, t3, _b3) = ssd_window(&tb, &mut client, xrgb(30, 140, 150), 2);
    let _pointer = mint_pointer(&mut client);
    client.sync();

    // The second window's band: content (25, 53), frame (24, 24,
    // 202, 130), the band's rows [24, 53) — press at (125, 38).
    // The third window's band: rows [48, 77) — press at (201, 51).
    move_to(&tb, 125, 38);
    button(&tb, true);
    client.sync();
    wait_for_on_with(&mut client, "configure", t2.id().as_u32(), 2);
    button(&tb, false);
    client.sync();
    move_to(&tb, 201, 51);
    button(&tb, true);
    client.sync();
    wait_for_on_with(&mut client, "configure", t3.id().as_u32(), 2);
    button(&tb, false);
    client.sync();

    // The third press, back on the second window's band, quickly —
    // the slot holds the *third* window's click: no pair for either
    // (the cross-window press reset the clock — the activation
    // dance's honest proposals may come and go, the maximized flag
    // never appears).
    move_to(&tb, 125, 38);
    button(&tb, true);
    client.sync();
    button(&tb, false);
    client.sync();
    assert_eq!(
        flag_proposals(&client, t2.id().as_u32()),
        0,
        "the cross-window press never pairs with the first window's own click"
    );
    assert_eq!(
        flag_proposals(&client, t3.id().as_u32()),
        0,
        "the third window's pending click died with the second window's press"
    );
    let _ = (w2, w3);
}

/// THE death sweep: the window's destroy takes the pending click with
/// it — a press where the dead window's band was never toggles
/// anything (the band is gone with the route), and the world stays
/// healthy.
#[test]
fn the_destroyed_windows_band_never_pairs() {
    let tb = Testbench::start_with("dbl-death", dbl_config("dt"));
    let mut client = TestClient::connect(&tb.addr);
    let (_filler, w3, t3, _b3, _pointer) = dbl_stage(&tb, &mut client);

    // The first click recorded (the band's middle row).
    move_to(&tb, 201, 51);
    button(&tb, true);
    client.sync();
    wait_for_on_with(&mut client, "configure", t3.id().as_u32(), 2);
    button(&tb, false);
    client.sync();

    // The window dies between the clicks (the surface's own
    // destroy).
    client.conn.destroy(&w3).expect("destroy");
    client.sync();

    // The second press where the band was: the band is gone with the
    // route — the press lands on the filler beneath (or the desktop),
    // never on a dead window's chrome, no proposal to a ghost.
    button(&tb, true);
    client.sync();
    button(&tb, false);
    client.sync();
    assert_eq!(
        count_on(&client, "configure", t3.id().as_u32()),
        2,
        "the dead window's band never pairs — no proposal to a ghost"
    );
    for _ in 0..3 {
        client.sync();
    }
    assert_eq!(count_on(&client, "close", t3.id().as_u32()), 0);
}
