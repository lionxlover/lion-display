//! Shared testbench for the ldp-clipboard conformance suites.
//!
//! Determinism doctrine (no `rand` dependency): the fuzz drivers use
//! the same xorshift64* the shell suite adopted (every output bit is
//! well-distributed; an LCG's low bits cycle — the Phase 12 lesson).
//! Every helper is pure so a failing case reproduces from its seed
//! alone.

#![allow(dead_code)] // shared across suites; each uses a subset

use ldp_clipboard::dnd::ClientKey;
use ldp_clipboard::event::DataEvent;
use ldp_clipboard::manager::{ClipboardManager, SeatKey};
use ldp_clipboard::{SourceKey, SurfaceKey};
use ldp_core::ids::ObjectId;
use ldp_core::limits::Limits;
use ldp_protocol::{check_signature, decode, Direction, Message, ValidationMode, REGISTRY};

/// A deterministic xorshift64* driver (the house fuzz RNG).
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    /// New driver from a seed (zero is promoted to one — zero is
    /// xorshift's fixed point).
    #[must_use]
    pub const fn seeded(seed: u64) -> Rng {
        if seed == 0 {
            Rng(1)
        } else {
            Rng(seed)
        }
    }

    /// Next raw output.
    #[must_use]
    pub fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// A bounded random value.
    #[must_use]
    pub fn below(&mut self, bound: u64) -> u64 {
        if bound == 0 {
            0
        } else {
            self.next_u64() % bound
        }
    }

    /// A random boolean.
    #[must_use]
    pub fn flip(&mut self) -> bool {
        self.next_u64() & 1 == 1
    }
}

/// Validate an emitted data event end-to-end: encode → decode →
/// strict signature check against the compiled schema, opcode
/// agreement, and FD-count agreement (send events declare one FD).
///
/// # Panics
///
/// On any mismatch — this is the conformance oracle.
pub fn assert_wire_valid(e: &DataEvent, object: ObjectId) -> Message {
    let msg = e.to_message(object);
    let limits = Limits::default();
    let bytes = msg.encode(&limits).expect("encode");
    assert_eq!(
        msg.required_fd_count(),
        e.fd_count(),
        "fd count disagrees with the typed vocabulary"
    );
    let back = decode(&bytes, e.fd_count(), &limits, ValidationMode::Strict).expect("decode");
    assert_eq!(back, msg, "data event did not round-trip");
    let (_, matched) = check_signature(
        &REGISTRY,
        &back,
        e.interface(),
        Direction::Event,
        1,
        ValidationMode::Strict,
    )
    .unwrap_or_else(|err| {
        panic!(
            "{}::{} failed strict signature: {err}",
            e.interface(),
            e.name()
        )
    });
    assert_eq!(matched.name, e.name(), "opcode resolved to the wrong event");
    msg
}

/// A managed test world: two seats, N clients, deterministic object
/// id allocation (client-tagged high bits so ids never collide
/// across clients — the per-connection namespace doctrine).
pub struct World {
    /// The manager under test.
    pub manager: ClipboardManager,
    next_obj: u32,
}

impl World {
    /// A world with `clients` devices on each of two seats.
    #[must_use]
    pub fn new(clients: u32) -> World {
        let mut w = World {
            manager: ClipboardManager::new(Limits::default()),
            next_obj: 0x4000,
        };
        for c in 0..clients {
            for s in 0..2u64 {
                let id = w.alloc(c);
                w.manager.create_device(
                    ClientKey(u64::from(c)),
                    SeatKey(s),
                    ObjectId::from_wire(id),
                );
            }
        }
        w
    }

    /// Allocate one object id "for client c" (tagged namespace).
    #[must_use]
    pub fn alloc(&mut self, c: u32) -> u32 {
        self.next_obj = self.next_obj.wrapping_add(1);
        (c << 24) | (self.next_obj & 0x00FF_FFFF)
    }

    /// Run `f` with the manager and a fresh object supplier (the
    /// counter lives in a local so the closure never borrows the
    /// world — the double-borrow trap).
    pub fn with_supplier<R>(
        &mut self,
        f: impl FnOnce(&mut ClipboardManager, &mut ldp_clipboard::manager::ObjectSupplier<'_>) -> R,
    ) -> R {
        let mut next = self.next_obj;
        // Offers are server-created objects: the wire's server-owned
        // range (bit 31) — the new_id direction rule of stage 3.
        let mut sup = move |client: ClientKey| {
            next = next.wrapping_add(1);
            ObjectId::from_wire(
                ldp_core::ids::SERVER_OBJECT_FLAG
                    | ((client.0 as u32) << 24)
                    | (next & 0x00FF_FFFF),
            )
        };
        let r = f(&mut self.manager, &mut sup);
        self.next_obj = next;
        r
    }
}

/// Standard identities used by every suite.
pub mod ids {
    use super::*;

    /// Client A.
    pub const A: ClientKey = ClientKey(0);
    /// Client B.
    pub const B: ClientKey = ClientKey(1);
    /// Client C (the unprivileged one).
    pub const C: ClientKey = ClientKey(2);
    /// The default seat.
    pub const SEAT: SeatKey = SeatKey(0);
    /// A drag origin surface.
    pub const ORIGIN: SurfaceKey = SurfaceKey(100);
    /// A receiver surface.
    pub const TARGET: SurfaceKey = SurfaceKey(101);
    /// A drag icon surface.
    pub const ICON: SurfaceKey = SurfaceKey(102);

    /// Create a source under client `c` with `mimes` offered.
    pub fn source(w: &mut World, c: ClientKey, mimes: &[&str]) -> SourceKey {
        let obj = ObjectId::from_wire(w.alloc(c.0 as u32));
        let key = w.manager.create_source(c, obj);
        for m in mimes {
            w.manager.source_offer(key, m).unwrap();
        }
        key
    }
}

/// A deterministic byte pattern source (the fuzz corpus generator):
/// `n` bytes where byte `i` = `(seed + i) & 0xFF` when `mode` is
/// Ramp, or a repeated 16-byte pattern otherwise.
#[derive(Clone, Copy, Debug)]
pub enum Pattern {
    /// A rolling byte ramp with a per-stream seed offset.
    Ramp(u8),
    /// A repeated 16-byte tag.
    Tag(u128),
}

impl Pattern {
    /// The byte at index `i`.
    #[must_use]
    pub fn byte(self, i: u64) -> u8 {
        match self {
            Pattern::Ramp(seed) => seed.wrapping_add(i as u8),
            Pattern::Tag(t) => ((t >> ((i % 16) * 8)) & 0xFF) as u8,
        }
    }

    /// A full buffer of `n` bytes.
    #[must_use]
    pub fn buffer(self, n: u64) -> Vec<u8> {
        (0..n).map(|i| self.byte(i)).collect()
    }

    /// A random pattern from the driver.
    #[must_use]
    pub fn random(rng: &mut Rng) -> Pattern {
        if rng.flip() {
            Pattern::Ramp((rng.next_u64() & 0xFF) as u8)
        } else {
            Pattern::Tag((rng.next_u64() as u128) | ((rng.next_u64() as u128) << 64))
        }
    }
}

/// A streaming checksum (xorshift64* over the byte stream + length):
/// cheap, order-sensitive, and strong enough to catch any
/// misordering or truncation the pump could produce.
#[derive(Clone, Copy, Debug)]
pub struct Checksum {
    state: u64,
    len: u64,
}

impl Checksum {
    /// An empty checksum.
    #[must_use]
    pub const fn new() -> Checksum {
        Checksum { state: 1, len: 0 }
    }

    /// Absorb one buffer.
    pub fn absorb(&mut self, buf: &[u8]) {
        for &b in buf {
            self.state ^= u64::from(b);
            self.state ^= self.state << 13;
            self.state ^= self.state >> 7;
            self.state ^= self.state << 17;
            self.state = self.state.wrapping_mul(0x2545_F491_4F6C_DD1D);
        }
        self.len += buf.len() as u64;
    }

    /// The digest (state and length; distinct lengths never collide).
    #[must_use]
    pub const fn digest(self) -> (u64, u64) {
        (self.state, self.len)
    }
}

/// The well-known MIME corpus the fuzz suites draw from.
pub const MIME_CORPUS: &[&str] = &[
    "text/plain",
    "text/plain;charset=utf-8",
    "text/plain;charset=us-ascii",
    "text/html",
    "image/png",
    "image/jpeg",
    "application/json",
    "application/octet-stream",
    "text/uri-list",
    "application/x-lionos-task;task=42",
    "image/svg+xml",
    "text/plain;charset=iso-8859-1",
];
