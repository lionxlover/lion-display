//! The CTA-861.3 HDMI HDR Static Metadata InfoFrame codec.
//!
//! The deterministic 30-byte packet the output's DRM/HDMI pipeline
//! forwards (Linux `drm_hdmi_infoframe_set` consumes exactly this
//! shape): a 3-byte header (type `0x87`, version 1, length 26), the
//! 26-byte payload, and a checksum byte making the whole packet sum
//! to zero mod 256.
//!
//! Byte map (CTA-861.3, HDR Static Metadata InfoFrame v1 — the layout
//! Linux implements):
//!
//! ```text
//! [0]      0x87    InfoFrame type (HDR)
//! [1]      0x01    version
//! [2]      26      payload length
//! [3]      EOTF (bits 0-3) | metadata-type descriptor (bits 4-7)
//! [4]      0       reserved
//! [5..21]  mastering primaries: Rxy Gxy Bxy Wxy, 8 × u16 BE,
//!          0.16 fixed (value = round(chromaticity × 65535))
//! [21..23] min mastering luminance, u16 BE, 0.0001 cd/m² units
//! [23..25] max mastering luminance, u16 BE, 1 cd/m² units
//! [25..27] max content light level,  u16 BE, 1 cd/m²
//! [27..29] max frame-average light level, u16 BE, 1 cd/m²
//! [29]     checksum: (0x100 - sum(bytes[0..29])) & 0xFF
//! ```
//!
//! Properties (tested): the packet sums to zero mod 256;
//! [`HdrInfoFrame::parse`] inverts [`HdrInfoFrame::encode`] exactly;
//! chromaticities round-trip to 1/65535 (~1.5e-5); the EOTF and
//! descriptor nibbles are validated, not echoed blindly.

use crate::metadata::HdrStaticMetadata;
use ldp_core::color::Primaries;

/// The EOTF nibble (CTA-861.3).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum Eotf {
    /// Traditional SDR gamma (0).
    Sdr,
    /// SMPTE ST 2084 PQ (1).
    Pq,
    /// ITU-R BT.2100 HLG (2).
    Hlg,
}

impl Eotf {
    /// Wire nibble.
    #[must_use]
    pub const fn to_wire(self) -> u8 {
        match self {
            Self::Sdr => 0,
            Self::Pq => 1,
            Self::Hlg => 2,
        }
    }

    /// Parse the nibble; unknown values are rejected (not ignored:
    /// this is our own output path, not foreign input).
    #[must_use]
    pub const fn from_wire(v: u8) -> Option<Eotf> {
        match v {
            0 => Some(Self::Sdr),
            1 => Some(Self::Pq),
            2 => Some(Self::Hlg),
            _ => None,
        }
    }
}

/// InfoFrame codec failures.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum InfoFrameError {
    /// Buffer is not the 30-byte packet.
    BadLength,
    /// Type/version/length header mismatch.
    BadHeader,
    /// Checksum does not zero the packet.
    BadChecksum,
    /// Reserved byte or metadata-type descriptor is wrong.
    BadReserved,
    /// EOTF nibble unknown.
    BadEotf,
}

impl core::fmt::Display for InfoFrameError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::BadLength => write!(f, "not a 30-byte HDR InfoFrame"),
            Self::BadHeader => write!(f, "type/version/length mismatch"),
            Self::BadChecksum => write!(f, "checksum does not zero the packet"),
            Self::BadReserved => write!(f, "reserved byte or descriptor wrong"),
            Self::BadEotf => write!(f, "unknown EOTF nibble"),
        }
    }
}

impl std::error::Error for InfoFrameError {}

/// One HDR Static Metadata InfoFrame.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct HdrInfoFrame {
    /// The EOTF nibble.
    pub eotf: Eotf,
    /// Static metadata type (v1: always 1).
    pub metadata_type: u8,
    /// The coded static metadata.
    pub metadata: HdrStaticMetadata,
}

const FRAME_LEN: usize = 30;
const TYPE_HDR: u8 = 0x87;

fn chroma_code(v: f32) -> u16 {
    // 0.16 fixed: value = round(chromaticity × 65535), saturating.
    let scaled = (v.clamp(0.0, 1.0) * 65_535.0).round();
    scaled.min(65_535.0) as u16
}

fn chroma_decode(code: u16) -> f32 {
    f32::from(code) / 65_535.0
}

fn push_u16(out: &mut [u8; FRAME_LEN], off: usize, v: u16) {
    out[off] = v.to_be_bytes()[0];
    out[off + 1] = v.to_be_bytes()[1];
}

impl HdrInfoFrame {
    /// Build from coded metadata (metadata type pinned to 1 — the v1
    /// static metadata type).
    #[must_use]
    pub const fn new(eotf: Eotf, metadata: HdrStaticMetadata) -> Self {
        Self {
            eotf,
            metadata_type: 1,
            metadata,
        }
    }

    /// Encode to the 30-byte packet.
    #[must_use]
    pub fn encode(&self) -> [u8; FRAME_LEN] {
        let mut out = [0u8; FRAME_LEN];
        out[0] = TYPE_HDR;
        out[1] = 1;
        out[2] = 26;
        out[3] = (self.metadata_type << 4) | self.eotf.to_wire();
        out[4] = 0;
        let c = self.metadata.primaries.chromaticities();
        let mut off = 5;
        for xy in (0..8).step_by(2) {
            push_u16(&mut out, off, chroma_code(c[xy]));
            push_u16(&mut out, off + 2, chroma_code(c[xy + 1]));
            off += 4;
        }
        push_u16(&mut out, 21, self.metadata.mastering_min);
        push_u16(&mut out, 23, self.metadata.mastering_max);
        push_u16(&mut out, 25, self.metadata.max_cll);
        push_u16(&mut out, 27, self.metadata.max_fall);
        // Checksum: the packet sums to zero mod 256.
        let sum: u32 = out[..FRAME_LEN - 1].iter().map(|&b| u32::from(b)).sum();
        out[FRAME_LEN - 1] = ((0x100 - (sum & 0xFF)) & 0xFF) as u8;
        out
    }

    /// Parse a 30-byte packet back (the round-trip inverse, with every
    /// header field validated).
    ///
    /// # Errors
    /// [`InfoFrameError`] on any structural problem.
    pub fn parse(bytes: &[u8]) -> Result<HdrInfoFrame, InfoFrameError> {
        if bytes.len() != FRAME_LEN {
            return Err(InfoFrameError::BadLength);
        }
        if bytes[0] != TYPE_HDR || bytes[1] != 1 || bytes[2] != 26 {
            return Err(InfoFrameError::BadHeader);
        }
        let sum: u32 = bytes[..FRAME_LEN - 1].iter().map(|&b| u32::from(b)).sum();
        let check = ((0x100 - (sum & 0xFF)) & 0xFF) as u8;
        if bytes[FRAME_LEN - 1] != check {
            return Err(InfoFrameError::BadChecksum);
        }
        if bytes[4] != 0 {
            return Err(InfoFrameError::BadReserved);
        }
        let metadata_type = bytes[3] >> 4;
        if metadata_type != 1 {
            return Err(InfoFrameError::BadReserved);
        }
        let eotf = Eotf::from_wire(bytes[3] & 0x0F).ok_or(InfoFrameError::BadEotf)?;
        // Decode the primaries back to the closest enum (the coding is
        // lossy at 1/65535 — pick the nearest standard set).
        let be16 = |off: usize| u16::from_be_bytes([bytes[off], bytes[off + 1]]);
        let mut best = Primaries::Bt709;
        let mut best_err = f32::INFINITY;
        for cand in [Primaries::Bt709, Primaries::DciP3, Primaries::Bt2020] {
            let cc = cand.chromaticities();
            let mut err = 0.0f32;
            for i in 0..4 {
                let gx = chroma_decode(be16(5 + i * 4));
                let gy = chroma_decode(be16(7 + i * 4));
                err = err.max((gx - cc[i * 2]).abs().max((gy - cc[i * 2 + 1]).abs()));
            }
            if err < best_err {
                best_err = err;
                best = cand;
            }
        }
        Ok(HdrInfoFrame {
            eotf,
            metadata_type,
            metadata: HdrStaticMetadata {
                primaries: best,
                mastering_min: be16(21),
                mastering_max: be16(23),
                max_cll: be16(25),
                max_fall: be16(27),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ldp_core::color::HdrMetadata;

    fn sample() -> HdrInfoFrame {
        let meta = HdrStaticMetadata::from_wire(&HdrMetadata::default()).unwrap();
        HdrInfoFrame::new(Eotf::Pq, meta)
    }

    #[test]
    fn packet_sums_to_zero() {
        let f = sample();
        let bytes = f.encode();
        let sum: u32 = bytes.iter().map(|&b| u32::from(b)).sum();
        assert_eq!(sum & 0xFF, 0, "checksum property");
        // Header spots.
        assert_eq!((bytes[0], bytes[1], bytes[2]), (0x87, 1, 26));
        // EOTF=PQ (1) with type descriptor 1 → 0x11.
        assert_eq!(bytes[3], 0x11);
        assert_eq!(bytes[4], 0);
        // Hand-computed chromaticity codes (BT.2020, D65):
        // R(0.708, 0.292) → 46399 (0xB53F), 19136 (0x4AC0);
        // G(0.170, 0.797) → 11141 (0x2B85), 52231 (0xCC07);
        // B(0.131, 0.046) → 8585 (0x2189), 3015 (0x0BC7);
        // W(0.3127, 0.3290) → 20493 (0x500D), 21561 (0x5439).
        assert_eq!((bytes[5], bytes[6]), (0xB5, 0x3F));
        assert_eq!((bytes[7], bytes[8]), (0x4A, 0xC0));
        assert_eq!((bytes[9], bytes[10]), (0x2B, 0x85));
        assert_eq!((bytes[11], bytes[12]), (0xCC, 0x07));
        assert_eq!((bytes[13], bytes[14]), (0x21, 0x89));
        assert_eq!((bytes[15], bytes[16]), (0x0B, 0xC7));
        assert_eq!((bytes[17], bytes[18]), (0x50, 0x0D));
        assert_eq!((bytes[19], bytes[20]), (0x54, 0x39));
        // Luminance codings: 0.005 nits = 50 units (0x0032) at
        // [21..23]; max 1000 (0x03E8) at [23..25]; CLL 1000 at
        // [25..27]; FALL 200 (0x00C8) at [27..29].
        assert_eq!((bytes[21], bytes[22]), (0x00, 0x32));
        assert_eq!((bytes[23], bytes[24]), (0x03, 0xE8));
        assert_eq!((bytes[25], bytes[26]), (0x03, 0xE8));
        assert_eq!((bytes[27], bytes[28]), (0x00, 0xC8));
    }

    #[test]
    fn round_trips_exactly() {
        for eotf in [Eotf::Sdr, Eotf::Pq, Eotf::Hlg] {
            for meta in [
                HdrMetadata::default(),
                HdrMetadata {
                    mastering_primaries: Primaries::Bt709,
                    mastering_luminance_min: ldp_core::color::Luminance::from_nits(0),
                    mastering_luminance_max: ldp_core::color::Luminance::from_nits(100),
                    max_cll: ldp_core::color::Luminance::from_nits(100),
                    max_fall: ldp_core::color::Luminance::from_nits(50),
                },
            ] {
                let f = HdrInfoFrame::new(eotf, HdrStaticMetadata::from_wire(&meta).unwrap());
                let bytes = f.encode();
                let back = HdrInfoFrame::parse(&bytes).unwrap();
                assert_eq!(back.eotf, eotf);
                assert_eq!(back.metadata, f.metadata);
                // Byte-stable: encoding is deterministic.
                assert_eq!(f.encode(), bytes);
            }
        }
    }

    #[test]
    fn rejects_corruption() {
        let bytes = sample().encode();
        assert_eq!(
            HdrInfoFrame::parse(&bytes[..29]),
            Err(InfoFrameError::BadLength)
        );
        let mut bad = bytes;
        bad[1] = 2; // version
        assert_eq!(HdrInfoFrame::parse(&bad), Err(InfoFrameError::BadHeader));
        let mut bad = bytes;
        bad[9] ^= 0x40; // flip a chromaticity bit → checksum
        assert_eq!(HdrInfoFrame::parse(&bad), Err(InfoFrameError::BadChecksum));
        let mut bad = bytes;
        bad[4] = 1; // reserved byte
                    // Fix the checksum so only the reserved check fires.
        let sum: u32 = bad[..FRAME_LEN - 1].iter().map(|&b| u32::from(b)).sum();
        bad[FRAME_LEN - 1] = ((0x100 - (sum & 0xFF)) & 0xFF) as u8;
        assert_eq!(HdrInfoFrame::parse(&bad), Err(InfoFrameError::BadReserved));
        let mut bad = bytes;
        bad[3] = 0x13; // metadata type 1, unknown EOTF nibble 3
        let sum: u32 = bad[..FRAME_LEN - 1].iter().map(|&b| u32::from(b)).sum();
        bad[FRAME_LEN - 1] = ((0x100 - (sum & 0xFF)) & 0xFF) as u8;
        assert_eq!(HdrInfoFrame::parse(&bad), Err(InfoFrameError::BadEotf));
        let mut bad = bytes;
        bad[3] = 0x21; // metadata type 2
        let sum: u32 = bad[..FRAME_LEN - 1].iter().map(|&b| u32::from(b)).sum();
        bad[FRAME_LEN - 1] = ((0x100 - (sum & 0xFF)) & 0xFF) as u8;
        assert_eq!(HdrInfoFrame::parse(&bad), Err(InfoFrameError::BadReserved));
    }
}
