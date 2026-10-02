//! Object and client identity.
//!
//! The wire object model (`docs/protocol.md` §3): a 32-bit ID per
//! connection, bit 31 marking server-allocated IDs, and a per-slot
//! generation discipline that makes ID reuse detectable. [`ObjectId`] is
//! the wire token; the (slot, generation) pair used by the server lives in
//! [`SlotId`]/[`Generation`].

use core::fmt;

/// Marker bit: IDs with this bit set are allocated by the server.
pub const SERVER_OBJECT_FLAG: u32 = 1 << 31;

/// The bootstrap object pre-bound on every connection (`ldp.core.connection`).
/// Lives in the client range but is *reserved*: clients may never allocate it.
pub const CONNECTION_OBJECT_ID: u32 = 1;

/// Maximum legal client-allocated object ID (bit 31 clear).
pub const MAX_CLIENT_OBJECT_ID: u32 = u32::MAX ^ SERVER_OBJECT_FLAG;

/// A wire object identifier for one connection.
///
/// Invariants enforced by the constructors:
///
/// * zero is never a valid [`ObjectId`] (it encodes "no object" in
///   nullable references),
/// * ID 1 in the client range is *reserved* for the pre-bound
///   `connection` bootstrap object — clients must never allocate it,
/// * IDs with bit 31 set are *server-allocated* — a client must never
///   choose them,
/// * IDs with bit 31 clear are *client-allocated* — the server must never
///   announce them.
///
/// ```
/// use ldp_core::ObjectId;
/// let id = ObjectId::client(0x1234).unwrap();
/// assert!(id.is_client_owned());
/// assert_eq!(id.as_u32(), 0x1234);
/// assert!(ObjectId::client(0).is_none(), "zero is not an object");
/// assert!(ObjectId::client(ObjectId::SERVER_FLAG).is_none(), "server bit must not be set");
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(transparent)]
pub struct ObjectId {
    raw: u32,
}

impl ObjectId {
    /// The bit that marks server-owned IDs (re-exported as a method-free
    /// constant on the type for discoverability).
    pub const SERVER_FLAG: u32 = SERVER_OBJECT_FLAG;

    /// The connection bootstrap object (pre-bound reserved client ID 1).
    pub const CONNECTION: ObjectId = ObjectId {
        raw: CONNECTION_OBJECT_ID,
    };

    /// Validate a client-chosen ID: non-zero, not the reserved bootstrap
    /// ID 1, bit 31 clear.
    pub const fn client(raw: u32) -> Option<ObjectId> {
        if raw == 0 || raw == CONNECTION_OBJECT_ID || (raw & SERVER_OBJECT_FLAG) != 0 {
            None
        } else {
            Some(ObjectId { raw })
        }
    }

    /// Validate a server-chosen ID: non-zero, bit 31 set.
    pub const fn server(raw: u32) -> Option<ObjectId> {
        if raw == 0 || (raw & SERVER_OBJECT_FLAG) == 0 {
            None
        } else {
            Some(ObjectId { raw })
        }
    }

    /// Reconstruct from a wire value **without validation** — for decoders
    /// that report invalid IDs as protocol errors themselves.
    pub const fn from_wire(raw: u32) -> ObjectId {
        ObjectId { raw }
    }

    /// Raw wire representation.
    pub const fn as_u32(self) -> u32 {
        self.raw
    }

    /// Whether the server allocated this ID.
    pub const fn is_server_owned(self) -> bool {
        (self.raw & SERVER_OBJECT_FLAG) != 0
    }

    /// Whether the client allocated this ID.
    pub const fn is_client_owned(self) -> bool {
        (self.raw & SERVER_OBJECT_FLAG) == 0
    }

    /// Slot index: the ID without the ownership bit. Server-side object
    /// tables key on this plus a [`Generation`].
    pub const fn slot(self) -> SlotId {
        SlotId::from_index(self.raw & !SERVER_OBJECT_FLAG)
    }
}

impl fmt::Debug for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ObjectId({:#010x}){}",
            self.raw,
            if self.is_server_owned() {
                " [server]"
            } else {
                ""
            }
        )
    }
}

/// Index into a per-connection object table (the ID stripped of the
/// ownership bit).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
#[repr(transparent)]
pub struct SlotId {
    index: u32,
}

impl SlotId {
    /// The connection object's slot.
    pub const CONNECTION: SlotId = SlotId {
        index: CONNECTION_OBJECT_ID,
    };

    /// Construct from a raw index.
    pub const fn from_index(index: u32) -> SlotId {
        SlotId { index }
    }

    /// Raw index value.
    pub const fn index(self) -> u32 {
        self.index
    }
}

/// Generation of an object occupying a [`SlotId`].
///
/// Server-side discipline (`docs/protocol.md` §3.1): a slot's generation
/// increments each time the object in it is destroyed and the slot becomes
/// free again. A message referencing `(slot, old_generation)` after reuse
/// is rejected as [`crate::error::ErrorCode::StaleObject`] instead of being
/// misdelivered.
/// The generation on the wire is implicit — the server tracks it — so this
/// type exists to make server bookkeeping type-safe.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Default)]
#[repr(transparent)]
pub struct Generation {
    gen: u32,
}

impl Generation {
    /// First generation of any slot.
    pub const FIRST: Generation = Generation { gen: 1 };

    /// Construct from a raw counter (0 is the reserved "never used" value).
    pub const fn from_raw(gen: u32) -> Generation {
        Generation { gen }
    }

    /// Raw counter value; 0 means the slot was never used.
    pub const fn as_u32(self) -> u32 {
        self.gen
    }

    /// Whether this slot has ever held an object.
    pub const fn is_initialized(self) -> bool {
        self.gen != 0
    }

    /// Next generation, wrapping past [`u32::MAX`] back to
    /// [`Generation::FIRST`]. Wrapping is deliberate: with 2^32 destructions
    /// between two uses of a reference, the attacker has earned the
    /// collision; the server also refuses to reuse a slot until at least
    /// one *other* generation has passed elsewhere (documented policy).
    #[must_use]
    pub const fn next(self) -> Generation {
        Generation {
            gen: if self.gen == u32::MAX {
                1
            } else {
                self.gen + 1
            },
        }
    }
}

/// Identity of one connected client, assigned by the server at accept time.
///
/// Used in audit records (the `welcome` event carries it) and server-side
/// logging. Not related to Unix credentials — see `ldp-transport` for
/// pid/uid.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
#[repr(transparent)]
pub struct ClientId {
    id: u32,
}

impl ClientId {
    /// The reserved zero identity — "not a client": the server's own
    /// origin for audit records and (Phase 47) the compositor's
    /// system wakes. Never a session's id — [`ClientId::new`] refuses
    /// it — so a world pump driven under this identity routes every
    /// emission to its owning client's outbox exactly as a
    /// client-driven wake would (the system wake never swallows
    /// events).
    pub const SERVER: ClientId = ClientId { id: 0 };

    /// Construct; zero is reserved for "not a client" (e.g. server-originated
    /// audit records).
    pub const fn new(id: u32) -> Option<ClientId> {
        if id == 0 {
            None
        } else {
            Some(ClientId { id })
        }
    }

    /// Raw value.
    pub const fn as_u32(self) -> u32 {
        self.id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_ids_reject_zero_and_server_bit() {
        assert!(ObjectId::client(0).is_none());
        assert!(
            ObjectId::client(1).is_none(),
            "ID 1 is the reserved bootstrap object"
        );
        assert!(ObjectId::client(1 << 31).is_none());
        assert!(ObjectId::client(u32::MAX).is_none());
        assert!(ObjectId::client(2).is_some());
        assert!(ObjectId::client(MAX_CLIENT_OBJECT_ID).is_some());
    }

    #[test]
    fn server_ids_require_bit() {
        assert!(ObjectId::server(1).is_none());
        assert!(ObjectId::server(0).is_none());
        assert!(ObjectId::server(1 << 31).is_some());
        assert!(ObjectId::server(u32::MAX).is_some());
    }

    #[test]
    fn connection_object_is_pre_bound() {
        let c = ObjectId::CONNECTION;
        assert!(
            c.is_client_owned(),
            "bootstrap lives in the reserved client range"
        );
        assert_eq!(c.slot(), SlotId::CONNECTION);
        assert_eq!(c.as_u32(), 1);
    }

    #[test]
    fn slot_strips_flag() {
        let id = ObjectId::server(0x8000_1234).unwrap();
        assert_eq!(id.slot().index(), 0x1234);
        let id = ObjectId::client(0x0000_1234).unwrap();
        assert_eq!(id.slot().index(), 0x1234);
    }

    #[test]
    fn generation_cycles_wrap_to_first() {
        let g = Generation::from_raw(u32::MAX);
        assert_eq!(g.next(), Generation::FIRST);
        assert_eq!(Generation::FIRST.next().as_u32(), 2);
        assert!(!Generation::from_raw(0).is_initialized());
    }

    #[test]
    fn client_id_rejects_zero() {
        assert!(ClientId::new(0).is_none());
        assert_eq!(ClientId::new(7).unwrap().as_u32(), 7);
    }

    #[test]
    fn debug_format_is_stable() {
        let id = ObjectId::client(0x20).unwrap();
        assert_eq!(format!("{id:?}"), "ObjectId(0x00000020)");
        let id = ObjectId::server(0x8000_0020).unwrap();
        assert_eq!(format!("{id:?}"), "ObjectId(0x80000020) [server]");
    }
}
