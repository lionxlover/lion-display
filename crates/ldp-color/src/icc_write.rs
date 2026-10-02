//! The canonical ICC profile writer.
//!
//! Serializes an [`IccProfile`] back to bytes in **one canonical
//! layout**: fixed tag order (`desc, wtpt, rXYZ, gXYZ, bXYZ, rTRC,
//! gTRC, bTRC, cprt?, chad?`), 4-byte-aligned data blocks, zeroed
//! date/ID fields (byte-stable across runs — the determinism doctrine),
//! and a version word of exactly `0x0200_0000` or `0x0400_0000`.
//!
//! This is the fixture engine of the round-trip exit criterion:
//! `parse(write(m)) == m` **and** `write(parse(write(m)))` byte-identical,
//! for every wire-representable model. Wire-representable means the
//! f64 model values sit on the fixed-point grids the format carries
//! (s15Fixed16 = 1/65536 for XYZ/parametric/chad values, u8Fixed8 =
//! 1/256 for the v2 single-entry gamma); the writer quantizes to those
//! grids and the tests construct fixtures on them. A model off-grid
//! round-trips through its quantized neighbor — documented, never
//! silent (values are rounded, never truncated).
//!
//! Version rules: v2 profiles use `curveType` TRCs and `desc`/
//! `text` text records; v4 uses `paraType` for parametric TRCs and
//! `mluc` text records. A parametric TRC in a v2 profile is a typed
//! error (parametric curves are a v4 feature), as is a non-ASCII
//! description in v2 (the v2 record is ASCII).

use crate::icc::IccProfile;
use crate::icc_trc::IccTrc;

/// Writer failures.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WriteError {
    /// A parametric TRC cannot be serialized into a v2 profile.
    ParametricNeedsV4,
    /// A v2 description must be ASCII.
    NonAsciiDescription,
    /// A model value is off the representable fixed-point range.
    FixedPointOutOfRange,
    /// Gamma outside the u8Fixed8 range for a v2 curveType.
    GammaOutOfRange,
}

impl core::fmt::Display for WriteError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let s = match self {
            Self::ParametricNeedsV4 => "parametric TRCs require a v4 profile",
            Self::NonAsciiDescription => "v2 descriptions must be ASCII",
            Self::FixedPointOutOfRange => "value off the s15Fixed16 range",
            Self::GammaOutOfRange => "gamma outside the u8Fixed8 range",
        };
        f.write_str(s)
    }
}

impl std::error::Error for WriteError {}

fn s15f16(v: f64) -> Result<[u8; 4], WriteError> {
    let scaled = (v * 65_536.0).round();
    if !(-2_147_483_648.0..=2_147_483_647.0).contains(&scaled) {
        return Err(WriteError::FixedPointOutOfRange);
    }
    Ok((scaled as i32).to_be_bytes())
}

fn u8f8(v: f64) -> Result<[u8; 2], WriteError> {
    let scaled = (v * 256.0).round();
    if !(0.0..=65_535.0).contains(&scaled) {
        return Err(WriteError::GammaOutOfRange);
    }
    Ok((scaled as u16).to_be_bytes())
}

fn push_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
}

fn push_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_be_bytes());
}

fn pad4(out: &mut Vec<u8>) {
    while out.len() % 4 != 0 {
        out.push(0);
    }
}

fn xyz_tag(xyz: [f64; 3]) -> Result<Vec<u8>, WriteError> {
    let mut t = Vec::with_capacity(20);
    t.extend_from_slice(b"XYZ ");
    push_u32(&mut t, 0);
    for v in xyz {
        t.extend_from_slice(&s15f16(v)?);
    }
    Ok(t)
}

fn trc_tag(trc: &IccTrc, version: u8) -> Result<Vec<u8>, WriteError> {
    let mut t = Vec::new();
    match trc {
        IccTrc::Identity => {
            t.extend_from_slice(b"curv");
            push_u32(&mut t, 0);
            push_u32(&mut t, 0);
        }
        IccTrc::Gamma(g) => {
            t.extend_from_slice(b"curv");
            push_u32(&mut t, 0);
            push_u32(&mut t, 1);
            t.extend_from_slice(&u8f8(*g)?);
            // curveType pads the single u16 entry to 4 bytes.
            t.extend_from_slice(&[0, 0]);
        }
        IccTrc::Table(table) => {
            t.extend_from_slice(b"curv");
            push_u32(&mut t, 0);
            push_u32(&mut t, table.len() as u32);
            for v in table {
                push_u16(&mut t, *v);
            }
        }
        IccTrc::Parametric(p) => {
            if version < 4 {
                return Err(WriteError::ParametricNeedsV4);
            }
            let n = match p.kind {
                0 => 1,
                1 => 3,
                2 => 4,
                3 => 5,
                4 => 7,
                _ => return Err(WriteError::FixedPointOutOfRange),
            };
            t.extend_from_slice(b"para");
            push_u32(&mut t, 0);
            push_u16(&mut t, u16::from(p.kind));
            push_u16(&mut t, 0);
            for v in [p.g, p.a, p.b, p.c, p.d, p.e, p.f].iter().take(n) {
                t.extend_from_slice(&s15f16(*v)?);
            }
        }
    }
    Ok(t)
}

fn desc_tag(text: &str, version: u8) -> Result<Vec<u8>, WriteError> {
    let mut t = Vec::new();
    if version < 4 {
        if !text.is_ascii() {
            return Err(WriteError::NonAsciiDescription);
        }
        // v2 textDescriptionType: ASCII record, empty Unicode/Script
        // records (the canonical form).
        t.extend_from_slice(b"desc");
        push_u32(&mut t, 0);
        push_u32(&mut t, text.len() as u32 + 1);
        t.extend_from_slice(text.as_bytes());
        t.push(0);
        push_u32(&mut t, 0); // Unicode count
        push_u32(&mut t, 0); // Scriptcode code+count+data (67 zeroed)
        t.extend_from_slice(&[0u8; 67]);
    } else {
        // v4 multiLocalizedUnicode, one en-US record, UTF-16BE.
        let units: Vec<u16> = text.encode_utf16().collect();
        let mut bytes = Vec::with_capacity(units.len() * 2);
        for u in units {
            push_u16(&mut bytes, u);
        }
        t.extend_from_slice(b"mluc");
        push_u32(&mut t, 0);
        push_u32(&mut t, 1); // one record
        push_u32(&mut t, 12); // record size
        push_u16(&mut t, 0x656E); // "en"
        push_u16(&mut t, 0x5553); // "US"
        push_u32(&mut t, bytes.len() as u32);
        push_u32(&mut t, 28); // offset from tag start
        t.extend_from_slice(&bytes);
        // mluc strings are 4-byte padded per the spec's alignment rule.
        pad4(&mut t);
    }
    Ok(t)
}

fn cprt_tag(text: &str, version: u8) -> Result<Vec<u8>, WriteError> {
    if version < 4 {
        if !text.is_ascii() {
            return Err(WriteError::NonAsciiDescription);
        }
        let mut t = Vec::with_capacity(8 + text.len() + 1);
        t.extend_from_slice(b"text");
        push_u32(&mut t, 0);
        t.extend_from_slice(text.as_bytes());
        t.push(0);
        Ok(t)
    } else {
        desc_tag(text, 4)
    }
}

fn chad_tag(m: &[f64; 9]) -> Result<Vec<u8>, WriteError> {
    let mut t = Vec::with_capacity(44);
    t.extend_from_slice(b"sf32");
    push_u32(&mut t, 0);
    for v in m {
        t.extend_from_slice(&s15f16(*v)?);
    }
    Ok(t)
}

/// Serialize a profile into canonical bytes.
///
/// # Errors
/// [`WriteError`] for models the canonical form cannot represent
/// (parametric TRCs at v2, non-ASCII v2 text, out-of-range values).
pub fn write(profile: &IccProfile) -> Result<Vec<u8>, WriteError> {
    let v = profile.version;
    let desc = desc_tag(&profile.description, v)?;
    let wtpt = xyz_tag(profile.white_point)?;
    let rxyz = xyz_tag(profile.columns[0])?;
    let gxyz = xyz_tag(profile.columns[1])?;
    let bxyz = xyz_tag(profile.columns[2])?;
    let rtrc = trc_tag(&profile.trcs[0], v)?;
    let gtrc = trc_tag(&profile.trcs[1], v)?;
    let btrc = trc_tag(&profile.trcs[2], v)?;
    let cprt = match &profile.copyright {
        Some(c) => Some(cprt_tag(c, v)?),
        None => None,
    };
    let chad = match profile.chad {
        Some(c) => Some(chad_tag(&c)?),
        None => None,
    };
    // Canonical tag order.
    let mut blocks: Vec<(&'static [u8], Vec<u8>)> = vec![
        (b"desc", desc),
        (b"wtpt", wtpt),
        (b"rXYZ", rxyz),
        (b"gXYZ", gxyz),
        (b"bXYZ", bxyz),
        (b"rTRC", rtrc),
        (b"gTRC", gtrc),
        (b"bTRC", btrc),
    ];
    if let Some(c) = cprt {
        blocks.push((b"cprt", c));
    }
    if let Some(c) = chad {
        blocks.push((b"chad", c));
    }
    let count = blocks.len();
    let table_end = 128 + 4 + count * 12;
    // Lay out the data blocks, 4-aligned.
    let mut data = Vec::new();
    let mut entries = Vec::with_capacity(count);
    for (sig, block) in &blocks {
        let off = table_end + data.len();
        entries.push((sig, off, block.len()));
        data.extend_from_slice(block);
        pad4(&mut data);
    }
    let total = table_end + data.len();
    let mut out = Vec::with_capacity(total);
    push_u32(&mut out, total as u32);
    out.extend_from_slice(&[0u8; 4]); // preferred CMM: none
    push_u32(&mut out, if v >= 4 { 0x0400_0000 } else { 0x0200_0000 });
    out.extend_from_slice(b"mntr");
    out.extend_from_slice(b"RGB ");
    out.extend_from_slice(b"XYZ ");
    out.extend_from_slice(&[0u8; 12]); // date: canonical zero
    out.extend_from_slice(b"acsp");
    out.extend_from_slice(&[0u8; 4]); // platform
    out.extend_from_slice(&[0u8; 4]); // flags
    out.extend_from_slice(&[0u8; 4]); // manufacturer
    out.extend_from_slice(&[0u8; 4]); // model
    out.extend_from_slice(&[0u8; 8]); // attributes
    push_u32(&mut out, profile.rendering_intent);
    for c in [0.964_2, 1.0, 0.824_9] {
        out.extend_from_slice(&s15f16(c)?);
    }
    out.extend_from_slice(&[0u8; 4]); // creator
    out.extend_from_slice(&[0u8; 16]); // profile ID: not computed
    out.extend_from_slice(&[0u8; 28]); // reserved
    debug_assert_eq!(out.len(), 128);
    push_u32(&mut out, count as u32);
    for (sig, off, len) in entries {
        out.extend_from_slice(sig);
        push_u32(&mut out, off as u32);
        push_u32(&mut out, len as u32);
    }
    debug_assert_eq!(out.len(), table_end);
    out.extend_from_slice(&data);
    debug_assert_eq!(out.len(), total);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_point_quantization() {
        assert_eq!(&s15f16(0.5).unwrap(), &0x0000_8000u32.to_be_bytes());
        assert_eq!(&s15f16(-0.5).unwrap(), &(-0x8000i32).to_be_bytes());
        assert_eq!(&s15f16(1.0).unwrap(), &0x0001_0000u32.to_be_bytes());
        assert!(matches!(
            s15f16(40_000.0),
            Err(WriteError::FixedPointOutOfRange)
        ));
        assert_eq!(&u8f8(2.2).unwrap(), &(563u16).to_be_bytes());
        assert!(matches!(u8f8(300.0), Err(WriteError::GammaOutOfRange)));
    }

    #[test]
    fn rejects_v2_parametric() {
        let trc = IccTrc::Parametric(crate::icc_trc::ParametricCurve {
            kind: 3,
            ..Default::default()
        });
        assert!(matches!(
            trc_tag(&trc, 2),
            Err(WriteError::ParametricNeedsV4)
        ));
    }
}
