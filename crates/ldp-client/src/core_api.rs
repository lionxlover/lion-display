//! Typed helpers for the two bootstrap interfaces and the event
//! handler trait.
//!
//! The generic event path (`queue::Event`) is schema-driven and covers
//! every interface; this module adds *checked, typed* views for the
//! `ldp.core.connection` and `ldp.core.registry` events every client
//! consumes — the `bind`/`sync`/`destroy` bookkeeping the connection
//! itself runs on, exposed to applications so protocol state and
//! application state never drift apart.

use ldp_core::bitset::Bitset128;
use ldp_core::error::{ErrorCode, LdpError, Result};
use ldp_core::wire::Value;
use ldp_protocol::OpSchema;

use crate::queue::Event;

/// Argument extraction failure: the event did not carry the argument
/// shape its own schema promised. Only a server (or schema) bug can
/// produce this after stage-3 validation.
fn bad_shape(interface: &str, op: &str, arg: &str) -> LdpError {
    LdpError::protocol(
        ErrorCode::SignatureMismatch,
        None,
        format!("{interface}.{op}: argument '{arg}' failed its schema shape"),
    )
}

/// `connection.welcome` — the handshake reply.
#[derive(Clone, Debug)]
pub struct Welcome {
    /// The server's protocol release number.
    pub protocol_release: u32,
    /// The server's advertised capability bits (`server_caps`).
    pub caps: Bitset128,
    /// This connection's audit identity.
    pub client_id: u32,
    /// Verified application identity (empty until a broker exists).
    pub app_id: Box<str>,
    /// The sandbox verdict for this client.
    pub sandbox: u32,
}

impl Welcome {
    /// Decode from a `welcome` event (stage-3 guarantees the arg count;
    /// this re-checks the types so the struct is honest).
    ///
    /// # Errors
    ///
    /// [`LdpError::Logic`] on a shape mismatch (a server bug).
    pub fn from_event(e: &Event) -> Result<Welcome> {
        let (
            Value::Uint32(protocol_release),
            Value::Bitset(caps),
            Value::Uint32(client_id),
            Value::String(app_id),
            Value::Enum(sandbox),
        ) = (
            e.args.first().cloned().unwrap_or(Value::Uint32(0)),
            e.args
                .get(1)
                .cloned()
                .unwrap_or(Value::Bitset(Bitset128::EMPTY)),
            e.args.get(2).cloned().unwrap_or(Value::Uint32(0)),
            e.args
                .get(3)
                .cloned()
                .unwrap_or(Value::String(String::new().into())),
            e.args.get(4).cloned().unwrap_or(Value::Enum(0)),
        )
        else {
            return Err(bad_shape(e.interface, "welcome", "args"));
        };
        Ok(Welcome {
            protocol_release,
            caps,
            client_id,
            app_id,
            sandbox,
        })
    }
}

/// `connection.sync_done` — round-trip barrier completion.
#[derive(Clone, Copy, Debug)]
pub struct SyncDone {
    /// The cookie the matching `sync` carried.
    pub cookie: u32,
}

impl SyncDone {
    /// Decode from a `sync_done` event.
    ///
    /// # Errors
    ///
    /// [`LdpError::Logic`] on a shape mismatch (a server bug).
    pub fn from_event(e: &Event) -> Result<SyncDone> {
        let Some(&Value::Uint32(cookie)) = e.args.first() else {
            return Err(bad_shape(e.interface, "sync_done", "cookie"));
        };
        Ok(SyncDone { cookie })
    }
}

/// `connection.pong` — keepalive reply.
#[derive(Clone, Copy, Debug)]
pub struct Pong {
    /// The cookie the matching `ping` carried.
    pub cookie: u32,
}

impl Pong {
    /// Decode from a `pong` event.
    ///
    /// # Errors
    ///
    /// [`LdpError::Logic`] on a shape mismatch (a server bug).
    pub fn from_event(e: &Event) -> Result<Pong> {
        let Some(&Value::Uint32(cookie)) = e.args.first() else {
            return Err(bad_shape(e.interface, "pong", "cookie"));
        };
        Ok(Pong { cookie })
    }
}

/// `connection.destroyed` — lifecycle confirmation.
#[derive(Clone, Copy, Debug)]
pub struct Destroyed {
    /// The raw object ID that was destroyed.
    pub object_id: u32,
    /// The cookie the matching `destroy` carried.
    pub cookie: u32,
}

impl Destroyed {
    /// Decode from a `destroyed` event.
    ///
    /// # Errors
    ///
    /// [`LdpError::Logic`] on a shape mismatch (a server bug).
    pub fn from_event(e: &Event) -> Result<Destroyed> {
        let (Some(&Value::Uint32(object_id)), Some(&Value::Uint32(cookie))) =
            (e.args.first(), e.args.get(1))
        else {
            return Err(bad_shape(e.interface, "destroyed", "object_id/cookie"));
        };
        Ok(Destroyed { object_id, cookie })
    }
}

/// `connection.revoked` — server-initiated teardown.
#[derive(Clone, Copy, Debug)]
pub struct Revoked {
    /// The raw object ID that was revoked.
    pub object_id: u32,
    /// Why (`revoked_reason`).
    pub reason: u32,
}

impl Revoked {
    /// Decode from a `revoked` event.
    ///
    /// # Errors
    ///
    /// [`LdpError::Logic`] on a shape mismatch (a server bug).
    pub fn from_event(e: &Event) -> Result<Revoked> {
        let (Some(&Value::Uint32(object_id)), Some(&Value::Enum(reason))) =
            (e.args.first(), e.args.get(1))
        else {
            return Err(bad_shape(e.interface, "revoked", "object_id/reason"));
        };
        Ok(Revoked { object_id, reason })
    }
}

/// `connection.error` — the fatal protocol error.
#[derive(Clone, Debug)]
pub struct FatalError {
    /// The wire taxonomy code.
    pub code: ErrorCode,
    /// Offending object ID (0 when none).
    pub object_id: u32,
    /// Human-readable diagnostic (not for display to end users).
    pub message: Box<str>,
}

impl FatalError {
    /// Decode from an `error` event.
    ///
    /// # Errors
    ///
    /// [`LdpError::Logic`] on a shape mismatch (a server bug).
    pub fn from_event(e: &Event) -> Result<FatalError> {
        let (
            Some(&Value::Enum(code)),
            Some(&Value::Uint32(object_id)),
            Some(Value::String(ref message)),
        ) = (e.args.first(), e.args.get(1), e.args.get(2))
        else {
            return Err(bad_shape(e.interface, "error", "code/object_id/message"));
        };
        Ok(FatalError {
            code: ErrorCode::from_wire(code).unwrap_or(ErrorCode::ServerError),
            object_id,
            message: message.clone(),
        })
    }
}

/// Application-supplied event sink.
///
/// One handler instance serves one connection; it is called from
/// dispatch, in dispatch order, and must not block: the connection
/// cannot read more events while a handler runs (single-threaded by
/// design, like the whole v1 client). Requests in reaction to an event
/// should be *queued* by the handler and sent after
/// [`Connection::dispatch_pending`](crate::Connection::dispatch_pending)
/// returns — the handler deliberately receives no connection handle,
/// which makes re-entrant sends a compile error instead of a deadlock.
pub trait EventHandler {
    /// One event, post-validation, post-classification, in dispatch
    /// order. FDs left untaken when the call returns are closed.
    ///
    /// # Errors
    ///
    /// Returning `Err` aborts dispatch and propagates to the caller of
    /// the dispatch method — the standard way to unwind out of a
    /// callback on an unrecoverable application condition.
    fn on_event(&mut self, event: &Event) -> Result<()>;

    /// The connection ended. Called once, after the last event has been
    /// dispatched (when the end was observable as an orderly EOF);
    /// never called for local drops. The default is a no-op.
    fn on_disconnect(&mut self, reason: &crate::error::DisconnectKind) {
        let _ = reason;
    }
}

/// A handler that observes nothing (tests, probes, defaults).
#[derive(Clone, Copy, Debug, Default)]
pub struct NoopHandler;

impl EventHandler for NoopHandler {
    fn on_event(&mut self, _event: &Event) -> Result<()> {
        Ok(())
    }
}

/// Test helper: the signature of the operation an event carried.
#[must_use]
pub fn op_of(event: &Event) -> &'static OpSchema {
    event.op
}

#[cfg(test)]
mod tests {
    use super::*;
    use ldp_protocol::REGISTRY;

    /// Build a synthetic event from typed values for the decode tests:
    /// arg order comes from the schema so shapes match the wire.
    fn synth(interface: &str, event: &str, args: Vec<Value>) -> Event {
        let iface = REGISTRY
            .interface(interface)
            .unwrap_or_else(|| panic!("{interface} not in schema"));
        let op = iface
            .events
            .iter()
            .find(|e| e.name == event)
            .unwrap_or_else(|| panic!("{interface}.{event} not in schema"));
        Event {
            seq: 1,
            target: ldp_core::ids::ObjectId::CONNECTION,
            interface: iface.name,
            version: 1,
            op,
            args,
            fds: ldp_transport::FdList::new(),
            class: crate::class::class_of(iface.name, op.name),
            urgent: false,
        }
    }

    #[test]
    fn welcome_decodes() {
        let e = synth(
            "ldp.core.connection",
            "welcome",
            vec![
                Value::Uint32(1),
                Value::Bitset(Bitset128::single(0)),
                Value::Uint32(42),
                Value::String("app".into()),
                Value::Enum(2),
            ],
        );
        let w = Welcome::from_event(&e).unwrap();
        assert_eq!(w.protocol_release, 1);
        assert!(w.caps.test(0));
        assert_eq!(w.client_id, 42);
        assert_eq!(&*w.app_id, "app");
        assert_eq!(w.sandbox, 2);
    }

    #[test]
    fn cookie_events_decode() {
        let e = synth("ldp.core.connection", "sync_done", vec![Value::Uint32(7)]);
        assert_eq!(SyncDone::from_event(&e).unwrap().cookie, 7);
        let e = synth("ldp.core.connection", "pong", vec![Value::Uint32(9)]);
        assert_eq!(Pong::from_event(&e).unwrap().cookie, 9);
    }

    #[test]
    fn lifecycle_events_decode() {
        let e = synth(
            "ldp.core.connection",
            "destroyed",
            vec![Value::Uint32(0x20), Value::Uint32(3)],
        );
        let d = Destroyed::from_event(&e).unwrap();
        assert_eq!(d.object_id, 0x20);
        assert_eq!(d.cookie, 3);
        let e = synth(
            "ldp.core.connection",
            "revoked",
            vec![Value::Uint32(0x8000_0005), Value::Enum(1)],
        );
        let r = Revoked::from_event(&e).unwrap();
        assert_eq!(r.object_id, 0x8000_0005);
        assert_eq!(r.reason, 1);
    }

    #[test]
    fn fatal_error_decodes() {
        let e = synth(
            "ldp.core.connection",
            "error",
            vec![
                Value::Enum(11),
                Value::Uint32(4),
                Value::String("nope".into()),
            ],
        );
        let f = FatalError::from_event(&e).unwrap();
        assert_eq!(f.code, ErrorCode::InvalidState);
        assert_eq!(f.object_id, 4);
        assert_eq!(&*f.message, "nope");
    }

    #[test]
    fn shape_mismatch_is_a_logic_error() {
        let e = synth(
            "ldp.core.connection",
            "sync_done",
            vec![Value::String("x".into())],
        );
        assert!(SyncDone::from_event(&e).is_err());
    }
}
