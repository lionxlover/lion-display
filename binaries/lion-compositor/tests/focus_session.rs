//! Phase 51's exit criterion (the focus and the view): the frozen
//! vocabulary the states arm's follow-ons named, served end to end
//! through the real protocol —
//!
//! * the **focus truth**: the `activated` state bit (frozen at index
//!   3 since v1, the machine's flag waiting for the focus-driven
//!   activation line) rides real proposals — the press sets it, the
//!   departure clears it, the frontmost window takes the keys when
//!   the holder leaves (the macOS key-window doctrine), and the
//!   grace window absorbs the transition (a client draining a
//!   drag's serials is never punished by a focus move it never
//!   asked for);
//! * the **view switch**: `shell.switch_workspace` moves the *viewed*
//!   space (the taskbar's line — the machine was built, the wire had
//!   no request), every window's visibility syncing through the
//!   set_workspace arm's own machinery, the keys landing on the
//!   frontmost window of the newly-viewed space, the hidden spaces'
//!   frame requests parking (App Nap's own seam), and every shell
//!   binder learning the actual space (`workspace_switched` — the
//!   taskbar that asked, the wallpaper daemon that did not);
//! * the **clamp**: an out-of-range switch lands on the last space,
//!   the reply's honest answer (never a silent miss).

mod testbench;

use ldp_client::Proxy;
use ldp_core::geometry::Rect;
use ldp_core::wire::Value;
use testbench::*;

use lion_compositor::server::CompositorConfig;
use lion_compositor::shell::{DockMode, ShellConfig};

/// The XRGB8888 fourcc ("XR24").
const XR24: u32 = 0x3432_5258;
/// The mock output: 1920x1080@60.
const OUT_W: usize = 1920;
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

/// Wait for one event of one name on one object.
fn wait_for_on(client: &mut TestClient, name: &str, target: u32) {
    client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == name && r.target == target)
    });
}

/// Wait until the toplevel object has recorded `n` configures.
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
    let fd = lion_compositor::sys::memfd("focus-pool").expect("memfd");
    {
        let mut file = std::fs::File::from(fd.try_clone().expect("clone"));
        file.write_all(pixels).expect("write pool");
    }
    fd
}

/// The focus/view configs: the dock serving (the shell active — the
/// cascade steps the windows apart, each press point sitting in its
/// window's un-shared corner; the dock's own ink lives in the bottom
/// band, away from every oracle point).
fn focus_config(tag: &str) -> CompositorConfig {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    CompositorConfig {
        shell: ShellConfig {
            dock: DockMode::Auto,
            ..ShellConfig::default()
        },
        socket: format!("lion-focus-{tag}-{n}-{}", std::process::id()),
        ..CompositorConfig::default()
    }
}

/// Map one plain window at its creation position. Returns (surface,
/// buffer, pool).
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

/// `toplevel.set_workspace(index)`.
fn set_workspace(client: &mut TestClient, toplevel: &Proxy, index: u32) {
    client
        .conn
        .send_request(toplevel, "set_workspace", vec![Value::Uint32(index)])
        .expect("set_workspace");
}

/// `shell.switch_workspace(index)` — the view switch (Phase 51).
fn switch_workspace(client: &mut TestClient, shell: &Proxy, index: u32) {
    client
        .conn
        .send_request(shell, "switch_workspace", vec![Value::Uint32(index)])
        .expect("switch_workspace");
}

/// The world's machine view of one toplevel's wanted states.
fn wanted_states(tb: &Testbench, surface: &Proxy, toplevel: &Proxy) -> ldp_shell::ToplevelStates {
    tb.world(|w| {
        for route in w.scene.routes.values() {
            if route.surface_obj.as_u32() == surface.id().as_u32() {
                return w
                    .toplevels
                    .entry(route.client.as_u32(), toplevel.id().as_u32())
                    .expect("the host entry")
                    .machine
                    .wanted();
            }
        }
        panic!("the surface has no route");
    })
}

/// The frame id of one recorded `frame_target` / `frame_dropped`.
fn frame_id(record: &Recorded) -> u64 {
    match record.args.first() {
        Some(Value::Uint64(id)) => *id,
        other => panic!("frame id argument is not a uint64: {other:?}"),
    }
}

/// THE focus narrative: the press sets the bit, the second window's
/// press moves it (the old holder clears, the new one sets — one
/// proposal each, the keyboard enter/leave riding the same
/// transitions), and the machine's wanted states agree with the wire.
#[test]
fn the_press_drives_the_activated_bit_between_windows() {
    let tb = Testbench::start_with("focus-press", focus_config("pr"));
    let mut client = TestClient::connect(&tb.addr);
    let shell = client.bind("ldp.shell.shell");
    let seat = client.bind("ldp.input.seat");
    client
        .conn
        .create_object(&seat, "get_pointer", vec![])
        .expect("pointer");

    // Two windows: A at the cascade's origin, B one step down-right
    // (the step is 24 — the press points sit in each window's
    // un-shared corner).
    let plum = xrgb(120, 40, 160);
    let (wa, _ba, _pa) = map_window(&tb, &mut client, plum, 300, 200);
    let ta = get_toplevel(&mut client, &shell, &wa);
    let (wb, _bb, _pb) = map_window(&tb, &mut client, xrgb(40, 120, 90), 300, 200);
    let tbb = get_toplevel(&mut client, &shell, &wb);
    client.sync();

    // The press on A's un-shared corner (the pointer starts at the
    // origin): A activates.
    tb.world_mut(|w| {
        w.queue_input(
            ldp_input::device::DeviceClass::Mouse,
            vec![ldp_input::normalizer::InputEvent::PointerMotion { dx: 10, dy: 10 }],
        );
        w.pump_input();
        w.queue_input(
            ldp_input::device::DeviceClass::Mouse,
            vec![
                ldp_input::normalizer::InputEvent::Button {
                    button: 0x110,
                    pressed: true,
                },
                ldp_input::normalizer::InputEvent::Button {
                    button: 0x110,
                    pressed: false,
                },
            ],
        );
        let routed = w.pump_input();
        assert!(routed > 0, "the press routed");
    });
    client.sync();
    wait_for_configures(&mut client, ta.id().as_u32(), 2);
    let a_on = last_on(&client, "configure", ta.id().as_u32());
    assert!(states_of(a_on).test(3), "A carries the activated flag");
    assert!(
        wanted_states(&tb, &wa, &ta).activated(),
        "the machine agrees"
    );

    // The press on B's un-shared corner (past A's right edge): B
    // sets, A clears.
    tb.world_mut(|w| {
        w.queue_input(
            ldp_input::device::DeviceClass::Mouse,
            vec![ldp_input::normalizer::InputEvent::PointerMotion { dx: 300, dy: 90 }],
        );
        w.pump_input();
        w.queue_input(
            ldp_input::device::DeviceClass::Mouse,
            vec![
                ldp_input::normalizer::InputEvent::Button {
                    button: 0x110,
                    pressed: true,
                },
                ldp_input::normalizer::InputEvent::Button {
                    button: 0x110,
                    pressed: false,
                },
            ],
        );
        let routed = w.pump_input();
        assert!(routed > 0, "the second press routed");
    });
    client.sync();
    wait_for_configures(&mut client, ta.id().as_u32(), 3);
    let a_off = last_on(&client, "configure", ta.id().as_u32());
    assert!(
        !states_of(a_off).test(3),
        "A's flag cleared when B took the keys"
    );
    wait_for_configures(&mut client, tbb.id().as_u32(), 2);
    let b_on = last_on(&client, "configure", tbb.id().as_u32());
    assert!(states_of(b_on).test(3), "B carries the flag");
    assert!(
        !wanted_states(&tb, &wa, &ta).activated(),
        "A's machine cleared"
    );
    assert!(
        wanted_states(&tb, &wb, &tbb).activated(),
        "B's machine agrees"
    );
}

/// THE focus holder's death: the frontmost window that remains takes
/// the keys (the promotion doctrine) — the dying window's own leave
/// and proposal emit nothing (its route and entry are gone).
#[test]
fn the_dying_focus_holder_promotes_the_frontmost() {
    let tb = Testbench::start_with("focus-death", focus_config("dt"));
    let mut client = TestClient::connect(&tb.addr);
    let shell = client.bind("ldp.shell.shell");
    let seat = client.bind("ldp.input.seat");
    client
        .conn
        .create_object(&seat, "get_pointer", vec![])
        .expect("pointer");

    // Two windows; the press focuses B (the topmost, second-mapped —
    // the press sits in B's un-shared corner, past A's right edge).
    let (wa, _ba, _pa) = map_window(&tb, &mut client, xrgb(120, 40, 160), 300, 200);
    let ta = get_toplevel(&mut client, &shell, &wa);
    let (wb, _bb, _pb) = map_window(&tb, &mut client, xrgb(40, 120, 90), 300, 200);
    let tbb = get_toplevel(&mut client, &shell, &wb);
    client.sync();
    tb.world_mut(|w| {
        w.queue_input(
            ldp_input::device::DeviceClass::Mouse,
            vec![ldp_input::normalizer::InputEvent::PointerMotion { dx: 310, dy: 100 }],
        );
        w.pump_input();
        w.queue_input(
            ldp_input::device::DeviceClass::Mouse,
            vec![
                ldp_input::normalizer::InputEvent::Button {
                    button: 0x110,
                    pressed: true,
                },
                ldp_input::normalizer::InputEvent::Button {
                    button: 0x110,
                    pressed: false,
                },
            ],
        );
        w.pump_input();
    });
    client.sync();
    wait_for_configures(&mut client, tbb.id().as_u32(), 2);
    assert!(states_of(last_on(&client, "configure", tbb.id().as_u32())).test(3));

    // B dies: the keys land on A (the frontmost remaining), A's bit
    // sets, and nothing is emitted for the dead window.
    client.conn.destroy(&tbb).expect("destroy toplevel");
    client.conn.destroy(&wb).expect("destroy surface");
    client.sync();
    wait_for_configures(&mut client, ta.id().as_u32(), 2);
    let promoted = last_on(&client, "configure", ta.id().as_u32());
    assert!(states_of(promoted).test(3), "A took the keys when B died");
    assert!(
        wanted_states(&tb, &wa, &ta).activated(),
        "the machine agrees"
    );
}

/// THE view switch: `shell.switch_workspace` moves the viewed space
/// (the taskbar's line). The windows' own states never change — no
/// window is proposed anything — while the ink flips (the old space's
/// leaves, the new space's shows), the sticky window shows on both,
/// the hidden space's frame requests park, the keys land on the
/// frontmost window of the newly-viewed space, and every shell binder
/// learns the actual space.
#[test]
#[allow(clippy::too_many_lines)] // one session, one narrative
fn the_view_switch_flips_visibility_and_reports_the_actual() {
    let tb = Testbench::start_with("focus-view", focus_config("vw"));
    let mut client = TestClient::connect(&tb.addr);
    let shell = client.bind("ldp.shell.shell");
    // The output bind: the enter/leave truth only fires for outputs
    // the client holds objects for (the visibility pass's doctrine).
    client.bind("ldp.core.output");
    let seat = client.bind("ldp.input.seat");
    client
        .conn
        .create_object(&seat, "get_pointer", vec![])
        .expect("pointer");

    // S (sticky, mapped first — the bottom), A on space 0, B on
    // space 3 (mapped last — the top of the routing order). The
    // cascade steps 24: A's un-shared band is x<48 y>100, B's is
    // x>=324, S's is x<24 y<24.
    let plum = xrgb(120, 40, 160);
    let (ws, _bs, _ps) = map_window(&tb, &mut client, xrgb(200, 160, 40), 200, 100);
    let ts = get_toplevel(&mut client, &shell, &ws);
    let (wa, _ba, _pa) = map_window(&tb, &mut client, plum, 300, 200);
    let ta = get_toplevel(&mut client, &shell, &wa);
    let (wb, _bb, _pb) = map_window(&tb, &mut client, xrgb(40, 120, 90), 300, 200);
    let tbb = get_toplevel(&mut client, &shell, &wb);
    client.sync();
    set_workspace(&mut client, &tbb, 3);
    wait_for_on(&mut client, "workspace_changed", tbb.id().as_u32());
    client
        .conn
        .send_request(&ts, "set_sticky", vec![Value::Bool(true)])
        .expect("set_sticky");
    wait_for_configures(&mut client, ts.id().as_u32(), 2);
    client.sync();

    // The press focuses A (over its un-shared band).
    tb.world_mut(|w| {
        w.queue_input(
            ldp_input::device::DeviceClass::Mouse,
            vec![ldp_input::normalizer::InputEvent::PointerMotion { dx: 30, dy: 150 }],
        );
        w.pump_input();
        w.queue_input(
            ldp_input::device::DeviceClass::Mouse,
            vec![
                ldp_input::normalizer::InputEvent::Button {
                    button: 0x110,
                    pressed: true,
                },
                ldp_input::normalizer::InputEvent::Button {
                    button: 0x110,
                    pressed: false,
                },
            ],
        );
        w.pump_input();
    });
    client.sync();
    wait_for_configures(&mut client, ta.id().as_u32(), 2);
    assert!(states_of(last_on(&client, "configure", ta.id().as_u32())).test(3));

    // The baseline: A's and S's ink (S sticky, B hidden on 3).
    let baseline = tb.scanout();
    assert_eq!(px(&baseline, 30, 150)[0], 120, "A's ink");
    assert_eq!(px(&baseline, 10, 10)[0], 200, "the sticky window's ink");
    assert_eq!(px(&baseline, 330, 100)[0], 0, "B hidden on space 3");

    // A's frame registration (the park's subject).
    let frames_before = client.events_of("frame_target").len();
    let _ = frames_before;
    client
        .conn
        .send_request(&wa, "frame", vec![Value::Uint64(7)])
        .expect("frame");
    client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "frame_target" && frame_id(r) == 7)
    });

    // The sticky window's own count never moves (a window's state
    // does not change with the view — only the keys' holders get
    // their focus proposals; the baseline below).
    let configs_s = client
        .events
        .records
        .iter()
        .filter(|r| r.event == "configure" && r.target == ts.id().as_u32())
        .count();
    switch_workspace(&mut client, &shell, 3);
    wait_for_on(&mut client, "workspace_switched", shell.id().as_u32());
    let switched = last_on(&client, "workspace_switched", shell.id().as_u32());
    assert_eq!(switched.args[0], Value::Uint32(3), "the actual space");

    client.sync();
    for _ in 0..3 {
        client.sync();
    }
    let configs_s_after = client
        .events
        .records
        .iter()
        .filter(|r| r.event == "configure" && r.target == ts.id().as_u32())
        .count();
    assert_eq!(
        configs_s, configs_s_after,
        "the sticky window's own state never moved"
    );

    // The ink flipped: A left, B shows, S stays (sticky everywhere).
    let view3 = tb.scanout();
    assert_eq!(px(&view3, 30, 150)[0], 0, "A left with its space");
    assert_eq!(px(&view3, 330, 100)[0], 40, "B shows with its space");
    assert_eq!(px(&view3, 10, 10)[0], 200, "the sticky window stayed");

    // The hidden space's frame requests park (App Nap's own seam):
    // no frame_target for A's live registration while hidden.
    assert!(
        !client
            .events
            .records
            .iter()
            .any(|r| r.event == "frame_target" && frame_id(r) == 8),
        "no new target while hidden"
    );
    client
        .conn
        .send_request(&wa, "frame", vec![Value::Uint64(8)])
        .expect("frame");
    for _ in 0..4 {
        client.sync();
    }
    assert!(
        !client
            .events
            .records
            .iter()
            .any(|r| r.event == "frame_target" && frame_id(r) == 8),
        "the hidden surface must not receive a frame_target"
    );

    // The keys followed the view: B (the frontmost homed on space 3)
    // activated, A cleared — the focus truth riding the same sweep.
    wait_for_configures(&mut client, ta.id().as_u32(), 3);
    assert!(
        !states_of(last_on(&client, "configure", ta.id().as_u32())).test(3),
        "A released the keys with its space"
    );
    wait_for_configures(&mut client, tbb.id().as_u32(), 3);
    assert!(
        states_of(last_on(&client, "configure", tbb.id().as_u32())).test(3),
        "B took the keys with its space"
    );
    assert!(
        !wanted_states(&tb, &ws, &ts).activated(),
        "the sticky window did not steal the keys (its home is 0)"
    );

    // The switch back: A's ink restores, A re-activates, B clears.
    switch_workspace(&mut client, &shell, 0);
    wait_for_n_of(&mut client, "workspace_switched", shell.id().as_u32(), 2);
    client.sync();
    let back = tb.scanout();
    assert_eq!(px(&back, 30, 150)[0], 120, "A returned with the view");
    assert_eq!(px(&back, 10, 10)[0], 200, "the sticky window, always");
    assert_eq!(px(&back, 330, 100)[0], 0, "B left with its space");
    wait_for_configures(&mut client, ta.id().as_u32(), 4);
    assert!(
        states_of(last_on(&client, "configure", ta.id().as_u32())).test(3),
        "A re-took the keys"
    );
    wait_for_configures(&mut client, tbb.id().as_u32(), 4);
    assert!(
        !states_of(last_on(&client, "configure", tbb.id().as_u32())).test(3),
        "B released them"
    );
    // The parked frame answers with the view (the registration is
    // still live — the surface never died).
    client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "frame_target" && frame_id(r) == 8)
    });
}

/// THE clamp and the broadcast: an out-of-range switch lands on the
/// last space (the count is 4 — spaces 0..3), the reply's honest
/// answer; and a *second* client that bound the shell global learns
/// the switch too (the wallpaper daemon's line — the broadcast rides
/// its outbox, the next wake drains it).
#[test]
fn the_switch_clamps_and_broadcasts_to_every_binder() {
    let tb = Testbench::start_with("focus-clamp", focus_config("cl"));
    let mut taskbar = TestClient::connect(&tb.addr);
    let shell = taskbar.bind("ldp.shell.shell");

    // The wallpaper daemon: a second client, its own shell bind (it
    // never asks for anything — it only listens).
    let mut daemon = TestClient::connect(&tb.addr);
    let daemon_shell = daemon.bind("ldp.shell.shell");
    daemon.sync();
    wait_for_on(&mut daemon, "workspace_count", daemon_shell.id().as_u32());
    assert_eq!(
        last_on(&daemon, "workspace_count", daemon_shell.id().as_u32()).args[0],
        Value::Uint32(4),
        "the daemon learned the count at bind"
    );

    // The clamp: 99 lands on space 3.
    switch_workspace(&mut taskbar, &shell, 99);
    wait_for_on(&mut taskbar, "workspace_switched", shell.id().as_u32());
    let clamped = last_on(&taskbar, "workspace_switched", shell.id().as_u32());
    assert_eq!(
        clamped.args[0],
        Value::Uint32(3),
        "the clamp's honest answer (the count is 4)"
    );

    // The daemon learned it too — the broadcast (its outbox, its next
    // wake).
    wait_for_on(
        &mut daemon,
        "workspace_switched",
        daemon_shell.id().as_u32(),
    );
    let told = last_on(&daemon, "workspace_switched", daemon_shell.id().as_u32());
    assert_eq!(
        told.args[0],
        Value::Uint32(3),
        "the daemon that never asked still knows"
    );

    // The switch to the same space is idempotent on the wire (the
    // seat's truth did not change — no event for a no-op).
    let events_before = taskbar
        .events
        .records
        .iter()
        .filter(|r| r.event == "workspace_switched")
        .count();
    switch_workspace(&mut taskbar, &shell, 3);
    taskbar.sync();
    let events_after = taskbar
        .events
        .records
        .iter()
        .filter(|r| r.event == "workspace_switched")
        .count();
    assert_eq!(
        events_before, events_after,
        "a switch to the viewed space is a no-op"
    );
}

/// Wait until `n` events of one name have arrived on one object.
fn wait_for_n_of(client: &mut TestClient, name: &str, target: u32, n: usize) {
    client.wait_until(|c| {
        c.records
            .iter()
            .filter(|r| r.event == name && r.target == target)
            .count()
            >= n
    });
}
