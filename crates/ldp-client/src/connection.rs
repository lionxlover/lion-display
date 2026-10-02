//! The connection: handshake, the inbound §9 pipeline, dispatch, and
//! the round-trip primitive.
//!
//! [`Connection`] owns one `AF_UNIX` stream and its framing pair, the
//! proxy map, the ID allocator, and the class-lane queue. The inbound
//! pipeline mirrors the server's (`docs/protocol.md` §9) with the
//! direction flipped: framing (transport stage 1) → structural decode
//! (stage 2) → **proxy resolution before the signature check** (stage 3
//! — the client-side type-confusion defense: an event must match the
//! interface of the object it targets, at that object's pinned version)
//! → classification into a dispatch lane.
//!
//! Driving model (v1): one thread per connection, blocking reads.
//! [`Connection::dispatch_pending`] drains dispatchable events to the
//! handler; [`Connection::pump_one`] reads one message off the socket
//! into the queue; [`Connection::roundtrip`] is the sync barrier
//! ("everything the server queued before my sync has been dispatched").
//! Nonblocking event loops flip the stream after the handshake and
//! drive [`Connection::dispatch_budget`] plus [`PumpOutcome::WouldBlock`]
//! themselves.
//!
//! # Handler re-entrancy
//!
//! Handlers receive [`Event`](crate::queue::Event) references only — no
//! connection handle. Re-entrant sends from inside dispatch are a
//! compile error by construction, which removes the classic
//! dispatch-inside-dispatch deadlock the hard way.

use std::time::Instant;

use ldp_core::error::{ErrorCode, LdpError};
use ldp_core::limits::Limits;
use ldp_core::wire::Value;
use ldp_protocol::REGISTRY;
use ldp_transport::addr::UnixAddr;
use ldp_transport::backpressure::NoHooks;
use ldp_transport::reader::FramedReader;
use ldp_transport::stream::TransportStream;
use ldp_transport::writer::FramedWriter;

use crate::allocator::IdAllocator;
use crate::class::EventClass;
use crate::config::ClientConfig;
use crate::core_api::{self, EventHandler};
use crate::error::{ClientError, DisconnectKind, Result};
use crate::proxy::{Proxy, ProxyMap};
use crate::queue::EventQueue;

/// Interface name of the registry (materialized by `get_registry`).
const REGISTRY_INTERFACE: &str = "ldp.core.registry";

/// Result of [`Connection::pump_one`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PumpOutcome {
    /// One message was validated and queued.
    Event,
    /// The nonblocking socket had nothing to read; retry on readiness.
    /// (Partial in-flight messages are retained by the reader.)
    WouldBlock,
}

/// Which round-trip completion the dispatch loop is waiting for.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Awaited {
    Sync(u32),
    Pong(u32),
}

/// One client connection to an LDP server.
pub struct Connection {
    pub(crate) stream: TransportStream,
    pub(crate) reader: FramedReader,
    pub(crate) writer: FramedWriter<NoHooks>,
    pub(crate) limits: Limits,
    config: ClientConfig,
    pub(crate) allocator: IdAllocator,
    pub(crate) proxies: ProxyMap,
    pub(crate) queue: EventQueue,
    pub(crate) welcome: Option<core_api::Welcome>,
    cookie_seq: u32,
    pub(crate) last_sync_cookie: Option<u32>,
    pub(crate) last_pong_cookie: Option<u32>,
    pub(crate) disconnect: Option<DisconnectKind>,
    pub(crate) awaited: Option<Awaited>,
}

impl Connection {
    /// Connect and handshake with the default configuration.
    ///
    /// # Errors
    ///
    /// [`ClientError::Ldp`] for transport failures (nothing listening,
    /// bad address) or a handshake timeout; [`ClientError::ServerError`]
    /// when the server rejects the handshake.
    pub fn connect(addr: &UnixAddr) -> Result<Connection> {
        Connection::connect_with(ClientConfig::default(), addr)
    }

    /// Connect and handshake under an explicit configuration.
    ///
    /// Sends `connection.hello(release, options)` and blocks for
    /// `welcome` (bounded by `handshake_timeout`; the stream is
    /// blocking until the handshake completes — flip to nonblocking
    /// afterwards for event-loop driving). A client requesting
    /// `large_messages` switches its own framing to 64 MiB after the
    /// request: the granted set is observable through what works
    /// (`docs/protocol.md` §5), so the reader must be ready for a
    /// granting server immediately.
    ///
    /// # Errors
    ///
    /// [`ClientError::Ldp`] for transport or timeout failures;
    /// [`ClientError::ServerError`] when the server rejects the
    /// handshake.
    pub fn connect_with(config: ClientConfig, addr: &UnixAddr) -> Result<Connection> {
        let stream = TransportStream::connect(addr)?;
        let mut conn = Connection {
            stream,
            reader: FramedReader::new(config.limits),
            writer: FramedWriter::without_hooks(config.limits),
            limits: config.limits,
            config,
            allocator: IdAllocator::new(),
            proxies: ProxyMap::new(),
            queue: EventQueue::new(),
            welcome: None,
            cookie_seq: 1,
            last_sync_cookie: None,
            last_pong_cookie: None,
            disconnect: None,
            awaited: None,
        };
        conn.handshake()?;
        Ok(conn)
    }

    /// The handshake: hello out, welcome in.
    fn handshake(&mut self) -> Result<()> {
        if self.welcome.is_some() {
            return Err(ClientError::HandshakeState(
                "the handshake completed already",
            ));
        }
        let proxy = Proxy::connection();
        self.send_request(
            &proxy,
            "hello",
            vec![
                Value::Uint32(self.config.release),
                Value::Bitset(self.config.options),
            ],
        )?;
        // large_messages request: widen our framing right away (see the
        // method docs for the negotiation reasoning).
        if self.config.wants_large_messages() {
            self.limits = Limits::LARGE_MESSAGES;
            self.reader = FramedReader::new(Limits::LARGE_MESSAGES);
            self.writer = FramedWriter::without_hooks(Limits::LARGE_MESSAGES);
        }
        let deadline = Instant::now() + self.config.handshake_timeout;
        loop {
            if Instant::now() >= deadline {
                return Err(ClientError::Ldp(LdpError::Logic {
                    what: "handshake timed out waiting for connection.welcome",
                }));
            }
            self.pump_one()?;
            // Nothing may precede welcome: the queue holds exactly it.
            if let Some(event) = self.queue.pop_next() {
                let is_welcome = event.interface == crate::proxy::CONNECTION_INTERFACE
                    && event.op.name == "welcome";
                if !is_welcome {
                    return Err(ClientError::Ldp(LdpError::protocol(
                        ErrorCode::InvalidState,
                        Some(event.target),
                        "an event arrived before connection.welcome",
                    )));
                }
                self.welcome = Some(core_api::Welcome::from_event(&event)?);
                return Ok(());
            }
        }
    }

    /// The server's `welcome` (present after the handshake).
    #[must_use]
    pub fn welcome(&self) -> Option<&core_api::Welcome> {
        self.welcome.as_ref()
    }

    /// The negotiated limits (post `large_messages` switch).
    #[must_use]
    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// The configuration this connection runs under.
    #[must_use]
    pub fn config(&self) -> &ClientConfig {
        &self.config
    }

    /// How the connection ended, once it did (`None` while live).
    #[must_use]
    pub fn disconnect_reason(&self) -> Option<&DisconnectKind> {
        self.disconnect.as_ref()
    }

    /// Whether the connection is still usable.
    #[must_use]
    pub fn is_alive(&self) -> bool {
        self.disconnect.is_none()
    }

    /// The proxy of the pre-bound connection object.
    #[must_use]
    pub fn connection_proxy(&self) -> Proxy {
        Proxy::connection()
    }

    /// The tracked registry proxy, if one exists.
    #[must_use]
    pub fn registry_proxy(&self) -> Option<Proxy> {
        self.proxies
            .entries()
            .find(|(p, _)| p.interface() == REGISTRY_INTERFACE)
            .map(|(p, _)| p.clone())
    }

    /// Queue depth per class (diagnostics and backpressure probes).
    #[must_use]
    pub fn queued(&self, class: EventClass) -> usize {
        self.queue.len(class)
    }

    /// Materialize the registry object (`connection.get_registry`)
    /// pinned at the schema's maximum version. Idempotent per call: it
    /// returns a *fresh* registry object every time (the protocol
    /// allows multiple registries), so callers that want one registry
    /// should check [`Self::registry_proxy`] first — [`Self::bind`]
    /// does exactly that.
    ///
    /// # Errors
    ///
    /// Transport/codec failures, ID exhaustion, or a dead connection.
    pub fn registry(&mut self) -> Result<Proxy> {
        self.ensure_alive()?;
        let iface = REGISTRY
            .interface(REGISTRY_INTERFACE)
            .ok_or(ClientError::Ldp(LdpError::Logic {
                what: "registry interface missing from the compiled schema",
            }))?;
        let new_id = self.allocator.allocate()?;
        let proxy = Proxy::new(new_id, iface.name, iface.version_max);
        self.proxies.insert_client(proxy.clone())?;
        self.send_request(
            &Proxy::connection(),
            "get_registry",
            vec![Value::Uint32(iface.version_max), Value::NewId(new_id)],
        )?;
        Ok(proxy)
    }

    /// Bind an advertised global at the schema's maximum version. A
    /// registry object is materialized automatically when none exists.
    ///
    /// The returned proxy is usable immediately; the `registry.bound`
    /// confirmation is delivered as an event (a following
    /// [`Self::roundtrip`] guarantees it has been dispatched).
    ///
    /// # Errors
    ///
    /// [`ClientError::NoSuchRequest`] when the interface is not in the
    /// schema; transport failures; a dead connection.
    pub fn bind(&mut self, interface: &str) -> Result<Proxy> {
        let version = REGISTRY.interface(interface).map_or(1, |i| i.version_max);
        self.bind_version(interface, version)
    }

    /// Bind at an explicit version (the server rejects out-of-range
    /// versions with a fatal `unsupported_version`).
    ///
    /// # Errors
    ///
    /// [`ClientError::NoSuchRequest`] when the interface is not in the
    /// schema; [`ClientError::Ldp`] on transport failure; ID
    /// exhaustion; a dead connection.
    pub fn bind_version(&mut self, interface: &str, version: u32) -> Result<Proxy> {
        self.ensure_alive()?;
        let iface = REGISTRY
            .interface(interface)
            .ok_or(ClientError::NoSuchRequest {
                interface: interface.into(),
                request: "bind".into(),
            })?;
        let registry = match self.registry_proxy() {
            Some(r) => r,
            None => self.registry()?,
        };
        let new_id = self.allocator.allocate()?;
        let proxy = Proxy::new(new_id, iface.name, version);
        self.proxies.insert_client(proxy.clone())?;
        self.send_request(
            &registry,
            "bind",
            vec![
                Value::String(interface.into()),
                Value::Uint32(version),
                Value::NewId(new_id),
            ],
        )?;
        Ok(proxy)
    }

    /// Send `connection.destroy(proxy)` with a fresh cookie and mark the
    /// proxy pending-destroy. The `destroyed` confirmation arrives as an
    /// event (dispatch removes the proxy from the map automatically).
    ///
    /// # Errors
    ///
    /// [`ClientError::UnknownProxy`] for foreign/untracked proxies;
    /// transport failures; a dead connection.
    pub fn destroy(&mut self, proxy: &Proxy) -> Result<u32> {
        self.ensure_alive()?;
        self.proxies.mark_pending_destroy(proxy.id())?;
        let cookie = self.next_cookie();
        self.send_request(
            &Proxy::connection(),
            "destroy",
            vec![Value::Uint32(proxy.id().as_u32()), Value::Uint32(cookie)],
        )?;
        Ok(cookie)
    }

    /// The sync round-trip: send `connection.sync(cookie)`, dispatch
    /// until the matching `sync_done` — which the barrier rule
    /// guarantees arrives after every event the server queued before
    /// the sync. Requires the blocking driving model (v1).
    ///
    /// # Errors
    ///
    /// Handler errors propagate; transport failures and server errors
    /// end the connection (see [`ClientError`]).
    pub fn roundtrip(&mut self, handler: &mut dyn EventHandler) -> Result<()> {
        self.await_roundtrip(handler, RoundtripKind::Sync)
    }

    /// The keepalive round-trip (`ping` → `pong`).
    ///
    /// # Errors
    ///
    /// Same family as [`Self::roundtrip`].
    pub fn ping(&mut self, handler: &mut dyn EventHandler) -> Result<()> {
        self.await_roundtrip(handler, RoundtripKind::Pong)
    }

    /// Shared body of `roundtrip`/`ping`.
    fn await_roundtrip(
        &mut self,
        handler: &mut dyn EventHandler,
        kind: RoundtripKind,
    ) -> Result<()> {
        self.ensure_alive()?;
        let cookie = self.next_cookie();
        let request = match kind {
            RoundtripKind::Sync => "sync",
            RoundtripKind::Pong => "ping",
        };
        self.send_request(&Proxy::connection(), request, vec![Value::Uint32(cookie)])?;
        self.awaited = Some(match kind {
            RoundtripKind::Sync => Awaited::Sync(cookie),
            RoundtripKind::Pong => Awaited::Pong(cookie),
        });
        let r = self.await_completion(handler);
        self.awaited = None;
        r
    }

    /// Dispatch until the awaited completion is observed.
    fn await_completion(&mut self, handler: &mut dyn EventHandler) -> Result<()> {
        loop {
            self.dispatch_pending(handler)?;
            let done = match self.awaited {
                Some(Awaited::Sync(c)) => self.last_sync_cookie == Some(c),
                Some(Awaited::Pong(c)) => self.last_pong_cookie == Some(c),
                None => true,
            };
            if done {
                return Ok(());
            }
            // The completion has not arrived yet: block for more input.
            // A disconnect surfaces as the returned error.
            self.pump_one()?;
        }
    }

    fn next_cookie(&mut self) -> u32 {
        let c = self.cookie_seq;
        self.cookie_seq = c.wrapping_add(1);
        c
    }

    pub(crate) fn ensure_alive(&self) -> Result<()> {
        match &self.disconnect {
            Some(kind) => Err(ClientError::Disconnected(kind.clone())),
            None => Ok(()),
        }
    }
}

/// Manual `Debug`: the connection holds a stream and framing state;
/// the interesting summary is identity + liveness + queue depth.
impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection")
            .field("alive", &self.is_alive())
            .field("disconnect", &self.disconnect)
            .field("queued_total", &self.queue.pushed())
            .finish_non_exhaustive()
    }
}

/// Which round-trip primitive is in flight.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum RoundtripKind {
    /// `sync` → `sync_done`.
    Sync,
    /// `ping` → `pong`.
    Pong,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pump_outcome_and_awaited_are_debug() {
        // Compile-level contract checks for types that never reach a
        // public assert otherwise.
        let o = PumpOutcome::Event;
        assert!(format!("{o:?}").contains("Event"));
        let a = Awaited::Sync(3);
        assert!(format!("{a:?}").contains("Sync"));
        let k = RoundtripKind::Pong;
        assert!(format!("{k:?}").contains("Pong"));
    }
}
