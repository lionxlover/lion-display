// Spec-printed constants are transcribed verbatim (that is the point of
// this suite — separators would obscure the transcription).
#![allow(clippy::unreadable_literal, clippy::excessive_precision)]

//! KAVT — known-answer vector tests against SMPTE/ITU references.
//!
//! THE Phase 14 exit criterion: "known-answer vector tests (SMPTE/ITU
//! references) pass within tolerance". Two layers of reference:
//!
//! 1. **Independent transcriptions**: the standard formulas re-written
//!    here from the spec texts with literal constants (different
//!    spellings than the crate's rational forms) — a transcription
//!    defect in the crate cannot reproduce the test's numbers.
//! 2. **Published decimal anchors**: values from the standards
//!    literature (sRGB 0.5 → 0.7353570, PQ 100 nits → 0.508,
//!    203 nits → 0.5806, the PQ black signal 7.309559e-7, HLG
//!    0.75 → 0.264898, the BT.709/BT.2020 luma coefficients and
//!    conversion matrices).
//!
//! Tolerances: f64 paths at 1e-9 or better; f32 paths at the f32
//! transcription noise (1e-5..1e-3 by magnitude); published 3–7
//! decimal anchors at the anchor's own precision.

use ldp_color::transfer::{decode_transfer, encode_transfer, pq_decode_f64, pq_encode_f64};
use ldp_core::color::TransferFunction;

// ---------------------------------------------------------------------------
// Independent ST 2084 transcription (constants as spec-printed decimals).
// ---------------------------------------------------------------------------

fn ref_pq_encode(l_frac: f64) -> f64 {
    // SMPTE ST 2084 inverse EOTF, constants as printed in the standard:
    // m1 = 0.1593017578125, m2 = 78.84375, c1 = 0.8359375,
    // c2 = 18.8515625, c3 = 18.6875.
    let m1 = 0.1593017578125;
    let m2 = 78.84375;
    let c1 = 0.8359375;
    let c2 = 18.8515625;
    let c3 = 18.6875;
    let yp = l_frac.powf(m1);
    ((c1 + c2 * yp) / (1.0 + c3 * yp)).powf(m2)
}

fn ref_pq_decode(v: f64) -> f64 {
    let m1 = 0.1593017578125;
    let m2 = 78.84375;
    let c1 = 0.8359375;
    let c2 = 18.8515625;
    let c3 = 18.6875;
    let vp = v.powf(1.0 / m2);
    let num = (vp - c1).max(0.0);
    let den = (c2 - c3 * vp).max(1e-9);
    (num / den).powf(1.0 / m1)
}

#[test]
fn pq_matches_the_independent_transcription() {
    for i in 0..=1000 {
        let v = f64::from(i) / 1000.0;
        let got = pq_decode_f64(v);
        let want = ref_pq_decode(v);
        assert!(
            (got - want).abs() <= 1e-12,
            "PQ decode transcription drift at {v}: {got} vs {want}"
        );
        let got_e = pq_encode_f64(v);
        let want_e = ref_pq_encode(v);
        assert!(
            (got_e - want_e).abs() <= 1e-12,
            "PQ encode transcription drift at {v}: {got_e} vs {want_e}"
        );
    }
}

#[test]
fn pq_published_anchors() {
    // The ST 2084 literature anchors (hand-verified values; tolerances
    // matched to each anchor's printed precision).
    let anchors: [(f64, f64, f64); 5] = [
        (100.0, 0.508, 1.5e-3),
        (203.0, 0.580_6, 1e-3),
        (1000.0, 0.752, 2e-3),
        (4000.0, 0.902_6, 2e-3),
        (10_000.0, 1.0, 1e-9),
    ];
    for (nits, want, tol) in anchors {
        let got = ref_pq_encode(nits / 10_000.0);
        assert!(
            (got - want).abs() <= tol,
            "PQ anchor {nits} nits: {got} vs {want}"
        );
    }
    // The PQ black signal (luminance 0's code — the curve's one
    // boundary point): c1^m2 = 7.309559e-7.
    assert!((ref_pq_encode(0.0) - 7.309_559e-7).abs() < 1e-12);
    // 10-bit table spots: code 512 → ≈92.2 nits, code 917 → ≈3776.8
    // nits (independent decode of the quantized signal).
    let nits_512 = ref_pq_decode(f64::from(512) / 1023.0) * 10_000.0;
    assert!((nits_512 - 92.2).abs() < 0.5, "code 512 → {nits_512} nits");
    let nits_917 = ref_pq_decode(f64::from(917) / 1023.0) * 10_000.0;
    assert!(
        (nits_917 - 3776.0).abs() < 2.0,
        "code 917 → {nits_917} nits"
    );
}

#[test]
fn f32_pq_curve_agrees_with_the_f64_reference() {
    // The f32 pipeline curve tracks the f64 reference within f32
    // transcription noise (worst near the top: the cancellation —
    // see the ramp tests; here sampled at 1/64 granularity).
    for i in 0..=64 {
        let v = i as f32 / 64.0;
        let got = decode_transfer(TransferFunction::Pq, v);
        let want = ref_pq_decode(f64::from(v)) as f32;
        assert!(
            (got - want).abs() <= 5e-5,
            "f32 PQ decode at {v}: {got} vs {want}"
        );
        let got_e = encode_transfer(TransferFunction::Pq, v);
        let want_e = ref_pq_encode(f64::from(v)) as f32;
        assert!(
            (got_e - want_e).abs() <= 5e-5,
            "f32 PQ encode at {v}: {got_e} vs {want_e}"
        );
    }
}

// ---------------------------------------------------------------------------
// sRGB (IEC 61966-2-1) and HLG (ITU-R BT.2100).
// ---------------------------------------------------------------------------

#[test]
fn srgb_published_anchors() {
    // Widely published 16-digit values.
    assert!((encode_transfer(TransferFunction::Srgb, 0.5) - 0.735_356_983_052_449_5).abs() < 1e-6);
    assert!((decode_transfer(TransferFunction::Srgb, 0.5) - 0.214_041_140_482_232_55).abs() < 1e-6);
    // The knee pair, exactly.
    assert!((decode_transfer(TransferFunction::Srgb, 0.04045) - 0.04045 / 12.92).abs() < 1e-9);
    assert!((encode_transfer(TransferFunction::Srgb, 0.0031308) - 0.0031308 * 12.92).abs() < 1e-9);
    // A mid-table independent transcription (exp/log form).
    for x in [0.1f32, 0.25, 0.9] {
        let want = (((x + 0.055) / 1.055) as f64).powf(2.4) as f32;
        let got = decode_transfer(TransferFunction::Srgb, x);
        assert!((got - want).abs() < 1e-6, "sRGB decode at {x}");
    }
}

#[test]
fn hlg_published_anchors() {
    // BT.2100 Table 5-class anchors (independent transcription via
    // exp/ln of the spec constants a, b, c).
    let a = 0.17883277_f64;
    let b = 0.28466892_f64;
    let c = 0.55991073_f64;
    let oetf_inv = |e: f64| -> f64 {
        if e <= 0.5 {
            e * e / 3.0
        } else {
            (((e - c) / a).exp() + b) / 12.0
        }
    };
    for e in [0.1f64, 0.5, 0.75, 0.9, 1.0] {
        let want = oetf_inv(e) as f32;
        let got = decode_transfer(TransferFunction::Hlg, e as f32);
        assert!(
            (got - want).abs() < 2e-6,
            "HLG decode at {e}: {got} vs {want}"
        );
    }
    // The published spot: signal 0.75 → scene linear 0.264898.
    assert!((decode_transfer(TransferFunction::Hlg, 0.75) - 0.264_898).abs() < 1e-3);
    // Definitional points.
    assert!((decode_transfer(TransferFunction::Hlg, 0.5) - 1.0 / 12.0).abs() < 1e-6);
    assert!((encode_transfer(TransferFunction::Hlg, 1.0) - 1.0).abs() < 1e-6);
}

// ---------------------------------------------------------------------------
// Matrices (ITU/IEC published values).
// ---------------------------------------------------------------------------

#[test]
fn published_matrix_and_luma_anchors() {
    use ldp_color::matrix::{luma_coefficients, primaries_matrix, rgb_to_xyz};
    use ldp_core::color::Primaries;

    // sRGB → XYZ (D65), published full precision.
    let srgb = rgb_to_xyz(Primaries::Bt709);
    let want = [
        0.412_390_8,
        0.357_584_3,
        0.180_480_8,
        0.212_639_0,
        0.715_168_7,
        0.072_192_3,
        0.019_330_8,
        0.119_194_8,
        0.950_532_2,
    ];
    for (g, w) in srgb.iter().zip(want) {
        assert!((g - w).abs() < 2e-4, "sRGB matrix {g} vs {w}");
    }
    // BT.2020 → XYZ, published.
    let bt2020 = rgb_to_xyz(Primaries::Bt2020);
    let want = [
        0.636_958_0,
        0.144_617_0,
        0.168_881_0,
        0.262_700_0,
        0.677_998_0,
        0.059_302_0,
        0.0,
        0.028_073_0,
        1.060_985_0,
    ];
    for (g, w) in bt2020.iter().zip(want) {
        assert!((g - w).abs() < 2e-4, "BT.2020 matrix {g} vs {w}");
    }
    // BT.709 ↔ BT.2020 conversions, published 4-dp (ITU-R BT.2087
    // class values).
    let fwd = primaries_matrix(Primaries::Bt709, Primaries::Bt2020);
    let want_fwd = [
        0.6270, 0.3293, 0.0437, 0.0691, 0.9195, 0.0114, 0.0164, 0.0880, 0.8956,
    ];
    for (g, w) in fwd.iter().zip(want_fwd) {
        assert!((g - w).abs() < 1e-3, "709→2020 {g} vs {w}");
    }
    // The luma coefficients, both published forms.
    for (g, w) in luma_coefficients(Primaries::Bt709)
        .iter()
        .zip([0.2126, 0.7152, 0.0722])
    {
        assert!((g - w).abs() < 1e-4);
    }
    for (g, w) in luma_coefficients(Primaries::Bt2020)
        .iter()
        .zip([0.2627, 0.6780, 0.0593])
    {
        assert!((g - w).abs() < 1e-4);
    }
    // The Y row of the published sRGB matrix equals the BT.709 luma —
    // cross-anchor.
    assert!((srgb[3] - 0.212_639).abs() < 2e-4);
}

// ---------------------------------------------------------------------------
// BT.2390 EETF structure anchors.
// ---------------------------------------------------------------------------

#[test]
fn bt2390_structural_anchors() {
    use ldp_color::tone::ToneMapper;
    use ldp_color::transfer::{pq_decode_f64, pq_encode_f64};

    // The EETF's PQ-domain structure, re-derived here independently:
    // bounds, 75% knee, identity below the knee, peak→peak.
    let (m_min, m_max, d_min, d_max) = (0.005, 1000.0, 0.005, 100.0);
    let p_min_m = ref_pq_encode(m_min / 10_000.0);
    let p_max_m = ref_pq_encode(m_max / 10_000.0);
    let p_max_d = ref_pq_encode(d_max / 10_000.0);
    let span = p_max_m - p_min_m;
    let d_hi = (p_max_d - p_min_m) / span;
    let knee = 0.75 * d_hi; // d_lo == 0 for the shared black
    let tm = ToneMapper::new((m_min as f32, m_max as f32), (d_min as f32, d_max as f32)).unwrap();

    // Reference EETF (smoothstep-free hand model): identity below the
    // knee; a plain linear-to-peak rolloff above (a coarse bound that
    // brackets the real curve's outputs).
    let map_ref = |y: f64| -> f64 {
        if y <= m_min {
            return y;
        }
        let p = ref_pq_encode(y.min(m_max) / 10_000.0);
        let u = (p - p_min_m) / span;
        let v = if u <= knee {
            u
        } else {
            knee + (d_hi - knee) * (u - knee) / (1.0 - knee)
        };
        let p_out = p_min_m + v * span;
        ref_pq_decode(p_out) * 10_000.0
    };
    // Below the knee the shipped EETF is exactly identity — the
    // reference agrees to PQ-inversion noise.
    for y in [0.01, 0.5, 5.0, 20.0] {
        let got = f64::from(tm.map_luminance(y as f32));
        assert!((got - y).abs() < 1e-3, "identity below knee at {y}");
        assert!((map_ref(y) - y).abs() < 1e-3, "reference identity at {y}");
    }
    // The knee luminance: u = knee → PQ → nits (both implementations).
    let knee_pq = p_min_m + knee * span;
    let knee_nits = pq_decode_f64(knee_pq) * 10_000.0;
    let ref_knee_nits = ref_pq_decode(knee_pq) * 10_000.0;
    assert!((knee_nits - ref_knee_nits).abs() < 1e-6);
    // The shipped curve stays within the identity..linear-rolloff
    // bracket above the knee (the Hermite rolls between them).
    for y in [30.0, 60.0, 100.0, 200.0, 500.0, 999.0] {
        let got = f64::from(tm.map_luminance(y as f32));
        let lower = map_ref(y); // linear rolloff reference
        assert!(
            got >= lower - 0.5,
            "EETF above the linear rolloff bound at {y}: {got} < {lower}"
        );
        assert!(got <= y + 0.5, "EETF never boosts above identity at {y}");
    }
    // Peak lands on peak (both).
    assert!((f64::from(tm.map_luminance(1000.0)) - 100.0).abs() < 0.05);
    assert!((map_ref(1000.0) - 100.0).abs() < 0.05);
    // The 203-nit reference white lands in the documented window
    // (BT.2408 anchoring; ≈ 87.6 nits hand-derived in the module).
    let w = f64::from(tm.map_luminance(203.0));
    assert!((85.0..=90.0).contains(&w), "203-nit anchor {w}");
    let _ = pq_encode_f64; // referenced for the module-level doc chain
}
