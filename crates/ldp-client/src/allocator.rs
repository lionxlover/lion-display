//! Client-side object-ID allocation.
//!
//! The wire contract (`docs/protocol.md` §3) splits the 32-bit ID space
//! at bit 31: client-chosen IDs have the bit clear, server-chosen IDs
//! have it set, and ID 1 is reserved for the pre-bound
//! `ldp.core.connection` object. [`IdAllocator`] hands out the client
//! half monotonically from 2 upward.
//!
//! The monotonic policy is deliberate. Free-list reuse is the classic
//! source of double-allocation bugs in ID-recycling protocols (the
//! generation discipline exists precisely because reuse is dangerous);
//! a single connection would need 2^31 - 2 object creations before
//! exhaustion, and per-connection object counts are bounded far below
//! that by `Limits::client_objects` anyway. Exhaustion is a hard error,
//! not a wrap.

use crate::error::{ClientError, Result};
use ldp_core::ids::{ObjectId, MAX_CLIENT_OBJECT_ID};

/// Monotonic allocator for client-chosen object IDs.
#[derive(Debug)]
pub struct IdAllocator {
    next: u32,
    /// Total handed out (observability; `next - 2` before any release).
    allocated: u64,
}

impl IdAllocator {
    /// A fresh allocator; the first allocation is ID 2.
    #[must_use]
    pub const fn new() -> IdAllocator {
        IdAllocator {
            next: 2,
            allocated: 0,
        }
    }

    /// The next ID a call to [`Self::allocate`] would return.
    #[must_use]
    pub const fn peek(&self) -> u32 {
        self.next
    }

    /// Total IDs allocated over this allocator's lifetime.
    #[must_use]
    pub const fn allocated(&self) -> u64 {
        self.allocated
    }

    /// Allocate the next client-owned ID.
    ///
    /// # Errors
    ///
    /// [`ClientError::IdExhausted`] once the client half of the ID space
    /// is spent — a connection this long-lived should reuse objects
    /// instead of allocating new ones.
    pub fn allocate(&mut self) -> Result<ObjectId> {
        let raw = self.next;
        if raw > MAX_CLIENT_OBJECT_ID {
            return Err(ClientError::IdExhausted);
        }
        // Overflow past MAX is impossible here: MAX is the last legal
        // value, so at most one increment lands on MAX + 1 and the next
        // call errors out.
        self.next = raw + 1;
        self.allocated += 1;
        // ObjectId::client re-checks the full invariant set (zero, the
        // reserved bootstrap ID, the server bit) — defense in depth
        // against future edits to this file.
        ObjectId::client(raw).ok_or(ClientError::IdExhausted)
    }
}

impl Default for IdAllocator {
    fn default() -> Self {
        IdAllocator::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_at_two_and_climbs() {
        let mut a = IdAllocator::new();
        assert_eq!(a.peek(), 2);
        assert_eq!(a.allocate().unwrap().as_u32(), 2);
        assert_eq!(a.allocate().unwrap().as_u32(), 3);
        assert_eq!(a.allocated(), 2);
        assert_eq!(a.peek(), 4);
    }

    #[test]
    fn never_emits_server_or_reserved_ids() {
        let mut a = IdAllocator::new();
        for _ in 0..1000 {
            let id = a.allocate().unwrap();
            assert!(id.is_client_owned());
            assert_ne!(id.as_u32(), 1);
        }
    }

    #[test]
    fn exhausts_exactly_at_the_boundary() {
        // Jump close to the ceiling, then walk across it.
        let mut a = IdAllocator::new();
        a.next = MAX_CLIENT_OBJECT_ID - 1;
        assert_eq!(a.allocate().unwrap().as_u32(), MAX_CLIENT_OBJECT_ID - 1);
        assert_eq!(a.allocate().unwrap().as_u32(), MAX_CLIENT_OBJECT_ID);
        assert!(matches!(a.allocate(), Err(ClientError::IdExhausted)));
        assert_eq!(a.allocated(), 2);
    }
}
