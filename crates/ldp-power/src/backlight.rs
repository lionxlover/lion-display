//! Backlight ramping — flicker-free level transitions.
//!
//! `set_brightness` never jumps: the level ramps toward the target in
//! fixed steps at a fixed interval (defaults: 16 steps at 20 ms — a
//! ~320 ms ramp, fast enough to feel responsive, slow enough that the
//! panel does not flicker). The dim stage of the idle machine and the
//! user-facing brightness slider both ride this one path.
//!
//! Properties (corpus-tested in tests/reanchor.rs):
//!
//! * Levels move strictly monotonically toward the target while a ramp
//!   is in flight, never overshooting, converging exactly.
//! * Every emitted level is within 0..=max.
//! * A new target set mid-ramp redirects from the *current* level
//!   without a jump (the ramp restarts from where the panel is).

use ldp_core::time::Mono;

/// Backlight ramp state.
#[derive(Clone, Debug)]
pub struct BacklightRamp {
    current: u32,
    target: u32,
    max: u32,
    step_interval_ns: u64,
    last_step: Option<Mono>,
}

/// The default step interval (20 ms).
pub const DEFAULT_STEP_MS: u64 = 20;

/// The default step count (16 — the ~320 ms ramp).
pub const DEFAULT_STEPS: u64 = 16;

impl BacklightRamp {
    /// A backlight at `level` of `max`, with the default ramp profile.
    #[must_use]
    pub fn new(level: u32, max: u32) -> Self {
        let max = max.max(1);
        BacklightRamp {
            current: level.min(max),
            target: level.min(max),
            max,
            step_interval_ns: DEFAULT_STEP_MS * 1_000_000,
            last_step: None,
        }
    }

    /// The current level.
    #[must_use]
    pub const fn level(&self) -> u32 {
        self.current
    }

    /// The target level.
    #[must_use]
    pub const fn target(&self) -> u32 {
        self.target
    }

    /// The maximum level.
    #[must_use]
    pub const fn max(&self) -> u32 {
        self.max
    }

    /// Whether a ramp is in flight.
    #[must_use]
    pub const fn ramping(&self) -> bool {
        self.current != self.target
    }

    /// Set a new target (clamped to max). The ramp starts from the
    /// current level at the next tick — no jump.
    pub fn set_target(&mut self, target: u32, now: Mono) {
        self.target = target.min(self.max);
        self.last_step = Some(now);
    }

    /// Advance one step if the interval elapsed. Returns the new level
    /// when it changed, `None` otherwise.
    ///
    /// The first step happens at the first tick at least one interval
    /// after `set_target`; subsequent steps at multiples of the
    /// interval (ticks may be irregular — the ramp tracks absolute
    /// time).
    pub fn tick(&mut self, now: Mono) -> Option<u32> {
        if self.current == self.target {
            return None;
        }
        let anchor = self.last_step?;
        if now.duration_since(anchor) < self.step_interval_ns {
            return None;
        }
        // How many intervals elapsed (catch-up for irregular ticks).
        let elapsed = now.as_ns().saturating_sub(anchor.as_ns());
        let intervals = elapsed / self.step_interval_ns.max(1);
        // Step size: the remaining distance split over the remaining
        // default step count, at least 1.
        let distance = self.target.abs_diff(self.current);
        let remaining_steps = DEFAULT_STEPS
            .saturating_sub(intervals.min(DEFAULT_STEPS))
            .max(1);
        let step = (distance / remaining_steps as u32).max(1);
        let before = self.current;
        if self.target > self.current {
            self.current = self.current.saturating_add(step).min(self.target);
        } else {
            self.current = self.current.saturating_sub(step).max(self.target);
        }
        self.last_step = Some(now);
        (self.current != before).then_some(self.current)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(m: u64) -> Mono {
        Mono::from_ms(m)
    }

    #[test]
    fn ramp_converges_without_overshoot() {
        let mut ramp = BacklightRamp::new(400, 1000);
        ramp.set_target(1000, ms(0));
        let mut levels = Vec::new();
        let mut t = 20u64;
        while let Some(level) = ramp.tick(ms(t)) {
            levels.push(level);
            t += 20;
        }
        assert_eq!(ramp.level(), 1000);
        assert_eq!(*levels.last().unwrap(), 1000);
        // Strictly increasing toward the target, all in range.
        for pair in levels.windows(2) {
            assert!(pair[1] > pair[0], "non-monotonic ramp {levels:?}");
        }
        assert!(levels.iter().all(|l| *l <= 1000));
    }

    #[test]
    fn downward_ramp_is_monotone_too() {
        let mut ramp = BacklightRamp::new(900, 1000);
        ramp.set_target(0, ms(0));
        let mut levels = Vec::new();
        let mut t = 20u64;
        while let Some(level) = ramp.tick(ms(t)) {
            levels.push(level);
            t += 20;
        }
        assert_eq!(ramp.level(), 0);
        for pair in levels.windows(2) {
            assert!(pair[1] < pair[0], "non-monotonic down-ramp {levels:?}");
        }
    }

    #[test]
    fn retarget_mid_ramp_does_not_jump() {
        let mut ramp = BacklightRamp::new(0, 1000);
        ramp.set_target(1000, ms(0));
        // Run two steps.
        assert!(ramp.tick(ms(20)).is_some());
        assert!(ramp.tick(ms(40)).is_some());
        // Retarget down mid-ramp: continues from the current level
        // (no jump at the retarget instant).
        let level_before = ramp.level();
        ramp.set_target(0, ms(40));
        assert_eq!(ramp.level(), level_before);
        let mut t = 60u64;
        while ramp.tick(ms(t)).is_some() {
            t += 20;
        }
        assert_eq!(ramp.level(), 0);
    }

    #[test]
    fn clamping_and_noop_ticks() {
        let mut ramp = BacklightRamp::new(500, 1000);
        ramp.set_target(5000, ms(0)); // clamped to max
        assert_eq!(ramp.target(), 1000);
        // Before the first interval: no step.
        assert!(ramp.tick(ms(19)).is_none());
        // First step lands strictly between 500 and 1000.
        let first = ramp.tick(ms(20)).unwrap();
        assert!(first > 500 && first < 1000);
        // Run to convergence.
        let mut t = 40u64;
        while ramp.tick(ms(t)).is_some() {
            t += 20;
        }
        assert_eq!(ramp.level(), 1000);
        // At target: no more steps.
        assert!(ramp.tick(ms(1000)).is_none());
        assert!(ramp.tick(ms(2000)).is_none());
    }
}
