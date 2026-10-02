//! # ldp-protocol — the LDP wire codec and schema tables
//!
//! Everything between the byte stream and dispatch:
//!
//! * [`envelope`] — the 16-byte message header and flags,
//! * [`codec`] — tagged argument units (`docs/protocol.md` §4) with the
//!   security property that no allocation proportional to untrusted
//!   sizes happens before all length fields are bounds-checked,
//! * [`message`] — whole-message [`encode`](Message::encode) /
//!   [`decode`] covering validation stages 1–2,
//! * [`validate`] — stage 3: signature checking against the schema,
//! * [`schema`] / [`registry`] / [`generated`] — the protocol surface as
//!   data, compiled from `spec/*.toml` by `ldpc` (committed output; CI
//!   fails on drift),
//! * [`blob`] — parser for the introspection blob (the spec,
//!   re-serialized) served by `registry.schema` and consumed by tooling.
//!
//! The crate is `#![forbid(unsafe_code)]` (a permanent rule,
//! CONTRIBUTING.md) and depends only on `ldp-core`.
//!
//! ```
//! use ldp_core::limits::Limits;
//! use ldp_core::wire::Value;
//! use ldp_protocol::{check_signature, decode, Direction, Message, ValidationMode, REGISTRY};
//!
//! // registry.bind(interface, version, new_id) — request opcode 1.
//! let msg = Message::new(2, 1)
//!     .arg(Value::String("ldp.core.output".into()))
//!     .arg(Value::Uint32(1))
//!     .arg(Value::NewId(ldp_core::ids::ObjectId::client(9).unwrap()));
//! let bytes = msg.encode(&Limits::DEFAULT).unwrap();
//! let fd_count = msg.required_fd_count();
//! let back = decode(&bytes, fd_count, &Limits::DEFAULT, ValidationMode::Strict).unwrap();
//! assert_eq!(back, msg);
//! let (_, op) = check_signature(
//!     &REGISTRY, &back, "ldp.core.registry", Direction::Request, 1,
//!     ValidationMode::Strict,
//! ).unwrap();
//! assert_eq!(op.name, "bind");
//! ```

#![forbid(unsafe_code)]

pub mod blob;
pub mod bytes;
pub mod codec;
pub mod envelope;
pub mod generated;
pub mod message;
pub mod registry;
pub mod schema;
pub mod validate;

pub use envelope::{Flags, Header, HEADER_BYTES};
pub use message::{decode, Message, ValidationMode};
pub use registry::SchemaRegistry;
pub use schema::{
    ArgSchema, BitsetDef, Direction, EnumDef, InterfaceSchema, ModuleSchema, OpSchema,
};
pub use validate::check_signature;

/// Every module schema compiled into this build (ldpc output).
pub use generated::{BLOB, MODULES};

/// The registry over every compiled module.
pub const REGISTRY: SchemaRegistry<'_> = SchemaRegistry::new(MODULES);
