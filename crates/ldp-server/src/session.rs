//! One client session: the protocol state machine over one stream.
//!
//! [`ClientSession`] owns everything per-connection: the framed
//! reader/writer, the negotiated limits, the generational object store,
//! the handshake state, and the granted options. [`ClientSession::run`]
//! is the blocking-drive loop (one session thread per connection in this
//! phase); [`ClientSession::step`] processes exactly one inbound message
//! and is the embedding point for the Phase 6 epoll main loop.
//!
//! The per-message pipeline is the `docs/protocol.md` §9 order:
//!
//! 1. framing (transport stage 1 — the reader),
//! 2. structural decode (stage 2, [`ldp_protocol::decode`]),
//! 3. object resolution *then* signature check (stage 3) — the target
//!    object's interface and pinned version select the signature, which
//!    is exactly the type-confusion defense,
//! 4. dispatch: built-in core interfaces here, everything else through
//!    the [`crate::dispatch::Dispatcher`] seam.
//!
//! Any protocol error is fatal: the session emits `connection.error`,
//! stops processing, and reports the end for reclamation.

use std::fmt;
use std::sync::Arc;

use ldp_core::bitset::Bitset128;
use ldp_core::error::{ErrorCode, LdpError, Result};
use ldp_core::ids::{ClientId, ObjectId};
use ldp_core::limits::Limits;
use ldp_core::wire::Value;
use ldp_protocol::{check_signature, decode, Message, ValidationMode, REGISTRY};
use ldp_transport::backpressure::NoHooks;
use ldp_transport::creds::PeerCreds;
use ldp_transport::error::{is_disconnect, is_would_block};
use ldp_transport::fd::FdList;
use ldp_transport::frame::Frame;
use ldp_transport::reader::FramedReader;
use ldp_transport::stream::TransportStream;
use ldp_transport::writer::FramedWriter;

use crate::audit::AuditRecord;
use crate::builtin;
use crate::config::ServerConfig;
use crate::dispatch::{DispatchCtx, Dispatcher};
use crate::object::{ObjectEntry, ObjectKind, ObjectStore};

/// Interface name of the pre-bound bootstrap object.
pub const CONNECTION_INTERFACE: &str = "ldp.core.connection";

/// Interface name of the registry (materialized by `get_registry`).
pub const REGISTRY_INTERFACE: &str = "ldp.core.registry";

/// How a session ended. `Clean` is a well-behaved disconnect;
/// `Crash` is a peer that vanished mid-message (EOF or reset with a
/// frame in flight); `Protocol` carries the fatal error code the client
/// was told about; `Io` and `Internal` are server-side conditions.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SessionEnd {
    /// The peer closed the socket between messages.
    Clean,
    /// The peer vanished with a message in flight.
    Crash,
    /// A fatal protocol error ended the connection (code delivered).
    Protocol(ErrorCode),
    /// A transport failure that is not a peer disconnect.
    Io,
    /// A session thread panicked (unwind mode) — a server bug.
    Internal,
}

impl fmt::Display for SessionEnd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Clean => f.write_str("clean disconnect"),
            Self::Crash => f.write_str("peer crash"),
            Self::Protocol(c) => write!(f, "protocol error {c}"),
            Self::Io => f.write_str("transport failure"),
            Self::Internal => f.write_str("internal failure"),
        }
    }
}

/// Result of processing one message (or one would-block/EOF event).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SessionOutcome {
    /// The session is live; keep going.
    Continue,
    /// The session is over; reclaim and stop.
    Ended(SessionEnd),
}

/// One connected client's protocol session.
pub struct ClientSession {
    stream: TransportStream,
    reader: FramedReader,
    writer: FramedWriter<NoHooks>,
    store: ObjectStore,
    limits: Limits,
    client_id: ClientId,
    creds: PeerCreds,
    config: Arc<ServerConfig>,
    handshake_done: bool,
    options: Bitset128,
    retained_fds: u32,
    client_release: u32,
}

impl ClientSession {
    /// Build a session over an accepted stream.
    pub(crate) fn new(
        stream: TransportStream,
        creds: PeerCreds,
        client_id: ClientId,
        config: Arc<ServerConfig>,
    ) -> Result<ClientSession> {
        let connection = REGISTRY
            .interface(CONNECTION_INTERFACE)
            .ok_or(LdpError::Logic {
                what: "connection interface missing from compiled schema",
            })?;
        let mut store = ObjectStore::new();
        store.insert_bootstrap(ObjectEntry {
            interface: connection.name,
            version: connection.version_max,
            kind: ObjectKind::Connection,
        })?;
        Ok(ClientSession {
            reader: FramedReader::new(config.limits),
            writer: FramedWriter::without_hooks(config.limits),
            stream,
            store,
            limits: config.limits,
            client_id,
            creds,
            config,
            handshake_done: false,
            options: Bitset128::EMPTY,
            retained_fds: 0,
            client_release: 0,
        })
    }

    /// The audit identity of this connection.
    #[must_use]
    pub fn client_id(&self) -> ClientId {
        self.client_id
    }

    /// The peer's credentials (read before any protocol byte).
    #[must_use]
    pub fn creds(&self) -> &PeerCreds {
        &self.creds
    }

    /// The limits currently in force (post-`large_messages`).
    #[must_use]
    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    /// The options granted at the handshake.
    #[must_use]
    pub fn options(&self) -> Bitset128 {
        self.options
    }

    /// The client's claimed protocol release.
    #[must_use]
    pub fn client_release(&self) -> u32 {
        self.client_release
    }

    /// Read-only store access.
    #[must_use]
    pub fn store(&self) -> &ObjectStore {
        &self.store
    }

    /// Mutable store access (dispatch paths).
    pub(crate) fn store_mut(&mut self) -> &mut ObjectStore {
        &mut self.store
    }

    pub(crate) fn retained_fds(&self) -> u32 {
        self.retained_fds
    }

    /// Whether the handshake (`hello`/`welcome`) has completed.
    #[must_use]
    pub fn handshake_done(&self) -> bool {
        self.handshake_done
    }

    pub(crate) fn set_retained_fds(&mut self, n: u32) {
        self.retained_fds = n;
    }

    pub(crate) fn audit(&self, record: AuditRecord) {
        self.config.audit.record(record);
    }

    /// Drive the session until it ends: blocking reads, one message at a
    /// time, reclamation at the end. Intended for blocking streams (the
    /// phase-4 one-thread-per-connection model); a `WouldBlock` result —
    /// only possible on nonblocking streams — yields and retries, so
    /// embedding `run` under a poll loop is correct but not efficient.
    ///
    /// Returns how the session ended. Reclamation (dispatcher callback,
    /// audit record) happens here, exactly once.
    pub fn run(&mut self, dispatcher: &mut dyn Dispatcher) -> SessionEnd {
        let end = loop {
            match self.step(dispatcher) {
                SessionOutcome::Continue => {}
                SessionOutcome::Ended(e) => break e,
            }
        };
        let objects: Vec<(ObjectId, ObjectEntry)> =
            self.store.live_objects().map(|(id, e)| (id, *e)).collect();
        dispatcher.on_session_end(self.client_id, &end, &objects);
        self.audit(AuditRecord::Disconnected {
            client: self.client_id,
            reason: end,
        });
        // A delivered fatal error must actually reach the peer: the
        // error-event write can would-block partway (a slow-draining
        // peer), and dropping the session right away would discard the
        // writer's pending tail — the peer would then read a truncated
        // frame and report a crash instead of the error. Push the tail
        // out with bounded patience before the socket closes.
        if matches!(end, SessionEnd::Protocol(_)) {
            self.drain_pending_writes();
        }
        end
    }

    /// Bounded teardown flush of the writer's pending tail (see
    /// [`ClientSession::run`]). A dead peer or an expired deadline
    /// simply stops the attempt — the error delivery was best effort
    /// by contract; this only removes the truncation race.
    fn drain_pending_writes(&mut self) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(200);
        while self.writer.pending_bytes() > 0 {
            if std::time::Instant::now() >= deadline {
                break;
            }
            match self.writer.flush(&mut self.stream) {
                Ok(_) => {}
                Err(_) => break, // the peer is gone; nothing left to deliver
            }
            // A congested nonblocking stream returns immediately (the
            // kernel refused the tail); give the peer a beat to drain
            // instead of spinning the deadline away.
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    /// Process exactly one inbound message (or transport event).
    /// Embedding event loops call this on readability.
    pub fn step(&mut self, dispatcher: &mut dyn Dispatcher) -> SessionOutcome {
        match self.reader.recv_msg(&mut self.stream) {
            Ok(frame) => self.handle_frame(frame, dispatcher),
            Err(e) if is_would_block(&e) => {
                std::thread::yield_now();
                SessionOutcome::Continue
            }
            Err(e) if is_disconnect(&e) => SessionOutcome::Ended(classify_disconnect(&e)),
            Err(e) => self.fatal(&e),
        }
    }

    fn handle_frame(&mut self, frame: Frame, dispatcher: &mut dyn Dispatcher) -> SessionOutcome {
        let fd_count = u32::from(frame.fd_count());
        let msg = match decode(
            frame.message_bytes(),
            fd_count,
            &self.limits,
            ValidationMode::Tolerant,
        ) {
            Ok(m) => m,
            Err(e) => return self.fatal(&e),
        };
        // FDs move out of the frame; untaken ones close at scope end.
        let mut fds = frame.fds;
        match self.dispatch_message(&msg, &mut fds, dispatcher) {
            Ok(()) => {
                // The wake point: the message is fully handled (replies
                // emitted, FD ownership settled) and the residual FD
                // table is still alive for the context. Compositor-side
                // dispatchers pump their frame loops here.
                let woke = {
                    let mut ctx = DispatchCtx::new(self, &mut fds);
                    dispatcher.on_wake(&mut ctx)
                };
                match woke {
                    Ok(()) => SessionOutcome::Continue,
                    Err(e) => self.fatal(&e),
                }
            }
            Err(e) => self.fatal(&e),
        }
    }

    /// Stage 3 + routing for one decoded message.
    fn dispatch_message(
        &mut self,
        msg: &Message,
        fds: &mut FdList,
        dispatcher: &mut dyn Dispatcher,
    ) -> Result<()> {
        if msg.object_id == 0 {
            return Err(LdpError::protocol(
                ErrorCode::InvalidObject,
                None,
                "message target 0 is not an object",
            ));
        }
        let target = ObjectId::from_wire(msg.object_id);

        // The handshake gate runs before any object resolution: any
        // message before `hello` — to any object — is invalid_state
        // (`docs/protocol.md` §6).
        if !self.handshake_done {
            let hello_opcode = REGISTRY
                .interface(CONNECTION_INTERFACE)
                .and_then(|i| i.requests.first())
                .map_or(1, |r| r.opcode);
            let is_hello =
                msg.object_id == ObjectId::CONNECTION.as_u32() && msg.opcode == hello_opcode;
            if !is_hello {
                return Err(LdpError::protocol(
                    ErrorCode::InvalidState,
                    Some(target),
                    "the first message on a connection must be connection.hello",
                ));
            }
        }

        let (interface, version, kind) = match self.store.lookup(target) {
            crate::object::Lookup::NotFound => {
                return Err(LdpError::protocol(
                    ErrorCode::InvalidObject,
                    Some(target),
                    "object ID is not bound on this connection",
                ));
            }
            crate::object::Lookup::Stale(_) => {
                return Err(LdpError::protocol(
                    ErrorCode::StaleObject,
                    Some(target),
                    "object was destroyed; the ID is stale",
                ));
            }
            crate::object::Lookup::Live(entry) => (entry.interface, entry.version, entry.kind),
        };

        // Stage 3: the signature of THIS object's interface and version.
        let (_iface, op) = check_signature(
            &REGISTRY,
            msg,
            interface,
            ldp_protocol::Direction::Request,
            version,
            ValidationMode::Tolerant,
        )?;

        match kind {
            ObjectKind::Connection => {
                builtin::handle_connection_request(self, msg, op, fds, dispatcher)
            }
            ObjectKind::Registry => {
                builtin::handle_registry_request(self, msg, op, target, fds, dispatcher)
            }
            ObjectKind::Global | ObjectKind::External => {
                let request = crate::dispatch::IncomingRequest {
                    object: target,
                    interface,
                    version,
                    op,
                    args: &msg.args,
                };
                let mut ctx = DispatchCtx::new(self, fds);
                dispatcher.on_request(&mut ctx, &request)
            }
        }
    }

    /// Emit one event on a live object (core built-ins and dispatchers
    /// share this path). FIFO order is the writer's queue order, so
    /// `sync_done` emitted after earlier events gives exactly the
    /// sync-barrier guarantee.
    pub(crate) fn send_event(
        &mut self,
        target: ObjectId,
        interface: &str,
        event: &str,
        args: Vec<Value>,
    ) -> Result<()> {
        let mut no_fds = FdList::new();
        self.send_event_fd(target, interface, event, args, &mut no_fds)
    }

    /// Emit one event with a batch of descriptors riding the message
    /// (the server-to-client FD path: `buffer.release` fences). The
    /// `Value::Fd(index)` args address `fds`; the writer takes ownership.
    pub(crate) fn send_event_fd(
        &mut self,
        target: ObjectId,
        interface: &str,
        event: &str,
        args: Vec<Value>,
        fds: &mut FdList,
    ) -> Result<()> {
        let iface = REGISTRY.interface(interface).ok_or(LdpError::Logic {
            what: "emit: interface missing from compiled schema",
        })?;
        let op = iface
            .events
            .iter()
            .find(|e| e.name == event)
            .ok_or(LdpError::Logic {
                what: "emit: event name not declared by the interface",
            })?;
        let mut msg = Message::new(target.as_u32(), op.opcode);
        for a in args {
            msg = msg.arg(a);
        }
        let bytes = msg.encode(&self.limits)?;
        // Congested is fine: the queue preserves order and is bounded by
        // the writer's ceiling (a peer that never reads wedges only
        // itself). Overflow and I/O failures propagate as fatal.
        self.writer.send_msg(&mut self.stream, &bytes, fds)?;
        Ok(())
    }

    /// Create an object on behalf of a dispatcher (the `new_id` path of
    /// interface factories).
    pub(crate) fn create_external(
        &mut self,
        id: ObjectId,
        interface: &str,
        version: u32,
    ) -> Result<ldp_core::ids::Generation> {
        let iface = REGISTRY.interface(interface).ok_or_else(|| {
            LdpError::protocol(
                ErrorCode::InvalidInterface,
                Some(id),
                format!("interface '{interface}' is not in the schema registry"),
            )
        })?;
        if self.store.live_count() >= u64::from(self.limits.client_objects) {
            return Err(LdpError::Limit {
                kind: ldp_core::error::LimitKind::ClientObjects,
                value: self.store.live_count(),
            });
        }
        let gen = self.store.insert_client(
            id,
            ObjectEntry {
                interface: iface.name,
                version,
                kind: ObjectKind::External,
            },
        )?;
        self.audit(AuditRecord::Bind {
            client: self.client_id,
            interface: iface.name.into(),
            version,
            object: id,
        });
        Ok(gen)
    }

    /// Apply the handshake to the session state (called by the built-in
    /// hello handler).
    pub(crate) fn complete_handshake(&mut self, release: u32, options: Bitset128) {
        self.client_release = release;
        self.options = options;
        self.handshake_done = true;
    }

    /// Grant `large_messages`: widen the limits and rebuild the framing
    /// pair. Safe only because this runs while processing the *first*
    /// message: the reader is between messages and the writer has never
    /// queued a byte (the first event is the `welcome` reply itself).
    pub(crate) fn enable_large_messages(&mut self) {
        self.limits = Limits::LARGE_MESSAGES;
        debug_assert_eq!(self.writer.pending_bytes(), 0);
        self.reader = FramedReader::new(self.limits);
        self.writer = FramedWriter::without_hooks(self.limits);
    }

    /// The server configuration this session runs under.
    #[must_use]
    pub fn config(&self) -> &ServerConfig {
        &self.config
    }

    /// Turn a fatal error into the session's end: audit it, deliver
    /// `connection.error` (best effort — the peer may already be gone),
    /// and report the end reason.
    fn fatal(&mut self, e: &LdpError) -> SessionOutcome {
        if let LdpError::Io(_) = e {
            let end = if is_disconnect(e) {
                classify_disconnect(e)
            } else {
                SessionEnd::Io
            };
            return SessionOutcome::Ended(end);
        }
        let (code, object, message) = ServerConfig::error_wire_parts(e);
        self.audit(AuditRecord::Fatal {
            client: self.client_id,
            code,
            object,
            detail: e.to_string().into(),
        });
        let object_wire = object.map_or(0, ObjectId::as_u32);
        // Best effort: congestion or a dead peer must not mask the end.
        let _ = self.send_event(
            ObjectId::CONNECTION,
            CONNECTION_INTERFACE,
            "error",
            vec![
                Value::Enum(code.to_wire()),
                Value::Uint32(object_wire),
                Value::String(message.into()),
            ],
        );
        SessionOutcome::Ended(SessionEnd::Protocol(code))
    }
}

/// Distinguish a between-messages EOF from a mid-message one using the
/// transport's error shapes (kind + "mid-" marker in the message).
fn classify_disconnect(e: &LdpError) -> SessionEnd {
    if let LdpError::Io(io) = e {
        if io.kind() == std::io::ErrorKind::UnexpectedEof {
            let text = io.to_string();
            if !text.contains("mid-") {
                return SessionEnd::Clean;
            }
        }
    }
    SessionEnd::Crash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disconnect_classification_matches_transport_shapes() {
        assert_eq!(
            classify_disconnect(&ldp_transport::error::eof_clean()),
            SessionEnd::Clean
        );
        assert_eq!(
            classify_disconnect(&ldp_transport::error::eof_mid("header")),
            SessionEnd::Crash
        );
        let reset = LdpError::Io(Arc::new(std::io::Error::from(
            std::io::ErrorKind::ConnectionReset,
        )));
        assert_eq!(classify_disconnect(&reset), SessionEnd::Crash);
    }

    #[test]
    fn session_end_display_is_stable() {
        assert_eq!(SessionEnd::Clean.to_string(), "clean disconnect");
        assert_eq!(
            SessionEnd::Protocol(ErrorCode::InvalidOpcode).to_string(),
            "protocol error invalid_opcode"
        );
    }
}
