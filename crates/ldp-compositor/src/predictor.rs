//! The per-output frame clock: vblank prediction on top of the
//! `ldp-core` PLL, plus the scheduler-facing grid arithmetic.
//!
//! The core [`VblankPredictor`] is the
//! shared PLL (re-anchor + proportional/integral correction on real flip
//! timestamps). This module wraps it into the timeline object the
//! [`FrameScheduler`](crate::scheduler::FrameScheduler) actually uses:
//!
//! * **strict** next-vblank lookups (the first vblank strictly *after* a
//!   time — a frame request landing exactly on a vblank can never make
//!   that vblank),
//! * **n-ahead extrapolation** through the effective step
//!   (`nominal period + PLL correction`), which stays exact at lock when
//!   the real panel period differs from the advertised mode — the plain
//!   period would accumulate the drift error per extrapolated step,
//! * **measured refresh bookkeeping**: the interval between the last two
//!   valid flips, used for `presented` feedback; invalid samples (a
//!   missed vblank, a stall, a duplicate) fall back to the PLL's current
//!   effective step so feedback is always a sane interval. Under VRR
//!   ([`FrameClock::arm_vrr`]) the validity band is the panel's own
//!   physical window — the LFC fast end and the stretched slow end are
//!   both honest single-period landings, and the feedback reports the
//!   cadence the panel actually ran (the Phase 39 presentation clock).
//!
//! All arithmetic is pure integer math on [`Mono`] nanoseconds; the clock
//! never queries wall time itself (the driver feeds it timestamps), which
//! is what makes scheduler decisions replayable.

use ldp_core::time::{Mono, RefreshInterval, VblankPredictor};

/// Observed bookkeeping for one page-flip event.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FlipObservation {
    /// Timestamp of the flip as fed to [`FrameClock::observe_flip`].
    pub ts: Mono,
    /// Total flips observed on this timeline (this one included).
    pub count: u64,
    /// Interval since the previous flip when it was a plausible single
    /// period (within half..1.5x the effective step); `None` otherwise.
    pub interval: Option<RefreshInterval>,
}

/// Per-output vblank timeline anchored on real flip timestamps.
///
/// One `FrameClock` per output (`docs/architecture.md` §10.3): outputs are
/// independent timelines. Unanchored until the first
/// [`observe_flip`](FrameClock::observe_flip); prediction methods return
/// `None` before that (the scheduler parks frame-target replies instead
/// of inventing deadlines).
#[derive(Clone, Debug)]
pub struct FrameClock {
    predictor: VblankPredictor,
    flips: u64,
    last_flip: Option<Mono>,
    last_interval: Option<RefreshInterval>,
    /// The panel's physical VRR window `(min_ns, max_ns)` when armed:
    /// the measured-interval validity band becomes exactly the window
    /// (a landing outside it is a stall or a duplicate, never a
    /// single period). `None`: the legacy half..1.5x band.
    vrr: Option<(u64, u64)>,
}

impl FrameClock {
    /// Start a clock for an output running at `nominal`.
    #[must_use]
    pub fn new(nominal: RefreshInterval) -> Self {
        Self {
            predictor: VblankPredictor::new(nominal, Mono::ZERO),
            flips: 0,
            last_flip: None,
            last_interval: None,
            vrr: None,
        }
    }

    /// Arm the VRR-aware measurement band (Phase 39's presentation
    /// clock): the panel may land a flip anywhere inside its physical
    /// window `[min_ns, max_ns]`, and every in-window interval is an
    /// honest single-period sample — the LFC fast end (a repeat at the
    /// minimum interval) no less than the stretched slow end. The
    /// window must strictly contain the nominal period
    /// (`0 < min_ns <= nominal < max_ns`); anything else is ignored
    /// (the legacy half..1.5x band stays — the honest degradation for
    /// a window the integrator did not fully probe).
    ///
    /// The deadline grid is untouched: predictions keep the nominal
    /// cadence (the scheduler's widened commit window absorbs the
    /// stretch); only the *measurement* learns the window, because
    /// only the measurement claims to report what the panel ran.
    pub fn arm_vrr(&mut self, min_ns: u64, max_ns: u64) {
        let nominal = self.predictor.refresh().as_ns();
        if min_ns > 0 && min_ns <= nominal && max_ns > nominal {
            self.vrr = Some((min_ns, max_ns));
        }
    }

    /// Feed a page-flip (or latched vblank) timestamp.
    ///
    /// Timestamps must be non-decreasing across calls (the scheduler
    /// enforces input ordering upstream). The PLL corrects drift from
    /// small residuals and re-anchors on discontinuities; the measured
    /// interval only accepts single-period samples so stalls and missed
    /// vblanks do not poison the `presented` refresh feedback.
    pub fn observe_flip(&mut self, ts: Mono) -> FlipObservation {
        let interval = self.measure_interval(ts);
        self.predictor.observe_flip(ts);
        self.flips += 1;
        self.last_flip = Some(ts);
        // An invalid sample (stall, duplicate, missed vblank) does not
        // clobber the last valid measurement: the panel's period did not
        // change just because a vblank went unobserved.
        if interval.is_some() {
            self.last_interval = interval;
        }
        FlipObservation {
            ts,
            count: self.flips,
            interval,
        }
    }

    /// Whether the timeline has been anchored by at least one flip.
    #[must_use]
    pub fn is_anchored(&self) -> bool {
        self.flips > 0
    }

    /// The nominal refresh this clock was constructed with.
    #[must_use]
    pub fn nominal(&self) -> RefreshInterval {
        self.predictor.refresh()
    }

    /// Best current estimate of the real refresh interval: the last
    /// single-period measurement when one exists, else the PLL's
    /// effective step (nominal + integrated correction).
    #[must_use]
    pub fn measured(&self) -> RefreshInterval {
        // The effective step is guaranteed positive; the nominal
        // fallback only guards the impossible zero case.
        self.last_interval.unwrap_or_else(|| {
            RefreshInterval::from_ns(self.predictor.effective_step_ns())
                .unwrap_or_else(|| self.predictor.refresh())
        })
    }

    /// Total flips observed.
    #[must_use]
    pub const fn flip_count(&self) -> u64 {
        self.flips
    }

    /// The most recent observed landing (nanoseconds; zero while
    /// unanchored). Under VRR this is the anchor the panel's next
    /// refresh opportunity grows from (the Phase 39 adaptive target).
    #[must_use]
    pub const fn last_flip_ns(&self) -> u64 {
        match self.last_flip {
            Some(t) => t.as_ns(),
            None => 0,
        }
    }

    /// The first predicted vblank strictly after `t`.
    ///
    /// `None` while unanchored. The prediction grid starts one nominal
    /// period after the corrected anchor (the flip cell itself already
    /// happened) and continues at the effective step, so the first
    /// predicted cell is exact at PLL lock even under period drift.
    /// Callers feed non-decreasing timestamps, so `t` never precedes
    /// the last observed flip.
    #[must_use]
    pub fn next_vblank(&self, t: Mono) -> Option<Mono> {
        if self.flips == 0 {
            return None;
        }
        let nominal = self.predictor.refresh().as_ns() as i128;
        let effective = self.predictor.effective_step_ns() as i128;
        let anchor =
            self.predictor.last_flip().as_ns() as i128 + i128::from(self.predictor.correction_ns());
        let first = anchor + nominal; // grid cell 1: exact at lock
        let t_ns = t.as_ns() as i128;
        let value = if t_ns < first {
            first
        } else {
            // First grid cell strictly beyond t: ceil((t+1-first)/eff)
            // further effective steps.
            let beyond = t_ns + 1 - first;
            let steps = (beyond + effective - 1) / effective;
            first + steps * effective
        };
        Some(Mono::from_ns(u64::try_from(value).unwrap_or(u64::MAX)))
    }

    /// The `n`-th predicted vblank strictly after `t` (`n >= 1`).
    ///
    /// Steps beyond the first use the effective step so period drift does
    /// not accumulate across the extrapolation.
    #[must_use]
    pub fn nth_vblank(&self, t: Mono, n: u32) -> Option<Mono> {
        let first = self.next_vblank(t)?;
        let extra = u64::from(n.saturating_sub(1)) * self.predictor.effective_step_ns();
        Some(first.saturating_add_ns(extra))
    }

    /// Interval since the previous flip if it is a plausible single
    /// period, `None` otherwise.
    ///
    /// The legacy band is half..1.5x the effective step (a jittered
    /// nominal landing is a period; a stall, a duplicate, or a
    /// missed-vblank gap is not). Under VRR the band is the panel's
    /// own window: the fast repeats and slow stretches the panel
    /// physically performs are exactly the samples the presentation
    /// clock exists to report.
    fn measure_interval(&self, ts: Mono) -> Option<RefreshInterval> {
        let last = self.last_flip?;
        let delta = ts.duration_since(last);
        let ok = if let Some((min_ns, max_ns)) = self.vrr {
            delta >= min_ns && delta <= max_ns
        } else {
            let step = self.predictor.effective_step_ns();
            delta >= step / 2 && delta <= step + step / 2
        };
        if !ok {
            return None;
        }
        RefreshInterval::from_ns(delta)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIXTY: u64 = 16_666_666;

    fn anchored_clock() -> FrameClock {
        let mut c = FrameClock::new(RefreshInterval::from_ns(SIXTY).unwrap());
        c.observe_flip(Mono::from_ns(SIXTY));
        c
    }

    #[test]
    fn unanchored_clock_predicts_nothing() {
        let c = FrameClock::new(RefreshInterval::from_ns(SIXTY).unwrap());
        assert!(!c.is_anchored());
        assert_eq!(c.next_vblank(Mono::ZERO), None);
        assert_eq!(c.nth_vblank(Mono::ZERO, 1), None);
        assert_eq!(c.flip_count(), 0);
    }

    #[test]
    fn next_vblank_is_strictly_after() {
        let c = anchored_clock(); // flip at t=16_666_666, correction 0
                                  // Exactly on the flip: strictly after excludes it — the next
                                  // vblank is one period out.
        assert_eq!(
            c.next_vblank(Mono::from_ns(SIXTY)),
            Some(Mono::from_ns(2 * SIXTY))
        );
        // Anywhere inside the period: the same next vblank.
        assert_eq!(
            c.next_vblank(Mono::from_ns(SIXTY - 1)),
            Some(Mono::from_ns(2 * SIXTY))
        );
        assert_eq!(
            c.next_vblank(Mono::from_ns(SIXTY + 1)),
            Some(Mono::from_ns(2 * SIXTY))
        );
        // At the next vblank: the one after that.
        assert_eq!(
            c.next_vblank(Mono::from_ns(2 * SIXTY)),
            Some(Mono::from_ns(3 * SIXTY))
        );
    }

    #[test]
    fn nth_vblank_extrapolates_with_effective_step() {
        let mut c = FrameClock::new(RefreshInterval::from_ns(SIXTY).unwrap());
        // Panel truly at ~55 Hz: feed exact 18_181_818 ns flips until the
        // PLL correction approaches the drift (~1.52 ms).
        let true_step = 18_181_818u64;
        let mut t = 0u64;
        for _ in 0..512 {
            t += true_step;
            c.observe_flip(Mono::from_ns(t));
        }
        let base = Mono::from_ns(t);
        let first = c.next_vblank(base).expect("anchored");
        // Next true vblank after the last flip is one true step later.
        let err_first = first.as_ns().abs_diff(t + true_step);
        assert!(err_first < 200_000, "first off by {err_first}");
        let fourth = c.nth_vblank(base, 4).expect("anchored");
        let want = t + 4 * true_step;
        let err_nth = fourth.as_ns().abs_diff(want);
        // The plain nominal period would be off by 3 * 1.5 ms = ~4.5 ms.
        assert!(err_nth < 800_000, "nth off by {err_nth}");
    }

    #[test]
    fn measured_interval_tracks_single_periods() {
        let mut c = anchored_clock();
        // A normal flip one period later is accepted.
        let obs = c.observe_flip(Mono::from_ns(2 * SIXTY));
        assert_eq!(obs.count, 2);
        assert_eq!(obs.interval.map(RefreshInterval::as_ns), Some(SIXTY));
        assert_eq!(c.measured().as_ns(), SIXTY);
        // A jittered interval is accepted as-is: measured() reports the
        // raw measurement, not the effective step (which stays nominal
        // while the PLL correction is still near zero).
        let obs = c.observe_flip(Mono::from_ns(3 * SIXTY + 100_000));
        assert_eq!(
            obs.interval.map(RefreshInterval::as_ns),
            Some(SIXTY + 100_000)
        );
        assert_eq!(c.measured().as_ns(), SIXTY + 100_000);
        // A stall (five periods) is rejected; measured keeps the old value.
        let obs = c.observe_flip(Mono::from_ns(8 * SIXTY + 100_000));
        assert_eq!(obs.interval, None);
        assert_eq!(c.measured().as_ns(), SIXTY + 100_000);
        // First flip of a fresh clock has no interval.
        let mut fresh = FrameClock::new(RefreshInterval::from_ns(SIXTY).unwrap());
        let obs = fresh.observe_flip(Mono::from_ns(SIXTY));
        assert_eq!(obs.interval, None);
        // Unmeasured clock falls back to the effective step (== nominal
        // while correction is zero).
        assert_eq!(fresh.measured().as_ns(), SIXTY);
    }

    #[test]
    fn duplicate_flip_ts_is_not_a_period() {
        let mut c = anchored_clock();
        let obs = c.observe_flip(Mono::from_ns(SIXTY)); // same ts as anchor
        assert_eq!(obs.interval, None);
        assert_eq!(c.flip_count(), 2);
    }

    #[test]
    fn saturation_in_extreme_extrapolation() {
        let c = anchored_clock();
        let near_end = Mono::from_ns(u64::MAX - 100);
        // Saturating arithmetic keeps the prediction finite and strictly
        // after the query time (no overflow panics).
        let v = c.nth_vblank(near_end, 8).expect("anchored");
        assert!(v.as_ns() >= near_end.as_ns());
    }

    // -- Phase 39: the VRR-aware measurement band --------------------------

    /// The mock's 48-144 Hz window over a 60 Hz nominal.
    const W144_MIN: u64 = 6_944_444;
    const W144_MAX: u64 = 20_833_333;

    #[test]
    fn vrr_window_accepts_the_lfc_fast_end() {
        // The panel repeats at its fast end: landings 8 ms apart are
        // inside the window but BELOW half the nominal period — the
        // legacy band rejects them as duplicates; the armed clock
        // reports the cadence the panel actually ran.
        let mut c = FrameClock::new(RefreshInterval::from_ns(SIXTY).unwrap());
        c.arm_vrr(W144_MIN, W144_MAX);
        c.observe_flip(Mono::from_ns(SIXTY));
        let obs = c.observe_flip(Mono::from_ns(SIXTY + 8_000_000));
        assert_eq!(obs.interval.map(RefreshInterval::as_ns), Some(8_000_000));
        assert_eq!(c.measured().as_ns(), 8_000_000);
        // The same landing without the window armed stays invalid (the
        // legacy half-band is the honest default for a fixed panel).
        let mut fixed = FrameClock::new(RefreshInterval::from_ns(SIXTY).unwrap());
        fixed.observe_flip(Mono::from_ns(SIXTY));
        let obs = fixed.observe_flip(Mono::from_ns(SIXTY + 8_000_000));
        assert_eq!(obs.interval, None);
        assert_eq!(fixed.measured().as_ns(), SIXTY);
    }

    #[test]
    fn vrr_window_accepts_the_stretched_slow_end() {
        // A wide window (a 30-120 Hz panel at a 60 Hz nominal): a 30 ms
        // stretched landing is beyond the legacy 1.5x band but a legal
        // in-window landing — the armed clock reports it.
        let mut c = FrameClock::new(RefreshInterval::from_ns(SIXTY).unwrap());
        c.arm_vrr(8_333_333, 33_333_333);
        c.observe_flip(Mono::from_ns(SIXTY));
        let obs = c.observe_flip(Mono::from_ns(SIXTY + 30_000_000));
        assert_eq!(obs.interval.map(RefreshInterval::as_ns), Some(30_000_000));
        assert_eq!(c.measured().as_ns(), 30_000_000);
        // Legacy: the same landing is a stall, not a period.
        let mut fixed = FrameClock::new(RefreshInterval::from_ns(SIXTY).unwrap());
        fixed.observe_flip(Mono::from_ns(SIXTY));
        assert_eq!(
            fixed
                .observe_flip(Mono::from_ns(SIXTY + 30_000_000))
                .interval,
            None
        );
    }

    #[test]
    fn out_of_window_landings_stay_invalid() {
        // Beyond the window's max the panel cannot land: a 25 ms gap on
        // the 48-144 window is a stall (no commit for a while), never a
        // single period — measured keeps the last honest sample.
        let mut c = FrameClock::new(RefreshInterval::from_ns(SIXTY).unwrap());
        c.arm_vrr(W144_MIN, W144_MAX);
        c.observe_flip(Mono::from_ns(SIXTY));
        c.observe_flip(Mono::from_ns(SIXTY + 20_000_000));
        assert_eq!(c.measured().as_ns(), 20_000_000);
        let obs = c.observe_flip(Mono::from_ns(SIXTY + 45_000_000));
        assert_eq!(obs.interval, None);
        assert_eq!(c.measured().as_ns(), 20_000_000);
        // And a duplicate (zero delta) is below the window's min.
        let obs = c.observe_flip(Mono::from_ns(SIXTY + 45_000_000));
        assert_eq!(obs.interval, None);
    }

    #[test]
    fn arming_ignores_windows_that_do_not_contain_nominal() {
        // A window without a min side (0) or one that excludes the
        // nominal is not a panel window — the legacy band stands.
        let mut c = FrameClock::new(RefreshInterval::from_ns(SIXTY).unwrap());
        c.arm_vrr(0, 33_333_333);
        c.observe_flip(Mono::from_ns(SIXTY));
        let obs = c.observe_flip(Mono::from_ns(SIXTY + 30_000_000));
        assert_eq!(obs.interval, None);
        let mut c2 = FrameClock::new(RefreshInterval::from_ns(SIXTY).unwrap());
        c2.arm_vrr(20_000_000, 33_333_333); // min above nominal
        c2.observe_flip(Mono::from_ns(SIXTY));
        assert_eq!(
            c2.observe_flip(Mono::from_ns(SIXTY + 8_000_000)).interval,
            None
        );
    }

    #[test]
    fn arming_leaves_the_deadline_grid_nominal() {
        // The presentation clock learns the window; the prediction
        // grid does not — deadlines keep the nominal cadence (the
        // scheduler's widened commit window absorbs the stretch).
        let plain = anchored_clock();
        let mut armed = FrameClock::new(RefreshInterval::from_ns(SIXTY).unwrap());
        armed.arm_vrr(W144_MIN, W144_MAX);
        armed.observe_flip(Mono::from_ns(SIXTY));
        assert_eq!(
            armed.next_vblank(Mono::from_ns(SIXTY)),
            plain.next_vblank(Mono::from_ns(SIXTY))
        );
    }
}
