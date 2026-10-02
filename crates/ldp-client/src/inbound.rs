//! The inbound pipeline: stages 1–3-in and the dispatch drain.
//!
//! [`Connection::pump_one`] reads one frame (transport stage 1),
//! structurally decodes it (stage 2), resolves the target proxy
//! *before* checking the event signature against that proxy's
//! interface and version (stage 3 — the client-side type-confusion
//! defense), classifies the event, and queues it in its lane.
//! [`Connection::dispatch_pending`] / [`Connection::dispatch_budget`]
//! drain dispatchable events: internal bookkeeping first (sync/pong
//! cookies, proxy removal on destroyed/revoked, automatic proxies for
//! server-announced objects), then the application handler.

use ldp_core::error::{ErrorCode, LdpError};
use ldp_core::ids::ObjectId;
use ldp_core::wire::{ArgType, Value};
use ldp_protocol::{check_signature, decode, Direction, ValidationMode, REGISTRY};
use ldp_transport::error::{is_disconnect, is_would_block};

use crate::class::class_of;
use crate::connection::{Connection, PumpOutcome};
use crate::core_api;
use crate::core_api::EventHandler;
use crate::error::{classify_disconnect, ClientError, DisconnectKind, Result};
use crate::queue::Event;

impl Connection {
    /// Read exactly one message off the socket into the queue
    /// (blocking): stages 1–3-in of the §9 pipeline.
    ///
    /// # Errors
    ///
    /// [`ClientError::Disconnected`] when the server goes away (sticky:
    /// later calls repeat it); [`ClientError::Ldp`] for
    /// framing/codec/signature failures (fatal by contract);
    /// [`ClientError::UnexpectedEvent`] for events targeting objects
    /// this client has no proxy for.
    pub fn pump_one(&mut self) -> Result<PumpOutcome> {
        self.ensure_alive()?;
        let frame = match self.reader.recv_msg(&mut self.stream) {
            Ok(f) => f,
            Err(e) => {
                // Would-block on a nonblocking socket is not a failure:
                // the reader retains any partial message.
                if is_would_block(&e) && self.stream.is_nonblocking() {
                    return Ok(PumpOutcome::WouldBlock);
                }
                return Err(self.note_disconnect(&e));
            }
        };
        let msg = decode(
            frame.message_bytes(),
            u32::from(frame.fd_count()),
            &self.limits,
            ValidationMode::Tolerant,
        )?;
        if msg.object_id == 0 {
            return Err(self.note_protocol_violation(None, "event target 0 is not an object"));
        }
        let target = ObjectId::from_wire(msg.object_id);
        // Stage 3: proxy resolution BEFORE the signature check.
        let Some(proxy) = self.proxies.get(target) else {
            return Err(ClientError::UnexpectedEvent {
                target: msg.object_id,
            });
        };
        let (interface, version) = (proxy.interface(), proxy.version());
        let (_iface, op) = match check_signature(
            &REGISTRY,
            &msg,
            interface,
            Direction::Event,
            version,
            ValidationMode::Tolerant,
        ) {
            Ok(found) => found,
            Err(e) => return Err(self.note_protocol_violation(Some(target), e.to_string())),
        };
        let event = Event {
            seq: 0, // assigned by push
            target,
            interface,
            version,
            op,
            args: msg.args,
            fds: frame.fds,
            class: class_of(interface, op.name),
            urgent: msg.flags.is_urgent(),
        };
        self.queue.push(event);
        Ok(PumpOutcome::Event)
    }

    /// Drain every dispatchable event to the handler (unbounded batch).
    /// Returns the number of events dispatched.
    ///
    /// # Errors
    ///
    /// Handler errors abort the drain and propagate;
    /// [`ClientError::ServerError`] when a fatal `connection.error` was
    /// dispatched (the connection is dead afterwards — the error event
    /// itself *was* forwarded to the handler first).
    pub fn dispatch_pending(&mut self, handler: &mut dyn EventHandler) -> Result<usize> {
        self.dispatch_budget(handler, usize::MAX)
    }

    /// Drain at most `budget` dispatchable events (bounded batches for
    /// event-loop integration; the input-latency guarantee holds for
    /// any budget >= 1).
    ///
    /// # Errors
    ///
    /// As [`Self::dispatch_pending`].
    pub fn dispatch_budget(
        &mut self,
        handler: &mut dyn EventHandler,
        budget: usize,
    ) -> Result<usize> {
        let mut n = 0usize;
        while n < budget {
            let Some(event) = self.queue.pop_next() else {
                break;
            };
            self.dispatch_one(&event, handler)?;
            drop(event); // untaken FDs close here
            n += 1;
        }
        Ok(n)
    }

    /// One event through internal bookkeeping and the handler. The
    /// caller owns the event (and therefore the FD-drop point).
    fn dispatch_one(&mut self, event: &Event, handler: &mut dyn EventHandler) -> Result<()> {
        // Internal reactions run first so the handler observes
        // post-reaction state (the proxy map already reflects
        // destroyed/revoked objects, new server objects are bound).
        match (event.interface, event.op.name) {
            (crate::proxy::CONNECTION_INTERFACE, "sync_done") => {
                let done = core_api::SyncDone::from_event(event)?;
                self.last_sync_cookie = Some(done.cookie);
            }
            (crate::proxy::CONNECTION_INTERFACE, "pong") => {
                let pong = core_api::Pong::from_event(event)?;
                self.last_pong_cookie = Some(pong.cookie);
            }
            (crate::proxy::CONNECTION_INTERFACE, "destroyed") => {
                let d = core_api::Destroyed::from_event(event)?;
                let _removed = self.proxies.remove(ObjectId::from_wire(d.object_id));
            }
            (crate::proxy::CONNECTION_INTERFACE, "revoked") => {
                let r = core_api::Revoked::from_event(event)?;
                let _removed = self.proxies.remove(ObjectId::from_wire(r.object_id));
            }
            (crate::proxy::CONNECTION_INTERFACE, "error") => {
                // Forward to the application first (it sees the full
                // diagnosis), then kill the connection.
                let fatal = core_api::FatalError::from_event(event)?;
                handler.on_event(event)?;
                self.disconnect.get_or_insert(DisconnectKind::LocalDrop);
                return Err(ClientError::ServerError {
                    code: fatal.code,
                    object: fatal.object_id,
                    message: fatal.message,
                });
            }
            _ => {
                // Server-created objects: bind proxies for every new_id
                // the event carries (e.g. data_device.data_offer).
                for (arg, value) in event.op.args.iter().zip(event.args.iter()) {
                    if arg.ty == ArgType::NewId {
                        if let Value::NewId(id) = value {
                            // A collision would be a server bug; surface
                            // it as the protocol violation it is.
                            self.proxies.insert_server(
                                *id,
                                arg.of.unwrap_or(""),
                                event.interface,
                            )?;
                        }
                    }
                }
            }
        }
        handler.on_event(event).map_err(ClientError::from)
    }

    /// The raw socket descriptor (epoll/poll registration; borrow only).
    #[must_use]
    pub fn raw_fd(&self) -> i32 {
        self.stream.raw_fd()
    }

    /// Flip the stream's blocking mode (event-loop integration; do it
    /// after the handshake, which requires blocking mode).
    ///
    /// # Errors
    ///
    /// Transport failures from fcntl.
    pub fn set_nonblocking(&mut self, on: bool) -> Result<()> {
        self.stream.set_nonblocking(on).map_err(ClientError::from)
    }

    /// Record a transport failure as the connection's end (sticky).
    pub(crate) fn note_disconnect(&mut self, e: &LdpError) -> ClientError {
        let kind = if is_disconnect(e) {
            classify_disconnect(e)
        } else {
            DisconnectKind::Transport
        };
        self.disconnect.get_or_insert(kind.clone());
        ClientError::Disconnected(kind)
    }

    /// Record a server-side protocol violation (client-side defense).
    fn note_protocol_violation(
        &mut self,
        object: Option<ObjectId>,
        what: impl Into<String>,
    ) -> ClientError {
        self.disconnect.get_or_insert(DisconnectKind::LocalDrop);
        ClientError::Ldp(LdpError::protocol(
            ErrorCode::MalformedMessage,
            object,
            what,
        ))
    }
}
