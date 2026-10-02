//! Phase 39's exit criteria — the fresh frame, end to end over the real
//! wire: the emission-path coalescing (the giants' latency tuning,
//! served), the discrete barrier, and the VRR-aware presentation
//! clock.
//!
//! * **The flood collapses to the freshest sample**: sixteen device
//!   batches parked between display frames deliver ONE `motion` (the
//!   latest coordinates, at the first batch's slot), every
//!   `relative_motion` delta, one `frame` terminator — a 1000 Hz
//!   device costs a frame-cadenced client one wake, not sixteen.
//! * **The click freezes its own sample**: a button between two
//!   samples delivers both motions with the click between them — the
//!   click's context is the position it rode with (the X11
//!   flush-before-button doctrine), never a fresher one.
//! * **The VRR-aware presentation clock**: under `--vrr` a
//!   fast-cadence client's flips land at the panel's minimum interval
//!   (the LFC fast end, 6.94 ms on the 48-144 window) and the
//!   `presented` feedback reports exactly that — where the legacy
//!   measurement band rejected anything under half the nominal period
//!   and fell back to reporting the 16.67 ms grid the panel was not
//!   running. Without `--vrr` the same client paces to the fixed grid
//!   and the feedback says so.
//! * **The flood rides the next frame within budget**: the coalesced
//!   emission path changes the delivery, never the frame — the
//!   input→photon measurement still closes within one nominal period.

mod testbench;

use ldp_core::geometry::Rect;
use ldp_core::wire::Value;
use ldp_input::device::DeviceClass;
use ldp_input::normalizer::InputEvent;
use testbench::*;

use lion_compositor::server::CompositorConfig;

/// The XRGB8888 fourcc ("XR24").
const XR24: u32 = 0x3432_5258;
/// A window at the origin: 128x128 opaque.
const WIN: usize = 128;
/// BTN_LEFT (the evdev button code).
const BTN_LEFT: u32 = 0x110;
/// The mock's 48-144 Hz VRR window: the minimum period (144 Hz).
const VRR_MIN: u64 = 6_944_444;
/// The 60 Hz nominal period (the fixed grid).
const NOMINAL: u64 = 16_666_666;

/// A memfd carrying the window bytes (the pool's backing).
fn memfd_of(bytes: &[u8]) -> std::os::fd::OwnedFd {
    use std::io::Write as _;
    let fd = lion_compositor::sys::memfd("latency-session-pool").expect("memfd");
    let mut file = std::fs::File::from(fd);
    file.write_all(bytes).expect("write the pool");
    file.set_len(bytes.len() as u64).expect("size the pool");
    std::os::fd::OwnedFd::from(file)
}

/// A connected client with a mapped 128x128 window at the origin and
/// its pointer object minted — the routing target.
struct Aimed {
    client: TestClient,
    #[allow(dead_code)]
    buffer: ldp_client::Proxy,
    surface: ldp_client::Proxy,
    pointer: ldp_client::Proxy,
}

fn aimed(tb: &Testbench, tag: &str) -> Aimed {
    let mut client = TestClient::connect(&tb.addr);
    let shm = client.bind("ldp.core.shm");
    let word = 0xFF20_2020u32; // opaque dark gray
    let bytes = vec![word.to_le_bytes(); WIN * WIN].concat();
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
    // The mapping commit (the placement lands with it).
    attach(&mut client, &surface, &buffer);
    damage(
        &mut client,
        &surface,
        &[Rect::new(0, 0, WIN as u32, WIN as u32)],
    );
    commit(&mut client, &surface, 1);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while tb.frames() == 0 {
        assert!(
            deadline > std::time::Instant::now(),
            "the compositor never rendered the presented buffer ({tag})"
        );
        client.sync();
    }
    let seat = client.bind("ldp.input.seat");
    let pointer = client
        .conn
        .create_object(&seat, "get_pointer", vec![])
        .expect("pointer");
    client.sync();
    Aimed {
        client,
        buffer,
        surface,
        pointer,
    }
}

/// One device batch through the scripted source (the same pump the
/// real evdev path takes).
fn batch(tb: &Testbench, events: Vec<InputEvent>) {
    tb.world_mut(|w| {
        w.queue_input(DeviceClass::Mouse, events);
        w.pump_input();
    });
}

/// THE exit criterion: sixteen parked batches, one delivered motion —
/// the freshest coordinates, the deltas in full, one terminator.
#[test]
fn the_flood_collapses_to_the_freshest_sample() {
    let tb = Testbench::start("flood");
    let mut a = aimed(&tb, "flood");

    // Sixteen batches between display frames — the client does not
    // drain while they park (the frame-cadenced client's life).
    for _ in 0..16 {
        batch(&tb, vec![InputEvent::PointerMotion { dx: 2, dy: 1 }]);
    }
    // ONE drain — the next wake point.
    a.client
        .wait_until(|c| c.records.iter().any(|r| r.event == "frame"));

    // The pointer-object records, in delivery order.
    let pointer_id = a.pointer.id().as_u32();
    let delivered: Vec<&Recorded> = a
        .client
        .events
        .records
        .iter()
        .filter(|r| r.target == pointer_id)
        .collect();
    let names: Vec<String> = delivered.iter().map(|r| r.event.clone()).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    // The collapse: ONE motion, ONE frame, every delta, and the enter
    // transition first (the router started at the origin — over the
    // window).
    assert_eq!(
        names.iter().filter(|n| **n == "motion").count(),
        1,
        "one motion survives the flood: {names:?}"
    );
    assert_eq!(
        names.iter().filter(|n| **n == "frame").count(),
        1,
        "one frame terminator: {names:?}"
    );
    assert_eq!(
        names.iter().filter(|n| **n == "relative_motion").count(),
        16,
        "every delta delivers in full: {names:?}"
    );
    assert_eq!(names.first(), Some(&"enter"), "the enter leads: {names:?}");
    // The surviving motion carries the FRESHEST coordinates — the
    // router's own position (sixteen accel-filtered (2,1) deltas).
    let motion = delivered
        .iter()
        .find(|r| r.event == "motion")
        .expect("the surviving motion");
    let (x, y) = match &motion.args[..] {
        [Value::Float32(x), Value::Float32(y)] => (x, y),
        other => panic!("the motion carries x + y (got {other:?})"),
    };
    tb.world(|w| {
        let (rx, ry) = w.input_pointer_position();
        assert!(
            (x - rx).abs() < 1.0 && (y - ry).abs() < 1.0,
            "the freshest coordinates: delivered ({x}, {y}), router ({rx}, {ry})"
        );
    });

    // And the flood rode the latency rig within budget — see the
    // dedicated proof below.
}

/// The discrete barrier over the wire: a click between two samples
/// delivers both, with the click between them — the click's context
/// is the position it rode with, never the fresher one.
#[test]
fn the_click_freezes_its_own_sample() {
    let tb = Testbench::start("barrier");
    let mut a = aimed(&tb, "barrier");

    // Enter the window first (a clean slate: the enter transition
    // done, the position known).
    batch(&tb, vec![InputEvent::PointerMotion { dx: 40, dy: 30 }]);
    a.client
        .wait_until(|c| c.records.iter().any(|r| r.event == "enter"));

    // The click's own sample, the click, a fresher sample — all
    // parked undrained.
    batch(&tb, vec![InputEvent::PointerMotion { dx: 4, dy: 4 }]);
    batch(
        &tb,
        vec![InputEvent::Button {
            button: BTN_LEFT,
            pressed: true,
        }],
    );
    batch(&tb, vec![InputEvent::PointerMotion { dx: 8, dy: 8 }]);
    a.client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "button" && r.args.len() == 2 && r.args[1] == Value::Enum(2))
    });

    let pointer_id = a.pointer.id().as_u32();
    let delivered: Vec<&Recorded> = a
        .client
        .events
        .records
        .iter()
        .filter(|r| r.target == pointer_id && r.event != "enter")
        .collect();
    let names: Vec<String> = delivered.iter().map(|r| r.event.clone()).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    // TWO motions — the barrier prevented the collapse across the
    // click — and the click sits between them.
    assert_eq!(
        names.iter().filter(|n| **n == "motion").count(),
        2,
        "the barrier holds both samples: {names:?}"
    );
    let click_at = names
        .iter()
        .position(|n| *n == "button")
        .expect("the click delivered");
    let motions: Vec<usize> = names
        .iter()
        .enumerate()
        .filter(|(_, n)| **n == "motion")
        .map(|(i, _)| i)
        .collect();
    assert!(
        motions[0] < click_at && click_at < motions[1],
        "the click rides between its own sample and the fresher one: {names:?}"
    );
    // The click's own sample kept ITS coordinates (frozen at the
    // barrier): the first motion is the (40+4, 30+4) position.
    let first = delivered
        .iter()
        .find(|r| r.event == "motion")
        .expect("the first motion");
    match &first.args[..] {
        [Value::Float32(x), Value::Float32(y)] => {
            assert!(
                *x >= 43.0 && *x <= 45.0 && *y >= 33.0 && *y <= 35.0,
                "the click's own sample (44, 34): got ({x}, {y})"
            );
        }
        other => panic!("the motion carries x + y (got {other:?})"),
    }
}

/// A committed client driving the panel at its fast end: one surface,
/// eight blind frame cycles (frame_request + commit, no drains — the
/// commits land back-to-back and the flips ride the minimum interval).
fn blind_cycles(a: &mut Aimed, count: u64) {
    for k in 1..=count {
        frame(&mut a.client, &a.surface, k);
        attach(&mut a.client, &a.surface, &a.buffer);
        damage(&mut a.client, &a.surface, &[Rect::new(0, 0, 2, 2)]);
        commit(&mut a.client, &a.surface, u32::try_from(k).unwrap_or(0));
    }
}

/// The refresh argument of a presented record.
fn refresh_of(record: &Recorded) -> u64 {
    match &record.args[2] {
        Value::Uint64(ns) => *ns,
        other => panic!("expected the refresh interval, got {other:?}"),
    }
}

/// THE VRR proof: under `--vrr`, an adaptive-mode surface committing
/// at readiness lands its flips at the panel's minimum interval (the
/// LFC fast end) and the presentation feedback reports exactly that —
/// the interval the legacy measurement band rejected as a duplicate —
/// with the verdict firing at the content's own flip (the opportunity
/// target), never a nominal cell behind it.
#[test]
fn the_vrr_clock_reports_the_fast_end() {
    let config = CompositorConfig {
        socket: format!("lion-it-vrrclock-{}", std::process::id()),
        vrr: true,
        ..CompositorConfig::default()
    };
    let tb = Testbench::start_with("vrr-clock", config);
    let mut a = aimed(&tb, "vrr-clock");
    // The surface opts into the window (the wire's presentation_mode:
    // adaptive) — the operator armed the panel with `--vrr`, the
    // surface asks for the window's pacing.
    a.client
        .conn
        .send_request(&a.surface, "set_presentation_mode", vec![Value::Enum(2)])
        .expect("set_presentation_mode: adaptive");

    blind_cycles(&mut a, 8);
    // The registrations present as the flips cross their (nominal)
    // targets — at the fast cadence that is a few cycles out; three
    // settled verdicts are the proof.
    a.client
        .wait_until(|c| c.records.iter().filter(|r| r.event == "presented").count() >= 3);
    let presented: Vec<&Recorded> = a
        .client
        .events
        .records
        .iter()
        .filter(|r| r.event == "presented")
        .collect();
    assert!(presented.len() >= 3, "the cycles presented");
    for record in &presented {
        assert_eq!(
            refresh_of(record),
            VRR_MIN,
            "the fast end reported honestly (frame {}): {} ns",
            match &record.args[0] {
                Value::Uint64(f) => *f,
                _ => 0,
            },
            refresh_of(record)
        );
    }
}

/// The contrast: without `--vrr` the same client paces to the fixed
/// grid and the feedback reports the nominal cadence — the pacing
/// doctrine, unchanged since Phase 25.
#[test]
fn the_fixed_grid_reports_the_nominal_cadence() {
    let tb = Testbench::start("grid-clock");
    let mut a = aimed(&tb, "grid-clock");

    blind_cycles(&mut a, 8);
    a.client
        .wait_until(|c| c.records.iter().filter(|r| r.event == "presented").count() >= 3);
    let presented: Vec<&Recorded> = a
        .client
        .events
        .records
        .iter()
        .filter(|r| r.event == "presented")
        .collect();
    assert!(presented.len() >= 3, "the cycles presented");
    for record in &presented {
        assert_eq!(
            refresh_of(record),
            NOMINAL,
            "the fixed grid reported honestly: {} ns",
            refresh_of(record)
        );
    }
}

/// The flood never delays the frame: the coalesced emission path
/// changes the delivery, not the render — the input→photon
/// measurement closes within one nominal period plus costs.
#[test]
fn the_flood_rides_the_next_frame_within_budget() {
    let tb = Testbench::start("flood-budget");
    let mut a = aimed(&tb, "flood-budget");

    for _ in 0..16 {
        batch(&tb, vec![InputEvent::PointerMotion { dx: 2, dy: 1 }]);
    }
    // The frame: a fresh commit renders and flips — the flood's
    // freshest sample rides it.
    frame(&mut a.client, &a.surface, 9);
    attach(&mut a.client, &a.surface, &a.buffer);
    damage(&mut a.client, &a.surface, &[Rect::new(0, 0, 2, 2)]);
    commit(&mut a.client, &a.surface, 9);
    a.client
        .wait_until(|c| c.records.iter().any(|r| r.event == "presented"));
    tb.world(|w| {
        let latency = w
            .last_input_photon_ns()
            .expect("the flood's measurement closed");
        assert!(
            latency <= NOMINAL + 2_000_000,
            "the flood rides the next frame within budget: {latency} ns"
        );
    });
}

/// One touch contact in the window at the origin (normalized sensor
/// coordinates, the 1920x1080 panel's space).
fn contact(id: i32, x: f32, y: f32) -> ldp_input::normalizer::TouchContact {
    ldp_input::normalizer::TouchContact {
        id,
        x,
        y,
        x_mm: x * 120.0,
        y_mm: y * 120.0,
        pressure: None,
        touch_major: None,
        touch_minor: None,
        orientation: None,
        tool: None,
    }
}

/// Phase 43: the touch flood collapses **per contact**, end to end
/// over the real wire. A 120 Hz touchscreen parking sixteen
/// two-finger batches between display frames delivers TWO motions —
/// each finger's own freshest sample in its own slot — plus the two
/// `down`s and ONE `frame` terminator: a frame-cadenced client reads
/// the gesture, not the flood, and one finger's sample never rides
/// another finger's slot.
#[test]
fn the_two_finger_flood_collapses_per_contact() {
    let tb = Testbench::start("touchflood");
    let mut a = aimed(&tb, "touchflood");

    // The touch object (the routing target for contact events).
    let seat = a.client.bind("ldp.input.seat");
    let touch = a
        .client
        .conn
        .create_object(&seat, "get_touch", vec![])
        .expect("touch");
    a.client.sync();

    // The gesture: two contacts inside the window at the origin
    // (normalized sensor coordinates, 1920x1080 panel), sixteen
    // batches — the first begins both contacts, the rest move them.
    for i in 0..16u32 {
        let f = i as f32;
        let points = vec![
            contact(1, 0.02 + f * 0.001, 0.05),
            contact(2, 0.05, 0.02 + f * 0.001),
        ];
        let began = if i == 0 { vec![1, 2] } else { Vec::new() };
        tb.world_mut(|w| {
            let time = ldp_core::time::Mono::from_ns(w.now().as_ns());
            w.queue_input(
                DeviceClass::Touchscreen,
                vec![InputEvent::Touch(ldp_input::normalizer::TouchUpdate {
                    time,
                    points,
                    began,
                    ended: Vec::new(),
                    dropped: false,
                })],
            );
            w.pump_input();
        });
    }

    // ONE drain — the next wake point (any touch frame delivers).
    a.client
        .wait_until(|c| c.records.iter().any(|r| r.event == "frame"));

    let touch_id = touch.id().as_u32();
    let delivered: Vec<&Recorded> = a
        .client
        .events
        .records
        .iter()
        .filter(|r| r.target == touch_id)
        .collect();
    let names: Vec<String> = delivered.iter().map(|r| r.event.clone()).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    // The per-contact collapse: two downs (discrete, never dropped),
    // TWO motions (one per contact — the freshest of each), ONE
    // frame. Thirty-two parked motions became two.
    assert_eq!(
        names.iter().filter(|n| **n == "down").count(),
        2,
        "both touch downs deliver: {names:?}"
    );
    assert_eq!(
        names.iter().filter(|n| **n == "motion").count(),
        2,
        "one motion per contact survives the flood: {names:?}"
    );
    assert_eq!(
        names.iter().filter(|n| **n == "frame").count(),
        1,
        "one frame terminator: {names:?}"
    );
    // Each motion carries its OWN contact's id and its OWN freshest
    // coordinates (normalized * 1920x1080 surface space): contact 1's
    // freshest x = (0.02 + 15 * 0.001) * 1920 = 67.2; the oldest
    // would be 0.02 * 1920 = 38.4 — the collapse keeps the freshest.
    let motions: Vec<&Recorded> = delivered
        .into_iter()
        .filter(|r| r.event == "motion")
        .collect();
    let mut by_id: Vec<(u32, f32)> = motions
        .iter()
        .map(|m| match &m.args[..] {
            [Value::Uint32(id), Value::Float32(x), _] => (*id, *x),
            other => panic!("touch motion carries (id, x, y) — got {other:?}"),
        })
        .collect();
    by_id.sort_by_key(|(id, _)| *id);
    assert_eq!(by_id.len(), 2, "two motions, one per contact");
    assert_eq!(by_id[0].0, 1, "contact 1's motion carries its own id");
    assert_eq!(by_id[1].0, 2, "contact 2's motion carries its own id");
    assert!(
        (by_id[0].1 - 67.2).abs() < 3.0,
        "contact 1's freshest x (67.2), not the oldest (38.4): {}",
        by_id[0].1
    );
    assert!(
        by_id[1].1 > 90.0,
        "contact 2's x is its own stream's (0.05 * 1920 = 96), not contact 1's: {}",
        by_id[1].1
    );
}
