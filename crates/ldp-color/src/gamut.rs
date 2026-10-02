//! Gamut mapping — luma-preserving desaturation with a soft knee.
//!
//! A linear RGB gamut is the unit cube `[0,1]³` in the target space, so
//! this mapper is primaries-agnostic: it works on linear RGB triples in
//! whatever target space the pipeline is encoding, plus that space's
//! luma coefficients (the achromatic axis `(Y,Y,Y)` needs them).
//!
//! The model (documented, property-tested):
//!
//! * Every color lives on the **constant-luma line** through its own
//!   achromatic point `A = (Y,Y,Y)`: `P(s) = A + s·(C − A)` with
//!   `s = 1` at the color itself. The gamut boundary along that line is
//!   at `s*` (analytic per-channel solve), and the color's **chroma
//!   ratio** is `r = 1/s*` — `r ≤ 1` in gamut, `r > 1` out.
//! * [`GamutMode::Clip`]: in-gamut colors pass through **bit-identically**
//!   (the identity fast-path doctrine shared with the renderer); out-of
//!   gamut colors land exactly on the boundary point `P(s*)` — minimal
//!   chroma loss, exact luma preservation.
//! * [`GamutMode::SoftKnee`]: the chroma ratio is compressed through a
//!   smoothstep shoulder — `r' = r` below the knee `k` (colors
//!   untouched), rolling up to the boundary ratio `1` at the ceiling
//!   `c`. Continuous at the knee, at the gamut edge and at the ceiling
//!   (no cliff when content straddles the boundary), monotone, output
//!   always in gamut. The price, documented: the top `(1−k)` chroma
//!   slice of the legal gamut is slightly desaturated — the ICC
//!   perceptual-intent trade.
//!
//! Luminance is preserved exactly in every mode (the tone mapper owns
//! luminance; this module owns chroma). Inputs are the tone mapper's
//! outputs, so `Y ∈ [0,1]` is contractual; `Y` is clamped defensively
//! and super-white/super-black collapse to white/black by construction.

/// Which soft-knee behavior a [`GamutMapper`] applies.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub enum GamutMode {
    /// Hard clip: in-gamut identity, out-of-gamut → boundary point.
    ///
    /// The default: compositing pipelines want the deterministic
    /// in-gamut identity fast path (nothing legal ever moves);
    /// [`GamutMode::SoftKnee`] is the explicit perceptual opt-in.
    #[default]
    Clip,
    /// Soft knee: smoothstep compression of the chroma ratio.
    SoftKnee {
        /// Knee start as a fraction of the boundary chroma (`0 < knee < 1`).
        knee: f32,
        /// Chroma ratio at which compression reaches the boundary
        /// (`> 1`); beyond it colors clip to the boundary exactly.
        ceiling: f32,
    },
}

impl GamutMode {
    /// The perceptual default: knee at 95% of boundary chroma, ceiling
    /// at 2× (architecture §12: "perceptual clipping with soft-knee").
    #[must_use]
    pub const fn perceptual() -> Self {
        Self::SoftKnee {
            knee: 0.95,
            ceiling: 2.0,
        }
    }
}

/// A gamut mapper for one target space's linear unit cube.
#[derive(Clone, Copy, Debug, Default)]
pub struct GamutMapper {
    mode: GamutMode,
}

impl GamutMapper {
    /// A mapper in the given mode.
    #[must_use]
    pub const fn new(mode: GamutMode) -> Self {
        Self { mode }
    }

    /// The mapper's mode.
    #[must_use]
    pub const fn mode(&self) -> GamutMode {
        self.mode
    }

    /// Map one linear RGB triple into the unit cube.
    ///
    /// `luma` are the target space's luma coefficients (e.g.
    /// [`crate::matrix::luma_coefficients`]). The mapping is exact-luma
    /// and deterministic; in-gamut colors are returned unchanged.
    #[must_use]
    pub fn map(&self, rgb: [f32; 3], luma: [f32; 3]) -> [f32; 3] {
        debug_assert!(
            rgb.iter().all(|c| c.is_finite()),
            "gamut mapper contract: finite inputs"
        );
        // Bit-identical fast path: fully in-gamut colors under Clip.
        if matches!(self.mode, GamutMode::Clip)
            && rgb[0] >= 0.0
            && rgb[0] <= 1.0
            && rgb[1] >= 0.0
            && rgb[1] <= 1.0
            && rgb[2] >= 0.0
            && rgb[2] <= 1.0
        {
            return rgb;
        }
        let y = (luma[0] * rgb[0] + luma[1] * rgb[1] + luma[2] * rgb[2]).clamp(0.0, 1.0);
        let delta = [rgb[0] - y, rgb[1] - y, rgb[2] - y];
        if delta == [0.0, 0.0, 0.0] {
            // Achromatic (or collapsed): the luma clamp is the answer.
            return [y, y, y];
        }
        // Boundary parameter s*: per-channel linear bounds keeping
        // P(s) = (y,y,y) + s·delta inside [0,1].
        let mut s_star = f32::INFINITY;
        for d in delta {
            if d > 0.0 {
                s_star = s_star.min((1.0 - y) / d);
            } else if d < 0.0 {
                s_star = s_star.min(y / -d);
            }
        }
        if !s_star.is_finite() || s_star <= 0.0 {
            // Degenerate: the constant-luma line exits the cube at the
            // achromatic point itself (super-white / super-black after
            // the y clamp) — collapse to it.
            return [y, y, y];
        }
        let ratio = 1.0 / s_star; // chroma ratio: <= 1 in gamut, > 1 out.
                                  // The output parameter along the line. s_out = 1 is the original
                                  // color; s_out = s_star is the boundary point; between is the
                                  // soft-knee compression region (ratio maps to r' in (knee, 1),
                                  // s_out = r'·s_star).
        let s_out = match self.mode {
            GamutMode::Clip => {
                if ratio <= 1.0 {
                    return rgb; // in-gamut: the color itself, exactly
                }
                s_star // out-of-gamut: the boundary point
            }
            GamutMode::SoftKnee { knee, ceiling } => {
                if ratio <= knee {
                    return rgb; // below the knee: untouched, exactly
                }
                if ratio >= ceiling {
                    s_star // beyond the ceiling: the boundary point
                } else {
                    let t = (ratio - knee) / (ceiling - knee);
                    let smooth = t * t * (3.0 - 2.0 * t);
                    let r_prime = knee + (1.0 - knee) * smooth;
                    r_prime * s_star
                }
            }
        };
        [
            y + s_out * delta[0],
            y + s_out * delta[1],
            y + s_out * delta[2],
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722]; // BT.709

    #[test]
    fn clip_mode_is_identity_in_gamut() {
        let m = GamutMapper::new(GamutMode::Clip);
        // Bit-identical: every legal color, including the edges.
        for rgb in [
            [0.0f32, 0.0, 0.0],
            [1.0, 1.0, 1.0],
            [1.0, 0.0, 0.0],
            [0.123_456_78, 0.5, 0.9],
            [0.0, 1.0, 0.0],
        ] {
            assert_eq!(m.map(rgb, LUMA), rgb);
        }
    }

    #[test]
    fn clip_mode_lands_on_the_boundary() {
        let m = GamutMapper::new(GamutMode::Clip);
        // Pure saturated overshoot desaturates on the constant-luma line
        // to the cube boundary — exact luma match.
        let out = m.map([1.2, -0.1, 0.05], LUMA);
        let y_in = LUMA[0] * 1.2 + LUMA[1] * -0.1 + LUMA[2] * 0.05;
        let y_out = LUMA[0] * out[0] + LUMA[1] * out[1] + LUMA[2] * out[2];
        assert!(
            (y_in.clamp(0.0, 1.0) - y_out).abs() < 2e-6,
            "luma preserved: {y_in} vs {y_out}"
        );
        for c in out {
            assert!((-1e-6..=1.0 + 1e-6).contains(&c), "in gamut: {out:?}");
        }
        // A channel overshoot alone clips that channel to 1.
        let out = m.map([1.5, 0.2, 0.1], LUMA);
        assert!((out[0] - 1.0).abs() < 1e-6, "overshoot channel clipped");
        assert!(out[1] >= -1e-6 && out[1] <= 1.0 + 1e-6);
        assert!(out[2] >= -1e-6 && out[2] <= 1.0 + 1e-6);
    }

    #[test]
    fn super_white_and_black_collapse() {
        let m = GamutMapper::new(GamutMode::Clip);
        assert_eq!(m.map([2.0, 2.0, 2.0], LUMA), [1.0, 1.0, 1.0]);
        assert_eq!(m.map([-1.0, -1.0, -1.0], LUMA), [0.0, 0.0, 0.0]);
        // Achromatic legal gray is untouched.
        assert_eq!(m.map([0.4, 0.4, 0.4], LUMA), [0.4, 0.4, 0.4]);
    }

    #[test]
    fn soft_knee_properties() {
        let m = GamutMapper::new(GamutMode::perceptual());
        // (a) in-gamut mid-tones untouched (below the knee — note a
        // pure primary sits exactly ON the boundary, chroma ratio 1).
        for rgb in [[0.1f32, 0.2, 0.3], [0.5, 0.5, 0.5], [0.05, 0.05, 0.3]] {
            assert_eq!(m.map(rgb, LUMA), rgb, "below-knee identity {rgb:?}");
        }
        // (b) out-of-gamut always lands in-gamut with luma preserved.
        for rgb in [
            [1.3f32, -0.2, 0.1],
            [0.2, 1.4, -0.3],
            [-0.2, -0.2, 1.6],
            [3.0, 0.0, 0.0],
        ] {
            let out = m.map(rgb, LUMA);
            for c in out {
                assert!((-1e-6..=1.0 + 1e-6).contains(&c), "in gamut {out:?}");
            }
            let y_in = (LUMA[0] * rgb[0] + LUMA[1] * rgb[1] + LUMA[2] * rgb[2]).clamp(0.0, 1.0);
            let y_out = LUMA[0] * out[0] + LUMA[1] * out[1] + LUMA[2] * out[2];
            assert!((y_in - y_out).abs() < 2e-6, "luma preserved {rgb:?}");
        }
        // (c) chroma monotonicity along a LUMA-NEUTRAL desaturation
        // ray (dot(luma, dir) = 0, so the constant-luma line is fixed;
        // on a luma-varying ray the output legitimately desaturates as
        // the cube narrows toward white): more input chroma never
        // yields less output chroma.
        let base = [0.3f32, 0.3, 0.3];
        let dir = [0.715_2f32, -0.212_6, 0.0]; // luma-neutral by design
        let mut prev_chroma = -1.0;
        for i in 0..=40 {
            let s = i as f32 / 10.0;
            let rgb = [
                base[0] + s * dir[0],
                base[1] + s * dir[1],
                base[2] + s * dir[2],
            ];
            let out = m.map(rgb, LUMA);
            let y = LUMA[0] * out[0] + LUMA[1] * out[1] + LUMA[2] * out[2];
            let chroma =
                ((out[0] - y).powi(2) + (out[1] - y).powi(2) + (out[2] - y).powi(2)).sqrt();
            assert!(chroma >= prev_chroma - 1e-6, "chroma monotone at s={s}");
            prev_chroma = chroma;
        }
    }

    #[test]
    fn soft_knee_is_continuous_at_the_boundary() {
        // A ray crossing the gamut boundary: the mapped output moves
        // continuously (no jump at the crossing) — C0 across the edge.
        let m = GamutMapper::new(GamutMode::perceptual());
        let dir = [0.8f32, 0.6, 0.2];
        let mut prev: Option<[f32; 3]> = None;
        for i in 0..=400 {
            let s = i as f32 / 200.0;
            let out = m.map([s * dir[0], s * dir[1], s * dir[2]], LUMA);
            if let Some(p) = prev {
                let jump = (out[0] - p[0])
                    .abs()
                    .max((out[1] - p[1]).abs())
                    .max((out[2] - p[2]).abs());
                // Step size 0.005 in s; output step must be comparable
                // (a cliff would be 10x the input step).
                assert!(jump < 0.02, "continuity jump {jump} at s={s}");
            }
            prev = Some(out);
        }
    }
}
