//! Phase 45's exit criterion (the supervisor half): the DWM-style
//! session rebuild, end to end over **real processes** — the
//! supervisor spawns the compiled `lion-compositor` binary on a pinned
//! abstract socket, a client presents a frame, the compositor is
//! killed with SIGKILL (the harsh death — no teardown, no last words),
//! and the supervisor's rebuild brings the *same socket* back; a
//! client reconnects and presents again. The session flashes and
//! rebuilds; the desktop survives its compositor.
//!
//! This is the Windows story ("DWM crash recovery: the session flashes
//! and rebuilds — the redirection surfaces persist, so windows survive
//! their compositor") translated to the LDP doctrine — the *clients*
//! own their buffers (the flip-model symmetry), so the survivors of a
//! compositor death are the clients themselves: a rebuilt server plus
//! a reconnecting client is the rebuilt session. And the operator's
//! stop (SIGTERM to the supervisor) is the graceful session end — the
//! supervisor forwards the stop and exits cleanly.

mod testbench;

use std::time::{Duration, Instant};

use ldp_client::Connection;
use ldp_core::geometry::Rect;
use ldp_transport::UnixAddr;
use testbench::*;

/// The XRGB8888 fourcc ("XR24").
const XR24: u32 = 0x3432_5258;
/// One small window's word: opaque dark teal.
const WORD: u32 = 0xFF30_4844;

/// THE exit criterion: crash → rebuild → reconnect, then the clean
/// stop. Every claim is a real process's real exit status or a real
/// event the rebuilt session delivered.
#[test]
fn the_session_rebuilds_after_a_killed_compositor() {
    let n = std::process::id();
    let socket = format!("lion-rebuild-{n}");
    let pid_file = std::env::temp_dir().join(format!("lion-rebuild-{n}.pid"));

    // ---- the supervised session ------------------------------------
    let mut supervisor = std::process::Command::new(env!("CARGO_BIN_EXE_lion-supervisor"))
        .arg("--socket")
        .arg(&socket)
        .arg("--compositor")
        .arg(env!("CARGO_BIN_EXE_lion-compositor"))
        .arg("--backoff-ms")
        .arg("80")
        .arg("--pid-file")
        .arg(&pid_file)
        .arg("--")
        .arg("--mode")
        .arg("headless")
        .arg("--quiet")
        .spawn()
        .expect("spawn lion-supervisor");
    let addr = UnixAddr::abstract_name(socket.as_bytes()).expect("the pin is a legal name");

    // The session comes up: the socket serves.
    let mut client = connect_until(&addr, "the first session serves");

    // A full client cycle presents over the real socket.
    present_one(&addr, &mut client);

    // ---- the harsh death --------------------------------------------
    let pid = read_pid(&pid_file);
    // SAFETY: `kill(2)` with SIGKILL on the pid the supervisor's own
    // pid file names (the compositor child — the supervisor spawned
    // it, the test spawned the supervisor); SIGKILL cannot fail on a
    // live owned pid short of ESRCH (already gone, which the next
    // phase of this test would catch anyway).
    let rc = unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
    assert_eq!(rc, 0, "the SIGKILL landed");

    // The rebuild: the same abstract name comes back (the dead
    // process's socket vanished with it; the supervisor's re-spawn
    // rebinds the pin). A reconnecting client gets a working session —
    // the DWM story's LDP translation.
    let mut rebuilt = connect_until(&addr, "the rebuilt session serves");

    // And the rebuilt session presents a fresh frame — the full cycle,
    // alive again exactly as before the death.
    present_one(&addr, &mut rebuilt);

    // ---- the operator's stop ---------------------------------------
    // SIGTERM to the supervisor: the graceful session end — the stop
    // is forwarded to the child, both processes exit, and the exit is
    // the clean one (0), never a crash loop's.
    let rc = unsafe { libc::kill(supervisor.id() as libc::pid_t, libc::SIGTERM) };
    assert_eq!(rc, 0, "the stop signal landed");
    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        match supervisor.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                assert!(
                    deadline > Instant::now(),
                    "the supervisor never exited after the stop"
                );
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => panic!("waiting the supervisor: {e}"),
        }
    };
    assert!(
        status.success(),
        "the supervisor exits cleanly after the operator stop (got {status})"
    );
}

/// Connect against `addr` until it serves (the bring-up and the
/// rebuild both take real time; the poll is the client's reconnect
/// doctrine in miniature).
fn connect_until(addr: &UnixAddr, what: &str) -> TestClient {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(conn) = Connection::connect(addr) {
            return TestClient {
                conn,
                events: Collector::default(),
            };
        }
        assert!(deadline > Instant::now(), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// One full client cycle: bind, pool, buffer, surface, frame, attach,
/// damage, commit — and the presentation verdict waited on.
fn present_one(_addr: &UnixAddr, client: &mut TestClient) {
    let shm = client.bind("ldp.core.shm");
    let compositor = client.bind("ldp.core.compositor");
    client.bind("ldp.core.output");

    let pixels: Vec<u8> = std::iter::repeat(WORD.to_le_bytes())
        .take(96 * 64)
        .flatten()
        .collect();
    let fd = {
        use std::io::Write as _;
        let fd = lion_compositor::sys::memfd("rebuild-pool").expect("memfd");
        {
            let mut sink = std::fs::File::from(fd.try_clone().expect("clone"));
            sink.write_all(&pixels).expect("fill the pool");
        }
        fd
    };
    let pool = create_pool(client, &shm, fd, (96 * 64 * 4) as i64);
    let buffer = create_buffer(client, &pool, 0, 96, 64, 96 * 4, XR24);
    let surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("create_surface");
    frame(client, &surface, 1);
    attach(client, &surface, &buffer);
    damage(client, &surface, &[Rect::new(0, 0, 96, 64)]);
    commit(client, &surface, 1);
    client.wait_until(|c| c.records.iter().any(|r| r.event == "presented"));
}

/// Read the supervisor's pid file (the child's pid — the test's window
/// into the supervised process).
fn read_pid(path: &std::path::Path) -> i32 {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(text) = std::fs::read_to_string(path) {
            if let Ok(pid) = text.trim().parse::<i32>() {
                return pid;
            }
        }
        assert!(deadline > Instant::now(), "the pid file never appeared");
        std::thread::sleep(Duration::from_millis(20));
    }
}
