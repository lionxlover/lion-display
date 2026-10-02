//! The generational object store — one per client session.
//!
//! Server-side bookkeeping for every object ID of one connection
//! (`docs/protocol.md` §3):
//!
//! * the table is keyed by the **full wire ID** — client-allocated IDs
//!   (bit 31 clear) and server-allocated IDs (bit 31 set) occupy
//!   disjoint key ranges, so a client's slot 5 and the server's
//!   `0x8000_0005` never collide,
//! * every slot carries a [`Generation`]: the first occupant is
//!   generation 1, each *re*-allocation increments it. Wire references
//!   resolve to the current occupant (a reused ID means the new object);
//!   references to destroyed-but-unreused slots are [`Lookup::Stale`],
//!   never-missed slots [`Lookup::NotFound`],
//! * [`StoreHandle`] pairs (id, generation) for **server-side stored
//!   references** — the misdelivery protection of §3.1: a handle captured
//!   for generation *g* resolves to [`Lookup::Stale`] once the slot has
//!   been destroyed or rebound, instead of silently addressing whatever
//!   occupies the ID now.

use std::collections::HashMap;

use ldp_core::error::{ErrorCode, LdpError, Result};
use ldp_core::ids::{Generation, ObjectId, CONNECTION_OBJECT_ID, SERVER_OBJECT_FLAG};

/// What kind of object occupies a slot. Dispatch routes on this: the
/// session core implements the two bootstrap interfaces, everything
/// else belongs to a [`crate::dispatch::Dispatcher`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ObjectKind {
    /// The pre-bound `ldp.core.connection` object (wire ID 1).
    Connection,
    /// A `ldp.core.registry` object (from `get_registry` or `bind`).
    Registry,
    /// An object created by `registry.bind`.
    Global,
    /// An object created by a dispatcher (compositor factories, …).
    External,
}

/// One live object.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ObjectEntry {
    /// Fully qualified interface name (static schema string).
    pub interface: &'static str,
    /// The version the object was pinned at.
    pub version: u32,
    /// Routing class.
    pub kind: ObjectKind,
}

/// Result of resolving a wire object ID.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lookup<'a> {
    /// The slot was never used on this connection.
    NotFound,
    /// The slot was used and its object destroyed (generation included).
    Stale(Generation),
    /// A live object.
    Live(&'a ObjectEntry),
}

/// A server-side stored reference: (object, generation-at-capture).
///
/// Clone it, store it in other objects' state, and resolve it with
/// [`ObjectStore::resolve_handle`] — a mismatch is reported as
/// [`Lookup::Stale`], never as the current occupant.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct StoreHandle {
    id: ObjectId,
    generation: Generation,
}

/// The per-connection object table.
#[derive(Debug)]
pub struct ObjectStore {
    /// Keyed by full wire ID (bit 31 separates the allocation ranges).
    slots: HashMap<u32, Slot>,
    /// Next server-allocated ID (bit 31 set, monotonic, wrap-skipping).
    next_server: u32,
}

#[derive(Debug)]
struct Slot {
    generation: Generation,
    entry: Option<ObjectEntry>,
}

impl ObjectStore {
    /// An empty store; the connection bootstrap object still needs to be
    /// inserted via [`ObjectStore::insert_bootstrap`].
    #[must_use]
    pub fn new() -> ObjectStore {
        ObjectStore {
            slots: HashMap::new(),
            next_server: SERVER_OBJECT_FLAG | 1,
        }
    }

    /// Insert the pre-bound connection object at reserved wire ID 1.
    ///
    /// # Errors
    ///
    /// [`LdpError::Logic`] if called twice (a construction bug, not peer
    /// input).
    pub fn insert_bootstrap(&mut self, entry: ObjectEntry) -> Result<()> {
        if self
            .slots
            .insert(
                CONNECTION_OBJECT_ID,
                Slot {
                    generation: Generation::FIRST,
                    entry: Some(entry),
                },
            )
            .is_some()
        {
            return Err(LdpError::Logic {
                what: "connection bootstrap object inserted twice",
            });
        }
        Ok(())
    }

    /// Insert a client-allocated object (the `new_id` of a request).
    ///
    /// The ID must be in the client range and not currently live; a slot
    /// left empty by an earlier destroy is *reused*, advancing its
    /// generation — that is the whole recycling discipline.
    ///
    /// # Errors
    ///
    /// [`LdpError::Protocol`] with [`ErrorCode::InvalidState`] when the
    /// ID is live or is the reserved bootstrap ID,
    /// [`ErrorCode::InvalidObject`] for server-range or zero IDs.
    pub fn insert_client(&mut self, id: ObjectId, entry: ObjectEntry) -> Result<Generation> {
        if id.as_u32() == 0 || id.is_server_owned() {
            return Err(LdpError::protocol(
                ErrorCode::InvalidObject,
                Some(id),
                "new object ID must be a client-allocated ID",
            ));
        }
        if id.as_u32() == CONNECTION_OBJECT_ID {
            return Err(LdpError::protocol(
                ErrorCode::InvalidObject,
                Some(id),
                "object ID 1 is the reserved bootstrap object",
            ));
        }
        let generation = match self.slots.get_mut(&id.as_u32()) {
            Some(slot) => {
                if slot.entry.is_some() {
                    return Err(LdpError::protocol(
                        ErrorCode::InvalidState,
                        Some(id),
                        "object ID already bound on this connection",
                    ));
                }
                slot.generation.next()
            }
            None => Generation::FIRST,
        };
        self.slots.insert(
            id.as_u32(),
            Slot {
                generation,
                entry: Some(entry),
            },
        );
        Ok(generation)
    }

    /// Allocate and insert a server-owned object (server-range ID,
    /// generation 1). Later phases use this when factories announce
    /// objects to the client.
    ///
    /// # Errors
    ///
    /// [`LdpError::Limit`] if the server range is exhausted (2^31 - 1
    /// live objects — unreachable in practice, but the allocator must
    /// not loop forever on wrap).
    pub fn insert_server(&mut self, entry: ObjectEntry) -> Result<ObjectId> {
        for _ in 0..SERVER_OBJECT_FLAG {
            let candidate = self.next_server;
            // Advance first (wrap to 0x8000_0001, skipping 0).
            self.next_server = if self.next_server == u32::MAX {
                SERVER_OBJECT_FLAG | 1
            } else {
                self.next_server + 1
            };
            if candidate == 0 {
                continue;
            }
            let id = ObjectId::from_wire(candidate);
            let slot = Slot {
                generation: Generation::FIRST,
                entry: Some(entry),
            };
            if self.slots.insert(candidate, slot).is_none() {
                return Ok(id);
            }
        }
        Err(LdpError::Limit {
            kind: ldp_core::error::LimitKind::ClientObjects,
            value: u64::from(SERVER_OBJECT_FLAG),
        })
    }

    /// Insert a *specific* server-range ID — the routed-announcement
    /// path: the id was minted by the owning subsystem's world-wide
    /// counter (the data family's offer announcements) and already
    /// rides the event's `new_id` argument, so the receiving store
    /// must hold exactly that id, not a fresh allocation.
    ///
    /// # Errors
    ///
    /// [`LdpError::Protocol`] with [`ErrorCode::InvalidObject`] for a
    /// non-server-range or zero ID; [`ErrorCode::InvalidState`] when
    /// the slot is already live.
    pub fn insert_server_at(&mut self, id: ObjectId, entry: ObjectEntry) -> Result<()> {
        if id.as_u32() == 0 || !id.is_server_owned() {
            return Err(LdpError::protocol(
                ErrorCode::InvalidObject,
                Some(id),
                "announced object ID must be in the server range",
            ));
        }
        if matches!(
            self.slots.get(&id.as_u32()),
            Some(Slot { entry: Some(_), .. })
        ) {
            return Err(LdpError::protocol(
                ErrorCode::InvalidState,
                Some(id),
                "announced object ID is already live on this connection",
            ));
        }
        self.slots.insert(
            id.as_u32(),
            Slot {
                generation: Generation::FIRST,
                entry: Some(entry),
            },
        );
        Ok(())
    }

    /// Resolve a wire object ID to its current state.
    #[must_use]
    pub fn lookup(&self, id: ObjectId) -> Lookup<'_> {
        match self.slots.get(&id.as_u32()) {
            None => Lookup::NotFound,
            Some(slot) => match &slot.entry {
                None => Lookup::Stale(slot.generation),
                Some(entry) => Lookup::Live(entry),
            },
        }
    }

    /// Capture a stored reference to the current occupant of `id`.
    #[must_use]
    pub fn handle(&self, id: ObjectId) -> Option<StoreHandle> {
        match self.slots.get(&id.as_u32()) {
            Some(slot) if slot.entry.is_some() => Some(StoreHandle {
                id,
                generation: slot.generation,
            }),
            _ => None,
        }
    }

    /// Resolve a stored reference: generation must match, otherwise the
    /// reference outlived its target and is stale.
    #[must_use]
    pub fn resolve_handle(&self, handle: StoreHandle) -> Lookup<'_> {
        match self.slots.get(&handle.id.as_u32()) {
            None => Lookup::NotFound,
            Some(slot) => match slot
                .entry
                .as_ref()
                .filter(|_| slot.generation == handle.generation)
            {
                Some(entry) => Lookup::Live(entry),
                None => Lookup::Stale(slot.generation),
            },
        }
    }

    /// Destroy the object at `id`, leaving the slot reusable.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::InvalidObject`] for never-used IDs,
    /// [`ErrorCode::InvalidState`] for already-destroyed (empty) slots —
    /// the double-destroy case.
    pub fn remove(&mut self, id: ObjectId) -> Result<ObjectEntry> {
        match self.slots.get_mut(&id.as_u32()) {
            None => Err(LdpError::protocol(
                ErrorCode::InvalidObject,
                Some(id),
                "object ID was never bound on this connection",
            )),
            Some(slot) => match slot.entry.take() {
                None => Err(LdpError::protocol(
                    ErrorCode::InvalidState,
                    Some(id),
                    "object already destroyed",
                )),
                Some(entry) => Ok(entry),
            },
        }
    }

    /// Number of live objects (the `client_objects` limit's measure).
    #[must_use]
    pub fn live_count(&self) -> u64 {
        self.slots.values().filter(|s| s.entry.is_some()).count() as u64
    }

    /// Iterate live objects (used for reclamation reporting).
    pub fn live_objects(&self) -> impl Iterator<Item = (ObjectId, &ObjectEntry)> {
        self.slots.iter().filter_map(|(&raw, slot)| {
            slot.entry
                .as_ref()
                .map(|entry| (ObjectId::from_wire(raw), entry))
        })
    }
}

impl Default for ObjectStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(kind: ObjectKind) -> ObjectEntry {
        ObjectEntry {
            interface: "ldp.core.output",
            version: 1,
            kind,
        }
    }

    #[test]
    fn bootstrap_lives_at_reserved_id_one() {
        let mut store = ObjectStore::new();
        store
            .insert_bootstrap(entry(ObjectKind::Connection))
            .unwrap();
        assert!(matches!(
            store.lookup(ObjectId::CONNECTION),
            Lookup::Live(e) if e.kind == ObjectKind::Connection
        ));
        assert_eq!(store.live_count(), 1);
        assert!(store
            .insert_bootstrap(entry(ObjectKind::Connection))
            .is_err());
    }

    #[test]
    fn client_range_and_ownership_rules() {
        let mut store = ObjectStore::new();
        assert!(store
            .insert_client(ObjectId::from_wire(0), entry(ObjectKind::Global))
            .is_err());
        assert!(store
            .insert_client(
                ObjectId::server(0x8000_0005).unwrap(),
                entry(ObjectKind::Global)
            )
            .is_err());
        assert!(store
            .insert_client(ObjectId::CONNECTION, entry(ObjectKind::Global))
            .is_err());
        let g = store
            .insert_client(ObjectId::client(2).unwrap(), entry(ObjectKind::Global))
            .unwrap();
        assert_eq!(g, Generation::FIRST);
        assert_eq!(store.live_count(), 1);
    }

    #[test]
    fn reuse_advances_generation_and_stale_between() {
        let mut store = ObjectStore::new();
        let id = ObjectId::client(9).unwrap();
        let g1 = store.insert_client(id, entry(ObjectKind::Global)).unwrap();
        let h1 = store.handle(id).unwrap();
        assert_eq!(h1.generation, g1);

        // Destroy: the slot is stale for wire references…
        store.remove(id).unwrap();
        assert!(matches!(store.lookup(id), Lookup::Stale(_)));
        // …and for stored handles.
        assert!(matches!(store.resolve_handle(h1), Lookup::Stale(_)));
        // Double destroy is invalid_state.
        let err = store.remove(id).unwrap_err();
        assert_eq!(err.wire_code(), Some(ErrorCode::InvalidState));

        // Reuse: generation advances, the wire ID resolves to the new
        // object, but the old handle stays stale.
        let g2 = store
            .insert_client(id, entry(ObjectKind::External))
            .unwrap();
        assert_eq!(g2, Generation::FIRST.next());
        assert!(matches!(store.lookup(id), Lookup::Live(_)));
        assert!(matches!(store.resolve_handle(h1), Lookup::Stale(_)));
        let h2 = store.handle(id).unwrap();
        assert!(matches!(store.resolve_handle(h2), Lookup::Live(_)));
    }

    #[test]
    fn never_used_is_not_found() {
        let mut store = ObjectStore::new();
        assert_eq!(store.lookup(ObjectId::client(2).unwrap()), Lookup::NotFound);
        assert!(store.handle(ObjectId::client(2).unwrap()).is_none());
        let err = store.remove(ObjectId::client(2).unwrap()).unwrap_err();
        assert_eq!(err.wire_code(), Some(ErrorCode::InvalidObject));
    }

    #[test]
    fn client_and_server_slots_do_not_collide() {
        let mut store = ObjectStore::new();
        store
            .insert_client(ObjectId::client(5).unwrap(), entry(ObjectKind::Global))
            .unwrap();
        let server_id = store.insert_server(entry(ObjectKind::External)).unwrap();
        // The first server ID is 0x8000_0001; slot 5 and 0x8000_0005 can
        // coexist regardless.
        assert_eq!(server_id.as_u32(), 0x8000_0001);
        store
            .insert_client(ObjectId::client(0x10).unwrap(), entry(ObjectKind::Global))
            .unwrap();
        let second = store.insert_server(entry(ObjectKind::External)).unwrap();
        assert_eq!(second.as_u32(), 0x8000_0002);
        assert_eq!(store.live_count(), 4);
        // Removing the client object does not touch the server table.
        store.remove(ObjectId::client(5).unwrap()).unwrap();
        assert!(matches!(store.lookup(second), Lookup::Live(_)));
        assert_eq!(store.live_count(), 3);
    }

    #[test]
    fn live_objects_iteration_is_complete() {
        let mut store = ObjectStore::new();
        store
            .insert_bootstrap(entry(ObjectKind::Connection))
            .unwrap();
        store
            .insert_client(ObjectId::client(2).unwrap(), entry(ObjectKind::Global))
            .unwrap();
        let server = store.insert_server(entry(ObjectKind::External)).unwrap();
        let seen: Vec<ObjectId> = store.live_objects().map(|(id, _)| id).collect();
        assert_eq!(seen.len(), 3);
        assert!(seen.contains(&ObjectId::CONNECTION));
        assert!(seen.contains(&server));
        // After removing one, iteration reflects it.
        store.remove(server).unwrap();
        assert_eq!(store.live_objects().count(), 2);
    }
}
