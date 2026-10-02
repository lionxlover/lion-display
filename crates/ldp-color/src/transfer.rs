//! The canonical transfer curves.
//!
//! f32 encode/decode pairs for the six wire transfer functions
//! ([`TransferFunction`]), **bit-identical** to the renderer's Phase 8
//! hook (the cross-consistency suite pins the two equal: same constants,
//! same branch order, same clamps — divergence would be a defect in
//! either crate). On top of those, this module carries the
//! **f64 PQ reference** ([`pq_decode_f64`], [`pq_encode_f64`]) — the
//! single arithmetic source for the integer PQ ramps, the BT.2390 tone
//! mapper, and the KAVT suite's independent anchors.
//!
//! Domains, locked:
//!
//! * sRGB / gamma 2.2 / 2.8 / linear: relative `0..=1` (1.0 = the
//!   description's reference white),
//! * PQ (SMPTE ST 2084): **absolute** nits/10000 — `1.0` = 10000 cd/m²,
//!   so `decode(0.508) ≈ 0.01` (100 nits),
//! * HLG (ITU-R BT.2100): the OETF⁻¹ to scene-linear `0..=1`; the
//!   display-side OOTF (system gamma) is display policy and lives in
//!   `ldp-hdr`, not in the curve.

use ldp_core::color::TransferFunction;

/// PQ constants, spelled exactly as ST 2084 writes them (rational forms).
#[allow(clippy::unreadable_literal)]
mod pq {
    /// m1 = 2610 / 16384.
    pub const M1: f64 = 2610.0 / 16384.0;
    /// m2 = 2523 / 4096 · 128.
    pub const M2: f64 = 2523.0 / 4096.0 * 128.0;
    /// c1 = 3424 / 4096.
    pub const C1: f64 = 3424.0 / 4096.0;
    /// c2 = 2413 / 4096 · 32.
    pub const C2: f64 = 2413.0 / 4096.0 * 32.0;
    /// c3 = 2392 / 4096 · 32.
    pub const C3: f64 = 2392.0 / 4096.0 * 32.0;
}

/// HLG constants, the ITU-R BT.2100 definitional values, verbatim.
#[allow(clippy::unreadable_literal, clippy::excessive_precision)]
mod hlg {
    /// OETF parameter a.
    pub const A: f32 = 0.17883277;
    /// OETF parameter b.
    pub const B: f32 = 0.28466892;
    /// OETF parameter c.
    pub const C: f32 = 0.55991073;
}

/// Decode a transfer function: encoded `0..=1` → linear `0..=1`
/// (relative for SDR curves; PQ decodes to absolute nits/10000).
#[must_use]
pub fn decode_transfer(tf: TransferFunction, v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    let out = match tf {
        TransferFunction::Srgb => {
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        }
        TransferFunction::Pq => {
            const M1: f32 = 2610.0 / 16384.0;
            const M2: f32 = 2523.0 / 4096.0 * 128.0;
            const C1: f32 = 3424.0 / 4096.0;
            const C2: f32 = 2413.0 / 4096.0 * 32.0;
            const C3: f32 = 2392.0 / 4096.0 * 32.0;
            let vp = v.powf(1.0 / M2);
            let num = (vp - C1).max(0.0);
            let den = (C2 - C3 * vp).max(1e-9);
            (num / den).powf(1.0 / M1)
        }
        TransferFunction::Hlg => {
            if v <= 0.5 {
                (v * v) / 3.0
            } else {
                (((v - hlg::C) / hlg::A).exp() + hlg::B) / 12.0
            }
        }
        TransferFunction::Gamma22 => v.powf(2.2),
        TransferFunction::Gamma28 => v.powf(2.8),
        // Linear (and future variants) passes through unchanged.
        _ => v,
    };
    // HLG's inverse OETF can overshoot 1.0 by a rounding step at the top
    // of the range (f32 `exp`); every curve's image is [0, 1].
    out.clamp(0.0, 1.0)
}

/// Encode a transfer function: linear `0..=1` → encoded `0..=1`
/// (PQ's input is absolute nits/10000 — exactly the curve's domain).
#[must_use]
#[allow(clippy::unreadable_literal)] // 0.0031308 is the sRGB knee, verbatim
pub fn encode_transfer(tf: TransferFunction, v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    match tf {
        TransferFunction::Srgb => {
            if v <= 0.0031308 {
                v * 12.92
            } else {
                1.055 * v.powf(1.0 / 2.4) - 0.055
            }
        }
        TransferFunction::Pq => {
            const M1: f32 = 2610.0 / 16384.0;
            const M2: f32 = 2523.0 / 4096.0 * 128.0;
            const C1: f32 = 3424.0 / 4096.0;
            const C2: f32 = 2413.0 / 4096.0 * 32.0;
            const C3: f32 = 2392.0 / 4096.0 * 32.0;
            let yp = v.powf(M1);
            ((C1 + C2 * yp) / (1.0 + C3 * yp)).powf(M2)
        }
        TransferFunction::Hlg => {
            if v <= 1.0 / 12.0 {
                (3.0 * v).sqrt()
            } else {
                hlg::A * (12.0 * v - hlg::B).ln() + hlg::C
            }
        }
        TransferFunction::Gamma22 => v.powf(1.0 / 2.2),
        TransferFunction::Gamma28 => v.powf(1.0 / 2.8),
        _ => v,
    }
}

/// The f64 PQ EOTF: signal `v ∈ 0..=1` → absolute nits/10000.
///
/// This is the reference arithmetic: the ramps, the BT.2390 tone mapper
/// and the KAVT anchors all evaluate PQ through this single f64 path
/// (transcribed from the ST 2084 formula), so integer tables never
/// inherit f32 rounding drift.
#[must_use]
pub fn pq_decode_f64(v: f64) -> f64 {
    let v = v.clamp(0.0, 1.0);
    let vp = v.powf(1.0 / pq::M2);
    let num = (vp - pq::C1).max(0.0);
    let den = (pq::C2 - pq::C3 * vp).max(1e-9);
    (num / den).powf(1.0 / pq::M1)
}

/// The f64 PQ inverse EOTF: absolute nits/10000 → signal `0..=1`.
#[must_use]
pub fn pq_encode_f64(y: f64) -> f64 {
    let y = y.clamp(0.0, 1.0);
    let yp = y.powf(pq::M1);
    ((pq::C1 + pq::C2 * yp) / (1.0 + pq::C3 * yp)).powf(pq::M2)
}

/// Absolute nits → PQ signal (convenience wrapper over [`pq_encode_f64`]).
#[must_use]
pub fn pq_encode_nits(nits: f64) -> f64 {
    pq_encode_f64(nits / 10_000.0)
}

/// PQ signal → absolute nits (convenience wrapper over [`pq_decode_f64`]).
#[must_use]
pub fn pq_decode_nits(v: f64) -> f64 {
    pq_decode_f64(v) * 10_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f64_pq_round_trips_tightly() {
        // f64 round trips to ~1e-12 through both directions (the f32
        // pair only manages ~2e-3 — the reason the tables exist).
        // Exact black is excluded: PQ's inverse EOTF maps luminance 0
        // to the black signal C1^M2 (see `pq_reference_anchors`).
        for v in [1e-6, 0.01, 0.1, 0.5, 0.508, 0.75, 0.9, 1.0] {
            let back = pq_encode_f64(pq_decode_f64(v));
            assert!(
                (back - v).abs() <= 1e-11,
                "f64 PQ round trip at {v}: {back}"
            );
            let forth = pq_decode_f64(pq_encode_f64(v));
            assert!(
                (forth - v).abs() <= 1e-12,
                "f64 PQ inverse round trip at {v}: {forth}"
            );
        }
    }

    #[test]
    fn pq_reference_anchors() {
        // Hand-verified ST 2084 anchors (see tests/kavt.rs for the full
        // independent transcription): 100 nits ≈ 0.508, 203 nits ≈ 0.5806,
        // 10 000 nits = 1.0 exactly, and true black encodes to the
        // black signal C1^M2 ≈ 7.3096e-7 (a published PQ constant —
        // luminance 0 is the curve's one non-bijective boundary point).
        assert!((pq_encode_nits(100.0) - 0.508).abs() < 1e-3);
        assert!((pq_encode_nits(203.0) - 0.5806).abs() < 5e-4);
        assert_eq!(pq_encode_nits(10_000.0), 1.0);
        assert_eq!(pq_decode_f64(1.0), 1.0);
        assert_eq!(pq_decode_f64(0.0), 0.0);
        let black = pq_encode_nits(0.0);
        assert!(
            (black - 7.309_559e-7).abs() < 1e-12,
            "PQ black signal {black}"
        );
        // And the black signal decodes back to (numerically) zero.
        assert!(pq_decode_f64(black) < 1e-12);
        assert_eq!(pq_decode_nits(1.0), 10_000.0);
    }

    #[test]
    fn f32_curves_match_their_domains() {
        // Every curve maps [0,1] → [0,1] monotonically (sampled).
        let tfs = [
            TransferFunction::Linear,
            TransferFunction::Srgb,
            TransferFunction::Pq,
            TransferFunction::Hlg,
            TransferFunction::Gamma22,
            TransferFunction::Gamma28,
        ];
        for tf in tfs {
            let mut prev = -1.0;
            for i in 0..=64 {
                let v = i as f32 / 64.0;
                let lin = decode_transfer(tf, v);
                assert!((0.0..=1.0).contains(&lin), "{tf:?} image at {v}");
                assert!(lin >= prev, "{tf:?} monotone at {v}");
                prev = lin;
                let enc = encode_transfer(tf, lin);
                assert!((0.0..=1.0).contains(&enc), "{tf:?} encoded range at {v}");
            }
        }
        // Out-of-range inputs clamp on both sides.
        assert_eq!(decode_transfer(TransferFunction::Srgb, -0.5), 0.0);
        assert_eq!(decode_transfer(TransferFunction::Srgb, 2.0), 1.0);
        assert_eq!(encode_transfer(TransferFunction::Hlg, 7.0), 1.0);
    }

    #[test]
    fn hlg_definitional_points() {
        // BT.2100: OETF⁻¹(0.5) = 1/12 exactly; OETF(1/12) = 0.5;
        // OETF(1.0) = 1.0 by construction of a, b, c; OETF⁻¹(0.75)
        // ≈ 0.2649 (published).
        assert!((decode_transfer(TransferFunction::Hlg, 0.5) - 1.0 / 12.0).abs() < 1e-6);
        assert!((encode_transfer(TransferFunction::Hlg, 1.0 / 12.0) - 0.5).abs() < 1e-6);
        assert!((encode_transfer(TransferFunction::Hlg, 1.0) - 1.0).abs() < 1e-6);
        assert!((decode_transfer(TransferFunction::Hlg, 0.75) - 0.264_898).abs() < 1e-3);
    }
}
