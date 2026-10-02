//! # ldp-remote — LDP over the network
//!
//! The post-v0.1.0 remote-transport direction (`docs/roadmap.md`,
//! "Milestone releases": v0.2.x). LDP's native transport is one
//! `AF_UNIX` `SOCK_STREAM` connection with `SCM_RIGHTS` descriptor
//! passing; this crate carries such sessions across hosts:
//!
//! * an **edge gateway** (`edge`) accepts ordinary local `AF_UNIX`
//!   client connections and bridges them to a remote hub — a client
//!   dialing the edge is indistinguishable from one dialing the
//!   compositor directly,
//! * a **hub gateway** (`hub`) accepts authenticated TCP sessions and
//!   bridges them to the compositor's real `AF_UNIX` socket,
//! * the **remote wire** (`wire`) is a 16-byte envelope protocol whose
//!   length fields are validated *before* any allocation — the
//!   length-bomb discipline the Wayland bridge learned in Phase 19,
//!   applied at layer zero of a new surface,
//! * the **FD relay vocabulary** translates every descriptor-bearing
//!   protocol path into bytes: shm pools ship once and then per-commit
//!   buffer ranges (`POOL_UPDATE`), eventfd fences ship their counter
//!   (`FENCE`), read-once files ship whole (`FD_SEGMENT`), and pipes
//!   stream bidirectionally chunk-by-chunk (`STREAM`),
//! * `tracker` is the decode-driven object table both sides maintain —
//!   the same `ldp-protocol` schema registry the server itself uses,
//!   so the relay's notion of "which object is which interface" can
//!   never drift from the protocol it forwards.
//!
//! # Security model
//!
//! * **Authentication**: every TCP session begins with a `HELLO`
//!   carrying a 32-byte bearer token, compared in constant time.
//!   Remote identity is the token; `SO_PEERCRED` cannot cross a
//!   network, so the hub attributes the whole session to the local
//!   account running the gateway (single-user remote display — the
//!   documented v0.2 scope; see `docs/maturity.md`).
//! * **Caps**: envelope bodies are capped and negotiated at `HELLO`
//!   (`max_envelope`), pooled shm bytes per session are capped
//!   (`pool_total_cap`), and every length is checked against the cap
//!   *before* the allocation that would honor it.
//! * **DoS surface**: a malicious remote can make the relay allocate at
//!   most `max_envelope` bytes per envelope and hold at most
//!   `pool_total_cap` shm bytes — bounded by construction, enforced
//!   with tests.
//! * **Boundaries** (explicit, never silent): GPU descriptors —
//!   `dmabuf.create`, `fence.import_sync_file`, `fence.import_syncobj`
//!   — are rejected with `BYE(unsupported_fd)`; explicit-sync GPU
//!   state cannot be represented as bytes and belongs to the
//!   Vulkan-era renderer work.
//!
//! # Ordering contract
//!
//! Envelopes are processed strictly in arrival order. The sending side
//! guarantees: for every relayed message, its FD envelopes
//! (`FD_SEGMENT` / `FENCE` / `STREAM` open) precede the `DATA`
//! envelope of the message itself, and every `POOL_UPDATE` /
//! `POOL_RESIZE` precedes the `DATA` envelope of the commit whose
//! pixels it feeds. The receiving side applies updates before
//! forwarding the message, so the compositor's mmap observes exactly
//! the pixel state the client committed — the shm write-then-commit
//! contract, preserved across a network.
//!
//! ```text
//!  client ──AF_UNIX──▶ edge gateway ══TCP (LDPR1)══▶ hub gateway ──AF_UNIX──▶ compositor
//!           (SCM_RIGHTS)      (envelopes +            (memfd             (SCM_RIGHTS,
//!                             inline FD bytes)         reconstruction)     read-only mmap)
//! ```
//!
//! # Safety policy
//!
//! `unsafe` exists only in [`sys`] as individually documented blocks
//! (`memfd_create`, `eventfd`, `pipe2`, `poll`, `shutdown` — the
//! ldp-transport/ldp-tools precedent); every other module is
//! `#![forbid(unsafe_code)]`. The crate is Linux-only, like the
//! transport it extends.

#![cfg_attr(not(target_os = "linux"), compile_error("ldp-remote is Linux-only: it relays AF_UNIX/SCM_RIGHTS sessions, a Linux transport contract"))]
#![deny(missing_docs)]

pub mod edge;
pub mod hub;
pub mod link;
pub mod sys;
pub mod tracker;
pub mod wire;

mod relay;

pub use edge::{EdgeConfig, EdgeGateway};
pub use hub::{HubConfig, HubGateway};
pub use link::{Keepalive, SessionCaps, SocketSet, AUTH_TOKEN_BYTES};
pub use tracker::{FdDisposition, FdSemantics, Plan, ShipBefore, Tracker};
pub use wire::{Envelope, EnvelopeKind, RemoteConfig, WIRE_HEADER_BYTES};

/// Crate result alias.
pub type Result<T> = ldp_core::error::Result<T>;
