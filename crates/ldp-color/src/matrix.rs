//! 3×3 color matrices and the chromaticity derivation.
//!
//! Row-major `[f32; 9]` matrices — the same convention as the renderer's
//! Phase 8 hook (which this crate canonicalizes; the cross-consistency
//! suite pins the two derivations equal). The RGB↔XYZ matrices are
//! derived from the definitional CIE 1931 xy chromaticities
//! ([`Primaries::chromaticities`]): primary direction vectors as columns,
//! scaled so the columns sum to the white point. Every shipped matrix is
//! anchored in the unit tests against the **published** values (sRGB /
//! BT.709, BT.2020 and DCI-P3 matrices rounded from the ITU and IEC
//! documents), so a derivation regression cannot hide.
//!
//! This module also carries the **Bradford chromatic-adaptation**
//! matrices (D65↔D50), the published Lindbloom constants, used by the ICC
//! import path: ICC PCS values are D50-adapted, and recovering
//! display-native (D65) primaries requires the inverse adaptation.

use ldp_core::color::Primaries;

/// Row-major 3×3 matrix: `[r0c0, r0c1, r0c2, r1c0, ...]`.
pub type Matrix3 = [f32; 9];

/// Identity matrix.
pub const IDENTITY: Matrix3 = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];

/// Apply: `[m] · v` for a column vector `v`.
#[must_use]
pub fn apply3(m: &Matrix3, v: [f32; 3]) -> [f32; 3] {
    [
        m[0] * v[0] + m[1] * v[1] + m[2] * v[2],
        m[3] * v[0] + m[4] * v[1] + m[5] * v[2],
        m[6] * v[0] + m[7] * v[1] + m[8] * v[2],
    ]
}

/// Matrix product `a · b`.
#[must_use]
pub fn mul3(a: &Matrix3, b: &Matrix3) -> Matrix3 {
    let mut out = [0.0f32; 9];
    for r in 0..3 {
        for c in 0..3 {
            out[r * 3 + c] = a[r * 3] * b[c] + a[r * 3 + 1] * b[3 + c] + a[r * 3 + 2] * b[6 + c];
        }
    }
    out
}

/// Transpose.
#[must_use]
pub fn transpose3(m: &Matrix3) -> Matrix3 {
    [m[0], m[3], m[6], m[1], m[4], m[7], m[2], m[5], m[8]]
}

/// Determinant via the cofactor expansion.
#[must_use]
pub fn det3(m: &Matrix3) -> f32 {
    m[0] * (m[4] * m[8] - m[5] * m[7]) - m[1] * (m[3] * m[8] - m[5] * m[6])
        + m[2] * (m[3] * m[7] - m[4] * m[6])
}

/// Inverse via the adjugate; `None` when (near-)singular.
///
/// The chromaticity-derived matrices of this module are invertible by
/// construction (three non-degenerate primaries); this returns `None`
/// only for pathological inputs, e.g. a degenerate ICC matrix.
#[must_use]
pub fn invert3(m: &Matrix3) -> Option<Matrix3> {
    let det = det3(m);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    Some([
        (m[4] * m[8] - m[5] * m[7]) * inv,
        (m[2] * m[7] - m[1] * m[8]) * inv,
        (m[1] * m[5] - m[2] * m[4]) * inv,
        (m[5] * m[6] - m[3] * m[8]) * inv,
        (m[0] * m[8] - m[2] * m[6]) * inv,
        (m[2] * m[3] - m[0] * m[5]) * inv,
        (m[3] * m[7] - m[4] * m[6]) * inv,
        (m[1] * m[6] - m[0] * m[7]) * inv,
        (m[0] * m[4] - m[1] * m[3]) * inv,
    ])
}

/// Build a matrix whose **columns** are `c0, c1, c2`.
#[must_use]
pub fn from_columns(c0: [f32; 3], c1: [f32; 3], c2: [f32; 3]) -> Matrix3 {
    [
        c0[0], c1[0], c2[0], c0[1], c1[1], c2[1], c0[2], c1[2], c2[2],
    ]
}

/// CIE xy chromaticity → XYZ direction vector (Y = 1).
fn xyz_dir(x: f32, y: f32) -> [f32; 3] {
    [x / y, 1.0, (1.0 - x - y) / y]
}

/// RGB→XYZ for a primaries set (columns are the white-scaled primaries).
///
/// The derivation: each primary's direction vector is scaled so that the
/// three scaled columns sum exactly to the white point's direction vector
/// (the Cramer solution of `columns · s = white`).
#[must_use]
pub fn rgb_to_xyz(p: Primaries) -> Matrix3 {
    let c = p.chromaticities();
    let dirs = [
        xyz_dir(c[0], c[1]),
        xyz_dir(c[2], c[3]),
        xyz_dir(c[4], c[5]),
    ];
    let white = xyz_dir(c[6], c[7]);
    let scale = solve_columns(dirs, white);
    from_columns(
        [
            dirs[0][0] * scale[0],
            dirs[0][1] * scale[0],
            dirs[0][2] * scale[0],
        ],
        [
            dirs[1][0] * scale[1],
            dirs[1][1] * scale[1],
            dirs[1][2] * scale[1],
        ],
        [
            dirs[2][0] * scale[2],
            dirs[2][1] * scale[2],
            dirs[2][2] * scale[2],
        ],
    )
}

/// XYZ→RGB (the exact inverse of [`rgb_to_xyz`]).
#[must_use]
pub fn xyz_to_rgb(p: Primaries) -> Matrix3 {
    // rgb_to_xyz is invertible by construction; a None here would be a
    // defect in the chromaticity tables themselves.
    invert3(&rgb_to_xyz(p)).unwrap_or(IDENTITY)
}

/// The RGB→RGB conversion between two primaries sets (row-major).
///
/// `source RGB → XYZ → target RGB`. Identical primaries yield the exact
/// identity matrix (the fast-path contract shared with the renderer).
#[must_use]
pub fn primaries_matrix(source: Primaries, target: Primaries) -> Matrix3 {
    if source == target {
        return IDENTITY;
    }
    let s = rgb_to_xyz(source);
    let t_inv = xyz_to_rgb(target);
    mul3(&t_inv, &s)
}

/// The luma coefficients of a primaries set: the Y row of RGB→XYZ.
///
/// BT.709 → `(0.2126, 0.7152, 0.0722)`, BT.2020 →
/// `(0.2627, 0.6780, 0.0593)` — the definitional ITU values, used as
/// KAVT anchors in the tests.
#[must_use]
pub fn luma_coefficients(p: Primaries) -> [f32; 3] {
    let m = rgb_to_xyz(p);
    [m[3], m[4], m[5]]
}

/// Solve `columns · s = white` via scalar triple products (Cramer).
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

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// The Bradford cone-response matrix (von Kries style, published
/// Lindbloom constants), row-major.
pub const BRADFORD: Matrix3 = [
    0.8951, 0.2664, -0.1614, -0.7502, 1.7135, 0.0367, 0.0389, -0.0685, 1.0296,
];

/// Bradford D65→D50 adaptation (row-major).
///
/// The von Kries sandwich `B⁻¹ · diag(s) · B` with the Bradford cone
/// response [`BRADFORD`], `s = cone(D50)/cone(D65)`, D65 =
/// (0.95047, 1.0, 1.08883), D50 = (0.96422, 1.0, 0.82521). Its exact
/// inverse is the matrix published in the sRGB/P3 ICC profiles' `chad`
/// tags (see the unit test: [`bradford_d50_to_d65`] matches the
/// published 7-decimal values, and the forward maps the D65 white
/// exactly onto D50).
pub fn bradford_d65_to_d50() -> Matrix3 {
    let d65 = [0.950_47, 1.0, 1.088_83];
    let d50 = [0.964_22, 1.0, 0.825_21];
    let w_src = apply3(&BRADFORD, d65);
    let w_dst = apply3(&BRADFORD, d50);
    let scale = [
        w_dst[0] / w_src[0],
        w_dst[1] / w_src[1],
        w_dst[2] / w_src[2],
    ];
    // B⁻¹ · diag(scale) · B
    let b_inv = invert3(&BRADFORD).unwrap_or(IDENTITY);
    let scaled = [
        BRADFORD[0] * scale[0],
        BRADFORD[1] * scale[0],
        BRADFORD[2] * scale[0],
        BRADFORD[3] * scale[1],
        BRADFORD[4] * scale[1],
        BRADFORD[5] * scale[1],
        BRADFORD[6] * scale[2],
        BRADFORD[7] * scale[2],
        BRADFORD[8] * scale[2],
    ];
    mul3(&b_inv, &scaled)
}

/// Bradford D50→D65 adaptation: the exact inverse of
/// [`bradford_d65_to_d50`]. ICC import applies this to PCS (D50) matrix
/// columns to recover display-native (D65) primaries.
#[must_use]
pub fn bradford_d50_to_d65() -> Matrix3 {
    invert3(&bradford_d65_to_d50()).unwrap_or(IDENTITY)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32, tol: f32) -> bool {
        (a - b).abs() <= tol
    }

    fn assert_matrix_close(a: &Matrix3, b: &Matrix3, tol: f32, what: &str) {
        for i in 0..9 {
            assert!(
                close(a[i], b[i], tol),
                "{what} row-major [{i}]: {} vs {}",
                a[i],
                b[i]
            );
        }
    }

    #[test]
    fn matrix_algebra_basics() {
        let a = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 10.0];
        let ainv = invert3(&a).expect("invertible");
        assert_matrix_close(&mul3(&a, &ainv), &IDENTITY, 1e-5, "a·a⁻¹");
        assert_matrix_close(&mul3(&ainv, &a), &IDENTITY, 1e-5, "a⁻¹·a");
        assert_matrix_close(&mul3(&IDENTITY, &a), &a, 0.0, "identity left");
        // apply3 agrees with mul3 on basis images.
        for v in [
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.25, -0.5, 0.75],
        ] {
            let via_apply = apply3(&a, v);
            let vm = from_columns(v, [0.0; 3], [0.0; 3]);
            let via_mul = [mul3(&a, &vm)[0], mul3(&a, &vm)[3], mul3(&a, &vm)[6]];
            assert_eq!(via_apply, via_mul);
        }
        // Singular matrix is rejected.
        assert!(invert3(&[1.0, 2.0, 3.0, 2.0, 4.0, 6.0, 0.0, 0.0, 0.0]).is_none());
        assert_eq!(
            transpose3(&a),
            [1.0, 4.0, 7.0, 2.0, 5.0, 8.0, 3.0, 6.0, 10.0]
        );
    }

    #[test]
    fn srgb_bt709_matrix_matches_published() {
        // IEC 61966-2-1 / sRGB, full precision, D65.
        let expect = [
            0.412_391, 0.357_584, 0.180_481, 0.212_639, 0.715_169, 0.072_192, 0.019_331, 0.119_195,
            0.950_532,
        ];
        assert_matrix_close(&rgb_to_xyz(Primaries::Bt709), &expect, 2e-4, "sRGB→XYZ");
        // Luma row = BT.709 luma coefficients (published 4-dp form).
        let l = luma_coefficients(Primaries::Bt709);
        for (got, want) in l.iter().zip([0.2126, 0.7152, 0.0722]) {
            assert!(close(*got, want, 1e-4), "BT.709 luma {got} vs {want}");
        }
    }

    #[test]
    fn bt2020_matrix_matches_published() {
        // ITU-R BT.2020 / BT.2087, full precision, D65.
        let expect = [
            0.636_958, 0.144_617, 0.168_881, 0.262_700, 0.677_998, 0.059_302, 0.0, 0.028_073,
            1.060_985,
        ];
        assert_matrix_close(&rgb_to_xyz(Primaries::Bt2020), &expect, 2e-4, "BT.2020→XYZ");
        let l = luma_coefficients(Primaries::Bt2020);
        for (got, want) in l.iter().zip([0.2627, 0.6780, 0.0593]) {
            assert!(close(*got, want, 1e-4), "BT.2020 luma {got} vs {want}");
        }
    }

    #[test]
    fn dci_p3_matrix_matches_published() {
        // Display P3 (D65 variant), full precision.
        let expect = [
            0.486_571, 0.265_668, 0.198_217, 0.228_975, 0.691_739, 0.079_287, 0.0, 0.045_113,
            1.043_944,
        ];
        assert_matrix_close(&rgb_to_xyz(Primaries::DciP3), &expect, 2e-4, "P3→XYZ");
    }

    #[test]
    fn primaries_conversions_match_published() {
        // BT.709 → BT.2020 (published 4-dp, ITU-R BT.2087-class values).
        let expect = [
            0.6270, 0.3293, 0.0437, 0.0691, 0.9195, 0.0114, 0.0164, 0.0880, 0.8956,
        ];
        assert_matrix_close(
            &primaries_matrix(Primaries::Bt709, Primaries::Bt2020),
            &expect,
            1e-3,
            "BT.709→BT.2020",
        );
        // BT.2020 → BT.709 inverse (published 4-dp).
        let expect_inv = [
            1.6605, -0.5876, -0.0728, -0.1246, 1.1329, -0.0083, -0.0182, -0.1006, 1.1187,
        ];
        assert_matrix_close(
            &primaries_matrix(Primaries::Bt2020, Primaries::Bt709),
            &expect_inv,
            1e-3,
            "BT.2020→BT.709",
        );
        // Identity fast path is exact.
        for p in [Primaries::Bt709, Primaries::DciP3, Primaries::Bt2020] {
            assert_eq!(primaries_matrix(p, p), IDENTITY);
        }
        // Round trip through a third space composes back to identity.
        for (a, b) in [
            (Primaries::Bt709, Primaries::Bt2020),
            (Primaries::DciP3, Primaries::Bt709),
            (Primaries::Bt2020, Primaries::DciP3),
        ] {
            let round = mul3(&primaries_matrix(a, b), &primaries_matrix(b, a));
            assert_matrix_close(&round, &IDENTITY, 1e-5, "round trip");
        }
    }

    #[test]
    fn primaries_preserve_white_and_bases() {
        // Unit basis vectors map to the white-scaled primary columns and
        // white (1,1,1) maps to the D65 white XYZ, for every space.
        for p in [Primaries::Bt709, Primaries::DciP3, Primaries::Bt2020] {
            let m = rgb_to_xyz(p);
            let white = apply3(&m, [1.0, 1.0, 1.0]);
            // D65 white: X/Z from the chromaticity, Y = 1.
            let c = p.chromaticities();
            let wx = c[6] / c[7];
            let wz = (1.0 - c[6] - c[7]) / c[7];
            assert!(close(white[0], wx, 1e-5), "{} white X", p.to_wire());
            assert!(close(white[1], 1.0, 1e-5), "{} white Y", p.to_wire());
            assert!(close(white[2], wz, 1e-5), "{} white Z", p.to_wire());
            // Columns sum to white (checked via apply on (1,1,1) above);
            // primaries_matrix · (1,1,1) = (1,1,1): white is preserved by
            // every RGB↔RGB conversion.
            for t in [Primaries::Bt709, Primaries::DciP3, Primaries::Bt2020] {
                let conv = primaries_matrix(p, t);
                let mapped = apply3(&conv, [1.0, 1.0, 1.0]);
                for (i, c) in mapped.iter().enumerate() {
                    assert!(close(*c, 1.0, 1e-5), "white preserved {p:?}→{t:?} ch {i}");
                }
            }
        }
    }

    #[test]
    fn bradford_matrices_match_published() {
        // The famous published matrix (7-dp, the sRGB/P3 ICC `chad` tag)
        // maps PCS D50 white exactly onto the D65 display white — i.e.
        // it IS the D50→D65 direction (hand-verified: M·D50 = D65 to
        // five decimals).
        let expect_d50_to_d65 = [
            0.955_576_6,
            -0.023_039_3,
            0.063_163_6,
            -0.028_283_1,
            1.009_941_6,
            0.021_007_7,
            0.012_298_2,
            -0.020_483_0,
            1.329_909_8,
        ];
        assert_matrix_close(
            &bradford_d50_to_d65(),
            &expect_d50_to_d65,
            1e-4,
            "Bradford D50→D65 (published)",
        );
        // Forward and inverse compose to identity.
        let fwd = bradford_d65_to_d50();
        let back = bradford_d50_to_d65();
        assert_matrix_close(&mul3(&fwd, &back), &IDENTITY, 1e-5, "Bradford round trip");
        // D65 white adapts exactly onto D50 white.
        let d65 = [0.950_47, 1.0, 1.088_83];
        let d50 = apply3(&fwd, d65);
        for (got, want) in d50.iter().zip([0.964_22, 1.0, 0.825_21]) {
            assert!(close(*got, want, 1e-4), "D65→D50 white {got} vs {want}");
        }
        // And the published direction check: M·D50 = D65.
        let d50_w = [0.964_22, 1.0, 0.825_21];
        let to_d65 = apply3(&back, d50_w);
        for (got, want) in to_d65.iter().zip([0.950_47, 1.0, 1.088_83]) {
            assert!(close(*got, want, 1e-4), "D50→D65 white {got} vs {want}");
        }
    }
}
