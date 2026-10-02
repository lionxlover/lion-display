//! Plane model — scanout hardware slots and their format capabilities.
//!
//! A plane is one hardware scanout slot feeding a CRTC. The primary plane
//! carries the framebuffer, overlay planes stack above it, the cursor
//! plane is the pointer sprite. What a plane *can do* is its
//! [`CrtcMask`] (which CRTCs it can feed) and its format capabilities:
//! the legacy `formats[]` array plus — on modern drivers — the
//! `IN_FORMATS` immutable blob, which pairs every format with the
//! modifier bitmask that supports it. That blob has a kernel-defined
//! binary layout, parsed here byte-exactly.

#![forbid(unsafe_code)]

use crate::error::{DisplayError, Result};
use crate::ids::{CrtcId, CrtcMask, PlaneId};
use ldp_core::buffer::{FourCC, Modifier};

/// Plane class (`DRM_PLANE_TYPE_*`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum PlaneType {
    /// An overlay plane (stacks above primary).
    Overlay,
    /// The CRTC's primary plane.
    Primary,
    /// The cursor sprite plane.
    Cursor,
}

impl PlaneType {
    /// Kernel constant.
    #[must_use]
    pub const fn kernel(self) -> u32 {
        match self {
            Self::Overlay => 0,
            Self::Primary => 1,
            Self::Cursor => 2,
        }
    }

    /// From kernel constant.
    #[must_use]
    pub const fn from_kernel(code: u32) -> Self {
        match code {
            1 => Self::Primary,
            2 => Self::Cursor,
            _ => Self::Overlay,
        }
    }
}

/// One (format, modifier) capability entry decoded from `IN_FORMATS`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FormatModifier {
    /// The pixel format.
    pub format: FourCC,
    /// A layout/compression modifier usable with that format.
    pub modifier: Modifier,
}

/// The `IN_FORMATS` blob, parsed.
///
/// Kernel layout (`drm_format_modifier_blob`): a 20-byte header
/// (version, count_formats, formats_offset, count_modifiers,
/// modifiers_offset — all LE u32, offsets counted from the blob start),
/// then `count_formats` LE u32 fourccs, then `count_modifiers` records of
/// (LE u64 modifier, LE u64 bitmap offset), each record followed by a
/// bitmap of `count_formats` bits (1 = the modifier supports that
/// format). Version must be 1.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct InFormats {
    /// Every (format, modifier) pair with its bit set.
    pub pairs: Vec<FormatModifier>,
}

impl InFormats {
    /// Parse the blob payload.
    ///
    /// # Errors
    /// [`DisplayError::BadBlob`] for short blobs, wrong version, or
    /// offsets/counts that walk outside the payload.
    ///
    /// # Panics
    /// Never in practice: the two `try_into().unwrap()` sites are
    /// guarded by exact slice lengths checked immediately before.
    pub fn parse(blob: &[u8]) -> Result<Self> {
        if blob.len() < 20 {
            return Err(DisplayError::BadBlob { what: "IN_FORMATS" });
        }
        let u32at = |o: usize| -> Result<u32> {
            blob.get(o..o + 4)
                .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
                .ok_or(DisplayError::BadBlob { what: "IN_FORMATS" })
        };
        let version = u32at(0)?;
        if version != 1 {
            return Err(DisplayError::BadBlob { what: "IN_FORMATS" });
        }
        let count_formats = u32at(4)? as usize;
        let formats_offset = u32at(8)? as usize;
        let count_modifiers = u32at(12)? as usize;
        let modifiers_offset = u32at(16)? as usize;

        let formats_end = formats_offset
            .checked_add(
                count_formats
                    .checked_mul(4)
                    .ok_or(DisplayError::BadBlob { what: "IN_FORMATS" })?,
            )
            .ok_or(DisplayError::BadBlob { what: "IN_FORMATS" })?;
        if formats_end > blob.len() {
            return Err(DisplayError::BadBlob { what: "IN_FORMATS" });
        }
        let mut formats = Vec::with_capacity(count_formats);
        for i in 0..count_formats {
            let o = formats_offset + i * 4;
            formats.push(FourCC::from_code(u32::from_le_bytes(
                blob[o..o + 4].try_into().unwrap(),
            )));
        }

        let mut pairs = Vec::new();
        for i in 0..count_modifiers {
            let base = modifiers_offset
                .checked_add(
                    i.checked_mul(16)
                        .ok_or(DisplayError::BadBlob { what: "IN_FORMATS" })?,
                )
                .ok_or(DisplayError::BadBlob { what: "IN_FORMATS" })?;
            let tail = blob
                .get(base..base + 16)
                .ok_or(DisplayError::BadBlob { what: "IN_FORMATS" })?;
            let modifier = Modifier::from_code(u64::from_le_bytes(tail[..8].try_into().unwrap()));
            let bitmap_off = u64::from_le_bytes(tail[8..].try_into().unwrap()) as usize;
            let bitmap_len = count_formats.div_ceil(8);
            let bitmap = blob
                .get(
                    bitmap_off
                        ..bitmap_off
                            .checked_add(bitmap_len)
                            .ok_or(DisplayError::BadBlob { what: "IN_FORMATS" })?,
                )
                .ok_or(DisplayError::BadBlob { what: "IN_FORMATS" })?;
            for (j, format) in formats.iter().enumerate() {
                if bitmap[j / 8] & (1 << (j % 8)) != 0 {
                    pairs.push(FormatModifier {
                        format: *format,
                        modifier,
                    });
                }
            }
        }
        Ok(Self { pairs })
    }

    /// Whether (format, modifier) scans out.
    #[must_use]
    pub fn supports(&self, format: FourCC, modifier: Modifier) -> bool {
        self.pairs
            .iter()
            .any(|p| p.format == format && p.modifier == modifier)
    }

    /// Encode an `IN_FORMATS` blob (mock device builder + test oracle).
    #[must_use]
    pub fn encode(pairs: &[FormatModifier]) -> Vec<u8> {
        // Unique formats first (preserving first-seen order).
        let mut formats: Vec<FourCC> = Vec::new();
        for p in pairs {
            if !formats.contains(&p.format) {
                formats.push(p.format);
            }
        }
        // Unique modifiers (first-seen order).
        let mut modifiers: Vec<Modifier> = Vec::new();
        for p in pairs {
            if !modifiers.contains(&p.modifier) {
                modifiers.push(p.modifier);
            }
        }
        let formats_offset = 20usize;
        let modifiers_offset = formats_offset + formats.len() * 4;
        let mut blob = Vec::with_capacity(64);
        blob.extend_from_slice(&1u32.to_le_bytes());
        blob.extend_from_slice(&(formats.len() as u32).to_le_bytes());
        blob.extend_from_slice(&(formats_offset as u32).to_le_bytes());
        blob.extend_from_slice(&(modifiers.len() as u32).to_le_bytes());
        blob.extend_from_slice(&(modifiers_offset as u32).to_le_bytes());
        for f in &formats {
            blob.extend_from_slice(&f.code().to_le_bytes());
        }
        // Kernel layout: the modifier records form one contiguous
        // array; the bitmaps follow it (each record points at its own).
        let bitmap_len = formats.len().div_ceil(8);
        let bitmap_base = modifiers_offset + modifiers.len() * 16;
        let mut records = Vec::new();
        let mut bitmaps = Vec::new();
        for m in &modifiers {
            let mut bitmap = vec![0u8; bitmap_len];
            for (j, f) in formats.iter().enumerate() {
                if pairs.iter().any(|p| p.format == *f && p.modifier == *m) {
                    bitmap[j / 8] |= 1 << (j % 8);
                }
            }
            let bitmap_off = bitmap_base + bitmaps.len();
            records.extend_from_slice(&m.code().to_le_bytes());
            records.extend_from_slice(&(bitmap_off as u64).to_le_bytes());
            bitmaps.extend_from_slice(&bitmap);
        }
        blob.extend_from_slice(&records);
        blob.extend_from_slice(&bitmaps);
        blob
    }
}

/// One plane snapshot.
#[derive(Clone, Debug, PartialEq)]
pub struct PlaneInfo {
    /// Object id.
    pub id: PlaneId,
    /// Plane class.
    pub kind: PlaneType,
    /// Which CRTCs this plane can feed.
    pub possible_crtcs: CrtcMask,
    /// CRTC currently using the plane, if any.
    pub current_crtc: Option<CrtcId>,
    /// Legacy format list (no modifier info).
    pub formats: Vec<FourCC>,
    /// Parsed `IN_FORMATS` blob when the driver supplies one.
    pub in_formats: Option<InFormats>,
}

impl PlaneInfo {
    /// Whether this plane can scan out (format, modifier). With an
    /// `IN_FORMATS` blob the answer is exact; without it the legacy list
    /// only proves the format, and the modifier check falls back to
    /// "linear or INVALID assumed OK" (the kernel's legacy contract).
    #[must_use]
    pub fn supports(&self, format: FourCC, modifier: Modifier) -> bool {
        match &self.in_formats {
            Some(inf) => inf.supports(format, modifier),
            None => {
                self.formats.contains(&format) && (modifier.is_linear() || modifier.is_invalid())
            }
        }
    }

    /// Whether the plane can feed CRTC index `i`.
    #[must_use]
    pub const fn feeds_crtc_index(&self, i: u32) -> bool {
        self.possible_crtcs.contains(i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const X: FourCC = FourCC::XRGB8888;
    const AR: FourCC = FourCC::ARGB8888;
    const NV: FourCC = FourCC::NV12;

    #[test]
    fn plane_type_round_trip() {
        for code in [0u32, 1, 2] {
            assert_eq!(PlaneType::from_kernel(code).kernel(), code);
        }
        assert_eq!(PlaneType::from_kernel(3), PlaneType::Overlay);
    }

    #[test]
    fn in_formats_round_trip() {
        let pairs = [
            FormatModifier {
                format: X,
                modifier: Modifier::LINEAR,
            },
            FormatModifier {
                format: X,
                modifier: Modifier::INTEL_X,
            },
            FormatModifier {
                format: AR,
                modifier: Modifier::LINEAR,
            },
            FormatModifier {
                format: NV,
                modifier: Modifier::LINEAR,
            },
        ];
        let blob = InFormats::encode(&pairs);
        let parsed = InFormats::parse(&blob).unwrap();
        // The blob layout is modifier-major; the pair ORDER therefore
        // differs from the input's. Compare as sorted sets of pairs.
        let mut got = parsed.pairs.clone();
        let mut want = pairs.to_vec();
        let key = |p: &FormatModifier| (p.format.code(), p.modifier.code());
        got.sort_by_key(key);
        want.sort_by_key(key);
        assert_eq!(got, want);
        // Capability spot checks.
        assert!(parsed.supports(X, Modifier::INTEL_X));
        assert!(!parsed.supports(AR, Modifier::INTEL_X));
        assert!(!parsed.supports(NV, Modifier::INTEL_Y));
    }

    #[test]
    fn in_formats_rejects_short_and_wrong_version() {
        assert!(InFormats::parse(&[0u8; 19]).is_err());
        let mut blob = InFormats::encode(&[FormatModifier {
            format: X,
            modifier: Modifier::LINEAR,
        }]);
        blob[0] = 2; // wrong version
        assert!(InFormats::parse(&blob).is_err());
    }

    #[test]
    fn legacy_fallback_semantics() {
        let plane = PlaneInfo {
            id: PlaneId::new(1).unwrap(),
            kind: PlaneType::Primary,
            possible_crtcs: CrtcMask::covering(2),
            current_crtc: None,
            formats: vec![X],
            in_formats: None,
        };
        assert!(plane.supports(X, Modifier::LINEAR));
        assert!(plane.supports(X, Modifier::INVALID)); // legacy: driver default
        assert!(!plane.supports(AR, Modifier::LINEAR));
        assert!(!plane.supports(X, Modifier::INTEL_X));
        assert!(plane.feeds_crtc_index(1));
        assert!(!plane.feeds_crtc_index(2));
    }
}
