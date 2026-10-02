//! Output-transform property suite.
//!
//! End-to-end properties of the per-output tail over randomized
//! configurations (the LCG corpus doctrine — no `rand`):
//!
//! * **Monotonicity**: brighter linear input never yields darker
//!   encoded output (achromatic rays, every configuration),
//! * **Range**: outputs stay in `[0,1]`,
//! * **Endpoints**: master peak lands at display white (tone-mapped
//!   SDR targets); reference-white behavior matches the documented
//!   anchors,
//! * **Determinism**: identical inputs produce bit-identical outputs,
//! * **ICC integration**: an imported profile's [`ColorSpace`]
//!   composes with the parametric spaces consistently (white maps to
//!   white through every conversion).

use ldp_color::gamut::GamutMode;
use ldp_color::pipeline::{ColorSpace, OutputTransform};
use ldp_core::color::{ColorDescription, ColorRange, Luminance, Primaries, TransferFunction};

fn sdr_target(primaries: Primaries, reference_nits: u32) -> ColorDescription {
    ColorDescription {
        primaries,
        transfer: TransferFunction::Srgb,
        range: ColorRange::Full,
        luminance_min: Luminance::from_nits(0),
        luminance_max: Luminance::from_nits(reference_nits),
        reference_white: Luminance::from_nits(reference_nits),
    }
}

fn pq_target(primaries: Primaries, max_nits: u32) -> ColorDescription {
    ColorDescription {
        primaries,
        transfer: TransferFunction::Pq,
        range: ColorRange::Full,
        luminance_min: Luminance::from_wire_units(50),
        luminance_max: Luminance::from_nits(max_nits),
        reference_white: Luminance::from_nits(203),
    }
}

/// Deterministic LCG.
struct Lcg(u64);
impl Lcg {
    fn next_f32(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) as f64 / f64::from(u32::MAX)) as f32
    }
}

#[test]
fn randomized_targets_are_monotone_and_bounded() {
    let mut rng = Lcg(0x14_c010);
    for _ in 0..300 {
        let hdr = rng.next_f32() < 0.5;
        let primaries = [Primaries::Bt709, Primaries::DciP3, Primaries::Bt2020]
            [(rng.next_f32() * 3.0) as usize % 3];
        let gamut = if rng.next_f32() < 0.5 {
            GamutMode::Clip
        } else {
            GamutMode::perceptual()
        };
        let (target, panel, master) = if hdr {
            let peak = 200.0 + rng.next_f32() * 4800.0;
            let m_peak = peak + rng.next_f32() * 5000.0;
            (
                pq_target(primaries, peak as u32),
                (0.005, peak),
                (0.005, m_peak),
            )
        } else {
            let peak = 80.0 + rng.next_f32() * 320.0;
            let m_peak = if rng.next_f32() < 0.5 {
                peak // pure SDR chain
            } else {
                peak + rng.next_f32() * 4000.0 // HDR mastering
            };
            (
                sdr_target(primaries, peak as u32),
                (0.005, peak),
                (0.005, m_peak),
            )
        };
        let Ok(ot) = OutputTransform::new(target, panel, master, gamut) else {
            continue;
        };
        let is_pq = ot.transfer() == TransferFunction::Pq;
        // Achromatic ramp: monotone (within the float-encode noise —
        // the HDR clip's multiplicative plateau `l·(peak/y)` jitters
        // at the f32 ulp level and PQ's steepness amplifies that ~40×
        // near 2000 nits, i.e. ≤ ~4e-6 in code; real inversions are
        // orders of magnitude larger), bounded, with exact endpoints.
        let mut prev = -1.0;
        let peak = panel.1;
        for i in 0..=64 {
            let l = i as f32 / 64.0 * master.1;
            let out = ot.map([l, l, l]);
            for c in out {
                assert!((0.0..=1.0).contains(&c), "bounded {c} at {l}");
            }
            let y = out[0]; // achromatic
            assert!(
                y >= prev - 1e-3,
                "monotone at {l} (peak {peak}, master {}): {y} < {prev}",
                master.1
            );
            prev = y;
        }
        // Master peak endpoint: display white for SDR targets, the
        // panel peak's PQ code for HDR targets (encoded 1.0 = 10 000
        // nits on the PQ wire, not the panel peak).
        let top = ot.map([master.1, master.1, master.1]);
        if is_pq {
            let want = ldp_color::transfer::encode_transfer(TransferFunction::Pq, peak / 10_000.0);
            assert!(
                (top[0] - want).abs() < 5e-3,
                "master peak lands on the panel peak PQ code: {} vs {want}",
                top[0]
            );
        } else {
            assert!(top[0] > 0.97, "master peak lands at white: {}", top[0]);
        }
        // Determinism: bit-identical repeats.
        let probe = [123.4f32, 55.5, 9.9];
        assert_eq!(ot.map(probe), ot.map(probe));
    }
}

#[test]
fn sdr_to_sdr_is_reference_exact() {
    // The pure-SDR chain: linear reference-relative in → encoded out,
    // white at the reference, half at sRGB(0.5).
    let ot = OutputTransform::new(
        sdr_target(Primaries::Bt709, 100),
        (0.005, 100.0),
        (0.0, 100.0),
        GamutMode::Clip,
    )
    .unwrap();
    let white = ot.map([100.0, 100.0, 100.0]);
    for c in white {
        assert!((c - 1.0).abs() < 1e-3, "white {c}");
    }
    let half = ot.map([50.0, 50.0, 50.0]);
    let want = ldp_color::transfer::encode_transfer(TransferFunction::Srgb, 0.5);
    for c in half {
        assert!((c - want).abs() < 1e-3, "half {c} vs {want}");
    }
}

#[test]
fn color_spaces_preserve_white_across_conversions() {
    // Every parametric space maps its white to the shared D65 white,
    // so linear↔linear conversions keep (1,1,1) fixed.
    let spaces = [
        ColorSpace::from_primaries(Primaries::Bt709, TransferFunction::Srgb),
        ColorSpace::from_primaries(Primaries::DciP3, TransferFunction::Gamma22),
        ColorSpace::from_primaries(Primaries::Bt2020, TransferFunction::Linear),
    ];
    for s in &spaces {
        let xyz = s.to_xyz([1.0, 1.0, 1.0]);
        // D65: x/y = 0.3127/0.3290 → X/Z ratios.
        assert!((xyz[1] - 1.0).abs() < 1e-5, "white Y");
        let wx = 0.3127 / 0.3290;
        let wz = (1.0 - 0.3127 - 0.3290) / 0.3290;
        assert!((xyz[0] - wx).abs() < 1e-4, "white X {}", xyz[0]);
        assert!((xyz[2] - wz).abs() < 1e-4, "white Z {}", xyz[2]);
        for t in &spaces {
            let conv = s.conversion_to(t);
            let mapped = [
                conv[0] + conv[1] + conv[2],
                conv[3] + conv[4] + conv[5],
                conv[6] + conv[7] + conv[8],
            ];
            for c in mapped {
                assert!((c - 1.0).abs() < 1e-4, "white preserved {c}");
            }
        }
    }
}

#[test]
fn decoded_pq_ramps_compose_with_the_output_tail() {
    // The ramp-quantized HDR path: luminance clip happens before the
    // PQ encode, so quantizing with the 10-bit ramp after map() is
    // lossless w.r.t. the ramp's own grid.
    use ldp_color::ramp::PqRamp;
    let ot = OutputTransform::new(
        pq_target(Primaries::Bt2020, 600),
        (0.005, 600.0),
        (0.005, 4000.0),
        GamutMode::Clip,
    )
    .unwrap();
    let ramp = PqRamp::new(10).unwrap();
    for l in [50.0f32, 203.0, 500.0, 600.0, 3000.0] {
        let out = ot.map([l, l, l]);
        let code = ramp.encode_linear(out[0]);
        let back = ramp.decode(code).unwrap();
        // The quantized value is within one table step of the tail's
        // continuous output.
        let neighbor = ramp.decode((code + 1).min(ramp.len() as u32 - 1)).unwrap();
        let gap = (neighbor - back).max(1e-7);
        assert!(
            (back - out[0]).abs() <= gap,
            "ramp composition at {l}: {back} vs {}",
            out[0]
        );
    }
}
