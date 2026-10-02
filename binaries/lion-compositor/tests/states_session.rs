//! Phase 49's exit criterion (the states arm): the frozen toplevel
//! vocabulary served end to end through the real protocol —
//!
//! * the **handshake**: `get_toplevel` proposes under the seat's
//!   interaction clock (the data family's serial reference), the
//!   shell bind answers `workspace_count`, and the initial configure
//!   carries the CSD insets (the system grid the client reserves);
//! * the **geometry verbs**: `maximize` proposes the workspace fill
//!   (usable minus insets), `fullscreen` the whole output (zero
//!   insets, the output pinned); the client acks, the next commit
//!   realizes — the position completes the placement, the restore
//!   point returns on the un-verb;
//! * the **visibility verbs**: `minimize` hides the window
//!   immediately (the ink leaves the canvas, the frame requests park
//!   — App Nap's own seam — the surface stays alive), `unminimize`
//!   restores the exact bytes; `set_workspace` moves the window
//!   (`workspace_changed` reports the clamped actual, the ink leaves
//!   the non-active space); `set_sticky` shows everywhere;
//! * the **hints**: the bounded strings land (their error codes
//!   honest), the size pair saturates;
//! * the **strict ack**: a stale serial is the protocol error the
//!   two-phase contract names.

mod testbench;

use ldp_client::ClientError;
use ldp_client::Proxy;
use ldp_core::geometry::Rect;
use ldp_core::wire::Value;
use testbench::*;

use ldp_compositor::surface::Surface;
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

/// Wait until the toplevel object has recorded `n` configures (the
/// mint's handshake is the first; every state intent supersedes it
/// with one more — the wait must fire on the *new* proposal, never
/// the handshake already collected).
fn wait_for_configures(client: &mut TestClient, target: u32, n: usize) {
    client.wait_until(|c| {
        c.records
            .iter()
            .filter(|r| r.event == "configure" && r.target == target)
            .count()
            >= n
    });
}

/// Wait until `n` events of one name have arrived on one object (the
/// move narrative's reports accumulate — the wait must fire on the
/// *new* one, never an earlier arrival).
fn wait_for_n(client: &mut TestClient, name: &str, target: u32, n: usize) {
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
    let fd = lion_compositor::sys::memfd("states-pool").expect("memfd");
    {
        let mut file = std::fs::File::from(fd.try_clone().expect("clone"));
        file.write_all(pixels).expect("write pool");
    }
    fd
}

/// The states-arm configs: `dock_off` keeps creation positions (the
/// clean 1920x1080 desktop — the visibility narratives' arithmetic),
/// `dock_on` carves the usable area (the geometry narratives' — the
/// cascade gives the second window a non-origin restore point, and
/// the dock's reservation makes maximize's fill honestly smaller
/// than fullscreen's cover).
fn states_config(tag: &str, dock: DockMode) -> CompositorConfig {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    CompositorConfig {
        shell: ShellConfig {
            dock,
            ..ShellConfig::default()
        },
        socket: format!("lion-states-{tag}-{n}-{}", std::process::id()),
        ..CompositorConfig::default()
    }
}

/// Map one plain window at its creation position (the dock's
/// placement policy owns the position when the dock serves). Returns
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

/// One toplevel state request (the no-argument verbs).
fn state_request(client: &mut TestClient, toplevel: &Proxy, verb: &str) {
    client
        .conn
        .send_request(toplevel, verb, vec![])
        .expect(verb);
}

/// `toplevel.set_workspace(index)`.
fn set_workspace(client: &mut TestClient, toplevel: &Proxy, index: u32) {
    client
        .conn
        .send_request(toplevel, "set_workspace", vec![Value::Uint32(index)])
        .expect("set_workspace");
}

/// `toplevel.ack_configure(serial)`.
fn ack_configure(client: &mut TestClient, toplevel: &Proxy, serial: u32) {
    client
        .conn
        .send_request(toplevel, "ack_configure", vec![Value::Uint32(serial)])
        .expect("ack_configure");
}

/// The serial of a recorded configure (arg 0).
fn serial_of(record: &Recorded) -> u32 {
    match &record.args[0] {
        Value::Uint32(serial) => *serial,
        other => panic!("configure's serial argument is not a uint32: {other:?}"),
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
        panic!("the surface has no route");
    })
}

/// The world's machine view of one toplevel: `(wanted states, title,
/// min/max hints, home space)` — the host's own truths.
fn machine_view(
    tb: &Testbench,
    surface: &Proxy,
    toplevel: &Proxy,
) -> (
    ldp_shell::ToplevelStates,
    String,
    (u32, u32),
    (u32, u32),
    u32,
) {
    tb.world(|w| {
        for (sid, route) in &w.scene.routes {
            if route.surface_obj.as_u32() == surface.id().as_u32() {
                let entry = w
                    .toplevels
                    .entry(route.client.as_u32(), toplevel.id().as_u32())
                    .expect("the host entry");
                let home = w
                    .spaces
                    .space_of(ldp_shell::WindowKey::new(sid.raw()))
                    .expect("the home space");
                return (
                    entry.machine.wanted(),
                    entry.machine.title().to_owned(),
                    entry.machine.min_size(),
                    entry.machine.max_size(),
                    home,
                );
            }
        }
        panic!("the surface has no route");
    })
}

/// THE handshake: the shell bind answers `workspace_count`, the mint
/// proposes under the seat's interaction clock (serial 1 — the one
/// serial the data family's clients reference), the initial
/// configure is the full ten-argument proposal the schema declares
/// (empty states, the client's own size, the CSD insets — the system
/// grid the client reserves, workspace 0, unpinned), and the ack of
/// the handshake is accepted (the strict two-phase includes the
/// mint: a well-behaved client that acks its first configure is
/// never punished).
#[test]
fn the_handshake_proposes_under_the_seat_serial() {
    let tb = Testbench::start_with("states-handshake", states_config("hs", DockMode::Off));
    let mut client = TestClient::connect(&tb.addr);
    let shell = client.bind("ldp.shell.shell");

    // The bind answers the spaces count (the spec's "after
    // binding"): the desktop doctrine's four.
    wait_for_on(&mut client, "workspace_count", shell.id().as_u32());
    let count = last_on(&client, "workspace_count", shell.id().as_u32());
    assert_eq!(count.args[0], Value::Uint32(4));

    // The mint: a surface, a toplevel over it.
    let compositor = client.bind("ldp.core.compositor");
    let surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("surface");
    let toplevel = get_toplevel(&mut client, &shell, &surface);

    wait_for_on(&mut client, "configure", toplevel.id().as_u32());
    let configure = last_on(&client, "configure", toplevel.id().as_u32());
    assert_eq!(
        configure.interface, "ldp.shell.toplevel",
        "the configure is the toplevel's own"
    );
    // The seat's interaction clock issued serial 1 for this mint
    // (the data family's `set_selection` gate references it).
    assert_eq!(serial_of(configure), 1);
    // Empty states, the client's own size.
    assert!(states_of(configure).is_empty());
    assert_eq!(configure.args[2], Value::Uint32(0));
    assert_eq!(configure.args[3], Value::Uint32(0));
    // The CSD insets: the system hit zone on all four sides (5
    // logical px at identity scale — the client lands on the system
    // grid).
    assert_eq!(configure.args[4], Value::Int32(5));
    assert_eq!(configure.args[5], Value::Int32(5));
    assert_eq!(configure.args[6], Value::Int32(5));
    assert_eq!(configure.args[7], Value::Int32(5));
    // Workspace 0, no output pinned.
    assert_eq!(configure.args[8], Value::Uint32(0));
    assert_eq!(configure.args[9], Value::Object(None));

    // The handshake ack is accepted (a well-behaved client that acks
    // its first configure is never punished): the connection lives —
    // `sync` would panic on the disconnect a protocol error would
    // have caused.
    ack_configure(&mut client, &toplevel, 1);
    for _ in 0..3 {
        client.sync();
    }
    let applied = machine_view(&tb, &surface, &toplevel);
    assert!(applied.0 .0.is_empty());
    assert_eq!(applied.4, 0, "the home space");
}

/// THE maximize narrative: the second window (the cascade's
/// non-origin step — its restore point) maximizes onto the workspace
/// fill — the proposal carries the derived content size (usable
/// minus the CSD insets) and the maximized flag, the client acks,
/// and the committing buffer realizes the placement (the position
/// moves to the usable origin; the fill is pixel-proven against both
/// neighbors: the window's own ink inside, the desktop outside the
/// fill).
#[test]
#[allow(clippy::too_many_lines)] // one session, one narrative
fn maximize_proposes_and_realizes_the_workspace_fill() {
    let tb = Testbench::start_with("states-max", states_config("max", DockMode::Auto));
    let mut client = TestClient::connect(&tb.addr);
    let shell = client.bind("ldp.shell.shell");

    // Two windows: the first at the cascade's origin, the second one
    // step down (24, 24) — the restore point the un-verb returns to.
    let (_w1, _b1, _p1) = map_window(&tb, &mut client, xrgb(200, 40, 40), 150, 90);
    let (w2, b2, _p2) = map_window(&tb, &mut client, xrgb(40, 80, 200), 200, 100);
    let t2 = get_toplevel(&mut client, &shell, &w2);
    client.sync();
    assert_eq!(position_of(&tb, &w2), (24, 24), "the cascade's second step");

    // The maximize intent: the proposal (the second configure — the
    // handshake was the first).
    state_request(&mut client, &t2, "maximize");
    wait_for_configures(&mut client, t2.id().as_u32(), 2);
    let proposal = last_on(&client, "configure", t2.id().as_u32());
    let serial = serial_of(proposal);
    assert_eq!(serial, 2, "the machine continues the domain after the mint");
    assert!(states_of(proposal).test(0), "the maximized flag");
    // The workspace fill: usable (1920 x 1080-84) minus the CSD hit
    // zone (10 each way).
    assert_eq!(proposal.args[2], Value::Uint32(1910));
    assert_eq!(proposal.args[3], Value::Uint32(986));
    assert_eq!(proposal.args[8], Value::Uint32(0), "home space 0");

    // The two-phase commit: ack, then the buffer that answers the
    // proposal's size.
    ack_configure(&mut client, &t2, serial);
    let before = tb.frames();
    let shm = client.bind("ldp.core.shm");
    let big_pixels: Vec<u8> = std::iter::repeat(xrgb(40, 80, 200).to_le_bytes())
        .take(1910 * 986)
        .flatten()
        .collect();
    let big_pool = create_pool(
        &mut client,
        &shm,
        pool_bytes(&big_pixels),
        (1910 * 986 * 4) as i64,
    );
    let big_buffer = create_buffer(&mut client, &big_pool, 0, 1910, 986, 1910 * 4, XR24);
    attach(&mut client, &w2, &big_buffer);
    damage(&mut client, &w2, &[Rect::new(0, 0, 1910, 986)]);
    commit(&mut client, &w2, 2);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while tb.frames() <= before {
        assert!(deadline > std::time::Instant::now(), "never rendered");
        client.sync();
    }

    // The realization: the position completes the placement (the
    // usable origin — the buffer's commit is the client's size
    // answer, the position is the server's).
    assert_eq!(position_of(&tb, &w2), (0, 0), "the workspace fill's origin");
    let _ = b2;
    // The pixel truths (away from the dock band): the fill's ink
    // covers the first window entirely (the second window is on
    // top), the desktop shows beyond the fill's right edge.
    let words = tb.scanout();
    assert_eq!(px(&words, 10, 10)[2], 200, "the fill covers the origin");
    assert_eq!(px(&words, 960, 500)[2], 200, "the fill's middle");
    assert_eq!(px(&words, 1905, 500)[2], 200, "the fill's last column");
    assert_eq!(px(&words, 1915, 500)[2], 0, "the desktop beyond the fill");
}

/// THE un-verb: unmaximize proposes the client's own size (0x0 — the
/// client's answer), and the realizing commit returns the window to
/// the restore point the cascade gave it (the position it held when
/// server geometry began).
#[test]
fn unmaximize_restores_the_pre_geometry_position() {
    let tb = Testbench::start_with("states-unmax", states_config("um", DockMode::Auto));
    let mut client = TestClient::connect(&tb.addr);
    let shell = client.bind("ldp.shell.shell");

    let (_w1, _b1, _p1) = map_window(&tb, &mut client, xrgb(200, 40, 40), 150, 90);
    let (w2, b2, _p2) = map_window(&tb, &mut client, xrgb(40, 80, 200), 200, 100);
    let t2 = get_toplevel(&mut client, &shell, &w2);
    client.sync();

    // Maximize + realize (the first geometry engagement captures the
    // restore point).
    state_request(&mut client, &t2, "maximize");
    wait_for_configures(&mut client, t2.id().as_u32(), 2);
    let mut serial = serial_of(last_on(&client, "configure", t2.id().as_u32()));
    ack_configure(&mut client, &t2, serial);
    let shm = client.bind("ldp.core.shm");
    let big_pixels: Vec<u8> = std::iter::repeat(xrgb(40, 80, 200).to_le_bytes())
        .take(1910 * 986)
        .flatten()
        .collect();
    let big_pool = create_pool(
        &mut client,
        &shm,
        pool_bytes(&big_pixels),
        (1910 * 986 * 4) as i64,
    );
    let big_buffer = create_buffer(&mut client, &big_pool, 0, 1910, 986, 1910 * 4, XR24);
    attach(&mut client, &w2, &big_buffer);
    damage(&mut client, &w2, &[Rect::new(0, 0, 1910, 986)]);
    commit(&mut client, &w2, 2);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let before = tb.frames();
    while tb.frames() <= before {
        assert!(deadline > std::time::Instant::now(), "never rendered");
        client.sync();
    }
    assert_eq!(position_of(&tb, &w2), (0, 0), "maximized at the origin");

    // The un-verb: the client's own size, the restore point.
    state_request(&mut client, &t2, "unmaximize");
    wait_for_configures(&mut client, t2.id().as_u32(), 3);
    let release = last_on(&client, "configure", t2.id().as_u32());
    serial = serial_of(release);
    assert!(states_of(release).is_empty(), "the flag releases");
    assert_eq!(release.args[2], Value::Uint32(0), "the client's own size");

    // The realizing commit: the original buffer, the original
    // position.
    ack_configure(&mut client, &t2, serial);
    let before = tb.frames();
    attach(&mut client, &w2, &b2);
    damage(&mut client, &w2, &[Rect::new(0, 0, 200, 100)]);
    commit(&mut client, &w2, 3);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while tb.frames() <= before {
        assert!(deadline > std::time::Instant::now(), "never rendered");
        client.sync();
    }
    assert_eq!(position_of(&tb, &w2), (24, 24), "the restore point");
    // The pixel truth: the first window's ink shows again (the fill
    // released).
    let words = tb.scanout();
    assert_eq!(px(&words, 10, 10)[0], 200, "the first window shows again");
    assert_eq!(px(&words, 200, 100)[2], 200, "the second window's own ink");
}

/// THE fullscreen doctrine: `fullscreen(null)` pins the primary (the
/// client's own bound output object rides the proposal) and proposes
/// the WHOLE output — zero insets, the full mode size — the cover
/// that includes the dock's reservation (maximize fills the usable
/// area; fullscreen covers the output: the pixel oracle at the right
/// edge shows the difference).
#[test]
fn fullscreen_covers_the_whole_output() {
    let tb = Testbench::start_with("states-fs", states_config("fs", DockMode::Auto));
    let mut client = TestClient::connect(&tb.addr);
    let shell = client.bind("ldp.shell.shell");
    let output = client.bind("ldp.core.output");
    client.sync();

    let (w1, _b1, _p1) = map_window(&tb, &mut client, xrgb(200, 40, 40), 150, 90);
    let t1 = get_toplevel(&mut client, &shell, &w1);
    client.sync();

    // The fullscreen intent (null: the current output).
    client
        .conn
        .send_request(&t1, "fullscreen", vec![Value::Object(None)])
        .expect("fullscreen");
    wait_for_configures(&mut client, t1.id().as_u32(), 2);
    let proposal = last_on(&client, "configure", t1.id().as_u32());
    let serial = serial_of(proposal);
    assert!(states_of(proposal).test(1), "the fullscreen flag");
    // The whole output, zero insets.
    assert_eq!(proposal.args[2], Value::Uint32(1920));
    assert_eq!(proposal.args[3], Value::Uint32(1080));
    for i in 4..8 {
        assert_eq!(
            proposal.args[i],
            Value::Int32(0),
            "fullscreen insets are zero"
        );
    }
    // The pin: the client's own bound output object.
    assert_eq!(proposal.args[9], Value::Object(Some(output.id())));

    // The realize: the full-output buffer, the output's origin.
    ack_configure(&mut client, &t1, serial);
    let shm = client.bind("ldp.core.shm");
    let pixels: Vec<u8> = std::iter::repeat(xrgb(30, 160, 90).to_le_bytes())
        .take(1920 * 1080)
        .flatten()
        .collect();
    let pool = create_pool(
        &mut client,
        &shm,
        pool_bytes(&pixels),
        (1920 * 1080 * 4) as i64,
    );
    let buffer = create_buffer(&mut client, &pool, 0, 1920, 1080, 1920 * 4, XR24);
    let before = tb.frames();
    attach(&mut client, &w1, &buffer);
    damage(&mut client, &w1, &[Rect::new(0, 0, 1920, 1080)]);
    commit(&mut client, &w1, 2);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while tb.frames() <= before {
        assert!(deadline > std::time::Instant::now(), "never rendered");
        client.sync();
    }
    assert_eq!(position_of(&tb, &w1), (0, 0), "the output's origin");
    // The cover reaches the output's last column (maximize's fill
    // stops ten short).
    let words = tb.scanout();
    assert_eq!(px(&words, 1915, 500)[1], 160, "the cover's last columns");
    assert_eq!(px(&words, 10, 10)[1], 160, "the cover's origin");
}

/// The machine's precedence, over the wire: fullscreen precedes
/// maximized — both flags ride the states, but the geometry is
/// fullscreen's (the whole output, zero insets).
#[test]
fn fullscreen_precedes_maximized_over_the_wire() {
    let tb = Testbench::start_with("states-prec", states_config("pr", DockMode::Auto));
    let mut client = TestClient::connect(&tb.addr);
    let shell = client.bind("ldp.shell.shell");
    client.bind("ldp.core.output");

    let (w1, _b1, _p1) = map_window(&tb, &mut client, xrgb(200, 40, 40), 150, 90);
    let t1 = get_toplevel(&mut client, &shell, &w1);
    client.sync();

    state_request(&mut client, &t1, "maximize");
    wait_for_configures(&mut client, t1.id().as_u32(), 2);
    client
        .conn
        .send_request(&t1, "fullscreen", vec![Value::Object(None)])
        .expect("fullscreen");
    wait_for_configures(&mut client, t1.id().as_u32(), 3);

    // The superseding proposal: both flags, fullscreen's geometry.
    let last = last_on(&client, "configure", t1.id().as_u32());
    assert!(
        states_of(last).test(0) && states_of(last).test(1),
        "maximized and fullscreen"
    );
    assert_eq!(last.args[2], Value::Uint32(1920), "fullscreen's size");
    assert_eq!(last.args[3], Value::Uint32(1080));
    assert_eq!(last.args[4], Value::Int32(0), "fullscreen's insets");
}

/// THE strict ack: a serial that is not the live proposal (stale,
/// unknown, or already realized) is the protocol error the contract
/// names — the dialog's doctrine, now the toplevel's.
#[test]
fn a_stale_ack_is_refused() {
    let tb = Testbench::start_with("states-ack", states_config("ak", DockMode::Off));
    let mut client = TestClient::connect(&tb.addr);
    let shell = client.bind("ldp.shell.shell");

    let (w1, _b1, _p1) = map_window(&tb, &mut client, xrgb(200, 40, 40), 150, 90);
    let t1 = get_toplevel(&mut client, &shell, &w1);
    client.sync();

    // The ack of a serial no proposal ever carried.
    ack_configure(&mut client, &t1, 99);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match client.conn.roundtrip(&mut client.events) {
            Err(ClientError::ServerError { code, .. }) => {
                assert_eq!(
                    code,
                    ldp_core::error::ErrorCode::InvalidState,
                    "the stale ack is invalid_state"
                );
                break;
            }
            Err(other) => panic!("expected invalid_state, got {other:?}"),
            Ok(()) => assert!(
                std::time::Instant::now() < deadline,
                "the server never refused the stale ack"
            ),
        }
    }
}

/// THE minimize narrative: hidden from the space, kept alive — the
/// ink leaves the canvas (the vacate claims repaint it away), the
/// live frame registration dies with `surface_hidden` (App Nap's own
/// termination), the parked request waits, and the unminimize
/// restores the exact bytes (the A/B/A byte oracle — the desktop
/// before equals the desktop after).
#[test]
#[allow(clippy::too_many_lines)] // one session, one narrative
fn minimize_hides_parks_and_unminimize_restores() {
    let tb = Testbench::start_with("states-min", states_config("mn", DockMode::Off));
    let mut client = TestClient::connect(&tb.addr);
    let shell = client.bind("ldp.shell.shell");
    // The output bind: the enter/leave truth only fires for outputs
    // the client holds objects for (the visibility pass's own
    // doctrine — the occlusion suite's pattern).
    client.bind("ldp.core.output");

    let plum = xrgb(120, 40, 160);
    let (w1, _b1, _p1) = map_window(&tb, &mut client, plum, 300, 200);
    let t1 = get_toplevel(&mut client, &shell, &w1);
    client.sync();

    // The window is entered on its output (the baseline the leave
    // answers).
    client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "enter_output" && r.target == w1.id().as_u32())
    });

    // The window holds a live frame registration (the park's
    // subject).
    frame(&mut client, &w1, 2);
    client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "frame_target" && frame_id(r) == 2)
    });

    // The baseline: the window's ink.
    let baseline = tb.scanout();
    assert_eq!(px(&baseline, 150, 100)[0], 120, "the window's ink");

    // The minimize: the configure carries the flag, the ink leaves,
    // the frame registration dies.
    state_request(&mut client, &t1, "minimize");
    wait_for_configures(&mut client, t1.id().as_u32(), 2);
    let proposal = last_on(&client, "configure", t1.id().as_u32());
    assert!(states_of(proposal).test(2), "the minimized flag");
    assert_eq!(proposal.args[2], Value::Uint32(0), "no geometry demand");
    // The visibility truths: the output leave, the frame drop.
    client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "leave_output" && r.target == w1.id().as_u32())
    });
    client.wait_until(|c| {
        c.records.iter().any(|r| {
            r.event == "frame_dropped" && frame_id(r) == 2 && drop_reason(r) == SURFACE_HIDDEN
        })
    });
    // The ink is gone (the vacate claims repainted it away).
    let hidden = tb.scanout();
    assert_eq!(px(&hidden, 150, 100)[0], 0, "the window is gone");
    assert_eq!(px(&hidden, 150, 100)[2], 0, "plain black");

    // The window is kept alive: the parked request waits (no
    // deadline for frame 3 while hidden).
    frame(&mut client, &w1, 3);
    for _ in 0..4 {
        client.sync();
    }
    assert!(
        !client
            .events
            .records
            .iter()
            .any(|r| r.event == "frame_target" && frame_id(r) == 3),
        "the hidden surface must not receive a frame_target"
    );

    // The unminimize: the flag releases, the parked request answers,
    // the output re-enters, the ink restores — the exact bytes.
    state_request(&mut client, &t1, "unminimize");
    wait_for_configures(&mut client, t1.id().as_u32(), 3);
    let restored = last_on(&client, "configure", t1.id().as_u32());
    assert!(states_of(restored).is_empty(), "the flag releases");
    client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "frame_target" && frame_id(r) == 3)
    });
    client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "enter_output" && r.target == w1.id().as_u32())
    });
    let after = tb.scanout();
    assert_eq!(
        after, baseline,
        "the desktop before equals the desktop after"
    );
}

/// The frame id of one recorded `frame_target` / `frame_dropped`.
fn frame_id(record: &Recorded) -> u64 {
    match record.args.first() {
        Some(Value::Uint64(id)) => *id,
        other => panic!("frame id argument is not a uint64: {other:?}"),
    }
}

/// The frame-drop reason's `surface_hidden` code (the App-Nap
/// termination's own enum value).
const SURFACE_HIDDEN: u64 = 2;

/// The drop reason of one recorded `frame_dropped` event.
fn drop_reason(record: &Recorded) -> u64 {
    match &record.args[1] {
        Value::Enum(reason) => u64::from(*reason),
        other => panic!("frame_dropped reason is not an enum: {other:?}"),
    }
}

/// THE workspace narrative: `set_workspace` moves the window (the
/// clamped actual rides `workspace_changed`, the configure's
/// workspace field carries it), the ink leaves the non-active space,
/// and the move back restores it.
#[test]
fn set_workspace_moves_reports_and_hides() {
    let tb = Testbench::start_with("states-ws", states_config("ws", DockMode::Off));
    let mut client = TestClient::connect(&tb.addr);
    let shell = client.bind("ldp.shell.shell");

    let plum = xrgb(120, 40, 160);
    let (w1, _b1, _p1) = map_window(&tb, &mut client, plum, 300, 200);
    let t1 = get_toplevel(&mut client, &shell, &w1);
    client.sync();

    // The move: space 3 (of 4).
    set_workspace(&mut client, &t1, 3);
    wait_for_n(&mut client, "workspace_changed", t1.id().as_u32(), 1);
    let moved = last_on(&client, "workspace_changed", t1.id().as_u32());
    assert_eq!(moved.args[0], Value::Uint32(3), "the actual space");
    wait_for_configures(&mut client, t1.id().as_u32(), 2);
    let proposal = last_on(&client, "configure", t1.id().as_u32());
    assert_eq!(
        proposal.args[8],
        Value::Uint32(3),
        "the configure's workspace"
    );
    // The ink leaves (space 3 is not the seat's active one).
    let gone = tb.scanout();
    assert_eq!(
        px(&gone, 150, 100)[0],
        0,
        "the window left the active space"
    );

    // The move back: the ink restores.
    set_workspace(&mut client, &t1, 0);
    wait_for_n(&mut client, "workspace_changed", t1.id().as_u32(), 2);
    let back = last_on(&client, "workspace_changed", t1.id().as_u32());
    assert_eq!(back.args[0], Value::Uint32(0));
    let restored = tb.scanout();
    assert_eq!(px(&restored, 150, 100)[0], 120, "the window returned");

    // The clamp: index 99 clamps into the count (space 3).
    set_workspace(&mut client, &t1, 99);
    wait_for_n(&mut client, "workspace_changed", t1.id().as_u32(), 3);
    let clamped = last_on(&client, "workspace_changed", t1.id().as_u32());
    assert_eq!(clamped.args[0], Value::Uint32(3), "the clamp");
    let home = machine_view(&tb, &w1, &t1).4;
    assert_eq!(home, 3, "the spaces machine's truth");
}

/// THE sticky doctrine: a sticky window shows on every space — the
/// move to a non-active space leaves the ink.
#[test]
fn sticky_windows_show_on_every_space() {
    let tb = Testbench::start_with("states-st", states_config("st", DockMode::Off));
    let mut client = TestClient::connect(&tb.addr);
    let shell = client.bind("ldp.shell.shell");

    let plum = xrgb(120, 40, 160);
    let (w1, _b1, _p1) = map_window(&tb, &mut client, plum, 300, 200);
    let t1 = get_toplevel(&mut client, &shell, &w1);
    client.sync();

    // Sticky on, then the move: the ink stays.
    client
        .conn
        .send_request(&t1, "set_sticky", vec![Value::Bool(true)])
        .expect("set_sticky");
    wait_for_configures(&mut client, t1.id().as_u32(), 2);
    let sticky = last_on(&client, "configure", t1.id().as_u32());
    assert!(states_of(sticky).test(4), "the sticky flag (bit 4)");
    set_workspace(&mut client, &t1, 3);
    wait_for_on(&mut client, "workspace_changed", t1.id().as_u32());
    let words = tb.scanout();
    assert_eq!(
        px(&words, 150, 100)[0],
        120,
        "the sticky window renders on the non-active space"
    );
}

/// THE hints: the bounded strings land on the machine (the taskbar's
/// future material), the bad ones are refused with the
/// bounded-string doctrine's own codes, and the size pair saturates
/// (a min above a max clamps to it — consistent by construction).
#[test]
fn the_hints_land_and_the_bad_ones_are_refused() {
    let tb = Testbench::start_with("states-hints", states_config("ht", DockMode::Off));
    let mut client = TestClient::connect(&tb.addr);
    let shell = client.bind("ldp.shell.shell");

    let (w1, _b1, _p1) = map_window(&tb, &mut client, xrgb(200, 40, 40), 150, 90);
    let t1 = get_toplevel(&mut client, &shell, &w1);
    client.sync();

    // The identity hints land.
    client
        .conn
        .send_request(&t1, "set_title", vec![Value::String("Lion Notes".into())])
        .expect("set_title");
    client
        .conn
        .send_request(
            &t1,
            "set_app_id",
            vec![Value::String("com.lionos.notes".into())],
        )
        .expect("set_app_id");
    client.sync();
    let view = machine_view(&tb, &w1, &t1);
    assert_eq!(view.1, "Lion Notes");
    assert_eq!(view.2, (0, 0), "no size hints yet");

    // The over-length title: the *client's own* validator refuses it
    // before the wire (the library's budget is the server's — the
    // defense-in-depth doctrine; the honest local refusal).
    let too_long = "x".repeat(4097).into_boxed_str();
    let refused = client
        .conn
        .send_request(&t1, "set_title", vec![Value::String(too_long)])
        .expect_err("the client refuses the over-length title");
    assert!(
        matches!(&refused, ClientError::Ldp(_)),
        "the local budget refusal: {refused:?}"
    );
    // A refused hint leaves the machine untouched.
    assert_eq!(machine_view(&tb, &w1, &t1).1, "Lion Notes");

    // The NUL title: the string's own invalid (a fresh client — the
    // refused one's connection died with the error, the doctrine).
    let mut client2 = TestClient::connect(&tb.addr);
    let shell2 = client2.bind("ldp.shell.shell");
    let compositor2 = client2.bind("ldp.core.compositor");
    let surface2 = client2
        .conn
        .create_object(&compositor2, "create_surface", vec![])
        .expect("surface");
    let t2 = get_toplevel(&mut client2, &shell2, &surface2);
    client2.sync();
    client2
        .conn
        .send_request(&t2, "set_title", vec![Value::String("a\0b".into())])
        .expect("send");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match client2.conn.roundtrip(&mut client2.events) {
            Err(ClientError::ServerError { code, .. }) => {
                assert_eq!(
                    code,
                    ldp_core::error::ErrorCode::InvalidString,
                    "the embedded NUL is the string's own invalid"
                );
                break;
            }
            Err(other) => panic!("expected invalid_string, got {other:?}"),
            Ok(()) => assert!(
                std::time::Instant::now() < deadline,
                "the server never refused the NUL title"
            ),
        }
    }

    // The size pair saturates: a max first, then a min above it.
    let mut client3 = TestClient::connect(&tb.addr);
    let shell3 = client3.bind("ldp.shell.shell");
    let compositor3 = client3.bind("ldp.core.compositor");
    let surface3 = client3
        .conn
        .create_object(&compositor3, "create_surface", vec![])
        .expect("surface");
    let t3 = get_toplevel(&mut client3, &shell3, &surface3);
    client3.sync();
    client3
        .conn
        .send_request(
            &t3,
            "set_max_size",
            vec![Value::Uint32(400), Value::Uint32(300)],
        )
        .expect("set_max_size");
    client3
        .conn
        .send_request(
            &t3,
            "set_min_size",
            vec![Value::Uint32(500), Value::Uint32(50)],
        )
        .expect("set_min_size");
    client3.sync();
    let (states, _title, min, max, _home) = machine_view(&tb, &surface3, &t3);
    assert!(states.0.is_empty());
    assert_eq!(min, (400, 50), "the min saturates at the established max");
    assert_eq!(max, (400, 300));
}
