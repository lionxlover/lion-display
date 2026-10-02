//! # ldp-vrr — adaptive-sync policy, refresh windows, tearing gate
//!
//! The VRR policy layer (`docs/architecture.md` §13, roadmap Phase 15):
//! a pure, deterministic, table-driven engine that decides **whether**
//! an output runs adaptive sync, **which** refresh window it may use,
//! **when** each committed frame should flip, and **whether** tearing is
//! permitted — never the latter as a side effect of the former.
//!
//! * [`caps`] — the output's adaptive-sync support: the panel window
//!   `[min, max]` refresh (validated so the nominal mode period sits
//!   inside it) and the `vrr_caps` wire bits (seamless toggle,
//!   fixed-rate fallback),
//! * [`policy`] — the decision table: policy (off / deadline / always),
//!   support, surface mix, and battery state combine into a
//!   [`policy::PolicyDecision`] carrying the effective window, the
//!   scheduler deadline widening, and a machine-checkable rationale,
//! * [`window`] — the window arithmetic: the deadline widening handed to
//!   [`ldp_compositor::scheduler::SchedulerConfig`] and
//!   [`window::select_flip`], the **flip-slip avoidance** rule (a flip
//!   never lands closer than the minimum refresh interval after the
//!   previous one, never later than the maximum stretch),
//! * [`refresh`] — the [`refresh::RefreshSelector`]: the per-output
//!   commit-at-ready latency optimizer that turns ready timestamps into
//!   flip rulings, including the fixed-rate fallback that defers
//!   below-minimum-rate frames as `throttled`,
//! * [`tearing`] — the tearing opt-in gate (immediate-mode surface +
//!   async-flip capability + session permission), with the independence
//!   property that VRR enablement never enters the decision,
//! * [`quirk`] — the quirk ledger's first page (Phase 41): the
//!   **effective floor** (`--vrr-floor N`) — the operator's honest
//!   minimum refresh rate for panels whose advertised range flickers
//!   at the bottom, clamped into every window consumer (the
//!   scheduler's widening, the wire advertisement, the LFC cadence),
//!   plus the per-output CSV grammar (`--vrr-floor F1,F2,…`, the
//!   `--scale` grammar's mirror),
//! * [`engine`] — the [`engine::VrrEngine`]: the stateful per-output
//!   composition of all of the above. Policy changes surface as
//!   [`engine::VrrEvent::WindowApplied`] (program `VRR_ENABLED` and the
//!   window bounds through the KMS backend), deferred frames as
//!   [`engine::VrrEvent::Throttled`] (forward as `frame_dropped`
//!   with reason `throttled` and the retry hint).
//!
//! # The three policies
//!
//! * **off** — fixed sync at the mode refresh. No window, no widening:
//!   the deadline scheduler behaves exactly as it did before VRR.
//! * **deadline** — VRR window mode. Frame targets stay on the nominal
//!   vblank grid; the panel grants late commits up to
//!   `max − nominal` of stretch, which is exactly the widening installed
//!   into the scheduler, so a commit inside the widened window still
//!   presents at a flip inside the panel window.
//! * **always** — lowest achievable latency, still tear-free. The
//!   selector owns the timing: every commit flips at
//!   `max(ready, last_flip + min)` — commit-at-ready with the
//!   flip-slip floor — and the scheduler widening spans the whole
//!   selectable range (`max − min`) so in-cadence commits never miss.
//!
//! # Determinism doctrine
//!
//! Matching the scheduler and the mock KMS device: **no clock reads**.
//! Every input carries its own [`Mono`](ldp_core::time::Mono) timestamp,
//! every decision is integer arithmetic, and the same input stream
//! always yields the same event vector — the golden timeline suites pin
//! those vectors to the nanosecond.
//!
//! # Safety and dependencies
//!
//! `#![forbid(unsafe_code)]`. Runtime dependencies: `ldp-core` and
//! `ldp-compositor` only. `ldp-display` is a dev-dependency used by the
//! integration suites to drive the deterministic mock panel.

#![forbid(unsafe_code)]

pub mod caps;
pub mod engine;
pub mod policy;
pub mod quirk;
pub mod refresh;
pub mod tearing;
pub mod window;

pub use caps::{OutputVrrSupport, PanelWindow, VrrCaps, WindowError};
pub use engine::{VrrEngine, VrrEvent};
pub use policy::{PolicyDecision, PolicyInputs, Rationale, VrrPolicy};
pub use quirk::{apply_floor, parse_floor_list, FloorError, FloorOutcome};
pub use refresh::{CommitRuling, RefreshSelector};
pub use tearing::{TearingDecision, TearingInputs};
pub use window::{FlipSelection, RefreshWindow};
