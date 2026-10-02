//! Golden color-pipeline conformance: ranges, primaries conversion, the
//! reference-white anchor, and scene-linear blending.
//!
//! Expectations are computed with independent f64 math in this file;
//! transfer-curve paths carry ±2 LSB tolerances (libm `powf` may differ by
//! one rounding step across platforms — the integer paths elsewhere are
//! byte-exact by construction).

use ldp_core::buffer::{BufferGeometry, FourCC, Modifier, PlaneLayout};
use ldp_core::color::{ColorDescription, ColorRange, Primaries, TransferFunction};
use ldp_core::geometry::{Rect, Region, Transform};
use ldp_renderer::testkit::{full_damage, render_to_buffer};
use ldp_renderer::{BufferView, OutputDesc, SurfaceLayer};

fn one_pixel(data: u32, format: FourCC) -> (Vec<u8>, BufferGeometry) {
    let layout = [PlaneLayout {
        offset: 0,
        stride: 4,
    }];
    let g = BufferGeometry::new(1, 1, format, Modifier::LINEAR, &layout, 4).unwrap();
    (data.to_le_bytes().to_vec(), g)
}

fn layer<'a>(
    data: &'a [u8],
    geometry: &BufferGeometry,
    color: ColorDescription,
    opacity: f32,
) -> SurfaceLayer<'a> {
    let view = BufferView::new(1, data, geometry.clone()).unwrap();
    SurfaceLayer::new(
        view,
        Rect::new(0, 0, 1, 1),
        Transform::Normal,
        color,
        opacity,
        Region::new(),
    )
}

fn out(color: ColorDescription) -> OutputDesc {
    OutputDesc::new(1, 1, FourCC::XRGB8888, color).unwrap()
}

fn got_rgb(v: u32) -> (u32, u32, u32) {
    ((v >> 16) & 0xFF, (v >> 8) & 0xFF, v & 0xFF)
}

/// sRGB transfer decode in f64 (independent).
fn srgb_decode(v: f64) -> f64 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// sRGB transfer encode in f64 (independent).
#[allow(clippy::unreadable_literal)] // the sRGB knee, verbatim
fn srgb_encode(v: f64) -> f64 {
    if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

#[test]
fn studio_range_layer_expands_through_the_pipeline() {
    // Studio-range sRGB content onto a full-range sRGB output: gray values
    // pass through range decode → transfer decode → encode → range encode,
    // which cancels back to the range-expanded value (within rounding).
    let mut studio = ColorDescription::srgb_sdr();
    studio.range = ColorRange::Studio;
    for v8 in [16u8, 64, 128, 200, 235] {
        let (data, g) = one_pixel(
            (u32::from(v8) << 16) | (u32::from(v8) << 8) | u32::from(v8),
            FourCC::XRGB8888,
        );
        let got = render_to_buffer(
            &out(ColorDescription::srgb_sdr()),
            &full_damage(&out(ColorDescription::srgb_sdr())),
            &[layer(&data, &g, studio, 1.0)],
        );
        // Expected: ((v*255-16)/219) re-encoded through sRGB decode+encode
        // (identity on the same value up to rounding) then re-expanded.
        let expanded = (f64::from(v8) - 16.0) / 219.0;
        let want = (srgb_encode(srgb_decode(expanded)) * 255.0).round() as u32;
        let (r, gch, b) = got_rgb(got[0]);
        let w = want.min(255);
        assert!(
            r.abs_diff(w) <= 2 && gch.abs_diff(w) <= 2 && b.abs_diff(w) <= 2,
            "v={v8}: got ({r},{gch},{b}) want {w}"
        );
    }
}

#[test]
fn bt2020_gray_stays_gray_but_saturation_moves() {
    // A gray pixel converted BT.2020 → BT.709 stays gray (achromatic axis,
    // shared D65 white); a saturated green does not survive unchanged.
    let gray = 0x00_80_80_80u32;
    // Saturated-but-not-extreme green: the conversion difference survives
    // the pipeline's end clamps (a pure 255 green clamps back to itself).
    let green = 0x00_00_C8_00u32;
    let mut bt2020 = ColorDescription::srgb_sdr();
    bt2020.primaries = Primaries::Bt2020;
    for (word, expect_unchanged) in [(gray, true), (green, false)] {
        let (data, g) = one_pixel(word, FourCC::XRGB8888);
        // Identity: same description — encoded-space write.
        let same = render_to_buffer(
            &out(bt2020),
            &full_damage(&out(bt2020)),
            &[layer(&data, &g, bt2020, 1.0)],
        );
        // Cross-description: 2020 layer onto a 709 output — linear path.
        let cross = render_to_buffer(
            &out(ColorDescription::srgb_sdr()),
            &full_damage(&out(ColorDescription::srgb_sdr())),
            &[layer(&data, &g, bt2020, 1.0)],
        );
        let s = got_rgb(same[0]);
        let c = got_rgb(cross[0]);
        if expect_unchanged {
            // Gray: all channels equal in both, within rounding.
            let (dr, dg, db) = (
                (s.0 as i32 - c.0 as i32).abs(),
                (s.1 as i32 - c.1 as i32).abs(),
                (s.2 as i32 - c.2 as i32).abs(),
            );
            assert!(dr <= 2 && dg <= 2 && db <= 2, "gray moved: {s:?} -> {c:?}");
            assert!(c.0.abs_diff(c.1) <= 2 && c.1.abs_diff(c.2) <= 2);
        } else {
            // Saturated 2020 green converts to a measurably different (and
            // clamped) 709 value: the G channel moves.
            let differs = s != c;
            assert!(differs, "conversion was a no-op for saturated green");
            assert!(
                (s.1 as i32 - c.1 as i32).abs() >= 3,
                "g survived unchanged: {s:?} -> {c:?}"
            );
        }
    }
}

#[test]
fn pq_reference_white_anchors_at_output_white() {
    // PQ-encoded content at the 203-nit HDR reference white lands at the
    // SDR output's reference white (0xFF for a gray, after sRGB encode of
    // relative 1.0); PQ black lands at 0.
    let pq = ColorDescription::pq_hdr();
    // ST 2084 encode of 203/10000 and of 0.
    let v_white = encode_pq(203.0 / 10_000.0);
    let v_black = 0.0f32;
    for (v, want) in [(v_white, 254u32), (v_black, 0u32)] {
        let word = pq_word(v, v, v);
        let (data, g) = one_pixel(word, FourCC::XRGB8888);
        let got = render_to_buffer(
            &out(ColorDescription::srgb_sdr()),
            &full_damage(&out(ColorDescription::srgb_sdr())),
            &[layer(&data, &g, pq, 1.0)],
        );
        let (r, gch, b) = got_rgb(got[0]);
        assert!(
            r.abs_diff(want) <= 3 && gch.abs_diff(want) <= 3 && b.abs_diff(want) <= 3,
            "pq anchor v={v:.4}: got ({r},{gch},{b}) want ~{want}"
        );
    }
}

/// ST 2084 encode in f64 (independent of the production f32 path).
fn encode_pq(y_abs: f64) -> f32 {
    let m1 = 2610.0 / 16384.0;
    let m2 = 2523.0 / 4096.0 * 128.0;
    let c1 = 3424.0 / 4096.0;
    let c2 = 2413.0 / 4096.0 * 32.0;
    let c3 = 2392.0 / 4096.0 * 32.0;
    let yp = y_abs.powf(m1);
    ((c1 + c2 * yp) / (1.0 + c3 * yp)).powf(m2) as f32
}

fn pq_word(r: f32, g: f32, b: f32) -> u32 {
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5).floor() as u32;
    (q(r) << 16) | (q(g) << 8) | q(b)
}

#[test]
fn pq_dim_content_scales_proportionally() {
    // 20-nit gray PQ content: relative to 203-nit white it is ~0.0985
    // linear; sRGB-encoded it lands near 0.34 — clearly below white and
    // above black, matching the independent model within tolerance.
    let pq = ColorDescription::pq_hdr();
    let v20 = encode_pq(20.0 / 10_000.0);
    let (data, g) = one_pixel(pq_word(v20, v20, v20), FourCC::XRGB8888);
    let got = render_to_buffer(
        &out(ColorDescription::srgb_sdr()),
        &full_damage(&out(ColorDescription::srgb_sdr())),
        &[layer(&data, &g, pq, 1.0)],
    );
    // Model: (20/203) linear → sRGB encode → 255.
    let want = (srgb_encode(20.0 / 203.0) * 255.0).round() as u32;
    let (r, gch, b) = got_rgb(got[0]);
    for c in [r, gch, b] {
        assert!(
            c.abs_diff(want) <= 4,
            "20-nit gray: {r},{gch},{b} want {want}"
        );
    }
}

#[test]
fn linear_blending_of_cross_description_layers() {
    // A studio-range sRGB layer at half opacity over an opaque full-range
    // sRGB base: both decode to linear, blend premultiplied-linear, then
    // encode. The independent f64 model pins the result.
    let base_color = ColorDescription::srgb_sdr();
    let mut top_color = ColorDescription::srgb_sdr();
    top_color.range = ColorRange::Studio;

    // Base: encoded 200/255 gray. Top: studio-encoded gray 128 (expanded
    // (128-16)/219 ≈ 0.5114), opacity 0.5.
    let (base, bg) = one_pixel(0x00_C8_C8_C8, FourCC::XRGB8888);
    let (top, tg) = one_pixel(0x00_80_80_80, FourCC::XRGB8888);

    let output = out(base_color);
    let layers = [
        layer(&base, &bg, base_color, 1.0),
        layer(&top, &tg, top_color, 0.5),
    ];
    let got = render_to_buffer(&output, &full_damage(&output), &layers);

    // Model (f64): base linear = srgb_decode(200/255). Top linear =
    // srgb_decode((128*? no: studio expand (128-16)/219)) then ×0.5 premul.
    let base_lin = srgb_decode(200.0 / 255.0);
    let top_lin = srgb_decode((128.0 - 16.0) / 219.0);
    let a = 0.5f64;
    let out_lin = a * top_lin + (1.0 - a) * base_lin; // both opaque: premul == straight
    let want = (srgb_encode(out_lin) * 255.0).round() as u32;
    let (r, gch, b) = got_rgb(got[0]);
    for c in [r, gch, b] {
        assert!(
            c.abs_diff(want) <= 3,
            "linear blend: ({r},{gch},{b}) want {want}"
        );
    }
}

#[test]
fn transfer_function_mismatch_takes_the_linear_path() {
    // Gamma-2.2 content onto an sRGB output: a mid gray must come out
    // *different* from the identity render (the curves differ), and close
    // to the independent model.
    let mut gamma = ColorDescription::srgb_sdr();
    gamma.transfer = TransferFunction::Gamma22;
    let (data, g) = one_pixel(0x00_80_80_80, FourCC::XRGB8888);
    let same = render_to_buffer(
        &out(gamma),
        &full_damage(&out(gamma)),
        &[layer(&data, &g, gamma, 1.0)],
    );
    let cross = render_to_buffer(
        &out(ColorDescription::srgb_sdr()),
        &full_damage(&out(ColorDescription::srgb_sdr())),
        &[layer(&data, &g, gamma, 1.0)],
    );
    assert_ne!(got_rgb(same[0]), got_rgb(cross[0]));
    // Model: decode 128/255 with gamma 2.2, re-encode with sRGB.
    let lin = (128.0 / 255.0f64).powf(2.2);
    let want = (srgb_encode(lin) * 255.0).round() as u32;
    let (r, gch, b) = got_rgb(cross[0]);
    for c in [r, gch, b] {
        assert!(
            c.abs_diff(want) <= 3,
            "gamma->srgb: ({r},{gch},{b}) want {want}"
        );
    }
}

#[test]
fn pipeline_respects_damage_clipping() {
    // A cross-description layer only paints where damage says so.
    let mut studio = ColorDescription::srgb_sdr();
    studio.range = ColorRange::Studio;
    let (data, g) = one_pixel(0x00_FF_FF_FF, FourCC::XRGB8888);
    let output = OutputDesc::new(2, 1, FourCC::XRGB8888, ColorDescription::srgb_sdr()).unwrap();
    let damage = Region::from_rect(Rect::new(1, 0, 1, 1));
    // Stretch the 1x1 buffer across the 2x1 output (nearest sampling).
    let view = BufferView::new(1, &data, g.clone()).unwrap();
    let stretched = SurfaceLayer::new(
        view,
        Rect::new(0, 0, 2, 1),
        Transform::Normal,
        studio,
        1.0,
        Region::new(),
    );
    let got = render_to_buffer(&output, &damage, &[stretched]);
    // Pixel 0 untouched (initial opaque black), pixel 1 painted white-ish.
    assert_eq!(got_rgb(got[0]), (0, 0, 0));
    assert!(got_rgb(got[1]).0 >= 253, "studio white expands to ~255");
}

#[test]
fn opacity_in_the_pipeline_path_blend_linearly() {
    // PQ layer at 25% opacity over an opaque sRGB base: linear model.
    let pq = ColorDescription::pq_hdr();
    let srgb = ColorDescription::srgb_sdr();
    let v = encode_pq(0.5); // 5000-nit content, far above the anchor
    let (top, tg) = one_pixel(pq_word(v, v, v), FourCC::XRGB8888);
    let (base, bg) = one_pixel(0x00_40_40_40, FourCC::XRGB8888);
    let output = out(srgb);
    let layers = [layer(&base, &bg, srgb, 1.0), layer(&top, &tg, pq, 0.25)];
    let got = render_to_buffer(&output, &full_damage(&output), &layers);
    // Model: PQ decode(0.5 encoded) — use the f64 inverse; then anchor
    // (divide by 0.0203) and clamp to 1.0 (5000 nits is above SDR white
    // after the anchor), blend 25% over the sRGB-decoded base.
    let base_lin = srgb_decode(64.0 / 255.0);
    let out_lin = 0.25 * 1.0 + 0.75 * base_lin;
    let want = (srgb_encode(out_lin) * 255.0).round() as u32;
    let (r, gch, b) = got_rgb(got[0]);
    for c in [r, gch, b] {
        assert!(
            c.abs_diff(want) <= 4,
            "pq opacity: ({r},{gch},{b}) want {want}"
        );
    }
}

// ---- the luminance tail (Phase 38) ---------------------------------
//
// The negotiated panel peak reaches the ink: a PQ layer on the PQ
// canvas carries its tone policy, and the decode path applies the
// luminance scale before the encode. The models below are independent
// f64 math (the same doctrine as the anchors above).

fn toned_layer<'a>(
    data: &'a [u8],
    geometry: &BufferGeometry,
    color: ColorDescription,
    tone: ldp_renderer::TonePolicy,
) -> SurfaceLayer<'a> {
    let mut l = layer(data, geometry, color, 1.0);
    l.tone = tone;
    l
}

#[test]
fn the_clip_tail_caps_bright_content_at_the_ceiling() {
    // The honest default (no metadata): content above the negotiated
    // ceiling maps down to exactly the ceiling; content below rides
    // untouched.
    let pq = ColorDescription::pq_hdr();
    let output = out(pq);
    // 4000-nit gray — far above a 600-nit panel.
    let v4000 = encode_pq(4000.0 / 10_000.0);
    let (data, g) = one_pixel(pq_word(v4000, v4000, v4000), FourCC::XRGB8888);
    let got = render_to_buffer(
        &output,
        &full_damage(&output),
        &[toned_layer(
            &data,
            &g,
            pq,
            ldp_renderer::TonePolicy::Clip {
                ceiling_nits: 600.0,
            },
        )],
    );
    // Model: the clip maps 4000 → 600 nits; the canvas encodes PQ of
    // 600/10000.
    let want = (encode_pq(600.0 / 10_000.0) * 255.0 + 0.5).floor() as u32;
    let (r, gch, b) = got_rgb(got[0]);
    for c in [r, gch, b] {
        assert!(
            c.abs_diff(want) <= 2,
            "clip at the ceiling: ({r},{gch},{b}) want ~{want}"
        );
    }
    // 300-nit gray — below the ceiling: the code passes (a decode/
    // encode round trip of the same code value, within tolerance).
    let v300 = encode_pq(300.0 / 10_000.0);
    let (data, g) = one_pixel(pq_word(v300, v300, v300), FourCC::XRGB8888);
    let got = render_to_buffer(
        &output,
        &full_damage(&output),
        &[toned_layer(
            &data,
            &g,
            pq,
            ldp_renderer::TonePolicy::Clip {
                ceiling_nits: 600.0,
            },
        )],
    );
    let want = (v300 * 255.0 + 0.5).floor() as u32;
    let (r, gch, b) = got_rgb(got[0]);
    for c in [r, gch, b] {
        assert!(
            c.abs_diff(want) <= 2,
            "below the ceiling rides untouched: ({r},{gch},{b}) want ~{want}"
        );
    }
}

#[test]
fn the_eetf_tail_rolls_off_through_the_knee() {
    // The metadata refinement: content declared mastered at
    // 0.005..4000 nits onto a 600-nit ceiling. Content at the master
    // peak saturates at the display peak; content in the rolloff lands
    // strictly between the identity and the clip; the map is monotone.
    let pq = ColorDescription::pq_hdr();
    let output = out(pq);
    let tone = ldp_renderer::TonePolicy::Eetf {
        master_min_nits: 5.0,
        master_max_nits: 4000.0,
        ceiling_nits: 600.0,
    };
    // (a) The master peak (4000 nits) saturates at the display peak.
    let v4000 = encode_pq(4000.0 / 10_000.0);
    let (data, g) = one_pixel(pq_word(v4000, v4000, v4000), FourCC::XRGB8888);
    let got = render_to_buffer(
        &output,
        &full_damage(&output),
        &[toned_layer(&data, &g, pq, tone)],
    );
    let peak_code = (encode_pq(600.0 / 10_000.0) * 255.0 + 0.5).floor() as u32;
    let (r, _, _) = got_rgb(got[0]);
    assert!(
        r.abs_diff(peak_code) <= 2,
        "master peak saturates at the display peak: {r} want ~{peak_code}"
    );
    // (b) A sweep is monotone in the code domain: brighter content in,
    // brighter (or equal) code out — the knee never inverts.
    let mut prev: Option<u32> = None;
    for nits in [20.0f64, 100.0, 300.0, 600.0, 1000.0, 2000.0, 4000.0] {
        let v = encode_pq(nits / 10_000.0);
        let (data, g) = one_pixel(pq_word(v, v, v), FourCC::XRGB8888);
        let got = render_to_buffer(
            &output,
            &full_damage(&output),
            &[toned_layer(&data, &g, pq, tone)],
        );
        let (r, _, _) = got_rgb(got[0]);
        if let Some(p) = prev {
            assert!(r >= p, "the knee is monotone at {nits} nits: {r} < {p}");
        }
        prev = Some(r);
    }
    // (c) The reference behavior, honestly pinned: the EETF maps the
    // whole mastering range onto the display range, so mid content
    // (1000 nits) compresses BELOW the hard clip's 600 (the BT.2390
    // doctrine — the peak fits by compressing the span, not just
    // capping it), while content below the knee (100 nits, under the
    // ~108-nit knee this pair implies) passes untouched: the C¹
    // identity segment.
    let v1000 = encode_pq(1000.0 / 10_000.0);
    let (data, g) = one_pixel(pq_word(v1000, v1000, v1000), FourCC::XRGB8888);
    let rolled = render_to_buffer(
        &output,
        &full_damage(&output),
        &[toned_layer(&data, &g, pq, tone)],
    );
    let clipped = render_to_buffer(
        &output,
        &full_damage(&output),
        &[toned_layer(
            &data,
            &g,
            pq,
            ldp_renderer::TonePolicy::Clip {
                ceiling_nits: 600.0,
            },
        )],
    );
    let untouched = render_to_buffer(&output, &full_damage(&output), &[layer(&data, &g, pq, 1.0)]);
    let (r_roll, _, _) = got_rgb(rolled[0]);
    let (r_clip, _, _) = got_rgb(clipped[0]);
    let (r_raw, _, _) = got_rgb(untouched[0]);
    assert!(
        r_roll < r_clip && r_clip < r_raw,
        "the knee compresses the span: raw {r_raw} > clipped {r_clip} > rolled {r_roll}"
    );
    // Below the knee: the identity segment — the code passes byte for
    // byte what the untouched render carries (within the decode/encode
    // round-trip tolerance).
    let v100 = encode_pq(100.0 / 10_000.0);
    let (data, g) = one_pixel(pq_word(v100, v100, v100), FourCC::XRGB8888);
    let rolled = render_to_buffer(
        &output,
        &full_damage(&output),
        &[toned_layer(&data, &g, pq, tone)],
    );
    let untouched = render_to_buffer(&output, &full_damage(&output), &[layer(&data, &g, pq, 1.0)]);
    let (r_roll, _, _) = got_rgb(rolled[0]);
    let (r_raw, _, _) = got_rgb(untouched[0]);
    assert!(
        r_roll.abs_diff(r_raw) <= 2,
        "below the knee is the identity segment: rolled {r_roll} ~= raw {r_raw}"
    );
}

#[test]
fn a_pass_policy_keeps_the_identity_bytes() {
    // The default policy is the pre-Phase-38 behavior byte for byte:
    // a same-description layer keeps the encoded-space fast path.
    let pq = ColorDescription::pq_hdr();
    let output = out(pq);
    let v = encode_pq(4000.0 / 10_000.0);
    let (data, g) = one_pixel(pq_word(v, v, v), FourCC::XRGB8888);
    let plain = render_to_buffer(&output, &full_damage(&output), &[layer(&data, &g, pq, 1.0)]);
    let pass = render_to_buffer(
        &output,
        &full_damage(&output),
        &[toned_layer(&data, &g, pq, ldp_renderer::TonePolicy::Pass)],
    );
    assert_eq!(plain, pass, "Pass is the pre-Phase-38 byte stream");
    // And the tail disqualifies the identity fast path: the same
    // layer with a clip policy renders *different* bytes.
    let clipped = render_to_buffer(
        &output,
        &full_damage(&output),
        &[toned_layer(
            &data,
            &g,
            pq,
            ldp_renderer::TonePolicy::Clip {
                ceiling_nits: 600.0,
            },
        )],
    );
    assert_ne!(plain, clipped, "the tail must not be bypassed");
}

#[test]
fn the_gl_path_refuses_a_tail_bearing_layer_typed() {
    // The honest boundary: the GL v1 stream composites Pass policies
    // only — a negotiated ceiling never silently drops on the floor.
    use ldp_renderer::gles::RefGles;
    use ldp_renderer::Renderer;
    let pq = ColorDescription::pq_hdr();
    let output = out(pq);
    let v = encode_pq(4000.0 / 10_000.0);
    let (data, g) = one_pixel(pq_word(v, v, v), FourCC::XRGB8888);
    let mut gl = ldp_renderer::GlesRenderer::new(Box::new(RefGles::new()));
    gl.begin_frame(&output, &full_damage(&output)).unwrap();
    let err = gl
        .submit(&[toned_layer(
            &data,
            &g,
            pq,
            ldp_renderer::TonePolicy::Clip {
                ceiling_nits: 600.0,
            },
        )])
        .expect_err("the GL v1 path must refuse the tail");
    assert!(
        matches!(err, ldp_renderer::RendererError::UnsupportedLayer { .. }),
        "the refusal is typed: {err:?}"
    );
}
