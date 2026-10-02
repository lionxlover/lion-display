//! Tone mapping — the BT.2390-structured EETF, and HDR→HDR clipping.
//!
//! The EETF (electrical-to-electrical transfer function) of ITU-R
//! BT.2390 §5.4 maps content mastered on one luminance range onto a
//! display with a narrower one. The structure implemented here, exactly
//! as the spec frames it:
//!
//! 1. **PQ-domain normalization** — master and display bounds are
//!    PQ-encoded, and the input luminance's PQ value is normalized over
//!    the *mastering* range (`u ∈ [0,1]`). PQ is approximately
//!    perceptually uniform, so this is the domain where "1:1 below the
//!    knee" means *perceptual* identity.
//! 2. **Knee at 75% of the display range** (in those normalized
//!    coordinates): below the knee the mapping is the exact identity
//!    (`v = u`, hence luminance-exact reproduction of dark and mid
//!    tones); above it, a rolloff carries the master's peak onto the
//!    display's peak.
//! 3. **A monotonicity-guaranteed rolloff**: a cubic Hermite with slope
//!    1 at the knee (C¹ continuity with the identity segment) and slope
//!    0 at the top, whenever the compression ratio permits it
//!    (Fritsch–Carlson condition `r ≥ 1/3`); otherwise — displays
//!    compressed more than 3:1 in PQ-normalized terms, where *any*
//!    slope-1-at-knee cubic is provably non-monotone — a smoothstep
//!    shoulder (monotone by construction, at the cost of a slope kink
//!    at the knee). Both branches share endpoints and semantics; the
//!    derivation is in the branch comments.
//!
//! Endpoints are exact: master black maps through (panels clamp below
//! their own black), master peak maps exactly onto display peak, and
//! the output never exceeds the display range. A display that covers
//! the mastering range disables tone mapping entirely.
//!
//! [`HdrToHdr`] is the BT.2390 §5.4.2 path for HDR targets: no EETF,
//! just peak clipping (documented there as the HDR→HDR guidance).

use crate::transfer::{pq_decode_f64, pq_encode_f64};

/// Construction failures of the tone mappers.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ToneError {
    /// `min >= max` (or non-positive bounds) in a luminance range.
    EmptyRange,
    /// Display peak at or below mastering black (nothing to map onto).
    InvertedRanges,
    /// Luminance above the 10 000-nit PQ ceiling.
    AboveCeiling,
}

impl core::fmt::Display for ToneError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::EmptyRange => write!(f, "luminance range needs 0 <= min < max"),
            Self::InvertedRanges => write!(f, "display peak below mastering black"),
            Self::AboveCeiling => write!(f, "luminance above the 10000-nit PQ ceiling"),
        }
    }
}

impl std::error::Error for ToneError {}

/// The BT.2390-structured EETF for one (master, display) pair.
#[derive(Clone, Copy, Debug)]
pub struct ToneMapper {
    master_min: f64,
    master_max: f64,
    display_max: f64,
    /// PQ of the mastering bounds.
    p_min_m: f64,
    p_max_m: f64,
    /// Display peak in normalized-master PQ coordinates.
    d_hi: f64,
    /// Knee (75% of the display range, normalized-master coords).
    knee: f64,
    /// Rolloff branch (see the module docs).
    smoothstep: bool,
    /// Display covers the master range: pure identity.
    identity: bool,
}

impl ToneMapper {
    /// Build the EETF for a mastering range and a display range, both
    /// in nits (`0 <= min < max <= 10000`).
    ///
    /// # Errors
    /// [`ToneError`] on empty/inverted ranges or bounds above the PQ
    /// ceiling.
    pub fn new(master: (f32, f32), display: (f32, f32)) -> Result<ToneMapper, ToneError> {
        let (m_min, m_max) = (f64::from(master.0), f64::from(master.1));
        let (d_min, d_max) = (f64::from(display.0), f64::from(display.1));
        if !(m_min >= 0.0 && m_min < m_max && d_min >= 0.0 && d_min < d_max) {
            return Err(ToneError::EmptyRange);
        }
        if m_max > 10_000.0 || d_max > 10_000.0 {
            return Err(ToneError::AboveCeiling);
        }
        if d_max <= m_min {
            return Err(ToneError::InvertedRanges);
        }
        let p_min_m = pq_encode_f64(m_min / 10_000.0);
        let p_max_m = pq_encode_f64(m_max / 10_000.0);
        let p_min_d = pq_encode_f64(d_min / 10_000.0);
        let p_max_d = pq_encode_f64(d_max / 10_000.0);
        let span = p_max_m - p_min_m;
        let d_lo = (p_min_d - p_min_m) / span;
        let d_hi = (p_max_d - p_min_m) / span;
        let identity = d_hi >= 1.0 && d_lo <= 0.0;
        let knee = d_lo + 0.75 * (d_hi - d_lo);
        // Fritsch–Carlson: a cubic Hermite over [x0,x1] with end slopes
        // m0, m1 is monotone when (m0/r)^2 + (m1/r)^2 <= 9 (r = chord
        // slope). With m0 = 1 (the C¹ knee) and m1 = 0 (the shoulder):
        // monotone iff r >= 1/3. Below that, smoothstep.
        let r = if (1.0 - knee) > 0.0 {
            (d_hi - knee) / (1.0 - knee)
        } else {
            1.0
        };
        Ok(ToneMapper {
            master_min: m_min,
            master_max: m_max,
            display_max: d_max,
            p_min_m,
            p_max_m,
            d_hi,
            knee,
            smoothstep: r < 1.0 / 3.0,
            identity,
        })
    }

    /// The EETF in normalized-master PQ coordinates (`u → v`).
    fn eetf(&self, u: f64) -> f64 {
        let u = u.clamp(0.0, 1.0);
        if self.identity || u <= self.knee {
            return u;
        }
        let (x0, y0) = (self.knee, self.knee);
        let (x1, y1) = (1.0, self.d_hi);
        if self.smoothstep {
            // S(t) = 3t² - 2t³: monotone, C0 at both ends.
            let t = (u - x0) / (x1 - x0);
            let s = t * t * (3.0 - 2.0 * t);
            y0 + (y1 - y0) * s
        } else {
            // Cubic Hermite, slope 1 at the knee (C¹ with the identity
            // segment), slope 0 at the display peak (the shoulder).
            let dx = x1 - x0;
            let t = (u - x0) / dx;
            let t2 = t * t;
            let t3 = t2 * t;
            (2.0 * t3 - 3.0 * t2 + 1.0) * y0
                + (t3 - 2.0 * t2 + t) * dx
                + (-2.0 * t3 + 3.0 * t2) * y1
            // (t3 - t2) * m1 * dx with m1 = 0 drops out.
        }
    }

    /// Map one absolute luminance (nits) through the EETF.
    ///
    /// Input is clipped to the mastering range first (the BT.2390
    /// input-clip rule): content above master peak lands exactly on
    /// display peak; content below mastering black passes through
    /// unchanged (panels clamp their own black; boosting sub-black
    /// chroma would be worse than the invisible error).
    #[must_use]
    pub fn map_luminance(&self, y: f32) -> f32 {
        let y64 = f64::from(y);
        if y64 <= self.master_min {
            return y; // below mastering black: unchanged
        }
        let y_eff = y64.min(self.master_max);
        let p = pq_encode_f64(y_eff / 10_000.0);
        let u = (p - self.p_min_m) / (self.p_max_m - self.p_min_m);
        let v = self.eetf(u);
        let p_out = self.p_min_m + v * (self.p_max_m - self.p_min_m);
        (pq_decode_f64(p_out) * 10_000.0) as f32
    }

    /// The per-pixel luminance scale: `map_luminance(y) / y`.
    ///
    /// The pipeline applies this scale to all three channels (the EETF
    /// is a luminance-domain map; hue is preserved by construction).
    /// Black maps to scale 1; the scale is bounded, never infinite.
    #[must_use]
    pub fn luminance_scale(&self, y: f32) -> f32 {
        if y <= 0.0 {
            return 1.0;
        }
        let out = self.map_luminance(y);
        if out <= 0.0 {
            return 1.0;
        }
        (out / y).min(4.0)
    }

    /// The mastering range (nits).
    #[must_use]
    pub fn mastering(&self) -> (f32, f32) {
        (self.master_min as f32, self.master_max as f32)
    }

    /// The display range (nits).
    #[must_use]
    pub fn display(&self) -> (f32, f32) {
        (self.master_min as f32, self.display_max as f32)
    }
}

/// HDR→HDR luminance handling: peak clipping only (BT.2390's guidance
/// when the target is itself an HDR display — no EETF, just never
/// exceed the panel).
///
/// Blacks are left alone: the panel clamps near-black itself, and
/// scaling up sub-panel blacks would multiply chroma (hue shifts).
#[derive(Clone, Copy, Debug)]
pub struct HdrToHdr {
    max_nits: f64,
}

impl HdrToHdr {
    /// Build for a panel peak in nits (`0 < max <= 10000`).
    ///
    /// # Errors
    /// [`ToneError::EmptyRange`] / [`ToneError::AboveCeiling`] on bad
    /// bounds.
    pub fn new(max_nits: f32) -> Result<HdrToHdr, ToneError> {
        let max = f64::from(max_nits);
        if max <= 0.0 || max.is_nan() {
            return Err(ToneError::EmptyRange);
        }
        if max > 10_000.0 {
            return Err(ToneError::AboveCeiling);
        }
        Ok(HdrToHdr { max_nits: max })
    }

    /// The panel peak (nits).
    #[must_use]
    pub fn max_nits(&self) -> f32 {
        self.max_nits as f32
    }

    /// Map one absolute luminance (nits): clipped at the panel peak,
    /// unchanged below it.
    #[must_use]
    pub fn map_luminance(&self, y: f32) -> f32 {
        y.min(self.max_nits as f32)
    }

    /// The per-pixel luminance scale ([`ToneMapper::luminance_scale`]'s
    /// contract): `max_nits/y` above the peak, else 1.
    #[must_use]
    pub fn luminance_scale(&self, y: f32) -> f32 {
        if y <= self.max_nits as f32 {
            1.0
        } else {
            (self.max_nits / f64::from(y)) as f32
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn construction_validates_ranges() {
        assert!(matches!(
            ToneMapper::new((100.0, 50.0), (0.005, 100.0)),
            Err(ToneError::EmptyRange)
        ));
        assert!(matches!(
            ToneMapper::new((0.005, 1000.0), (0.005, 0.004)),
            Err(ToneError::EmptyRange)
        ));
        assert!(ToneMapper::new((0.005, 1000.0), (0.005, 1000.0)).is_ok());
        assert!(matches!(
            ToneMapper::new((0.005, 20_000.0), (0.005, 100.0)),
            Err(ToneError::AboveCeiling)
        ));
        // Display peak below master black: nothing to map onto.
        assert!(matches!(
            ToneMapper::new((0.005, 1000.0), (0.0001, 0.004)),
            Err(ToneError::InvertedRanges)
        ));
    }

    #[test]
    fn classic_1000_to_100_curve() {
        // The canonical SDR-target case. Endpoints and anchors:
        let tm = ToneMapper::new((0.005, 1000.0), (0.005, 100.0)).unwrap();
        // Master peak lands exactly on display peak.
        assert!((tm.map_luminance(1000.0) - 100.0).abs() < 0.05);
        // Below the knee (≈ 27.6 nits for this pair) is
        // luminance-exact (PQ-domain identity).
        for y in [0.01f32, 0.5, 5.0, 20.0] {
            let out = tm.map_luminance(y);
            assert!((out - y).abs() < 1e-3, "below-knee identity at {y}: {out}");
        }
        // BT.2408 reference white (203 nits) lands in the mid-80s on a
        // 100-nit display (hand-derived ≈ 87.6 in the module docs).
        let white = tm.map_luminance(203.0);
        assert!((85.0..=90.0).contains(&white), "203-nit anchor: {white}");
        // Monotone over the full range.
        let mut prev = -1.0;
        for i in 0..=1000 {
            let y = i as f64 / 10.0;
            let out = tm.map_luminance(y as f32);
            assert!(out >= prev - 1e-4, "monotone at {y}");
            assert!(out <= 100.0 + 1e-3, "range containment at {y}");
            prev = out;
        }
    }

    #[test]
    fn extreme_compression_takes_the_smoothstep_branch() {
        // 10000 -> 100: compression ratio r < 1/3 forces smoothstep.
        let tm = ToneMapper::new((0.005, 10_000.0), (0.005, 100.0)).unwrap();
        assert!(tm.smoothstep, "extreme compression uses smoothstep");
        assert!(tm.map_luminance(10_000.0) <= 100.0 + 1e-3);
        let mut prev = -1.0;
        for i in 0..=500 {
            let y = i as f64 / 20.0;
            let out = tm.map_luminance(y as f32);
            assert!(out >= prev - 1e-4, "monotone at {y}");
            prev = out;
        }
    }

    #[test]
    fn identity_when_display_covers_master() {
        let tm = ToneMapper::new((0.005, 1000.0), (0.005, 4000.0)).unwrap();
        assert!(tm.identity);
        for y in [0.01f32, 1.0, 203.0, 999.0, 1000.0] {
            assert!((tm.map_luminance(y) - y).abs() < 1e-3, "identity at {y}");
            assert!((tm.luminance_scale(y) - 1.0).abs() < 1e-4);
        }
    }

    #[test]
    fn luminance_scale_properties() {
        let tm = ToneMapper::new((0.005, 1000.0), (0.005, 100.0)).unwrap();
        // Black and sub-black: scale exactly 1 (never a blow-up).
        assert_eq!(tm.luminance_scale(0.0), 1.0);
        assert_eq!(tm.luminance_scale(0.001), 1.0);
        // Below the knee: scale ≈ 1 (luminance-exact region).
        assert!((tm.luminance_scale(10.0) - 1.0).abs() < 1e-3);
        // Above master peak: scale = display_max / y exactly.
        let s = tm.luminance_scale(2000.0);
        assert!((s - 100.0 / 2000.0).abs() < 1e-5);
        // In between: bounded, positive.
        for i in 1..=400 {
            let y = i as f32;
            let s = tm.luminance_scale(y);
            assert!(s > 0.0 && s <= 1.05, "scale bounded at {y}: {s}");
        }
    }

    #[test]
    fn randomized_ranges_are_monotone_and_bounded() {
        // LCG corpus (the project's no-rand doctrine).
        let mut seed = 0x5eed_1234u64;
        let mut next = || {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (seed >> 33) as f64 / f64::from(u32::MAX)
        };
        for _ in 0..200 {
            let m_min = 0.005 + next() * 0.5;
            let m_max = (m_min + 1.0 + next() * 9_000.0).min(10_000.0);
            let d_min = 0.005 + next() * 0.5;
            let d_max = (d_min + 0.5 + next() * (m_max - d_min)).min(10_000.0);
            let Ok(tm) =
                ToneMapper::new((m_min as f32, m_max as f32), (d_min as f32, d_max as f32))
            else {
                continue; // display peak <= master black: skip
            };
            let mut prev = -1.0;
            for i in 0..=200 {
                let y = (m_min + (f64::from(i) / 200.0) * (m_max - m_min)) as f32;
                let out = tm.map_luminance(y);
                assert!(out >= prev - 1e-3, "monotone {m_min}..{m_max} → {d_max}");
                assert!(out <= d_max as f32 + 0.1, "bounded by display peak");
                prev = out;
            }
            // Master peak exactly onto display peak.
            let peak = tm.map_luminance(m_max as f32);
            assert!((peak - d_max as f32).abs() < 0.2, "peak endpoint {peak}");
        }
    }

    #[test]
    fn hdr_to_hdr_clips_only_the_top() {
        let h = HdrToHdr::new(600.0).unwrap();
        assert_eq!(h.map_luminance(100.0), 100.0);
        assert_eq!(h.map_luminance(600.0), 600.0);
        assert_eq!(h.map_luminance(4000.0), 600.0);
        assert_eq!(h.luminance_scale(0.0), 1.0);
        assert_eq!(h.luminance_scale(100.0), 1.0);
        assert!((h.luminance_scale(1200.0) - 0.5).abs() < 1e-6);
        assert!(matches!(HdrToHdr::new(0.0), Err(ToneError::EmptyRange)));
        assert!(matches!(
            HdrToHdr::new(20_000.0),
            Err(ToneError::AboveCeiling)
        ));
    }
}
