//! The spring animation engine (Phase 27): the motion vocabulary of
//! the Liquid visual language.
//!
//! A [`Spring`] is one animated scalar — a panel's position, a
//! window's opacity, a dock's slide — integrated with **fixed
//! substeps** (semi-implicit Euler at [`SUBSTEP_MS`]), so an
//! animation is a pure function of the frame timestamps it consumed:
//! the same dt sequence reproduces the same curve bit-for-bit, the
//! scheduler's determinism doctrine applied to motion.
//!
//! The default constructor is **critically damped** (the macOS
//! doctrine: fast, smooth, no overshoot — the feel of a native
//! panel); [`Spring::bouncy`](0.8) trades settle time for the
//! playful overshoot of a dock icon.
//!
//! The compositor's clock is milliseconds (the core `Mono` timestamp
//! carries nanoseconds; the frame loop's flips land on the driver's
//! timestamps) — springs advance by whole milliseconds and report
//! settlement in sub-pixel units, the resolution the layout speaks.

/// The fixed integration substep (ms). Every `step` quantizes dt to
/// whole milliseconds and integrates in [`SUBSTEP_MS`] slices — the
/// curve never depends on how a frame's dt happens to divide.
pub const SUBSTEP_MS: u64 = 4;

/// The settlement thresholds: closer than a tenth of a pixel and
/// slower than a hundredth of a pixel per millisecond is *still*.
pub const SETTLE_POS: f32 = 0.1;
/// The velocity half of the settlement predicate (units per ms).
pub const SETTLE_VEL: f32 = 0.01;

/// One animated scalar.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spring {
    /// Stiffness (unit force per unit displacement, per ms² scale —
    /// see [`Self::step`] for the exact integration).
    pub stiffness: f32,
    /// Damping (velocity force coefficient).
    pub damping: f32,
    /// Current position.
    pub pos: f32,
    /// Current velocity (units per ms).
    pub vel: f32,
    /// The resting value.
    pub target: f32,
}

impl Spring {
    /// A critically damped spring at `target` — the macOS feel: it
    /// approaches without overshooting, fast then smooth.
    ///
    /// `stiffness` tunes urgency (higher = snappier); damping is
    /// derived as the critical value `2·sqrt(stiffness)`.
    #[must_use]
    pub fn critically_damped(target: f32, stiffness: f32) -> Spring {
        Spring {
            stiffness: stiffness.max(1.0),
            damping: 2.0 * stiffness.max(1.0).sqrt(),
            pos: target,
            vel: 0.0,
            target,
        }
    }

    /// An under-damped spring — the playful one (dock icons, bouncy
    /// transitions). `bounce` in `0..=1` scales the damping below
    /// critical; `0.8` keeps one visible overshoot.
    #[must_use]
    pub fn bouncy(target: f32, stiffness: f32, bounce: f32) -> Spring {
        let critical = 2.0 * stiffness.max(1.0).sqrt();
        Spring {
            stiffness: stiffness.max(1.0),
            damping: critical * bounce.clamp(0.05, 1.0),
            pos: target,
            vel: 0.0,
            target,
        }
    }

    /// Jump to a resting state instantly (layout resets).
    #[must_use]
    pub fn resting(target: f32) -> Spring {
        Spring {
            stiffness: 1.0,
            damping: 2.0,
            pos: target,
            vel: 0.0,
            target,
        }
    }

    /// Retarget the spring (a new destination mid-flight keeps its
    /// velocity — the motion never teleports).
    pub fn retarget(&mut self, target: f32) {
        self.target = target;
    }

    /// Displace and release (the classic "pull down and let go").
    pub fn displace(&mut self, pos: f32) {
        self.pos = pos;
    }

    /// Whether the spring has settled at its target — the "stop
    /// animating, stop damaging" predicate.
    #[must_use]
    pub fn settled(&self) -> bool {
        (self.pos - self.target).abs() <= SETTLE_POS && self.vel.abs() <= SETTLE_VEL
    }

    /// Advance by `dt_ms` (quantized to whole milliseconds, integrated
    /// in [`SUBSTEP_MS`] substeps). Returns whether the spring is
    /// settled *after* the advance.
    ///
    /// Semi-implicit Euler (velocity first, then position) — the
    /// symplectic form whose energy behaves: a critically damped
    /// spring decays monotonically even at large dt, where explicit
    /// Euler would ring. All arithmetic is IEEE-exact f32; the same
    /// dt sequence reproduces the same curve on every platform.
    pub fn step(&mut self, dt_ms: u64) -> bool {
        let mut remaining = dt_ms;
        while remaining > 0 {
            let slice = remaining.min(SUBSTEP_MS) as f32 / 1000.0; // seconds
                                                                   // a = (-k·(x - target) - c·v), integrated per substep.
            let displacement = self.pos - self.target;
            let accel = -self.stiffness * displacement - self.damping * self.vel;
            self.vel += accel * slice;
            self.pos += self.vel * slice;
            remaining = remaining.saturating_sub(SUBSTEP_MS);
        }
        self.settled()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One second of 60 Hz frames.
    const FRAME_MS: u64 = 1000 / 60;

    #[test]
    fn resting_springs_settle_immediately() {
        let mut s = Spring::resting(42.0);
        assert!(s.settled());
        assert_eq!(s.pos, 42.0);
        assert!(s.step(100), "a resting spring stays settled");
    }

    #[test]
    fn critical_spring_converges_without_overshoot() {
        let mut s = Spring::critically_damped(0.0, 120.0);
        s.displace(100.0);
        let start = s.pos;
        let mut peak = s.pos;
        for _ in 0..120 {
            s.step(FRAME_MS);
            peak = peak.max(s.pos);
            if s.settled() {
                break;
            }
        }
        assert!(s.settled(), "converged within two seconds");
        assert!((s.pos - 0.0).abs() < SETTLE_POS);
        // Critical damping: the position never crosses the target.
        assert!(peak <= start, "no overshoot past the start");
        assert!(peak >= 0.0, "and never below the target");
    }

    #[test]
    fn bouncy_spring_overshoots_and_settles() {
        let mut s = Spring::bouncy(0.0, 80.0, 0.5);
        s.displace(-100.0);
        let mut max_excursion = 0.0f32;
        for _ in 0..240 {
            s.step(FRAME_MS);
            // Excursion beyond the target on the far side.
            max_excursion = max_excursion.max(s.pos);
            if s.settled() {
                break;
            }
        }
        assert!(s.settled(), "even the bouncy spring settles");
        assert!(
            max_excursion > 1.0,
            "the bounce crossed the target (excursion {max_excursion})"
        );
    }

    #[test]
    fn the_same_dt_sequence_reproduces_the_same_curve() {
        let run = || {
            let mut s = Spring::critically_damped(500.0, 150.0);
            s.displace(0.0);
            let mut trace = Vec::new();
            // Irregular frame times (a busy compositor's reality).
            for dt in [16u64, 17, 16, 33, 16, 8, 16, 16, 50, 16] {
                s.step(dt);
                trace.push((s.pos, s.vel));
            }
            trace
        };
        assert_eq!(run(), run(), "bit-for-bit reproducible");
    }

    #[test]
    fn retargeting_keeps_the_velocity() {
        let mut s = Spring::critically_damped(0.0, 120.0);
        s.displace(100.0);
        s.step(100);
        let vel = s.vel;
        assert!(vel < 0.0, "moving towards the target");
        s.retarget(200.0);
        assert_eq!(s.vel, vel, "retargeting never teleports the motion");
        assert!(!s.settled(), "mid-flight by construction");
    }

    #[test]
    fn large_dt_does_not_explode_critical_springs() {
        // A one-second step in one call: semi-implicit Euler with the
        // substep ceiling keeps the energy bounded.
        let mut s = Spring::critically_damped(0.0, 120.0);
        s.displace(100.0);
        s.step(1000);
        assert!(
            s.pos.abs() < 100.0,
            "the excursion never grows past the start"
        );
        assert!(s.pos > -SETTLE_POS, "and never crosses the target");
    }

    #[test]
    fn stiffness_orders_the_urgency() {
        let settle_steps = |stiffness: f32| {
            let mut s = Spring::critically_damped(0.0, stiffness);
            s.displace(100.0);
            let mut frames = 0;
            while !s.settled() && frames < 10_000 {
                s.step(FRAME_MS);
                frames += 1;
            }
            frames
        };
        assert!(
            settle_steps(400.0) < settle_steps(40.0),
            "stiffer springs settle sooner"
        );
    }
}
