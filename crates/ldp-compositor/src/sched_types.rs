//! The scheduler's public vocabulary: emissions, policy, and the
//! per-surface bookkeeping types.
//!
//! These types are the wire-facing surface of
//! [`FrameScheduler`](crate::scheduler::FrameScheduler) — the events it
//! emits (mappable 1:1 onto `ldp.core.surface`'s `frame_target`,
//! `presented`, and `frame_dropped`), the policy knobs
//! (`SchedulerConfig`, validated at construction), and the input
//! vocabulary the replay harness records (`SchedInput`).

use ldp_core::time::{FrameDeadline, FrameDropReason, Mono, PresentationMode, PresentationTiming};

use crate::semantics::SceneProfile;
use crate::surface::SurfaceId;

/// Scheduler emission, in the order the driver should dispatch them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SchedEvent {
    /// Reply to a frame registration: the deadline contract
    /// (`surface.frame_target` on the wire).
    FrameTarget {
        /// Surface the registration belongs to.
        surface: SurfaceId,
        /// Client-chosen frame identifier.
        frame: u64,
        /// The contract itself.
        deadline: FrameDeadline,
    },
    /// A registered frame reached the display (`surface.presented`).
    Presented {
        /// Surface that presented.
        surface: SurfaceId,
        /// Presentation feedback.
        timing: PresentationTiming,
    },
    /// A registered frame did not reach the display (`surface.frame_dropped`).
    FrameDropped {
        /// Surface that was dropped.
        surface: SurfaceId,
        /// The dropped frame identifier.
        frame: u64,
        /// Why.
        reason: FrameDropReason,
    },
}

/// Policy knobs for one output's scheduler.
///
/// Defaults model a typical 60 Hz direct-render pipeline; every knob is
/// exercised by the golden suite, which pins the exact behavior.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SchedulerConfig {
    /// Estimated cost of submitting the atomic commit (ns).
    pub submit_cost_ns: u64,
    /// Flip programming-to-scanout latency (ns).
    pub flip_latency_ns: u64,
    /// Remaining budget below which a new registration retargets the
    /// next vblank instead of a doomed current one (ns).
    pub min_commit_lead_ns: u64,
    /// How late a flip report may arrive after its predicted vblank and
    /// still count as that vblank (ns) — absorbs timestamp jitter.
    pub arrival_slack_ns: u64,
    /// Vsync pipeline depth: vblanks of latency (`depth >= 1`).
    pub vsync_depth: u32,
    /// Misses required before the escalation ladder adds one lead step.
    pub escalate_after: u32,
    /// Cap on extra lead steps from escalation.
    pub max_extra_lead: u32,
    /// Consecutive presentations required to de-escalate one step.
    pub deescalate_hits: u32,
    /// Adaptive-mode deadline widening (ns); zero disables the window
    /// until the Phase 15 VRR policy engine supplies real bounds.
    pub vrr_window_ns: u64,
    /// The VRR window's minimum period (ns; 0 = unprobed). With
    /// `vrr_window_ns > 0` this arms the presentation clock's
    /// window-aware measurement band at construction (Phase 39): the
    /// min side is the LFC fast end — without it the band stays legacy
    /// (the honest degradation for a partially probed window).
    pub vrr_min_ns: u64,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            submit_cost_ns: 400_000,
            flip_latency_ns: 600_000,
            min_commit_lead_ns: 500_000,
            arrival_slack_ns: 250_000,
            vsync_depth: 1,
            escalate_after: 1,
            max_extra_lead: 2,
            deescalate_hits: 4,
            vrr_window_ns: 0,
            vrr_min_ns: 0,
        }
    }
}

/// A config that cannot describe a working pipeline.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ConfigError;

impl SchedulerConfig {
    /// Validate the policy shape (field sanity only; cross-checks against
    /// a real refresh interval belong to the embedder, who knows the
    /// output mode).
    ///
    /// # Errors
    /// [`ConfigError`] when any structural invariant is violated (zero
    /// depths/streaks, or an out-of-range lead cap).
    pub const fn validate(&self) -> Result<(), ConfigError> {
        if self.vsync_depth == 0
            || self.escalate_after == 0
            || self.deescalate_hits == 0
            || self.max_extra_lead > 16
        {
            return Err(ConfigError);
        }
        Ok(())
    }
}

/// One live frame registration on a surface.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Registration {
    pub(crate) frame: u64,
    pub(crate) mode: PresentationMode,
    /// Predicted time of the targeted vblank.
    pub(crate) target_vblank: Mono,
    /// Deadline: `target_vblank - submit_cost - flip_latency`.
    pub(crate) deadline: Mono,
    /// When the window is considered closed (the deadline, widened by
    /// the adaptive window). `Mono::MAX` for immediate mode (no gate).
    pub(crate) expiry: Mono,
    /// A commit arrived in time; presents at the target flip.
    pub(crate) satisfied: bool,
    /// Reply deferred because the timeline is not live (pre-first-flip
    /// or parked); answered when it goes live.
    pub(crate) pending_reply: bool,
}

/// Per-surface scheduling state.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) struct SurfaceSlot {
    pub(crate) mode: PresentationMode,
    pub(crate) hidden: bool,
    /// Phase 47 — the semantic scene profile: the per-surface frame
    /// budget doctrine. `Desktop` (the default) is the identity — the
    /// operator's configured policy stands, byte-identical with the
    /// pre-Phase-47 scheduler; `Creative`/`Gaming` floor the
    /// admission predicate at the profile's budget
    /// ([`SceneProfile::budget_ns`]). A live registration keeps the
    /// contract it was answered with (the `set_mode` doctrine: the
    /// profile affects *future* targets).
    pub(crate) profile: SceneProfile,
    pub(crate) registration: Option<Registration>,
    pub(crate) miss_streak: u32,
    pub(crate) hit_streak: u32,
}

impl SurfaceSlot {
    pub(crate) fn extra_lead(&self, config: &SchedulerConfig) -> u32 {
        self.miss_streak
            .saturating_div(config.escalate_after)
            .min(config.max_extra_lead)
    }
}

/// One timestamped scheduler input, as recorded by the replay harness.
///
/// The driver vocabulary: every input carries its own timestamp and the
/// stream must be non-decreasing (the scheduler asserts it).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SchedInput {
    /// A page-flip / vblank observation.
    Flip {
        /// Flip completion time.
        ts: Mono,
    },
    /// A client frame registration.
    FrameRequest {
        /// Surface.
        surface: SurfaceId,
        /// Client-chosen frame ID.
        frame: u64,
        /// Request time.
        ts: Mono,
    },
    /// A client commit.
    Commit {
        /// Surface.
        surface: SurfaceId,
        /// Arrival time.
        ts: Mono,
    },
    /// A visibility change.
    SetVisibility {
        /// Surface.
        surface: SurfaceId,
        /// Now hidden?
        hidden: bool,
        /// Change time.
        ts: Mono,
    },
    /// A presentation-mode change.
    SetMode {
        /// Surface.
        surface: SurfaceId,
        /// New mode.
        mode: PresentationMode,
        /// Change time.
        ts: Mono,
    },
    /// A scene-profile change (Phase 47): the surface's frame-budget
    /// doctrine. Affects *future* registrations — a live one keeps
    /// the contract it was answered with.
    SetProfile {
        /// Surface.
        surface: SurfaceId,
        /// New profile.
        profile: SceneProfile,
        /// Change time.
        ts: Mono,
    },
    /// The output parked.
    Park {
        /// Park time.
        ts: Mono,
    },
    /// The output resumed.
    Resume {
        /// Resume time.
        ts: Mono,
    },
}

impl SchedInput {
    /// The input's timestamp.
    pub(crate) fn ts(&self) -> Mono {
        match *self {
            Self::Flip { ts }
            | Self::FrameRequest { ts, .. }
            | Self::Commit { ts, .. }
            | Self::SetVisibility { ts, .. }
            | Self::SetMode { ts, .. }
            | Self::SetProfile { ts, .. }
            | Self::Park { ts }
            | Self::Resume { ts } => ts,
        }
    }
}

/// How many extra vblank steps a frame-target search may walk when the
/// remaining budget is below the minimum lead (bounded so a pathological
/// refresh can never loop).
pub(crate) const LEAD_WALK_LIMIT: u32 = 3;

/// The admission floor one registration's walk demands: the
/// operator's configured minimum commit lead, raised by the surface's
/// scene-profile budget when the profile claims one (Phase 47). A
/// `Desktop` surface (or a pre-Phase-47 caller) sees the config
/// verbatim — the identity that keeps every golden byte-pinned.
fn admission_floor(config: &SchedulerConfig, profile: SceneProfile) -> u64 {
    config.min_commit_lead_ns.max(profile.budget_ns())
}

/// Construct a registration and its `frame_target` reply for a live
/// timeline (the pure half of the scheduler's target selection).
///
/// Walks the predicted vblank grid out to `depth + extra` lead (bumping
/// further while the remaining budget is below the admission floor —
/// the config's minimum commit lead raised by the surface's Phase-47
/// scene-profile budget, bounded by [`LEAD_WALK_LIMIT`]); the deadline
/// is `target_vblank - submit_cost - flip_latency`, widened into an
/// expiry window for adaptive mode and unbounded for immediate mode.
///
/// Phase 39 — the adaptive opportunity: an adaptive-mode surface under
/// a *fully probed* VRR window (`vrr_window_ns > 0 && vrr_min_ns > 0`)
/// targets the panel's next refresh **opportunity** — the earliest
/// in-window landing — instead of a nominal grid cell. The panel under
/// VRR is event-driven: a flip lands at the earliest legal instant
/// after the previous landing, so a nominal cell may already sit
/// *behind* the flip that carries the content. Targeting the
/// opportunity makes the verdict fire at the content's own flip
/// (`presented_at` = the honest landing, `refresh` = the cadence the
/// panel actually ran), and the deadlines pace at the panel's fast end
/// while the client keeps up — the frame pacing follows the content,
/// which is the point of adaptive sync. An unprobed window (min side
/// missing) keeps the nominal walk — the honest degradation, never a
/// guess.
pub(crate) fn sched_build_registration(
    clock: &crate::predictor::FrameClock,
    config: &SchedulerConfig,
    ts: Mono,
    frame: u64,
    mode: PresentationMode,
    extra: u32,
    profile: SceneProfile,
) -> (Registration, FrameDeadline) {
    if mode == PresentationMode::Adaptive && config.vrr_window_ns > 0 && config.vrr_min_ns > 0 {
        return sched_build_adaptive(clock, config, ts, frame, extra, profile);
    }
    let depth = if mode == PresentationMode::Immediate {
        1
    } else {
        config.vsync_depth
    };
    let lead = depth + extra;
    let mut chosen: Option<(Mono, Mono)> = None;
    for bump in 0..=LEAD_WALK_LIMIT {
        let target = clock.nth_vblank(ts, lead + bump).expect("timeline is live");
        if mode == PresentationMode::Immediate {
            chosen = Some((target, ts));
            break;
        }
        let costs = config.submit_cost_ns + config.flip_latency_ns;
        let deadline = Mono::from_ns(target.as_ns().saturating_sub(costs));
        let budget = deadline.as_ns().saturating_sub(ts.as_ns());
        if budget >= admission_floor(config, profile) || bump == LEAD_WALK_LIMIT {
            chosen = Some((target, deadline));
            break;
        }
    }
    let (target_vblank, deadline) = chosen.expect("the walk always terminates");
    let expiry = match mode {
        PresentationMode::Adaptive => deadline.saturating_add_ns(config.vrr_window_ns),
        PresentationMode::Immediate => Mono::from_ns(u64::MAX),
        _ => deadline,
    };
    let reg = Registration {
        frame,
        mode,
        target_vblank,
        deadline,
        expiry,
        satisfied: false,
        pending_reply: false,
    };
    let reply = FrameDeadline {
        deadline,
        target_vblank,
        refresh: clock.measured(),
        budget_ns: deadline.as_ns().saturating_sub(ts.as_ns()),
        mode,
    };
    (reg, reply)
}

/// The adaptive-opportunity registration (Phase 39): the target is
/// the panel's next refresh opportunity under the probed VRR window.
///
/// Model: under VRR the panel is event-driven — a flip lands at the
/// earliest legal instant `max(last_landing + min, submit)` (each
/// refresh at least `min` after the previous, the flip clamped into
/// the window). The `lead`-th refresh opportunity therefore arrives
/// no earlier than `last_landing + lead * min`, and no earlier than
/// the request itself. The walk bumps one `min` step at a time while
/// the deadline budget is too thin (the same bounded escalation as
/// the nominal walk), the deadline is the opportunity minus the
/// pipeline costs, and the expiry widens by the window (a late
/// commit may consume the stretch — Phase 31's doctrine, kept).
fn sched_build_adaptive(
    clock: &crate::predictor::FrameClock,
    config: &SchedulerConfig,
    ts: Mono,
    frame: u64,
    extra: u32,
    profile: SceneProfile,
) -> (Registration, FrameDeadline) {
    let mode = PresentationMode::Adaptive;
    let floor = admission_floor(config, profile);
    let lead = config.vsync_depth.max(1) + extra;
    let min_ns = config.vrr_min_ns;
    let costs = config.submit_cost_ns + config.flip_latency_ns;
    // The earliest instant the lead-th refresh can arrive: refreshes
    // sit at least `min` apart, so `last_landing + lead * min`; a
    // request arriving later pushes the opportunity past itself.
    let anchor_ns = clock.last_flip_ns();
    let base_ns = anchor_ns
        .saturating_add(u64::from(lead) * min_ns)
        .max(ts.as_ns());
    let mut target_ns = base_ns;
    let mut bump: u32 = 0;
    while bump < LEAD_WALK_LIMIT {
        let budget = target_ns.saturating_sub(costs).saturating_sub(ts.as_ns());
        if budget >= floor {
            break;
        }
        target_ns = target_ns.saturating_add(min_ns);
        bump += 1;
    }
    let target_vblank = Mono::from_ns(target_ns);
    let deadline = Mono::from_ns(target_ns.saturating_sub(costs));
    let expiry = deadline.saturating_add_ns(config.vrr_window_ns);
    let reg = Registration {
        frame,
        mode,
        target_vblank,
        deadline,
        expiry,
        satisfied: false,
        pending_reply: false,
    };
    let reply = FrameDeadline {
        deadline,
        target_vblank,
        refresh: clock.measured(),
        budget_ns: deadline.as_ns().saturating_sub(ts.as_ns()),
        mode,
    };
    (reg, reply)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_validation() {
        assert_eq!(SchedulerConfig::default().validate(), Ok(()));
        for bad in [
            SchedulerConfig {
                vsync_depth: 0,
                ..SchedulerConfig::default()
            },
            SchedulerConfig {
                escalate_after: 0,
                ..SchedulerConfig::default()
            },
            SchedulerConfig {
                deescalate_hits: 0,
                ..SchedulerConfig::default()
            },
            SchedulerConfig {
                max_extra_lead: 17,
                ..SchedulerConfig::default()
            },
        ] {
            assert_eq!(bad.validate(), Err(ConfigError));
        }
    }

    #[test]
    fn extra_lead_ladder_math() {
        let config = SchedulerConfig {
            escalate_after: 2,
            max_extra_lead: 3,
            ..SchedulerConfig::default()
        };
        let slot = SurfaceSlot {
            miss_streak: 0,
            ..SurfaceSlot::default()
        };
        assert_eq!(slot.extra_lead(&config), 0);
        let slot = SurfaceSlot {
            miss_streak: 1,
            ..SurfaceSlot::default()
        };
        assert_eq!(slot.extra_lead(&config), 0); // below escalate_after
        let slot = SurfaceSlot {
            miss_streak: 5,
            ..SurfaceSlot::default()
        };
        assert_eq!(slot.extra_lead(&config), 2); // 5/2, capped at 3
        let slot = SurfaceSlot {
            miss_streak: 99,
            ..SurfaceSlot::default()
        };
        assert_eq!(slot.extra_lead(&config), 3); // cap
    }

    // -- Phase 39: the adaptive opportunity ------------------------------

    /// The mock's 48-144 Hz window over a 60 Hz nominal.
    const VRR_MIN: u64 = 6_944_444;
    const VRR_MAX: u64 = 20_833_333;
    const NOMINAL: u64 = 16_666_666;

    fn adaptive_config() -> SchedulerConfig {
        SchedulerConfig {
            vrr_window_ns: VRR_MAX - NOMINAL,
            vrr_min_ns: VRR_MIN,
            ..SchedulerConfig::default()
        }
    }

    fn anchored(nominal: u64, landing: u64) -> crate::predictor::FrameClock {
        let mut clock = crate::predictor::FrameClock::new(
            ldp_core::time::RefreshInterval::from_ns(nominal).unwrap(),
        );
        clock.observe_flip(Mono::from_ns(landing));
        clock
    }

    #[test]
    fn adaptive_registration_targets_the_refresh_opportunity() {
        // Depth 1, anchored at the nominal cell: the target is the
        // next refresh opportunity (the last landing plus one min),
        // the deadline sits the pipeline costs inside it, and the
        // expiry widens by the window (Phase 31's late-commit stretch).
        let clock = anchored(NOMINAL, NOMINAL);
        let config = adaptive_config();
        let (reg, reply) = sched_build_registration(
            &clock,
            &config,
            Mono::from_ns(NOMINAL),
            1,
            PresentationMode::Adaptive,
            0,
            SceneProfile::Desktop,
        );
        assert_eq!(reg.target_vblank.as_ns(), NOMINAL + VRR_MIN);
        assert_eq!(reg.deadline.as_ns(), NOMINAL + VRR_MIN - 1_000_000);
        assert_eq!(
            reg.expiry.as_ns(),
            NOMINAL + VRR_MIN - 1_000_000 + (VRR_MAX - NOMINAL)
        );
        assert_eq!(reply.target_vblank, reg.target_vblank);
        assert_eq!(reply.deadline, reg.deadline);
        assert_eq!(reply.mode, PresentationMode::Adaptive);
    }

    #[test]
    fn adaptive_registration_bumps_when_the_budget_is_thin() {
        // A request arriving just before the opportunity leaves a
        // budget thinner than the minimum commit lead: the walk bumps
        // exactly one min step (the bounded escalation).
        let clock = anchored(NOMINAL, NOMINAL);
        let config = adaptive_config();
        let ts = NOMINAL + VRR_MIN - 700_000;
        let (reg, _reply) = sched_build_registration(
            &clock,
            &config,
            Mono::from_ns(ts),
            1,
            PresentationMode::Adaptive,
            0,
            SceneProfile::Desktop,
        );
        // The first candidate's budget is 300 us < 500 us: bumped once.
        assert_eq!(reg.target_vblank.as_ns(), NOMINAL + 2 * VRR_MIN);
        assert!(reg.deadline.as_ns() >= ts, "the deadline is makeable");
    }

    #[test]
    fn adaptive_registration_floors_at_the_request() {
        // A request arriving mid-window (past the base opportunity):
        // the panel cannot land a flip before its submission, so the
        // opportunity floors at the request itself (and the thin
        // budget bumps, bounded by the walk limit).
        let clock = anchored(NOMINAL, NOMINAL);
        let config = adaptive_config();
        let ts = NOMINAL + 10_000_000; // 26_666_666, past the base opportunity
        let (reg, _reply) = sched_build_registration(
            &clock,
            &config,
            Mono::from_ns(ts),
            1,
            PresentationMode::Adaptive,
            0,
            SceneProfile::Desktop,
        );
        // The first candidate (the request itself) leaves a saturated
        // zero budget: one min bump restores a makeable deadline.
        assert_eq!(reg.target_vblank.as_ns(), ts + VRR_MIN);
        assert!(reg.deadline.as_ns() >= ts, "the deadline is makeable");
    }

    #[test]
    fn adaptive_mode_without_a_probed_window_stays_nominal() {
        // An adaptive surface with no window (the default config) or a
        // half-probed one (max side only) keeps the nominal walk —
        // Phase 31's behavior: nominal targets, the widened expiry.
        let clock = anchored(NOMINAL, NOMINAL);
        let config = SchedulerConfig::default();
        let (reg, _) = sched_build_registration(
            &clock,
            &config,
            Mono::from_ns(NOMINAL),
            1,
            PresentationMode::Adaptive,
            0,
            SceneProfile::Desktop,
        );
        assert_eq!(
            reg.target_vblank,
            clock.nth_vblank(Mono::from_ns(NOMINAL), 1).unwrap()
        );
        let half_probed = SchedulerConfig {
            vrr_window_ns: VRR_MAX - NOMINAL,
            vrr_min_ns: 0,
            ..SchedulerConfig::default()
        };
        let (reg2, _) = sched_build_registration(
            &clock,
            &half_probed,
            Mono::from_ns(NOMINAL),
            1,
            PresentationMode::Adaptive,
            0,
            SceneProfile::Desktop,
        );
        assert_eq!(
            reg2.target_vblank, reg.target_vblank,
            "the min side gates the opportunity path"
        );
    }
}
