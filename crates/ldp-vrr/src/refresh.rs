//! The refresh selector: the per-output latency optimizer.
//!
//! Turns "content is ready at `t`" into a flip ruling:
//!
//! * **VRR on** — [`select_flip`](crate::window::select_flip()) with
//!   flip-slip avoidance: the earliest legal slot
//!   `max(ready, anchor + min)`, bounded by `anchor + max`. A commit
//!   that arrives after the whole window has stretched past `max` has
//!   missed it; then the panel capability decides the fallback:
//!   with the `fixed_rate` bit (and deadline policy) the frame is
//!   **deferred** onto the nominal grid — the `throttled` path, with a
//!   retry hint — otherwise the flip **catches up** at readiness (the
//!   panel idled past its stretch; the next scanout starts now).
//!   Phase 41 adds the **LFC cadence** ([`lfc_interval`]): content
//!   slower than the window bridges on repeats *aligned to the
//!   content's own phase* — the giants' low-framerate-compensation
//!   arithmetic, judder-free by construction — and the **latch**
//!   ([`RefreshSelector::lfc_engaged`]): the anti-flap dwell that
//!   keeps the repeat regime warm while content hovers at the
//!   boundary instead of alternating stretch/repeat (the visible
//!   pumping the driver quirk tables name `lfc-flap`).
//! * **VRR off** — the nominal grid: the first grid point strictly
//!   after readiness (the same arithmetic the mock timeline runs),
//!   mirroring the deadline scheduler's fixed-sync contract.
//!
//! The selector sequences like the KMS queue: while a flip is
//! outstanding, later rulings anchor on its target (one outstanding
//! flip per CRTC, matching the real UAPI), and a completed flip
//! re-anchors on its timestamp. All inputs carry timestamps; rulings
//! are pure functions of `(anchor, ready, window, policy)`.

use ldp_core::time::Mono;

use crate::policy::VrrPolicy;
use crate::window::{select_flip, FlipSelection, RefreshWindow};

/// The LFC cadence for content whose period exceeds the window
/// (Phase 41): the fewest repeats whose *aligned* interval fits.
///
/// Content at period `P` below the window's floor (`P > max`) cannot
/// ride the stretch — the panel would lapse out of adaptive sync (or,
/// with the Phase 41 floor, flicker in the band the operator
/// removed). The giants' LFC answer: repeat the front buffer on a
/// cadence `P / k` locked to the content's own phase, `k` the fewest
/// repeats whose interval clears the stretch ceiling —
/// `k = ceil(P / max)`, so `P / k ≤ max` by construction and every
/// content flip lands exactly on a repeat slot (no judder: the
/// repeats divide the content period, never fight it).
///
/// Returns `(k, interval)` — the repeat count per content frame and
/// the cadence between repeats — or `None` when LFC does not apply:
/// in-window content (`P ≤ max`, the repeats are idle), or a window
/// too narrow to bridge (`P / k < min` even at the fewest `k` — the
/// miss ruling stands, the content flip catches up late).
///
/// Integer honesty: the cadence is `P / k` floored; the *slots* land
/// at `(j · P) / k` (see [`RefreshSelector::lfc_repeat_at`]), each
/// consecutive gap one of `{floor(P/k), ceil(P/k)}` — both at least
/// the floor of the cadence, which the `≥ min` check pins.
#[must_use]
pub fn lfc_interval(window: &RefreshWindow, period_ns: u64) -> Option<(u64, u64)> {
    let max_ns = window.max_ns();
    if period_ns <= max_ns {
        return None; // in-window content: the repeats are idle
    }
    let k = period_ns.div_ceil(max_ns);
    let interval = period_ns / k;
    (interval >= window.min_ns()).then_some((k, interval))
}

/// What the frame loop should do with content that became ready.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CommitRuling {
    /// Submit the flip; it will land at this time (a legal window slot
    /// or a nominal grid point).
    FlipAt(Mono),
    /// The fixed-rate fallback deferred the frame: do not submit; the
    /// surface is told `frame_dropped(throttled)` and may retry at the
    /// returned grid point.
    Defer {
        /// When the deferred frame's slot opens on the nominal grid.
        retry_at: Mono,
    },
    /// The window was missed without a fixed-rate fallback: submit
    /// immediately; the panel has idled past its stretch so the flip
    /// completes at readiness.
    Late(Mono),
}

impl CommitRuling {
    /// The flip time this ruling schedules, if any (`Defer` schedules
    /// nothing).
    #[must_use]
    pub const fn flip_at(&self) -> Option<Mono> {
        match *self {
            Self::FlipAt(at) | Self::Late(at) => Some(at),
            Self::Defer { .. } => None,
        }
    }
}

/// Per-output flip-time selector.
#[derive(Clone, Copy, Debug)]
pub struct RefreshSelector {
    window: RefreshWindow,
    nominal_ns: u64,
    fixed_rate: bool,
    policy: VrrPolicy,
    vrr_enabled: bool,
    /// Last completed scanout.
    last_flip: Mono,
    /// Target of the one outstanding flip, if any (the sequencing
    /// anchor while the queue is busy).
    pending: Option<Mono>,
    /// The LFC latch (Phase 41): whether the repeat regime holds —
    /// engaged by a missed window (content stretched past the
    /// ceiling), held through boundary-hugging flips, cleared only
    /// by a flip landing comfortably in-window (at least one minimum
    /// interval clear of the ceiling — sustained recovery). The
    /// embedder consults it to keep the repeat cadence warm across
    /// the flap instead of alternating stretch/repeat (the visible
    /// pumping the driver quirk tables name).
    lfc: bool,
}

impl RefreshSelector {
    /// Build a selector for an output whose effective state is decided.
    ///
    /// `vrr_enabled` comes from the policy table; when false the
    /// selector answers nominal-grid rulings.
    #[must_use]
    pub const fn new(
        window: RefreshWindow,
        nominal_ns: u64,
        fixed_rate: bool,
        policy: VrrPolicy,
        vrr_enabled: bool,
    ) -> Self {
        Self {
            window,
            nominal_ns,
            fixed_rate,
            policy,
            vrr_enabled,
            last_flip: Mono::ZERO,
            pending: None,
            lfc: false,
        }
    }

    /// Whether the LFC repeat regime holds (Phase 41's anti-flap
    /// latch): `true` from the first missed window until a flip
    /// lands comfortably in-window. While held, the embedder keeps
    /// the LFC cadence armed — [`Self::lfc_repeat_at`] for the slot,
    /// [`Self::repeat_at`] for the idle floor — so content hovering
    /// at the boundary rides warmed repeats instead of flapping.
    #[must_use]
    pub const fn lfc_engaged(&self) -> bool {
        self.lfc
    }

    /// The next LFC repeat slot for content whose period is
    /// `period_ns`: the first aligned slot after the sequencing
    /// anchor — `anchor + P/k` (see [`lfc_interval`] for the cadence
    /// arithmetic). The anchor should be the content phase origin
    /// (the last content flip's completion); `None` when the cadence
    /// does not serve this period (in-window content, or a window too
    /// narrow to bridge).
    #[must_use]
    pub fn lfc_repeat_at(&self, period_ns: u64) -> Option<Mono> {
        let (_, interval) = lfc_interval(&self.window, period_ns)?;
        Some(self.anchor().saturating_add_ns(interval))
    }

    /// The effective window.
    #[must_use]
    pub const fn window(&self) -> &RefreshWindow {
        &self.window
    }

    /// The sequencing anchor: the outstanding flip's target when the
    /// queue is busy, otherwise the last completed scanout.
    #[must_use]
    pub const fn anchor(&self) -> Mono {
        match self.pending {
            Some(target) => target,
            None => self.last_flip,
        }
    }

    /// Restore the sequencing anchor after a policy-driven rebuild:
    /// an anchor beyond the last completed scanout can only be an
    /// outstanding flip target, which must keep sequencing later
    /// rulings.
    pub(crate) fn restore_anchor(&mut self, anchor: Mono) {
        if anchor.as_ns() > self.last_flip.as_ns() {
            self.pending = Some(anchor);
        } else {
            self.pending = None;
        }
    }

    /// A scanout completed at `ts` (page-flip or idle vblank): the
    /// timeline re-anchors and the queue drains.
    pub fn observe_flip(&mut self, ts: Mono) {
        if ts.as_ns() > self.last_flip.as_ns() {
            self.last_flip = ts;
        }
        self.pending = None;
    }

    /// Content became ready at `ready_at`; what should the frame loop
    /// do with it?
    ///
    /// Rulings that schedule a flip (`FlipAt`, `Late`) occupy the
    /// single outstanding slot; a following ruling before the flip
    /// completes sequences after it.
    #[must_use]
    pub fn commit_ready(&mut self, ready_at: Mono) -> CommitRuling {
        if !self.vrr_enabled {
            return self.grid_ruling(ready_at);
        }
        let anchor = self.anchor();
        match select_flip(anchor, ready_at, &self.window) {
            FlipSelection::At(at) => {
                // The latch's clear rule (Phase 41): only a flip
                // landing *comfortably* in-window — at least one
                // minimum interval clear of the ceiling — counts as
                // sustained recovery. A boundary-hugging flip (within
                // `min` of expiry) keeps the repeat regime warm: the
                // flap guard.
                let comfortable =
                    at.as_ns() + self.window.min_ns() <= anchor.as_ns() + self.window.max_ns();
                if comfortable {
                    self.lfc = false;
                }
                self.pending = Some(at);
                CommitRuling::FlipAt(at)
            }
            FlipSelection::Missed => {
                // The latch's engage rule: the stretch crossed the
                // ceiling — the repeat regime arms (and stays armed
                // through the recovery dwell).
                self.lfc = true;
                // ready_at is past anchor + max. The fixed-rate
                // fallback applies only under deadline policy (always
                // mode wants the latency, never a deferral).
                if self.fixed_rate && self.policy == VrrPolicy::Deadline {
                    let retry = self.next_grid_after(anchor, ready_at);
                    CommitRuling::Defer { retry_at: retry }
                } else {
                    self.pending = Some(ready_at);
                    CommitRuling::Late(ready_at)
                }
            }
        }
    }

    /// The LFC repeat ruling (Phase 31): when *no* content is ready
    /// but the panel's window would stretch past its maximum — the
    /// content rate fell below the minimum refresh — the display side
    /// re-submits the current front buffer at the next
    /// `last_flip + k * min` instant, keeping the panel inside its
    /// VRR window (the backlight stays lit, the scanout clock keeps
    /// ticking; the alternative — letting the window lapse — drops
    /// the panel out of adaptive sync and costs a full re-sync).
    ///
    /// This is a *display-side* pacing rule: the repeat target is the
    /// earliest in-window grid multiple of the minimum interval; the
    /// content flip that arrives before it simply takes its own
    /// ruling (the repeat is a floor, never a block).
    #[must_use]
    pub fn repeat_at(&self) -> Mono {
        let min_ns = self.window.min_ns().max(1);
        let last = self.anchor().as_ns();
        Mono::from_ns(last.div_ceil(min_ns) * min_ns)
    }

    /// Nominal-grid ruling (VRR off): the first grid point strictly
    /// after `ready_at`, sequenced off the anchor — the mock
    /// timeline's fixed-sync arithmetic.
    fn grid_ruling(&mut self, ready_at: Mono) -> CommitRuling {
        let anchor = self.anchor();
        let at = self.next_grid_strictly_after(anchor, ready_at);
        self.pending = Some(at);
        CommitRuling::FlipAt(at)
    }

    /// `anchor + ceil((ready - anchor)/nominal) · nominal`: the first
    /// grid point at-or-after `ready_at` (used by the deferral).
    fn next_grid_after(&self, anchor: Mono, ready_at: Mono) -> Mono {
        let elapsed = ready_at.as_ns().saturating_sub(anchor.as_ns());
        let periods = elapsed.div_ceil(self.nominal_ns);
        anchor.saturating_add_ns(periods * self.nominal_ns)
    }

    /// The first grid point strictly after `ready_at` (a flip submitted
    /// at a vblank lands on the next one).
    fn next_grid_strictly_after(&self, anchor: Mono, ready_at: Mono) -> Mono {
        let elapsed = ready_at.as_ns().saturating_sub(anchor.as_ns());
        let periods = elapsed / self.nominal_ns + 1;
        anchor.saturating_add_ns(periods * self.nominal_ns)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: u64 = 1_000_000_000 / 144;
    const MAX: u64 = 1_000_000_000 / 48;
    const NOM: u64 = 16_666_666;

    fn window() -> RefreshWindow {
        RefreshWindow::from_ns(MIN, MAX).unwrap()
    }

    fn t(ns: u64) -> Mono {
        Mono::from_ns(ns)
    }

    fn vrr_selector(policy: VrrPolicy, fixed_rate: bool) -> RefreshSelector {
        RefreshSelector::new(window(), NOM, fixed_rate, policy, true)
    }

    #[test]
    fn in_window_commit_flips_at_ready_or_floor() {
        let mut s = vrr_selector(VrrPolicy::Deadline, true);
        s.observe_flip(t(1_000_000));
        // Ready before the floor: waits for anchor + min.
        assert_eq!(
            s.commit_ready(t(1_500_000)),
            CommitRuling::FlipAt(t(1_000_000 + MIN))
        );
        // Fresh anchor: ready past the floor flips exactly at ready.
        let mut s = vrr_selector(VrrPolicy::Deadline, true);
        s.observe_flip(t(1_000_000));
        assert_eq!(
            s.commit_ready(t(1_000_000 + MIN + 1)),
            CommitRuling::FlipAt(t(1_000_000 + MIN + 1))
        );
    }

    #[test]
    fn outstanding_flip_sequences_later_rulings() {
        let mut s = vrr_selector(VrrPolicy::Deadline, true);
        s.observe_flip(t(1_000_000));
        // First ruling: floor slot at 1ms + MIN.
        assert_eq!(
            s.commit_ready(t(1_200_000)),
            CommitRuling::FlipAt(t(1_000_000 + MIN))
        );
        // A second commit before that flip completes sequences off its
        // target, not the completed scanout.
        assert_eq!(
            s.commit_ready(t(1_300_000)),
            CommitRuling::FlipAt(t(1_000_000 + 2 * MIN))
        );
        // Completion drains the queue and re-anchors: a commit ready
        // right after the completed flip sequences off the new anchor
        // (its own floor), not the drained pending target.
        s.observe_flip(t(1_000_000 + MIN));
        assert_eq!(
            s.commit_ready(t(1_000_000 + MIN + 10)),
            CommitRuling::FlipAt(t(1_000_000 + 2 * MIN))
        );
    }

    #[test]
    fn missed_window_defers_with_fixed_rate_under_deadline() {
        let mut s = vrr_selector(VrrPolicy::Deadline, true);
        s.observe_flip(t(1_000_000));
        // 40 ms later (window max is 20.83 ms): missed.
        let ready = t(1_000_000 + 40_000_000);
        match s.commit_ready(ready) {
            CommitRuling::Defer { retry_at } => {
                // Next nominal grid point at-or-after ready, anchored
                // on the completed scanout: 41_000_000 + ceil(0) =>
                // elapsed 40_000_000 / 16_666_666 = 2 (ceil 3? no: 40
                // ms / 16.67 ms = 2.4 => ceil 3) => 1_000_000 +
                // 3 * 16_666_666 = 50_999_998.
                assert_eq!(retry_at.as_ns(), 1_000_000 + 3 * NOM);
                assert!(retry_at.as_ns() >= ready.as_ns());
            }
            other => panic!("expected Defer, got {other:?}"),
        }
        // A deferral schedules nothing: the next ruling still sees the
        // completed scanout as anchor (pending stays empty).
        assert_eq!(s.anchor().as_ns(), 1_000_000);
    }

    #[test]
    fn missed_window_catches_up_without_fixed_rate() {
        let mut s = vrr_selector(VrrPolicy::Deadline, false);
        s.observe_flip(t(1_000_000));
        let ready = t(1_000_000 + 40_000_000);
        assert_eq!(s.commit_ready(ready), CommitRuling::Late(ready));
        // The catch-up flip occupies the outstanding slot.
        assert_eq!(s.anchor().as_ns(), ready.as_ns());
    }

    #[test]
    fn always_policy_never_defers() {
        let mut s = vrr_selector(VrrPolicy::Always, true);
        s.observe_flip(t(1_000_000));
        let ready = t(1_000_000 + 40_000_000);
        assert_eq!(s.commit_ready(ready), CommitRuling::Late(ready));
    }

    #[test]
    fn vrr_off_answers_nominal_grid() {
        let mut s = RefreshSelector::new(window(), NOM, true, VrrPolicy::Off, false);
        s.observe_flip(t(NOM));
        // Mid-frame commit: lands on the next grid point.
        assert_eq!(
            s.commit_ready(t(NOM + 1_000_000)),
            CommitRuling::FlipAt(t(2 * NOM))
        );
        // A commit exactly at a grid point lands on the following one.
        assert_eq!(s.commit_ready(t(2 * NOM)), CommitRuling::FlipAt(t(3 * NOM)));
    }

    #[test]
    fn ruling_flip_at_helper() {
        let d = CommitRuling::Defer { retry_at: t(1_000) };
        assert_eq!(d.flip_at(), None);
        assert_eq!(CommitRuling::FlipAt(t(2_000)).flip_at(), Some(t(2_000)));
        assert_eq!(CommitRuling::Late(t(3_000)).flip_at(), Some(t(3_000)));
    }

    // ---- the LFC cadence and the latch (Phase 41) --------------------

    #[test]
    fn lfc_cadence_table() {
        // The 48-144 Hz window around 60 Hz: content below 48 Hz
        // bridges on phase-aligned repeats.
        let w = window();
        // In-window content: the repeats are idle.
        assert_eq!(lfc_interval(&w, NOM), None);
        assert_eq!(lfc_interval(&w, MAX), None);
        // 40 Hz (25 ms): k=2 — two repeats per content frame, 12.5 ms
        // apart (the giants' "LFC at 2x content").
        assert_eq!(lfc_interval(&w, 25_000_000), Some((2, 12_500_000)));
        // 30 Hz (33.33 ms): still k=2, 16.67 ms apart.
        assert_eq!(lfc_interval(&w, 33_333_333), Some((2, 16_666_666)));
        // 24 Hz (41.67 ms): k=2 at the ceiling's edge — 20.83 ms, the
        // longest legal cadence, still in-window.
        assert_eq!(lfc_interval(&w, 41_666_666), Some((2, 20_833_333)));
        // 20 Hz (50 ms): k=3, 16.67 ms apart.
        assert_eq!(lfc_interval(&w, 50_000_000), Some((3, 16_666_666)));
        // 10 Hz (100 ms): k=5, 20 ms apart.
        assert_eq!(lfc_interval(&w, 100_000_000), Some((5, 20_000_000)));
        // Every cadence respects both window bounds.
        for period in [MAX + 1, 25_000_000, 33_333_333, 50_000_000, 100_000_000] {
            if let Some((_, interval)) = lfc_interval(&w, period) {
                assert!(interval >= MIN, "the cadence violates the slip floor");
                assert!(interval <= MAX, "the cadence violates the stretch ceiling");
            }
        }
    }

    #[test]
    fn lfc_declines_a_window_too_narrow_to_bridge() {
        // A narrow window (18-20 ms): content at 21 ms has k=2 →
        // 10.5 ms < 18 ms — no legal cadence; the miss ruling stands.
        let narrow = RefreshWindow::from_ns(18_000_000, 20_000_000).unwrap();
        assert_eq!(lfc_interval(&narrow, 21_000_000), None);
        // Content slow enough to clear even the narrow window's floor
        // at some k: 40 ms → k=2 → 20 ms — exactly the ceiling.
        assert_eq!(lfc_interval(&narrow, 40_000_000), Some((2, 20_000_000)));
    }

    #[test]
    fn lfc_repeat_slots_align_to_the_anchor() {
        let mut s = vrr_selector(VrrPolicy::Deadline, true);
        s.observe_flip(t(1_000_000));
        // 40 Hz content: the next aligned slot is 12.5 ms past the
        // anchor (the first of the two repeats).
        assert_eq!(s.lfc_repeat_at(25_000_000), Some(t(1_000_000 + 12_500_000)));
        // In-window content: no slot (the repeats are idle).
        assert_eq!(s.lfc_repeat_at(NOM), None);
    }

    #[test]
    fn the_latch_engages_on_the_miss_and_clears_on_recovery() {
        let mut s = vrr_selector(VrrPolicy::Deadline, false);
        s.observe_flip(t(1_000_000));
        assert!(!s.lfc_engaged(), "a fresh selector has not engaged");
        // Content 40 ms later: the window (20.83 ms) missed — engage.
        let _ = s.commit_ready(t(1_000_000 + 40_000_000));
        assert!(s.lfc_engaged(), "the miss engages the repeat regime");
        // The catch-up flip completes; recovered content lands
        // comfortably in-window (12 ms — one full min clear of the
        // 20.83 ms ceiling): the latch clears.
        s.observe_flip(t(1_000_000 + 40_000_000));
        let _ = s.commit_ready(t(1_000_000 + 52_000_000));
        assert!(!s.lfc_engaged(), "comfortable recovery clears the latch");
    }

    #[test]
    fn the_latch_holds_through_boundary_hugging_flips() {
        // THE FLAP: content hovering at the boundary alternates
        // miss/in-window. A flip landing within `min` of the ceiling
        // is *not* recovery — the latch holds, the repeats stay warm.
        let mut s = vrr_selector(VrrPolicy::Deadline, false);
        s.observe_flip(t(1_000_000));
        // The miss engages.
        let _ = s.commit_ready(t(1_000_000 + 25_000_000));
        assert!(s.lfc_engaged());
        s.observe_flip(t(1_000_000 + 25_000_000));
        // The recovery attempt: ready 19.5 ms after the new anchor —
        // in-window (≤ 20.83 ms) but boundary-hugging (only 1.33 ms
        // clear, less than the 6.94 ms min interval).
        let _ = s.commit_ready(t(1_000_000 + 25_000_000 + 19_500_000));
        assert!(
            s.lfc_engaged(),
            "a boundary-hugging flip is not recovery — no flap"
        );
        // Sustained recovery (comfortably inside) clears.
        s.observe_flip(t(1_000_000 + 25_000_000 + 19_500_000));
        let _ = s.commit_ready(t(1_000_000 + 25_000_000 + 19_500_000 + 12_000_000));
        assert!(!s.lfc_engaged());
    }

    #[test]
    fn the_latch_is_inert_when_vrr_is_off() {
        // The grid path never touches the latch (fixed sync has no
        // repeat regime).
        let mut s = RefreshSelector::new(window(), NOM, true, VrrPolicy::Off, false);
        s.observe_flip(t(NOM));
        assert!(!s.lfc_engaged());
        let _ = s.commit_ready(t(2 * NOM));
        assert!(!s.lfc_engaged());
    }
}
