//! Color-space and HDR metadata types.
//!
//! The wire vocabulary of `spec/core.toml` (`color_primaries`,
//! `transfer_function`, `color_range`, `set_color`, `set_hdr_metadata`,
//! `output.color`). The math (matrices, EOTFs, tone mapping) is Phase 14's
//! `ldp-color`; this module carries the deterministic, wire-safe types.
//!
//! Luminance convention: **1e-4 cd/m² units** in `u32` (10000 = 1 nit),
//! covering 0..429,496 cd/m² with 0.1-nit resolution — enough for every
//! shipping panel and headroom for reference monitors.

use core::fmt;

/// Color primaries (wire: `ldp.core.color_primaries`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum Primaries {
    /// sRGB / HDTV primaries (BT.709).
    Bt709,
    /// Display P3 (digital cinema subset).
    DciP3,
    /// Ultra-wide gamut (BT.2020).
    Bt2020,
}

impl Primaries {
    /// Wire value.
    #[must_use]
    pub const fn to_wire(self) -> u32 {
        match self {
            Self::Bt709 => 1,
            Self::DciP3 => 2,
            Self::Bt2020 => 3,
        }
    }

    /// Parse a wire value; unknown values must be ignored by receivers.
    #[must_use]
    pub const fn from_wire(v: u32) -> Option<Primaries> {
        match v {
            1 => Some(Self::Bt709),
            2 => Some(Self::DciP3),
            3 => Some(Self::Bt2020),
            _ => None,
        }
    }

    /// CIE 1931 xy chromaticities of the RGB primaries and white point —
    /// the *definitional* values `ldp-color` derives matrices from. Order:
    /// `(rx, ry, gx, gy, bx, by, wx, wy)`.
    #[must_use]
    pub const fn chromaticities(self) -> [f32; 8] {
        match self {
            Self::Bt709 => [
                0.640, 0.330, // R
                0.300, 0.600, // G
                0.150, 0.060, // B
                0.3127, 0.3290, // D65 white
            ],
            Self::DciP3 => [0.680, 0.320, 0.265, 0.690, 0.150, 0.060, 0.3127, 0.3290],
            Self::Bt2020 => [0.708, 0.292, 0.170, 0.797, 0.131, 0.046, 0.3127, 0.3290],
        }
    }
}

/// Transfer characteristic (wire: `ldp.core.transfer_function`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum TransferFunction {
    /// Linear light.
    Linear,
    /// sRGB IEC 61966-2-1.
    Srgb,
    /// Perceptual quantizer, SMPTE ST 2084 (HDR).
    Pq,
    /// Hybrid log-gamma, ITU-R BT.2100 (HDR broadcast).
    Hlg,
    /// Legacy CRT gamma 2.2.
    Gamma22,
    /// Legacy CRT gamma 2.8.
    Gamma28,
}

impl TransferFunction {
    /// Wire value.
    #[must_use]
    pub const fn to_wire(self) -> u32 {
        match self {
            Self::Linear => 1,
            Self::Srgb => 2,
            Self::Pq => 3,
            Self::Hlg => 4,
            Self::Gamma22 => 5,
            Self::Gamma28 => 6,
        }
    }

    /// Parse a wire value.
    #[must_use]
    pub const fn from_wire(v: u32) -> Option<TransferFunction> {
        match v {
            1 => Some(Self::Linear),
            2 => Some(Self::Srgb),
            3 => Some(Self::Pq),
            4 => Some(Self::Hlg),
            5 => Some(Self::Gamma22),
            6 => Some(Self::Gamma28),
            _ => None,
        }
    }

    /// Whether this transfer function is scene-referred HDR (PQ, HLG) —
    /// content that may exceed the SDR reference white and requires tone
    /// mapping on SDR outputs.
    #[must_use]
    pub const fn is_hdr(self) -> bool {
        matches!(self, Self::Pq | Self::Hlg)
    }
}

/// Component range (wire: `ldp.core.color_range`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum ColorRange {
    /// Full-range 0..1 components.
    Full,
    /// Studio (limited) range, e.g. 16..235 in 8-bit YCbCr encodings.
    Studio,
}

impl ColorRange {
    /// Wire value.
    #[must_use]
    pub const fn to_wire(self) -> u32 {
        match self {
            Self::Full => 1,
            Self::Studio => 2,
        }
    }

    /// Parse a wire value.
    #[must_use]
    pub const fn from_wire(v: u32) -> Option<ColorRange> {
        match v {
            1 => Some(Self::Full),
            2 => Some(Self::Studio),
            _ => None,
        }
    }
}

/// Luminance values in 1e-4 cd/m² units (0.1-nit resolution, u32 range).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[repr(transparent)]
pub struct Luminance {
    deci_milli_nits: u32,
}

impl Luminance {
    /// From 1e-4 cd/m² units (the wire unit).
    pub const fn from_wire_units(v: u32) -> Luminance {
        Luminance { deci_milli_nits: v }
    }

    /// From whole nits (cd/m²), saturating above 429,496.7295 nits.
    pub const fn from_nits(nits: u32) -> Luminance {
        Luminance {
            deci_milli_nits: nits.saturating_mul(10_000),
        }
    }

    /// Wire units (1e-4 cd/m²).
    #[must_use]
    pub const fn as_wire_units(self) -> u32 {
        self.deci_milli_nits
    }

    /// Approximate whole nits (truncating sub-nit precision).
    #[must_use]
    pub const fn as_nits(self) -> u32 {
        self.deci_milli_nits / 10_000
    }
}

impl fmt::Display for Luminance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} nit", self.as_nits())
    }
}

/// A parametric color description (`surface.set_color`, `output.color`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ColorDescription {
    /// RGB primaries.
    pub primaries: Primaries,
    /// Transfer characteristic.
    pub transfer: TransferFunction,
    /// Component range.
    pub range: ColorRange,
    /// Minimum luminance the mastering display can reproduce.
    pub luminance_min: Luminance,
    /// Maximum luminance the mastering display can reproduce.
    pub luminance_max: Luminance,
    /// SDR reference white in this description.
    pub reference_white: Luminance,
}

impl Default for ColorDescription {
    /// Standard sRGB SDR: BT.709 primaries, sRGB transfer, full range,
    /// 0..80-nit range with an 80-nit reference (classic sRGB assumption
    /// of ~80 cd/m² for paper white, later re-anchored by `output.color`).
    fn default() -> Self {
        Self::srgb_sdr()
    }
}

impl ColorDescription {
    /// The default sRGB SDR description.
    pub const fn srgb_sdr() -> Self {
        ColorDescription {
            primaries: Primaries::Bt709,
            transfer: TransferFunction::Srgb,
            range: ColorRange::Full,
            luminance_min: Luminance::from_nits(0),
            luminance_max: Luminance::from_nits(80),
            reference_white: Luminance::from_nits(80),
        }
    }

    /// A PQ HDR description with typical mastering bounds
    /// (BT.2408: 0.005..1000 nits, 203-nit SDR reference).
    #[must_use]
    pub const fn pq_hdr() -> Self {
        ColorDescription {
            primaries: Primaries::Bt2020,
            transfer: TransferFunction::Pq,
            range: ColorRange::Full,
            luminance_min: Luminance::from_wire_units(50),
            luminance_max: Luminance::from_nits(1000),
            reference_white: Luminance::from_nits(203),
        }
    }

    /// Whether this describes HDR content (HDR transfer function).
    #[must_use]
    pub const fn is_hdr(&self) -> bool {
        self.transfer.is_hdr()
    }
}

/// Static HDR10 mastering metadata (`surface.set_hdr_metadata`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct HdrMetadata {
    /// Primaries of the mastering display.
    pub mastering_primaries: Primaries,
    /// Minimum mastering luminance.
    pub mastering_luminance_min: Luminance,
    /// Maximum mastering luminance.
    pub mastering_luminance_max: Luminance,
    /// Max content light level (CLL).
    pub max_cll: Luminance,
    /// Max frame-average light level (FALL).
    pub max_fall: Luminance,
}

impl Default for HdrMetadata {
    fn default() -> Self {
        // Conservative defaults mirroring typical HDR10 mastering
        // (BT.2408 guidance): 0.005..1000 nits, CLL 1000, FALL 200.
        HdrMetadata {
            mastering_primaries: Primaries::Bt2020,
            mastering_luminance_min: Luminance::from_wire_units(50),
            mastering_luminance_max: Luminance::from_nits(1000),
            max_cll: Luminance::from_nits(1000),
            max_fall: Luminance::from_nits(200),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_round_trips() {
        for v in 1..=3u32 {
            let p = Primaries::from_wire(v).unwrap();
            assert_eq!(p.to_wire(), v);
        }
        assert!(Primaries::from_wire(0).is_none());
        assert!(Primaries::from_wire(4).is_none());

        for v in 1..=6u32 {
            let t = TransferFunction::from_wire(v).unwrap();
            assert_eq!(t.to_wire(), v);
        }
        assert!(TransferFunction::from_wire(7).is_none());

        assert_eq!(ColorRange::from_wire(1).unwrap().to_wire(), 1);
        assert_eq!(ColorRange::from_wire(2).unwrap().to_wire(), 2);
        assert!(ColorRange::from_wire(3).is_none());
    }

    #[test]
    fn hdr_detection() {
        assert!(TransferFunction::Pq.is_hdr());
        assert!(TransferFunction::Hlg.is_hdr());
        assert!(!TransferFunction::Srgb.is_hdr());
        assert!(!TransferFunction::Linear.is_hdr());
        assert!(!ColorDescription::srgb_sdr().is_hdr());
        assert!(ColorDescription::pq_hdr().is_hdr());
    }

    #[test]
    fn luminance_units() {
        assert_eq!(Luminance::from_nits(100).as_wire_units(), 1_000_000);
        assert_eq!(Luminance::from_wire_units(1_000_000).as_nits(), 100);
        assert_eq!(Luminance::from_nits(203).as_nits(), 203);
        // 0.1-nit resolution survives round trip.
        assert_eq!(Luminance::from_wire_units(10_050).as_wire_units(), 10_050);
        // Display truncates sub-nit only in the convenience view.
        assert_eq!(Luminance::from_wire_units(10_050).as_nits(), 1);
        assert_eq!(Luminance::from_nits(0).to_string(), "0 nit");
    }

    #[test]
    fn chromaticities_are_sane_cie_points() {
        for p in [Primaries::Bt709, Primaries::DciP3, Primaries::Bt2020] {
            let c = p.chromaticities();
            // x, y in (0, 1); x+y <= 1 for real chromaticities.
            for i in (0..8).step_by(2) {
                assert!(c[i] > 0.0 && c[i] < 1.0, "x out of range");
                assert!(c[i + 1] > 0.0 && c[i + 1] < 1.0, "y out of range");
                assert!(
                    c[i] + c[i + 1] <= 1.0 + f32::EPSILON,
                    "outside spectral locus"
                );
            }
            // Shared D65 white point.
            assert_eq!((c[6], c[7]), (0.3127, 0.3290));
        }
    }

    #[test]
    fn defaults_are_the_documented_values() {
        let d = ColorDescription::default();
        assert_eq!(d, ColorDescription::srgb_sdr());
        assert_eq!(d.reference_white.as_nits(), 80);
        let h = HdrMetadata::default();
        assert_eq!(h.max_cll.as_nits(), 1000);
        assert_eq!(h.max_fall.as_nits(), 200);
        assert_eq!(h.mastering_luminance_min.as_wire_units(), 50);
    }
}
