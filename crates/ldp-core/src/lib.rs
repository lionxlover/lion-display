//! # ldp-core — foundation types of the Lion Display Protocol
//!
//! Shared vocabulary for every other LDP crate: [`ObjectId`] identity with
//! the client/server split and generation discipline, [`VersionRange`]
//! negotiation, the [`ErrorCode`] taxonomy, the wire [`Value`] model
//! (mirroring `docs/protocol.md` §4), geometry and damage algebra
//! ([`Rect`], [`Region`], [`Transform`]), frame-timing types ([`Mono`],
//! [`FrameDeadline`]), color and HDR metadata types, buffer formats
//! ([`FourCC`], [`Modifier`]), the 128-bit [`Bitset128`], security
//! [`ScopeSet`]s and 256-bit [`AccessToken`]s, and the protocol
//! [`Limits`].
//!
//! ## Design rules enforced by this crate
//!
//! * **Zero dependencies** — the foundation builds anywhere, forever.
//! * **No `unsafe`** — `#![forbid(unsafe_code)]`; syscalls live in
//!   `ldp-transport` and above, never here.
//! * **Determinism** — no floats where rationals suffice ([`ScaleFactor`]
//!   is Q8.8), no NaN reaches a wire value ([`Value`] constructors reject
//!   non-finite floats), timestamps are monotonic nanoseconds.
//! * **Wire-first** — every type here either appears on the wire or
//!   constrains something that does; the encoding rules are documented on
//!   the type and tested in this crate.
//!
//! ## Where things go next
//!
//! `ldp-protocol` (Phase 2) turns `spec/*.toml` into schema tables and
//! implements the codec around [`wire`] values; `ldp-transport` (Phase 3)
//! carries framed messages over Unix sockets; the server validates
//! generational identity with [`ids`].
//!
//! [`Bitset128`]: crate::bitset::Bitset128
//! [`ScopeSet`]: crate::caps::ScopeSet

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![warn(rust_2018_idioms)]
#![warn(unused_qualifications)]

pub mod bitset;
pub mod buffer;
pub mod caps;
pub mod color;
pub mod error;
pub mod geometry;
pub mod ids;
pub mod limits;
pub mod prelude;
pub mod scale;
pub mod time;
pub mod token;
pub mod version;
pub mod wire;

pub use prelude::*;
