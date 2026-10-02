//! Fractional scaling.
//!
//! Scale factors on the wire are Q8.8 rationals (`scale_q8`: value / 256)
//! — deterministic across peers, immune to float parsing drift, exact for
//! the common steps (0.5x increments and every integer up to 255.996).

use core::fmt;

/// A scale factor in 1/256 units.
///
/// ```
/// use ldp_core::ScaleFactor;
/// let s = ScaleFactor::from_q8(384).unwrap(); // 1.5x
/// assert_eq!(s.to_f32(), 1.5);
/// assert_eq!(s.scale_px(100), 150);
/// let s = ScaleFactor::from_f32_lossy(2.0).unwrap();
/// assert_eq!(s.to_q8(), 512);
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
#[repr(transparent)]
pub struct ScaleFactor {
    q8: u32,
}

impl ScaleFactor {
    /// 1:1 scale.
    pub const IDENTITY: ScaleFactor = ScaleFactor { q8: 256 };

    /// Construct from Q8.8; zero is rejected (it is not a scale).
    pub const fn from_q8(q8: u32) -> Option<ScaleFactor> {
        if q8 == 0 {
            None
        } else {
            Some(ScaleFactor { q8 })
        }
    }

    /// Approximate a float scale (e.g. 1.25); rejects non-finite and
    /// non-positive values, rounds to the nearest 1/256 step.
    pub fn from_f32_lossy(v: f32) -> Option<ScaleFactor> {
        if !v.is_finite() || v <= 0.0 {
            return None;
        }
        let q8 = (v * 256.0).round();
        if !(0.0..=u32::MAX as f32).contains(&q8) || q8 < 1.0 {
            return None;
        }
        Some(ScaleFactor { q8: q8 as u32 })
    }

    /// Raw Q8.8 wire value.
    #[must_use]
    pub const fn to_q8(self) -> u32 {
        self.q8
    }

    /// Exact binary fraction as f32 (Q8.8 is representable exactly).
    #[must_use]
    pub const fn to_f32(self) -> f32 {
        self.q8 as f32 / 256.0
    }

    /// Whether this is 1:1.
    #[must_use]
    pub const fn is_identity(self) -> bool {
        self.q8 == 256
    }

    /// Scale a logical length into device pixels, rounding up (buffer
    /// allocation must never lose a pixel).
    #[must_use]
    pub fn scale_px_up(self, logical: u32) -> u32 {
        let scaled = (u64::from(logical) * u64::from(self.q8)).div_ceil(256);
        scaled.min(u64::from(u32::MAX)) as u32
    }

    /// Scale a logical length into device pixels, rounding to nearest.
    #[must_use]
    pub fn scale_px(self, logical: u32) -> u32 {
        let scaled = (u64::from(logical) * u64::from(self.q8) + 128) / 256;
        scaled.min(u64::from(u32::MAX)) as u32
    }

    /// Inverse-scale a device length into logical pixels, rounding down
    /// (logical space must fit inside the device allocation).
    #[must_use]
    pub fn unscale_px(self, device: u32) -> u32 {
        ((u64::from(device) * 256) / u64::from(self.q8)).min(u64::from(u32::MAX)) as u32
    }
}

impl fmt::Display for ScaleFactor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_f32())
    }
}

impl Default for ScaleFactor {
    fn default() -> Self {
        Self::IDENTITY
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn q8_representations() {
        assert_eq!(ScaleFactor::from_q8(0), None);
        assert_eq!(ScaleFactor::from_q8(256).unwrap(), ScaleFactor::IDENTITY);
        assert_eq!(ScaleFactor::from_q8(384).unwrap().to_f32(), 1.5);
        assert_eq!(ScaleFactor::from_q8(128).unwrap().to_f32(), 0.5);
        assert_eq!(ScaleFactor::from_q8(512).unwrap().to_f32(), 2.0);
        assert!(ScaleFactor::from_q8(256).unwrap().is_identity());
    }

    #[test]
    fn float_conversion_rejects_garbage() {
        assert!(ScaleFactor::from_f32_lossy(0.0).is_none());
        assert!(ScaleFactor::from_f32_lossy(-1.0).is_none());
        assert!(ScaleFactor::from_f32_lossy(f32::NAN).is_none());
        assert!(ScaleFactor::from_f32_lossy(f32::INFINITY).is_none());
        assert_eq!(ScaleFactor::from_f32_lossy(1.25).unwrap().to_q8(), 320);
        assert_eq!(ScaleFactor::from_f32_lossy(1.5).unwrap().to_q8(), 384);
    }

    #[test]
    fn pixel_math_is_exact_for_common_scales() {
        let half = ScaleFactor::from_q8(128).unwrap();
        assert_eq!(half.scale_px(100), 50);
        assert_eq!(half.scale_px_up(101), 51); // 50.5 rounds up
        let one_half = ScaleFactor::from_q8(384).unwrap();
        assert_eq!(one_half.scale_px(100), 150);
        assert_eq!(one_half.scale_px_up(101), 152); // 151.5 rounds up
        assert_eq!(one_half.unscale_px(150), 100);
        let two = ScaleFactor::from_q8(512).unwrap();
        assert_eq!(two.scale_px(333), 666);
        assert_eq!(two.unscale_px(666), 333);
    }

    #[test]
    fn upscaling_never_loses_pixels() {
        for q8 in [128u32, 256, 320, 384, 512, 640] {
            let s = ScaleFactor::from_q8(q8).unwrap();
            for logical in 1..2000u32 {
                let up = s.scale_px_up(logical);
                assert!(
                    f64::from(up) >= f64::from(logical) * f64::from(s.to_f32()),
                    "under-allocation"
                );
            }
        }
    }

    #[test]
    fn default_is_identity() {
        assert_eq!(ScaleFactor::default(), ScaleFactor::IDENTITY);
        assert_eq!(ScaleFactor::IDENTITY.to_string(), "1");
    }
}
