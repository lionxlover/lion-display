//! The Phase 10 exit criterion: a full client session over the real
//! protocol — handshake, registry, globals, output cascade, buffer
//! exchange through a real memfd pool, atomic commit, presentation
//! feedback (frame_target + presented), and the buffer release fence —
//! green in CI, headless.

#![allow(clippy::too_many_lines)]

mod testbench;

use std::io::Write;

use ldp_core::geometry::Rect;
use ldp_core::wire::Value;
use testbench::*;

/// A memfd pool carrying explicit pixel bytes.
fn pool_bytes(pixels: &[u8]) -> std::os::fd::OwnedFd {
    let fd = lion_compositor::sys::memfd("full-session").expect("memfd");
    let mut file = std::fs::File::from(fd);
    file.write_all(pixels).expect("write pixels");
    file.into()
}

/// 2x2 XRGB8888 (stride 8): red, green / blue, white.
fn quad_pixels() -> Vec<u8> {
    let mut v = Vec::new();
    for word in [0x00FF_0000u32, 0x0000_FF00, 0x0000_00FF, 0x00FF_FFFF] {
        v.extend_from_slice(&word.to_le_bytes());
    }
    v
}

#[test]
fn full_session_including_buffer_exchange_and_presentation() {
    let tb = Testbench::start("full");
    let mut client = TestClient::connect(&tb.addr);

    // ---- registry and globals --------------------------------------
    let _registry = client.conn.registry().expect("registry");
    client.sync();
    let globals: Vec<String> = client
        .events
        .records
        .iter()
        .filter(|r| r.event == "global")
        .filter_map(|r| match &r.args[0] {
            Value::String(s) => Some(s.to_string()),
            _ => None,
        })
        .collect();
    for iface in ["ldp.core.compositor", "ldp.core.shm", "ldp.core.output"] {
        assert!(
            globals.iter().any(|g| g == iface),
            "{iface} must be advertised (got {globals:?})"
        );
    }

    // ---- shm: formats then a real pool ------------------------------
    let shm = client.bind("ldp.core.shm");
    client.wait_until(|c| c.records.iter().filter(|r| r.event == "format").count() >= 2);
    let formats: Vec<u32> = client
        .events
        .records
        .iter()
        .filter(|r| r.event == "format")
        .filter_map(|r| match r.args[0] {
            Value::Uint32(f) => Some(f),
            _ => None,
        })
        .collect();
    assert!(formats.contains(&0x3432_5258), "XRGB8888 advertised");
    assert!(formats.contains(&0x3432_5241), "ARGB8888 advertised");

    let pool = create_pool(&mut client, &shm, pool_bytes(&quad_pixels()), 16);
    let buffer = create_buffer(&mut client, &pool, 0, 2, 2, 8, 0x3432_5258);
    client.sync();

    // ---- compositor + surface ---------------------------------------
    let compositor = client.bind("ldp.core.compositor");
    let surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("create_surface");
    client.sync();

    // ---- output cascade ----------------------------------------------
    let output = client.bind("ldp.core.output");
    client.wait_until(|c| c.records.iter().any(|r| r.event == "name"));
    let geometry = client.last("geometry");
    match (&geometry.args[0], &geometry.args[1]) {
        (Value::Int32(x), Value::Int32(y)) => assert_eq!((*x, *y), (0, 0)),
        other => panic!("geometry position args {other:?}"),
    }
    let name = client.last("name");
    match &name.args[0] {
        Value::String(s) => assert_eq!(&**s, "eDP-1"),
        other => panic!("name arg {other:?}"),
    }
    let vrr = client.last("vrr");
    match (&vrr.args[0], &vrr.args[1]) {
        (Value::Uint32(min), Value::Uint32(max)) => {
            assert_eq!((*min, *max), (48_000, 144_000));
        }
        other => panic!("vrr args {other:?}"),
    }
    let modes = client.events_of("mode");
    assert!(modes.len() >= 2, "the panel's mode list replays");

    // ---- the presentation contract -----------------------------------
    // frame(1) → frame_target: a deadline on the 60 Hz grid, with the
    // refresh interval and a positive budget.
    frame(&mut client, &surface, 1);
    client.wait_until(|c| c.records.iter().any(|r| r.event == "frame_target"));
    let target = client.last("frame_target");
    let (frame_id, deadline, refresh, budget, mode) = match (
        &target.args[0],
        &target.args[1],
        &target.args[2],
        &target.args[3],
        &target.args[4],
    ) {
        (
            Value::Uint64(frame_arg),
            Value::Ts(target_arg),
            Value::Uint64(refresh_arg),
            Value::Uint64(budget_arg),
            Value::Enum(mode_arg),
        ) => (
            *frame_arg,
            *target_arg,
            *refresh_arg,
            *budget_arg,
            *mode_arg,
        ),
        other => panic!("frame_target args {other:?}"),
    };
    assert_eq!(frame_id, 1);
    assert_eq!(refresh, 16_666_666, "60 Hz exact from the mock timeline");
    assert_eq!(mode, 1, "vsync mode");
    let now = tb.now_ns();
    assert!(
        deadline > now && budget <= deadline,
        "deadline {deadline} must be ahead of now {now}"
    );

    // ---- buffer exchange: attach, damage, commit ---------------------
    attach(&mut client, &surface, &buffer);
    damage(&mut client, &surface, &[Rect::new(0, 0, 2, 2)]);
    commit(&mut client, &surface, 0x1234);

    // committed echoes the cookie; presented lands at the next vblank.
    client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "committed" && r.args[0] == Value::Uint32(0x1234))
    });
    client.wait_until(|c| c.records.iter().any(|r| r.event == "presented"));
    let presented = client.last("presented");
    let (pframe, ts, prefresh) = match (&presented.args[0], &presented.args[1], &presented.args[2])
    {
        (Value::Uint64(frame_arg), Value::Ts(ts_arg), Value::Uint64(refresh_arg)) => {
            (*frame_arg, *ts_arg, *refresh_arg)
        }
        other => panic!("presented args {other:?}"),
    };
    assert_eq!(pframe, 1, "the registered frame presented");
    assert_eq!(
        ts, 33_333_332,
        "the flip lands on the 60 Hz vblank grid (16,666,666 + period)"
    );
    assert_eq!(prefresh, 16_666_666);

    // enter_output followed the mapping.
    let entered = client.events_of("enter_output");
    assert_eq!(entered.len(), 1, "exactly one enter_output");
    match &entered[0].args[0] {
        Value::Object(Some(o)) => assert_eq!(o.as_u32(), output.id().as_u32()),
        other => panic!("enter_output arg {other:?}"),
    }

    // ---- pixel truth: the client's quad is on the scanout ------------
    let scanout = tb.scanout();
    assert_eq!(scanout.len(), 1920 * 1080);
    assert_eq!(scanout[0], 0xFFFF_0000, "top-left red");
    assert_eq!(scanout[1], 0xFF00_FF00, "top-right green");
    assert_eq!(scanout[1920], 0xFF00_00FF, "bottom-left blue");
    assert_eq!(scanout[1921], 0xFFFF_FFFF, "bottom-right white");
    assert_eq!(scanout[2], 0xFF00_0000, "the desktop behind is black");
    assert_eq!(scanout[1919], 0xFF00_0000);

    // ---- clean teardown ----------------------------------------------
    destroy(&mut client, &buffer);
    destroy(&mut client, &surface);
    client.wait_until(|c| c.records.iter().any(|r| r.event == "destroyed"));
    drop(client);

    // The scene drains: no routes survive the session.
    client_gone_scene_drains(&tb);
}

fn client_gone_scene_drains(tb: &Testbench) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        let drained = tb.world(|w| w.scene.routes.is_empty() && w.outboxes.is_empty());
        if drained {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("the scene did not drain after the client disconnected");
}
