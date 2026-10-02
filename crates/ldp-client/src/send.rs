//! The outbound request path: schema-checked sends, factory requests,
//! and writer flushing.
//!
//! Every request passes the same client-side stage-3 discipline the
//! server applies: the message is signature-checked against the target
//! proxy's interface and pinned version *before* any byte reaches the
//! wire. [`Connection::create_object`] additionally splices an
//! allocator-fresh `new_id` into the schema-declared argument position
//! and registers the resulting proxy — the typed factory flow
//! (`compositor.create_surface`, `seat.get_pointer`, …).
//!
//! Congestion policy (blocking v1 streams): a `sendmsg` the kernel
//! refuses queues locally and is flushed by repeated
//! [`FramedWriter::flush`](ldp_transport::writer::FramedWriter::flush) calls — the
//! server-side session thread is
//! always reading, so a full socket is transient, never a deadlock.

use ldp_core::wire::{ArgType, Value};
use ldp_protocol::{check_signature, Direction, Message, ValidationMode, REGISTRY};
use ldp_transport::fd::FdList;
use ldp_transport::writer::{FlushOutcome, SendOutcome};

use crate::connection::Connection;
use crate::error::{ClientError, Result};
use crate::proxy::Proxy;

impl Connection {
    /// Run a factory request (`compositor.create_surface`,
    /// `seat.get_pointer`, …): allocate the `new_id`, splice it into the
    /// argument list at the schema-declared position, send, and register
    /// the returned proxy at the interface the request's `new_id`
    /// argument declares (pinned at that interface's schema maximum).
    ///
    /// `rest_args` are the request's non-`new_id` arguments in
    /// declaration order. The request must declare exactly one `new_id`
    /// with an `of` qualifier — every v1 factory does.
    ///
    /// # Errors
    ///
    /// [`ClientError::UnknownProxy`] when the factory proxy is not
    /// tracked; [`ClientError::NoSuchRequest`] when the request or its
    /// `new_id` interface is absent from the schema; transport
    /// failures; ID exhaustion; a dead connection.
    pub fn create_object(
        &mut self,
        factory: &Proxy,
        request: &str,
        rest_args: Vec<Value>,
    ) -> Result<Proxy> {
        let mut no_fds = FdList::new();
        self.create_object_fd(factory, request, rest_args, &mut no_fds)
    }

    /// The descriptor-carrying factory flow (`shm.create_pool`): the
    /// `Value::Fd(index)` arguments address `fds`, whose ownership
    /// transfers with the send — the client-side counterpart of the
    /// server's FD-carrying event emission.
    ///
    /// # Errors
    ///
    /// As [`Self::create_object`], plus `fd_mismatch` when the argument
    /// FD indices and the batch disagree.
    pub fn create_object_fd(
        &mut self,
        factory: &Proxy,
        request: &str,
        rest_args: Vec<Value>,
        fds: &mut FdList,
    ) -> Result<Proxy> {
        self.ensure_alive()?;
        if self.proxies.get(factory.id()).is_none() {
            return Err(ClientError::UnknownProxy {
                target: factory.id().as_u32(),
            });
        }
        let opcode = crate::proxy::request_opcode(factory.interface(), request)?;
        let iface = REGISTRY
            .interface(factory.interface())
            .ok_or(ClientError::NoSuchRequest {
                interface: factory.interface().into(),
                request: request.into(),
            })?;
        let op = iface.requests.iter().find(|r| r.opcode == opcode).ok_or(
            ClientError::NoSuchRequest {
                interface: factory.interface().into(),
                request: request.into(),
            },
        )?;
        let pos = op.args.iter().position(|a| a.ty == ArgType::NewId).ok_or(
            ClientError::NoSuchRequest {
                interface: factory.interface().into(),
                request: request.into(),
            },
        )?;
        let of = op.args[pos].of.ok_or(ClientError::NoSuchRequest {
            interface: factory.interface().into(),
            request: request.into(),
        })?;
        let target_iface = crate::proxy::resolve_interface(factory.interface(), of).ok_or(
            ClientError::NoSuchRequest {
                interface: factory.interface().into(),
                request: request.into(),
            },
        )?;
        let new_id = self.allocator.allocate()?;
        let mut args = rest_args;
        args.insert(pos, Value::NewId(new_id));
        let proxy = Proxy::new(new_id, target_iface.name, target_iface.version_max);
        self.proxies.insert_client(proxy.clone())?;
        self.send_request_fd(factory, request, args, fds)?;
        Ok(proxy)
    }

    /// Send one request through the schema-checked path (no FDs).
    ///
    /// The message is signature-checked against the proxy's interface
    /// and pinned version *before* any byte hits the wire — the same
    /// stage-3 discipline the server applies, catching application bugs
    /// locally instead of as fatal `connection.error`s.
    ///
    /// # Errors
    ///
    /// [`ClientError::UnknownProxy`], [`ClientError::NoSuchRequest`],
    /// codec/limit errors, or a dead connection.
    pub fn send_request(&mut self, proxy: &Proxy, request: &str, args: Vec<Value>) -> Result<()> {
        let mut fds = FdList::new();
        self.send_request_fd(proxy, request, args, &mut fds)
    }

    /// Send one request with an FD batch (the FD indices in `args`
    /// address `fds`; the writer cross-checks the declared count).
    ///
    /// # Errors
    ///
    /// As [`Self::send_request`], plus `fd_mismatch` when the argument
    /// FD indices and the batch disagree.
    pub fn send_request_fd(
        &mut self,
        proxy: &Proxy,
        request: &str,
        args: Vec<Value>,
        fds: &mut FdList,
    ) -> Result<()> {
        self.ensure_alive()?;
        if self.proxies.get(proxy.id()).is_none() {
            return Err(ClientError::UnknownProxy {
                target: proxy.id().as_u32(),
            });
        }
        let opcode = crate::proxy::request_opcode(proxy.interface(), request)?;
        let mut msg = Message::new(proxy.id().as_u32(), opcode);
        for a in args {
            msg = msg.arg(a);
        }
        // HAS_REPLY mirrors the schema's declared reply correlation.
        let has_reply = REGISTRY.interface(proxy.interface()).is_some_and(|i| {
            i.requests
                .iter()
                .any(|r| r.opcode == opcode && r.reply.is_some())
        });
        if has_reply {
            msg = msg.reply();
        }
        // Client-side stage 3 before the wire.
        check_signature(
            &REGISTRY,
            &msg,
            proxy.interface(),
            Direction::Request,
            proxy.version(),
            ValidationMode::Tolerant,
        )?;
        let bytes = msg.encode(&self.limits)?;
        self.send_bytes(&bytes, fds)
    }

    /// Encode-validated bytes + FD batch onto the stream, flushing a
    /// congested queue (blocking stream: the server always drains).
    fn send_bytes(&mut self, bytes: &[u8], fds: &mut FdList) -> Result<()> {
        match self.writer.send_msg(&mut self.stream, bytes, fds) {
            Ok(SendOutcome::Sent) => Ok(()),
            Ok(SendOutcome::Congested { .. }) => self.flush(),
            Err(e) => Err(self.note_disconnect(&e)),
        }
    }

    /// Drain the writer's queue (escape hatch for callers driving long
    /// transfers themselves).
    ///
    /// # Errors
    ///
    /// Transport failures classify as disconnects.
    pub fn flush(&mut self) -> Result<()> {
        loop {
            match self.writer.flush(&mut self.stream) {
                Ok(FlushOutcome::Drained) => return Ok(()),
                Ok(FlushOutcome::Congested { .. }) => {}
                Err(e) => return Err(self.note_disconnect(&e)),
            }
        }
    }
}
