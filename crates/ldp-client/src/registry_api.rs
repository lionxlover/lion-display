//! Typed views of the `ldp.core.registry` events: bind confirmation
//! and the global advertisement stream.
//!
//! Every registry event is a **control-class barrier** (see
//! [`crate::class`]): `bound` correlates with a `registry.bind` request,
//! `global`/`global_remove` are the advertised-global stream replayed on
//! registry materialization and maintained over the connection's
//! lifetime. The `revoked`-before-`global_remove` ordering the spec
//! promises is exactly the barrier discipline the queue enforces.

use ldp_core::error::{LdpError, Result};
use ldp_core::wire::Value;

use crate::queue::Event;

/// Argument extraction failure: the event did not carry the argument
/// shape its own schema promised. Only a server (or schema) bug can
/// produce this after stage-3 validation.
fn bad_shape(interface: &str, op: &str, arg: &str) -> LdpError {
    LdpError::protocol(
        ldp_core::error::ErrorCode::SignatureMismatch,
        None,
        format!("{interface}.{op}: argument '{arg}' failed its schema shape"),
    )
}

/// `registry.bound` — bind confirmation.
#[derive(Clone, Debug)]
pub struct Bound {
    /// The interface that was bound.
    pub interface: Box<str>,
    /// The version actually pinned (== requested on success).
    pub version: u32,
}

impl Bound {
    /// Decode from a `bound` event.
    ///
    /// # Errors
    ///
    /// [`LdpError::Logic`] on a shape mismatch (a server bug).
    pub fn from_event(e: &Event) -> Result<Bound> {
        let (Some(Value::String(ref interface)), Some(&Value::Uint32(version))) =
            (e.args.first(), e.args.get(1))
        else {
            return Err(bad_shape(e.interface, "bound", "interface/version"));
        };
        Ok(Bound {
            interface: interface.clone(),
            version,
        })
    }
}

/// `registry.global` — a global advertisement.
#[derive(Clone, Debug)]
pub struct Global {
    /// The advertised interface's fully qualified name.
    pub interface: Box<str>,
    /// The lowest version the server supports.
    pub version_min: u32,
    /// The highest version the server supports.
    pub version_max: u32,
}

impl Global {
    /// Decode from a `global` event.
    ///
    /// # Errors
    ///
    /// [`LdpError::Logic`] on a shape mismatch (a server bug).
    pub fn from_event(e: &Event) -> Result<Global> {
        let (
            Some(Value::String(ref interface)),
            Some(&Value::Uint32(version_min)),
            Some(&Value::Uint32(version_max)),
        ) = (e.args.first(), e.args.get(1), e.args.get(2))
        else {
            return Err(bad_shape(
                e.interface,
                "global",
                "interface/version_min/version_max",
            ));
        };
        Ok(Global {
            interface: interface.clone(),
            version_min,
            version_max,
        })
    }
}

/// `registry.global_remove` — an interface disappeared.
///
/// The event is a zero-argument *barrier* on the frozen v1 wire: the
/// objects of the removed interface were revoked — each carrying
/// `revoked_reason::interface_removed` — *before* this event fires,
/// so the identity of what left is already in the revocation stream
/// the client observed. This event's contract is the ordering: a
/// client that reacts to it may rely on every pre-removal object of
/// that interface being gone already. (The decoder here historically
/// demanded a `String` argument the spec never declared — the
/// schema's zero-arg shape and the frozen snapshot both disagree with
/// it, so the first conformant emission would have failed the decode;
/// fixed to follow the spec.)
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct GlobalRemove;

impl GlobalRemove {
    /// Decode from a `global_remove` event.
    ///
    /// # Errors
    ///
    /// [`LdpError::Logic`] on a shape mismatch (a server bug).
    pub fn from_event(e: &Event) -> Result<GlobalRemove> {
        if e.args.is_empty() {
            Ok(GlobalRemove)
        } else {
            Err(bad_shape(
                e.interface,
                "global_remove",
                "no arguments (the v1 wire carries none)",
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::class::class_of;
    use ldp_protocol::REGISTRY;

    /// Build a synthetic registry event from typed values.
    fn synth(event: &str, args: Vec<Value>) -> Event {
        let iface = REGISTRY
            .interface("ldp.core.registry")
            .expect("registry interface is compiled");
        let op = iface
            .events
            .iter()
            .find(|e| e.name == event)
            .unwrap_or_else(|| panic!("ldp.core.registry.{event} not in schema"));
        Event {
            seq: 1,
            target: ldp_core::ids::ObjectId::from_wire(2),
            interface: iface.name,
            version: 1,
            op,
            args,
            fds: ldp_transport::FdList::new(),
            class: class_of(iface.name, op.name),
            urgent: false,
        }
    }

    #[test]
    fn registry_events_decode() {
        let e = synth(
            "bound",
            vec![Value::String("ldp.core.output".into()), Value::Uint32(1)],
        );
        let b = Bound::from_event(&e).unwrap();
        assert_eq!(&*b.interface, "ldp.core.output");
        assert_eq!(b.version, 1);
        let e = synth(
            "global",
            vec![
                Value::String("ldp.core.output".into()),
                Value::Uint32(1),
                Value::Uint32(1),
            ],
        );
        let g = Global::from_event(&e).unwrap();
        assert_eq!(g.version_min, 1);
        assert_eq!(g.version_max, 1);
        let e = synth("global_remove", vec![]);
        assert_eq!(GlobalRemove::from_event(&e).unwrap(), GlobalRemove);
        // The historical decoder demanded a `String` argument the
        // spec never declared; the conformant event is empty, and a
        // stuffed arg is the shape error it must stay.
        let e = synth(
            "global_remove",
            vec![Value::String("ldp.core.output".into())],
        );
        assert!(GlobalRemove::from_event(&e).is_err());
    }
}
