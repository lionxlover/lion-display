//! The color pipeline composition: input [`ColorSpace`]s and the
//! per-output [`OutputTransform`] tail.
//!
//! Layering (architecture §12): the *renderer's* Phase 8 hook owns the
//! per-pixel per-surface path (range → transfer decode → primaries →
//! reference-white anchor → blend). This module is the rest of the
//! story:
//!
//! * [`ColorSpace`] — an input space, either parametric (primaries +
//!   transfer function) or **imported from an ICC profile** (the
//!   matrix/TRC path with its native D65 matrix and per-channel TRC
//!   curves). It converts encoded RGB → linear RGB → XYZ and builds
//!   linear↔linear conversion matrices into other spaces.
//! * [`OutputTransform`] — the **output tail** every composited frame
//!   passes through: per-pixel luminance tone mapping (BT.2390 EETF
//!   for SDR targets, peak clipping for HDR/PQ targets) → gamut
//!   mapping (luma-preserving soft-knee) → transfer encoding.
//!
//! Contract of [`OutputTransform::map`]: the input is linear RGB in the
//! **target's primaries expressed in absolute nits** (HDR chains carry
//! absolute values natively — PQ decodes to nits; SDR chains are
//! relative and must be anchored to nits first, which is
//! `ldp-hdr`'s adaptation job — never a hidden constant here). The
//! output is encoded RGB in `[0,1]`, ready for the target's transfer
//! encoding or its integer PQ ramp.

use crate::gamut::{GamutMapper, GamutMode};
use crate::icc::IccProfile;
use crate::icc_trc::IccTrc;
use crate::matrix::{self, Matrix3};
use crate::tone::{HdrToHdr, ToneError, ToneMapper};
use crate::transfer::{decode_transfer, encode_transfer};
use ldp_core::color::{ColorDescription, ColorRange, Primaries, TransferFunction};

/// One channel's transfer curve: parametric, or an imported ICC TRC.
#[derive(Clone, PartialEq, Debug)]
pub enum Curve {
    /// A wire transfer function.
    Parametric(TransferFunction),
    /// An ICC profile TRC.
    Icc(IccTrc),
}

impl Curve {
    /// Decode encoded → linear.
    #[must_use]
    pub fn decode(&self, v: f32) -> f32 {
        match self {
            Self::Parametric(tf) => decode_transfer(*tf, v),
            Self::Icc(trc) => trc.decode(v),
        }
    }

    /// Encode linear → encoded.
    #[must_use]
    pub fn encode(&self, v: f32) -> f32 {
        match self {
            Self::Parametric(tf) => encode_transfer(*tf, v),
            Self::Icc(trc) => trc.encode(v),
        }
    }
}

/// An input color space: linear-primaries matrix plus per-channel
/// curves.
#[derive(Clone, PartialEq, Debug)]
pub struct ColorSpace {
    to_xyz: Matrix3,
    from_xyz: Matrix3,
    curves: [Curve; 3],
}

impl ColorSpace {
    /// A parametric space (primaries + one transfer function).
    #[must_use]
    pub fn from_primaries(p: Primaries, tf: TransferFunction) -> Self {
        let to_xyz = matrix::rgb_to_xyz(p);
        let from_xyz = matrix::xyz_to_rgb(p);
        Self {
            to_xyz,
            from_xyz,
            curves: [
                Curve::Parametric(tf),
                Curve::Parametric(tf),
                Curve::Parametric(tf),
            ],
        }
    }

    /// A space imported from an ICC matrix/TRC profile (display-native
    /// D65 matrix, per-channel TRCs).
    #[must_use]
    pub fn from_icc(profile: &IccProfile) -> Self {
        let native = profile.native_rgb_to_xyz();
        let mut to_xyz = [0.0f32; 9];
        for (t, v) in to_xyz.iter_mut().zip(native) {
            *t = v as f32;
        }
        let from_xyz = matrix::invert3(&to_xyz).unwrap_or(matrix::IDENTITY);
        let curves = [
            Curve::Icc(profile.trcs[0].clone()),
            Curve::Icc(profile.trcs[1].clone()),
            Curve::Icc(profile.trcs[2].clone()),
        ];
        Self {
            to_xyz,
            from_xyz,
            curves,
        }
    }

    /// Decode encoded RGB → linear RGB (this space).
    #[must_use]
    pub fn decode_linear(&self, encoded: [f32; 3]) -> [f32; 3] {
        [
            self.curves[0].decode(encoded[0].clamp(0.0, 1.0)),
            self.curves[1].decode(encoded[1].clamp(0.0, 1.0)),
            self.curves[2].decode(encoded[2].clamp(0.0, 1.0)),
        ]
    }

    /// Encode linear RGB (this space) → encoded RGB.
    #[must_use]
    pub fn encode(&self, linear: [f32; 3]) -> [f32; 3] {
        [
            self.curves[0].encode(linear[0].clamp(0.0, 1.0)),
            self.curves[1].encode(linear[1].clamp(0.0, 1.0)),
            self.curves[2].encode(linear[2].clamp(0.0, 1.0)),
        ]
    }

    /// Linear RGB (this space) → CIE XYZ (D65).
    #[must_use]
    pub fn to_xyz(&self, linear: [f32; 3]) -> [f32; 3] {
        matrix::apply3(&self.to_xyz, linear)
    }

    /// CIE XYZ (D65) → linear RGB (this space).
    #[must_use]
    pub fn from_xyz(&self, xyz: [f32; 3]) -> [f32; 3] {
        matrix::apply3(&self.from_xyz, xyz)
    }

    /// The linear-this-space → linear-`target` conversion matrix.
    #[must_use]
    pub fn conversion_to(&self, target: &ColorSpace) -> Matrix3 {
        matrix::mul3(&target.from_xyz, &self.to_xyz)
    }

    /// The space's luma coefficients (Y row of its RGB→XYZ matrix).
    #[must_use]
    pub fn luma(&self) -> [f32; 3] {
        [self.to_xyz[3], self.to_xyz[4], self.to_xyz[5]]
    }

    /// The RGB→XYZ matrix (row-major).
    #[must_use]
    pub fn rgb_to_xyz_matrix(&self) -> Matrix3 {
        self.to_xyz
    }
}

/// [`OutputTransform`] construction failures.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum TransformError {
    /// Studio-range outputs are not a v1 target (outputs are
    /// full-range; range handling is the sampling side).
    StudioTarget,
    /// Bad tone-mapping bounds (see [`ToneError`]).
    Tone(ToneError),
}

impl core::fmt::Display for TransformError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::StudioTarget => write!(f, "studio-range output targets unsupported"),
            Self::Tone(e) => write!(f, "tone mapper: {e}"),
        }
    }
}

impl std::error::Error for TransformError {}

impl From<ToneError> for TransformError {
    fn from(e: ToneError) -> Self {
        Self::Tone(e)
    }
}

/// The per-output tail: tone-map scale → gamut map → encode.
#[derive(Clone, Debug)]
pub struct OutputTransform {
    primaries: Primaries,
    transfer: TransferFunction,
    luma: [f32; 3],
    /// Encoded 1.0's luminance (SDR reference white; 10 000 for PQ).
    reference_nits: f32,
    tone: Option<ToneMapper>,
    hdr_clip: Option<HdrToHdr>,
    gamut: GamutMapper,
}

impl OutputTransform {
    /// Build the output tail.
    ///
    /// * `target` — the output's color description (primaries,
    ///   transfer, reference white; full range),
    /// * `panel` — the output's luminance range in nits,
    /// * `mastering` — the content's mastering range in nits (SDR
    ///   content on an SDR output passes `(0, reference_white)` so no
    ///   tone mapping happens),
    /// * `gamut` — the gamut mapping mode.
    ///
    /// # Errors
    /// [`TransformError`] on studio-range targets or bad bounds.
    pub fn new(
        target: ColorDescription,
        panel: (f32, f32),
        mastering: (f32, f32),
        gamut: GamutMode,
    ) -> Result<OutputTransform, TransformError> {
        if target.range == ColorRange::Studio {
            return Err(TransformError::StudioTarget);
        }
        let reference_nits = if target.transfer == TransferFunction::Pq {
            10_000.0
        } else {
            let nits = f64::from(target.reference_white.as_wire_units()) / 10_000.0;
            nits as f32
        };
        let (tone, hdr_clip) = if target.transfer == TransferFunction::Pq {
            (None, Some(HdrToHdr::new(panel.1)?))
        } else if mastering.1 > panel.1 {
            (Some(ToneMapper::new(mastering, panel)?), None)
        } else {
            (None, None)
        };
        Ok(OutputTransform {
            primaries: target.primaries,
            transfer: target.transfer,
            luma: matrix::luma_coefficients(target.primaries),
            reference_nits,
            tone,
            hdr_clip,
            gamut: GamutMapper::new(gamut),
        })
    }

    /// The target primaries.
    #[must_use]
    pub fn primaries(&self) -> Primaries {
        self.primaries
    }

    /// The target transfer function.
    #[must_use]
    pub fn transfer(&self) -> TransferFunction {
        self.transfer
    }

    /// Whether this tail tone-maps (SDR target with HDR mastering).
    #[must_use]
    pub fn is_tone_mapped(&self) -> bool {
        self.tone.is_some()
    }

    /// Map linear absolute-nits RGB (in the target's primaries) to
    /// encoded `[0,1]` RGB.
    ///
    /// Pipeline: luminance scale (EETF or HDR clip) → reference-white
    /// normalization (SDR) or PQ-domain scaling (HDR) → gamut map →
    /// transfer encode. Deterministic: specified IEEE ops plus the
    /// tolerance-anchored transfer `powf` family.
    #[must_use]
    pub fn map(&self, linear_nits: [f32; 3]) -> [f32; 3] {
        let mut rgb = linear_nits;
        let y = self.luma[0] * rgb[0] + self.luma[1] * rgb[1] + self.luma[2] * rgb[2];
        if let Some(t) = &self.tone {
            let s = t.luminance_scale(y);
            rgb = [rgb[0] * s, rgb[1] * s, rgb[2] * s];
        } else if let Some(h) = &self.hdr_clip {
            let s = h.luminance_scale(y);
            rgb = [rgb[0] * s, rgb[1] * s, rgb[2] * s];
        }
        // Normalize into the encode domain: SDR relative to the
        // reference white, PQ into nits/10000.
        let domain = if self.transfer == TransferFunction::Pq {
            10_000.0
        } else {
            self.reference_nits
        };
        let rel = [rgb[0] / domain, rgb[1] / domain, rgb[2] / domain];
        let mapped = self.gamut.map(rel, self.luma);
        [
            encode_transfer(self.transfer, mapped[0]),
            encode_transfer(self.transfer, mapped[1]),
            encode_transfer(self.transfer, mapped[2]),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ldp_core::color::Luminance;

    #[test]
    fn parametric_space_conversions_match_matrix_module() {
        let s = ColorSpace::from_primaries(Primaries::Bt709, TransferFunction::Srgb);
        let t = ColorSpace::from_primaries(Primaries::Bt2020, TransferFunction::Linear);
        let conv = s.conversion_to(&t);
        let expect = matrix::primaries_matrix(Primaries::Bt709, Primaries::Bt2020);
        for i in 0..9 {
            assert!((conv[i] - expect[i]).abs() < 1e-6, "conversion [{i}]");
        }
        // decode/encode round trip per channel.
        let enc = [0.25f32, 0.5, 0.75];
        let lin = s.decode_linear(enc);
        let back = s.encode(lin);
        for i in 0..3 {
            assert!((back[i] - enc[i]).abs() < 2e-3, "round trip ch {i}");
        }
        // XYZ round trip.
        let xyz = s.to_xyz(lin);
        let back_lin = s.from_xyz(xyz);
        for i in 0..3 {
            assert!((back_lin[i] - lin[i]).abs() < 1e-5, "xyz round trip {i}");
        }
        // Luma row matches the matrix module.
        let l = s.luma();
        for (a, b) in l.iter().zip(matrix::luma_coefficients(Primaries::Bt709)) {
            assert_eq!(*a, b);
        }
    }

    #[test]
    fn sdr_passthrough_is_unity() {
        // Pure SDR chain: BT.709/sRGB content onto the same target —
        // no tone mapping, reference white through.
        let target = ColorDescription::srgb_sdr();
        let ot = OutputTransform::new(target, (0.005, 80.0), (0.0, 80.0), GamutMode::Clip).unwrap();
        assert!(!ot.is_tone_mapped());
        // 80-nit white → encoded 1.0.
        let out = ot.map([80.0, 80.0, 80.0]);
        for c in out {
            assert!((c - 1.0).abs() < 1e-3, "white at reference: {c}");
        }
        // 40-nit gray → sRGB(0.5).
        let out = ot.map([40.0, 40.0, 40.0]);
        let want = encode_transfer(TransferFunction::Srgb, 0.5);
        for c in out {
            assert!((c - want).abs() < 1e-3, "half white: {c} vs {want}");
        }
    }

    #[test]
    fn hdr_mastering_maps_onto_sdr_target() {
        // BT.2020/PQ 1000-nit content → BT.709/sRGB 100-nit output.
        let target = ColorDescription {
            primaries: Primaries::Bt709,
            transfer: TransferFunction::Srgb,
            range: ColorRange::Full,
            luminance_min: Luminance::from_nits(0),
            luminance_max: Luminance::from_nits(100),
            reference_white: Luminance::from_nits(100),
        };
        let ot =
            OutputTransform::new(target, (0.005, 100.0), (0.005, 1000.0), GamutMode::Clip).unwrap();
        assert!(ot.is_tone_mapped());
        // Reference white 203 nits → ≈ 0.876 relative → sRGB ≈ 0.936.
        let out = ot.map([203.0, 203.0, 203.0]);
        for c in out {
            assert!((0.92..=0.95).contains(&c), "203-nit anchor: {c}");
        }
        // Master peak (1000-nit white) lands at display white.
        let out = ot.map([1000.0, 1000.0, 1000.0]);
        for c in out {
            assert!((c - 1.0).abs() < 0.02, "peak → display white: {c}");
        }
        // Blacks stay black; the ramp is monotone end to end.
        let out = ot.map([0.01, 0.01, 0.01]);
        for c in out {
            assert!(c < 0.01, "near black stays black: {c}");
        }
        let mut prev = -1.0;
        for i in 0..=100 {
            let l = f64::from(i) / 100.0 * 1000.0;
            let out = ot.map([l as f32, l as f32, l as f32]);
            let y = out[0]; // achromatic: channels equal
            assert!(y >= prev, "ramp monotone at {l}");
            prev = y;
        }
    }

    #[test]
    fn pq_target_clips_at_panel_peak() {
        let target = ColorDescription {
            primaries: Primaries::Bt2020,
            transfer: TransferFunction::Pq,
            range: ColorRange::Full,
            luminance_min: Luminance::from_wire_units(50),
            luminance_max: Luminance::from_nits(4000),
            reference_white: Luminance::from_nits(203),
        };
        let ot =
            OutputTransform::new(target, (0.005, 600.0), (0.005, 4000.0), GamutMode::Clip).unwrap();
        assert!(!ot.is_tone_mapped());
        // 500-nit gray (below the panel peak) is PQ-encoded unchanged.
        let v500 = 500.0 / 10_000.0;
        let want = encode_transfer(TransferFunction::Pq, v500);
        let out = ot.map([500.0, 500.0, 500.0]);
        for c in out {
            assert!((c - want).abs() < 1e-5, "below-peak passthrough: {c}");
        }
        // 3000-nit gray clips to the 600-nit peak's PQ code.
        let v600 = 600.0 / 10_000.0;
        let want = encode_transfer(TransferFunction::Pq, v600);
        let out = ot.map([3000.0, 3000.0, 3000.0]);
        for c in out {
            assert!((c - want).abs() < 1e-3, "above-peak clip: {c}");
        }
    }

    #[test]
    fn studio_targets_are_rejected() {
        let mut target = ColorDescription::srgb_sdr();
        target.range = ColorRange::Studio;
        assert!(matches!(
            OutputTransform::new(target, (0.005, 80.0), (0.0, 80.0), GamutMode::Clip),
            Err(TransformError::StudioTarget)
        ));
    }
}
