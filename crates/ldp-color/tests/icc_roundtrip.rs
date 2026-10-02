//! ICC round-trip exit criterion: parse ↔ write on fixture profiles.
//!
//! "ICC parse round-trips on fixture profiles" — realized as three
//! fixtures (sRGB-class v2 with table TRCs, Display-P3-class v4 with
//! parametric TRCs and a `chad`, gamma-2.2 v2), each built on the
//! fixed-point grids the wire format carries:
//!
//! * `parse(write(m)) == m` exactly (model equality — the f64 model
//!   sits on the s15Fixed16 / u8Fixed8 grids),
//! * `write(parse(write(m)))` is **byte-identical** (the canonical
//!   layout is a fixed point),
//! * resolution returns the right parametric description per fixture,
//!   with fit errors under the documented thresholds,
//! * corrupted profiles fail with typed errors, never panics.
//!
//! Fixture construction: the PCS columns are the Bradford-adapted
//! standard primaries (`chad · rgb_to_xyz` columns), quantized to
//! s15Fixed16 — exactly how real sRGB/P3 profiles are built, so the
//! de-adaptation path (parse → native matrix) is exercised against
//! the known primaries, not just against itself.

use ldp_color::icc::{parse, IccProfile};
use ldp_color::icc_trc::{IccTrc, ParametricCurve};
use ldp_color::icc_write::write;
use ldp_color::matrix;
use ldp_core::color::TransferFunction;
use ldp_core::color::{ColorDescription, Luminance, Primaries};

/// Quantize to the s15Fixed16 grid (the wire's resolution).
fn q16(v: f64) -> f64 {
    (v * 65_536.0).round() / 65_536.0
}

/// Quantize to the u8Fixed8 grid (v2 curveType gamma resolution).
fn q8(v: f64) -> f64 {
    (v * 256.0).round() / 256.0
}

/// The D50-adapted (PCS) columns of a primaries set, on-grid.
fn pcs_columns(p: Primaries) -> [[f64; 3]; 3] {
    let chad = matrix::bradford_d65_to_d50();
    let m = matrix::rgb_to_xyz(p);
    let adapt = |col: [f64; 3]| -> [f64; 3] {
        [
            q16(f64::from(chad[0]) * col[0]
                + f64::from(chad[1]) * col[1]
                + f64::from(chad[2]) * col[2]),
            q16(f64::from(chad[3]) * col[0]
                + f64::from(chad[4]) * col[1]
                + f64::from(chad[5]) * col[2]),
            q16(f64::from(chad[6]) * col[0]
                + f64::from(chad[7]) * col[1]
                + f64::from(chad[8]) * col[2]),
        ]
    };
    [
        adapt([f64::from(m[0]), f64::from(m[3]), f64::from(m[6])]),
        adapt([f64::from(m[1]), f64::from(m[4]), f64::from(m[7])]),
        adapt([f64::from(m[2]), f64::from(m[5]), f64::from(m[8])]),
    ]
}

/// A 33-point sRGB TRC table on the u16 grid.
fn srgb_table() -> Vec<u16> {
    (0..33)
        .map(|i| {
            let x = f64::from(i) / 32.0;
            let lin = if x <= 0.04045 {
                x / 12.92
            } else {
                ((x + 0.055) / 1.055).powf(2.4)
            };
            (lin * 65_535.0).round() as u16
        })
        .collect()
}

/// The sRGB parametric TRC (ICC type 3), constants on-grid.
fn srgb_parametric() -> ParametricCurve {
    ParametricCurve {
        kind: 3,
        g: q16(2.4),
        a: q16(1.0 / 1.055),
        b: q16(0.055 / 1.055),
        c: q16(1.0 / 12.92),
        d: q16(0.04045),
        e: 0.0,
        f: 0.0,
    }
}

fn srgb_v2_fixture() -> IccProfile {
    IccProfile {
        version: 2,
        description: "sRGB-class v2 fixture".to_string(),
        copyright: Some("LDP fixture, CC0".to_string()),
        rendering_intent: 0,
        white_point: [q16(0.9642), q16(1.0), q16(0.8249)],
        columns: pcs_columns(Primaries::Bt709),
        trcs: [
            IccTrc::Table(srgb_table()),
            IccTrc::Table(srgb_table()),
            IccTrc::Table(srgb_table()),
        ],
        chad: None,
    }
}

fn p3_v4_fixture() -> IccProfile {
    // v4 with the sRGB-shaped parametric TRC and a Bradford chad.
    let mut chad = [0.0f64; 9];
    for (dst, src) in chad.iter_mut().zip(matrix::bradford_d65_to_d50()) {
        *dst = q16(f64::from(src));
    }
    IccProfile {
        version: 4,
        description: "Display-P3-class v4 fixture".to_string(),
        copyright: None,
        rendering_intent: 1,
        white_point: [q16(0.9642), q16(1.0), q16(0.8249)],
        columns: pcs_columns(Primaries::DciP3),
        trcs: [
            IccTrc::Parametric(srgb_parametric()),
            IccTrc::Parametric(srgb_parametric()),
            IccTrc::Parametric(srgb_parametric()),
        ],
        chad: Some(chad),
    }
}

fn gamma_v2_fixture() -> IccProfile {
    IccProfile {
        version: 2,
        description: "Gamma 2.2 v2 fixture".to_string(),
        copyright: None,
        rendering_intent: 0,
        white_point: [q16(0.9642), q16(1.0), q16(0.8249)],
        columns: pcs_columns(Primaries::Bt709),
        // u8Fixed8 grid: 2.2 is NOT on it; the fixture uses 563/256.
        trcs: [
            IccTrc::Gamma(q8(2.2)),
            IccTrc::Gamma(q8(2.2)),
            IccTrc::Gamma(q8(2.2)),
        ],
        chad: None,
    }
}

#[test]
fn fixtures_round_trip_model_and_bytes() {
    for fixture in [srgb_v2_fixture(), p3_v4_fixture(), gamma_v2_fixture()] {
        let bytes = write(&fixture).expect("fixture serializes");
        // Header sanity: size word agrees, acsp present.
        let size = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
        assert_eq!(size, bytes.len(), "size word");
        assert_eq!(&bytes[36..40], b"acsp");
        // Model round trip: exact.
        let parsed = parse(&bytes).expect("fixture parses");
        assert_eq!(parsed, fixture, "model round trip {}", fixture.description);
        // Canonical fixed point: byte-identical re-serialization.
        let bytes2 = write(&parsed).expect("re-serializes");
        assert_eq!(bytes, bytes2, "canonical bytes {}", fixture.description);
        // And once more through the cycle (idempotence).
        let parsed2 = parse(&bytes2).unwrap();
        assert_eq!(parsed2, parsed);
    }
}

#[test]
fn resolution_fits_the_right_descriptions() {
    let sdr = (
        Luminance::from_nits(0),
        Luminance::from_nits(80),
        Luminance::from_nits(80),
    );
    // sRGB v2: BT.709 + sRGB, both exact fits.
    let p = parse(&write(&srgb_v2_fixture()).unwrap()).unwrap();
    let r = p.resolve(sdr);
    assert_eq!(r.description.primaries, Primaries::Bt709);
    assert_eq!(r.description.transfer, TransferFunction::Srgb);
    assert!(
        r.primaries_error < 0.01,
        "sRGB primaries fit {}",
        r.primaries_error
    );
    assert!(
        r.transfer_error < 0.01,
        "sRGB transfer fit {}",
        r.transfer_error
    );
    assert_eq!(r.description.reference_white.as_nits(), 80);
    // P3 v4: DCI-P3 primaries + sRGB transfer.
    let p = parse(&write(&p3_v4_fixture()).unwrap()).unwrap();
    let r = p.resolve(sdr);
    assert_eq!(r.description.primaries, Primaries::DciP3);
    assert_eq!(r.description.transfer, TransferFunction::Srgb);
    assert!(
        r.primaries_error < 0.01,
        "P3 primaries fit {}",
        r.primaries_error
    );
    // Gamma 2.2 v2: BT.709 + Gamma22 (563/256 = 2.1992… ≈ 2.2).
    let p = parse(&write(&gamma_v2_fixture()).unwrap()).unwrap();
    let r = p.resolve(sdr);
    assert_eq!(r.description.primaries, Primaries::Bt709);
    assert_eq!(r.description.transfer, TransferFunction::Gamma22);
    assert!(r.transfer_error < 0.01, "gamma fit {}", r.transfer_error);
    let _ = ColorDescription::default(); // doc-chain reference
}

#[test]
fn native_matrices_recover_the_primaries() {
    // The de-adapted native RGB→XYZ matrices land back on the standard
    // D65 matrices (within the Bradford + quantization round trip).
    for (fixture, prim) in [
        (srgb_v2_fixture(), Primaries::Bt709),
        (p3_v4_fixture(), Primaries::DciP3),
        (gamma_v2_fixture(), Primaries::Bt709),
    ] {
        let p = parse(&write(&fixture).unwrap()).unwrap();
        let native = p.native_rgb_to_xyz();
        let want = matrix::rgb_to_xyz(prim);
        for i in 0..9 {
            assert!(
                (native[i] - f64::from(want[i])).abs() < 5e-3,
                "{} native matrix [{i}]: {} vs {}",
                fixture.description,
                native[i],
                want[i]
            );
        }
    }
}

#[test]
fn corrupted_profiles_fail_typed() {
    let good = write(&srgb_v2_fixture()).unwrap();
    // Truncation at every length ≥ 132+... and a few below.
    for cut in [0usize, 4, 131, 132, 140, good.len() - 1] {
        assert!(parse(&good[..cut]).is_err(), "truncate at {cut}");
    }
    // Size word disagreeing.
    let mut bad = good.clone();
    bad[3] ^= 1;
    assert!(parse(&bad).is_err());
    // Bad class.
    let mut bad = good.clone();
    bad[12..16].copy_from_slice(b"scnr");
    assert!(parse(&bad).is_err());
    // Non-acsp.
    let mut bad = good.clone();
    bad[36..40].copy_from_slice(b"xxxx");
    assert!(parse(&bad).is_err());
    // Illuminant off D50.
    let mut bad = good.clone();
    let off = (0.5f64 * 65_536.0) as i32;
    bad[68..72].copy_from_slice(&off.to_be_bytes());
    assert!(parse(&bad).is_err());
    // Tag count absurd.
    let mut bad = good.clone();
    bad[128..132].copy_from_slice(&4096u32.to_be_bytes());
    assert!(parse(&bad).is_err());
    // A corrupted TRC payload (wrong signature inside rTRC): find the
    // rTRC entry and clobber its first payload byte.
    let count = u32::from_be_bytes([bad[128], bad[129], bad[130], bad[131]]) as usize;
    for i in 0..count {
        let base = 132 + i * 12;
        if &bad[base..base + 4] == b"rTRC" {
            let off =
                u32::from_be_bytes([bad[base + 4], bad[base + 5], bad[base + 6], bad[base + 7]])
                    as usize;
            bad[off] = b'X';
            break;
        }
    }
    assert!(parse(&bad).is_err(), "corrupted TRC signature rejected");
}

#[test]
fn writer_rejects_unrepresentable_models() {
    use ldp_color::icc_write::WriteError;
    // Parametric TRC at v2.
    let mut m = srgb_v2_fixture();
    m.trcs = [
        IccTrc::Parametric(srgb_parametric()),
        IccTrc::Parametric(srgb_parametric()),
        IccTrc::Parametric(srgb_parametric()),
    ];
    assert_eq!(write(&m), Err(WriteError::ParametricNeedsV4));
    // Non-ASCII description at v2.
    let mut m = srgb_v2_fixture();
    m.description = "café".to_string();
    assert_eq!(write(&m), Err(WriteError::NonAsciiDescription));
    // Off-range fixed point.
    let mut m = srgb_v2_fixture();
    m.columns[0][0] = 40_000.0;
    assert_eq!(write(&m), Err(WriteError::FixedPointOutOfRange));
    // v4 handles the non-ASCII description (mluc/UTF-16).
    let mut m = p3_v4_fixture();
    m.description = "café ✓".to_string();
    let bytes = write(&m).expect("v4 mluc accepts non-ASCII");
    assert_eq!(parse(&bytes).unwrap(), m, "utf-16 round trip");
}
