//! Crash reclamation and FD hygiene — serialized (process-wide FD
//! counts are only stable when this suite runs alone in its binary).
//!
//! The phase-4 exit criteria exercised here: a peer that dies mid-frame
//! (SIGKILL, clean-FIN, and reset variants) leaves the server healthy,
//! reclaims the session, closes every FD, and keeps serving others.

#![cfg(target_os = "linux")]

mod common;

use std::io::Write as _;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{op, MockClient, TestServer};
use ldp_core::bitset::Bitset128;
use ldp_core::error::Result;
use ldp_core::ids::ObjectId;
use ldp_core::wire::Value;
use ldp_protocol::Message;

use ldp_server::{DispatchCtx, Dispatcher, IncomingRequest, ObjectEntry, SessionEnd};

/// Serialize the whole binary: FD-count assertions need the process to
/// themselves.
static SUITE: Mutex<()> = Mutex::new(());

fn ready(server: &TestServer, replayed_globals: usize) -> MockClient {
    let mut c = MockClient::connect(&server.addr);
    c.hello(1, Bitset128::EMPTY);
    c.expect_welcome();
    c.get_registry(1, 2);
    for _ in 0..replayed_globals {
        let _ = c.expect_global();
    }
    c
}

#[derive(Default)]
struct TakeNothing {
    ended: usize,
}

impl Dispatcher for TakeNothing {
    fn on_request(&mut self, _ctx: &mut DispatchCtx<'_>, _r: &IncomingRequest<'_>) -> Result<()> {
        Ok(())
    }
    fn on_session_end(
        &mut self,
        _client: ldp_core::ids::ClientId,
        _reason: &SessionEnd,
        _objects: &[(ObjectId, ObjectEntry)],
    ) {
        self.ended += 1;
    }
}

#[test]
fn untaken_fds_close_on_return() {
    let _guard = SUITE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let server = TestServer::start_with("crash-untaken", common::standard_config(), || {
        TakeNothing::default()
    });
    let before = ldp_transport::count_open_fds().unwrap();
    let mut c = ready(&server, 4);
    c.bind(2, "ldp.core.shm", 1, 9);
    c.expect_bound();
    let m = Message::new(9, op("ldp.core.shm", "create_pool"))
        .arg(Value::Fd(0))
        .arg(Value::Int64(4096))
        .arg(Value::NewId(ObjectId::client(10).unwrap()));
    let mut fds = ldp_transport::FdList::new();
    fds.push(std::fs::File::open("/dev/null").unwrap().into());
    c.send_with_fds(&m, &mut fds);
    c.sync(1);
    drop(c);
    assert!(server.wait_live_zero(Duration::from_secs(2)));
    let after = ldp_transport::count_open_fds().unwrap();
    assert_eq!(before, after, "the untaken fd must not leak");
}

/// Kill a child process with a half-written frame on the socket: the
/// server must reclaim the session and stay healthy.
fn kill9_child_writes(addr_name: &str, bytes: &[u8]) {
    let script = r#"
import os, socket, sys, time
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect("\0" + sys.argv[1])
s.sendall(sys.stdin.buffer.read())
time.sleep(0.05)
os.kill(os.getpid(), 9)
"#
    .to_string();
    let mut child = std::process::Command::new("python3")
        .arg("-c")
        .arg(script)
        .arg(addr_name)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .expect("python3 child");
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(bytes).expect("child stdin");
    drop(stdin); // EOF: the child's read() completes and it proceeds
    let status = child.wait().expect("child wait");
    assert!(
        !status.success(),
        "the child must die by signal, not exit cleanly"
    );
}

/// The abstract-namespace name of the test server's listen address.

#[test]
fn kill9_mid_header_is_reclaimed_and_server_stays_healthy() {
    let _guard = SUITE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let audit = Arc::new(ldp_server::AuditRecorder::new(64));
    let cfg = ldp_server::ServerConfig {
        globals: vec![ldp_server::GlobalAdvert::new("ldp.core.output")],
        audit: Arc::clone(&audit) as Arc<dyn ldp_server::AuditSink>,
        ..ldp_server::ServerConfig::default()
    };
    let server = TestServer::start("crash-kill9", cfg);
    let fd_baseline = ldp_transport::count_open_fds().unwrap();

    // A healthy client first: proves the server is serving.
    let mut healthy = ready(&server, 2);
    healthy.sync(1);

    // The doomed child: connects, writes 10 bytes of a 16-byte header,
    // then SIGKILLs itself.
    let mut partial_header = [0u8; 10];
    partial_header[0..4].copy_from_slice(&1u32.to_le_bytes());
    partial_header[4..8].copy_from_slice(&1u32.to_le_bytes());
    kill9_child_writes(server.name(), &partial_header);

    // The server reclaims the crashed session — exactly the healthy
    // one remains…
    assert!(
        server.wait_live(1, Duration::from_secs(5)),
        "the crashed session must be reclaimed"
    );
    // …and is still healthy for the pre-existing client.
    healthy.ping(2);
    healthy.sync(3);
    drop(healthy);
    assert!(server.wait_live_zero(Duration::from_secs(5)));

    // FD hygiene: back to the baseline once everything is gone.
    let fd_after = ldp_transport::count_open_fds().unwrap();
    assert_eq!(fd_baseline, fd_after, "no fd may leak from the crash");

    // The audit trail says what happened.
    let records = audit.snapshot();
    assert!(records.iter().any(|r| matches!(
        r,
        ldp_server::AuditRecord::Disconnected {
            reason: SessionEnd::Crash,
            ..
        }
    )));
}

#[test]
fn kill9_with_unread_data_and_mid_payload() {
    let _guard = SUITE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let server = TestServer::start("crash-kill9-rst", common::standard_config());
    let fd_baseline = ldp_transport::count_open_fds().unwrap();

    // Variant (a): the child completes hello (so the server emits an
    // unread welcome — pending rx data at close turns the disconnect
    // into a reset), then dies with a partial second frame.
    let mut hello_frame = Message::new(1, op("ldp.core.connection", "hello"))
        .arg(Value::Uint32(1))
        .arg(Value::Bitset(Bitset128::EMPTY))
        .encode(&ldp_core::limits::Limits::DEFAULT)
        .unwrap();
    let mut partial = hello_frame.clone();
    partial.truncate(20); // half of a follow-up frame's header
    hello_frame.extend_from_slice(&partial);
    kill9_child_writes(server.name(), &hello_frame);

    assert!(
        server.wait_live_zero(Duration::from_secs(5)),
        "the reset-disconnect session must be reclaimed"
    );

    // Variant (b): mid-payload EOF from a Rust client that drops the
    // socket with a frame in flight.
    let mut half = ready(&server, 4);
    // Hand-write half a frame: 16-byte header claiming 2 words + 8 bytes.
    let mut bytes = [0u8; 24];
    bytes[0..4].copy_from_slice(&2u32.to_le_bytes());
    bytes[4..8].copy_from_slice(&1u32.to_le_bytes());
    half.send_partial(&bytes[..12]); // header + 4 payload bytes, then close
    drop(half);
    assert!(server.wait_live_zero(Duration::from_secs(5)));

    let fd_after = ldp_transport::count_open_fds().unwrap();
    assert_eq!(fd_baseline, fd_after);
}

#[test]
fn error_sessions_are_audited_and_reclaimed() {
    let _guard = SUITE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let audit = Arc::new(ldp_server::AuditRecorder::new(64));
    let cfg = ldp_server::ServerConfig {
        globals: vec![ldp_server::GlobalAdvert::new("ldp.core.output")],
        audit: Arc::clone(&audit) as Arc<dyn ldp_server::AuditSink>,
        ..ldp_server::ServerConfig::default()
    };
    let server = TestServer::start("crash-err", cfg);
    let mut bad = ready(&server, 2);
    // A protocol violation ends the session with the delivered code.
    let m = Message::new(1, op("ldp.core.connection", "destroy"))
        .arg(Value::Uint32(4242))
        .arg(Value::Uint32(1));
    bad.send(&m);
    bad.expect_fatal(ldp_core::error::ErrorCode::InvalidObject);
    assert!(server.wait_live_zero(Duration::from_secs(2)));

    let records = audit.snapshot();
    assert!(records.iter().any(|r| matches!(
        r,
        ldp_server::AuditRecord::Fatal {
            code: ldp_core::error::ErrorCode::InvalidObject,
            ..
        }
    )));
    assert!(records.iter().any(|r| matches!(
        r,
        ldp_server::AuditRecord::Disconnected {
            reason: SessionEnd::Protocol(_),
            ..
        }
    )));
}

#[derive(Default)]
struct Hoarder {
    fds: Vec<std::os::fd::OwnedFd>,
}

impl Dispatcher for Hoarder {
    fn on_request(
        &mut self,
        ctx: &mut DispatchCtx<'_>,
        request: &IncomingRequest<'_>,
    ) -> Result<()> {
        if request.op.name == "create_pool" {
            self.fds.push(ctx.take_fd(0)?);
        }
        Ok(())
    }
    fn on_session_end(
        &mut self,
        _: ldp_core::ids::ClientId,
        _: &SessionEnd,
        _: &[(ObjectId, ObjectEntry)],
    ) {
        self.fds.clear();
    }
}

#[test]
fn retained_fd_ceiling_is_enforced() {
    let _guard = SUITE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let cfg = ldp_server::ServerConfig {
        globals: vec![ldp_server::GlobalAdvert::new("ldp.core.shm")],
        limits: ldp_core::limits::Limits {
            client_fds: 2,
            ..ldp_core::limits::Limits::DEFAULT
        },
        ..ldp_server::ServerConfig::default()
    };
    let server = TestServer::start_with("crash-fd-ceiling", cfg, Hoarder::default);
    let mut c = ready(&server, 2);
    c.bind(2, "ldp.core.shm", 1, 9);
    c.expect_bound();

    let pool = |id: u32| {
        Message::new(9, op("ldp.core.shm", "create_pool"))
            .arg(Value::Fd(0))
            .arg(Value::Int64(4096))
            .arg(Value::NewId(ObjectId::client(id).unwrap()))
    };
    let send = |c: &mut MockClient, id: u32| {
        let mut fds = ldp_transport::FdList::new();
        fds.push(std::fs::File::open("/dev/null").unwrap().into());
        c.send_with_fds(&pool(id), &mut fds);
    };
    send(&mut c, 10); // retained fd 1
    send(&mut c, 11); // retained fd 2
    send(&mut c, 12); // over the ceiling of 2
    c.expect_fatal(ldp_core::error::ErrorCode::LimitExceeded);
    assert!(server.wait_live_zero(Duration::from_secs(2)));
}
