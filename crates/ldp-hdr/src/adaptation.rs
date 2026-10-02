//! Luminance adaptation — the SDR↔HDR anchoring policy.
//!
//! Two seams need explicit anchoring (architecture §12, "the luminance
//! adaptation ... matching the LionOS appearance pipeline"):
//!
//! * **SDR-in-HDR**: an HDR output compositing SDR surfaces must place
//!   SDR reference white somewhere sensible instead of leaving it at
//!   whatever the SDR description's own reference implies. The
//!   [`LuminanceAdapter`] carries that policy: a canvas level in nits
//!   (default 203 — the BT.2408 convention), monotone in both the
//!   input value and the canvas level (the exit-criterion property),
//!   with the exact identity at the SDR reference itself.
//! * **HLG display adaptation**: the OOTF system gamma depends on the
//!   display's peak luminance — BT.2100's formula
//!   `γ = 1.2 + 0.42·log₁₀(Lw / 1000)` — implemented by
//!   [`hlg_system_gamma`] with the published anchors pinned (1000 nits
//!   → 1.2, 2000 → ≈ 1.3264, 400 → ≈ 1.0329).

use ldp_core::color::Luminance;

/// The BT.2408 SDR reference white (203 nits) — the default canvas.
pub const SDR_REFERENCE_NITS: f32 = 203.0;

/// Adaptation construction failures.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AdaptationError {
    /// Canvas level outside (0, 10 000] nits.
    BadCanvas,
    /// Peak luminance outside (0, 10 000] nits.
    BadPeak,
}

impl core::fmt::Display for AdaptationError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::BadCanvas => write!(f, "canvas level outside (0, 10000] nits"),
            Self::BadPeak => write!(f, "peak luminance outside (0, 10000] nits"),
        }
    }
}

impl std::error::Error for AdaptationError {}

/// The SDR-in-HDR canvas: where SDR white lands on the HDR output.
#[derive(Clone, Copy, Debug)]
pub struct LuminanceAdapter {
    canvas_nits: f32,
}

impl Default for LuminanceAdapter {
    fn default() -> Self {
        // BT.2408: 203 nits.
        Self {
            canvas_nits: SDR_REFERENCE_NITS,
        }
    }
}

impl LuminanceAdapter {
    /// A canvas at `canvas_nits` (where SDR reference white lands).
    ///
    /// # Errors
    /// [`AdaptationError::BadCanvas`] outside (0, 10 000].
    pub fn new(canvas_nits: f32) -> Result<LuminanceAdapter, AdaptationError> {
        if !(canvas_nits > 0.0 && canvas_nits <= 10_000.0) {
            return Err(AdaptationError::BadCanvas);
        }
        Ok(LuminanceAdapter { canvas_nits })
    }

    /// A canvas from wire luminance.
    ///
    /// # Errors
    /// [`AdaptationError::BadCanvas`] outside (0, 10 000].
    pub fn from_luminance(canvas: Luminance) -> Result<LuminanceAdapter, AdaptationError> {
        let nits = f64::from(canvas.as_wire_units()) / 10_000.0;
        Self::new(nits as f32)
    }

    /// The canvas level (nits).
    #[must_use]
    pub fn canvas_nits(&self) -> f32 {
        self.canvas_nits
    }

    /// Anchor SDR-relative `0..=1` (1.0 = the SDR description's
    /// reference white) into absolute nits.
    ///
    /// The per-pixel scale for a whole SDR surface is
    /// [`LuminanceAdapter::sdr_to_absolute`] applied channel-wise.
    #[must_use]
    pub fn sdr_to_absolute(&self, relative: f32) -> f32 {
        (relative.clamp(0.0, 1.0) * self.canvas_nits).clamp(0.0, 10_000.0)
    }

    /// The absolute→SDR-relative inverse (anchoring HDR content into
    /// an SDR canvas's relative domain).
    #[must_use]
    pub fn absolute_to_sdr(&self, nits: f32) -> f32 {
        (nits.max(0.0) / self.canvas_nits).min(1.0)
    }

    /// The SDR surface's channel scale into absolute nits.
    #[must_use]
    pub fn channel_scale(&self) -> f32 {
        self.canvas_nits
    }
}

/// The BT.2100 HLG OOTF system gamma for a display peak (nits):
/// `γ = 1.2 + 0.42·log₁₀(Lw / 1000)`.
///
/// Published anchors: 1000 nits → exactly 1.2; 2000 → 1.3264…;
/// 400 → 1.0329…; 100 → 0.78.
#[must_use]
pub fn hlg_system_gamma(peak_nits: f64) -> f64 {
    1.2 + 0.42 * (peak_nits / 1000.0).log10()
}

/// Apply the HLG OOTF to one scene-linear value for a display peak
/// (nits): display-linear = `scene^γ` (peak-normalized — BT.2100's
/// α re-normalization is folded into the peak-relative domain).
///
/// # Errors
/// [`AdaptationError::BadPeak`] outside (0, 10 000].
pub fn hlg_ootf(scene_linear: f32, peak_nits: f32) -> Result<f32, AdaptationError> {
    if !(peak_nits > 0.0 && peak_nits <= 10_000.0) {
        return Err(AdaptationError::BadPeak);
    }
    let gamma = hlg_system_gamma(f64::from(peak_nits));
    Ok(scene_linear.clamp(0.0, 1.0).powf(gamma as f32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canvas_validation() {
        assert!(LuminanceAdapter::new(0.0).is_err());
        assert!(LuminanceAdapter::new(-1.0).is_err());
        assert!(LuminanceAdapter::new(20_000.0).is_err());
        assert_eq!(
            LuminanceAdapter::new(203.0).unwrap().canvas_nits(),
            LuminanceAdapter::default().canvas_nits()
        );
        assert_eq!(
            LuminanceAdapter::from_luminance(Luminance::from_nits(300))
                .unwrap()
                .canvas_nits(),
            300.0
        );
    }

    #[test]
    fn anchoring_is_monotone_and_invertible() {
        // THE exit criterion: luminance adaptation monotonicity —
        // output is monotone in the input value AND in the canvas
        // level, with the identity at the reference.
        let a = LuminanceAdapter::new(203.0).unwrap();
        // (a) monotone in the input.
        let mut prev = -1.0;
        for i in 0..=100 {
            let v = a.sdr_to_absolute(i as f32 / 100.0);
            assert!(v >= prev, "input monotone at {i}");
            prev = v;
        }
        // (b) monotone in the canvas (for a positive input).
        for rel in [0.1f32, 0.5, 1.0] {
            let mut prev = -1.0;
            for canvas in [50.0f32, 100.0, 203.0, 400.0, 1000.0] {
                let out = LuminanceAdapter::new(canvas).unwrap().sdr_to_absolute(rel);
                assert!(out >= prev, "canvas monotone at {canvas}");
                prev = out;
            }
        }
        // (c) identity at the reference: white lands on the canvas.
        assert_eq!(a.sdr_to_absolute(1.0), 203.0);
        assert_eq!(a.sdr_to_absolute(0.0), 0.0);
        // (d) invertible inside the domain.
        for i in 0..=50 {
            let rel = i as f32 / 50.0;
            let nits = a.sdr_to_absolute(rel);
            assert!(
                (a.absolute_to_sdr(nits) - rel).abs() < 1e-4,
                "inverse at {rel}"
            );
        }
        // (e) out-of-domain clamps both ways.
        assert_eq!(a.sdr_to_absolute(2.0), 203.0);
        assert_eq!(a.sdr_to_absolute(-1.0), 0.0);
        assert_eq!(a.absolute_to_sdr(5000.0), 1.0);
        assert_eq!(a.absolute_to_sdr(-5.0), 0.0);
    }

    #[test]
    fn hlg_gamma_published_anchors() {
        // BT.2100's formula, pinned at the published points.
        assert!((hlg_system_gamma(1000.0) - 1.2).abs() < 1e-12);
        assert!((hlg_system_gamma(2000.0) - 1.326_432_6).abs() < 1e-6);
        assert!((hlg_system_gamma(400.0) - 1.032_864_8).abs() < 1e-6);
        assert!((hlg_system_gamma(100.0) - 0.78).abs() < 1e-12);
        // Monotone in the peak (the log term).
        let mut prev = -10.0;
        for i in 1..=100 {
            let peak = i as f64 * 100.0;
            let g = hlg_system_gamma(peak);
            assert!(g >= prev, "gamma monotone at {peak}");
            prev = g;
        }
    }

    #[test]
    fn hlg_ootf_properties() {
        // Endpoints exact; monotone; peak-dependent ordering (a dimmer
        // panel lifts mid tones: γ < 1.2 → v^γ > v for v < 1).
        assert_eq!(hlg_ootf(0.0, 1000.0).unwrap(), 0.0);
        assert_eq!(hlg_ootf(1.0, 1000.0).unwrap(), 1.0);
        let mid_1000 = hlg_ootf(0.5, 1000.0).unwrap();
        let mid_400 = hlg_ootf(0.5, 400.0).unwrap();
        let mid_2000 = hlg_ootf(0.5, 2000.0).unwrap();
        assert!(mid_400 > mid_1000, "dimmer panel lifts mid tones");
        assert!(mid_2000 < mid_1000, "brighter panel darkens mid tones");
        assert!((mid_1000 - 0.5f32.powf(1.2)).abs() < 1e-6);
        assert!(matches!(hlg_ootf(0.5, 0.0), Err(AdaptationError::BadPeak)));
        assert!(matches!(
            hlg_ootf(0.5, 20_000.0),
            Err(AdaptationError::BadPeak)
        ));
    }
}

/// The peak-luminance negotiation (Phase 31): the composite's
/// effective mastering peak — the stack's brightest content clamped
/// to what the panel can actually show (never the reverse: a panel
/// brighter than the content simply never reaches its top; content
/// brighter than the panel is the tone mapper's job, and the
/// negotiated peak is the mapper's ceiling). Zero content luminance
/// (an all-SDR stack) keeps the panel's own peak (the SDR canvas's
/// headroom).
#[must_use]
pub fn negotiate_peak(panel_peak: Luminance, stack_max: Luminance) -> Luminance {
    let panel = panel_peak.as_nits();
    let stack = stack_max.as_nits();
    if stack == 0 || stack >= panel {
        panel_peak
    } else {
        Luminance::from_nits(stack)
    }
}

#[cfg(test)]
mod negotiate_tests {
    use super::*;

    #[test]
    fn the_stack_never_raises_the_panel() {
        let panel = Luminance::from_nits(600);
        let bright = Luminance::from_nits(1000);
        assert_eq!(negotiate_peak(panel, bright).as_nits(), 600);
    }

    #[test]
    fn dimmer_content_negotiates_down() {
        let panel = Luminance::from_nits(1000);
        let content = Luminance::from_nits(400);
        assert_eq!(negotiate_peak(panel, content).as_nits(), 400);
    }

    #[test]
    fn an_sdr_stack_keeps_the_panel_peak() {
        let panel = Luminance::from_nits(600);
        assert_eq!(
            negotiate_peak(panel, Luminance::from_nits(0)).as_nits(),
            600
        );
    }
}
