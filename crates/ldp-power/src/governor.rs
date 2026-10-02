//! The GPU clock governor — the frequency doctrine (Phase 35).
//!
//! A compositing GPU's honest load is *frames actually rendered*, not
//! frames the panel could show. A static desktop renders nothing (the
//! damage-driven pump proves that); a 60 Hz animation renders at the
//! panel's full rate; a 30 Hz slideshow renders at half. The clock
//! that serves the render engine should follow that signal — the
//! giants' DVFS does exactly this, in silicon we cannot touch, over a
//! seam we can model: the governor decides, the host applies.
//!
//! The doctrine is asymmetric hysteresis, because the two directions
//! fail differently:
//!
//! * **Up is instant.** One decision interval above the high watermark
//!   and the clock steps up (default: load > 2/3 for one interval).
//!   Under-clocking a heavy frame is *visible* — a missed deadline,
//!   a janky animation — so the cost of stepping up late is paid in
//!   the one currency the compositor never spends: frame pacing.
//! * **Down is patient.** Three consecutive intervals below the low
//!   watermark (default: load < 1/3) before the clock steps down. The
//!   cost of stepping down early is one heavy frame rendered at a slow
//!   clock — recoverable, invisible at the watermark's margin, but
//!   worth avoiding when the load is merely *bursty* (a tooltip
//!   animating over an otherwise still desktop).
//!
//! The load signal is honest by construction: `interval` receives the
//! *landed flips* of the interval and the interval's duration, and
//! load is flips over the flips the refresh grid could have landed —
//! the zero-composite frame counts (it lands), a static scene counts
//! zero (nothing lands), a 60 Hz animation on a 60 Hz panel counts
//! one. The governor never sees a clock: the host passes durations.
//!
//! The P-states are abstract rungs (`0` = deepest, `3` = fastest);
//! the mapping to megahertz is the host's (the driver's frequency
//! table), and the energy ledger prices each rung with the model's
//! documented figures.

use ldp_core::time::RefreshInterval;

/// One governor P-state rung. `0` is the deepest (slowest, cheapest),
/// [`ClockGovernor::TOP`] the fastest. Ordered: deeper is smaller.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
pub enum PState {
    /// The deepest rung — the static-scene clock.
    P0,
    /// Cruise — light, steady animation.
    P1,
    /// Busy — dense compositing.
    P2,
    /// The ceiling — everything rendered, every frame.
    P3,
}

impl PState {
    /// The rung's index (0..=3) — the residency histogram's slot.
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::P0 => 0,
            Self::P1 => 1,
            Self::P2 => 2,
            Self::P3 => 3,
        }
    }

    /// The rung's name (the report line).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::P0 => "p0",
            Self::P1 => "p1",
            Self::P2 => "p2",
            Self::P3 => "p3",
        }
    }

    /// The rung above (saturating at the top).
    const fn step_up(self) -> Self {
        match self {
            Self::P0 => Self::P1,
            Self::P1 => Self::P2,
            Self::P2 | Self::P3 => Self::P3,
        }
    }

    /// The rung below (saturating at the floor).
    const fn step_down(self) -> Self {
        match self {
            Self::P0 | Self::P1 => Self::P0,
            Self::P2 => Self::P1,
            Self::P3 => Self::P2,
        }
    }
}

/// A governor decision (data — the host applies the rung to the
/// driver's frequency table).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum GovernorEvent {
    /// The clock stepped up (one busy interval above the high
    /// watermark).
    Up,
    /// The clock stepped down (three quiet intervals below the low
    /// watermark).
    Down,
}

/// The GPU clock governor.
#[derive(Clone, Debug)]
pub struct ClockGovernor {
    /// The current rung.
    state: PState,
    /// Consecutive intervals below the low watermark.
    quiet: u32,
    /// Consecutive intervals above the high watermark.
    busy: u32,
    /// The high watermark (load fraction, e.g. 2/3).
    high: (u64, u64),
    /// The low watermark (load fraction, e.g. 1/3).
    low: (u64, u64),
    /// Intervals needed below the low watermark before stepping down.
    down_hysteresis: u32,
    /// Residency: intervals spent at each rung (the report).
    residency: [u64; 4],
    /// Total decisions up / down.
    ups: u64,
    downs: u64,
}

impl ClockGovernor {
    /// The top rung.
    pub const TOP: PState = PState::P3;

    /// The default governor: watermarks 1/3 and 2/3, down-hysteresis
    /// three intervals, born at the floor (bring-up owes nothing).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: PState::P0,
            quiet: 0,
            busy: 0,
            high: (2, 3),
            low: (1, 3),
            down_hysteresis: 3,
            residency: [0; 4],
            ups: 0,
            downs: 0,
        }
    }

    /// The current rung.
    #[must_use]
    pub const fn state(&self) -> PState {
        self.state
    }

    /// Total step-up decisions.
    #[must_use]
    pub const fn ups(&self) -> u64 {
        self.ups
    }

    /// Total step-down decisions.
    #[must_use]
    pub const fn downs(&self) -> u64 {
        self.downs
    }

    /// Intervals spent at each rung `[p0, p1, p2, p3]`.
    #[must_use]
    pub const fn residency(&self) -> [u64; 4] {
        self.residency
    }

    /// Report one decision interval's activity: `flips` landed flips
    /// on the pacing grid whose nominal period is `nominal`, over an
    /// interval of `dt_ms` milliseconds. Returns the decision, if
    /// any. Load is the flip density against the grid's capacity —
    /// the refresh-rate-honest fraction:
    ///
    /// * a static desktop (zero flips) loads 0;
    /// * a full-rate animation loads 1;
    /// * a half-rate animation loads 1/2 (above the low watermark,
    ///   below the high one — the hold zone: no decision, the rung
    ///   persists).
    ///
    /// A `dt` the nominal cannot divide (a short first interval, a
    /// long poll-paced gap) saturates at the integer flip count — the
    /// load is never invented: fewer than one grid period of interval
    /// with one flip is load 1.
    pub fn interval(
        &mut self,
        flips: u64,
        nominal: RefreshInterval,
        dt_ms: u64,
    ) -> Vec<GovernorEvent> {
        // The grid's capacity over this interval: how many nominal
        // periods fit in dt. Zero (a sub-period interval) saturates:
        // any flip at all is full load.
        let nominal_ns = nominal.as_ns().max(1);
        let window_ns = dt_ms * 1_000_000;
        let capacity = if window_ns >= nominal_ns {
            window_ns / nominal_ns
        } else {
            u64::from(flips > 0)
        };
        // Load as a fraction of capacity, compared against the
        // watermarks without floating point: flips/capacity >< hi/lo.
        let (hi_num, hi_den) = self.high;
        let (lo_num, lo_den) = self.low;
        let busy = flips * hi_den > capacity.saturating_mul(hi_num);
        let quiet = flips * lo_den < capacity.saturating_mul(lo_num);
        self.residency[self.state.index()] += 1;
        if busy {
            self.busy += 1;
            self.quiet = 0;
        } else if quiet {
            self.quiet += 1;
            self.busy = 0;
        } else {
            // The hold zone: neither watermark crossed — both streaks
            // age out (a mixed interval is not evidence either way).
            self.busy = 0;
            self.quiet = 0;
        }
        // Up is instant: one busy interval steps up (saturating at
        // the top — further busy intervals hold).
        if self.busy >= 1 && self.state < Self::TOP {
            self.state = self.state.step_up();
            self.ups += 1;
            self.busy = 0;
            return vec![GovernorEvent::Up];
        }
        // Down is patient: `down_hysteresis` consecutive quiet
        // intervals step down (saturating at the floor).
        if self.quiet >= self.down_hysteresis && self.state > PState::P0 {
            self.state = self.state.step_down();
            self.downs += 1;
            self.quiet = 0;
            return vec![GovernorEvent::Down];
        }
        Vec::new()
    }
}

impl Default for ClockGovernor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ldp_core::time::RefreshInterval;

    /// 60 Hz nominal.
    fn hz60() -> RefreshInterval {
        RefreshInterval::from_millihz(60_000).expect("60 Hz")
    }

    /// A 100 ms decision interval at 60 Hz: capacity 6 flips.
    fn interval_100ms() -> u64 {
        100
    }

    #[test]
    fn born_at_the_floor() {
        let g = ClockGovernor::new();
        assert_eq!(g.state(), PState::P0);
        assert_eq!(g.residency(), [0, 0, 0, 0]);
    }

    #[test]
    fn a_static_scene_stays_at_the_floor() {
        let mut g = ClockGovernor::new();
        for _ in 0..40 {
            assert_eq!(g.interval(0, hz60(), interval_100ms()), vec![]);
        }
        assert_eq!(g.state(), PState::P0);
        assert_eq!(g.residency()[0], 40);
    }

    #[test]
    fn one_busy_interval_steps_up_instantly() {
        let mut g = ClockGovernor::new();
        // Capacity 6, one interval of 5 flips: 5/6 > 2/3 — busy.
        assert_eq!(
            g.interval(5, hz60(), interval_100ms()),
            vec![GovernorEvent::Up]
        );
        assert_eq!(g.state(), PState::P1);
        // Another busy interval: up again (the instant ladder).
        assert_eq!(
            g.interval(6, hz60(), interval_100ms()),
            vec![GovernorEvent::Up]
        );
        assert_eq!(g.state(), PState::P2);
        assert_eq!(g.ups(), 2);
    }

    #[test]
    fn the_ladder_saturates_at_the_top() {
        let mut g = ClockGovernor::new();
        for _ in 0..10 {
            let _ = g.interval(6, hz60(), interval_100ms());
        }
        assert_eq!(g.state(), ClockGovernor::TOP);
        // Busy at the top: no event, no phantom rung.
        assert_eq!(g.interval(6, hz60(), interval_100ms()), vec![]);
        assert_eq!(g.ups(), 3);
    }

    #[test]
    fn down_needs_three_consecutive_quiet_intervals() {
        let mut g = ClockGovernor::new();
        // Climb to P2.
        let _ = g.interval(6, hz60(), interval_100ms());
        let _ = g.interval(6, hz60(), interval_100ms());
        assert_eq!(g.state(), PState::P2);
        // Two quiet intervals: not yet.
        assert_eq!(g.interval(0, hz60(), interval_100ms()), vec![]);
        assert_eq!(g.interval(0, hz60(), interval_100ms()), vec![]);
        assert_eq!(g.state(), PState::P2);
        // The third steps down.
        assert_eq!(
            g.interval(0, hz60(), interval_100ms()),
            vec![GovernorEvent::Down]
        );
        assert_eq!(g.state(), PState::P1);
        assert_eq!(g.downs(), 1);
    }

    #[test]
    fn a_busy_interval_breaks_the_quiet_streak() {
        let mut g = ClockGovernor::new();
        let _ = g.interval(6, hz60(), interval_100ms());
        assert_eq!(g.state(), PState::P1);
        // Two quiet, then a busy blip, then quiet again: the streak
        // restarted — the third consecutive quiet never accumulates.
        let _ = g.interval(0, hz60(), interval_100ms());
        let _ = g.interval(0, hz60(), interval_100ms());
        let _ = g.interval(6, hz60(), interval_100ms());
        assert_eq!(g.state(), PState::P2); // the blip stepped up
        let _ = g.interval(0, hz60(), interval_100ms());
        let _ = g.interval(0, hz60(), interval_100ms());
        assert_eq!(g.state(), PState::P2); // still two-of-three
        assert_eq!(
            g.interval(0, hz60(), interval_100ms()),
            vec![GovernorEvent::Down]
        );
    }

    #[test]
    fn half_rate_animation_holds_its_rung() {
        let mut g = ClockGovernor::new();
        // Climb to the top with full-rate work.
        for _ in 0..5 {
            let _ = g.interval(6, hz60(), interval_100ms());
        }
        assert_eq!(g.state(), ClockGovernor::TOP);
        // Half rate: 3/6 = 1/2 — the hold zone between the
        // watermarks. Neither streak grows; the rung persists.
        for _ in 0..20 {
            assert_eq!(g.interval(3, hz60(), interval_100ms()), vec![]);
        }
        assert_eq!(g.state(), ClockGovernor::TOP);
        assert_eq!(g.ups(), 3);
        assert_eq!(g.downs(), 0);
    }

    #[test]
    fn the_quarter_rate_slideshow_walks_down_patiently() {
        let mut g = ClockGovernor::new();
        for _ in 0..5 {
            let _ = g.interval(6, hz60(), interval_100ms());
        }
        assert_eq!(g.state(), ClockGovernor::TOP);
        // 1/6 < 1/3: quiet intervals, three at a time per step.
        for _ in 0..3 {
            let _ = g.interval(1, hz60(), interval_100ms());
        }
        assert_eq!(g.state(), PState::P2);
        for _ in 0..3 {
            let _ = g.interval(1, hz60(), interval_100ms());
        }
        assert_eq!(g.state(), PState::P1);
        for _ in 0..3 {
            let _ = g.interval(1, hz60(), interval_100ms());
        }
        assert_eq!(g.state(), PState::P0);
        // The floor holds.
        let _ = g.interval(1, hz60(), interval_100ms());
        assert_eq!(g.state(), PState::P0);
    }

    #[test]
    fn short_intervals_saturate_honestly() {
        let mut g = ClockGovernor::new();
        // A 10 ms interval (capacity 0 at 60 Hz): one flip is load 1.
        assert_eq!(g.interval(1, hz60(), 10), vec![GovernorEvent::Up]);
        // Zero flips in a short interval: load 0, quiet.
        let mut h = ClockGovernor::new();
        assert_eq!(h.interval(0, hz60(), 10), vec![]);
    }

    #[test]
    fn residency_counts_intervals_per_rung() {
        let mut g = ClockGovernor::new();
        let _ = g.interval(6, hz60(), interval_100ms()); // P0→P1
        let _ = g.interval(0, hz60(), interval_100ms());
        let _ = g.interval(0, hz60(), interval_100ms());
        let _ = g.interval(0, hz60(), interval_100ms()); // P1→P0
        let _ = g.interval(0, hz60(), interval_100ms());
        let r = g.residency();
        assert_eq!(r[0], 2); // the first (busy) + the last
        assert_eq!(r[1], 3); // the three at P1
        assert_eq!(r[2] + r[3], 0);
    }
}
