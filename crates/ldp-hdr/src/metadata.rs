//! HDR10 static-metadata validation and normalization.
//!
//! The wire type ([`HdrMetadata`]) is carried as-is from clients; this
//! module is the server-side gate that decides what is *presentable*
//! and normalizes it into the InfoFrame-ready coding
//! ([`HdrStaticMetadata`]):
//!
//! * the CTA sentinels: `0` means *unknown* (no mastering bound, no
//!   CLL/FALL) — kept as-is, never fabricated,
//! * mastering bounds must be ordered (`min < max`, `max > 0`) and
//!   under the 10 000-nit PQ ceiling,
//! * `max_fall` is an average and cannot exceed `max_cll` — clamped
//!   to it (normalization, not rejection: a sloppy-but-harmless
//!   overshoot),
//! * the InfoFrame's luminance codings are exact integer transforms
//!   of the wire's 1e-4 cd/m² units (min) and whole nits (max),
//!   saturating at the u16 ceilings.
//!
//! Validation happens *before* any InfoFrame allocation (the
//! validation-before-allocation doctrine); denials are typed errors,
//! never silent coercion.

use ldp_core::color::{HdrMetadata, Primaries};

/// Metadata validation failures.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum MetadataError {
    /// Mastering luminances unordered or zero (`min >= max` or
    /// `max == 0`).
    BadMasteringRange,
    /// A luminance exceeds the 10 000-nit PQ ceiling.
    AboveCeiling,
}

impl core::fmt::Display for MetadataError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::BadMasteringRange => write!(f, "mastering luminances unordered or zero"),
            Self::AboveCeiling => write!(f, "luminance above the 10000-nit PQ ceiling"),
        }
    }
}

impl std::error::Error for MetadataError {}

/// The InfoFrame-ready static metadata (all integer codings).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct HdrStaticMetadata {
    /// Mastering-display primaries.
    pub primaries: Primaries,
    /// Minimum mastering luminance in 1e-4 cd/m² (the InfoFrame's own
    /// unit — the wire's unit, passed through, u16-saturating).
    pub mastering_min: u16,
    /// Maximum mastering luminance in whole cd/m² (u16-saturating).
    pub mastering_max: u16,
    /// Max content light level in whole cd/m²; 0 = unknown.
    pub max_cll: u16,
    /// Max frame-average light level in whole cd/m²; 0 = unknown.
    pub max_fall: u16,
}

impl HdrStaticMetadata {
    /// Validate and normalize wire metadata into the coded form.
    ///
    /// # Errors
    /// [`MetadataError`] on unordered mastering bounds or
    /// above-PQ-ceiling luminances.
    pub fn from_wire(meta: &HdrMetadata) -> Result<HdrStaticMetadata, MetadataError> {
        let min = u64::from(meta.mastering_luminance_min.as_wire_units());
        let max = u64::from(meta.mastering_luminance_max.as_wire_units());
        if max == 0 || min >= max {
            return Err(MetadataError::BadMasteringRange);
        }
        // Wire units are 1e-4 cd/m²: the PQ ceiling is 100 000 000.
        if max > 100_000_000 || u64::from(meta.max_cll.as_wire_units()) > 100_000_000 {
            return Err(MetadataError::AboveCeiling);
        }
        let cll_nits = u64::from(meta.max_cll.as_nits());
        let mut fall_nits = u64::from(meta.max_fall.as_nits());
        // FALL is an average: it cannot exceed CLL (normalize).
        if meta.max_cll.as_wire_units() > 0 {
            fall_nits = fall_nits.min(cll_nits);
        }
        let clamp16 = |v: u64| v.min(u64::from(u16::MAX)) as u16;
        Ok(HdrStaticMetadata {
            primaries: meta.mastering_primaries,
            mastering_min: clamp16(min),
            mastering_max: clamp16(max / 10_000),
            max_cll: clamp16(cll_nits),
            max_fall: clamp16(fall_nits),
        })
    }

    /// Whether the mastering bounds are the "unknown" sentinel.
    #[must_use]
    pub fn bounds_unknown(&self) -> bool {
        self.mastering_min == 0 && self.mastering_max == 0
    }

    /// Whether CLL is the "unknown" sentinel.
    #[must_use]
    pub fn cll_unknown(&self) -> bool {
        self.max_cll == 0
    }

    /// Whether FALL is the "unknown" sentinel.
    #[must_use]
    pub fn fall_unknown(&self) -> bool {
        self.max_fall == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ldp_core::color::Luminance;

    #[test]
    fn validates_ordering_and_ceiling() {
        let mut m = HdrMetadata::default();
        // Default (0.005..1000) is valid.
        assert!(HdrStaticMetadata::from_wire(&m).is_ok());
        // Unordered bounds.
        m = HdrMetadata {
            mastering_luminance_min: Luminance::from_nits(500),
            mastering_luminance_max: Luminance::from_nits(100),
            ..m
        };
        assert_eq!(
            HdrStaticMetadata::from_wire(&m),
            Err(MetadataError::BadMasteringRange)
        );
        // Zero max.
        m = HdrMetadata {
            mastering_luminance_min: Luminance::from_nits(0),
            mastering_luminance_max: Luminance::from_nits(0),
            ..m
        };
        assert_eq!(
            HdrStaticMetadata::from_wire(&m),
            Err(MetadataError::BadMasteringRange)
        );
        // Above the PQ ceiling.
        m = HdrMetadata {
            mastering_luminance_max: Luminance::from_wire_units(100_000_001),
            ..m
        };
        assert_eq!(
            HdrStaticMetadata::from_wire(&m),
            Err(MetadataError::AboveCeiling)
        );
        m = HdrMetadata {
            mastering_luminance_max: Luminance::from_nits(1000),
            ..m
        };
        m = HdrMetadata {
            max_cll: Luminance::from_wire_units(100_000_001),
            ..m
        };
        assert_eq!(
            HdrStaticMetadata::from_wire(&m),
            Err(MetadataError::AboveCeiling)
        );
    }

    #[test]
    fn normalizes_the_codings() {
        // The BT.2408-style defaults.
        let m = HdrMetadata::default();
        let h = HdrStaticMetadata::from_wire(&m).unwrap();
        assert_eq!(h.primaries, Primaries::Bt2020);
        assert_eq!(h.mastering_min, 50); // 0.005 nits in 1e-4 units
        assert_eq!(h.mastering_max, 1000);
        assert_eq!(h.max_cll, 1000);
        assert_eq!(h.max_fall, 200);
        assert!(!h.bounds_unknown() && !h.cll_unknown() && !h.fall_unknown());
        // FALL > CLL is normalized down to CLL.
        let m2 = HdrMetadata {
            max_fall: Luminance::from_nits(2000),
            ..m
        };
        let h2 = HdrStaticMetadata::from_wire(&m2).unwrap();
        assert_eq!(h2.max_fall, 1000);
        // Sub-nit precision is preserved in the min coding.
        let m3 = HdrMetadata {
            mastering_luminance_min: Luminance::from_wire_units(12_345),
            ..m
        };
        assert_eq!(
            HdrStaticMetadata::from_wire(&m3).unwrap().mastering_min,
            12_345
        );
    }

    #[test]
    fn unknown_sentinels_survive() {
        let mut m = HdrMetadata {
            max_cll: Luminance::from_nits(0),
            max_fall: Luminance::from_nits(0),
            mastering_luminance_min: Luminance::from_nits(0),
            mastering_luminance_max: Luminance::from_nits(0),
            ..HdrMetadata::default()
        };
        // Zero bounds are rejected as unordered — the CTA "unknown
        // mastering" case is represented by min == 0 with a real max.
        assert!(HdrStaticMetadata::from_wire(&m).is_err());
        m = HdrMetadata {
            mastering_luminance_max: Luminance::from_nits(1000),
            ..m
        };
        let h = HdrStaticMetadata::from_wire(&m).unwrap();
        assert!(h.cll_unknown() && h.fall_unknown());
        assert!(!h.bounds_unknown());
        assert_eq!(h.mastering_min, 0);
    }

    #[test]
    fn u16_saturation_is_documented_behavior() {
        // The min-luminance coding (1e-4 cd/m² units) saturates at
        // 6.5535 nits — a legal-but-rare lifted mastering black. The
        // max/CLL/FALL codings can never saturate: the PQ-ceiling gate
        // bounds them to 10 000 nits < 65 535.
        let m = HdrMetadata {
            mastering_luminance_min: Luminance::from_nits(10),
            ..HdrMetadata::default()
        };
        let h = HdrStaticMetadata::from_wire(&m).unwrap();
        assert_eq!(h.mastering_min, u16::MAX);
        assert_eq!(h.mastering_max, 1000);
    }
}
