//! # ldp-server — the LDP server core
//!
//! The connection/session engine of an LDP server
//! (`docs/architecture.md` §4–5, Phase 4 of the roadmap):
//!
//! * [`config`] — [`ServerConfig`]: release, capability bits, advertised
//!   globals, grantable options, limits, client ceiling, audit sink,
//!   sandbox policy,
//! * [`server`] — [`Server`]: accept-with-credentials, the client-ID
//!   allocator, the `max_clients` ceiling, one session thread per
//!   connection (with panic isolation),
//! * [`session`] — [`ClientSession`]: the per-connection state machine —
//!   handshake, negotiated limits, and the §9 pipeline: framing →
//!   structural decode → **object resolution before signature check**
//!   (the type-confusion defense) → dispatch,
//! * [`object`] — [`ObjectStore`]: the generational per-connection
//!   object table (wire references resolve to current occupants; stored
//!   handles are generation-checked — the ID-reuse misdelivery
//!   protection),
//! * [`builtin`] — the two interfaces the session core implements
//!   itself: `ldp.core.connection` (hello/welcome, sync, ping, the
//!   central `destroy`/`destroyed` lifecycle, `get_registry` bootstrap)
//!   and `ldp.core.registry` (bind/bound with version pinning, global
//!   advertisement replay, introspection),
//! * [`dispatch`] — the [`Dispatcher`] seam: every other interface is
//!   implemented through it (the Phase 6 compositor *is* a dispatcher);
//!   dispatchers emit events, create and revoke objects, and take
//!   ownership of message FDs through [`DispatchCtx`],
//! * [`schema_json`] — the compact JSON rendering behind
//!   `registry.schema`,
//! * [`audit`] — the [`AuditSink`] interface points (connection,
//!   handshake, bind/destroy, denials, fatal errors, disconnects,
//!   rejections).
//!
//! # Safety and dependencies
//!
//! `#![forbid(unsafe_code)]` — all syscall `unsafe` lives in
//! `ldp-transport`'s audited `sys` module. Runtime dependencies are the
//! three LDP layer crates only.
//!
//! # Phase-4 scope
//!
//! This crate is the *protocol* server: object lifecycle, dispatch,
//! ceilings, reclamation, audit. Rendering, input routing, and window
//! management arrive with the compositor phases and plug in through
//! [`Dispatcher`].
//!
//! ```
//! use ldp_core::bitset::Bitset128;
//! use ldp_server::{GlobalAdvert, Server, ServerConfig};
//! use ldp_transport::{TransportListener, UnixAddr};
//!
//! # fn main() -> ldp_core::error::Result<()> {
//! let config = ServerConfig {
//!     globals: vec![GlobalAdvert::new("ldp.core.output")],
//!     ..ServerConfig::default()
//! };
//! let mut server = Server::new(config)?;
//! let addr = UnixAddr::abstract_name(
//!     format!("\u{0}ldp-doc-{}", std::process::id()).as_bytes(),
//! )?;
//! let mut listener = TransportListener::bind(&addr, 8)?;
//! // Accept is blocking; drive it from your event loop or
//! // serve_blocking. See the integration tests for full sessions.
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]

pub mod audit;
pub mod builtin;
pub mod config;
pub mod dispatch;
pub mod object;
pub mod schema_json;
pub mod server;
pub mod session;

pub use audit::{AuditRecord, AuditRecorder, AuditSink, NullAudit};
pub use config::{
    connection_options, server_caps, GlobalAdvert, SandboxFlavor, SandboxPolicy, ServerConfig,
};
pub use dispatch::{DispatchCtx, Dispatcher, IncomingRequest, NullDispatcher};
pub use object::{Lookup, ObjectEntry, ObjectKind, ObjectStore, StoreHandle};
pub use schema_json::render_interface;
pub use server::Server;
pub use session::{
    ClientSession, SessionEnd, SessionOutcome, CONNECTION_INTERFACE, REGISTRY_INTERFACE,
};
