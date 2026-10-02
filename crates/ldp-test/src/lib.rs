//! # ldp-test — deterministic test support for LDP
//!
//! The Phase 19 support crate (`docs/roadmap.md`): the shared machinery
//! behind the conformance suites, the deterministic fuzzers (`fuzz/`),
//! the integration stress/crash/hotplug gates (`tests/`), and the
//! benchmark harness (`ldp-bench`).
//!
//! Doctrine — everything here is **deterministic and seed-addressed**:
//!
//! * [`rng`] — [`SplitMix64`], the one PRNG every harness shares: same
//!   seed, same stream, on every machine and every run. A failure is
//!   reproducible by its seed alone.
//! * [`mutate`] — byte- and structure-level mutators for the fuzzers:
//!   they never invent non-determinism, they only spend RNG budget.
//! * [`conformance`] — the spec-walking conformance runner: for every
//!   interface, direction, and opcode compiled into
//!   [`ldp_protocol::REGISTRY`], synthesize a canonical message from
//!   the signature, prove the encode/decode/strict-validation
//!   round-trip, and sweep the per-type boundary values. The spec is
//!   the oracle; the suite is derived, not enumerated by hand.
//! * `bench` — the benchmark harness: sample-taking runners with
//!   median/mean/p95 statistics, JSON and markdown report rendering.
//! * [`json`] — the minimal JSON writer used by the reports (no
//!   dependencies; the same hand-rolled discipline as
//!   `ldp-server`'s `schema_json`).
//!
//! This crate is test *support*: it depends on the crates it drives
//! but nothing depends on it. It contains no unsafe code and never
//! will.

#![forbid(unsafe_code)]

pub mod bench;
pub mod benchmarks;
pub mod conformance;
pub mod json;
pub mod mutate;
pub mod rng;

pub use rng::SplitMix64;
