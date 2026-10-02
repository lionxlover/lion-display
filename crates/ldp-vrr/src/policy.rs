//! The policy decision table (`docs/architecture.md` §13).
//!
//! One pure function, [`decide`], over the full input cross-product:
//! policy (off / deadline / always) × output support × surface mix ×
//! battery state. Every row is pinned by the exhaustive conformance
//! suite (`tests/policy_table.rs`, the Phase 15 exit criterion), and
//! each decision carries a [`Rationale`] so the table is auditable —
//! the same discipline as the HDR output-mode table.
//!
//! # The table (normative)
//!
//! | policy | support | adaptive demand | battery | → vrr | rationale |
//! |---|---|---|---|---|---|
//! | off | any | any | any | off | `PolicyOff` |
//! | deadline/always | none | any | any | off | `Unsupported` |
//! | deadline/always | window | no | any | off | `NoDemand` |
//! | deadline/always | window | yes | saver | off | `BatterySaver` |
//! | deadline | window | yes | ok | **on** | `DeadlineWindow` |
//! | always | window | yes | ok | **on** | `AlwaysOn` |
//!
//! Adaptive demand means at least one live surface in
//! [`PresentationMode::Adaptive`] (or the policy is `always`, which is
//! itself a demand). Tearing never appears in this table: it is a
//! separate opt-in decided by [`crate::tearing`], and no VRR state
//! change can produce or suppress it.

use ldp_core::time::PresentationMode;

use crate::caps::OutputVrrSupport;
use crate::window::RefreshWindow;

/// The per-output adaptive-sync policy (wire: `ldp.session.vrr_policy`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum VrrPolicy {
    /// Fixed sync at the mode refresh.
    #[default]
    Off,
    /// VRR window mode: commit at ready within `[min, max]` refresh.
    Deadline,
    /// Lowest achievable latency, still tear-free.
    Always,
}

impl VrrPolicy {
    /// Wire value.
    #[must_use]
    pub const fn to_wire(self) -> u32 {
        match self {
            Self::Off => 1,
            Self::Deadline => 2,
            Self::Always => 3,
        }
    }

    /// Parse a wire value.
    #[must_use]
    pub const fn from_wire(v: u32) -> Option<Self> {
        match v {
            1 => Some(Self::Off),
            2 => Some(Self::Deadline),
            3 => Some(Self::Always),
            _ => None,
        }
    }
}

/// The inputs to the policy table.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PolicyInputs<'a> {
    /// The configured policy (user preference, via session
    /// `configure_display`).
    pub policy: VrrPolicy,
    /// The output's adaptive-sync support (`None` = panel lacks VRR).
    pub support: Option<&'a OutputVrrSupport>,
    /// At least one live surface schedules adaptively.
    pub adaptive_demand: bool,
    /// The battery saver state (fed by the power layer; Phase 16
    /// wires the real source — the knob is pure input here).
    pub battery_saver: bool,
}

impl PolicyInputs<'_> {
    /// Whether the policy itself asks for adaptive sync.
    #[must_use]
    pub const fn wants_vrr(&self) -> bool {
        !matches!(self.policy, VrrPolicy::Off)
    }

    /// Demand for VRR: an adaptive surface, or the always policy
    /// (which applies to every commit on the output).
    #[must_use]
    pub const fn has_demand(&self) -> bool {
        self.adaptive_demand || matches!(self.policy, VrrPolicy::Always)
    }
}

/// Why the table reached its decision (machine-checkable audit trail).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum Rationale {
    /// The policy is `off`.
    PolicyOff,
    /// The panel has no adaptive-sync window.
    Unsupported,
    /// Nothing on the output schedules adaptively.
    NoDemand,
    /// Battery saver forces fixed sync.
    BatterySaver,
    /// Deadline-window mode engaged.
    DeadlineWindow,
    /// Always-on (lowest latency) mode engaged.
    AlwaysOn,
}

/// The table's output for one output.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PolicyDecision {
    /// Whether `VRR_ENABLED` should be programmed true on the CRTC.
    pub vrr_enabled: bool,
    /// The effective panel window (present iff `vrr_enabled`).
    pub window: Option<RefreshWindow>,
    /// The deadline widening to install into
    /// [`ldp_compositor::scheduler::SchedulerConfig::vrr_window_ns`]:
    /// `max − nominal` under deadline policy (the stretch a late commit
    /// may consume), the full `max − min` span under always (every
    /// in-window commit is on time), zero when off.
    pub scheduler_window_ns: u64,
    /// The audit rationale.
    pub rationale: Rationale,
}

/// The table itself. Pure; identical inputs yield identical decisions.
#[must_use]
pub const fn decide(inputs: &PolicyInputs<'_>) -> PolicyDecision {
    let PolicyInputs {
        policy,
        support,
        adaptive_demand,
        battery_saver,
    } = *inputs;
    if matches!(policy, VrrPolicy::Off) {
        return PolicyDecision {
            vrr_enabled: false,
            window: None,
            scheduler_window_ns: 0,
            rationale: Rationale::PolicyOff,
        };
    }
    let Some(support) = support else {
        return PolicyDecision {
            vrr_enabled: false,
            window: None,
            scheduler_window_ns: 0,
            rationale: Rationale::Unsupported,
        };
    };
    let demand = adaptive_demand || matches!(policy, VrrPolicy::Always);
    if !demand {
        return PolicyDecision {
            vrr_enabled: false,
            window: None,
            scheduler_window_ns: 0,
            rationale: Rationale::NoDemand,
        };
    }
    if battery_saver {
        return PolicyDecision {
            vrr_enabled: false,
            window: None,
            scheduler_window_ns: 0,
            rationale: Rationale::BatterySaver,
        };
    }
    let window = RefreshWindow::from_support(support);
    let scheduler_window_ns = match policy {
        VrrPolicy::Always => window.max_ns().saturating_sub(window.min_ns()),
        // Deadline: the panel grants a late commit up to max stretch;
        // the nominal grid point it targets is nominal away, so the
        // usable grace is the difference.
        _ => window.max_ns().saturating_sub(support.nominal_ns()),
    };
    let rationale = match policy {
        VrrPolicy::Always => Rationale::AlwaysOn,
        _ => Rationale::DeadlineWindow,
    };
    PolicyDecision {
        vrr_enabled: true,
        window: Some(window),
        scheduler_window_ns,
        rationale,
    }
}

/// Whether a surface mix creates adaptive demand.
///
/// Utility for embedders: any surface in adaptive mode demands the
/// window. (Immediate-mode surfaces demand tearing capability instead —
/// see [`crate::tearing`]; they never create VRR demand by themselves.)
#[must_use]
pub const fn mode_demands_vrr(mode: PresentationMode) -> bool {
    matches!(mode, PresentationMode::Adaptive)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caps::VrrCaps;

    const MIN: u64 = 1_000_000_000 / 144;
    const MAX: u64 = 1_000_000_000 / 48;
    const NOM: u64 = 16_666_666;

    fn support() -> OutputVrrSupport {
        OutputVrrSupport::new(
            MIN,
            MAX,
            VrrCaps {
                seamless: true,
                fixed_rate: true,
            },
            NOM,
        )
        .unwrap()
    }

    #[test]
    fn policy_wire_round_trip() {
        assert_eq!(VrrPolicy::default(), VrrPolicy::Off);
        for v in 1..=3u32 {
            let p = VrrPolicy::from_wire(v).unwrap();
            assert_eq!(p.to_wire(), v);
        }
        assert!(VrrPolicy::from_wire(0).is_none());
        assert!(VrrPolicy::from_wire(4).is_none());
    }

    #[test]
    fn off_is_off_regardless() {
        for support in [None, Some(support())] {
            for demand in [false, true] {
                for battery in [false, true] {
                    let d = decide(&PolicyInputs {
                        policy: VrrPolicy::Off,
                        support: support.as_ref(),
                        adaptive_demand: demand,
                        battery_saver: battery,
                    });
                    assert!(!d.vrr_enabled);
                    assert_eq!(d.window, None);
                    assert_eq!(d.scheduler_window_ns, 0);
                    assert_eq!(d.rationale, Rationale::PolicyOff);
                }
            }
        }
    }

    #[test]
    fn deadline_row_math() {
        let s = support();
        let d = decide(&PolicyInputs {
            policy: VrrPolicy::Deadline,
            support: Some(&s),
            adaptive_demand: true,
            battery_saver: false,
        });
        assert!(d.vrr_enabled);
        assert_eq!(d.rationale, Rationale::DeadlineWindow);
        assert_eq!(d.scheduler_window_ns, MAX - NOM);
        assert_eq!(d.window.map(|w| (w.min_ns(), w.max_ns())), Some((MIN, MAX)));
    }

    #[test]
    fn always_row_math() {
        let s = support();
        let d = decide(&PolicyInputs {
            policy: VrrPolicy::Always,
            support: Some(&s),
            adaptive_demand: false, // the policy itself is the demand
            battery_saver: false,
        });
        assert!(d.vrr_enabled);
        assert_eq!(d.rationale, Rationale::AlwaysOn);
        assert_eq!(d.scheduler_window_ns, MAX - MIN);
    }

    #[test]
    fn battery_beats_demand() {
        let s = support();
        let d = decide(&PolicyInputs {
            policy: VrrPolicy::Always,
            support: Some(&s),
            adaptive_demand: true,
            battery_saver: true,
        });
        assert!(!d.vrr_enabled);
        assert_eq!(d.rationale, Rationale::BatterySaver);
    }

    #[test]
    fn demand_utility() {
        assert!(mode_demands_vrr(PresentationMode::Adaptive));
        assert!(!mode_demands_vrr(PresentationMode::Vsync));
        assert!(!mode_demands_vrr(PresentationMode::Immediate));
    }
}
