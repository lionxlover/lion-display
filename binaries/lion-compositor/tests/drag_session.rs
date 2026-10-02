//! Phase 50's exit criterion (the operator's hand): the interactive
//! move/resize vocabulary served end to end through the real
//! protocol —
//!
//! * the **move**: `start_move` under the seat's interaction serial
//!   (the freshness gate), every pointer motion moving the window at
//!   the input pump's cadence (server truth — zero configures, the
//!   client never learns positions it cannot use), the keep band
//!   holding the title grip reachable, and the button's release
//!   leaving the geometry standing;
//! * the **resize**: `start_resize` along the engaged edges, the
//!   edge-algebra target proposed through the two-phase commit (the
//!   `resizing` state riding every live proposal), the client's
//!   ack+commit realizing the geometry — the position and the buffer
//!   land together, never tearing — and the release's final proposal
//!   clearing the state;
//! * the **grace window**: a client acking the configure it actually
//!   *saw* (superseded by the pointer's next move) is never at fault
//!   — the machine's bounded acknowledgment window;
//! * the **demotion**: dragging a maximized window by its title
//!   releases the states and restores the floating size under the
//!   pointer's proportional grip (the Windows 11 / macOS title-drag
//!   doctrine);
//! * the **refusals**: the stale serial, the geometry-stated resize,
//!   and the out-of-domain edges — each the typed protocol error the
//!   doctrine names.

mod testbench;

use ldp_client::ClientError;
use ldp_client::Proxy;
use ldp_compositor::surface::Surface;
use ldp_core::geometry::Rect;
use ldp_core::wire::Value;
use testbench::*;

use ldp_input::device::DeviceClass;
use ldp_input::normalizer::InputEvent;
use lion_compositor::server::CompositorConfig;
use lion_compositor::shell::{DockMode, ShellConfig};

/// The XRGB8888 fourcc ("XR24").
const XR24: u32 = 0x3432_5258;
/// The mock output: 1920x1080@60.
const OUT_W: usize = 1920;
/// BTN_LEFT (the evdev button code).
const BTN_LEFT: u32 = 0x110;

/// One XRGB word.
fn xrgb(r: u8, g: u8, b: u8) -> u32 {
    u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b)
}

/// One scanout pixel as an RGB triple.
fn px(words: &[u32], x: usize, y: usize) -> [u32; 4] {
    let v = words[y * OUT_W + x];
    [
        (v >> 16) & 0xFF,
        (v >> 8) & 0xFF,
        v & 0xFF,
        (v >> 24) & 0xFF,
    ]
}

/// Wait until the toplevel object has recorded `n` configures (the
/// count-based doctrine — the wait must fire on the *new* proposal,
/// never one already collected).
fn wait_for_configures(client: &mut TestClient, target: u32, n: usize) {
    client.wait_until(|c| {
        c.records
            .iter()
            .filter(|r| r.event == "configure" && r.target == target)
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

/// The serial of a recorded configure (arg 0).
fn serial_of(record: &Recorded) -> u32 {
    match &record.args[0] {
        Value::Uint32(serial) => *serial,
        other => panic!("configure's serial argument is not a uint32: {other:?}"),
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
    let fd = lion_compositor::sys::memfd("drag-pool").expect("memfd");
    {
        let mut file = std::fs::File::from(fd.try_clone().expect("clone"));
        file.write_all(pixels).expect("write pool");
    }
    fd
}

/// The drag-session configs: the clean 1920x1080 desktop, dock off —
/// the drag narratives' arithmetic (the keep band against the full
/// usable area, the cascade's (24, 24) second step).
fn drag_config(tag: &str) -> CompositorConfig {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    CompositorConfig {
        shell: ShellConfig {
            dock: DockMode::Off,
            ..ShellConfig::default()
        },
        socket: format!("lion-drag-{tag}-{n}-{}", std::process::id()),
        ..CompositorConfig::default()
    }
}

/// Map one plain window at its creation position. Returns
/// (surface, buffer, pool).
fn map_window(
    tb: &Testbench,
    client: &mut TestClient,
    fill: u32,
    w: usize,
    h: usize,
) -> (Proxy, Proxy, Proxy) {
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
    (surface, buffer, pool)
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

/// `toplevel.ack_configure(serial)`.
fn ack_configure(client: &mut TestClient, toplevel: &Proxy, serial: u32) {
    client
        .conn
        .send_request(toplevel, "ack_configure", vec![Value::Uint32(serial)])
        .expect("ack_configure");
}

/// `toplevel.start_move(seat, serial)`.
fn start_move(client: &mut TestClient, toplevel: &Proxy, seat: &Proxy, serial: u32) {
    client
        .conn
        .send_request(
            toplevel,
            "start_move",
            vec![Value::Object(Some(seat.id())), Value::Uint32(serial)],
        )
        .expect("start_move");
}

/// `toplevel.start_resize(seat, serial, edges)` — the wire enum.
fn start_resize(client: &mut TestClient, toplevel: &Proxy, seat: &Proxy, serial: u32, edges: u32) {
    client
        .conn
        .send_request(
            toplevel,
            "start_resize",
            vec![
                Value::Object(Some(seat.id())),
                Value::Uint32(serial),
                Value::Enum(edges),
            ],
        )
        .expect("start_resize");
}

/// One scripted input batch through the same pump the real devices
/// take (the drag's heartbeat).
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

/// The world's bounds of one surface (the tree's own truth).
fn bounds_of(tb: &Testbench, surface: &Proxy) -> Rect {
    tb.world(|w| {
        for (sid, route) in &w.scene.routes {
            if route.surface_obj.as_u32() == surface.id().as_u32() {
                return w
                    .scene
                    .tree
                    .get(*sid)
                    .map_or(Rect::new(0, 0, 0, 0), Surface::last_bounds);
            }
        }
        panic!("the surface has no route");
    })
}

/// The world's live-drag introspection (the tests' oracle): the mode
/// and the window-start rect, or `None` when no drag is live.
fn drag_view(tb: &Testbench) -> Option<(String, Rect)> {
    tb.world(|w| {
        w.drags.live().map(|d| {
            let mode = match d.mode {
                lion_compositor::shell::DragMode::Move => "move".to_owned(),
                lion_compositor::shell::DragMode::Resize(_) => "resize".to_owned(),
            };
            (mode, d.window_start)
        })
    })
}

/// THE move narrative: the press retargets nothing that matters here,
/// `start_move` mints the grip under the seat's serial, every motion
/// batch moves the window by exactly the pointer's travel (server
/// truth — the toplevel never sees a configure), the keep band holds
/// the grip when the drag tries to leave the desktop, and the release
/// leaves the geometry standing.
#[test]
#[allow(clippy::too_many_lines)] // one session, one narrative
fn the_move_drives_the_window_at_the_pumps_cadence() {
    let tb = Testbench::start_with("drag-move", drag_config("mv"));
    let mut client = TestClient::connect(&tb.addr);
    let shell = client.bind("ldp.shell.shell");
    let seat = client.bind("ldp.input.seat");
    // The pointer mint: the routed events the press and motions
    // deliver (the wake contract's own narrative).
    client
        .conn
        .create_object(&seat, "get_pointer", vec![])
        .expect("pointer");

    let teal = xrgb(30, 140, 150);
    let (w1, _b1, _p1) = map_window(&tb, &mut client, teal, 300, 200);
    let t1 = get_toplevel(&mut client, &shell, &w1);
    client.sync();
    // The handshake drew seat serial 1 (the freshness gate's value).
    wait_for_configures(&mut client, t1.id().as_u32(), 1);
    let serial = serial_of(last_on(&client, "configure", t1.id().as_u32()));
    assert_eq!(serial, 1);

    // The press on the window (the router's pointer starts at (0,0)
    // — already over it; the click-to-focus fires — Phase 51's focus
    // truth: the press *activates* the window, one configure beyond
    // the handshake — the drag arm follows).
    motion(&tb, 150, 100);
    button(&tb, true);
    client.sync();
    wait_for_configures(&mut client, t1.id().as_u32(), 2);
    assert!(
        states_of(last_on(&client, "configure", t1.id().as_u32())).test(3),
        "the press activated the window (bit 3)"
    );

    // The grip: the serial is fresh, the window is mapped and
    // visible. The drag host records the mode and the anchor rect.
    start_move(&mut client, &t1, &seat, serial);
    client.sync();
    let (mode, anchor) = drag_view(&tb).expect("the drag is live");
    assert_eq!(mode, "move");
    assert_eq!(
        (anchor.x, anchor.y),
        (0, 0),
        "the window started at the cascade's origin"
    );
    assert_eq!((anchor.w, anchor.h), (300, 200));

    // The motion: the window follows by exactly the pointer's travel
    // (server truth, applied now — the damage engine repaints both
    // ends). No configure ever: the toplevel's count stays at two —
    // the handshake and the press's activation; a move adds none.
    motion(&tb, 120, 60);
    assert_eq!(position_of(&tb, &w1), (120, 60));
    motion(&tb, 30, -20);
    assert_eq!(position_of(&tb, &w1), (150, 40));
    let configures = client
        .events
        .records
        .iter()
        .filter(|r| r.event == "configure" && r.target == t1.id().as_u32())
        .count();
    assert_eq!(
        configures, 2,
        "a move never proposes — position is server truth"
    );

    // The pointer's own desktop clamp is the drag's first boundary
    // (the router never leaves the outputs' union): a huge leftward
    // motion pins the pointer at x=0, the window follows to
    // pointer-travel it can reach. The keep band itself is the
    // second boundary — the dedicated test below pins it.
    motion(&tb, -5000, 0);
    assert_eq!(position_of(&tb, &w1), (-150, 40));

    // The pixel truths at a settled position: back inside, the window
    // follows the pointer exactly (the pointer-oracle discipline —
    // the window's travel IS the pointer's travel, whatever the
    // router's accel curve did with the batch).
    motion(&tb, 500, 0);
    let (px1, py1) = pointer_of(&tb);
    let landed = ((px1 - 150.0) as i32, (py1 - 100.0) as i32);
    assert_eq!(position_of(&tb, &w1), landed);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let before = tb.frames();
    while tb.frames() <= before {
        assert!(deadline > std::time::Instant::now(), "never rendered");
        client.sync();
    }
    let words = tb.scanout();
    let (lx, ly) = (landed.0.max(0) as usize, landed.1.max(0) as usize);
    assert_eq!(
        px(&words, lx + 10, ly + 10)[1],
        140,
        "the window's ink followed"
    );
    assert_eq!(px(&words, lx, ly)[1], 140, "the window's origin");
    assert_eq!(
        px(&words, lx.saturating_sub(100), ly + 10)[1],
        0,
        "the desktop it left behind"
    );

    // The release: the grip lifts, the geometry stands, the drag is
    // gone.
    button(&tb, false);
    client.sync();
    assert!(drag_view(&tb).is_none(), "the drag ended");
    assert_eq!(position_of(&tb, &w1), landed);
}

/// THE keep band: when the pointer's own desktop clamp lets the
/// window hang past the area's edge (the grip started deep inside),
/// the drag's keep band holds `48` physical px of the window
/// reachable — the operator can always find the title grip again
/// (the DWM/macOS "you cannot lose your window" doctrine).
#[test]
fn the_keep_band_holds_the_grip_reachable_over_the_wire() {
    let tb = Testbench::start_with("drag-keep", drag_config("kp"));
    let mut client = TestClient::connect(&tb.addr);
    let shell = client.bind("ldp.shell.shell");
    let seat = client.bind("ldp.input.seat");
    client
        .conn
        .create_object(&seat, "get_pointer", vec![])
        .expect("pointer");

    let teal = xrgb(30, 140, 150);
    let (w1, _b1, _p1) = map_window(&tb, &mut client, teal, 300, 200);
    let t1 = get_toplevel(&mut client, &shell, &w1);
    client.sync();
    wait_for_configures(&mut client, t1.id().as_u32(), 1);
    let serial = serial_of(last_on(&client, "configure", t1.id().as_u32()));

    // The grip starts deep inside (the pointer at (400, 300) — two
    // modest steps, the router's accel curve untripped).
    motion(&tb, 200, 150);
    motion(&tb, 200, 150);
    assert_eq!(pointer_of(&tb), (400.0, 300.0));
    button(&tb, true);
    client.sync();
    start_move(&mut client, &t1, &seat, serial);
    client.sync();

    // The huge leftward drag: the pointer pins at the desktop's left
    // edge (its own clamp), the raw position would hang 400 px off —
    // the keep band holds 48 px reachable.
    motion(&tb, -500, 0);
    assert_eq!(pointer_of(&tb), (0.0, 300.0), "the pointer's own clamp");
    assert_eq!(
        position_of(&tb, &w1),
        (-252, 0),
        "the keep band's own floor"
    );

    button(&tb, false);
    client.sync();
    assert!(drag_view(&tb).is_none());
}

/// The world's router pointer (the tests' oracle — the drag's own
/// arithmetic source).
fn pointer_of(tb: &Testbench) -> (f32, f32) {
    tb.world(lion_compositor::World::input_pointer_position)
}

/// THE resize narrative: the bottom-right grip drives the edge
/// algebra through the two-phase commit — the `resizing` state rides
/// the live proposal, the client acks and commits the resized buffer,
/// the geometry realizes with the position unchanged (the top-left
/// anchors), and the release's final proposal clears the state.
#[test]
#[allow(clippy::too_many_lines)] // one session, one narrative
fn the_resize_proposes_and_realizes_the_edge_algebra() {
    let tb = Testbench::start_with("drag-resize", drag_config("rs"));
    let mut client = TestClient::connect(&tb.addr);
    let shell = client.bind("ldp.shell.shell");
    let seat = client.bind("ldp.input.seat");
    client
        .conn
        .create_object(&seat, "get_pointer", vec![])
        .expect("pointer");

    let teal = xrgb(30, 140, 150);
    let (w1, _b1, _p1) = map_window(&tb, &mut client, teal, 200, 100);
    let t1 = get_toplevel(&mut client, &shell, &w1);
    client.sync();
    wait_for_configures(&mut client, t1.id().as_u32(), 1);
    let serial = serial_of(last_on(&client, "configure", t1.id().as_u32()));

    motion(&tb, 100, 50);
    button(&tb, true);
    client.sync();
    // Phase 51 — the focus truth: the press activates the window
    // (configure #2, bit 3); the resize's proposals follow it.
    wait_for_configures(&mut client, t1.id().as_u32(), 2);
    assert!(
        states_of(last_on(&client, "configure", t1.id().as_u32())).test(3),
        "the press activated the window (bit 3)"
    );

    // The bottom-right grip (wire 8).
    start_resize(&mut client, &t1, &seat, serial, 8);
    client.sync();
    let (mode, anchor) = drag_view(&tb).expect("the drag is live");
    assert_eq!(mode, "resize");
    assert_eq!((anchor.w, anchor.h), (200, 100));

    // The motion: the target grows by the travel, proposed through
    // the two-phase commit.
    motion(&tb, 150, 80);
    wait_for_configures(&mut client, t1.id().as_u32(), 3);
    let proposal = last_on(&client, "configure", t1.id().as_u32());
    let p_serial = serial_of(proposal);
    assert!(states_of(proposal).test(5), "the resizing flag (bit 5)");
    assert!(states_of(proposal).test(3), "the activation rides along");
    assert_eq!(proposal.args[2], Value::Uint32(350));
    assert_eq!(proposal.args[3], Value::Uint32(180));

    // The two-phase realize: ack, the resized buffer, the commit —
    // the geometry lands with the position the anchor holds (the
    // top-left never moved).
    ack_configure(&mut client, &t1, p_serial);
    let shm = client.bind("ldp.core.shm");
    let pixels: Vec<u8> = std::iter::repeat(teal.to_le_bytes())
        .take(350 * 180)
        .flatten()
        .collect();
    let pool = create_pool(
        &mut client,
        &shm,
        pool_bytes(&pixels),
        (350 * 180 * 4) as i64,
    );
    let buffer = create_buffer(&mut client, &pool, 0, 350, 180, 350 * 4, XR24);
    let before = tb.frames();
    attach(&mut client, &w1, &buffer);
    damage(&mut client, &w1, &[Rect::new(0, 0, 350, 180)]);
    commit(&mut client, &w1, 2);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while tb.frames() <= before {
        assert!(deadline > std::time::Instant::now(), "never rendered");
        client.sync();
    }
    assert_eq!((position_of(&tb, &w1)), (0, 0), "the top-left anchors");
    assert_eq!(
        (bounds_of(&tb, &w1).w, bounds_of(&tb, &w1).h),
        (350, 180),
        "the realized size"
    );

    // The pixel truth: the resized window's ink at the far corner.
    let words = tb.scanout();
    assert_eq!(px(&words, 345, 175)[1], 140, "the grown corner");
    assert_eq!(px(&words, 355, 185)[1], 0, "the desktop beyond");

    // The release: the final proposal clears the state, carries the
    // last size; the client acks (the commit's own frame — the
    // geometry already stands).
    button(&tb, false);
    wait_for_configures(&mut client, t1.id().as_u32(), 4);
    let final_proposal = last_on(&client, "configure", t1.id().as_u32());
    assert!(
        !states_of(final_proposal).test(5),
        "the resizing flag clears"
    );
    assert_eq!(final_proposal.args[2], Value::Uint32(350));
    assert_eq!(final_proposal.args[3], Value::Uint32(180));
    let f_serial = serial_of(final_proposal);
    ack_configure(&mut client, &t1, f_serial);
    attach(&mut client, &w1, &buffer);
    damage(&mut client, &w1, &[Rect::new(0, 0, 350, 180)]);
    commit(&mut client, &w1, 3);
    client.sync();
    assert!(drag_view(&tb).is_none(), "the drag ended");
    assert_eq!(position_of(&tb, &w1), (0, 0));
}

/// THE left-edge resize: the engaged edges follow the pointer, the
/// opposite corner anchors — the origin *moves* with the grip, and
/// the realize lands the position and the buffer together (never
/// tearing).
#[test]
fn the_left_edge_resize_moves_the_origin() {
    let tb = Testbench::start_with("drag-left", drag_config("lf"));
    let mut client = TestClient::connect(&tb.addr);
    let shell = client.bind("ldp.shell.shell");
    let seat = client.bind("ldp.input.seat");
    client
        .conn
        .create_object(&seat, "get_pointer", vec![])
        .expect("pointer");

    let plum = xrgb(120, 40, 160);
    let (w1, _b1, _p1) = map_window(&tb, &mut client, plum, 200, 100);
    let t1 = get_toplevel(&mut client, &shell, &w1);
    client.sync();
    wait_for_configures(&mut client, t1.id().as_u32(), 1);
    let serial = serial_of(last_on(&client, "configure", t1.id().as_u32()));

    motion(&tb, 100, 50);
    button(&tb, true);
    client.sync();
    // Phase 51 — the focus truth: the press activates the window
    // (configure #2, bit 3).
    wait_for_configures(&mut client, t1.id().as_u32(), 2);
    assert!(
        states_of(last_on(&client, "configure", t1.id().as_u32())).test(3),
        "the press activated the window (bit 3)"
    );
    // The top-left grip (wire 5).
    start_resize(&mut client, &t1, &seat, serial, 5);
    client.sync();

    // The motion: both engaged edges follow (the window shrinks, the
    // origin moves to (60, 40)).
    motion(&tb, 60, 40);
    wait_for_configures(&mut client, t1.id().as_u32(), 3);
    let proposal = last_on(&client, "configure", t1.id().as_u32());
    assert_eq!(proposal.args[2], Value::Uint32(140));
    assert_eq!(proposal.args[3], Value::Uint32(60));

    // The realize: the position and the buffer land together — the
    // origin at (200-140, 100-60) = (60, 40) (the bottom-right
    // anchors).
    let p_serial = serial_of(proposal);
    ack_configure(&mut client, &t1, p_serial);
    let shm = client.bind("ldp.core.shm");
    let pixels: Vec<u8> = std::iter::repeat(plum.to_le_bytes())
        .take(140 * 60)
        .flatten()
        .collect();
    let pool = create_pool(
        &mut client,
        &shm,
        pool_bytes(&pixels),
        (140 * 60 * 4) as i64,
    );
    let buffer = create_buffer(&mut client, &pool, 0, 140, 60, 140 * 4, XR24);
    let before = tb.frames();
    attach(&mut client, &w1, &buffer);
    damage(&mut client, &w1, &[Rect::new(0, 0, 140, 60)]);
    commit(&mut client, &w1, 2);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while tb.frames() <= before {
        assert!(deadline > std::time::Instant::now(), "never rendered");
        client.sync();
    }
    assert_eq!(
        position_of(&tb, &w1),
        (60, 40),
        "the origin moved with the grip"
    );
    assert_eq!((bounds_of(&tb, &w1).w, bounds_of(&tb, &w1).h), (140, 60));

    // The release ends the drag (the final proposal's ack is the
    // client's own business; the geometry stands).
    button(&tb, false);
    client.sync();
    assert!(drag_view(&tb).is_none());
}

/// THE grace window: a client acking the configure it actually *saw*
/// — superseded by the pointer's next move before the ack arrived —
/// is never at fault (the connection lives, the acked size realizes
/// with the client's committed buffer, the fresher proposal stays
/// live for the next ack).
#[test]
fn the_superseded_serial_is_acknowledgeable() {
    let tb = Testbench::start_with("drag-grace", drag_config("gr"));
    let mut client = TestClient::connect(&tb.addr);
    let shell = client.bind("ldp.shell.shell");
    let seat = client.bind("ldp.input.seat");
    client
        .conn
        .create_object(&seat, "get_pointer", vec![])
        .expect("pointer");

    let teal = xrgb(30, 140, 150);
    let (w1, _b1, _p1) = map_window(&tb, &mut client, teal, 200, 100);
    let t1 = get_toplevel(&mut client, &shell, &w1);
    client.sync();
    wait_for_configures(&mut client, t1.id().as_u32(), 1);
    let serial = serial_of(last_on(&client, "configure", t1.id().as_u32()));

    motion(&tb, 100, 50);
    button(&tb, true);
    client.sync();
    // Phase 51 — the press's activation (configure #2) sits in the
    // grace window the drag's own proposals park beside.
    wait_for_configures(&mut client, t1.id().as_u32(), 2);
    start_resize(&mut client, &t1, &seat, serial, 8);
    client.sync();

    // Two motions, each delivered before the next (the roundtrip
    // between them is the frame-cadence client's own rhythm).
    motion(&tb, 50, 30);
    wait_for_configures(&mut client, t1.id().as_u32(), 3);
    let first = serial_of(last_on(&client, "configure", t1.id().as_u32()));
    motion(&tb, 50, 30);
    wait_for_configures(&mut client, t1.id().as_u32(), 4);
    let second = serial_of(last_on(&client, "configure", t1.id().as_u32()));
    assert!(second > first);

    // The graceful ack: the *first* configure's serial, superseded by
    // the second — accepted, never the protocol error.
    ack_configure(&mut client, &t1, first);
    for _ in 0..3 {
        client.sync();
    }
    // And the live one acks normally after it.
    ack_configure(&mut client, &t1, second);
    for _ in 0..3 {
        client.sync();
    }

    // The realize with the acked (fresher) size: the connection's
    // whole narrative survives.
    let shm = client.bind("ldp.core.shm");
    let pixels: Vec<u8> = std::iter::repeat(teal.to_le_bytes())
        .take(300 * 160)
        .flatten()
        .collect();
    let pool = create_pool(
        &mut client,
        &shm,
        pool_bytes(&pixels),
        (300 * 160 * 4) as i64,
    );
    let buffer = create_buffer(&mut client, &pool, 0, 300, 160, 300 * 4, XR24);
    let before = tb.frames();
    attach(&mut client, &w1, &buffer);
    damage(&mut client, &w1, &[Rect::new(0, 0, 300, 160)]);
    commit(&mut client, &w1, 2);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while tb.frames() <= before {
        assert!(deadline > std::time::Instant::now(), "never rendered");
        client.sync();
    }
    assert_eq!((bounds_of(&tb, &w1).w, bounds_of(&tb, &w1).h), (300, 160));

    button(&tb, false);
    client.sync();
}

/// THE demotion: dragging a maximized window by its title releases
/// the states, restores the floating size under the pointer's
/// proportional grip, and carries the drag on from there — the
/// Windows 11 / macOS title-drag doctrine, never tearing (the size
/// realizes at the client's ack+commit cadence).
#[test]
#[allow(clippy::too_many_lines)] // one session, one narrative
fn dragging_a_maximized_window_demotes_under_the_pointer() {
    let tb = Testbench::start_with("drag-demote", drag_config("dm"));
    let mut client = TestClient::connect(&tb.addr);
    let shell = client.bind("ldp.shell.shell");
    let seat = client.bind("ldp.input.seat");
    client
        .conn
        .create_object(&seat, "get_pointer", vec![])
        .expect("pointer");

    let teal = xrgb(30, 140, 150);
    let (w1, _b1, _p1) = map_window(&tb, &mut client, teal, 200, 100);
    let t1 = get_toplevel(&mut client, &shell, &w1);
    client.sync();
    wait_for_configures(&mut client, t1.id().as_u32(), 1);
    let serial = serial_of(last_on(&client, "configure", t1.id().as_u32()));

    // The maximize narrative's prelude: engage, ack, realize.
    client
        .conn
        .send_request(&t1, "maximize", vec![])
        .expect("maximize");
    wait_for_configures(&mut client, t1.id().as_u32(), 2);
    let max_serial = serial_of(last_on(&client, "configure", t1.id().as_u32()));
    ack_configure(&mut client, &t1, max_serial);
    let shm = client.bind("ldp.core.shm");
    let big: Vec<u8> = std::iter::repeat(teal.to_le_bytes())
        .take(1920 * 1080)
        .flatten()
        .collect();
    let big_pool = create_pool(
        &mut client,
        &shm,
        pool_bytes(&big),
        (1920 * 1080 * 4) as i64,
    );
    let big_buffer = create_buffer(&mut client, &big_pool, 0, 1920, 1080, 1920 * 4, XR24);
    let before = tb.frames();
    attach(&mut client, &w1, &big_buffer);
    damage(&mut client, &w1, &[Rect::new(0, 0, 1920, 1080)]);
    commit(&mut client, &w1, 2);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while tb.frames() <= before {
        assert!(deadline > std::time::Instant::now(), "never rendered");
        client.sync();
    }
    assert_eq!(position_of(&tb, &w1), (0, 0), "maximized at the origin");

    // The grip: the pointer at the maximized window's exact middle
    // (twelve modest steps — the router's accel curve untripped),
    // then the title drag.
    for _ in 0..12 {
        motion(&tb, 80, 45);
    }
    let (px0, py0) = pointer_of(&tb);
    assert_eq!((px0, py0), (960.0, 540.0));
    button(&tb, true);
    client.sync();
    // Phase 51 — the focus truth: the press activated the window
    // (configure #3, beyond the handshake and the maximize).
    wait_for_configures(&mut client, t1.id().as_u32(), 3);
    assert!(
        states_of(last_on(&client, "configure", t1.id().as_u32())).test(3),
        "the press activated the window (bit 3)"
    );
    start_move(&mut client, &t1, &seat, serial);
    client.sync();

    // The demotion: the states released, the floating size restored
    // (the 200x100 the engagement displaced), the window detached
    // under the pointer's *proportional* grip — the middle of the big
    // window maps to the middle of the small one: (960-100, 540-50).
    // The activation survives the demotion (the dragged window keeps
    // the keys — Phase 51's own truth).
    wait_for_configures(&mut client, t1.id().as_u32(), 4);
    let demoted = last_on(&client, "configure", t1.id().as_u32());
    assert!(
        !states_of(demoted).test(0) && !states_of(demoted).test(1),
        "the geometry states released"
    );
    assert!(states_of(demoted).test(3), "still the key window");
    assert_eq!(demoted.args[2], Value::Uint32(200));
    assert_eq!(demoted.args[3], Value::Uint32(100));
    // The anchor's own arithmetic, derived from the pointer oracle
    // (the same truth the server computed from).
    let expect_anchor = (
        (px0 - (px0 / 1920.0) * 200.0).round() as i32,
        (py0 - (py0 / 1080.0) * 100.0).round() as i32,
    );
    assert_eq!(expect_anchor, (860, 490));
    assert_eq!(
        position_of(&tb, &w1),
        (860, 490),
        "the anchored detach (the pointer keeps its proportional grip)"
    );

    // The drag carries on: the deltas move the (still-big) window —
    // the shrink realizes at the client's cadence, never tearing. The
    // window follows the pointer exactly (the pointer-oracle
    // discipline: whatever the router's accel curve did with the
    // batch, the window's travel IS the pointer's travel).
    motion(&tb, 40, 30);
    let (px1, py1) = pointer_of(&tb);
    let landed = (860 + (px1 - px0) as i32, 490 + (py1 - py0) as i32);
    assert_eq!(
        position_of(&tb, &w1),
        landed,
        "the window follows the pointer"
    );

    // The realize: the restore-size buffer, the dragged position.
    let d_serial = serial_of(demoted);
    ack_configure(&mut client, &t1, d_serial);
    let small: Vec<u8> = std::iter::repeat(teal.to_le_bytes())
        .take(200 * 100)
        .flatten()
        .collect();
    let small_pool = create_pool(
        &mut client,
        &shm,
        pool_bytes(&small),
        (200 * 100 * 4) as i64,
    );
    let small_buffer = create_buffer(&mut client, &small_pool, 0, 200, 100, 200 * 4, XR24);
    let before = tb.frames();
    attach(&mut client, &w1, &small_buffer);
    damage(&mut client, &w1, &[Rect::new(0, 0, 200, 100)]);
    commit(&mut client, &w1, 3);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while tb.frames() <= before {
        assert!(deadline > std::time::Instant::now(), "never rendered");
        client.sync();
    }
    assert_eq!(position_of(&tb, &w1), landed, "the dragged position");
    assert_eq!((bounds_of(&tb, &w1).w, bounds_of(&tb, &w1).h), (200, 100));

    // The pixel truth: the shrunk window under the pointer's grip.
    let words = tb.scanout();
    let (lx, ly) = landed;
    assert_eq!(
        px(&words, (lx + 40) as usize, (ly + 20) as usize)[1],
        140,
        "the grip's ink"
    );
    assert_eq!(
        px(&words, (lx - 10).max(0) as usize, (ly - 10).max(0) as usize)[1],
        0,
        "the desktop the big window left"
    );

    // The release ends the drag.
    button(&tb, false);
    client.sync();
    assert!(drag_view(&tb).is_none());
}

/// THE refusals: the stale serial, the geometry-stated resize, and
/// the out-of-domain edges — each the typed protocol error the
/// doctrine names (each on its own connection: the error kills the
/// session, the doctrine).
#[test]
fn the_drag_doctrines_refusals_are_typed() {
    let tb = Testbench::start_with("drag-refuse", drag_config("rf"));

    // The stale serial: a fresh connection, a mapped window, a serial
    // the seat never issued (0 — the clock drew 1 at the handshake).
    let mut stale = TestClient::connect(&tb.addr);
    let shell = stale.bind("ldp.shell.shell");
    let seat = stale.bind("ldp.input.seat");
    let (w1, _b1, _p1) = map_window(&tb, &mut stale, xrgb(30, 140, 150), 200, 100);
    let t1 = get_toplevel(&mut stale, &shell, &w1);
    stale.sync();
    start_move(&mut stale, &t1, &seat, 0);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match stale.conn.roundtrip(&mut stale.events) {
            Err(ClientError::ServerError { code, .. }) => {
                assert_eq!(
                    code,
                    ldp_core::error::ErrorCode::InvalidState,
                    "the stale serial is the freshness gate's own refusal"
                );
                break;
            }
            Err(other) => panic!("expected invalid_state, got {other:?}"),
            Ok(()) => assert!(
                std::time::Instant::now() < deadline,
                "the server never refused the stale serial"
            ),
        }
    }

    // The geometry-stated resize: maximize first (the states own the
    // size), then the grip.
    let mut maxed = TestClient::connect(&tb.addr);
    let shell = maxed.bind("ldp.shell.shell");
    let seat = maxed.bind("ldp.input.seat");
    let (w2, _b2, _p2) = map_window(&tb, &mut maxed, xrgb(200, 40, 40), 150, 90);
    let t2 = get_toplevel(&mut maxed, &shell, &w2);
    maxed.sync();
    maxed
        .conn
        .send_request(&t2, "maximize", vec![])
        .expect("maximize");
    maxed.sync();
    start_resize(&mut maxed, &t2, &seat, 1, 8);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match maxed.conn.roundtrip(&mut maxed.events) {
            Err(ClientError::ServerError { code, .. }) => {
                assert_eq!(
                    code,
                    ldp_core::error::ErrorCode::InvalidState,
                    "the geometry states own the size"
                );
                break;
            }
            Err(other) => panic!("expected invalid_state, got {other:?}"),
            Ok(()) => assert!(
                std::time::Instant::now() < deadline,
                "the server never refused the geometry-stated resize"
            ),
        }
    }

    // The out-of-domain edges (wire 9 — the enum carries 1..=8).
    let mut edges = TestClient::connect(&tb.addr);
    let shell = edges.bind("ldp.shell.shell");
    let seat = edges.bind("ldp.input.seat");
    let (w3, _b3, _p3) = map_window(&tb, &mut edges, xrgb(40, 80, 200), 150, 90);
    let t3 = get_toplevel(&mut edges, &shell, &w3);
    edges.sync();
    edges
        .conn
        .send_request(
            &t3,
            "start_resize",
            vec![
                Value::Object(Some(seat.id())),
                Value::Uint32(1),
                Value::Enum(9),
            ],
        )
        .expect("start_resize");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match edges.conn.roundtrip(&mut edges.events) {
            Err(ClientError::ServerError { code, .. }) => {
                assert_eq!(
                    code,
                    ldp_core::error::ErrorCode::OutOfRange,
                    "the edges domain is the enum's own"
                );
                break;
            }
            Err(other) => panic!("expected out_of_range, got {other:?}"),
            Ok(()) => assert!(
                std::time::Instant::now() < deadline,
                "the server never refused the out-of-domain edges"
            ),
        }
    }
}

/// THE death sweep: the dragged window's destroy ends the grip (the
/// drag holds nothing; a later motion on the dead grip moves
/// nothing), and the world stays healthy.
#[test]
fn the_dragged_windows_death_ends_the_grip() {
    let tb = Testbench::start_with("drag-death", drag_config("dd"));
    let mut client = TestClient::connect(&tb.addr);
    let shell = client.bind("ldp.shell.shell");
    let seat = client.bind("ldp.input.seat");
    client
        .conn
        .create_object(&seat, "get_pointer", vec![])
        .expect("pointer");

    let teal = xrgb(30, 140, 150);
    let (w1, _b1, _p1) = map_window(&tb, &mut client, teal, 300, 200);
    let t1 = get_toplevel(&mut client, &shell, &w1);
    client.sync();
    wait_for_configures(&mut client, t1.id().as_u32(), 1);
    let serial = serial_of(last_on(&client, "configure", t1.id().as_u32()));

    motion(&tb, 150, 100);
    button(&tb, true);
    client.sync();
    start_move(&mut client, &t1, &seat, serial);
    client.sync();
    assert!(drag_view(&tb).is_some());

    // The window dies mid-drag (the surface's own destroy).
    client.conn.destroy(&w1).expect("destroy");
    client.sync();
    assert!(drag_view(&tb).is_none(), "the grip died with the window");

    // A late motion moves nothing (the drag is gone; the router's
    // position updates, no geometry follows).
    motion(&tb, 100, 100);
    client.sync();
    assert!(drag_view(&tb).is_none());
    // The world stays healthy: the sync round-trips.
    for _ in 0..3 {
        client.sync();
    }
}
