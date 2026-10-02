//! Phase 14 HDR exit-criteria suite.
//!
//! * **Luminance adaptation monotonicity** (the roadmap EC): the
//!   SDR-in-HDR canvas is monotone in the input value and in the
//!   canvas level over a randomized corpus, and the full
//!   adaptation → output-tail chain (SDR gray ramp onto a PQ panel)
//!   is monotone with the canvas anchored exactly at its level.
//! * **Policy determinism + no-flap**: randomized stacks drive the
//!   [`ModeController`] with dwell hysteresis — mode changes never
//!   exceed one per dwell window, and identical input sequences
//!   produce identical mode sequences.
//! * **Metadata → InfoFrame plumbing**: the full
//!   `HdrMetadata` → validation → [`HdrInfoFrame`] → bytes → parse
//!   chain round-trips, with the checksum property holding at every
//!   step.

use ldp_color::gamut::GamutMode;
use ldp_color::pipeline::OutputTransform;
use ldp_core::color::{ColorDescription, ColorRange, Luminance, Primaries, TransferFunction};
use ldp_hdr::adaptation::LuminanceAdapter;
use ldp_hdr::infoframe::{Eotf, HdrInfoFrame};
use ldp_hdr::metadata::HdrStaticMetadata;
use ldp_hdr::policy::{ModeController, OutputCaps, OutputMode, StackSummary};

/// Deterministic LCG (the project's no-rand doctrine).
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
fn adaptation_monotonicity_over_a_randomized_corpus() {
    let mut rng = Lcg(0x14_a0);
    for _ in 0..500 {
        let canvas = 40.0 + rng.next_f32() * 1960.0;
        let a = LuminanceAdapter::new(canvas).unwrap();
        // Monotone in the input value.
        let mut prev = -1.0;
        for i in 0..=32 {
            let v = a.sdr_to_absolute(i as f32 / 32.0);
            assert!(v >= prev, "input monotone (canvas {canvas})");
            prev = v;
        }
        // Monotone in the canvas level at several inputs.
        for rel in [0.05f32, 0.25, 0.5, 0.75, 1.0] {
            let mut prev = -1.0;
            for k in 0..=16 {
                let c = 40.0 + k as f32 * 120.0;
                let out = LuminanceAdapter::new(c).unwrap().sdr_to_absolute(rel);
                assert!(out >= prev, "canvas monotone at {rel}/{c}");
                prev = out;
            }
        }
        // Identity anchors: white on the canvas, black at zero.
        assert_eq!(a.sdr_to_absolute(1.0), canvas);
        assert_eq!(a.sdr_to_absolute(0.0), 0.0);
    }
}

#[test]
fn sdr_ramp_onto_a_pq_panel_is_monotone_and_anchored() {
    // The full chain: SDR-relative gray → canvas anchoring → PQ output
    // tail. Reference white lands exactly on the canvas's PQ code; the
    // ramp is monotone; a brighter canvas lifts every gray level.
    let pq_target = ColorDescription {
        primaries: Primaries::Bt2020,
        transfer: TransferFunction::Pq,
        range: ColorRange::Full,
        luminance_min: Luminance::from_wire_units(50),
        luminance_max: Luminance::from_nits(1000),
        reference_white: Luminance::from_nits(203),
    };
    let ot = |panel: f32| {
        OutputTransform::new(
            pq_target,
            (0.005, panel),
            (0.005, 10_000.0),
            GamutMode::Clip,
        )
        .unwrap()
    };
    let encode_pq =
        |nits: f32| ldp_color::transfer::encode_transfer(TransferFunction::Pq, nits / 10_000.0);

    for (canvas, panel) in [(203.0f32, 1000.0), (300.0, 1000.0), (203.0, 600.0)] {
        let a = LuminanceAdapter::new(canvas).unwrap();
        let transform = ot(panel);
        let mut prev = -1.0;
        for i in 0..=64 {
            let rel = i as f32 / 64.0;
            let nits = a.sdr_to_absolute(rel);
            let out = transform.map([nits, nits, nits]);
            for c in out {
                assert!((0.0..=1.0).contains(&c), "range at {rel}");
            }
            assert!(out[0] >= prev - 1e-6, "chain monotone at {rel}");
            prev = out[0];
        }
        // White anchors exactly on the canvas (within the panel).
        let white = transform.map([canvas, canvas, canvas]);
        let want = encode_pq(canvas.min(panel));
        assert!(
            (white[0] - want).abs() < 2e-3,
            "white anchored at {canvas}: {white:?} vs {want}"
        );
    }
    // A brighter canvas lifts every mid gray (the appearance policy).
    let a203 = LuminanceAdapter::new(203.0).unwrap();
    let a400 = LuminanceAdapter::new(400.0).unwrap();
    let t = ot(1000.0);
    for i in 1..=32 {
        let rel = i as f32 / 32.0;
        let lo = t.map([a203.sdr_to_absolute(rel); 3])[0];
        let hi = t.map([a400.sdr_to_absolute(rel); 3])[0];
        assert!(hi >= lo, "brighter canvas lifts {rel}");
    }
}

#[test]
fn policy_is_deterministic_and_never_flaps() {
    let mut rng = Lcg(0x14_b0);
    let caps = [
        OutputCaps::sdr_only(),
        OutputCaps::hdr_display(),
        OutputCaps {
            hdr: true,
            pq: false,
            hlg: true,
            max_luminance: Luminance::from_nits(1000),
            wide_gamut: true,
        },
    ];
    let dwell = 4u32;
    let mut ctl = ModeController::new(OutputMode::Sdr, dwell);
    let mut mirror = ModeController::new(OutputMode::Sdr, dwell);
    let mut last_change_step: i32 = -(dwell as i32) - 1;
    let mut changes = 0;
    for step in 0..2000 {
        let caps = caps[(rng.next_f32() * 3.0) as usize % 3];
        let stack = StackSummary {
            any_pq: rng.next_f32() < 0.4,
            any_hlg: rng.next_f32() < 0.2,
            max_content_luminance: Luminance::from_nits((rng.next_f32() * 4000.0) as u32),
            any_wide_gamut: rng.next_f32() < 0.5,
        };
        let before = ctl.mode();
        let mode = ctl.step(&caps, &stack);
        let mirror_mode = mirror.step(&caps, &stack);
        assert_eq!(mode, mirror_mode, "determinism at step {step}");
        if mode != before {
            changes += 1;
            assert!(
                step - last_change_step > dwell as i32,
                "flap at step {step} (last {last_change_step})"
            );
            last_change_step = step;
        }
    }
    // With randomized stacks over 2000 steps and dwell 4, some
    // switching must have happened (the hysteresis is not a lock).
    assert!(changes > 0, "hysteresis never switched in 2000 steps");
    assert!(changes < 2000 / 4, "switching bounded by dwell");
}

#[test]
fn metadata_to_infoframe_plumbing_round_trips() {
    let mut rng = Lcg(0x14_c0);
    for _ in 0..300 {
        let meta = ldp_core::color::HdrMetadata {
            mastering_primaries: [Primaries::Bt709, Primaries::DciP3, Primaries::Bt2020]
                [(rng.next_f32() * 3.0) as usize % 3],
            mastering_luminance_min: Luminance::from_wire_units((rng.next_f32() * 60_000.0) as u32),
            mastering_luminance_max: Luminance::from_nits(100 + (rng.next_f32() * 9_000.0) as u32),
            max_cll: Luminance::from_nits((rng.next_f32() * 10_000.0) as u32),
            max_fall: Luminance::from_nits((rng.next_f32() * 5_000.0) as u32),
        };
        let Ok(coded) = HdrStaticMetadata::from_wire(&meta) else {
            continue; // unordered bounds (min >= max) — legitimately rejected
        };
        let eotf = [Eotf::Sdr, Eotf::Pq, Eotf::Hlg][(rng.next_f32() * 3.0) as usize % 3];
        let frame = HdrInfoFrame::new(eotf, coded);
        let bytes = frame.encode();
        // The checksum property holds.
        let sum: u32 = bytes.iter().map(|&b| u32::from(b)).sum();
        assert_eq!(sum & 0xFF, 0);
        // Round trip.
        let back = HdrInfoFrame::parse(&bytes).unwrap();
        assert_eq!(back.eotf, eotf);
        assert_eq!(back.metadata, coded);
        assert_eq!(back.metadata.primaries, meta.mastering_primaries);
        // Deterministic bytes.
        assert_eq!(frame.encode(), bytes);
    }
}

#[test]
fn full_output_configuration_composes() {
    // One coherent end-to-end story: an HDR-capable output, a PQ
    // surface stack, the canvas for SDR chrome, and the InfoFrame the
    // output sends — every piece agreeing on the same numbers.
    let caps = OutputCaps::hdr_display();
    let stack = StackSummary {
        any_pq: true,
        any_hlg: false,
        max_content_luminance: Luminance::from_nits(1000),
        any_wide_gamut: true,
    };
    let mut ctl = ModeController::new(OutputMode::Sdr, 2);
    assert_eq!(ctl.step(&caps, &stack), OutputMode::Sdr);
    assert_eq!(ctl.step(&caps, &stack), OutputMode::Sdr);
    assert_eq!(ctl.step(&caps, &stack), OutputMode::HdrPq);
    // The InfoFrame for that mode: PQ EOTF + the stack's mastering.
    let meta = ldp_core::color::HdrMetadata::default();
    let frame = HdrInfoFrame::new(Eotf::Pq, HdrStaticMetadata::from_wire(&meta).unwrap());
    let bytes = frame.encode();
    assert_eq!(bytes[3] & 0x0F, Eotf::Pq.to_wire());
    assert_eq!(u16::from_be_bytes([bytes[23], bytes[24]]), 1000);
    // And the SDR canvas that keeps the desktop chrome at BT.2408
    // reference while the HDR surface owns the highlights.
    let canvas = LuminanceAdapter::default();
    assert_eq!(canvas.canvas_nits(), 203.0);
    let _ = ColorRange::Full;
}
