//! LDP power — idle stages, DPMS orchestration, backlight ramping,
//! suspend/resume with timeline re-anchoring, panel self-refresh, GPU
//! clock governance, and the energy ledger.
//!
//! The power layer of `docs/architecture.md` §18:
//!
//! * [`idle`]: the inhibitor-aware stage machine behind the
//!   `ldp.session.session.idle` events (blur/display/idle inhibitor
//!   delay semantics, monotone ladder, activity reset).
//! * [`backlight`]: flicker-free level ramps — the one path both the
//!   dim stage and the brightness slider ride.
//! * [`suspend`]: the sleeping/resumed pair and the vblank-grid
//!   re-anchor the compositor applies after the clock discontinuity
//!   (the Phase 16 re-anchoring exit criterion).
//! * [`psr`]: panel self-refresh — the sleeping-panel doctrine: the
//!   per-output decision machine that earns entry through consecutive
//!   quiet flip opportunities and names every exit (Phase 35).
//! * [`governor`]: the GPU clock ladder — asymmetric hysteresis over
//!   the landed-flip load signal (up instant, down patient), the
//!   DVFS seam the host's driver table applies (Phase 35).
//! * [`ledger`]: the energy accounting model — a documented
//!   parametric cost table and the accumulated time-in-state account
//!   that turns "power efficient" into a CI-reproducible ratio
//!   (Phase 35).
//!
//! Purity doctrine: the crate reads no clock (every `tick` takes the
//! injected `Mono`), touches no sysfs, issues no ioctls — the host
//! applies the emitted data and reports device state back. The
//! suspend controller is deliberately blind to inhibitors and fences
//! (the session and GPU layers own those), the same seam discipline
//! the VRR tearing gate uses.
//!
//! `#![forbid(unsafe_code)]` crate-wide; zero external dependencies.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod backlight;
pub mod governor;
pub mod idle;
pub mod ledger;
pub mod psr;
pub mod suspend;

pub use backlight::{BacklightRamp, DEFAULT_STEPS, DEFAULT_STEP_MS};
pub use governor::{ClockGovernor, GovernorEvent, PState};
pub use idle::{IdleEvent, IdleMachine, IdleStage, IdleTimeouts, InhibitMask};
pub use ledger::{EnergyLedger, LedgerState, PowerModel};
pub use psr::{PsrEvent, PsrExit, PsrMachine, PsrState};
pub use suspend::{ReAnchor, ResumeReport, SleepKind, SuspendController, SuspendState};
