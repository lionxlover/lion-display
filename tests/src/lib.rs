//! # ldp-integration — the Phase 19 cross-crate gates
//!
//! The three integration suites of `docs/roadmap.md` Phase 19, driven
//! against the **real in-process compositor** (the Phase 10 vertical
//! slice: full protocol stack, mock KMS device, live timeline):
//!
//! * [`stress`] — the stress gate: 32 concurrent clients, mixed
//!   workloads (frame cycles with real memfd pools, sync round-trips,
//!   buffer churn, crash-and-reconnect), stability asserted at the
//!   end. Compressed by default; `LDP_STRESS_FULL=1` runs the full
//!   15-minute gate.
//! * [`crash`] — the crash corpus: clients vanishing at every
//!   protocol cut point (before hello, mid pool, after commit,
//!   mid-frame) with a healthy canary session after each — the
//!   server must reclaim and stay unwedged.
//! * [`hotplug`] — the hotplug simulation: connector topology churn
//!   injected into the compositor's own mock device while the frame
//!   pipeline keeps serving clients.
//!
//! The shared driver lives in [`harness`]: it replicates the Phase 10
//! testbench shape (an in-process compositor on a background accept
//! loop, protocol clients over the real transport) as a library, so
//! these suites are ordinary `cargo test` targets.

#![forbid(unsafe_code)]
// The whole crate is a test harness: every public function fails
// loudly (panics with the failing step named) by design — per-step
// `# Panics` sections would add noise, not information.
#![allow(clippy::missing_panics_doc)]

pub mod crash;
pub mod harness;
pub mod hotplug;
pub mod stress;

/// Whether the *full* (15-minute) stress gate was requested via
/// `LDP_STRESS_FULL=1`.
#[must_use]
pub fn full_stress_requested() -> bool {
    std::env::var("LDP_STRESS_FULL").is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}
