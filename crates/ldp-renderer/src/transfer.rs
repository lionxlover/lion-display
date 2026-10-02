//! Transfer-function curves: the deterministic decode/encode pair every
//! backend and the color pipeline share.
//!
//! All curves map `[0,1] → [0,1]`; PQ decodes to *absolute* nits/10000
//! (1.0 = 10000 nits) which the pipeline's reference-white anchor
//! renormalizes. Only these curves touch libm `powf` — the golden suites
//! anchor them with tolerances while every integer path stays byte-exact.

use ldp_core::color::TransferFunction;

/// Decode a transfer function: encoded `0..=1` → linear `0..=1` (relative,
/// 1.0 = the description's reference white for SDR curves; PQ decodes to
/// absolute nits/10000 and is normalized by the caller's anchor).
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
            // SMPTE ST 2084. Constants: m1 = 2610/16384,
            // m2 = 2523/4096 * 128, c1 = 3424/4096, c2 = 2413/4096 * 32,
            // c3 = 2392/4096 * 32. The result is absolute nits/10000
            // (1.0 = 10000 nits).
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
            // OETF^-1 to scene-linear 0..1 (the OOTF is display-side and
            // deferred to the reference-white anchor). The constants are
            // ITU-R BT.2100 definitional values, kept verbatim.
            #[allow(clippy::unreadable_literal, clippy::excessive_precision)]
            const A: f32 = 0.17883277;
            #[allow(clippy::unreadable_literal, clippy::excessive_precision)]
            const B: f32 = 0.28466892;
            #[allow(clippy::unreadable_literal, clippy::excessive_precision)]
            const C: f32 = 0.55991073;
            if v <= 0.5 {
                (v * v) / 3.0
            } else {
                (((v - C) / A).exp() + B) / 12.0
            }
        }
        TransferFunction::Gamma22 => v.powf(2.2),
        TransferFunction::Gamma28 => v.powf(2.8),
        // Linear (and future variants) passes through unchanged.
        _ => v,
    };
    // HLG's inverse OETF can overshoot 1.0 by a rounding step at the top of
    // the range (f32 `exp`); every curve's image is [0, 1], so clamp once
    // at the exit.
    out.clamp(0.0, 1.0)
}

/// Encode a transfer function: linear `0..=1` → encoded `0..=1`.
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
            // `v` is absolute nits/10000 — exactly the curve's input domain.
            let yp = v.powf(M1);
            ((C1 + C2 * yp) / (1.0 + C3 * yp)).powf(M2)
        }
        TransferFunction::Hlg => {
            // ITU-R BT.2100 definitional constants, kept verbatim.
            #[allow(clippy::unreadable_literal, clippy::excessive_precision)]
            const A: f32 = 0.17883277;
            #[allow(clippy::unreadable_literal, clippy::excessive_precision)]
            const B: f32 = 0.28466892;
            #[allow(clippy::unreadable_literal, clippy::excessive_precision)]
            const C: f32 = 0.55991073;
            if v <= 1.0 / 12.0 {
                (3.0 * v).sqrt()
            } else {
                A * (12.0 * v - B).ln() + C
            }
        }
        TransferFunction::Gamma22 => v.powf(1.0 / 2.2),
        TransferFunction::Gamma28 => v.powf(1.0 / 2.8),
        // Linear (and future variants) passes through unchanged.
        _ => v,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sRGB knee constant (test-side alias for readability).
    #[allow(clippy::unreadable_literal)] // the sRGB knee, verbatim
    const SRGB_KNEE: f32 = 0.0031308;

    fn close(a: f32, b: f32, tol: f32) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn srgb_transfer_anchors() {
        // Definitional anchors: sRGB(0.5) ≈ 0.21404, inverse at the knee.
        assert!(close(
            decode_transfer(TransferFunction::Srgb, 0.5),
            0.21404,
            1e-3
        ));
        assert!(close(
            decode_transfer(TransferFunction::Srgb, 0.04045),
            0.04045 / 12.92,
            1e-6
        ));
        assert!(close(
            encode_transfer(TransferFunction::Srgb, SRGB_KNEE),
            SRGB_KNEE * 12.92,
            1e-6
        ));
    }
    #[test]
    fn transfers_round_trip() {
        for tf in [
            TransferFunction::Linear,
            TransferFunction::Srgb,
            TransferFunction::Gamma22,
            TransferFunction::Gamma28,
            TransferFunction::Pq,
            TransferFunction::Hlg,
        ] {
            for v in [0.02f32, 0.25, 0.5, 0.75, 1.0] {
                let lin = decode_transfer(tf, v);
                let back = encode_transfer(tf, lin);
                assert!(close(back, v, 2e-3), "{tf:?} round trip at {v}");
                assert!((0.0..=1.0).contains(&lin), "{tf:?} linear range");
            }
        }
    }
    #[test]
    fn pq_absolute_decoding() {
        // PQ 1.0 encodes 10000 nits: decode → 1.0 (nits/10000).
        assert!(close(decode_transfer(TransferFunction::Pq, 1.0), 1.0, 1e-4));
        // PQ 0.508 ≈ 100 nits (ST 2084 table) → 0.01.
        assert!(close(
            decode_transfer(TransferFunction::Pq, 0.508),
            0.01,
            2e-3
        ));
    }
    #[test]
    fn hlg_midpoint_is_quarter() {
        // HLG 0.5 ↔ scene linear 1/12.
        assert!(close(
            decode_transfer(TransferFunction::Hlg, 0.5),
            1.0 / 12.0,
            1e-6
        ));
        assert!(close(
            encode_transfer(TransferFunction::Hlg, 1.0 / 12.0),
            0.5,
            1e-6
        ));
    }
}
