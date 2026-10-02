//! The per-output VRR engine: policy, selector, and the event stream.
//!
//! [`VrrEngine`] composes the pure layers into the stateful object the
//! frame loop drives. Configuration changes (policy, battery state,
//! surface mix) flow through the policy table; when the **effective**
//! KMS state changes — enablement or window bounds — a
//! [`VrrEvent::WindowApplied`] is emitted, and the embedder programs
//! `VRR_ENABLED` (plus, on real hardware, the window bounds) through
//! the next atomic commit. Frame-ready queries go through the
//! [`RefreshSelector`]; its fixed-rate deferrals surface as
//! [`VrrEvent::Throttled`], which the embedder forwards as
//! `frame_dropped(throttled)` with the retry hint.
//!
//! The engine also owns the scheduler seam:
//! [`VrrEngine::scheduler_config`] returns the base
//! [`SchedulerConfig`] with `vrr_window_ns` installed from the current
//! decision, so the deadline scheduler and the panel agree on the same
//! window. Timeline inputs (`observe_flip`, `commit_ready`) carry their
//! own timestamps and must be non-decreasing — the same contract as the
//! scheduler's driver.

use ldp_compositor::scheduler::SchedulerConfig;
use ldp_compositor::SurfaceId;
use ldp_core::time::Mono;

use crate::caps::OutputVrrSupport;
use crate::policy::{decide, PolicyDecision, PolicyInputs, VrrPolicy};
use crate::refresh::{CommitRuling, RefreshSelector};
use crate::window::RefreshWindow;

/// An emission from the engine, in dispatch order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VrrEvent {
    /// The effective KMS state changed (or the initial state at
    /// construction): program `VRR_ENABLED` accordingly; when enabling,
    /// `window` carries the bounds and `seamless` whether the toggle
    /// needs a modeset on this panel.
    WindowApplied {
        /// `VRR_ENABLED` value to program.
        enable: bool,
        /// The window bounds (present iff enabled).
        window: Option<RefreshWindow>,
        /// Whether the panel toggles without a modeset.
        seamless: bool,
    },
    /// The fixed-rate fallback deferred a frame: forward as
    /// `frame_dropped(throttled)`; `retry_at` is the grid point the
    /// surface should aim for next.
    Throttled {
        /// The deferred surface.
        surface: SurfaceId,
        /// The deferred frame identifier.
        frame: u64,
        /// When the deferred frame's slot opens.
        retry_at: Mono,
    },
}

/// Per-output adaptive-sync engine.
pub struct VrrEngine {
    support: Option<OutputVrrSupport>,
    policy: VrrPolicy,
    battery_saver: bool,
    adaptive_demand: bool,
    decision: PolicyDecision,
    selector: RefreshSelector,
    sched_base: SchedulerConfig,
    out: Vec<VrrEvent>,
    last_ts: Mono,
}

impl VrrEngine {
    /// Build the engine for an output.
    ///
    /// The first [`VrrEvent::WindowApplied`] (the initial state) is
    /// already queued: drain it and program the CRTC before the first
    /// flip.
    pub fn new(
        support: Option<OutputVrrSupport>,
        policy: VrrPolicy,
        sched_base: SchedulerConfig,
    ) -> Self {
        let decision = decide(&PolicyInputs {
            policy,
            support: support.as_ref(),
            adaptive_demand: false,
            battery_saver: false,
        });
        let selector = Self::build_selector(support.as_ref(), &decision, policy);
        let seamless = support.is_some_and(|s| s.caps().seamless);
        let out = vec![VrrEvent::WindowApplied {
            enable: decision.vrr_enabled,
            window: decision.window,
            seamless,
        }];
        Self {
            support,
            policy,
            battery_saver: false,
            adaptive_demand: false,
            decision,
            selector,
            sched_base,
            out,
            last_ts: Mono::ZERO,
        }
    }

    /// The current policy-table decision (audit and window access).
    #[must_use]
    pub const fn decision(&self) -> &PolicyDecision {
        &self.decision
    }

    /// The scheduler config with the current window installed
    /// (`vrr_window_ns` from the decision; every other knob passes
    /// through from the base).
    #[must_use]
    pub const fn scheduler_config(&self) -> SchedulerConfig {
        SchedulerConfig {
            vrr_window_ns: self.decision.scheduler_window_ns,
            ..self.sched_base
        }
    }

    /// Take the pending emissions (dispatch order preserved).
    pub fn drain(&mut self) -> Vec<VrrEvent> {
        core::mem::take(&mut self.out)
    }

    /// Change the policy (session `configure_display` / `apply`).
    pub fn set_policy(&mut self, policy: VrrPolicy) {
        if policy != self.policy {
            self.policy = policy;
            self.recompute();
        }
    }

    /// Change the battery-saver state (fed by the power layer).
    pub fn set_battery_saver(&mut self, saver: bool) {
        if saver != self.battery_saver {
            self.battery_saver = saver;
            self.recompute();
        }
    }

    /// Report whether any live surface schedules adaptively.
    pub fn set_adaptive_demand(&mut self, demand: bool) {
        if demand != self.adaptive_demand {
            self.adaptive_demand = demand;
            self.recompute();
        }
    }

    /// A scanout completed at `ts` (page-flip completion or idle
    /// vblank): re-anchors the selector's timeline.
    pub fn observe_flip(&mut self, ts: Mono) {
        self.enter(ts);
        self.selector.observe_flip(ts);
    }

    /// Whether the LFC repeat regime holds (Phase 41's anti-flap
    /// latch, the selector's own state): the embedder keeps the
    /// repeat cadence warm while content hovers at the window's
    /// boundary — the flap the driver quirk tables name.
    #[must_use]
    pub const fn lfc_engaged(&self) -> bool {
        self.selector.lfc_engaged()
    }

    /// The next LFC repeat slot for content at `period_ns` (the
    /// phase-aligned cadence — see
    /// [`crate::refresh::lfc_interval`]), or `None` when the cadence
    /// does not serve this period.
    #[must_use]
    pub fn lfc_repeat_at(&self, period_ns: u64) -> Option<Mono> {
        self.selector.lfc_repeat_at(period_ns)
    }

    /// Content became ready on `surface` (frame `frame`) at `ready_at`:
    /// the ruling the frame loop executes. Deferrals also queue a
    /// [`VrrEvent::Throttled`].
    #[must_use]
    pub fn commit_ready(&mut self, surface: SurfaceId, frame: u64, ready_at: Mono) -> CommitRuling {
        self.enter(ready_at);
        let ruling = self.selector.commit_ready(ready_at);
        if let CommitRuling::Defer { retry_at } = ruling {
            self.out.push(VrrEvent::Throttled {
                surface,
                frame,
                retry_at,
            });
        }
        ruling
    }

    /// Input-ordering contract (mirrors the scheduler's driver).
    fn enter(&mut self, ts: Mono) {
        assert!(
            ts.as_ns() >= self.last_ts.as_ns(),
            "vrr engine inputs must be non-decreasing (got {} after {})",
            ts.as_ns(),
            self.last_ts.as_ns()
        );
        self.last_ts = ts;
    }

    /// Re-run the table; when the effective KMS state changed, emit
    /// [`VrrEvent::WindowApplied`] and rebuild the selector while
    /// preserving its sequencing anchor.
    fn recompute(&mut self) {
        let decision = decide(&PolicyInputs {
            policy: self.policy,
            support: self.support.as_ref(),
            adaptive_demand: self.adaptive_demand,
            battery_saver: self.battery_saver,
        });
        let changed = decision.vrr_enabled != self.decision.vrr_enabled
            || decision.window != self.decision.window;
        if changed {
            let seamless = self.support.is_some_and(|s| s.caps().seamless);
            self.out.push(VrrEvent::WindowApplied {
                enable: decision.vrr_enabled,
                window: decision.window,
                seamless,
            });
            let mut selector = Self::build_selector(self.support.as_ref(), &decision, self.policy);
            selector.restore_anchor(self.selector.anchor());
            self.selector = selector;
        }
        self.decision = decision;
    }

    /// The selector matching a decision (degenerate window when the
    /// panel lacks support — the grid path answers then).
    fn build_selector(
        support: Option<&OutputVrrSupport>,
        decision: &PolicyDecision,
        configured: VrrPolicy,
    ) -> RefreshSelector {
        let (window, nominal_ns, fixed_rate) = match support {
            Some(s) => (
                RefreshWindow::from_support(s),
                s.nominal_ns(),
                s.caps().fixed_rate,
            ),
            None => {
                // No panel window: a degenerate 60 Hz window so the
                // selector has well-formed bounds; with VRR decided
                // off it only ever answers nominal-grid rulings.
                (
                    RefreshWindow::from_ns(16_666_666, 16_666_666)
                        .expect("degenerate window is well-formed"),
                    16_666_666,
                    false,
                )
            }
        };
        RefreshSelector::new(
            window,
            nominal_ns,
            fixed_rate,
            configured,
            decision.vrr_enabled,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: u64 = 1_000_000_000 / 144;
    const MAX: u64 = 1_000_000_000 / 48;
    const NOM: u64 = 16_666_666;

    fn support(seamless: bool, fixed_rate: bool) -> OutputVrrSupport {
        OutputVrrSupport::new(
            MIN,
            MAX,
            crate::caps::VrrCaps {
                seamless,
                fixed_rate,
            },
            NOM,
        )
        .unwrap()
    }

    #[test]
    fn initial_state_emits_window_applied() {
        let mut e = VrrEngine::new(
            Some(support(true, true)),
            VrrPolicy::Deadline,
            SchedulerConfig::default(),
        );
        let events = e.drain();
        assert_eq!(
            events,
            vec![VrrEvent::WindowApplied {
                enable: false, // no adaptive demand yet
                window: None,
                seamless: true
            }]
        );
        assert_eq!(e.scheduler_config().vrr_window_ns, 0);
        assert!(e.drain().is_empty());
    }

    #[test]
    fn demand_engages_the_window() {
        let mut e = VrrEngine::new(
            Some(support(true, true)),
            VrrPolicy::Deadline,
            SchedulerConfig::default(),
        );
        e.drain();
        e.set_adaptive_demand(true);
        let events = e.drain();
        assert_eq!(
            events,
            vec![VrrEvent::WindowApplied {
                enable: true,
                window: Some(RefreshWindow::from_ns(MIN, MAX).unwrap()),
                seamless: true
            }]
        );
        assert_eq!(e.scheduler_config().vrr_window_ns, MAX - NOM);
    }

    #[test]
    fn battery_saver_disables_and_reenables() {
        let mut e = VrrEngine::new(
            Some(support(true, true)),
            VrrPolicy::Deadline,
            SchedulerConfig::default(),
        );
        e.drain();
        e.set_adaptive_demand(true);
        e.drain();
        e.set_battery_saver(true);
        assert_eq!(
            e.drain(),
            vec![VrrEvent::WindowApplied {
                enable: false,
                window: None,
                seamless: true
            }]
        );
        assert_eq!(e.scheduler_config().vrr_window_ns, 0);
        e.set_battery_saver(false);
        assert_eq!(
            e.drain(),
            vec![VrrEvent::WindowApplied {
                enable: true,
                window: Some(RefreshWindow::from_ns(MIN, MAX).unwrap()),
                seamless: true
            }]
        );
    }

    #[test]
    fn redundant_sets_do_not_flap() {
        let mut e = VrrEngine::new(
            Some(support(true, true)),
            VrrPolicy::Deadline,
            SchedulerConfig::default(),
        );
        e.drain();
        e.set_adaptive_demand(true);
        e.drain();
        for _ in 0..5 {
            e.set_adaptive_demand(true);
            e.set_policy(VrrPolicy::Deadline);
            e.set_battery_saver(false);
        }
        assert!(e.drain().is_empty(), "no state change, no emission");
    }

    #[test]
    fn deferral_emits_throttled() {
        let mut e = VrrEngine::new(
            Some(support(true, true)),
            VrrPolicy::Deadline,
            SchedulerConfig::default(),
        );
        e.drain();
        e.set_adaptive_demand(true);
        e.drain();
        e.observe_flip(Mono::from_ns(1_000_000));
        let ready = Mono::from_ns(1_000_000 + 40_000_000);
        let surface = SurfaceId::from_raw(7);
        let ruling = e.commit_ready(surface, 12, ready);
        assert!(matches!(ruling, CommitRuling::Defer { .. }));
        assert_eq!(
            e.drain(),
            vec![VrrEvent::Throttled {
                surface,
                frame: 12,
                retry_at: Mono::from_ns(1_000_000 + 3 * NOM)
            }]
        );
    }

    #[test]
    fn timestamps_must_be_ordered() {
        let mut e = VrrEngine::new(
            Some(support(true, true)),
            VrrPolicy::Deadline,
            SchedulerConfig::default(),
        );
        e.drain();
        e.observe_flip(Mono::from_ns(5_000_000));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            e.observe_flip(Mono::from_ns(4_000_000));
        }));
        assert!(result.is_err(), "out-of-order input must assert");
    }

    #[test]
    fn unsupported_panel_stays_off() {
        let mut e = VrrEngine::new(None, VrrPolicy::Always, SchedulerConfig::default());
        let events = e.drain();
        assert!(matches!(
            events.as_slice(),
            [VrrEvent::WindowApplied {
                enable: false,
                window: None,
                ..
            }]
        ));
        e.set_adaptive_demand(true);
        assert!(e.drain().is_empty());
        assert_eq!(e.scheduler_config().vrr_window_ns, 0);
    }
}
