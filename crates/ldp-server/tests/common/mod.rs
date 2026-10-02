//! Shared integration-test harness: a raw-codec mock client and a
//! controllable test server.
//!
//! The mock client speaks exactly the wire protocol (`ldp-protocol`
//! encode/decode over `ldp-transport` framing) — no server-side types
//! on the client path, so conformance failures surface as protocol
//! behavior, not library coupling. This module is also the seed of the
//! Phase 5 client library.

#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ldp_core::bitset::Bitset128;
use ldp_core::error::LdpError;
use ldp_core::limits::Limits;
use ldp_core::wire::Value;
use ldp_protocol::{decode, Message, ValidationMode, REGISTRY};
use ldp_server::{Dispatcher, Server, ServerConfig};
use ldp_transport::{
    FdList, FramedReader, FramedWriter, NoHooks, TransportListener, TransportStream, UnixAddr,
};

static ADDR_SEQ: AtomicU64 = AtomicU64::new(0);

/// A unique abstract address name per call (tests run in parallel).
pub fn addr_name(tag: &str) -> String {
    let n = ADDR_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("ldp-it-{tag}-{}-{n}", std::process::id())
}

/// A unique abstract address per call.
pub fn addr(tag: &str) -> UnixAddr {
    UnixAddr::abstract_name(addr_name(tag).as_bytes()).unwrap()
}

/// Request opcode of `interface.name` from the compiled schema.
pub fn op(interface: &str, name: &str) -> u32 {
    let i = REGISTRY.interface(interface).unwrap();
    i.requests.iter().find(|r| r.name == name).unwrap().opcode
}

/// Event opcode of `interface.name` from the compiled schema.
pub fn ev(interface: &str, name: &str) -> u32 {
    let i = REGISTRY.interface(interface).unwrap();
    i.events.iter().find(|r| r.name == name).unwrap().opcode
}

/// The connection object's reserved wire ID.
pub const CONNECTION: u32 = 1;

/// A raw-protocol client over a blocking stream.
pub struct MockClient {
    stream: TransportStream,
    reader: FramedReader,
    writer: FramedWriter<NoHooks>,
    limits: Limits,
}

impl MockClient {
    /// Connect to `addr`.
    pub fn connect(addr: &UnixAddr) -> MockClient {
        MockClient {
            stream: TransportStream::connect(addr).unwrap(),
            reader: FramedReader::new(Limits::DEFAULT),
            writer: FramedWriter::without_hooks(Limits::DEFAULT),
            limits: Limits::DEFAULT,
        }
    }

    /// Send one message (no FDs), under the client's negotiated limits.
    pub fn send(&mut self, msg: &Message) {
        let bytes = msg.encode(&self.limits).unwrap();
        let mut fds = FdList::new();
        self.writer
            .send_msg(&mut self.stream, &bytes, &mut fds)
            .unwrap();
    }

    /// Mirror the server's `large_messages` negotiation: widen both
    /// framing directions to 64 MiB.
    pub fn negotiate_large_messages(&mut self) {
        self.limits = Limits::LARGE_MESSAGES;
        self.reader = FramedReader::new(Limits::LARGE_MESSAGES);
        self.writer = FramedWriter::without_hooks(Limits::LARGE_MESSAGES);
    }

    /// Send one message with an FD batch.
    pub fn send_with_fds(&mut self, msg: &Message, fds: &mut FdList) {
        let bytes = msg.encode(&Limits::DEFAULT).unwrap();
        self.writer.send_msg(&mut self.stream, &bytes, fds).unwrap();
    }

    /// Send pre-encoded raw bytes (malformed-corpus helper).
    pub fn send_raw(&mut self, bytes: &[u8]) {
        let mut fds = FdList::new();
        self.writer
            .send_msg(&mut self.stream, bytes, &mut fds)
            .unwrap();
    }

    /// Write a deliberate partial frame straight to the socket (crash
    /// tests: a frame in flight when the peer vanishes).
    pub fn send_partial(&mut self, bytes: &[u8]) {
        let n = self.stream.send_chunk(bytes, None).unwrap();
        assert_eq!(n, bytes.len(), "partial write must fully land");
    }

    /// Receive one message (blocks).
    pub fn recv(&mut self) -> Message {
        let frame = self.reader.recv_msg(&mut self.stream).unwrap();
        decode(
            frame.message_bytes(),
            u32::from(frame.fd_count()),
            &Limits::DEFAULT,
            ValidationMode::Tolerant,
        )
        .unwrap()
    }

    /// Receive until the transport errors (EOF assertion helper).
    pub fn recv_err(&mut self) -> LdpError {
        self.reader.recv_msg(&mut self.stream).unwrap_err()
    }

    // ---- typed protocol helpers ------------------------------------

    /// `connection.hello`.
    pub fn hello(&mut self, release: u32, options: Bitset128) {
        let m = Message::new(CONNECTION, op("ldp.core.connection", "hello"))
            .arg(Value::Uint32(release))
            .arg(Value::Bitset(options));
        self.send(&m);
    }

    /// Expect `connection.welcome`; returns `(release, client_id, sandbox)`.
    pub fn expect_welcome(&mut self) -> (u32, u32, u32) {
        let m = self.recv();
        assert_eq!(m.object_id, CONNECTION);
        assert_eq!(m.opcode, ev("ldp.core.connection", "welcome"));
        let Value::Uint32(release) = m.args[0] else {
            panic!("welcome release")
        };
        let Value::Uint32(client_id) = m.args[2] else {
            panic!("welcome client_id")
        };
        let Value::String(app_id) = &m.args[3] else {
            panic!("welcome app_id")
        };
        assert!(app_id.is_empty());
        let Value::Enum(sandbox) = m.args[4] else {
            panic!("welcome sandbox")
        };
        (release, client_id, sandbox)
    }

    /// `connection.get_registry` at client-chosen `new_id`.
    pub fn get_registry(&mut self, version: u32, new_id: u32) {
        let m = Message::new(CONNECTION, op("ldp.core.connection", "get_registry"))
            .arg(Value::Uint32(version))
            .arg(Value::NewId(
                ldp_core::ids::ObjectId::client(new_id).unwrap(),
            ));
        self.send(&m);
    }

    /// `registry.bind` on registry object `on`.
    pub fn bind(&mut self, on: u32, interface: &str, version: u32, new_id: u32) {
        let m = Message::new(on, op("ldp.core.registry", "bind"))
            .arg(Value::String(interface.into()))
            .arg(Value::Uint32(version))
            .arg(Value::NewId(
                ldp_core::ids::ObjectId::client(new_id).unwrap(),
            ));
        self.send(&m);
    }

    /// `connection.sync` and expect `sync_done` with the same cookie.
    pub fn sync(&mut self, cookie: u32) {
        let m =
            Message::new(CONNECTION, op("ldp.core.connection", "sync")).arg(Value::Uint32(cookie));
        self.send(&m);
        let back = self.recv();
        assert_eq!(back.object_id, CONNECTION);
        assert_eq!(back.opcode, ev("ldp.core.connection", "sync_done"));
        assert_eq!(back.args[0], Value::Uint32(cookie));
    }

    /// `connection.ping` and expect `pong`.
    pub fn ping(&mut self, cookie: u32) {
        let m =
            Message::new(CONNECTION, op("ldp.core.connection", "ping")).arg(Value::Uint32(cookie));
        self.send(&m);
        let back = self.recv();
        assert_eq!(back.opcode, ev("ldp.core.connection", "pong"));
        assert_eq!(back.args[0], Value::Uint32(cookie));
    }

    /// `connection.destroy` and expect `destroyed` echoing both args.
    pub fn destroy(&mut self, object_id: u32, cookie: u32) {
        let m = Message::new(CONNECTION, op("ldp.core.connection", "destroy"))
            .arg(Value::Uint32(object_id))
            .arg(Value::Uint32(cookie));
        self.send(&m);
        let back = self.recv();
        assert_eq!(back.object_id, CONNECTION);
        assert_eq!(back.opcode, ev("ldp.core.connection", "destroyed"));
        assert_eq!(back.args[0], Value::Uint32(object_id));
        assert_eq!(back.args[1], Value::Uint32(cookie));
    }

    /// Expect the fatal `connection.error` event with `code`, then EOF
    /// (the server closes after delivering it).
    pub fn expect_fatal(&mut self, code: ldp_core::error::ErrorCode) {
        let m = self.recv();
        assert_eq!(m.object_id, CONNECTION);
        assert_eq!(m.opcode, ev("ldp.core.connection", "error"));
        let Value::Enum(wire) = m.args[0] else {
            panic!("error code")
        };
        assert_eq!(wire, code.to_wire(), "expected {code}, got wire {wire}");
        let err = self.recv_err();
        assert!(
            ldp_transport::is_disconnect(&err),
            "server must close after the error event, got {err}"
        );
    }

    /// Expect one `registry.global` event: `(interface, min, max)`.
    pub fn expect_global(&mut self) -> (String, u32, u32) {
        let m = self.recv();
        assert_eq!(m.opcode, ev("ldp.core.registry", "global"));
        let Value::String(i) = &m.args[0] else {
            panic!("global interface")
        };
        let Value::Uint32(min) = m.args[1] else {
            panic!("global min")
        };
        let Value::Uint32(max) = m.args[2] else {
            panic!("global max")
        };
        (i.to_string(), min, max)
    }

    /// Expect one `registry.bound` event: `(interface, version)`.
    pub fn expect_bound(&mut self) -> (String, u32) {
        let m = self.recv();
        assert_eq!(m.opcode, ev("ldp.core.registry", "bound"));
        let Value::String(i) = &m.args[0] else {
            panic!("bound interface")
        };
        let Value::Uint32(v) = m.args[1] else {
            panic!("bound version")
        };
        (i.to_string(), v)
    }
}

/// A controllable in-thread server: accept loop on its own thread,
/// stoppable by connecting a dummy client (wakes the blocked accept).
pub struct TestServer {
    /// The address clients connect to.
    pub addr: UnixAddr,
    /// The abstract-namespace name (without the leading NUL) — for
    /// child-process helpers that reconstruct the address.
    name: String,
    server: Arc<Server>,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl TestServer {
    /// Start with the [`NullDispatcher`](ldp_server::NullDispatcher).
    pub fn start(tag: &str, config: ServerConfig) -> TestServer {
        TestServer::start_with(tag, config, || ldp_server::NullDispatcher)
    }

    /// Start with a per-session dispatcher factory.
    pub fn start_with<D>(
        tag: &str,
        config: ServerConfig,
        factory: impl Fn() -> D + Send + Sync + Clone + 'static,
    ) -> TestServer
    where
        D: Dispatcher + 'static,
    {
        let server = Arc::new(Server::new(config).unwrap());
        let name = addr_name(tag);
        let addr = UnixAddr::abstract_name(name.as_bytes()).unwrap();
        let listener = TransportListener::bind(&addr, 32).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let (loop_server, loop_stop) = (Arc::clone(&server), Arc::clone(&stop));
        let join = std::thread::Builder::new()
            .name("ldp-test-accept".into())
            .spawn(move || {
                let mut listener = listener;
                loop {
                    if loop_stop.load(Ordering::Relaxed) {
                        break;
                    }
                    match loop_server.accept_session(&mut listener) {
                        Ok(Some(session)) => {
                            if loop_server.spawn_session(session, factory.clone()).is_err() {
                                break;
                            }
                        }
                        Ok(None) => {}
                        Err(_) => break,
                    }
                }
            })
            .unwrap();
        TestServer {
            addr,
            name,
            server,
            stop,
            join: Some(join),
        }
    }

    /// The abstract-namespace name of the listen address.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Live session count.
    pub fn live(&self) -> u32 {
        self.server.live_sessions()
    }

    /// Poll until no session is live, or `timeout` elapses.
    pub fn wait_live_zero(&self, timeout: Duration) -> bool {
        self.wait_live(0, timeout)
    }

    /// Poll until exactly `expected` sessions are live, or `timeout`
    /// elapses.
    pub fn wait_live(&self, expected: u32, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self.live() == expected {
                return true;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        self.live() == expected
    }

    /// Stop the accept loop and wait for it.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Wake the blocked accept with a throwaway connection.
        let _ = TransportStream::connect(&self.addr);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
        // Drain to ZERO: the wake connection is accepted right before
        // the loop exits, and the test's own sessions may still be
        // processing EOF — either would leak an FD into the next test's
        // baseline. Bounded so a leaked client cannot wedge teardown.
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.live() > 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Share one dispatcher state across session threads (tests only).
pub struct Shared<D>(pub Arc<Mutex<D>>);

impl<D: Dispatcher> Dispatcher for Shared<D> {
    fn on_request(
        &mut self,
        ctx: &mut ldp_server::DispatchCtx<'_>,
        request: &ldp_server::IncomingRequest<'_>,
    ) -> ldp_core::error::Result<()> {
        self.0.lock().unwrap().on_request(ctx, request)
    }

    fn on_session_end(
        &mut self,
        client: ldp_core::ids::ClientId,
        reason: &ldp_server::SessionEnd,
        objects: &[(ldp_core::ids::ObjectId, ldp_server::ObjectEntry)],
    ) {
        self.0
            .lock()
            .unwrap()
            .on_session_end(client, reason, objects);
    }

    fn on_bind(
        &mut self,
        ctx: &mut ldp_server::DispatchCtx<'_>,
        object: ldp_core::ids::ObjectId,
        interface: &str,
        version: u32,
    ) -> ldp_core::error::Result<()> {
        self.0
            .lock()
            .unwrap()
            .on_bind(ctx, object, interface, version)
    }

    fn on_destroy(
        &mut self,
        ctx: &mut ldp_server::DispatchCtx<'_>,
        object: ldp_core::ids::ObjectId,
        interface: &str,
    ) -> ldp_core::error::Result<()> {
        self.0.lock().unwrap().on_destroy(ctx, object, interface)
    }

    fn on_wake(&mut self, ctx: &mut ldp_server::DispatchCtx<'_>) -> ldp_core::error::Result<()> {
        self.0.lock().unwrap().on_wake(ctx)
    }
}

/// The standard test advertisement: a couple of globals.
pub fn standard_config() -> ServerConfig {
    ServerConfig {
        globals: vec![
            ldp_server::GlobalAdvert::new("ldp.core.output"),
            ldp_server::GlobalAdvert::new("ldp.core.compositor"),
            ldp_server::GlobalAdvert::new("ldp.core.shm"),
        ],
        ..ServerConfig::default()
    }
}
