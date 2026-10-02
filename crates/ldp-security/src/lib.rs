//! LDP capability security — the permission matrix, the session broker,
//! and the hash-chained audit log.
//!
//! This crate is the *policy* half of `docs/threat-model.md`: it
//! decides who may do what, mints and validates the 256-bit bearer
//! tokens that carry grants, and records every decision in a
//! tamper-evident chain. The *enforcement* half lives in the server
//! (scope checks before argument validation, `unauthorized` + audit on
//! denial); both share this crate's types.
//!
//! Layered policy (deny-by-default):
//!
//! 1. [`manifest`]: the launch-time baseline. Apps ship a signed
//!    manifest; the broker verifies it before the app connects and
//!    derives the connection's baseline scopes from it.
//! 2. [`matrix`]: the operation→requirement table (F / M / E / M+E /
//!    impossible) and the [`matrix::decide`] enforcement answer.
//! 3. [`broker`]: the only component allowed to make grant decisions —
//!    escalation requests, rate limiting, the brokered user prompt
//!    (a seam, [`broker::PromptDecider`]), token minting.
//! 4. [`grant`]: the grant table — submission validation with
//!    constant-time compares, expiry, revocation, app binding (the
//!    forgery/replay rules).
//! 5. [`chain`]: the audit log. SHA-256 ([`sha256`], pure Rust, NIST
//!    vectors pinned) chains every record; verification recomputes the
//!    retained window and reports the first break.
//!
//! Purity doctrine: the crate reads no clock (callers inject
//! [`Mono`](ldp_core::time::Mono) everywhere), opens no sockets, and
//! draws entropy only through the [`grant::TokenSeed`] seam — the
//! deployment host plugs OS randomness, tests plug the deterministic
//! [`grant::LcgSeed`]. `#![forbid(unsafe_code)]` crate-wide.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod broker;
pub mod chain;
pub mod grant;
pub mod manifest;
pub mod matrix;
pub mod sha256;

pub use broker::{BrokerConfig, EscalationOutcome, PromptAnswer, PromptDecider, SessionBroker};
pub use chain::{AuditAction, AuditChain, AuditEvent, AuditFilter, ChainResult, ChainedRecord};
pub use grant::{DeniedReason, GrantTable, LcgSeed, SubmitOutcome, TokenSeed};
pub use manifest::{AppManifest, ManifestError};
pub use matrix::{
    decide, manifest_baselines, requirement, Decision, Operation, Requirement, ALL_OPERATIONS,
};
