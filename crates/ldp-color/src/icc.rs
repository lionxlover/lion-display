//! ICC v2/v4 matrix-TRC profile import.
//!
//! Scope (architecture §12, "parametric-first"): **display-class
//! (`mntr`) RGB matrix/TRC profiles with XYZ PCS** — the profile class
//! surface descriptions are built from. Lab/LUT-based profiles are out
//! of scope and rejected with typed errors (documented, never silently
//! mishandled). The parsed [`IccProfile`] model stores **f64** values
//! exactly as the fixed-point wire encodings carry them, so the
//! canonical writer ([`crate::icc_write`]) can re-serialize without
//! drift — the round-trip exit criterion.
//!
//! What the import produces:
//!
//! * the model itself (description, white point, RGB→PCS columns, TRCs,
//!   optional `chad`),
//! * [`IccProfile::native_rgb_to_xyz`]: the **display-native (D65)**
//!   RGB→XYZ matrix — PCS values are D50-adapted, so the import applies
//!   the exact inverse of the profile's `chad` when present (v4), or
//!   the Bradford D50→D65 inverse for v2 profiles (the recorded v2
//!   convention, what lcms does),
//! * [`IccProfile::resolve`]: the closest parametric
//!   [`ColorDescription`] (the `ldp.color.color_profile.description`
//!   contract: primaries fitted by chromaticity distance, transfer
//!   fitted by curve error, luminances supplied by the caller —
//!   matrix/TRC profiles carry none).
//!
//! Malformed input fails with [`IccError`] — never a panic, never a
//! partial profile (the validation-before-allocation doctrine).

use crate::icc_parse::{be_u32, parse_chad, parse_text_like, parse_trc, parse_xyz, s15f16};
use crate::icc_trc::IccTrc;
use crate::matrix;
use crate::transfer::decode_transfer;
use ldp_core::color::{ColorDescription, Luminance, Primaries, TransferFunction};

const TAG_DESC: u32 = u32::from_be_bytes(*b"desc");
const TAG_CPRT: u32 = u32::from_be_bytes(*b"cprt");
const TAG_WTPT: u32 = u32::from_be_bytes(*b"wtpt");
const TAG_RXYZ: u32 = u32::from_be_bytes(*b"rXYZ");
const TAG_GXYZ: u32 = u32::from_be_bytes(*b"gXYZ");
const TAG_BXYZ: u32 = u32::from_be_bytes(*b"bXYZ");
const TAG_RTRC: u32 = u32::from_be_bytes(*b"rTRC");
const TAG_GTRC: u32 = u32::from_be_bytes(*b"gTRC");
const TAG_BTRC: u32 = u32::from_be_bytes(*b"bTRC");
const TAG_KTRC: u32 = u32::from_be_bytes(*b"kTRC");
const TAG_CHAD: u32 = u32::from_be_bytes(*b"chad");

const D50: [f64; 3] = [0.964_2, 1.0, 0.824_9];

/// ICC parse/rejection failures.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum IccError {
    /// Smaller than a header plus tag table.
    TooSmall,
    /// Header size word disagrees with the buffer length.
    BadSize,
    /// Major version outside 2..=4.
    UnsupportedVersion,
    /// Device class is not `mntr` (matrix/TRC display profiles only).
    BadClass,
    /// Data colour space is not `RGB `.
    BadSpace,
    /// PCS is not `XYZ ` (Lab needs LUTs — out of scope).
    BadPcs,
    /// Missing the `acsp` signature.
    BadSignature,
    /// Header illuminant is not PCS D50.
    BadIlluminant,
    /// Tag table malformed (count/offsets/duplicates).
    BadTagTable,
    /// A required tag is missing.
    MissingTag,
    /// A tag payload's signature/type is wrong for its role.
    BadTagType,
    /// A tag payload is truncated or out of bounds.
    BadTagData,
    /// A TRC is non-invertible (non-monotone table, degenerate params).
    BadTrc,
    /// The media white point is not the D50-adapted white.
    BadWhitePoint,
    /// A fixed-point value is out of range.
    BadFixedPoint,
}

impl core::fmt::Display for IccError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let s = match self {
            Self::TooSmall => "profile smaller than a header",
            Self::BadSize => "header size word disagrees with length",
            Self::UnsupportedVersion => "major version outside 2..=4",
            Self::BadClass => "device class is not mntr",
            Self::BadSpace => "data space is not RGB",
            Self::BadPcs => "PCS is not XYZ (Lab/LUT profiles unsupported)",
            Self::BadSignature => "missing acsp signature",
            Self::BadIlluminant => "header illuminant is not D50",
            Self::BadTagTable => "malformed tag table",
            Self::MissingTag => "required tag missing",
            Self::BadTagType => "tag payload type wrong for its role",
            Self::BadTagData => "truncated or out-of-bounds tag data",
            Self::BadTrc => "non-invertible TRC",
            Self::BadWhitePoint => "media white is not the D50-adapted white",
            Self::BadFixedPoint => "fixed-point value out of range",
        };
        f.write_str(s)
    }
}

impl std::error::Error for IccError {}

/// The parsed matrix/TRC profile model (f64-exact fixed-point values).
#[derive(Clone, PartialEq, Debug)]
pub struct IccProfile {
    /// Major version (2, 3 or 4); re-serialization is canonical (2.0 /
    /// 4.0 words).
    pub version: u8,
    /// Profile description (`desc` tag).
    pub description: String,
    /// Copyright (`cprt` tag), when present.
    pub copyright: Option<String>,
    /// Rendering intent (0 perceptual, 1 colorimetric, ...).
    pub rendering_intent: u32,
    /// Media white point in PCS XYZ (≈ D50 by construction).
    pub white_point: [f64; 3],
    /// RGB→PCS columns: `[rXYZ, gXYZ, bXYZ]`.
    pub columns: [[f64; 3]; 3],
    /// The three channel TRCs.
    pub trcs: [IccTrc; 3],
    /// The chromatic adaptation matrix (native→PCS, row-major), when
    /// the profile carries a `chad` tag.
    pub chad: Option<[f64; 9]>,
}

/// Parse a complete profile.
///
/// # Errors
/// [`IccError`] on any structural problem — see the variant docs.
pub fn parse(bytes: &[u8]) -> Result<IccProfile, IccError> {
    if bytes.len() < 132 {
        return Err(IccError::TooSmall);
    }
    if be_u32(bytes, 0) as usize != bytes.len() {
        return Err(IccError::BadSize);
    }
    let major = (be_u32(bytes, 8) >> 24) as u8;
    if !matches!(major, 2..=4) {
        return Err(IccError::UnsupportedVersion);
    }
    if &bytes[12..16] != b"mntr" {
        return Err(IccError::BadClass);
    }
    if &bytes[16..20] != b"RGB " {
        return Err(IccError::BadSpace);
    }
    if &bytes[20..24] != b"XYZ " {
        return Err(IccError::BadPcs);
    }
    if &bytes[36..40] != b"acsp" {
        return Err(IccError::BadSignature);
    }
    let illum = [s15f16(bytes, 68)?, s15f16(bytes, 72)?, s15f16(bytes, 76)?];
    for i in 0..3 {
        if (illum[i] - D50[i]).abs() > 0.05 {
            return Err(IccError::BadIlluminant);
        }
    }
    let intent = be_u32(bytes, 64);
    let count = be_u32(bytes, 128) as usize;
    let table_end = 132 + count * 12;
    if table_end > bytes.len() || count > 1024 {
        return Err(IccError::BadTagTable);
    }
    let mut tags: Vec<(u32, usize, usize)> = Vec::with_capacity(count);
    for i in 0..count {
        let base = 132 + i * 12;
        let sig = be_u32(bytes, base);
        let off = be_u32(bytes, base + 4) as usize;
        let len = be_u32(bytes, base + 8) as usize;
        if off < table_end || off.saturating_add(len) > bytes.len() || len < 4 {
            return Err(IccError::BadTagTable);
        }
        if tags.iter().any(|&(s, _, _)| s == sig) {
            return Err(IccError::BadTagTable);
        }
        tags.push((sig, off, len));
    }
    let find = |sig: u32| tags.iter().find(|&&(s, _, _)| s == sig).copied();
    let tag_red = find(TAG_RXYZ).ok_or(IccError::MissingTag)?;
    let tag_green = find(TAG_GXYZ).ok_or(IccError::MissingTag)?;
    let tag_blue = find(TAG_BXYZ).ok_or(IccError::MissingTag)?;
    let tag_white = find(TAG_WTPT).ok_or(IccError::MissingTag)?;
    let tag_desc = find(TAG_DESC).ok_or(IccError::MissingTag)?;
    let white = parse_xyz(bytes, tag_white.1, tag_white.2)?;
    for i in 0..3 {
        if (white[i] - D50[i]).abs() > 0.05 {
            return Err(IccError::BadWhitePoint);
        }
    }
    let columns = [
        parse_xyz(bytes, tag_red.1, tag_red.2)?,
        parse_xyz(bytes, tag_green.1, tag_green.2)?,
        parse_xyz(bytes, tag_blue.1, tag_blue.2)?,
    ];
    let trcs =
        if let (Some(rt), Some(gt), Some(bt)) = (find(TAG_RTRC), find(TAG_GTRC), find(TAG_BTRC)) {
            [
                parse_trc(bytes, rt.1, rt.2)?,
                parse_trc(bytes, gt.1, gt.2)?,
                parse_trc(bytes, bt.1, bt.2)?,
            ]
        } else {
            // Sloppy profiles sharing a kTRC: one curve for all three.
            let tag_gray = find(TAG_KTRC).ok_or(IccError::MissingTag)?;
            let trc = parse_trc(bytes, tag_gray.1, tag_gray.2)?;
            [trc.clone(), trc.clone(), trc]
        };
    let chad = match find(TAG_CHAD) {
        Some(c) => Some(parse_chad(bytes, c.1, c.2)?),
        None => None,
    };
    let copyright = match find(TAG_CPRT) {
        Some(c) => Some(parse_text_like(bytes, c.1, c.2)?),
        None => None,
    };
    Ok(IccProfile {
        version: major,
        description: parse_text_like(bytes, tag_desc.1, tag_desc.2)?,
        copyright,
        rendering_intent: intent,
        white_point: white,
        columns,
        trcs,
        chad,
    })
}

pub(crate) fn matrix_det64(m: &[f64; 9]) -> f64 {
    m[0] * (m[4] * m[8] - m[5] * m[7]) - m[1] * (m[3] * m[8] - m[5] * m[6])
        + m[2] * (m[3] * m[7] - m[4] * m[6])
}

fn mul64(a: &[f64; 9], b: &[f64; 9]) -> [f64; 9] {
    let mut out = [0.0; 9];
    for r in 0..3 {
        for c in 0..3 {
            out[r * 3 + c] = a[r * 3] * b[c] + a[r * 3 + 1] * b[3 + c] + a[r * 3 + 2] * b[6 + c];
        }
    }
    out
}

fn columns_to_matrix64(c: &[[f64; 3]; 3]) -> [f64; 9] {
    [
        c[0][0], c[1][0], c[2][0], c[0][1], c[1][1], c[2][1], c[0][2], c[1][2], c[2][2],
    ]
}

impl IccProfile {
    /// The display-native (D65) RGB→XYZ matrix, row-major.
    ///
    /// `chad`⁻¹ applied exactly when the profile carries one; the
    /// Bradford D50→D65 inverse otherwise (the v2 convention).
    #[must_use]
    pub fn native_rgb_to_xyz(&self) -> [f64; 9] {
        let pcs = columns_to_matrix64(&self.columns);
        let adapt: [f64; 9] = if let Some(c) = &self.chad {
            // Exact inverse of the recorded adaptation.
            let det = matrix_det64(c);
            let inv = 1.0 / det;
            [
                (c[4] * c[8] - c[5] * c[7]) * inv,
                (c[2] * c[7] - c[1] * c[8]) * inv,
                (c[1] * c[5] - c[2] * c[4]) * inv,
                (c[5] * c[6] - c[3] * c[8]) * inv,
                (c[0] * c[8] - c[2] * c[6]) * inv,
                (c[2] * c[3] - c[0] * c[5]) * inv,
                (c[3] * c[7] - c[4] * c[6]) * inv,
                (c[1] * c[6] - c[0] * c[7]) * inv,
                (c[0] * c[4] - c[1] * c[3]) * inv,
            ]
        } else {
            // f32 Bradford inverse, widened — the published
            // constants carry 4 decimal places anyway.
            let f = matrix::bradford_d50_to_d65();
            [
                f64::from(f[0]),
                f64::from(f[1]),
                f64::from(f[2]),
                f64::from(f[3]),
                f64::from(f[4]),
                f64::from(f[5]),
                f64::from(f[6]),
                f64::from(f[7]),
                f64::from(f[8]),
            ]
        };
        mul64(&adapt, &pcs)
    }

    /// Resolve to the closest parametric description (the
    /// `color_profile.description` contract).
    ///
    /// `luminances` supplies the (min, max, reference) luminances —
    /// matrix/TRC profiles carry none; `ColorDescription::srgb_sdr`'s
    /// values are the natural default.
    #[must_use]
    pub fn resolve(&self, luminances: (Luminance, Luminance, Luminance)) -> ResolvedDescription {
        let native = self.native_rgb_to_xyz();
        // Chromaticities of the native primaries and white.
        let chroma = |col: [f64; 3]| -> (f64, f64) {
            let s = col[0] + col[1] + col[2];
            (col[0] / s, col[1] / s)
        };
        let native_cols = [
            [native[0], native[3], native[6]],
            [native[1], native[4], native[7]],
            [native[2], native[5], native[8]],
        ];
        let native_white = [
            native_cols[0][0] + native_cols[1][0] + native_cols[2][0],
            native_cols[0][1] + native_cols[1][1] + native_cols[2][1],
            native_cols[0][2] + native_cols[1][2] + native_cols[2][2],
        ];
        let mut best = Primaries::Bt709;
        let mut best_err = f64::INFINITY;
        for cand in [Primaries::Bt709, Primaries::DciP3, Primaries::Bt2020] {
            let m = matrix::rgb_to_xyz(cand);
            let cols = [
                [f64::from(m[0]), f64::from(m[3]), f64::from(m[6])],
                [f64::from(m[1]), f64::from(m[4]), f64::from(m[7])],
                [f64::from(m[2]), f64::from(m[5]), f64::from(m[8])],
            ];
            let white = [
                cols[0][0] + cols[1][0] + cols[2][0],
                cols[0][1] + cols[1][1] + cols[2][1],
                cols[0][2] + cols[1][2] + cols[2][2],
            ];
            let mut err = 0.0f64;
            for i in 0..3 {
                let (nx, ny) = chroma(native_cols[i]);
                let (cx, cy) = chroma(cols[i]);
                err = err.max(((nx - cx).powi(2) + (ny - cy).powi(2)).sqrt());
            }
            let (nx, ny) = chroma(native_white);
            let (cx, cy) = chroma(white);
            err = err.max(((nx - cx).powi(2) + (ny - cy).powi(2)).sqrt());
            if err < best_err {
                best_err = err;
                best = cand;
            }
        }
        // Transfer fit over the display-relative SDR curves (PQ/HLG
        // are absolute/scene-referred — not ICC TRC shapes).
        let mut best_tf = TransferFunction::Srgb;
        let mut best_tf_err = f64::INFINITY;
        for tf in [
            TransferFunction::Linear,
            TransferFunction::Srgb,
            TransferFunction::Gamma22,
            TransferFunction::Gamma28,
        ] {
            let mut err = 0.0f64;
            for i in 1..=20 {
                let x = i as f32 / 20.0;
                let want = decode_transfer(tf, x);
                let got = self.trcs[0].decode(x);
                err = f64::from((want - got).abs()).max(err);
            }
            if err < best_tf_err {
                best_tf_err = err;
                best_tf = tf;
            }
        }
        ResolvedDescription {
            description: ColorDescription {
                primaries: best,
                transfer: best_tf,
                range: ldp_core::color::ColorRange::Full,
                luminance_min: luminances.0,
                luminance_max: luminances.1,
                reference_white: luminances.2,
            },
            primaries_error: best_err as f32,
            transfer_error: best_tf_err as f32,
        }
    }
}

/// The resolution result with its fit diagnostics.
#[derive(Clone, Copy, Debug)]
pub struct ResolvedDescription {
    /// The closest parametric description.
    pub description: ColorDescription,
    /// Max xy distance of the fitted primaries (0 ≈ exact).
    pub primaries_error: f32,
    /// Max curve error of the fitted transfer (0 ≈ exact).
    pub transfer_error: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_short_and_malformed_headers() {
        assert_eq!(parse(&[]), Err(IccError::TooSmall));
        let mut b = vec![0u8; 200];
        // Size word disagrees.
        assert_eq!(parse(&b), Err(IccError::BadSize));
        b[0..4].copy_from_slice(&200u32.to_be_bytes());
        // Version 5.
        b[8..12].copy_from_slice(&0x0500_0000u32.to_be_bytes());
        assert_eq!(parse(&b), Err(IccError::UnsupportedVersion));
        b[8..12].copy_from_slice(&0x0200_0000u32.to_be_bytes());
        assert_eq!(parse(&b), Err(IccError::BadClass));
        b[12..16].copy_from_slice(b"mntr");
        assert_eq!(parse(&b), Err(IccError::BadSpace));
        b[16..20].copy_from_slice(b"RGB ");
        assert_eq!(parse(&b), Err(IccError::BadPcs));
        b[20..24].copy_from_slice(b"XYZ ");
        assert_eq!(parse(&b), Err(IccError::BadSignature));
        b[36..40].copy_from_slice(b"acsp");
        // Illuminant zeros → not D50.
        assert_eq!(parse(&b), Err(IccError::BadIlluminant));
        let d50 = |v: f64| ((v * 65_536.0) as i32).to_be_bytes();
        b[68..72].copy_from_slice(&d50(0.9642));
        b[72..76].copy_from_slice(&d50(1.0));
        b[76..80].copy_from_slice(&d50(0.8249));
        // Zero tags → missing required tags.
        b[128..132].copy_from_slice(&0u32.to_be_bytes());
        assert_eq!(parse(&b), Err(IccError::MissingTag));
        // Absurd tag count.
        b[128..132].copy_from_slice(&5000u32.to_be_bytes());
        assert_eq!(parse(&b), Err(IccError::BadTagTable));
    }

    #[test]
    fn fixed_point_helpers_round_trip() {
        // s15Fixed16 survives parse exactly (the model stores f64).
        let mut b = [0u8; 4];
        for raw in [0i32, 1, -1, 65_536, -65_536, 0x4000_0000, -0x4000_0000] {
            b.copy_from_slice(&raw.to_be_bytes());
            let v = s15f16(&b, 0).unwrap();
            assert_eq!((v * 65_536.0) as i32, raw, "s15f16 exact");
        }
        // Out of range (|v| >= 32767) is rejected.
        b.copy_from_slice(&0x7FFF_FFFFu32.to_be_bytes());
        assert_eq!(s15f16(&b, 0), Err(IccError::BadFixedPoint));
    }
}
