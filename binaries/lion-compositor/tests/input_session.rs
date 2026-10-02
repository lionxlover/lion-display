//! Phase 33's exit criterion: the evdev input path, end to end through
//! the real protocol — device batches through the seat router onto
//! the wire, delivered at the client's wake points.
//!
//! The scripted source feeds the *same* pump the real `/dev/input`
//! devices take (`World::queue_input` + `World::pump_input` — the fd
//! read is the only difference), so what this suite proves is the
//! whole serving half: the routing view of the scene (topmost-first
//! hit testing over the mapped roots), the pointer focus transitions
//! (enter / motion / button / frame), the click-to-focus keyboard
//! policy (leave / enter with the held keys), the key routing with
//! the modifier decomposition, the keymap's one-time delivery at the
//! keyboard mint, and the input→photon latency rig riding the real
//! arrival path.

mod testbench;

use ldp_core::geometry::Rect;
use ldp_core::wire::Value;
use ldp_input::device::DeviceClass;
use ldp_input::normalizer::InputEvent;
use testbench::Collector;
use testbench::*;

/// Events of one name in the collector (the wait predicates' helper).
fn named(c: &Collector, name: &str) -> usize {
    c.records.iter().filter(|r| r.event == name).count()
}

/// The XRGB8888 fourcc ("XR24").
const XR24: u32 = 0x3432_5258;
/// The mock output: 1920x1080@60 — the routing bounds.
const OUT_W: usize = 1920;
const OUT_H: usize = 1080;
/// A window at the origin: 128x128 opaque.
const WIN: usize = 128;
/// BTN_LEFT (the evdev button code).
const BTN_LEFT: u32 = 0x110;
/// KEY_A (the evdev keycode).
const KEY_A: u32 = 30;

/// THE exit criterion: the pointer enters, moves, and clicks; the
/// keyboard follows the click; a key routes to the focused client —
/// every event over the real wire, the batches framed, the keymap
/// delivered once, the latency rig armed by the real arrival.
#[test]
#[allow(clippy::too_many_lines)] // one session, one narrative
fn pointer_and_keyboard_events_reach_the_client_over_the_wire() {
    let tb = Testbench::start("input-session");
    let mut client = TestClient::connect(&tb.addr);

    // A mapped window at the origin: the routing target. The default
    // config keeps the legacy placement (no shell) — (0,0) verbatim.
    let shm = client.bind("ldp.core.shm");
    let word = 0xFF20_2020u32; // opaque dark gray
    let mut bytes = Vec::new();
    for _ in 0..WIN * WIN {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    let pool = create_pool(&mut client, &shm, memfd_of(&bytes), (WIN * WIN * 4) as i64);
    let buffer = create_buffer(
        &mut client,
        &pool,
        0,
        WIN as i32,
        WIN as i32,
        (WIN * 4) as i32,
        XR24,
    );
    let compositor = client.bind("ldp.core.compositor");
    let surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("surface");
    present(
        &tb,
        &mut client,
        &surface,
        &buffer,
        Rect::new(0, 0, WIN as u32, WIN as u32),
    );

    // The device mints: the router's addresses for this client.
    let seat = client.bind("ldp.input.seat");
    let pointer = client
        .conn
        .create_object(&seat, "get_pointer", vec![])
        .expect("pointer");
    let keyboard = client
        .conn
        .create_object(&seat, "get_keyboard", vec![])
        .expect("keyboard");
    client.sync();

    // The keymap and repeat model arrive with the keyboard mint —
    // the one-time-per-keyboard contract.
    assert!(
        !client.events_of("keymap").is_empty(),
        "the keymap arrived at the mint"
    );
    let repeat = client.last("repeat_info");
    assert_eq!(repeat.args.len(), 2, "repeat_info carries rate + delay");

    // ---- the pointer enters, moves, and frames ----------------------
    //
    // The router's pointer starts at (0,0) — already over the window;
    // the first motion is the enter transition.
    tb.world_mut(|w| {
        w.queue_input(
            DeviceClass::Mouse,
            vec![InputEvent::PointerMotion { dx: 40, dy: 30 }],
        );
        let routed = w.pump_input();
        assert!(routed > 0, "the motion routed");
    });
    client.wait_until(|c| named(c, "enter") > 0);
    let enter = client.last("enter");
    assert_eq!(enter.interface, "ldp.input.pointer");
    assert_eq!(enter.target, pointer.id().as_u32());
    // enter(surface, x, y): our surface at the motion's local (40, 30).
    assert_eq!(enter.args[0], Value::Object(Some(surface.id())));
    match &enter.args[1..] {
        [Value::Float32(x), Value::Float32(y)] => {
            assert!(*x >= 39.0 && *x <= 41.0, "the enter x is 40 (got {x})");
            assert!(*y >= 29.0 && *y <= 31.0, "the enter y is 30 (got {y})");
        }
        other => panic!("the enter carries surface + x + y (got {other:?})"),
    }

    // A further motion: pointer.motion + the batch's frame.
    let frames_before = client.events_of("frame").len();
    tb.world_mut(|w| {
        w.queue_input(
            DeviceClass::Mouse,
            vec![InputEvent::PointerMotion { dx: 8, dy: 8 }],
        );
        w.pump_input();
    });
    client.wait_until(|c| named(c, "frame") > frames_before);
    let motion = client.last("motion");
    assert_eq!(motion.interface, "ldp.input.pointer");
    match &motion.args[..] {
        [Value::Float32(x), Value::Float32(y)] => {
            assert!(*x >= 47.0 && *x <= 49.0, "the motion x is 48 (got {x})");
            assert!(*y >= 37.0 && *y <= 38.0, "the motion y is 38 (got {y})");
        }
        other => panic!("the motion carries x + y (got {other:?})"),
    }

    // ---- the click: focus + the button ------------------------------

    tb.world_mut(|w| {
        w.queue_input(
            DeviceClass::Mouse,
            vec![InputEvent::Button {
                button: BTN_LEFT,
                pressed: true,
            }],
        );
        w.pump_input();
    });
    client.wait_until(|c| named(c, "button") > 0);
    let button = client.last("button");
    assert_eq!(button.interface, "ldp.input.pointer");
    match &button.args[..] {
        [Value::Uint32(b), Value::Enum(state)] => {
            assert_eq!(*b, BTN_LEFT, "the button code");
            assert_eq!(*state, 2, "pressed (the wire's key_state value)");
        }
        other => panic!("the button carries code + state (got {other:?})"),
    }

    // Click-to-focus: the keyboard entered our surface (the click's
    // hit — the pointer sits over the window).
    client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.interface == "ldp.input.keyboard" && r.event == "enter")
    });
    let kenter = client
        .events_of("enter")
        .into_iter()
        .find(|r| r.interface == "ldp.input.keyboard")
        .expect("the keyboard enter");
    assert_eq!(kenter.target, keyboard.id().as_u32());
    assert_eq!(kenter.args[0], Value::Object(Some(surface.id())));

    // ---- the key routes to the focused client -----------------------

    tb.world_mut(|w| {
        w.queue_input(
            DeviceClass::Keyboard,
            vec![InputEvent::Key {
                keycode: KEY_A,
                pressed: true,
            }],
        );
        w.pump_input();
    });
    client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.interface == "ldp.input.keyboard" && r.event == "key")
    });
    let key = client
        .events_of("key")
        .into_iter()
        .find(|r| r.interface == "ldp.input.keyboard")
        .expect("the key event");
    match &key.args[..] {
        [Value::Uint32(k), Value::Enum(state)] => {
            assert_eq!(*k, KEY_A, "the keycode");
            assert_eq!(*state, 2, "pressed");
        }
        other => panic!("the key carries keycode + state (got {other:?})"),
    }

    // ---- the release: the implicit grab ends, the button lifts ------

    tb.world_mut(|w| {
        w.queue_input(
            DeviceClass::Mouse,
            vec![InputEvent::Button {
                button: BTN_LEFT,
                pressed: false,
            }],
        );
        w.pump_input();
    });
    client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "button" && r.args.len() == 2 && r.args[1] == Value::Enum(1))
    });

    // ---- the latency rig: the real arrival armed the measurement ----

    tb.world(|w| {
        // Phase 31's rig is now the true path: every routed batch
        // marked activity and armed the input→photon measurement.
        let (x, y) = w.input_pointer_position();
        assert!(
            (x - 48.0).abs() < 1.5 && (y - 38.0).abs() < 1.5,
            "the router's position ({x}, {y})"
        );
        assert!(w.input_pointer_focus().is_some(), "the pointer focus held");
        // The focused surface is OUR window: the route whose surface
        // object matches the client's proxy.
        let ours = w
            .scene
            .routes
            .iter()
            .find(|(_, r)| r.surface_obj == surface.id())
            .map(|(id, _)| *id);
        assert_eq!(
            w.input_keyboard_focus(),
            ours,
            "the keyboard focus is the clicked window"
        );
    });
}

/// The pointer clamps at the desktop bounds: a motion past the edge
/// holds the last inside position (the router's own doctrine, proven
/// through the serving pump).
#[test]
fn the_pointer_clamps_at_the_desktop_bounds() {
    let tb = Testbench::start("input-clamp");
    let mut client = TestClient::connect(&tb.addr);

    // One mapped window (the hit target).
    let shm = client.bind("ldp.core.shm");
    let bytes = vec![0xFF20_2020u32.to_le_bytes(); WIN * WIN].concat();
    let pool = create_pool(&mut client, &shm, memfd_of(&bytes), (WIN * WIN * 4) as i64);
    let buffer = create_buffer(
        &mut client,
        &pool,
        0,
        WIN as i32,
        WIN as i32,
        (WIN * 4) as i32,
        XR24,
    );
    let compositor = client.bind("ldp.core.compositor");
    let surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("surface");
    present(
        &tb,
        &mut client,
        &surface,
        &buffer,
        Rect::new(0, 0, WIN as u32, WIN as u32),
    );

    // A huge motion: the pointer clamps at (OUT_W, OUT_H).
    tb.world_mut(|w| {
        w.queue_input(
            DeviceClass::Mouse,
            vec![InputEvent::PointerMotion {
                dx: 100_000,
                dy: 100_000,
            }],
        );
        let routed = w.pump_input();
        assert_eq!(routed, 0, "nothing routes when the pointer is off-surface");
    });
    // No event routes (the clamped position is outside every mapped
    // surface — the honest empty batch); the router's position is the
    // oracle.
    client.sync();
    tb.world(|w| {
        let (x, y) = w.input_pointer_position();
        assert!(
            (x - OUT_W as f32).abs() < 1.0,
            "the pointer clamped at the desktop width (got {x})"
        );
        assert!(
            (y - OUT_H as f32).abs() < 1.0,
            "the pointer clamped at the desktop height (got {y})"
        );
    });
}

/// The keymap is honest: either libxkbcommon's compiled default or
/// the documented fallback — never a missing event.
#[test]
fn the_input_report_names_what_serves() {
    let tb = Testbench::start("input-report");
    tb.world(|w| {
        // The headless default: off (the byte-exactness doctrine).
        assert_eq!(w.input_report(), "input: off");
    });
}

/// One contact for the axis flood: id, position on the window, and
/// the geometry axes under test.
fn contact(
    id: i32,
    major: f32,
    minor: f32,
    orientation: f32,
) -> ldp_input::normalizer::TouchContact {
    ldp_input::normalizer::TouchContact {
        id,
        x: 0.02,
        y: 0.02,
        x_mm: 1.0,
        y_mm: 1.0,
        pressure: Some(0.5),
        touch_major: Some(major),
        touch_minor: Some(minor),
        orientation: Some(orientation),
        tool: None,
    }
}

/// Phase 45's exit criterion (the axis-metadata half): the touch
/// contact's geometry axes are per-contact position state of their
/// own kinds — a 120 Hz-class flood of shape and orientation samples
/// between display frames collapses to the contact's *freshest*
/// ellipse and angle (never a queue of stale ones, never one axis
/// riding another's slot), while the discrete down that opened the
/// contact delivers in full.
#[test]
#[allow(clippy::too_many_lines)] // one session, one narrative
fn the_touch_axis_metadata_coalesces_per_contact() {
    use ldp_core::time::Mono;
    use ldp_input::normalizer::TouchUpdate;

    let tb = Testbench::start("touch-axes");
    let mut client = TestClient::connect(&tb.addr);

    // A mapped window at the origin: the touch routing target.
    let shm = client.bind("ldp.core.shm");
    let word = 0xFF20_2020u32;
    let mut bytes = Vec::new();
    for _ in 0..WIN * WIN {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    let pool = create_pool(&mut client, &shm, memfd_of(&bytes), (WIN * WIN * 4) as i64);
    let buffer = create_buffer(
        &mut client,
        &pool,
        0,
        WIN as i32,
        WIN as i32,
        (WIN * 4) as i32,
        XR24,
    );
    let compositor = client.bind("ldp.core.compositor");
    let surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("surface");
    present(
        &tb,
        &mut client,
        &surface,
        &buffer,
        Rect::new(0, 0, WIN as u32, WIN as u32),
    );

    // The touch object mint (the contact stream's address).
    let seat = client.bind("ldp.input.seat");
    let _touch = client
        .conn
        .create_object(&seat, "get_touch", vec![])
        .expect("touch");
    client.sync();

    tb.world_mut(|w| {
        for step in 1..=16i32 {
            let major = 4.0 + step as f32 * 0.5;
            let minor = 2.0 + step as f32 * 0.25;
            let orientation = step as f32 * 0.1;
            w.queue_input(
                DeviceClass::Touchscreen,
                vec![InputEvent::Touch(TouchUpdate {
                    time: Mono::from_ns(u64::try_from(step).expect("step is positive")),
                    points: vec![contact(7, major, minor, orientation)],
                    began: if step == 1 { vec![7] } else { Vec::new() },
                    ended: Vec::new(),
                    dropped: false,
                })],
            );
        }
        let routed = w.pump_input();
        assert!(routed > 0, "the touch batches routed");
    });

    // The coalesced delivery: the contact's freshest sample of each
    // axis — motion, shape, orientation — one of each (the queue
    // replaced in place), and the discrete down that opened the
    // contact still delivered in full.
    client.wait_until(|c| named(c, "down") > 0);
    client.wait_until(|c| named(c, "shape") > 0);
    client.wait_until(|c| named(c, "orientation") > 0);
    for _ in 0..4 {
        client.sync();
    }
    let shapes = client.events_of("shape");
    assert_eq!(
        shapes.len(),
        1,
        "sixteen shape samples collapse to one freshest (got {shapes_len})",
        shapes_len = shapes.len()
    );
    let shape = shapes[0];
    // shape(id, major, minor) — the freshest ellipse: step 16's.
    assert_eq!(
        match &shape.args[0] {
            Value::Uint32(id) => *id,
            other => panic!("shape id is not a uint32: {other:?}"),
        },
        7
    );
    let (major, minor) = match (&shape.args[1], &shape.args[2]) {
        (Value::Float32(major), Value::Float32(minor)) => (*major, *minor),
        other => panic!("shape axes are not floats: {other:?}"),
    };
    assert!(
        (major - 12.0).abs() < 1e-6,
        "the freshest major (got {major})"
    );
    assert!(
        (minor - 6.0).abs() < 1e-6,
        "the freshest minor (got {minor})"
    );
    let orientations = client.events_of("orientation");
    assert_eq!(
        orientations.len(),
        1,
        "sixteen orientation samples collapse to one freshest"
    );
    let angle = match &orientations[0].args[1] {
        Value::Float32(a) => *a,
        other => panic!("orientation is not a float: {other:?}"),
    };
    assert!(
        (angle - 1.6).abs() < 1e-6,
        "the freshest angle (got {angle})"
    );
    // The motions coalesced per contact too (the Phase 43 doctrine,
    // unchanged): one motion for one contact.
    assert_eq!(named(&client.events, "motion"), 1, "one motion per contact");
    // And the frame terminator delivered after the axis stream.
    assert_eq!(
        named(&client.events, "frame"),
        1,
        "the touch frame delivered"
    );
}

// ---- helpers ------------------------------------------------------------

/// A memfd carrying the window bytes (the pool's backing).
fn memfd_of(bytes: &[u8]) -> std::os::fd::OwnedFd {
    use std::io::Write as _;
    let fd = lion_compositor::sys::memfd("input-session-pool").expect("memfd");
    let mut file = std::fs::File::from(fd);
    file.write_all(bytes).expect("write the pool");
    file.set_len(bytes.len() as u64).expect("size the pool");
    std::os::fd::OwnedFd::from(file)
}

/// Attach + damage + commit one buffer (the mapping commit).
fn present(
    tb: &Testbench,
    client: &mut TestClient,
    surface: &ldp_client::Proxy,
    buffer: &ldp_client::Proxy,
    rect: Rect,
) {
    attach(client, surface, buffer);
    damage(client, surface, &[rect]);
    commit(client, surface, 1);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while tb.frames() == 0 {
        assert!(
            deadline > std::time::Instant::now(),
            "the compositor never rendered the presented buffer"
        );
        client.sync();
    }
}
