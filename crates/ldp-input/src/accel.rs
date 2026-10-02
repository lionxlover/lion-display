//! Pointer acceleration: the two-tier smooth curve, latency-first.
//!
//! The filter converts a stream of per-frame relative deltas into an
//! accelerated delta pair. The design contract (architecture §14):
//!
//! * **Latency-first:** velocity is estimated from the current event
//!   alone — distance over the inter-event interval. No smoothing
//!   window, no history lag; a single sample of jitter is cheaper
//!   than a frame of delay on the pointer path.
//! * **Two-tier smooth curve:** below `threshold` the factor is
//!   exactly 1 (the pointer is precise when slow); between
//!   `threshold` and `plateau_speed` the factor follows a smoothstep
//!   from 1 to `plateau_factor`; above, it is constant.
//!   Smoothstep's zero first derivative at both endpoints makes the
//!   tier boundaries C¹-continuous — the finger never feels a kink.
//! * **Direction preservation:** the accelerated vector is strictly a
//!   positive scalar multiple of the raw vector — the filter can
//!   change speed, never direction.
//!
//! The whole curve is a pure function of speed; the only state is the
//! previous event's timestamp (for the interval). That state is
//! injectable, so golden traces are deterministic.
//!
//! Properties (all tested below): the factor is monotonically
//! non-decreasing in speed, bounded by `[1, plateau_factor]`,
//! continuous at both tier boundaries, and zero input produces zero
//! output.

#![forbid(unsafe_code)]

use ldp_core::time::Mono;

/// The curve parameters.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct AccelProfile {
    /// Speeds at or below this (device counts per second) are
    /// unaccelerated.
    pub threshold: f32,
    /// The speed at which the plateau factor is fully reached.
    pub plateau_speed: f32,
    /// The maximum acceleration factor (always ≥ 1).
    pub plateau_factor: f32,
}

impl AccelProfile {
    /// The default curve: precise below 200 counts/s, saturating at
    /// 2500 counts/s with a 3.5× ceiling — the classic desktop feel.
    #[must_use]
    pub const fn default_profile() -> AccelProfile {
        AccelProfile {
            threshold: 200.0,
            plateau_speed: 2500.0,
            plateau_factor: 3.5,
        }
    }

    /// The acceleration factor for a speed (device counts per second).
    ///
    /// Pure: no state, no clocks. This is the function the property
    /// suite pins.
    #[must_use]
    pub fn factor(&self, speed: f32) -> f32 {
        if !(speed.is_finite()) || speed <= self.threshold {
            return 1.0;
        }
        if self.plateau_speed <= self.threshold || speed >= self.plateau_speed {
            return self.plateau_factor.max(1.0);
        }
        // The smoothstep tier: t in (0,1), factor in (1, plateau).
        let t = (speed - self.threshold) / (self.plateau_speed - self.threshold);
        let s = t * t * (3.0 - 2.0 * t);
        1.0 + s * (self.plateau_factor.max(1.0) - 1.0)
    }

    /// Validate the profile's shape (threshold < plateau speed,
    /// plateau factor ≥ 1, all finite and positive where required).
    #[must_use]
    pub fn is_sane(&self) -> bool {
        self.threshold.is_finite()
            && self.plateau_speed.is_finite()
            && self.plateau_factor.is_finite()
            && self.threshold >= 0.0
            && self.plateau_speed > self.threshold
            && self.plateau_factor >= 1.0
    }
}

/// One filtered motion: the raw delta and its accelerated form.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct AccelMotion {
    /// The unaccelerated delta (device counts).
    pub dx_unaccel: f32,
    /// The unaccelerated delta.
    pub dy_unaccel: f32,
    /// The accelerated delta.
    pub dx_accel: f32,
    /// The accelerated delta.
    pub dy_accel: f32,
    /// The factor applied.
    pub factor: f32,
}

/// The stateful filter: one event of history (the last timestamp).
#[derive(Clone, Debug)]
pub struct PointerAccel {
    profile: AccelProfile,
    last_time: Option<Mono>,
}

impl PointerAccel {
    /// A filter with a profile and no history.
    #[must_use]
    pub fn new(profile: AccelProfile) -> PointerAccel {
        PointerAccel {
            profile,
            last_time: None,
        }
    }

    /// The active profile.
    #[must_use]
    pub fn profile(&self) -> AccelProfile {
        self.profile
    }

    /// Forget the interval history (device reset, focus loss, drop).
    pub fn reset(&mut self) {
        self.last_time = None;
    }

    /// Filter one motion batch at `time`.
    ///
    /// The first event after a reset (or with a non-increasing
    /// timestamp) gets factor 1: guessing a velocity from nothing is
    /// how filters invent motion.
    ///
    /// # Panics
    /// Never: every input is finite arithmetic on owned state.
    #[must_use]
    #[allow(clippy::similar_names)]
    pub fn filter(&mut self, dx: i32, dy: i32, time: Mono) -> AccelMotion {
        self.filter_f32(dx as f32, dy as f32, time)
    }

    /// The float-delta form (touchpads speak millimeters).
    ///
    /// # Panics
    /// Never: every input is finite arithmetic on owned state.
    #[must_use]
    #[allow(clippy::similar_names)]
    pub fn filter_f32(&mut self, dx: f32, dy: f32, time: Mono) -> AccelMotion {
        let raw_dx = dx;
        let raw_dy = dy;
        let factor = match self.last_time {
            Some(last) if time.as_ns() > last.as_ns() => {
                let dt_s = (time.as_ns() - last.as_ns()) as f32 / 1e9;
                let speed = (raw_dx * raw_dx + raw_dy * raw_dy).sqrt() / dt_s;
                self.profile.factor(speed)
            }
            _ => 1.0,
        };
        self.last_time = Some(time);
        AccelMotion {
            dx_unaccel: raw_dx,
            dy_unaccel: raw_dy,
            dx_accel: raw_dx * factor,
            dy_accel: raw_dy * factor,
            factor,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: f32 = 1e9; // one second in ns, as the raw unit here.

    #[test]
    fn factor_is_bounded() {
        let p = AccelProfile::default_profile();
        for i in 0..=2000 {
            let v = i as f32 * 2.0; // 0..4000 counts/s
            let f = p.factor(v);
            assert!((1.0..=p.plateau_factor).contains(&f), "v={v} f={f}");
        }
        // Negative and NaN speeds are identity, not panics.
        assert_eq!(p.factor(-5.0), 1.0);
        assert_eq!(p.factor(f32::NAN), 1.0);
    }

    #[test]
    fn factor_is_monotone_non_decreasing() {
        let p = AccelProfile::default_profile();
        let mut last = 0.0f32;
        for i in 0..=5000 {
            let v = i as f32;
            let f = p.factor(v);
            assert!(f >= last, "v={v} f={f} last={last}");
            last = f;
        }
    }

    #[test]
    fn factor_is_continuous_at_boundaries() {
        let p = AccelProfile::default_profile();
        let eps = 1e-3;
        // Lower boundary: factor(threshold±ε) ≈ 1.
        for e in [eps, 10.0 * eps] {
            let below = p.factor(p.threshold - e);
            let above = p.factor(p.threshold + e);
            assert!((below - 1.0).abs() < 1e-6);
            assert!((above - 1.0).abs() < 1e-4, "above={above}");
        }
        // Upper boundary: factor(plateau±ε) ≈ plateau_factor.
        let below = p.factor(p.plateau_speed - eps);
        let above = p.factor(p.plateau_speed + eps);
        assert!((below - p.plateau_factor).abs() < 1e-4);
        assert_eq!(above, p.plateau_factor);
    }

    #[test]
    fn smoothstep_has_zero_slope_at_threshold() {
        // C¹ continuity at the tier join: the derivative from the
        // right of the threshold is ~0 (smoothstep f'(0)=0).
        let p = AccelProfile::default_profile();
        let eps = 1.0; // one count/s
        let slope = (p.factor(p.threshold + eps) - p.factor(p.threshold)) / eps;
        assert!(slope.abs() < 1e-2, "slope={slope}");
    }

    #[test]
    fn direction_is_preserved() {
        let mut acc = PointerAccel::new(AccelProfile::default_profile());
        let t0 = Mono::from_ns(0);
        // Prime the interval with one slow event.
        let _ = acc.filter(1, 0, t0);
        let mut t = t0;
        for &(dx, dy) in &[(40i32, 0i32), (0, -40), (30, 40), (-60, -80), (5, 5)] {
            t = t.saturating_add_ns(8_000_000); // 8 ms → fast
            let m = acc.filter(dx, dy, t);
            // Strictly positive scalar multiple of the raw vector.
            let k = m.factor;
            assert!(k > 0.0);
            assert!((m.dx_accel - dx as f32 * k).abs() < 1e-4);
            assert!((m.dy_accel - dy as f32 * k).abs() < 1e-4);
            // Cross product zero: same direction (or zero vector).
            let cross = m.dx_accel * m.dy_unaccel - m.dy_accel * m.dx_unaccel;
            assert!(cross.abs() < 1e-3, "cross={cross}");
        }
    }

    #[test]
    fn slow_motion_is_unaccelerated() {
        let mut acc = PointerAccel::new(AccelProfile::default_profile());
        let t0 = Mono::from_ns(0);
        let _ = acc.filter(1, 0, t0);
        // 2 counts per 50 ms = 40 counts/s: below the 200 threshold.
        let m = acc.filter(2, 1, t0.saturating_add_ns(50_000_000));
        assert_eq!(m.factor, 1.0);
        assert_eq!(m.dx_accel, 2.0);
        assert_eq!(m.dy_accel, 1.0);
    }

    #[test]
    fn fast_motion_reaches_the_plateau() {
        let mut acc = PointerAccel::new(AccelProfile::default_profile());
        let t0 = Mono::from_ns(0);
        let _ = acc.filter(1, 0, t0);
        // 50 counts per 8 ms = 6250 counts/s: above the plateau speed.
        let m = acc.filter(50, 0, t0.saturating_add_ns(8_000_000));
        assert_eq!(m.factor, 3.5);
        assert_eq!(m.dx_accel, 175.0);
    }

    #[test]
    fn zero_input_is_zero_output() {
        let mut acc = PointerAccel::new(AccelProfile::default_profile());
        let t = Mono::from_ns(S as u64);
        let m = acc.filter(0, 0, t);
        assert_eq!(m.dx_accel, 0.0);
        assert_eq!(m.dy_accel, 0.0);
    }

    #[test]
    fn first_event_and_repeated_timestamps_get_identity() {
        let mut acc = PointerAccel::new(AccelProfile::default_profile());
        let t = Mono::from_ns(S as u64);
        // First event: no interval, no guess.
        let m = acc.filter(100, 100, t);
        assert_eq!(m.factor, 1.0);
        // Non-increasing timestamp: no interval either.
        let m = acc.filter(100, 100, t);
        assert_eq!(m.factor, 1.0);
        // Reset clears history.
        let _ = acc.filter(100, 0, t.saturating_add_ns(8_000_000));
        acc.reset();
        let m = acc.filter(100, 100, t.saturating_add_ns(16_000_000));
        assert_eq!(m.factor, 1.0);
    }

    #[test]
    fn golden_curve_points() {
        let p = AccelProfile::default_profile();
        // Hand-checked points of the default curve.
        assert_eq!(p.factor(0.0), 1.0);
        assert_eq!(p.factor(200.0), 1.0);
        assert_eq!(p.factor(2500.0), 3.5);
        assert_eq!(p.factor(9000.0), 3.5);
        // Midpoint of the tier: smoothstep(0.5) = 0.5 → factor 2.25.
        let mid = p.factor(1350.0);
        assert!((mid - 2.25).abs() < 1e-5, "mid={mid}");
    }

    #[test]
    fn profile_sanity() {
        assert!(AccelProfile::default_profile().is_sane());
        assert!(!AccelProfile {
            threshold: 100.0,
            plateau_speed: 50.0,
            plateau_factor: 2.0
        }
        .is_sane());
        assert!(!AccelProfile {
            threshold: 100.0,
            plateau_speed: 200.0,
            plateau_factor: 0.5
        }
        .is_sane());
    }
}
