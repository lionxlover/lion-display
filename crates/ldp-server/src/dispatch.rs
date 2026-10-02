//! The dispatcher: interface implementations beyond the session core.
//!
//! The session core owns `ldp.core.connection` and `ldp.core.registry`.
//! Every request on any other object — a surface commit, a data-device
//! offer, a session request — is forwarded to a [`Dispatcher`] after
//! passing all three validation stages. The dispatcher acts through
//! [`DispatchCtx`]: emitting events, creating and revoking objects, and
//! taking ownership of the message's file descriptors.
//!
//! Phase 4 ships the routing and lifecycle machinery with
//! [`NullDispatcher`] (consume and audit). The compositor (Phase 6+) is
//! *a* dispatcher, not a fork of the session core — this trait is the
//! seam that keeps protocol plumbing and window system logic apart.

use ldp_core::error::{ErrorCode, LdpError, Result};
use ldp_core::ids::{ClientId, Generation, ObjectId};
use ldp_core::wire::Value;
use ldp_protocol::schema::OpSchema;
use ldp_protocol::REGISTRY;
use std::os::fd::OwnedFd;

use crate::object::{ObjectEntry, ObjectKind, ObjectStore};
use crate::session::{ClientSession, SessionEnd};

/// One validated, fully stage-3-checked request on a non-core object.
#[derive(Debug)]
pub struct IncomingRequest<'a> {
    /// The object the request targets.
    pub object: ObjectId,
    /// Interface of the target (from the object store).
    pub interface: &'static str,
    /// Version the object was pinned at.
    pub version: u32,
    /// The matched operation signature.
    pub op: &'static OpSchema,
    /// Decoded arguments, in order.
    pub args: &'a [Value],
}

/// Interface implementations outside the session core.
///
/// One dispatcher instance serves one session (the server spawns one per
/// connection); wrap shared state in `Arc<Mutex<…>>` when a fleet-wide
/// view is needed.
pub trait Dispatcher: Send {
    /// Handle one request. Returning `Err` is a *fatal* protocol error
    /// for the connection (the taxonomy has no non-fatal rejections);
    /// `Ok(())` means the request was consumed — replies, if any, were
    /// emitted through `ctx`.
    ///
    /// Untaken file descriptors of the message are closed by the session
    /// when the call returns.
    ///
    /// # Errors
    ///
    /// Any [`LdpError`] the implementation considers fatal: protocol
    /// errors (the taxonomy of `docs/protocol.md` §8), limits, or I/O
    /// failures — the session core turns them into `connection.error`
    /// and the connection's end.
    fn on_request(
        &mut self,
        ctx: &mut DispatchCtx<'_>,
        request: &IncomingRequest<'_>,
    ) -> Result<()>;

    /// The session ended: reclaim per-object state and every file
    /// descriptor taken via [`DispatchCtx::take_fd`]. The object list is
    /// the store's final contents (still live objects).
    fn on_session_end(
        &mut self,
        client: ClientId,
        reason: &SessionEnd,
        objects: &[(ObjectId, ObjectEntry)],
    ) {
        let _ = (client, reason, objects);
    }

    /// A global was bound (or a singleton object created via
    /// `registry.bind`): the object is live in the store and `bound` has
    /// been emitted. This is where globals with initial events act —
    /// `ldp.core.output` replays its geometry/mode cascade, `ldp.core.shm`
    /// advertises its formats. The hook exists for exactly the
    /// compositor-shaped dispatchers of Phase 10+; the default consumes
    /// the bind.
    ///
    /// # Errors
    ///
    /// Fatal protocol errors end the connection (same contract as
    /// [`Dispatcher::on_request`]); emission failures propagate.
    fn on_bind(
        &mut self,
        _ctx: &mut DispatchCtx<'_>,
        _object: ObjectId,
        _interface: &str,
        _version: u32,
    ) -> Result<()> {
        Ok(())
    }

    /// The client destroyed an object through the central
    /// `connection.destroy` lifecycle: the object is already removed
    /// from the store and `destroyed` has been emitted. Dispatchers that
    /// shadow protocol objects with real resources (surfaces, pools,
    /// buffers) release them here.
    ///
    /// # Errors
    ///
    /// As [`Dispatcher::on_bind`].
    fn on_destroy(
        &mut self,
        _ctx: &mut DispatchCtx<'_>,
        _object: ObjectId,
        _interface: &str,
    ) -> Result<()> {
        Ok(())
    }

    /// A registry object was created via `connection.get_registry`
    /// (Phase 44, the dynamic-globals seam): the object is live in the
    /// store and the initial `global` replay is complete. Servers with
    /// a dynamic advertisement set track the registry here — it is
    /// the delivery target for every later `registry.global` /
    /// `global_remove` fan-out. The default consumes the registry
    /// (the static-server behavior: the advertisement set never
    /// changes after the replay).
    ///
    /// # Errors
    ///
    /// As [`Dispatcher::on_bind`].
    fn on_registry(&mut self, _ctx: &mut DispatchCtx<'_>, _object: ObjectId) -> Result<()> {
        Ok(())
    }

    /// A wake point: the session just finished handling one inbound
    /// message successfully (request, sync, ping, anything). Long-lived
    /// servers whose state advances between client messages — the
    /// Phase-10 frame loop advancing its virtual clock to pending
    /// device events — pump here and emit what became due.
    ///
    /// Emission targets must be live objects; the residual FD table of
    /// the just-handled message is still attached to `ctx` (untaken
    /// descriptors close when the call returns, same as `on_request`).
    ///
    /// # Errors
    ///
    /// As [`Dispatcher::on_bind`].
    fn on_wake(&mut self, _ctx: &mut DispatchCtx<'_>) -> Result<()> {
        Ok(())
    }
}

/// The default dispatcher: consumes every request; the session audits it
/// as unhandled. Used by the phase-4 server and by tests that exercise
/// only the core protocol.
#[derive(Clone, Copy, Debug, Default)]
pub struct NullDispatcher;

impl Dispatcher for NullDispatcher {
    fn on_request(
        &mut self,
        _ctx: &mut DispatchCtx<'_>,
        _request: &IncomingRequest<'_>,
    ) -> Result<()> {
        Ok(())
    }
}

/// What a dispatcher may do with the session it is serving.
pub struct DispatchCtx<'a> {
    session: &'a mut ClientSession,
    fds: &'a mut ldp_transport::FdList,
}

impl<'a> DispatchCtx<'a> {
    /// Assemble from the session and the message's FD table (the session
    /// calls this; dispatchers never construct contexts).
    pub(crate) fn new(session: &'a mut ClientSession, fds: &'a mut ldp_transport::FdList) -> Self {
        DispatchCtx { session, fds }
    }

    /// The connection's audit identity.
    #[must_use]
    pub fn client_id(&self) -> ClientId {
        self.session.client_id()
    }

    /// The peer's `SO_PEERCRED` identity.
    #[must_use]
    pub fn creds(&self) -> &ldp_transport::PeerCreds {
        self.session.creds()
    }

    /// The negotiated limits.
    #[must_use]
    pub fn limits(&self) -> ldp_core::limits::Limits {
        *self.session.limits()
    }

    /// Read-only view of the object store.
    #[must_use]
    pub fn store(&self) -> &ObjectStore {
        self.session.store()
    }

    /// FDs riding the current message.
    #[must_use]
    pub fn fd_count(&self) -> u32 {
        self.fds.len() as u32
    }

    /// Take ownership of FD `index` of the current message (the natural
    /// pattern is to take exactly what the signature's `fd` arguments
    /// reference).
    ///
    /// Taken FDs count against the connection's `client_fds` ceiling
    /// until [`DispatchCtx::fd_released`] reports them closed — closing
    /// is the dispatcher's job (usually by dropping the `OwnedFd`).
    ///
    /// # Errors
    ///
    /// [`ErrorCode::LimitExceeded`] when the retained-FD ceiling is hit;
    /// `None`-by-`Err` is impossible for in-range indices, out-of-range
    /// indices are a caller bug reported as [`ErrorCode::FdMismatch`].
    pub fn take_fd(&mut self, index: u32) -> Result<OwnedFd> {
        let retained = self.session.retained_fds();
        if retained >= self.session.limits().client_fds {
            return Err(LdpError::Limit {
                kind: ldp_core::error::LimitKind::ClientFds,
                value: u64::from(retained),
            });
        }
        match self.fds.remove(index as usize) {
            Some(fd) => {
                self.session.set_retained_fds(retained + 1);
                Ok(fd)
            }
            None => Err(LdpError::malformed(
                ErrorCode::FdMismatch,
                format!("fd index {index} is not in this message's table"),
            )),
        }
    }

    /// Report that a previously taken FD was closed (release accounting).
    pub fn fd_released(&mut self) {
        let retained = self.session.retained_fds();
        self.session.set_retained_fds(retained.saturating_sub(1));
    }

    /// Emit an event on a live object. The event name is looked up in
    /// the *target object's* interface — emitting an event of another
    /// interface is a server bug, not a protocol state.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::InvalidObject`] when the target is not live;
    /// [`ErrorCode::InvalidOpcode`] when the interface declares no such
    /// event; transport and limit errors propagate (fatal).
    pub fn emit(&mut self, target: ObjectId, event: &str, args: Vec<Value>) -> Result<()> {
        let interface = match self.session.store().lookup(target) {
            crate::object::Lookup::Live(entry) => entry.interface,
            _ => {
                return Err(LdpError::protocol(
                    ErrorCode::InvalidObject,
                    Some(target),
                    "emit: target object is not live",
                ));
            }
        };
        self.session.send_event(target, interface, event, args)
    }

    /// Emit an event that carries file descriptors (the server-to-client
    /// FD path: `buffer.release` fences). `Value::Fd(index)` arguments
    /// address `fds`, from which the writer takes ownership of every
    /// descriptor on success — on failure the batch is untouched and
    /// closing remains the caller's job.
    ///
    /// # Errors
    ///
    /// As [`DispatchCtx::emit`].
    pub fn emit_fd(
        &mut self,
        target: ObjectId,
        event: &str,
        args: Vec<Value>,
        fds: &mut ldp_transport::FdList,
    ) -> Result<()> {
        let interface = match self.session.store().lookup(target) {
            crate::object::Lookup::Live(entry) => entry.interface,
            _ => {
                return Err(LdpError::protocol(
                    ErrorCode::InvalidObject,
                    Some(target),
                    "emit_fd: target object is not live",
                ));
            }
        };
        self.session
            .send_event_fd(target, interface, event, args, fds)
    }

    /// Create an object owned by the client (the `new_id` of the request
    /// currently being handled) at the given interface.
    ///
    /// # Errors
    ///
    /// Propagates the store's ownership/collision checks
    /// ([`ErrorCode::InvalidObject`], [`ErrorCode::InvalidState`]) and
    /// [`ErrorCode::LimitExceeded`] for the object-count ceiling;
    /// [`ErrorCode::InvalidInterface`] for unknown interface names.
    pub fn create_object(
        &mut self,
        id: ObjectId,
        interface: &str,
        version: u32,
    ) -> Result<Generation> {
        self.session.create_external(id, interface, version)
    }

    /// Create a server-announced object at a *specific* server-range
    /// ID — the routed-announcement path (the data family's offer
    /// objects): the id rides the event's `new_id` argument, minted by
    /// the owning subsystem's world-wide counter, so the store holds
    /// exactly that id.
    ///
    /// # Errors
    ///
    /// As [`ObjectStore::insert_server_at`] (non-server-range or live
    /// ID); [`ErrorCode::LimitExceeded`] for the object-count ceiling;
    /// [`ErrorCode::InvalidInterface`] for unknown interface names.
    pub fn create_announced(&mut self, id: ObjectId, interface: &str, version: u32) -> Result<()> {
        let iface = REGISTRY.interface(interface).ok_or_else(|| {
            LdpError::protocol(
                ErrorCode::InvalidInterface,
                Some(id),
                format!("interface '{interface}' is not in the schema registry"),
            )
        })?;
        if self.session.store().live_count() >= u64::from(self.session.limits().client_objects) {
            return Err(LdpError::Limit {
                kind: ldp_core::error::LimitKind::ClientObjects,
                value: self.session.store().live_count(),
            });
        }
        self.session.store_mut().insert_server_at(
            id,
            ObjectEntry {
                interface: iface.name,
                version,
                kind: ObjectKind::External,
            },
        )?;
        self.session.audit(crate::audit::AuditRecord::Bind {
            client: self.session.client_id(),
            interface: iface.name.into(),
            version,
            object: id,
        });
        Ok(())
    }

    /// Revoke an object the client did not destroy: removes it from the
    /// store and emits `connection.revoked` — the server-initiated
    /// teardown path (`docs/protocol.md` §3.2).
    ///
    /// # Errors
    ///
    /// [`ErrorCode::InvalidObject`] / [`ErrorCode::InvalidState`] from
    /// the store (a revoked-twice race is a server bug); emission errors
    /// propagate.
    pub fn revoke(&mut self, target: ObjectId, reason: u32) -> Result<()> {
        let entry = self.session.store_mut().remove(target)?;
        self.session.audit(crate::audit::AuditRecord::Revoke {
            client: self.session.client_id(),
            interface: entry.interface.into(),
            object: target,
            reason,
        });
        self.session.send_event(
            ObjectId::CONNECTION,
            crate::session::CONNECTION_INTERFACE,
            "revoked",
            vec![Value::Uint32(target.as_u32()), Value::Enum(reason)],
        )
    }

    /// Object kind of a live object (routing information for shared
    /// dispatchers).
    #[must_use]
    pub fn kind_of(&self, id: ObjectId) -> Option<ObjectKind> {
        match self.session.store().lookup(id) {
            crate::object::Lookup::Live(entry) => Some(entry.kind),
            _ => None,
        }
    }
}
