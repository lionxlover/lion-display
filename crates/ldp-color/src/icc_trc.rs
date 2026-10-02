//! ICC TRC curves — the model behind `rTRC`/`gTRC`/`bTRC` tags.
//!
//! Parsed shapes (byte-level parsing lives in [`crate::icc`]; this
//! module owns the math):
//!
//! * [`IccTrc::Identity`] — `curveType` with zero entries,
//! * [`IccTrc::Gamma`] — `curveType` with one entry (a u8Fixed8 gamma)
//!   or `paraType` type 0,
//! * [`IccTrc::Table`] — `curveType` with ≥ 2 entries (a u16 sampling
//!   of the curve; strictly increasing is validated at parse so the
//!   inverse is well-defined),
//! * [`IccTrc::Parametric`] — v4 `paraType` types 1–4, the ICC
//!   parametric families with their piecewise inverses spelled out
//!   below.
//!
//! Domain contract: encoded and linear are both `0..=1` (ICC TRCs are
//! display-relative). All arithmetic is f32 with clamped inputs; the
//! model stores f64 (exact s15Fixed16/u8Fixed8/u16 conversions — the
//! round-trip guarantee needs the fixed-point values to survive
//! untouched).

/// A parsed TRC curve.
#[derive(Clone, PartialEq, Debug)]
pub enum IccTrc {
    /// The identity (linear) curve.
    Identity,
    /// A pure power gamma.
    Gamma(f64),
    /// A sampled table of u16 code points over `[0,1]` (strictly
    /// increasing, validated at parse).
    Table(Vec<u16>),
    /// A v4 parametric curve (types 0–4; type 0 folds into
    /// [`IccTrc::Gamma`]).
    Parametric(ParametricCurve),
}

/// The ICC v4 parametric curve families (type 0 is the pure power
/// `Y = X^g`; it usually folds into [`IccTrc::Gamma`] but stays
/// representable).
///
/// Evaluation (X = encoded, Y = linear, per the ICC parametricCurveType
/// table):
///
/// * type 1: `Y = (aX + b)^g` for `X ≥ −b/a`, else `0`,
/// * type 2: `Y = (aX + b)^g + c` for `X ≥ −b/a`, else `c`,
/// * type 3: `Y = (aX + b)^g + e` for `X < d`, else `cX`,
/// * type 4: `Y = (aX + b)^g + e` for `X < d`, else `(cX + f)^g`.
///
/// The inverses mirror the branches (see `ParametricCurve::encode` (the encode path));
/// flat regions invert to their domain edge, clamped into `[0,1]`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ParametricCurve {
    /// The parametric type (1..=4).
    pub kind: u8,
    /// Exponent `g`.
    pub g: f64,
    /// Parameter `a`.
    pub a: f64,
    /// Parameter `b`.
    pub b: f64,
    /// Parameter `c`.
    pub c: f64,
    /// Parameter `d` (branch point).
    pub d: f64,
    /// Parameter `e`.
    pub e: f64,
    /// Parameter `f`.
    pub f: f64,
}

impl Default for ParametricCurve {
    fn default() -> Self {
        Self {
            kind: 0,
            g: 1.0,
            a: 1.0,
            b: 0.0,
            c: 1.0,
            d: 0.0,
            e: 0.0,
            f: 0.0,
        }
    }
}

impl IccTrc {
    /// Decode: encoded `0..=1` → linear `0..=1`.
    #[must_use]
    pub fn decode(&self, x: f32) -> f32 {
        let x = x.clamp(0.0, 1.0);
        match self {
            Self::Identity => x,
            Self::Gamma(g) => x.powf(*g as f32),
            Self::Table(t) => table_lookup(t, x),
            Self::Parametric(p) => p.decode(x),
        }
    }

    /// Encode: linear `0..=1` → encoded `0..=1` (the exact inverse).
    #[must_use]
    pub fn encode(&self, y: f32) -> f32 {
        let y = y.clamp(0.0, 1.0);
        match self {
            Self::Identity => y,
            Self::Gamma(g) => y.powf(1.0 / *g as f32),
            Self::Table(t) => table_reverse(t, y),
            Self::Parametric(p) => p.encode(y),
        }
    }

    /// The pure-gamma exponent, when this curve is one (resolution
    /// fitting uses this).
    #[must_use]
    pub fn as_gamma(&self) -> Option<f64> {
        match *self {
            Self::Gamma(g) => Some(g),
            Self::Parametric(p) if p.kind == 0 => Some(p.g),
            _ => None,
        }
    }
}

fn table_lookup(table: &[u16], x: f32) -> f32 {
    if table.len() < 2 {
        return x;
    }
    let pos = x * ((table.len() - 1) as f32);
    let i = pos.floor() as usize;
    let i = i.min(table.len() - 2);
    let frac = pos - i as f32;
    let v0 = f32::from(table[i]) / 65_535.0;
    let v1 = f32::from(table[i + 1]) / 65_535.0;
    v0 + (v1 - v0) * frac
}

fn table_reverse(table: &[u16], y: f32) -> f32 {
    if table.len() < 2 {
        return y;
    }
    let target = (y.clamp(0.0, 1.0) * 65_535.0).round() as u32;
    // First index whose entry >= target (strictly increasing table).
    let mut lo = 0usize;
    let mut hi = table.len(); // exclusive
    while lo < hi {
        let mid = (lo + hi) / 2;
        if u32::from(table[mid]) < target {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    if lo == 0 {
        return 0.0;
    }
    if lo >= table.len() {
        return 1.0;
    }
    let v0 = u32::from(table[lo - 1]);
    let v1 = u32::from(table[lo]);
    if v1 == v0 {
        return (lo - 1) as f32 / (table.len() - 1) as f32;
    }
    let frac = ((target - v0) as f32) / ((v1 - v0) as f32);
    let base = (lo - 1) as f32 / (table.len() - 1) as f32;
    let step = 1.0 / (table.len() - 1) as f32;
    (base + step * frac).clamp(0.0, 1.0)
}

impl ParametricCurve {
    fn decode(&self, x: f32) -> f32 {
        let xf = f64::from(x);
        // Branch orientation (the sRGB proof): the (a, b, g, e) power
        // branch covers X ≥ d; the c-branch is the low-X toe (linear
        // for type 3, powered for type 4).
        let y = match self.kind {
            0 => xf.powf(self.g),
            1 => {
                if xf >= -self.b / self.a {
                    (self.a * xf + self.b).powf(self.g)
                } else {
                    0.0
                }
            }
            2 => {
                if xf >= -self.b / self.a {
                    (self.a * xf + self.b).powf(self.g) + self.c
                } else {
                    self.c
                }
            }
            3 => {
                if xf >= self.d {
                    (self.a * xf + self.b).powf(self.g) + self.e
                } else {
                    self.c * xf
                }
            }
            4 => {
                if xf >= self.d {
                    (self.a * xf + self.b).powf(self.g) + self.e
                } else {
                    (self.c * xf + self.f).powf(self.g)
                }
            }
            _ => xf,
        };
        (y as f32).clamp(0.0, 1.0)
    }

    fn encode(&self, y: f32) -> f32 {
        let yf = f64::from(y);
        // Inverses mirror the decode branches; the decision boundary
        // is the value the branch produces at d (flat toes invert to
        // their domain edge, clamped into [0,1]).
        let x = match self.kind {
            0 => yf.powf(1.0 / self.g),
            1 => {
                if yf > 0.0 {
                    (yf.powf(1.0 / self.g) - self.b) / self.a
                } else {
                    0.0
                }
            }
            2 => {
                if yf > self.c {
                    ((yf - self.c).powf(1.0 / self.g) - self.b) / self.a
                } else {
                    (-self.b / self.a).max(0.0)
                }
            }
            3 => {
                // Linear toe below d: its ceiling value c·d separates.
                if yf > self.c * self.d {
                    ((yf - self.e).powf(1.0 / self.g) - self.b) / self.a
                } else {
                    yf / self.c
                }
            }
            4 => {
                let yd = (self.a * self.d + self.b).powf(self.g) + self.e;
                if yf >= yd {
                    ((yf - self.e).powf(1.0 / self.g) - self.b) / self.a
                } else {
                    (yf.max(0.0).powf(1.0 / self.g) - self.f) / self.c
                }
            }
            _ => yf,
        };
        (x as f32).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(trc: &IccTrc, xs: &[f32], tol: f32) {
        for &x in xs {
            let y = trc.decode(x);
            let back = trc.encode(y);
            assert!(
                (back - x).abs() <= tol,
                "{trc:?} round trip at {x}: {y} → {back}"
            );
            assert!((0.0..=1.0).contains(&y), "image at {x}: {y}");
        }
    }

    #[test]
    fn gamma_curves_round_trip() {
        round_trip(&IccTrc::Identity, &[0.0, 0.25, 0.5, 1.0], 0.0);
        round_trip(&IccTrc::Gamma(2.2), &[0.02, 0.25, 0.5, 0.9], 2e-3);
        round_trip(&IccTrc::Gamma(1.8), &[0.02, 0.25, 0.5, 0.9], 2e-3);
    }

    #[test]
    fn table_curves_round_trip() {
        // A 17-point sRGB-ish table.
        let table: Vec<u16> = (0..17)
            .map(|i| {
                let x = i as f32 / 16.0;
                let lin = if x <= 0.04045 {
                    x / 12.92
                } else {
                    ((x + 0.055) / 1.055).powf(2.4)
                };
                (lin * 65_535.0).round() as u16
            })
            .collect();
        let trc = IccTrc::Table(table);
        round_trip(&trc, &[0.05, 0.2, 0.4, 0.6, 0.8, 0.95], 0.03);
        // Endpoints and monotonicity.
        assert!((trc.decode(0.0)).abs() < 1e-6);
        assert!((trc.decode(1.0) - 1.0).abs() < 1e-6);
        let mut prev = -1.0;
        for i in 0..=64 {
            let v = trc.decode(i as f32 / 64.0);
            assert!(v >= prev);
            prev = v;
        }
    }

    #[test]
    fn parametric_curves_round_trip() {
        // Type 3 with the sRGB constants (the canonical v4 sRGB TRC:
        // linear below the knee, power above) — decode cross-checked
        // against the closed form on both branches.
        let srgb = ParametricCurve {
            kind: 3,
            g: 2.4,
            a: 1.0 / 1.055,
            b: 0.055 / 1.055,
            c: 1.0 / 12.92,
            d: 0.04045,
            e: 0.0,
            f: 0.0,
        };
        for x in [0.001f32, 0.02, 0.25, 0.5, 0.75, 1.0] {
            let want_lin = if x < 0.04045 {
                x / 12.92
            } else {
                ((x + 0.055) / 1.055).powf(2.4)
            };
            let got = IccTrc::Parametric(srgb).decode(x);
            assert!(
                (got - want_lin).abs() < 2e-3,
                "srgb para decode at {x}: {got} vs {want_lin}"
            );
        }
        round_trip(&IccTrc::Parametric(srgb), &[0.02, 0.25, 0.5, 0.9], 3e-3);
        // Type 4: generic constants, both branches exercised.
        let t4 = ParametricCurve {
            kind: 4,
            g: 2.2,
            a: 0.9,
            b: 0.05,
            c: 1.1,
            d: 0.5,
            e: 0.02,
            f: 0.01,
        };
        round_trip(&IccTrc::Parametric(t4), &[0.05, 0.3, 0.6, 0.9], 3e-3);
        // Type 1/2/3 spot round trips.
        let t1 = ParametricCurve {
            kind: 1,
            g: 2.4,
            a: 1.0 / 1.055,
            b: 0.055 / 1.055,
            ..Default::default()
        };
        round_trip(&IccTrc::Parametric(t1), &[0.05, 0.5, 0.9], 3e-3);
        let t2 = ParametricCurve {
            kind: 2,
            g: 2.4,
            a: 1.0 / 1.055,
            b: 0.055 / 1.055,
            c: 0.01,
            ..Default::default()
        };
        round_trip(&IccTrc::Parametric(t2), &[0.05, 0.5, 0.9], 3e-3);
        let t3 = ParametricCurve {
            kind: 3,
            g: 2.4,
            a: 1.0 / 1.055,
            b: 0.055 / 1.055,
            c: 1.0 / 12.92,
            d: 0.04045,
            e: 0.0,
            f: 0.0,
        };
        round_trip(&IccTrc::Parametric(t3), &[0.02, 0.25, 0.5, 0.9], 3e-3);
    }

    #[test]
    fn as_gamma_extraction() {
        assert_eq!(IccTrc::Gamma(2.2).as_gamma(), Some(2.2));
        assert_eq!(IccTrc::Identity.as_gamma(), None);
        assert_eq!(IccTrc::Table(vec![1u16, 2]).as_gamma(), None);
        let p0 = ParametricCurve {
            kind: 0,
            g: 2.2,
            ..Default::default()
        };
        assert_eq!(IccTrc::Parametric(p0).as_gamma(), Some(2.2));
    }
}
