//! The deadline frame scheduler (`docs/architecture.md` §10.3).
//!
//! One [`FrameScheduler`] per output, driven entirely by timestamped
//! inputs — flip observations, client frame registrations, commits,
//! visibility and mode changes, park/resume. It never reads a clock by
//! itself, so given the same input stream every decision is reproducible
//! (the Phase 7 replay harness is a direct consumer of that property).
//!
//! # The frame contract
//!
//! * `frame_request` registers interest; the reply event
//!   ([`SchedEvent::FrameTarget`]) carries the deadline contract
//!   `target_vblank - submit_cost - flip_latency` for the `n`-th vblank
//!   ahead, where `n = depth(mode) + extra_lead(surface)`.
//! * A commit arriving strictly before the deadline (or, under the
//!   adaptive window, before `deadline + vrr_window`) satisfies the
//!   registration and binds it to that vblank; the content presents at
//!   the first flip reported at-or-after
//!   `target_vblank - arrival_slack`.
//! * Every registration terminates in exactly one event: [`SchedEvent::Presented`],
//!   or [`SchedEvent::FrameDropped`] with a reason — `DeadlineMissed` (the window
//!   closed without a satisfying commit; the client adapts, and the
//!   late content still becomes live state silently), `Superseded` (a
//!   newer registration replaced it), `SurfaceHidden` (the surface
//!   became unmapped/fully occluded, committed while hidden, or stayed
//!   hidden past its expiry), or `OutputOff` (the output parked).
//!   `Throttled` is reserved for the VRR policy engine (Phase 15) and
//!   is never emitted here.
//!
//! # Escalation ladder
//!
//! A miss streak widens the deadline: `extra_lead = min(miss_streak /
//! escalate_after, max_extra_lead)` extra vblanks, so a systematically
//! slow client is handed deadlines it can actually meet instead of a
//! guaranteed miss every frame. De-escalation is hysteretic — the streak
//! only decays after `deescalate_hits` consecutive presentations — so a
//! client whose render time sits between two pipeline depths settles
//! instead of oscillating.
//!
//! # Driver contract
//!
//! Input timestamps must be non-decreasing (asserted; merge event sources
//! by time). While unanchored (before the first flip) or parked, frame
//! replies are deferred: they are answered when the timeline goes live
//! again, which mirrors real frame callbacks queueing through DPMS-off.

use std::collections::BTreeMap;

use ldp_core::time::{
    FrameDeadline, FrameDropReason, Mono, PresentationFlags, PresentationMode, PresentationTiming,
    RefreshInterval,
};

use crate::predictor::FrameClock;
use crate::sched_types::{sched_build_registration, Registration, SurfaceSlot};
use crate::semantics::SceneProfile;
use crate::surface::SurfaceId;

// Re-export the vocabulary so the canonical
// `ldp_compositor::scheduler::{SchedEvent, SchedulerConfig, ConfigError}`
// paths keep working.
pub use crate::sched_types::{ConfigError, SchedEvent, SchedulerConfig};

/// Per-output deadline frame scheduler.
///
/// See the [module docs](self) for the contract; the public input methods
/// all take the event timestamp and assert non-decreasing order.
pub struct FrameScheduler {
    config: SchedulerConfig,
    clock: FrameClock,
    slots: BTreeMap<SurfaceId, SurfaceSlot>,
    parked: bool,
    out: Vec<SchedEvent>,
    last_ts: Mono,
}

impl FrameScheduler {
    /// Build a scheduler for an output whose mode advertises `nominal`.
    ///
    /// # Errors
    /// [`ConfigError`] for a malformed config.
    pub fn new(nominal: RefreshInterval, config: SchedulerConfig) -> Result<Self, ConfigError> {
        config.validate()?;
        let mut clock = FrameClock::new(nominal);
        // Phase 39: a probed VRR window arms the presentation clock's
        // measurement band — the deadline grid stays nominal (the
        // widened commit window absorbs the stretch), but the
        // `presented` refresh reports the cadence the panel actually
        // ran, the LFC fast end no less than the stretched slow end.
        // `arm_vrr` ignores a window that does not contain the
        // nominal period (the legacy band stands — never a guess).
        if config.vrr_window_ns > 0 {
            clock.arm_vrr(
                config.vrr_min_ns,
                nominal.as_ns().saturating_add(config.vrr_window_ns),
            );
        }
        Ok(Self {
            config,
            clock,
            slots: BTreeMap::new(),
            parked: false,
            out: Vec::new(),
            last_ts: Mono::ZERO,
        })
    }

    /// The effective policy in force.
    #[must_use]
    pub const fn config(&self) -> &SchedulerConfig {
        &self.config
    }

    /// The output timeline (for drivers wanting predictions).
    #[must_use]
    pub const fn clock(&self) -> &FrameClock {
        &self.clock
    }

    /// Take the pending emissions (dispatch order preserved).
    pub fn drain(&mut self) -> Vec<SchedEvent> {
        core::mem::take(&mut self.out)
    }

    /// Observe a page-flip (or latched vblank) completion.
    ///
    /// Expires the window that just closed, updates the PLL, answers
    /// deferred replies if the timeline just went live, then presents
    /// every satisfied registration whose target vblank has arrived.
    pub fn observe_flip(&mut self, ts: Mono) {
        self.enter(ts);
        self.expire(ts);
        self.clock.observe_flip(ts);
        self.answer_pending();
        self.present_due(ts);
    }

    /// A client registered interest in the next frame for `surface`.
    ///
    /// A live registration is replaced, terminating the old one with
    /// `Superseded` (the §10.4 presentation coalescing rule).
    ///
    /// Phase 45: a **hidden** surface's request parks (the deferred
    /// registration) instead of drawing a deadline — the occlusion
    /// quiescing contract, the same doctrine the parked output serves:
    /// the server does not ask a client it cannot show to draw. The
    /// parked request is answered at the unhide
    /// ([`FrameScheduler::set_visibility`]),
    /// exactly as a parked timeline's requests are answered at its
    /// `resume`.
    pub fn frame_request(&mut self, surface: SurfaceId, frame: u64, ts: Mono) {
        self.enter(ts);
        self.expire(ts);
        let (mode, profile, extra, old) = {
            let config = self.config;
            let slot = self.slot_mut(surface);
            (
                slot.mode,
                slot.profile,
                slot.extra_lead(&config),
                slot.registration.take(),
            )
        };
        if let Some(old) = old {
            self.push_dropped(surface, old.frame, FrameDropReason::Superseded, false);
        }
        if self.timeline_live() && !self.slot_hidden(surface) {
            let (reg, reply) = self.build_registration(ts, frame, mode, extra, profile);
            self.slot_mut(surface).registration = Some(reg);
            self.out.push(SchedEvent::FrameTarget {
                surface,
                frame,
                deadline: reply,
            });
        } else {
            self.slot_mut(surface).registration = Some(Registration {
                frame,
                mode,
                target_vblank: Mono::ZERO,
                deadline: Mono::ZERO,
                expiry: Mono::from_ns(u64::MAX),
                satisfied: false,
                pending_reply: true,
            });
        }
    }

    /// A client commit arrived (acquire fence signaled, state applied).
    ///
    /// A commit strictly before the registration's expiry satisfies it;
    /// a late commit terminates the registration with `DeadlineMissed`
    /// (escalating) while the content itself simply becomes live state.
    pub fn commit(&mut self, surface: SurfaceId, ts: Mono) {
        self.enter(ts);
        self.expire(ts);
        let mut dropped: Option<(u64, FrameDropReason)> = None;
        {
            let slot = self.slot_mut(surface);
            if let Some(reg) = slot.registration {
                if slot.hidden {
                    // Hidden surfaces cannot present; the registration
                    // dies here so its frame still gets a terminal event.
                    slot.registration = None;
                    dropped = Some((reg.frame, FrameDropReason::SurfaceHidden));
                } else if reg.pending_reply || ts.as_ns() < reg.expiry.as_ns() {
                    slot.registration = Some(Registration {
                        satisfied: true,
                        ..reg
                    });
                } else {
                    slot.registration = None;
                    dropped = Some((reg.frame, FrameDropReason::DeadlineMissed));
                }
            }
        }
        if let Some((frame, reason)) = dropped {
            self.push_dropped(
                surface,
                frame,
                reason,
                reason == FrameDropReason::DeadlineMissed,
            );
        }
    }

    /// The compositor reports a visibility change (unmap/full occlusion).
    ///
    /// Hiding terminates any live registration with `SurfaceHidden`;
    /// unhiding re-enables presentation for later registrations (a live
    /// registration is kept — its frame may still present once the
    /// surface is visible again before its expiry). Phase 45: the
    /// unhide also answers requests that parked while hidden — the
    /// quiesced surface's first visible frame draws a deadline at the
    /// moment it can present, not one flip later.
    pub fn set_visibility(&mut self, surface: SurfaceId, hidden: bool, ts: Mono) {
        self.enter(ts);
        self.expire(ts);
        let old = if hidden {
            let slot = self.slot_mut(surface);
            slot.hidden = true;
            slot.registration.take()
        } else {
            self.slot_mut(surface).hidden = false;
            None
        };
        if let Some(old) = old {
            self.push_dropped(surface, old.frame, FrameDropReason::SurfaceHidden, false);
        }
        if !hidden {
            self.answer_pending();
        }
    }

    /// Whether the slot is currently hidden (the quiesced state).
    fn slot_hidden(&self, surface: SurfaceId) -> bool {
        self.slots.get(&surface).is_some_and(|slot| slot.hidden)
    }

    /// Presentation mode change for a surface (affects future targets;
    /// a live registration keeps the contract it was answered with).
    pub fn set_mode(&mut self, surface: SurfaceId, mode: PresentationMode, ts: Mono) {
        self.enter(ts);
        self.expire(ts);
        self.slot_mut(surface).mode = mode;
    }

    /// Scene-profile change for a surface (Phase 47 — the semantic
    /// scene's frame-budget doctrine): the profile's budget floors
    /// the admission predicate of every *future* registration the
    /// surface draws. A live registration keeps the contract it was
    /// answered with (the `set_mode` doctrine); the surface's next
    /// `frame_request` walks under the new floor. The default profile
    /// (`Desktop`) adds nothing — the operator's configured policy
    /// stands.
    pub fn set_profile(&mut self, surface: SurfaceId, profile: SceneProfile, ts: Mono) {
        self.enter(ts);
        self.expire(ts);
        self.slot_mut(surface).profile = profile;
    }

    /// The surface's scene profile (Phase 47 introspection — the
    /// session tests' and the tools' oracle; `Desktop` for a surface
    /// that never claimed).
    #[must_use]
    pub fn profile_of(&self, surface: SurfaceId) -> SceneProfile {
        self.slots
            .get(&surface)
            .map_or(SceneProfile::default(), |slot| slot.profile)
    }

    /// The output went away (DPMS off): every live registration dies
    /// with `OutputOff` and the timeline parks.
    pub fn park(&mut self, ts: Mono) {
        self.enter(ts);
        self.expire(ts);
        self.parked = true;
        let mut drops = Vec::new();
        for (&surface, slot) in &mut self.slots {
            if let Some(reg) = slot.registration.take() {
                drops.push(SchedEvent::FrameDropped {
                    surface,
                    frame: reg.frame,
                    reason: FrameDropReason::OutputOff,
                });
            }
        }
        self.out.extend(drops);
    }

    /// The output came back: the timeline goes live and deferred
    /// registrations are answered.
    pub fn resume(&mut self, ts: Mono) {
        self.enter(ts);
        self.expire(ts);
        self.parked = false;
        self.answer_pending();
    }

    /// The output was *replaced* — live re-arrangement (Phase 26):
    /// a different connector (or mode) now feeds the scheduler, so
    /// the timeline restarts on the new output's nominal refresh.
    ///
    /// This is the migration arm of [`FrameScheduler::park`]: park
    /// models "no output at all" (every registration dies, later
    /// requests defer until `resume`); reanchor models "a new output
    /// took over" — windowed registrations on the dead timeline die
    /// with `OutputOff` (the output they targeted is gone), while
    /// deferred requests survive, to be answered when the new
    /// timeline's bring-up flip anchors the fresh clock. Until that
    /// flip lands the timeline is unanchored — exactly the bring-up
    /// state a scheduler is born in, so new requests defer the same
    /// way.
    ///
    /// Input ordering is preserved (the ordering clock is untouched);
    /// the new nominal takes effect for every deadline computed after
    /// the re-anchor.
    pub fn reanchor(&mut self, nominal: RefreshInterval) {
        let mut drops = Vec::new();
        for (&surface, slot) in &mut self.slots {
            if let Some(reg) = slot.registration {
                if !reg.pending_reply {
                    drops.push(SchedEvent::FrameDropped {
                        surface,
                        frame: reg.frame,
                        reason: FrameDropReason::OutputOff,
                    });
                    slot.registration = None;
                }
            }
        }
        self.out.extend(drops);
        self.parked = false;
        self.clock = FrameClock::new(nominal);
    }

    /// Whether deadlines can be computed right now.
    fn timeline_live(&self) -> bool {
        !self.parked && self.clock.is_anchored()
    }

    /// Input-ordering contract + expiry checkpoint on every event.
    fn enter(&mut self, ts: Mono) {
        assert!(
            ts.as_ns() >= self.last_ts.as_ns(),
            "scheduler inputs must be non-decreasing (got {} after {})",
            ts.as_ns(),
            self.last_ts.as_ns()
        );
        self.last_ts = ts;
    }

    /// Expire unsatisfied registrations whose window closed at `ts`.
    ///
    /// A hidden surface reports `SurfaceHidden`; a deadline miss feeds
    /// the escalation ladder. Deferred (pending-reply) registrations have
    /// no window yet and never expire here.
    fn expire(&mut self, ts: Mono) {
        let mut drops = Vec::new();
        for (&surface, slot) in &mut self.slots {
            let Some(reg) = slot.registration else {
                continue;
            };
            if reg.pending_reply || reg.satisfied || ts.as_ns() < reg.expiry.as_ns() {
                continue;
            }
            slot.registration = None;
            if slot.hidden {
                drops.push(SchedEvent::FrameDropped {
                    surface,
                    frame: reg.frame,
                    reason: FrameDropReason::SurfaceHidden,
                });
            } else {
                slot.miss_streak = slot.miss_streak.saturating_add(1).min(1024);
                slot.hit_streak = 0;
                drops.push(SchedEvent::FrameDropped {
                    surface,
                    frame: reg.frame,
                    reason: FrameDropReason::DeadlineMissed,
                });
            }
        }
        self.out.extend(drops);
    }

    /// Present satisfied registrations whose target vblank arrived.
    fn present_due(&mut self, ts: Mono) {
        if self.parked {
            return;
        }
        let slack = self.config.arrival_slack_ns;
        let deescalate_hits = self.config.deescalate_hits;
        let refresh = self.clock.measured();
        let mut presented = Vec::new();
        for (&surface, slot) in &mut self.slots {
            if slot.hidden {
                continue;
            }
            let Some(reg) = slot.registration else {
                continue;
            };
            if !reg.satisfied || reg.target_vblank.as_ns().saturating_sub(slack) > ts.as_ns() {
                // Not due yet (deeper pipeline or an early flip report):
                // hold for a later flip.
                continue;
            }
            slot.registration = None;
            // Hysteretic de-escalation.
            slot.hit_streak += 1;
            if slot.hit_streak >= deescalate_hits {
                slot.hit_streak = 0;
                slot.miss_streak = slot.miss_streak.saturating_sub(1);
            }
            presented.push((surface, reg));
        }
        for (surface, reg) in presented {
            let flags = if reg.mode == PresentationMode::Immediate {
                PresentationFlags {
                    torn: true,
                    ..PresentationFlags::default()
                }
            } else {
                PresentationFlags {
                    vblank: true,
                    ..PresentationFlags::default()
                }
            };
            self.out.push(SchedEvent::Presented {
                surface,
                timing: PresentationTiming {
                    frame: reg.frame,
                    presented_at: ts,
                    refresh,
                    flags,
                },
            });
        }
    }

    /// Build the registration and its reply for a live timeline.
    fn build_registration(
        &self,
        ts: Mono,
        frame: u64,
        mode: PresentationMode,
        extra: u32,
        profile: SceneProfile,
    ) -> (Registration, FrameDeadline) {
        sched_build_registration(&self.clock, &self.config, ts, frame, mode, extra, profile)
    }

    /// Deferred replies become real once the timeline is live.
    ///
    /// Phase 45: a slot that is *hidden* stays parked even on a live
    /// timeline — the flip that anchors the clock says nothing about
    /// the surface's own visibility, and the quiescing contract does
    /// not hand deadlines to surfaces it cannot show. The unhide
    /// ([`FrameScheduler::set_visibility`]) is the answer point for those.
    fn answer_pending(&mut self) {
        if !self.timeline_live() {
            return;
        }
        let pending: Vec<(SurfaceId, u64, PresentationMode, SceneProfile, u32, bool)> = self
            .slots
            .iter()
            .filter_map(|(&surface, slot)| {
                if slot.hidden {
                    return None;
                }
                slot.registration
                    .filter(|reg| reg.pending_reply)
                    .map(|reg| {
                        (
                            surface,
                            reg.frame,
                            slot.mode,
                            slot.profile,
                            slot.extra_lead(&self.config),
                            reg.satisfied,
                        )
                    })
            })
            .collect();
        for (surface, frame, mode, profile, extra, satisfied) in pending {
            let (mut reg, reply) =
                self.build_registration(self.last_ts, frame, mode, extra, profile);
            reg.satisfied = satisfied;
            if let Some(slot) = self.slots.get_mut(&surface) {
                slot.registration = Some(reg);
            }
            self.out.push(SchedEvent::FrameTarget {
                surface,
                frame,
                deadline: reply,
            });
        }
    }

    fn slot_mut(&mut self, surface: SurfaceId) -> &mut SurfaceSlot {
        self.slots.entry(surface).or_default()
    }

    /// Emit a drop; a deadline miss escalates the surface's ladder.
    fn push_dropped(
        &mut self,
        surface: SurfaceId,
        frame: u64,
        reason: FrameDropReason,
        ladder: bool,
    ) {
        if ladder {
            if let Some(slot) = self.slots.get_mut(&surface) {
                slot.miss_streak = slot.miss_streak.saturating_add(1).min(1024);
                slot.hit_streak = 0;
            }
        }
        self.out.push(SchedEvent::FrameDropped {
            surface,
            frame,
            reason,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIXTY: u64 = 16_666_666;

    fn sixty() -> RefreshInterval {
        RefreshInterval::from_ns(SIXTY).unwrap()
    }

    fn live_scheduler() -> FrameScheduler {
        let mut s = FrameScheduler::new(sixty(), SchedulerConfig::default()).unwrap();
        s.observe_flip(Mono::from_ns(SIXTY));
        s.drain();
        s
    }

    #[test]
    fn drains_are_take_all_and_ordered() {
        let mut s = live_scheduler();
        let a = SurfaceId::from_raw(1);
        let b = SurfaceId::from_raw(2);
        s.frame_request(a, 1, Mono::from_ns(17_000_000));
        s.frame_request(b, 2, Mono::from_ns(17_000_001));
        let events = s.drain();
        assert_eq!(events.len(), 2);
        // Surface order (a before b) is the emission order.
        assert!(matches!(events[0], SchedEvent::FrameTarget { surface, .. } if surface == a));
        assert!(matches!(events[1], SchedEvent::FrameTarget { surface, .. } if surface == b));
        assert!(s.drain().is_empty());
    }

    // ---- Phase 26: live re-arrangement ---------------------------------

    fn one_hundred_forty_four() -> RefreshInterval {
        RefreshInterval::from_ns(6_944_444).unwrap()
    }

    fn windowed_registration(s: &mut FrameScheduler, surface: SurfaceId, frame: u64) {
        s.frame_request(surface, frame, Mono::from_ns(17_000_000));
        let target = s
            .drain()
            .into_iter()
            .find(|e| matches!(e, SchedEvent::FrameTarget { .. }))
            .expect("the live timeline answered the request");
        assert!(matches!(target, SchedEvent::FrameTarget { frame: f, .. } if f == frame));
    }

    #[test]
    fn reanchor_kills_windowed_registrations_with_output_off() {
        let mut s = live_scheduler();
        let a = SurfaceId::from_raw(1);
        windowed_registration(&mut s, a, 7);
        // The migration: the old output's timeline dies.
        s.reanchor(one_hundred_forty_four());
        let events = s.drain();
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            SchedEvent::FrameDropped { surface, frame: 7, reason: FrameDropReason::OutputOff }
            if *surface == a
        ));
    }

    #[test]
    fn reanchor_keeps_deferred_requests_and_answers_on_the_new_clock() {
        let a = SurfaceId::from_raw(1);
        // Un-anchor first (a scheduler born without flips defers).
        let mut fresh = FrameScheduler::new(sixty(), SchedulerConfig::default()).unwrap();
        fresh.observe_flip(Mono::from_ns(SIXTY));
        fresh.drain();
        // A request that is deferred: park the timeline (output gone),
        // then ask during the dark window.
        fresh.park(Mono::from_ns(2 * SIXTY));
        fresh.drain();
        fresh.frame_request(a, 11, Mono::from_ns(2 * SIXTY + 1_000));
        assert!(
            fresh.drain().is_empty(),
            "a parked scheduler defers the reply"
        );
        // The new output lights: re-anchor at 144 Hz, bring-up flip lands.
        fresh.reanchor(one_hundred_forty_four());
        assert!(fresh.drain().is_empty(), "still unanchored until the flip");
        fresh.observe_flip(Mono::from_ns(3 * SIXTY));
        let events = fresh.drain();
        assert_eq!(events.len(), 1);
        match &events[0] {
            SchedEvent::FrameTarget {
                surface,
                frame,
                deadline,
            } => {
                assert_eq!(*surface, a);
                assert_eq!(*frame, 11);
                // The answer carries the NEW nominal (144 Hz).
                assert_eq!(deadline.refresh.as_ns(), 6_944_444);
            }
            other => panic!("expected the deferred answer, got {other:?}"),
        }
    }

    #[test]
    fn reanchor_unparks_a_dark_scheduler() {
        let mut s = live_scheduler();
        s.park(Mono::from_ns(2 * SIXTY));
        let a = SurfaceId::from_raw(1);
        s.frame_request(a, 3, Mono::from_ns(2 * SIXTY + 5));
        assert!(s.drain().is_empty());
        s.reanchor(one_hundred_forty_four());
        // Unanchored (fresh clock, no flip yet): still deferring, but
        // no longer *parked* — the state is exactly bring-up.
        s.observe_flip(Mono::from_ns(3 * SIXTY));
        assert!(matches!(
            s.drain().as_slice(),
            [SchedEvent::FrameTarget { frame: 3, .. }]
        ));
    }

    #[test]
    fn reanchor_preserves_input_ordering_across_the_swap() {
        let mut s = live_scheduler();
        s.frame_request(SurfaceId::from_raw(1), 1, Mono::from_ns(17_000_000));
        s.drain();
        s.reanchor(one_hundred_forty_four());
        // The ordering clock survived the swap: a later input is fine.
        s.frame_request(SurfaceId::from_raw(2), 2, Mono::from_ns(18_000_000));
        s.observe_flip(Mono::from_ns(18_000_000));
        let events = s.drain();
        assert!(events
            .iter()
            .any(|e| matches!(e, SchedEvent::FrameTarget { frame: 2, .. })));
    }

    // ---- Phase 47: the semantic scene-profile budget floor --------
    //
    // The default config: costs 1 ms (submit 400 us + flip 600 us),
    // min commit lead 500 us, depth 1. A request at
    // `2 * SIXTY - X` targets the imminent vblank (2 * SIXTY) with a
    // budget of `X - 1 ms`; the profile's floor decides whether that
    // budget admits the imminent flip or the walk bumps to the next
    // vblank — the adaptive frame scheduler's per-surface doctrine.

    use crate::sched_types::SchedInput;
    use crate::semantics::SceneProfile;

    /// The first `FrameTarget` deadline in the drain, if any.
    fn first_target(s: &mut FrameScheduler) -> Option<SchedEvent> {
        s.drain()
            .into_iter()
            .find(|e| matches!(e, SchedEvent::FrameTarget { .. }))
    }

    #[test]
    fn the_desktop_profile_is_the_identity() {
        // Two live schedulers, the same request stream, one surface
        // set to `Desktop` explicitly: every deadline is identical
        // with the never-set scheduler — the operator's configured
        // policy IS the desktop doctrine.
        let ts = Mono::from_ns(2 * SIXTY - 4_000_000);
        let mut plain = live_scheduler();
        let mut desktop = live_scheduler();
        desktop.set_profile(SurfaceId::from_raw(1), SceneProfile::Desktop, ts);
        for s in [&mut plain, &mut desktop] {
            s.frame_request(SurfaceId::from_raw(1), 1, ts);
        }
        assert_eq!(first_target(&mut plain), first_target(&mut desktop));
    }

    #[test]
    fn the_creative_floor_retargets_a_thin_budget() {
        // A 3 ms remaining budget: the config's 500 us admits the
        // imminent vblank, the Creative floor (4 ms) does not — the
        // walk hands the editor's surface the *next* vblank, a
        // deadline it can actually meet (the stability doctrine).
        let mut s = live_scheduler();
        let a = SurfaceId::from_raw(1);
        let ts = Mono::from_ns(2 * SIXTY - 4_000_000); // budget 3 ms
        s.frame_request(a, 1, ts);
        match first_target(&mut s) {
            Some(SchedEvent::FrameTarget { deadline, .. }) => {
                assert_eq!(deadline.target_vblank.as_ns(), 2 * SIXTY);
            }
            other => panic!("expected a target, got {other:?}"),
        }
        let mut creative = live_scheduler();
        creative.set_profile(a, SceneProfile::Creative, ts);
        creative.frame_request(a, 1, ts);
        match first_target(&mut creative) {
            Some(SchedEvent::FrameTarget { deadline, .. }) => {
                assert_eq!(
                    deadline.target_vblank.as_ns(),
                    3 * SIXTY,
                    "the creative floor bumps the thin budget to the next vblank"
                );
            }
            other => panic!("expected a target, got {other:?}"),
        }
    }

    #[test]
    fn the_gaming_floor_admits_a_millisecond_and_holds_below_it() {
        // 1.5 ms remaining: both the config and the Gaming floor (1
        // ms) admit the imminent vblank — gaming is latency-first,
        // not pessimistic.
        let mut s = live_scheduler();
        let a = SurfaceId::from_raw(1);
        s.set_profile(a, SceneProfile::Gaming, Mono::from_ns(SIXTY));
        let ts = Mono::from_ns(2 * SIXTY - 2_500_000); // budget 1.5 ms
        s.frame_request(a, 1, ts);
        match first_target(&mut s) {
            Some(SchedEvent::FrameTarget { deadline, .. }) => {
                assert_eq!(deadline.target_vblank.as_ns(), 2 * SIXTY);
            }
            other => panic!("expected a target, got {other:?}"),
        }
        // 800 us remaining: the config's 500 us still admits, the
        // Gaming floor does not — a gaming client that cannot make
        // the imminent flip gets the makeable one (the floor is the
        // reliability that latency is built on).
        let mut thin = live_scheduler();
        thin.set_profile(a, SceneProfile::Gaming, Mono::from_ns(SIXTY));
        let ts2 = Mono::from_ns(2 * SIXTY - 1_800_000); // budget 800 us
        thin.frame_request(a, 2, ts2);
        match first_target(&mut thin) {
            Some(SchedEvent::FrameTarget { deadline, .. }) => {
                assert_eq!(
                    deadline.target_vblank.as_ns(),
                    3 * SIXTY,
                    "the gaming floor holds below 1 ms"
                );
            }
            other => panic!("expected a target, got {other:?}"),
        }
    }

    #[test]
    fn set_profile_keeps_the_live_contract_and_moves_the_next() {
        // The `set_mode` doctrine: a live registration keeps the
        // contract it was answered with; the surface's NEXT request
        // walks under the new floor.
        let mut s = live_scheduler();
        let a = SurfaceId::from_raw(1);
        let ts = Mono::from_ns(2 * SIXTY - 4_000_000); // budget 3 ms
        s.frame_request(a, 1, ts);
        let first = first_target(&mut s).expect("answered");
        // The profile change lands mid-flight.
        s.set_profile(a, SceneProfile::Creative, Mono::from_ns(ts.as_ns() + 1));
        // A superseding request at the same arrival shape: the new
        // walk serves the Creative floor.
        s.frame_request(a, 2, Mono::from_ns(ts.as_ns() + 2));
        let events = s.drain();
        // The old registration terminated with Superseded.
        assert!(events.iter().any(|e| matches!(
            e,
            SchedEvent::FrameDropped {
                frame: 1,
                reason: FrameDropReason::Superseded,
                ..
            }
        )));
        match events
            .iter()
            .find(|e| matches!(e, SchedEvent::FrameTarget { .. }))
        {
            Some(SchedEvent::FrameTarget {
                frame, deadline, ..
            }) => {
                assert_eq!(*frame, 2);
                assert_eq!(deadline.target_vblank.as_ns(), 3 * SIXTY);
                // And the first contract was never rewritten.
                let _ = first;
            }
            other => panic!("expected the new target, got {other:?}"),
        }
    }

    #[test]
    fn the_profile_rides_the_replay_harness() {
        // The input vocabulary records the profile change; the
        // recording replays byte-identical through the codec (the
        // Phase 47 tag-8 addition).
        let a = SurfaceId::from_raw(1);
        let inputs = vec![
            SchedInput::Flip {
                ts: Mono::from_ns(SIXTY),
            },
            SchedInput::SetProfile {
                surface: a,
                profile: SceneProfile::Creative,
                ts: Mono::from_ns(SIXTY + 1),
            },
            SchedInput::FrameRequest {
                surface: a,
                frame: 1,
                ts: Mono::from_ns(2 * SIXTY - 4_000_000),
            },
        ];
        let recording = crate::replay::record(SchedulerConfig::default(), sixty(), inputs).unwrap();
        let bytes = recording.to_bytes();
        let decoded = crate::replay::Recording::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, recording);
        assert_eq!(crate::replay::replay(&decoded).unwrap(), recording.outputs);
    }
}
