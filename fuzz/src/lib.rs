//! # ldp-fuzz — deterministic fuzzers for the LDP stack
//!
//! Five seed-addressed targets (`docs/roadmap.md` Phase 19), each a
//! library function so CI runs them under its own budget and anyone
//! can reproduce a finding by seed alone:
//!
//! * [`codec`] — the LDP wire codec: every iteration synthesizes a
//!   *valid* message straight from the compiled spec
//!   ([`ldp_test::conformance::spec_message`]), proves the strict
//!   round-trip invariant, then feeds structure-aware and blind
//!   mutants through the decoder.
//! * [`transport`] — the framing reader over real socketpairs:
//!   fragmented delivery (partial headers, split payloads), declared
//!   FD-count lies, and real SCM_RIGHTS traffic — with process FD
//!   stability asserted across the whole run.
//! * [`dispatch`] — the server dispatch path: a real `ldp-server`
//!   `Server` with the null dispatcher, fuzzed client bursts
//!   (spec-shaped garbage, stale object ids, mid-message vanishing)
//!   — sessions must die cleanly and the server must stay healthy.
//! * [`x11`] — the X11 bridge request framing: short and
//!   BIG-REQUESTS framing, both endiannesses, oversize and
//!   zero-length escapes.
//! * [`wayland`] — the Wayland bridge wire codec against the pinned
//!   interface tables: valid round-trips and mutant decodes.
//!
//! No external fuzzing framework: the mutators live in
//! `ldp-test::mutate`, the RNG is `ldp-test::rng::SplitMix64`, and
//! every run is a pure function of `(seed, iterations)`.
//!
//! CI budget: `tests/fuzz_ci.rs` runs all five targets at the default
//! scale (asserting clean completion, both accept and reject paths
//! exercised, zero panics via a counting panic hook, and FD
//! stability). `LDP_FUZZ_SCALE=<n>` multiplies every iteration count
//! for longer soak runs.

#![forbid(unsafe_code)]

pub mod codec;
pub mod dispatch;
pub mod report;
pub mod transport;
pub mod wayland;
pub mod x11;

pub use report::TargetReport;

/// The default iteration scale (multiplicative, overridable via
/// `LDP_FUZZ_SCALE` for soak runs).
#[must_use]
pub fn scale() -> u64 {
    std::env::var("LDP_FUZZ_SCALE")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|s| *s > 0)
        .unwrap_or(1)
}
