//! The color pipeline: range → transfer → primaries → reference-white
//! anchor, with an identity fast path — and, since Phase 38, the
//! **luminance tail**.
//!
//! This is Phase 8's *hook* (`docs/roadmap.md`): the full deterministic
//! decode/encode math every backend shares, while ldp-color (Phase 14)
//! layers tone-mapping policy and HDR merge strategies on top. The
//! composited policy is locked as:
//!
//! * **Identity descriptions composite in the target's encoded space**
//!   (pixman/GL-fast-path equivalence — integer premultiplied `over`,
//!   bit-stable, never touches libm),
//! * **Cross-description composites blend in scene-linear** — decode,
//!   convert primaries, anchor reference whites, blend, encode.
//! * **HDR layers on the PQ canvas carry a luminance tail** (Phase 38):
//!   the negotiated panel peak and the content's declared mastering
//!   bounds conclude as a per-layer [`TonePolicy`], and the pipeline
//!   applies it in the linear domain right after the anchor — the
//!   system's rolloff instead of the panel's hard clip. `Pass` keeps
//!   every pre-Phase-38 pixel oracle byte-identical.
//!
//! All math is IEEE f32 mul/add/div/sqrt plus `powf` for the transfer
//! curves; `sqrt`/`div`/`mul` are exactly specified, so only the transfer
//! curves can vary between libm implementations — golden tests anchor them
//! with tolerances while every integer path stays byte-exact. The tail's
//! knee adds its own `powf` pair (the PQ-domain EETF), under the same
//! tolerance doctrine as the curves themselves.

use ldp_core::color::{ColorDescription, ColorRange, Primaries, TransferFunction};

use crate::layer::TonePolicy;
use crate::transfer::{decode_transfer, encode_transfer};

/// Row-major 3×3 matrix.
pub type Matrix3 = [f32; 9];

/// The RGB→RGB conversion between two primaries sets (row-major).
///
/// Derived through XYZ: source RGB → XYZ (white-point-scaled primary
/// columns) → target RGB (inverse of the target's own matrix). Identical
/// primaries yield the identity matrix.
#[must_use]
pub fn primaries_matrix(source: Primaries, target: Primaries) -> Matrix3 {
    if source == target {
        return [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
    }
    let s = rgb_to_xyz(source);
    let t = rgb_to_xyz(target);
    let t_inv = invert3(t);
    mul3(t_inv, s)
}

/// RGB→XYZ matrix (columns are the white-scaled primary vectors).
fn rgb_to_xyz(p: Primaries) -> Matrix3 {
    let c = p.chromaticities();
    let dirs = [
        xyz_dir(c[0], c[1]),
        xyz_dir(c[2], c[3]),
        xyz_dir(c[4], c[5]),
    ];
    let white = xyz_dir(c[6], c[7]);
    let scale = solve_columns(dirs, white);
    // Column i of the matrix = scale[i] * dirs[i].
    [
        dirs[0][0] * scale[0],
        dirs[1][0] * scale[1],
        dirs[2][0] * scale[2],
        dirs[0][1] * scale[0],
        dirs[1][1] * scale[1],
        dirs[2][1] * scale[2],
        dirs[0][2] * scale[0],
        dirs[1][2] * scale[1],
        dirs[2][2] * scale[2],
    ]
}

fn xyz_dir(x: f32, y: f32) -> [f32; 3] {
    [x / y, 1.0, (1.0 - x - y) / y]
}

/// Solve `columns · s = white` via scalar triple products (Cramer);
/// correct by construction for the column-basis convention.
fn solve_columns(columns: [[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    let [c0, c1, c2] = columns;
    let det = dot(c0, cross(c1, c2));
    let inv_det = 1.0 / det;
    [
        dot(v, cross(c1, c2)) * inv_det,
        dot(c0, cross(v, c2)) * inv_det,
        dot(c0, cross(c1, v)) * inv_det,
    ]
}

/// Dot product.
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Cross product.
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// Invert a row-major 3×3.
fn invert3(m: Matrix3) -> Matrix3 {
    let det = m[0] * (m[4] * m[8] - m[5] * m[7]) - m[1] * (m[3] * m[8] - m[5] * m[6])
        + m[2] * (m[3] * m[7] - m[4] * m[6]);
    let inv = 1.0 / det;
    [
        (m[4] * m[8] - m[5] * m[7]) * inv,
        (m[2] * m[7] - m[1] * m[8]) * inv,
        (m[1] * m[5] - m[2] * m[4]) * inv,
        (m[5] * m[6] - m[3] * m[8]) * inv,
        (m[0] * m[8] - m[2] * m[6]) * inv,
        (m[2] * m[3] - m[0] * m[5]) * inv,
        (m[3] * m[7] - m[4] * m[6]) * inv,
        (m[1] * m[6] - m[0] * m[7]) * inv,
        (m[0] * m[4] - m[1] * m[3]) * inv,
    ]
}

/// Row-major matrix product.
fn mul3(a: Matrix3, b: Matrix3) -> Matrix3 {
    let mut out = [0.0f32; 9];
    for r in 0..3 {
        for c in 0..3 {
            out[r * 3 + c] = a[r * 3] * b[c] + a[r * 3 + 1] * b[3 + c] + a[r * 3 + 2] * b[6 + c];
        }
    }
    out
}

/// A resolved per-layer color conversion.
///
/// Built once per layer (never per pixel); [`ColorPipeline::is_identity`]
/// selects the encoded-space fast path.
///
/// Phase 38 adds the **luminance tail** ([`ColorPipeline::with_tone`]):
/// a [`TonePolicy`] concluded by the compositor — the negotiated
/// panel peak, and the content's declared mastering bounds — applied
/// in the linear domain right after the anchor. The tail only serves
/// a PQ target (the HDR canvas); every other combination keeps the
/// pre-Phase-38 bytes. A tail-bearing pipeline is never `identity`
/// (the encoded-space fast paths must not bypass the tail).
#[derive(Clone, Copy, Debug)]
pub struct ColorPipeline {
    identity: bool,
    src_range: ColorRange,
    src_transfer: TransferFunction,
    matrix: Matrix3,
    /// Reference-white anchor: target white / source white (linear scale).
    anchor: f32,
    out_transfer: TransferFunction,
    out_range: ColorRange,
    /// The luminance tail (Phase 38): the tone policy baked into the
    /// linear domain, `None` when passing.
    tail: Option<ToneTail>,
    /// Luma coefficients of the *target* primaries (the tail's
    /// luminance read; identity pipelines never carry a tail).
    luma: [f32; 3],
    /// Linear-domain 1.0 in nits on this pipeline's target (PQ:
    /// 10 000; the SDR reference otherwise) — the tail's unit.
    linear_to_nits: f32,
}

/// The baked luminance tail (Phase 38): one of the two curves the
/// [`TonePolicy`] resolves to, in the pipeline's linear domain.
///
/// * [`ToneTail::Clip`] — the honest default (no metadata): scale
///   down anything above the ceiling, leave everything below alone
///   (the BT.2390 clip guidance: never exceed the panel).
/// * [`ToneTail::Eetf`] — the BT.2390-structured knee from the
///   content's mastering range onto the ceiling: PQ-domain
///   normalized-master coordinates, the knee at 75% of the display
///   span, a cubic-Hermite rolloff with slope 1 at the knee (C¹
///   with the identity segment) and slope 0 at the ceiling (the
///   shoulder), smoothstep where the chord is too shallow for the
///   Hermite's monotonicity (the Fritsch–Carlson guard). The shape
///   mirrors `ldp-color`'s f64 reference EETF in f32 — the
///   renderer's precision doctrine — and the golden suite anchors
///   the published points (identity below the master range,
///   rolloff between, saturation at the ceiling).
#[derive(Clone, Copy, Debug)]
enum ToneTail {
    /// Ceiling clip: `ceiling/y` above the ceiling, else 1. (The
    /// ceiling is in *nits*, the same unit `luminance_scale`
    /// receives — consistent with the Eetf arm.)
    Clip {
        /// The ceiling (nits).
        ceiling_nits: f32,
    },
    /// The knee: mapping absolute nits through the PQ-domain EETF.
    /// (The negotiated ceiling lives structurally in `d_hi` — the
    /// display peak in normalized-master coordinates.)
    Eetf {
        /// Mastering floor (nits).
        master_min: f32,
        /// Mastering peak (nits).
        master_max: f32,
        /// PQ of the mastering bounds (encoded domain).
        p_min_m: f32,
        p_max_m: f32,
        /// The ceiling in normalized-master PQ coordinates.
        d_hi: f32,
        /// The knee point in the same coordinates.
        knee: f32,
        /// The Fritsch–Carlson fallback: too-shallow chords take the
        /// C0 smoothstep instead of the Hermite.
        smoothstep: bool,
    },
}

impl ToneTail {
    /// The per-pixel luminance scale for one absolute luminance
    /// (nits): the mapped luminance over the input.
    ///
    /// Blacks pass through unchanged (both curves leave sub-master
    /// and near-black values alone — scaling up black would multiply
    /// chroma, and the panel clamps its own black).
    fn luminance_scale(&self, y_nits: f32) -> f32 {
        match *self {
            ToneTail::Clip { ceiling_nits } => {
                if y_nits <= ceiling_nits || y_nits <= 0.0 {
                    1.0
                } else {
                    ceiling_nits / y_nits
                }
            }
            ToneTail::Eetf {
                master_min,
                master_max,
                p_min_m,
                p_max_m,
                d_hi,
                knee,
                smoothstep,
                ..
            } => {
                if y_nits <= master_min || y_nits <= 0.0 {
                    return 1.0; // below mastering black: unchanged
                }
                // The BT.2390 input-clip rule: content above the
                // master peak lands exactly on the display peak.
                let y_eff = y_nits.min(master_max);
                let p = encode_transfer(TransferFunction::Pq, y_eff / 10_000.0);
                let span = p_max_m - p_min_m;
                let u = if span > 0.0 {
                    (p - p_min_m) / span
                } else {
                    0.0
                };
                let v = if u <= knee {
                    u
                } else {
                    // The rolloff branch: Hermite (C¹ at the knee,
                    // shoulder at the ceiling) or smoothstep.
                    let dx = 1.0 - knee;
                    let t = if dx > 0.0 { (u - knee) / dx } else { 1.0 };
                    let t = t.clamp(0.0, 1.0);
                    if smoothstep {
                        t * t * (3.0 - 2.0 * t)
                    } else {
                        // Cubic Hermite: slope 1 at the knee, slope 0
                        // at the ceiling (the m1 term drops out).
                        let t2 = t * t;
                        let t3 = t2 * t;
                        (2.0 * t3 - 3.0 * t2 + 1.0) * knee
                            + (t3 - 2.0 * t2 + t) * dx
                            + (-2.0 * t3 + 3.0 * t2) * d_hi
                    }
                };
                let p_out = p_min_m + v * span;
                let mapped = decode_transfer(TransferFunction::Pq, p_out) * 10_000.0;
                if mapped >= y_nits {
                    1.0 // never brighten: the identity floor
                } else {
                    (mapped / y_nits).clamp(0.0, 1.0)
                }
            }
        }
    }
}

impl ColorPipeline {
    /// Resolve the conversion from a source description to a target.
    #[must_use]
    pub fn new(source: &ColorDescription, target: &ColorDescription) -> ColorPipeline {
        let identity = *source == *target;
        // PQ decodes to absolute nits/10000; other curves are relative to
        // their reference white. Normalize both sides to "1.0 = reference
        // white in nits / 10000" before the anchor ratio.
        let src_scale = ref_white_scale(source);
        let out_scale = ref_white_scale(target);
        let anchor = out_scale / src_scale;
        ColorPipeline {
            identity,
            src_range: source.range,
            src_transfer: source.transfer,
            matrix: primaries_matrix(source.primaries, target.primaries),
            anchor,
            out_transfer: target.transfer,
            out_range: target.range,
            tail: None,
            luma: [0.0, 0.0, 0.0],
            linear_to_nits: 1.0,
        }
    }

    /// Resolve the conversion with a luminance tail (Phase 38): the
    /// tone policy the compositor concluded for this layer, applied
    /// in the linear domain after the anchor.
    ///
    /// The tail serves only a PQ target carrying a non-`Pass`
    /// policy whose ceiling sits inside the PQ range — every other
    /// combination (SDR canvas, `Pass`, or a ceiling at/above the
    /// canvas's own 10 000-nit domain) is the plain pipeline, bytes
    /// identical. A served tail also disqualifies the identity fast
    /// path: the encoded-space copies must not bypass the rolloff.
    #[must_use]
    pub fn with_tone(
        source: &ColorDescription,
        target: &ColorDescription,
        tone: &TonePolicy,
    ) -> ColorPipeline {
        let mut p = Self::new(source, target);
        if target.transfer != TransferFunction::Pq {
            return p; // the SDR canvas keeps the anchor-pull doctrine
        }
        // The linear domain on a PQ target is absolute nits/10000.
        p.linear_to_nits = 10_000.0;
        p.luma = luma_coefficients(target.primaries);
        let tail = match *tone {
            TonePolicy::Pass => None,
            TonePolicy::Clip { ceiling_nits } => {
                if ceiling_nits <= 0.0 || ceiling_nits >= 10_000.0 {
                    None
                } else {
                    Some(ToneTail::Clip { ceiling_nits })
                }
            }
            TonePolicy::Eetf {
                master_min_nits,
                master_max_nits,
                ceiling_nits,
            } => {
                // Degenerate policies fall back to the honest clip:
                // an unordered range or a ceiling outside the domain
                // maps down to what is actually known (the panel).
                if master_min_nits < master_max_nits
                    && master_max_nits > 0.0
                    && master_max_nits <= 10_000.0
                    && ceiling_nits > 0.0
                    && ceiling_nits < 10_000.0
                {
                    Some(Self::bake_eetf(
                        master_min_nits,
                        master_max_nits,
                        ceiling_nits,
                    ))
                } else if ceiling_nits > 0.0 && ceiling_nits < 10_000.0 {
                    Some(ToneTail::Clip { ceiling_nits })
                } else {
                    None
                }
            }
        };
        if tail.is_some() {
            p.tail = tail;
            // The tail-bearing pipeline takes the scene-linear path
            // even for equal descriptions — the rolloff must run.
            p.identity = false;
        }
        p
    }

    /// Bake the BT.2390-structured knee (f32, the renderer's
    /// precision doctrine — the f64 reference lives in `ldp-color`).
    fn bake_eetf(master_min: f32, master_max: f32, ceiling: f32) -> ToneTail {
        let p_min_m = encode_transfer(TransferFunction::Pq, master_min / 10_000.0);
        let p_max_m = encode_transfer(TransferFunction::Pq, master_max / 10_000.0);
        let p_min_d = encode_transfer(TransferFunction::Pq, 0.0);
        let p_max_d = encode_transfer(TransferFunction::Pq, ceiling / 10_000.0);
        let span = p_max_m - p_min_m;
        let d_lo = if span > 0.0 {
            (p_min_d - p_min_m) / span
        } else {
            0.0
        };
        let d_hi = if span > 0.0 {
            (p_max_d - p_min_m) / span
        } else {
            1.0
        };
        let knee = d_lo + 0.75 * (d_hi - d_lo);
        // Fritsch–Carlson monotonicity: with m0 = 1 (the C¹ knee) and
        // m1 = 0 (the shoulder), monotone iff the chord slope >= 1/3.
        let r = if (1.0 - knee) > 0.0 {
            (d_hi - knee) / (1.0 - knee)
        } else {
            1.0
        };
        ToneTail::Eetf {
            master_min,
            master_max,
            p_min_m,
            p_max_m,
            d_hi,
            knee,
            smoothstep: r < 1.0 / 3.0,
        }
    }

    /// Whether source and target descriptions match (fast path: no range,
    /// transfer, primaries or anchor work at all).
    #[must_use]
    pub fn is_identity(&self) -> bool {
        self.identity
    }

    /// The reference-white anchor factor (diagnostic; 1.0 when identity).
    #[must_use]
    pub fn anchor(&self) -> f32 {
        self.anchor
    }

    /// Decode straight encoded-domain source RGB into output-relative
    /// linear RGB (clamped to the output's white).
    ///
    /// Phase 38: a tail-bearing pipeline then applies the luminance
    /// policy — the decoded linear luminance (in the target's domain,
    /// converted to nits) reads the tail's scale, and the scale
    /// multiplies all three channels (a luminance-domain map; hue is
    /// preserved by construction — the same contract `ldp-color`'s
    /// per-pixel scale carries).
    #[must_use]
    pub fn decode_straight(&self, rgb: [f32; 3]) -> [f32; 3] {
        let mut v = [0.0f32; 3];
        for i in 0..3 {
            let c = range_decode(rgb[i], self.src_range);
            v[i] = decode_transfer(self.src_transfer, c);
        }
        // Primaries conversion (row-major).
        let mut out = [
            self.matrix[0] * v[0] + self.matrix[1] * v[1] + self.matrix[2] * v[2],
            self.matrix[3] * v[0] + self.matrix[4] * v[1] + self.matrix[5] * v[2],
            self.matrix[6] * v[0] + self.matrix[7] * v[1] + self.matrix[8] * v[2],
        ];
        for c in &mut out {
            *c = (*c * self.anchor).clamp(0.0, 1.0);
        }
        if let Some(tail) = &self.tail {
            let y = self.luma[0] * out[0] + self.luma[1] * out[1] + self.luma[2] * out[2];
            let y_nits = y * self.linear_to_nits;
            let scale = tail.luminance_scale(y_nits);
            if scale != 1.0 {
                for c in &mut out {
                    *c *= scale;
                }
            }
        }
        out
    }

    /// Encode output-relative linear straight RGB back to the target's
    /// encoded domain.
    #[must_use]
    pub fn encode_straight(&self, rgb: [f32; 3]) -> [f32; 3] {
        let mut out = [0.0f32; 3];
        for i in 0..3 {
            let e = encode_transfer(self.out_transfer, rgb[i]);
            out[i] = range_encode(e, self.out_range);
        }
        out
    }

    /// Decode straight encoded-domain *target* RGB (framebuffer content)
    /// into target-relative linear — the destination side of the pipeline:
    /// output range + output transfer only, no primaries work or anchor
    /// (the framebuffer is already in the target's space).
    #[must_use]
    pub fn decode_target_straight(&self, rgb: [f32; 3]) -> [f32; 3] {
        let mut out = [0.0f32; 3];
        for i in 0..3 {
            let c = range_decode(rgb[i], self.out_range);
            out[i] = decode_transfer(self.out_transfer, c);
        }
        out
    }
}

/// Reference-white scale of a description: the factor that makes a
/// transfer-decoded value *reference-white-relative* (1.0 = the
/// description's reference white). PQ decodes to absolute nits/10000, so
/// its scale is the white itself in those units; every relative curve is
/// already white-relative.
fn ref_white_scale(d: &ColorDescription) -> f32 {
    // as_wire_units() is 1e-4 cd/m²; nits/10000 is 1e-4 cd/m² / 1e4.
    let white = (d.reference_white.as_wire_units() as f32) / 1e8;
    match d.transfer {
        TransferFunction::Pq => white.max(1e-6),
        _ => 1.0,
    }
}

/// Luma coefficients of a primaries set (the Y row of the RGB→XYZ
/// solve — the luminance read the tail's scale operates on).
fn luma_coefficients(p: Primaries) -> [f32; 3] {
    let m = rgb_to_xyz(p);
    // Row-major: the second row (index 3..6) is Y.
    [m[3], m[4], m[5]]
}

/// Range decode: encoded `0..=1` → normalized `0..=1`.
fn range_decode(v: f32, range: ColorRange) -> f32 {
    match range {
        ColorRange::Studio => (v * 255.0 - 16.0) / 219.0,
        _ => v,
    }
}

/// Range encode: normalized `0..=1` → encoded `0..=1`.
fn range_encode(v: f32, range: ColorRange) -> f32 {
    match range {
        ColorRange::Studio => (v * 219.0 + 16.0) / 255.0,
        _ => v,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32, tol: f32) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn primaries_matrix_identity_and_neutral() {
        let id = primaries_matrix(Primaries::Bt709, Primaries::Bt709);
        assert_eq!(id, [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);
        // Gray (equal RGB) stays gray under any primaries conversion
        // (achromatic axis maps to the achromatic axis, both D65 white).
        let m = primaries_matrix(Primaries::DciP3, Primaries::Bt709);
        let gray = apply3(m, [0.5, 0.5, 0.5]);
        assert!(close(gray[0], 0.5, 1e-5), "{gray:?}");
        assert!(close(gray[1], 0.5, 1e-5));
        assert!(close(gray[2], 0.5, 1e-5));
    }

    fn apply3(m: Matrix3, v: [f32; 3]) -> [f32; 3] {
        [
            m[0] * v[0] + m[1] * v[1] + m[2] * v[2],
            m[3] * v[0] + m[4] * v[1] + m[5] * v[2],
            m[6] * v[0] + m[7] * v[1] + m[8] * v[2],
        ]
    }

    #[test]
    fn p3_green_into_709_shrinks() {
        // P3-D65 green is outside the 709 gamut: converting pure P3 green
        // into 709 RGB yields a large G with small *negative* R/B (the
        // pipeline clamps at encode time); back-conversion restores it.
        let fwd = primaries_matrix(Primaries::DciP3, Primaries::Bt709);
        let g709 = apply3(fwd, [0.0, 1.0, 0.0]);
        assert!(g709[1] > 0.9 && g709[1] < 1.2, "green survives: {g709:?}");
        assert!(
            g709[0] > -0.35 && g709[0] < 0.0,
            "r slightly out of gamut: {g709:?}"
        );
        assert!(
            g709[2] > -0.2 && g709[2] < 0.0,
            "b slightly out of gamut: {g709:?}"
        );
        let back = primaries_matrix(Primaries::Bt709, Primaries::DciP3);
        let restored = apply3(back, g709);
        for (got, want) in restored.iter().zip([0.0, 1.0, 0.0]) {
            assert!(close(*got, want, 1e-5));
        }
    }

    #[test]
    fn identity_pipeline_skips_work() {
        let p = ColorPipeline::new(&ColorDescription::srgb_sdr(), &ColorDescription::srgb_sdr());
        assert!(p.is_identity());
        assert!(close(p.anchor(), 1.0, 1e-6));
        let p = ColorPipeline::new(&ColorDescription::pq_hdr(), &ColorDescription::srgb_sdr());
        assert!(!p.is_identity());
    }

    #[test]
    fn studio_range_round_trip() {
        for v in [0.1f32, 0.5, 0.9] {
            let e = range_encode(v, ColorRange::Studio);
            assert!((16.0 / 255.0..=235.0 / 255.0).contains(&e));
            assert!(close(range_decode(e, ColorRange::Studio), v, 1e-6));
        }
    }

    #[test]
    fn pq_to_sdr_anchor_pulls_down_hdr_white() {
        // PQ content at its 203-nit reference white should land at the SDR
        // output's reference white — relative 1.0 in both spaces.
        let src = ColorDescription::pq_hdr();
        let dst = ColorDescription::srgb_sdr();
        let p = ColorPipeline::new(&src, &dst);
        // The PQ-*encoded* value of 203 nits (ST 2084 curve).
        let v_enc = encode_transfer(TransferFunction::Pq, 203.0 / 10_000.0);
        let decoded = p.decode_straight([v_enc, v_enc, v_enc]);
        for c in decoded {
            assert!(close(c, 1.0, 2e-3), "anchored {decoded:?}");
        }
        // Content far above reference white clamps at the output white.
        let hot = encode_transfer(TransferFunction::Pq, 1000.0 / 10_000.0);
        let clamped = p.decode_straight([hot, hot, hot]);
        assert!(clamped.iter().all(|&c| c <= 1.0));
    }

    #[test]
    fn ycbcr_coefficients_agree_with_primaries_matrix() {
        // The luminance row of the RGB→XYZ solve *is* the Kr/Kg/Kb set.
        let m = rgb_to_xyz(Primaries::Bt709);
        let c = crate::yuv::ycbcr_coefficients(Primaries::Bt709);
        // Row 1 (Y) of the RGB→XYZ matrix at unit white: scale sum = 1.
        let y_of_white = m[3] + m[4] + m[5];
        assert!(close(y_of_white, 1.0, 1e-6));
        assert!(close(m[3] / y_of_white, c.kr, 1e-6));
        assert!(close(m[5] / y_of_white, c.kb, 1e-6));
    }
}
