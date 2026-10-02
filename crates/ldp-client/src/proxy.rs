//! Proxy objects: the client's mirror of the server's object store.
//!
//! A [`Proxy`] is the client-side handle for one protocol object: its
//! wire ID, its interface, and the version the object is pinned at.
//! Every request the client sends targets a proxy, and every event the
//! server sends resolves against one — the client mirrors the server's
//! stage-3 order (object resolution *before* signature check), so a
//! server that targets events at an object this client never held, or
//! that is already torn down with confirmation, is rejected locally as
//! a protocol violation.
//!
//! Lifecycle states matter for the destroy race: after the client sends
//! `connection.destroy`, the object is still live server-side and
//! events for it may legitimately arrive until the `destroyed`
//! confirmation. [`ProxyMap`] therefore keeps [`State::Live`] entries
//! through the destroy window and only removes them when the
//! confirmation (or `connection.revoked`) arrives.

use ldp_core::ids::ObjectId;
use ldp_protocol::{Direction, REGISTRY};
use std::collections::HashMap;

use crate::error::{ClientError, Result};

/// Interface name of the pre-bound bootstrap object.
pub const CONNECTION_INTERFACE: &str = "ldp.core.connection";

/// A client-side handle to one server object.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Proxy {
    id: ObjectId,
    interface: &'static str,
    version: u32,
}

impl Proxy {
    /// Build a proxy from validated parts (the connection's send and
    /// bind paths; the ID allocator and schema supply the values).
    #[must_use]
    pub const fn new(id: ObjectId, interface: &'static str, version: u32) -> Proxy {
        Proxy {
            id,
            interface,
            version,
        }
    }

    /// A proxy for the pre-bound `ldp.core.connection` object (ID 1).
    ///
    /// # Panics
    ///
    /// Never in a coherent build: the connection interface is part of
    /// the compiled schema (the whole client depends on it).
    #[must_use]
    pub fn connection() -> Proxy {
        let iface = REGISTRY
            .interface(CONNECTION_INTERFACE)
            .expect("connection interface is part of the compiled schema");
        Proxy {
            id: ObjectId::CONNECTION,
            interface: iface.name,
            version: iface.version_max,
        }
    }

    /// The wire ID (client- or server-owned range).
    #[must_use]
    pub const fn id(&self) -> ObjectId {
        self.id
    }

    /// The fully qualified interface name.
    #[must_use]
    pub const fn interface(&self) -> &'static str {
        self.interface
    }

    /// The version this object was pinned at (bind version, or the
    /// schema maximum for server-created objects).
    #[must_use]
    pub const fn version(&self) -> u32 {
        self.version
    }
}

/// Lifecycle state of one tracked object.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    /// Bound and live server-side.
    Live,
    /// The client sent `connection.destroy`; the confirmation has not
    /// arrived yet. Events still resolve (the server-side object is
    /// alive until it processes the request).
    PendingDestroy,
}

/// The client-side object table: wire ID → proxy + lifecycle state.
#[derive(Debug)]
pub struct ProxyMap {
    objects: HashMap<u32, (Proxy, State)>,
}

impl ProxyMap {
    /// An empty map with the pre-bound connection object installed.
    #[must_use]
    pub fn new() -> ProxyMap {
        let mut map = ProxyMap {
            objects: HashMap::new(),
        };
        map.objects.insert(
            ObjectId::CONNECTION.as_u32(),
            (Proxy::connection(), State::Live),
        );
        map
    }

    /// Number of tracked objects (including the bootstrap object).
    #[must_use]
    pub fn len(&self) -> usize {
        self.objects.len()
    }

    /// Whether any object is tracked (always true in practice: the
    /// bootstrap object never leaves).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }

    /// Look up a proxy by wire ID.
    #[must_use]
    pub fn get(&self, id: ObjectId) -> Option<&Proxy> {
        self.objects.get(&id.as_u32()).map(|(p, _)| p)
    }

    /// Look up a proxy and its lifecycle state.
    #[must_use]
    pub fn get_with_state(&self, id: ObjectId) -> Option<(&Proxy, State)> {
        self.objects.get(&id.as_u32()).map(|(p, s)| (p, *s))
    }

    /// Insert a client-allocated proxy (from `bind` / `get_registry` /
    /// factory requests with a `new_id`). Overwriting a live entry is a
    /// caller bug (the allocator guarantees freshness) and is reported
    /// instead of silently replacing.
    ///
    /// # Errors
    ///
    /// [`ClientError::Ldp`] with [`ldp_core::error::ErrorCode::InvalidState`] when the ID
    /// is already tracked.
    pub fn insert_client(&mut self, proxy: Proxy) -> Result<()> {
        let raw = proxy.id.as_u32();
        if self.objects.contains_key(&raw) {
            return Err(ClientError::Ldp(ldp_core::error::LdpError::protocol(
                ldp_core::error::ErrorCode::InvalidState,
                Some(proxy.id),
                "proxy insert over a live object ID",
            )));
        }
        self.objects.insert(raw, (proxy, State::Live));
        Ok(())
    }

    /// Record a server-created object announced by an event's `new_id`
    /// argument (`data_offer`, and later server factories). The
    /// interface is resolved from the argument's `of` qualifier —
    /// short names resolve within the declaring interface's module,
    /// fully qualified names directly.
    ///
    /// A collision means the server announced an ID this client already
    /// holds — a protocol violation; the caller decides the reaction.
    ///
    /// # Errors
    ///
    /// [`ClientError::Ldp`] with [`ldp_core::error::ErrorCode::InvalidObject`] when the
    /// `of` reference does not resolve to a known interface, or
    /// [`ldp_core::error::ErrorCode::InvalidState`] on ID collision.
    pub fn insert_server(&mut self, id: ObjectId, of: &str, declaring: &str) -> Result<()> {
        let interface = resolve_interface(declaring, of).ok_or_else(|| {
            ClientError::Ldp(ldp_core::error::LdpError::protocol(
                ldp_core::error::ErrorCode::InvalidInterface,
                Some(id),
                format!("server announced new_id of unknown interface '{of}'"),
            ))
        })?;
        let proxy = Proxy {
            id,
            interface: interface.name,
            version: interface.version_max,
        };
        self.insert_client(proxy)
    }

    /// Mark a proxy as awaiting its `destroyed` confirmation.
    ///
    /// # Errors
    ///
    /// [`ClientError::UnknownProxy`] when the object is not tracked
    /// (double destroy, or a foreign proxy).
    pub fn mark_pending_destroy(&mut self, id: ObjectId) -> Result<()> {
        match self.objects.get_mut(&id.as_u32()) {
            Some((_, state)) => {
                *state = State::PendingDestroy;
                Ok(())
            }
            None => Err(ClientError::UnknownProxy {
                target: id.as_u32(),
            }),
        }
    }

    /// Remove a proxy (after `destroyed` or `revoked`). Returns the
    /// removed proxy for confirmation bookkeeping.
    #[must_use]
    pub fn remove(&mut self, id: ObjectId) -> Option<Proxy> {
        self.objects.remove(&id.as_u32()).map(|(p, _)| p)
    }

    /// Every tracked proxy with its state (diagnostics and tests).
    pub fn entries(&self) -> impl Iterator<Item = (&Proxy, State)> {
        self.objects.values().map(|(p, s)| (p, *s))
    }
}

impl Default for ProxyMap {
    fn default() -> Self {
        ProxyMap::new()
    }
}

/// Resolve an `of` qualifier against the compiled schema: dotted names
/// are global; short names resolve within `declaring`'s module.
pub(crate) fn resolve_interface(
    declaring: &str,
    of: &str,
) -> Option<&'static ldp_protocol::InterfaceSchema> {
    if of.contains('.') {
        REGISTRY.interface(of)
    } else {
        let module = REGISTRY.module_of_interface(declaring)?;
        module
            .interfaces
            .iter()
            .copied()
            .find(|i| i.name.ends_with(&format!(".{of}")))
    }
}

/// Look up a request's opcode by name on an interface (schema helper
/// shared by the connection's send path).
///
/// # Errors
///
/// [`ClientError::NoSuchRequest`] when the interface has no such
/// request.
pub fn request_opcode(interface: &str, request: &str) -> Result<u32> {
    let iface = REGISTRY
        .interface(interface)
        .ok_or(ClientError::NoSuchRequest {
            interface: interface.into(),
            request: request.into(),
        })?;
    iface
        .requests
        .iter()
        .find(|r| r.name == request)
        .map(|r| r.opcode)
        .ok_or_else(|| ClientError::NoSuchRequest {
            interface: interface.into(),
            request: request.into(),
        })
}

/// The direction of an operation lookup (re-exported convenience).
pub use Direction::Request;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_object_is_preinstalled() {
        let map = ProxyMap::new();
        let p = map.get(ObjectId::CONNECTION).unwrap();
        assert_eq!(p.interface(), CONNECTION_INTERFACE);
        assert_eq!(p.version(), 1);
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn insert_and_remove_round_trip() {
        let mut map = ProxyMap::new();
        let id = ObjectId::client(0x20).unwrap();
        map.insert_client(Proxy {
            id,
            interface: "ldp.core.output",
            version: 1,
        })
        .unwrap();
        assert_eq!(map.len(), 2);
        let removed = map.remove(id).unwrap();
        assert_eq!(removed.interface(), "ldp.core.output");
        assert!(map.get(id).is_none());
        // The bootstrap object survives everything.
        assert!(map.get(ObjectId::CONNECTION).is_some());
    }

    #[test]
    fn duplicate_insert_is_rejected() {
        let mut map = ProxyMap::new();
        let id = ObjectId::client(0x21).unwrap();
        map.insert_client(Proxy {
            id,
            interface: "ldp.core.output",
            version: 1,
        })
        .unwrap();
        assert!(map
            .insert_client(Proxy {
                id,
                interface: "ldp.core.output",
                version: 1,
            })
            .is_err());
    }

    #[test]
    fn pending_destroy_states_are_trackable() {
        let mut map = ProxyMap::new();
        let id = ObjectId::client(0x22).unwrap();
        map.insert_client(Proxy {
            id,
            interface: "ldp.core.surface",
            version: 1,
        })
        .unwrap();
        map.mark_pending_destroy(id).unwrap();
        assert_eq!(
            map.get_with_state(id).map(|(_, s)| s),
            Some(State::PendingDestroy)
        );
        // Still resolvable through the destroy window.
        assert!(map.get(id).is_some());
        assert!(map
            .mark_pending_destroy(ObjectId::client(0x99).unwrap())
            .is_err());
    }

    #[test]
    fn server_objects_resolve_short_and_dotted_of() {
        let mut map = ProxyMap::new();
        let id = ObjectId::server(0x8000_0005).unwrap();
        map.insert_server(id, "data_offer", "ldp.data.data_device")
            .unwrap();
        assert_eq!(map.get(id).unwrap().interface(), "ldp.data.data_offer");
        let id2 = ObjectId::server(0x8000_0006).unwrap();
        map.insert_server(id2, "ldp.core.surface", "ldp.data.data_device")
            .unwrap();
        assert_eq!(map.get(id2).unwrap().interface(), "ldp.core.surface");
        assert!(map
            .insert_server(
                ObjectId::server(0x8000_0007).unwrap(),
                "no_such_iface",
                "ldp.data.data_device"
            )
            .is_err());
    }

    #[test]
    fn request_opcode_lookup() {
        assert_eq!(request_opcode("ldp.core.connection", "hello").unwrap(), 1);
        assert_eq!(request_opcode("ldp.core.connection", "sync").unwrap(), 2);
        assert!(request_opcode("ldp.core.connection", "nope").is_err());
        assert!(request_opcode("ldp.not.real", "hello").is_err());
    }
}
