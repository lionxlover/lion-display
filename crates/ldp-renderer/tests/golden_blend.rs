//! Golden blend conformance: the encoded-space premultiplied `over`
//! operator, opacity, damage clipping, and layer order — all hand-computed.
//!
//! Every expectation here is integer fixed-point math written out in this
//! file (the same `mul255` rule, re-derived), so the suite pins the blend
//! kernels byte-exactly rather than trusting them.

use ldp_core::buffer::{BufferGeometry, FourCC, Modifier, PlaneLayout};
use ldp_core::color::ColorDescription;
use ldp_core::geometry::{Rect, Region, Transform};
use ldp_renderer::testkit::{full_damage, render_to_buffer};
use ldp_renderer::{BufferView, OutputDesc, Renderer, SoftwareRenderer, SurfaceLayer};

/// The one blending rounding rule: `v * a / 255`, round half up.
fn mul255(v: u32, a: u32) -> u32 {
    (v * a + 127) / 255
}

/// Premultiplied over of src (premul) onto dst (premul), integer math.
fn over(src: [u8; 4], dst: [u8; 4], opacity_q: u32) -> [u8; 4] {
    let a2 = mul255(u32::from(src[3]), opacity_q);
    if a2 == 0 {
        return dst;
    }
    let inv = 255 - a2;
    let oa = a2 + mul255(u32::from(dst[3]), inv);
    let mix = |s: u32, d: u32| (mul255(s, opacity_q) + mul255(d, inv)).min(oa);
    [
        mix(u32::from(src[0]), u32::from(dst[0])) as u8,
        mix(u32::from(src[1]), u32::from(dst[1])) as u8,
        mix(u32::from(src[2]), u32::from(dst[2])) as u8,
        oa as u8,
    ]
}

fn pack(px: [u8; 4]) -> u32 {
    (u32::from(px[3]) << 24) | (u32::from(px[0]) << 16) | (u32::from(px[1]) << 8) | u32::from(px[2])
}

fn unpack(v: u32) -> [u8; 4] {
    [
        ((v >> 16) & 0xFF) as u8,
        ((v >> 8) & 0xFF) as u8,
        (v & 0xFF) as u8,
        ((v >> 24) & 0xFF) as u8,
    ]
}

/// A 1x1 ARGB premultiplied pixel buffer.
fn argbb_pixel(px: [u8; 4]) -> (Vec<u8>, BufferGeometry) {
    let layout = [PlaneLayout {
        offset: 0,
        stride: 4,
    }];
    let g = BufferGeometry::new(1, 1, FourCC::ARGB8888, Modifier::LINEAR, &layout, 4).unwrap();
    // Memory B, G, R, A (premul).
    (vec![px[2], px[1], px[0], px[3]], g)
}

/// A 1x1 opaque XRGB pixel buffer.
fn xrgb_pixel(r: u8, g: u8, b: u8) -> (Vec<u8>, BufferGeometry) {
    let layout = [PlaneLayout {
        offset: 0,
        stride: 4,
    }];
    let geom = BufferGeometry::new(1, 1, FourCC::XRGB8888, Modifier::LINEAR, &layout, 4).unwrap();
    (vec![b, g, r, 0x00], geom)
}

fn layer<'a>(
    data: &'a [u8],
    geometry: &BufferGeometry,
    dest: Rect,
    opacity: f32,
) -> SurfaceLayer<'a> {
    let view = BufferView::new(1, data, geometry.clone()).unwrap();
    SurfaceLayer::new(
        view,
        dest,
        Transform::Normal,
        ColorDescription::srgb_sdr(),
        opacity,
        Region::new(),
    )
}

fn out(w: u32, h: u32) -> OutputDesc {
    OutputDesc::new(w, h, FourCC::ARGB8888, ColorDescription::srgb_sdr()).unwrap()
}

#[test]
fn semi_transparent_over_opaque_is_the_hand_computed_word() {
    // src premul (r=64,g=32,b=16,a=128) over opaque dst (r=255,g=0,b=0).
    let (src, sg) = argbb_pixel([64, 32, 16, 128]);
    let (dst, dg) = xrgb_pixel(255, 0, 0);
    let output = out(1, 1);
    let layers = [
        layer(&dst, &dg, Rect::new(0, 0, 1, 1), 1.0),
        layer(&src, &sg, Rect::new(0, 0, 1, 1), 1.0),
    ];
    let got = render_to_buffer(&output, &full_damage(&output), &layers);
    // Hand math: a2 = 128; inv = 127.
    // r = 64 + mul255(255,127) = 64 + 127 = 191.
    // g = 32 + 0 = 32. b = 16 + 0 = 16. oa = 128 + 127 = 255.
    let expected = over([64, 32, 16, 128], [255, 0, 0, 255], 255);
    assert_eq!(expected, [191, 32, 16, 255]);
    assert_eq!(got[0], pack(expected));
}

#[test]
fn opacity_folds_into_the_premul_source() {
    // Opaque src (200, 100, 50) at opacity 128/255 over opaque dst (10, 20, 30).
    let (src, sg) = xrgb_pixel(200, 100, 50);
    let (dst, dg) = xrgb_pixel(10, 20, 30);
    let output = out(1, 1);
    let layers = [
        layer(&dst, &dg, Rect::new(0, 0, 1, 1), 1.0),
        layer(&src, &sg, Rect::new(0, 0, 1, 1), 128.0 / 255.0),
    ];
    let got = render_to_buffer(&output, &full_damage(&output), &layers);
    let expected = over([200, 100, 50, 255], [10, 20, 30, 255], 128);
    assert_eq!(got[0], pack(expected));
    // The independent hand value: a2 = mul255(255,128) = 128;
    // r = mul255(200,128) + mul255(10,127) = 100 + 5 = 105.
    assert_eq!(expected[0], 105);
}

#[test]
fn fully_transparent_pixels_leave_the_destination() {
    let (src, sg) = argbb_pixel([0, 0, 0, 0]); // a = 0
    let (dst, dg) = xrgb_pixel(9, 8, 7);
    let output = out(1, 1);
    let layers = [
        layer(&dst, &dg, Rect::new(0, 0, 1, 1), 1.0),
        layer(&src, &sg, Rect::new(0, 0, 1, 1), 1.0),
    ];
    let got = render_to_buffer(&output, &full_damage(&output), &layers);
    assert_eq!(unpack(got[0]), [9, 8, 7, 255]);
}

#[test]
fn layer_order_is_back_to_front() {
    // Two overlapping semi-transparent layers: swapping the order must
    // change the result (the over operator is not commutative), and both
    // orders must match the hand model.
    let (a, ag) = argbb_pixel([255, 0, 0, 96]);
    let (b, bg) = argbb_pixel([0, 0, 255, 160]);
    let output = out(1, 1);
    let base = layer(&a, &ag, Rect::new(0, 0, 1, 1), 1.0);
    let top = layer(&b, &bg, Rect::new(0, 0, 1, 1), 1.0);
    let got_ab = render_to_buffer(&output, &full_damage(&output), &[base.clone(), top.clone()]);
    let got_ba = render_to_buffer(&output, &full_damage(&output), &[top, base]);
    // Over the initial opaque black framebuffer.
    let want_ab = over(
        [0, 0, 255, 160],
        over([255, 0, 0, 96], [0, 0, 0, 255], 255),
        255,
    );
    let want_ba = over(
        [255, 0, 0, 96],
        over([0, 0, 255, 160], [0, 0, 0, 255], 255),
        255,
    );
    assert_eq!(got_ab[0], pack(want_ab));
    assert_eq!(got_ba[0], pack(want_ba));
    assert_ne!(got_ab[0], got_ba[0]);
}

#[test]
fn damage_clips_writes_exactly() {
    // Frame 1 paints the full pattern; frame 2 damages only the center
    // pixel and re-submits only the intersecting layer. Everything outside
    // the damage must hold frame 1's content.
    let (px, pg) = xrgb_pixel(10, 20, 30);
    let (px2, pg2) = xrgb_pixel(200, 100, 50);
    let output = out(3, 3);
    let mut renderer = SoftwareRenderer::new();
    renderer
        .begin_frame(&output, &full_damage(&output))
        .unwrap();
    renderer
        .submit(&[layer(&px, &pg, Rect::new(0, 0, 3, 3), 1.0)])
        .unwrap();
    renderer.end_frame().unwrap();
    let frame1 = renderer.readout().to_vec();

    let damage = Region::from_rect(Rect::new(1, 1, 1, 1));
    renderer.begin_frame(&output, &damage).unwrap();
    renderer
        .submit(&[layer(&px2, &pg2, Rect::new(0, 0, 3, 3), 1.0)])
        .unwrap();
    let stats = renderer.end_frame().unwrap().stats;
    assert_eq!(stats.pixels_damaged, 1);
    assert_eq!(stats.pixels_opaque, 1);
    // The untouched sentinel check: 8 pixels keep frame 1's word.
    let sentinel = frame1[0];
    let frame2 = renderer.readout().to_vec();

    let mut expected = frame1;
    expected[4] = pack([200, 100, 50, 255]);
    assert_eq!(frame2, expected);
    assert_eq!(frame2.iter().filter(|&&p| p == sentinel).count(), 8);
}

#[test]
fn overlapping_damage_rects_blend_once() {
    // Two overlapping damage rects covering the same pixel: the merged
    // span must composite the layer exactly once (a double blend would
    // darken a translucent layer).
    let (src, sg) = argbb_pixel([128, 128, 128, 128]);
    let output = out(3, 1);
    let mut damage = Region::new();
    damage.add(Rect::new(0, 0, 2, 1));
    damage.add(Rect::new(1, 0, 2, 1));
    let got = render_to_buffer(
        &output,
        &damage,
        &[layer(&src, &sg, Rect::new(0, 0, 3, 1), 1.0)],
    );
    // Over opaque black: one blend of a=128 gives premul 128s with a=255.
    // A second blend would give mul255(128,127)+128 = 191 per channel.
    for (i, &p) in got.iter().enumerate() {
        let want = pack(over([128, 128, 128, 128], [0, 0, 0, 255], 255));
        assert_eq!(p, want, "pixel {i} blended more than once");
    }
}

#[test]
fn stats_account_opaque_and_blended_work() {
    let (opaque, og) = xrgb_pixel(1, 2, 3);
    let (alpha, ag) = argbb_pixel([10, 20, 30, 100]);
    let output = out(4, 2);
    let mut renderer = SoftwareRenderer::new();
    renderer
        .begin_frame(&output, &full_damage(&output))
        .unwrap();
    let stats = renderer
        .submit(&[
            layer(&opaque, &og, Rect::new(0, 0, 4, 2), 1.0),
            layer(&alpha, &ag, Rect::new(1, 0, 2, 1), 1.0),
        ])
        .unwrap();
    assert_eq!(stats.layers, 2);
    assert_eq!(stats.layers_rendered, 2);
    assert_eq!(stats.pixels_opaque, 8);
    assert_eq!(stats.pixels_blended, 2);
    assert_eq!(stats.pixels_damaged, 8 + 2);
    let done = renderer.end_frame().unwrap();
    assert_eq!(done.stats, stats);
}

#[test]
fn layer_outside_damage_reports_not_rendered() {
    let (px, pg) = xrgb_pixel(5, 6, 7);
    let output = out(4, 4);
    let damage = Region::from_rect(Rect::new(0, 0, 1, 1));
    let mut renderer = SoftwareRenderer::new();
    renderer.begin_frame(&output, &damage).unwrap();
    let stats = renderer
        .submit(&[layer(&px, &pg, Rect::new(2, 2, 2, 2), 1.0)])
        .unwrap();
    assert_eq!(stats.layers, 1);
    assert_eq!(stats.layers_rendered, 0);
    assert_eq!(stats.pixels_damaged, 0);
}

#[test]
fn x_output_forces_full_alpha_on_blends() {
    // Blending onto an XRGB output: result alpha is always 255.
    let (src, sg) = argbb_pixel([40, 50, 60, 90]);
    let output = OutputDesc::new(1, 1, FourCC::XRGB8888, ColorDescription::srgb_sdr()).unwrap();
    let got = render_to_buffer(
        &output,
        &full_damage(&output),
        &[layer(&src, &sg, Rect::new(0, 0, 1, 1), 1.0)],
    );
    assert_eq!(got[0] >> 24, 0xFF);
    let px = unpack(got[0]);
    let want = over([40, 50, 60, 90], [0, 0, 0, 255], 255);
    assert_eq!(px[..3], want[..3]);
    // XRGB pack keeps premul over black: channels are the premul values.
    assert_eq!(px[0], 40);
}

#[test]
fn blend_math_is_stable_across_runs() {
    // Determinism smoke: the same frame rendered twice is bit-identical,
    // and the opacity row matches the hand model.
    let (a, ag) = argbb_pixel([77, 66, 55, 123]);
    let (b, bg) = xrgb_pixel(200, 150, 100);
    let output = out(2, 2);
    let mk = || {
        vec![
            layer(&b, &bg, Rect::new(0, 0, 2, 2), 1.0),
            layer(&a, &ag, Rect::new(0, 1, 2, 1), 0.75),
        ]
    };
    let r1 = render_to_buffer(&output, &full_damage(&output), &mk());
    let r2 = render_to_buffer(&output, &full_damage(&output), &mk());
    assert_eq!(r1, r2);
    // 0.75 quantizes to 191; the bottom row is the hand model over the
    // opaque base.
    let q = 191;
    let want = over([77, 66, 55, 123], [200, 150, 100, 255], q);
    assert_eq!(r1[2], pack(want));
    assert_eq!(r1[3], pack(want));
    // The untouched top row is the base.
    assert_eq!(unpack(r1[0]), [200, 150, 100, 255]);
}

// ---- the Phase 22 CopyPremul path -----------------------------------

/// A 1×N ARGB8888 premultiplied row buffer.
fn argb_row(pixels: &[[u8; 4]]) -> (Vec<u8>, BufferGeometry) {
    let n = pixels.len() as u32;
    let layout = [PlaneLayout {
        offset: 0,
        stride: n * 4,
    }]; // stride u32; storage u64 below
    let g = BufferGeometry::new(
        n,
        1,
        FourCC::ARGB8888,
        Modifier::LINEAR,
        &layout,
        u64::from(n * 4),
    )
    .unwrap();
    let mut data = Vec::with_capacity((n * 4) as usize);
    for px in pixels {
        // Memory B, G, R, A (premul).
        data.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
    }
    (data, g)
}

/// An all-opaque ARGB layer onto a same-format output lands *bit-identical*
/// to its source words (the fast path's equivalence to the blend), across
/// many chunk boundaries.
#[test]
fn opaque_argb_layer_lands_bit_identical_to_the_source() {
    // 200 px wide: three full 64-px chunks plus a tail.
    let n = 200usize;
    let mut pixels = Vec::with_capacity(n);
    for i in 0..n {
        let v = (i * 37 % 256) as u8;
        pixels.push([v, v.wrapping_add(1), v.wrapping_add(2), 255]);
    }
    let (src, sg) = argb_row(&pixels);
    let output = out(n as u32, 1);
    let layers = [layer(&src, &sg, Rect::new(0, 0, n as u32, 1), 1.0)];
    let got = render_to_buffer(&output, &full_damage(&output), &layers);
    for (i, word) in got.iter().enumerate() {
        assert_eq!(*word, pack(pixels[i]), "pixel {i} must be the source word");
    }
    // Every pixel took the opaque write; nothing blended.
    let mut renderer = SoftwareRenderer::new();
    renderer
        .begin_frame(&output, &full_damage(&output))
        .unwrap();
    let stats = renderer.submit(&layers).unwrap();
    renderer.end_frame().unwrap();
    assert_eq!(stats.pixels_opaque, n as u64);
    assert_eq!(stats.pixels_blended, 0);
}

/// A mixed-opacity ARGB layer: opaque pixels land as the source word and
/// translucent pixels blend exactly per the hand model — the per-chunk
/// fallback must reproduce the Alpha path's math to the byte.
#[test]
fn mixed_arggb_layer_blends_only_the_translucent_pixels() {
    // 200 px: one translucent pixel inside each 64-px chunk region.
    let n = 200usize;
    let mut pixels = Vec::with_capacity(n);
    for i in 0..n {
        let v = (i * 41 % 256) as u8;
        pixels.push([v, v.wrapping_add(3), v.wrapping_add(7), 255]);
    }
    // Translucent pixels at 10, 74, 138 (premul values with a=128).
    pixels[10] = [64, 32, 16, 128];
    pixels[74] = [128, 0, 200, 100];
    pixels[138] = [255, 255, 0, 1];
    let (src, sg) = argb_row(&pixels);
    // Opaque background.
    let (dst, dg) = argb_row(&[[200u8, 150, 100, 255]]);
    let output = out(n as u32, 1);
    let layers = [
        layer(&dst, &dg, Rect::new(0, 0, n as u32, 1), 1.0),
        layer(&src, &sg, Rect::new(0, 0, n as u32, 1), 1.0),
    ];
    let got = render_to_buffer(&output, &full_damage(&output), &layers);
    for (i, word) in got.iter().enumerate() {
        let want = if pixels[i][3] == 255 {
            pixels[i]
        } else {
            over(pixels[i], [200, 150, 100, 255], 255)
        };
        assert_eq!(*word, pack(want), "pixel {i}");
    }
    // The fully-transparent-ish pixel (a=1) leaves a visible blend too.
    assert_eq!(
        unpack(got[138]),
        over([255, 255, 0, 1], [200, 150, 100, 255], 255)
    );
}
