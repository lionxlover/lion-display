//! ICC tag-payload parsing — the byte-level readers.
//!
//! Everything that decodes one tag's payload out of the profile
//! buffer lives here: big-endian scalars, the s15Fixed16 grid, and
//! the payload readers for `XYZType`/`curveType`/`parametricCurveType`/
//! text records/`sf32` chromatic adaptations. The model-level
//! orchestration (header, tag table, the [`crate::icc::IccProfile`]
//! assembly) lives in [`crate::icc`]; the fixed-point values are
//! returned as exact f64 (the round-trip contract).
//!
//! Rejection is typed ([`crate::icc::IccError`]) and total: no
//! payload reader ever panics or returns a partial value.

use crate::icc::IccError;
use crate::icc_trc::{IccTrc, ParametricCurve};

const SIG_XYZ: u32 = u32::from_be_bytes(*b"XYZ ");
const SIG_CURV: u32 = u32::from_be_bytes(*b"curv");
const SIG_PARA: u32 = u32::from_be_bytes(*b"para");
const SIG_MLUC: u32 = u32::from_be_bytes(*b"mluc");
const SIG_TEXT: u32 = u32::from_be_bytes(*b"text");
const SIG_SF32: u32 = u32::from_be_bytes(*b"sf32");
const TAG_DESC: u32 = u32::from_be_bytes(*b"desc");

pub(crate) fn be_u16(b: &[u8], off: usize) -> u16 {
    u16::from_be_bytes([b[off], b[off + 1]])
}

pub(crate) fn be_u32(b: &[u8], off: usize) -> u32 {
    u32::from_be_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

pub(crate) fn s15f16(b: &[u8], off: usize) -> Result<f64, IccError> {
    let raw = be_u32(b, off) as i32;
    let v = f64::from(raw) / 65_536.0;
    if !v.is_finite() || v.abs() >= 32_767.0 {
        return Err(IccError::BadFixedPoint);
    }
    Ok(v)
}

fn check_sig(b: &[u8], off: usize, want: u32) -> Result<(), IccError> {
    if be_u32(b, off) == want {
        Ok(())
    } else {
        Err(IccError::BadTagType)
    }
}

pub(crate) fn parse_xyz(b: &[u8], off: usize, len: usize) -> Result<[f64; 3], IccError> {
    if len < 20 {
        return Err(IccError::BadTagData);
    }
    check_sig(b, off, SIG_XYZ)?;
    Ok([
        s15f16(b, off + 8)?,
        s15f16(b, off + 12)?,
        s15f16(b, off + 16)?,
    ])
}

pub(crate) fn parse_trc(b: &[u8], off: usize, len: usize) -> Result<IccTrc, IccError> {
    let sig = be_u32(b, off);
    if sig == SIG_CURV {
        if len < 12 {
            return Err(IccError::BadTagData);
        }
        let count = be_u32(b, off + 8) as usize;
        match count {
            0 => Ok(IccTrc::Identity),
            1 => Ok(IccTrc::Gamma(f64::from(be_u16(b, off + 12)) / 256.0)),
            _ => {
                if len < 12 + count * 2 || count > 65_536 {
                    return Err(IccError::BadTagData);
                }
                let mut table = Vec::with_capacity(count);
                for i in 0..count {
                    table.push(be_u16(b, off + 12 + i * 2));
                }
                // Strictly increasing: required for the inverse.
                for w in table.windows(2) {
                    if w[0] >= w[1] {
                        return Err(IccError::BadTrc);
                    }
                }
                Ok(IccTrc::Table(table))
            }
        }
    } else if sig == SIG_PARA {
        if len < 12 {
            return Err(IccError::BadTagData);
        }
        let kind = be_u16(b, off + 8);
        let n_params = match kind {
            0 => 1,
            1 => 3,
            2 => 4,
            3 => 5,
            4 => 7,
            _ => return Err(IccError::BadTagType),
        };
        if len < 12 + n_params * 4 {
            return Err(IccError::BadTagData);
        }
        let mut p = [0.0f64; 7];
        for (i, v) in p.iter_mut().enumerate().take(n_params) {
            *v = s15f16(b, off + 12 + i * 4)?;
        }
        let curve = ParametricCurve {
            kind: kind as u8,
            g: p[0],
            a: p[1],
            b: p[2],
            c: p[3],
            d: p[4],
            e: p[5],
            f: p[6],
        };
        // Invertibility: positive exponent, non-zero multipliers where
        // the inverse divides.
        if curve.g <= 0.0
            || (kind >= 1 && curve.a == 0.0)
            || ((kind == 3 || kind == 4) && curve.c == 0.0)
        {
            return Err(IccError::BadTrc);
        }
        if kind == 0 {
            Ok(IccTrc::Gamma(curve.g))
        } else {
            Ok(IccTrc::Parametric(curve))
        }
    } else {
        Err(IccError::BadTagType)
    }
}

pub(crate) fn parse_text_like(b: &[u8], off: usize, len: usize) -> Result<String, IccError> {
    let sig = be_u32(b, off);
    if sig == TAG_DESC {
        // v2 textDescriptionType: ASCII count + bytes (Unicode/script
        // tails ignored — canonical fixtures are ASCII).
        if len < 12 {
            return Err(IccError::BadTagData);
        }
        let count = be_u32(b, off + 8) as usize;
        if count > len - 12 || count > 4096 {
            return Err(IccError::BadTagData);
        }
        let bytes = &b[off + 12..off + 12 + count];
        let end = bytes.iter().position(|&c| c == 0).unwrap_or(bytes.len());
        Ok(String::from_utf8_lossy(&bytes[..end]).into_owned())
    } else if sig == SIG_MLUC {
        // v4 multiLocalizedUnicode: take the first record.
        if len < 28 {
            return Err(IccError::BadTagData);
        }
        let count = be_u32(b, off + 8) as usize;
        if count == 0 || len < 28 + count * 12 {
            return Err(IccError::BadTagData);
        }
        let str_len = be_u32(b, off + 20) as usize;
        let str_off = be_u32(b, off + 24) as usize;
        if str_len % 2 != 0 || str_off.saturating_add(str_len) > len {
            return Err(IccError::BadTagData);
        }
        let units: Vec<u16> = (0..str_len / 2)
            .map(|i| be_u16(b, off + str_off + i * 2))
            .collect();
        Ok(String::from_utf16_lossy(&units))
    } else if sig == SIG_TEXT {
        let bytes = &b[off + 8..off + len];
        let end = bytes.iter().position(|&c| c == 0).unwrap_or(bytes.len());
        Ok(String::from_utf8_lossy(&bytes[..end]).into_owned())
    } else {
        Err(IccError::BadTagType)
    }
}

pub(crate) fn parse_chad(b: &[u8], off: usize, len: usize) -> Result<[f64; 9], IccError> {
    if len < 8 + 36 {
        return Err(IccError::BadTagData);
    }
    check_sig(b, off, SIG_SF32)?;
    let mut m = [0.0f64; 9];
    for (i, v) in m.iter_mut().enumerate() {
        *v = s15f16(b, off + 8 + i * 4)?;
    }
    if crate::icc::matrix_det64(&m).abs() < 1e-9 {
        return Err(IccError::BadTrc);
    }
    Ok(m)
}
