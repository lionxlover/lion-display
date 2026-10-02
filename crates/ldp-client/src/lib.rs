//! # ldp-client — the LDP client library
//!
//! The client half of the protocol stack (`docs/architecture.md` §5,
//! roadmap Phase 5): connection and handshake, proxy objects, typed
//! event dispatch with class lanes, the sync round-trip, and
//! disconnect/reconnect helpers.
//!
//! * [`Connection`] — one server session: `hello`/`welcome` handshake,
//!   the inbound §9 pipeline (framing → decode → **proxy resolution
//!   before signature check** → classification), blocking pump +
//!   bounded-budget dispatch, [`Connection::roundtrip`] barriers,
//! * [`proxy::Proxy`] / [`proxy::ProxyMap`] — the client-side object
//!   model with destroy-window states and automatic proxies for
//!   server-announced objects (`new_id` events),
//! * [`class`] / [`queue`] — the lane scheduler: input-class events are
//!   dispatched before every event that arrived after them, whatever
//!   the presentation backlog; control-class events are barriers that
//!   never reorder against the past,
//! * [`core_api`] — typed views of the `connection`/`registry` events
//!   every client consumes, plus the [`core_api::EventHandler`] trait,
//! * [`reconnect`] — [`Reconnector`]: bounded exponential backoff with
//!   deterministic jitter, application-driven session rebuild.
//!
//! # Threading
//!
//! One connection, one thread (v1): the handler receives events only,
//! never a connection handle, so dispatch re-entrancy is a compile
//! error rather than a deadlock. Blocking reads park in
//! [`Connection::pump_one`]; event loops flip to nonblocking after the
//! handshake and drive `pump_one` → [`PumpOutcome::WouldBlock`] plus
//! [`Connection::dispatch_budget`] themselves.
//!
//! # Example
//!
//! ```
//! use ldp_client::{Connection, NoopHandler};
//! use ldp_transport::UnixAddr;
//!
//! # fn main() -> ldp_client::Result<()> {
//! # let name = format!("\u{0}ldp-lib-doc-{}", std::process::id());
//! let addr = UnixAddr::abstract_name(name.as_bytes())?;
//! // A real server listens here; the handshake would fail otherwise.
//! # drop(addr);
//! # Ok(())
//! # }
//! ```
//!
//! The full-session flow (against a live `ldp-server`) is exercised by
//! this crate's integration tests: connect → handshake → registry →
//! globals → bind → roundtrip → ping → destroy → clean disconnect.
//!
//! # Safety and dependencies
//!
//! `#![forbid(unsafe_code)]` — all syscall `unsafe` lives in
//! `ldp-transport`'s audited `sys` module. Runtime dependencies are the
//! LDP layer crates only; no async runtime, no `rand` (reconnect jitter
//! is a seeded LCG).

#![forbid(unsafe_code)]

pub mod allocator;
pub mod class;
pub mod config;
pub mod connection;
pub mod core_api;
pub mod error;
pub mod inbound;
pub mod proxy;
pub mod queue;
pub mod reconnect;
pub mod registry_api;
pub mod send;

pub use allocator::IdAllocator;
pub use class::{class_of, EventClass};
pub use config::{ClientConfig, ReconnectPolicy, CLIENT_RELEASE};
pub use connection::{Connection, PumpOutcome};
pub use core_api::{
    Destroyed, EventHandler, FatalError, NoopHandler, Pong, Revoked, SyncDone, Welcome,
};
pub use error::{classify_disconnect, ClientError, DisconnectKind, Result as ClientResult, Result};
pub use proxy::{Proxy, ProxyMap};
pub use queue::{Event, EventQueue};
pub use reconnect::{connect_with_retry, Rebuild, ReconnectOutcome, Reconnector};
pub use registry_api::{Bound, Global, GlobalRemove};

#[cfg(test)]
mod tests {
    #[test]
    fn crate_surface_is_reachable() {
        // Compile-level smoke: the prelude names resolve and defaults
        // are the documented v1 values.
        assert_eq!(crate::EventClass::Input.priority(), 0);
        assert_eq!(crate::ReconnectPolicy::default().max_attempts, 8);
        assert_eq!(
            crate::ClientConfig::default().release,
            crate::CLIENT_RELEASE
        );
        assert_eq!(crate::CLIENT_RELEASE, 1);
    }
}
