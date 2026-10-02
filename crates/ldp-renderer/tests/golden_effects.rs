//! Golden Liquid conformance (Phase 27): the corner coverage, the
//! shadow material, and the frosted backdrop — hand-computed, the same
//! discipline as `golden_blend.rs` (expectations written out in this
//! file from the integer rules, not trusted from the implementation).
//!
//! The oracle constants derive from the coverage geometry: for a
//! radius-2 corner of a rect at the origin, pixel (0,0)'s center sits
//! `sqrt(4.5)` from the arc center, so coverage is
//! `0.5 - (sqrt(4.5) - 2) = 0.37868` → 97; pixel (0,1) sits
//! `sqrt(2.5)` away → `0.5 - (sqrt(2.5) - 2) = 0.91886` → 234.

use ldp_core::buffer::{BufferGeometry, FourCC, Modifier, PlaneLayout};
use ldp_core::color::ColorDescription;
use ldp_core::geometry::{Rect, Region, Transform};
use ldp_renderer::testkit::full_damage;
use ldp_renderer::{
    BackdropParams, BufferView, EdgeLightParams, LayerStyle, OutputDesc, Renderer, ShadowParams,
    SoftwareRenderer, SurfaceLayer,
};

/// Render with an explicit background clear (the testkit's
/// `render_to_buffer` renders over the framebuffer's initial black).
fn render_with_clear(
    output: &OutputDesc,
    layers: &[SurfaceLayer<'_>],
    bg: (u8, u8, u8, u8),
) -> Vec<u32> {
    let mut renderer = SoftwareRenderer::new();
    renderer.begin_frame(output, &full_damage(output)).unwrap();
    renderer.clear_damage(bg.0, bg.1, bg.2, bg.3).unwrap();
    renderer.submit(layers).unwrap();
    renderer.end_frame().unwrap();
    renderer.readout().to_vec()
}

/// The one blending rounding rule: `v * a / 255`, round half up.
fn mul255(v: u32, a: u32) -> u32 {
    (v * a + 127) / 255
}

/// Premultiplied over of src (premul) onto an opaque dst, integer math.
fn over_opaque(src: [u8; 4], dst: [u8; 4]) -> [u8; 4] {
    let a2 = u32::from(src[3]);
    let inv = 255 - a2;
    let mix = |s: u32, d: u32| (mul255(s, 255) + mul255(d, inv)).min(255);
    [
        mix(u32::from(src[0]), u32::from(dst[0])) as u8,
        mix(u32::from(src[1]), u32::from(dst[1])) as u8,
        mix(u32::from(src[2]), u32::from(dst[2])) as u8,
        255,
    ]
}

/// A w x h opaque XRGB buffer of one color.
fn solid(w: u32, h: u32, rgb: [u8; 3]) -> (Vec<u8>, BufferGeometry) {
    let layout = [PlaneLayout {
        offset: 0,
        stride: w * 4,
    }];
    let geom = BufferGeometry::new(
        w,
        h,
        FourCC::XRGB8888,
        Modifier::LINEAR,
        &layout,
        u64::from(w * h * 4),
    )
    .unwrap();
    let mut data = Vec::with_capacity((w * h * 4) as usize);
    for _ in 0..w * h {
        data.extend_from_slice(&[rgb[2], rgb[1], rgb[0], 0x00]);
    }
    (data, geom)
}

/// A w x h ARGB premultiplied buffer of one color.
fn solid_premul(w: u32, h: u32, px: [u8; 4]) -> (Vec<u8>, BufferGeometry) {
    let layout = [PlaneLayout {
        offset: 0,
        stride: w * 4,
    }];
    let geom = BufferGeometry::new(
        w,
        h,
        FourCC::ARGB8888,
        Modifier::LINEAR,
        &layout,
        u64::from(w * h * 4),
    )
    .unwrap();
    let mut data = Vec::with_capacity((w * h * 4) as usize);
    for _ in 0..w * h {
        // Memory B, G, R, A (premul).
        data.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
    }
    (data, geom)
}

/// One XRGB layer over the output.
fn xrgb_layer<'a>(data: &'a [u8], geometry: &BufferGeometry, dest: Rect) -> SurfaceLayer<'a> {
    let view = BufferView::new(1, data, geometry.clone()).unwrap();
    SurfaceLayer::new(
        view,
        dest,
        Transform::Normal,
        ColorDescription::srgb_sdr(),
        1.0,
        Region::from_rect(Rect::new(0, 0, geometry.width(), geometry.height())),
    )
}

fn unpack(v: u32) -> [u8; 4] {
    [
        ((v >> 16) & 0xFF) as u8,
        ((v >> 8) & 0xFF) as u8,
        (v & 0xFF) as u8,
        ((v >> 24) & 0xFF) as u8,
    ]
}

/// THE corner oracle: a radius-2 rounded white window over black.
#[test]
fn rounded_corner_pixels_are_the_coverage_oracle() {
    // 6x6 white window at (2,2), radius 2. The corner (2,2) carries
    // coverage 97: white premul scaled = [97,97,97,97] over black
    // [0,0,0,255] -> r = mul255(97,255) + mul255(0, 158) = 97.
    let (data, geom) = solid(6, 6, [255, 255, 255]);
    let mut layer = xrgb_layer(&data, &geom, Rect::new(2, 2, 6, 6));
    layer.style = LayerStyle {
        corner_radius: 2,
        ..LayerStyle::default()
    };
    let output = OutputDesc::new(10, 10, FourCC::XRGB8888, ColorDescription::srgb_sdr()).unwrap();
    let fb = render_with_clear(&output, &[layer], (0, 0, 0, 0xFF));

    let corner = unpack(fb[2 * 10 + 2]);
    assert_eq!(corner, [97, 97, 97, 255], "the (0,0) corner pixel");
    let next = unpack(fb[2 * 10 + 3]);
    // Pixel (1,0) of the window: coverage 234 (the hand oracle).
    assert_eq!(next, [234, 234, 234, 255], "the (1,0) edge pixel");
    // The interior is untouched by the corner machinery.
    let interior = unpack(fb[5 * 10 + 5]);
    assert_eq!(interior, [255, 255, 255, 255]);
    // Outside the destination the background survives.
    let outside = unpack(fb[0]);
    assert_eq!(outside, [0, 0, 0, 255]);
    // Symmetry: the four corners agree.
    let c00 = fb[2 * 10 + 2];
    let c07 = fb[2 * 10 + 7];
    let c70 = fb[7 * 10 + 2];
    let c77 = fb[7 * 10 + 7];
    assert_eq!(c00, c07, "top corners agree");
    assert_eq!(c00, c70, "left corners agree");
    assert_eq!(c00, c77, "all four agree");
}

/// Radius 0 styles are the plain path: byte-identical to Phase 26.
#[test]
fn plain_style_is_byte_identical() {
    let (data, geom) = solid(4, 4, [200, 100, 50]);
    let output = OutputDesc::new(8, 8, FourCC::XRGB8888, ColorDescription::srgb_sdr()).unwrap();
    let plain = xrgb_layer(&data, &geom, Rect::new(1, 1, 4, 4));
    let mut styled = xrgb_layer(&data, &geom, Rect::new(1, 1, 4, 4));
    styled.style = LayerStyle::default();
    let a = render_with_clear(&output, &[plain], (0, 0, 0, 0xFF));
    let b = render_with_clear(&output, &[styled], (0, 0, 0, 0xFF));
    assert_eq!(a, b, "the default style renders the plain bytes");
}

/// THE shadow oracle: a hard shadow strip beside the window.
#[test]
fn hard_shadow_strip_is_the_oracle() {
    // White 4x4 window at (2,2) with a hard (blur 0) black shadow at
    // alpha 128, offset (2, 0): the two-pixel strip right of the
    // window carries over([0,0,0,128], [255,255,255,255]) =
    // r = 0 + mul255(255,127) = 127... wait — the shadow's premul
    // color is [0,0,0] with alpha 128, so r = mul255(0,255) +
    // mul255(255, 127) = 127.
    let (data, geom) = solid(4, 4, [255, 255, 255]);
    let mut layer = xrgb_layer(&data, &geom, Rect::new(2, 2, 4, 4));
    layer.style = LayerStyle {
        shadow: Some(ShadowParams {
            radius: 0,
            blur: 0,
            passes: 1,
            color: [0, 0, 0],
            alpha: 128,
            offset: (2, 0),
        }),
        ..LayerStyle::default()
    };
    let output = OutputDesc::new(12, 8, FourCC::XRGB8888, ColorDescription::srgb_sdr()).unwrap();
    let fb = render_with_clear(&output, &[layer], (40, 40, 40, 0xFF));
    // The strip: x in [6, 8), y in [2, 6) — inside the silhouette
    // (offset +2), outside the ink (no hole), coverage 255.
    let strip = unpack(fb[3 * 12 + 6]);
    let expected = over_opaque([0, 0, 0, 128], [40, 40, 40, 255]);
    assert_eq!(strip, expected, "the hard shadow strip");
    // The strip's alpha fold: r = 0 + mul255(40,127) = 20.
    assert_eq!(strip[0], 20, "hand value: mul255(40,127) = 20");
    // The window itself stays opaque white (the ink hole keeps its
    // own shadow out).
    let window = unpack(fb[3 * 12 + 4]);
    assert_eq!(window, [255, 255, 255, 255]);
    // Beyond the strip: background again.
    let beyond = unpack(fb[3 * 12 + 8]);
    assert_eq!(beyond, [40, 40, 40, 255]);
}

/// The blurred shadow fades monotonically away from the window.
#[test]
fn blurred_shadow_fades_monotonically() {
    let (data, geom) = solid(4, 4, [255, 255, 255]);
    let mut layer = xrgb_layer(&data, &geom, Rect::new(4, 4, 4, 4));
    layer.style = LayerStyle {
        corner_radius: 0,
        shadow: Some(ShadowParams {
            radius: 0,
            blur: 2,
            passes: 3,
            color: [0, 0, 0],
            alpha: 255,
            offset: (0, 0),
        }),
        ..LayerStyle::default()
    };
    let output = OutputDesc::new(16, 12, FourCC::XRGB8888, ColorDescription::srgb_sdr()).unwrap();
    let fb = render_with_clear(&output, &[layer], (200, 200, 200, 0xFF));
    // March right from the window edge along the middle row: the
    // shadow's contribution may only weaken.
    let row = 6;
    let mut last_dark: i32 = 255;
    for x in 8..16 {
        let px = unpack(fb[row * 16 + x]);
        let dark = 255 - i32::from(px[0]);
        assert!(
            dark <= last_dark,
            "shadow strengthened at x={x} ({dark} > {last_dark})"
        );
        last_dark = dark;
    }
    // And far away there is no shadow at all.
    let far = unpack(fb[row * 16 + 15]);
    assert_eq!(far, [200, 200, 200, 255]);
}

/// THE frost oracle: the tint veil over a known backdrop, then the
/// translucent ink over the material.
#[test]
fn frost_material_is_the_veil_then_ink() {
    // Backdrop: opaque red left half, opaque blue right half. Window:
    // 4x2 at (0,0) spanning both halves, frosted with a pure white
    // 50% veil (no blur, no desaturation), ink = 50% white premul.
    let output = OutputDesc::new(4, 2, FourCC::ARGB8888, ColorDescription::srgb_sdr()).unwrap();
    let (red_data, red_geom) = solid(2, 2, [255, 0, 0]);
    let red = xrgb_layer(&red_data, &red_geom, Rect::new(0, 0, 2, 2));
    let (blue_data, blue_geom) = solid(2, 2, [0, 0, 255]);
    let blue = xrgb_layer(&blue_data, &blue_geom, Rect::new(2, 0, 2, 2));
    let (ink_data, ink_geom) = solid_premul(4, 2, [128, 128, 128, 128]);
    let mut window = xrgb_layer(&ink_data, &ink_geom, Rect::new(0, 0, 4, 2));
    window.style = LayerStyle {
        backdrop: Some(BackdropParams {
            blur: 0,
            passes: 0,
            saturation: 255,
            tint: [255, 255, 255],
            tint_alpha: 128,
        }),
        ..LayerStyle::default()
    };
    let fb = render_with_clear(&output, &[red, blue, window], (0, 0, 0, 0xFF));
    // Left pixel: material = veil over red = [255,128,128,255]; ink =
    // 50% white over that: r = 128 + mul255(255,127) = 255,
    // g = b = 128 + mul255(128,127) = 64+128 = ... mul255(128,127) =
    // (16256+127)/255 = 64; so g = 128 + 64 = 192.
    let left = unpack(fb[0]);
    assert_eq!(left[0], 255, "red through veil and ink");
    assert_eq!(left[1], 192, "green: 128 + mul255(128,127) = 192");
    assert_eq!(left[2], 192, "blue: 128 + mul255(128,127) = 192");
    assert_eq!(left[3], 255, "opaque");
    // Right pixel (x=2, the blue half): material = veil over blue =
    // [128,128,255,255]; ink over it.
    let right = unpack(fb[2]);
    assert_eq!(right[0], 192, "red: 128 + mul255(128,127) = 192");
    assert_eq!(right[1], 192);
    assert_eq!(right[2], 255, "blue through veil and ink");
}

/// Frost is damage-clipped: undamaged pixels keep their old content.
#[test]
fn frost_respects_the_damage_clip_contract() {
    let output = OutputDesc::new(8, 4, FourCC::XRGB8888, ColorDescription::srgb_sdr()).unwrap();
    let (bg_data, bg_geom) = solid(8, 4, [255, 0, 0]);
    let bg = xrgb_layer(&bg_data, &bg_geom, Rect::new(0, 0, 8, 4));
    let (ink_data, ink_geom) = solid_premul(8, 2, [64, 64, 64, 64]);
    let mut window = xrgb_layer(&ink_data, &ink_geom, Rect::new(0, 0, 8, 2));
    window.style = LayerStyle {
        backdrop: Some(BackdropParams {
            blur: 0,
            passes: 0,
            saturation: 255,
            tint: [0, 0, 0],
            tint_alpha: 255,
        }),
        ..LayerStyle::default()
    };
    // Damage only the right half: the left half must stay red.
    let damage = Region::from_rect(Rect::new(4, 0, 4, 4));
    let mut renderer = SoftwareRenderer::new();
    renderer.begin_frame(&output, &damage).unwrap();
    renderer.clear_damage(255, 0, 0, 0xFF).unwrap();
    renderer.submit(&[bg, window]).unwrap();
    renderer.end_frame().unwrap();
    let fb = renderer.readout();
    let left = unpack(fb[0]);
    assert_eq!(
        left,
        [0, 0, 0, 255],
        "undamaged pixels keep the framebuffer's initial content"
    );
    // The damaged right half went through the full material (black
    // veil) then the 25% ink: r = mul255(64,255) + mul255(0,191) = 64.
    let right = unpack(fb[5]);
    assert_eq!(right, [64, 64, 64, 255]);
}

/// Styled stats: the effect pixels are counted, plain frames stay 0.
#[test]
fn stats_count_effect_pixels() {
    let output = OutputDesc::new(10, 10, FourCC::XRGB8888, ColorDescription::srgb_sdr()).unwrap();
    let (data, geom) = solid(6, 6, [255, 255, 255]);
    let mut layer = xrgb_layer(&data, &geom, Rect::new(2, 2, 6, 6));
    layer.style = LayerStyle {
        corner_radius: 2,
        ..LayerStyle::default()
    };
    let mut renderer = SoftwareRenderer::new();
    renderer
        .begin_frame(&output, &full_damage(&output))
        .unwrap();
    renderer.clear_damage(0, 0, 0, 0xFF).unwrap();
    let stats = renderer.submit(&[layer]).unwrap();
    renderer.end_frame().unwrap();
    assert_eq!(stats.pixels_effect, 36, "the whole destination counts");
    assert_eq!(stats.pixels_opaque + stats.pixels_blended, 36);
    // Plain frame: zero effect pixels.
    let (data2, geom2) = solid(6, 6, [1, 2, 3]);
    let plain = xrgb_layer(&data2, &geom2, Rect::new(2, 2, 6, 6));
    let mut renderer = SoftwareRenderer::new();
    renderer
        .begin_frame(&output, &full_damage(&output))
        .unwrap();
    let stats = renderer.submit(&[plain]).unwrap();
    renderer.end_frame().unwrap();
    assert_eq!(stats.pixels_effect, 0, "plain frames carry no effect cost");
}

/// The oversize radius clamps to the stadium, never panics.
#[test]
fn oversize_radius_clamps_to_a_stadium() {
    let (data, geom) = solid(4, 2, [10, 20, 30]);
    let mut layer = xrgb_layer(&data, &geom, Rect::new(0, 0, 4, 2));
    layer.style = LayerStyle {
        corner_radius: 99,
        ..LayerStyle::default()
    };
    let output = OutputDesc::new(4, 2, FourCC::XRGB8888, ColorDescription::srgb_sdr()).unwrap();
    let fb = render_with_clear(&output, &[layer], (0, 0, 0, 0xFF));
    // A stadium of a 4x2: only the middle columns stay fully covered;
    // the extreme corners fall outside the rounded ink.
    let corner = unpack(fb[0]);
    assert!(
        corner[0] < 10,
        "the stadium clips the corner (got {})",
        corner[0]
    );
    let middle = unpack(fb[5]);
    assert_eq!(middle, [10, 20, 30, 255]);
}

/// Repeated frames keep the persistent framebuffer honest: the frost
/// of frame two samples frame one's *result* beneath the layer.
#[test]
fn styled_frames_compose_across_renders() {
    let output = OutputDesc::new(6, 4, FourCC::XRGB8888, ColorDescription::srgb_sdr()).unwrap();
    let (bg_data, bg_geom) = solid(6, 4, [0, 128, 255]);
    let (ink_data, ink_geom) = solid_premul(4, 2, [255, 255, 255, 255]);
    let mut renderer = SoftwareRenderer::new();
    for _ in 0..3 {
        let bg = xrgb_layer(&bg_data, &bg_geom, Rect::new(0, 0, 6, 4));
        let mut window = xrgb_layer(&ink_data, &ink_geom, Rect::new(1, 1, 4, 2));
        window.style = LayerStyle {
            corner_radius: 0,
            backdrop: Some(BackdropParams {
                blur: 0,
                passes: 0,
                saturation: 255,
                tint: [255, 255, 255],
                tint_alpha: 0,
            }),
            ..LayerStyle::default()
        };
        renderer
            .begin_frame(&output, &full_damage(&output))
            .unwrap();
        renderer.clear_damage(0, 0, 0, 0xFF).unwrap();
        renderer.submit(&[bg, window]).unwrap();
        renderer.end_frame().unwrap();
    }
    let fb = renderer.readout();
    // The window area: material (no veil, no blur) over the backdrop,
    // then the opaque ink: the ink wins exactly.
    let ink = unpack(fb[2 * 6 + 3]);
    assert_eq!(ink, [255, 255, 255, 255]);
    // Outside the window the backdrop survives the three rounds.
    let outside = unpack(fb[0]);
    assert_eq!(outside, [0, 128, 255, 255]);
}

// ---- Phase 40: the material depth ---------------------------------

/// THE vibrant boost oracle: the saturation dial extended *past* the
/// backdrop's own chroma. Expectations written out by hand from the
/// integer rules, the file's discipline — luma
/// `(77R + 150G + 29B + 128) >> 8`, each channel extended by
/// `boost/255` of its luma-distance rounded half away from zero.
///
/// Backdrop cherry `(200, 60, 60)`: luma = `(15400 + 9000 + 1740 +
/// 128) >> 8` = 102. At saturation 383 (boost 128):
/// R: delta 98, v = 98·128 = 12544, adj = (12544+127)/255 = 49,
/// R' = 249; G and B: delta −42, v = −5376, adj = −((5376+127)/255)
/// = −21, so 39. The boost moves channels *away* from luma — the
/// desaturation arm's mirror (below, the same backdrop at saturation
/// 128 lands towards luma: R = (200·128 + 102·127 + 127)/255 = 151,
/// G = B = (60·128 + 102·127 + 127)/255 = 81).
#[test]
fn vibrant_frost_boosts_chroma_away_from_luma() {
    let output = OutputDesc::new(2, 2, FourCC::XRGB8888, ColorDescription::srgb_sdr()).unwrap();
    let (bg_data, bg_geom) = solid(2, 2, [200, 60, 60]);
    let (ink_data, ink_geom) = solid_premul(2, 2, [0, 0, 0, 0]);
    let boosted = {
        let bg = xrgb_layer(&bg_data, &bg_geom, Rect::new(0, 0, 2, 2));
        let mut window = xrgb_layer(&ink_data, &ink_geom, Rect::new(0, 0, 2, 2));
        window.style = LayerStyle {
            backdrop: Some(BackdropParams {
                blur: 0,
                passes: 0,
                saturation: 383,
                tint: [255, 255, 255],
                tint_alpha: 0,
            }),
            ..LayerStyle::default()
        };
        render_with_clear(&output, &[bg, window], (0, 0, 0, 0xFF))
    };
    let px = unpack(boosted[0]);
    assert_eq!(px, [249, 39, 39, 255], "the boost arm, hand-computed");
    // The desaturation mirror: the same backdrop at 128 moves towards
    // luma — the two arms diverge in opposite directions from identity.
    let desat = {
        let bg = xrgb_layer(&bg_data, &bg_geom, Rect::new(0, 0, 2, 2));
        let mut window = xrgb_layer(&ink_data, &ink_geom, Rect::new(0, 0, 2, 2));
        window.style = LayerStyle {
            backdrop: Some(BackdropParams {
                blur: 0,
                passes: 0,
                saturation: 128,
                tint: [255, 255, 255],
                tint_alpha: 0,
            }),
            ..LayerStyle::default()
        };
        render_with_clear(&output, &[bg, window], (0, 0, 0, 0xFF))
    };
    let px = unpack(desat[0]);
    assert_eq!(px, [151, 81, 81, 255], "the legacy arm, hand-computed");
    // The identity between them: saturation 255 keeps the backdrop.
    let identity = {
        let bg = xrgb_layer(&bg_data, &bg_geom, Rect::new(0, 0, 2, 2));
        let mut window = xrgb_layer(&ink_data, &ink_geom, Rect::new(0, 0, 2, 2));
        window.style = LayerStyle {
            backdrop: Some(BackdropParams {
                blur: 0,
                passes: 0,
                saturation: 255,
                tint: [255, 255, 255],
                tint_alpha: 0,
            }),
            ..LayerStyle::default()
        };
        render_with_clear(&output, &[bg, window], (0, 0, 0, 0xFF))
    };
    assert_eq!(unpack(identity[0]), [200, 60, 60, 255]);
}

/// THE edge-light oracle: the 1-pixel hairline traced inside a
/// radius-2 rounded window of ink `(40, 40, 40)` on an 8x6 output —
/// the ring is the silhouette's coverage minus its 1-px erosion's.
/// Every expectation below is derived from the SDF rules:
///
/// * `(0, 3)` (straight left edge): outer 255, the eroded rect
///   (1,1,6,4) r=1 scores 0 (its edge sits exactly a half-pixel out)
///   — ring 255, an opaque white stroke;
/// * `(4, 0)` (straight top edge): same — 255;
/// * `(4, 3)` (deep interior): both 255 — the ink alone;
/// * `(1, 3)` (one in from the straight edge): the eroded rect's
///   coverage is already 255 (the inner boundary lands exactly
///   between the columns) — the ink alone;
/// * `(0, 0)` (the corner): outer coverage 97 (the file's own
///   worked example — `sqrt(4.5)` from the arc center), inner 0 —
///   ring 97, white at alpha `mul255(255, 97)` = 97 over the ink
///   *through its own corner coverage* (`mul255(40, 97)` = 15 over
///   the black clear): `97 + mul255(15, 158)` = 97 + 9 = 106;
/// * `(1, 1)` (the corner's inner diagonal): outer 255, inner
///   `0.5 + (1 − sqrt(0.5))` = 0.793 → 202 — ring 53, white at 53
///   over full ink (the pixel center sits inside the outer straight
///   region): `53 + mul255(40, 202)` = 53 + 32 = 85 — the ring's
///   soft inner corner.
#[test]
fn edge_light_traces_the_inner_ring() {
    let output = OutputDesc::new(8, 6, FourCC::XRGB8888, ColorDescription::srgb_sdr()).unwrap();
    let (ink_data, ink_geom) = solid(8, 6, [40, 40, 40]);
    let mut window = xrgb_layer(&ink_data, &ink_geom, Rect::new(0, 0, 8, 6));
    window.style = LayerStyle {
        corner_radius: 2,
        edge_light: Some(EdgeLightParams {
            color: [255, 255, 255],
            alpha: 255,
        }),
        ..LayerStyle::default()
    };
    let fb = render_with_clear(&output, &[window], (0, 0, 0, 0xFF));
    let at = |x: u32, y: u32| unpack(fb[(y * 8 + x) as usize]);
    // The straight edges carry the full stroke.
    assert_eq!(at(0, 3), [255, 255, 255, 255], "left edge");
    assert_eq!(at(4, 0), [255, 255, 255, 255], "top edge");
    assert_eq!(at(7, 2), [255, 255, 255, 255], "right edge");
    assert_eq!(at(3, 5), [255, 255, 255, 255], "bottom edge");
    // The interior keeps the ink.
    assert_eq!(at(4, 3), [40, 40, 40, 255], "deep interior");
    assert_eq!(at(1, 3), [40, 40, 40, 255], "one in from the straight edge");
    // The corners: the antialiased outer arc, then the soft inner.
    assert_eq!(at(0, 0), [106, 106, 106, 255], "the corner's outer arc");
    assert_eq!(at(1, 1), [85, 85, 85, 255], "the corner's soft inner ring");
    // The identity: without the light, the same window renders the
    // Phase 27 bytes (the hairline adds and never disturbs) — the
    // plain corner is ink-through-coverage, strictly darker than the
    // ringed one.
    let plain = {
        let (ink_data, ink_geom) = solid(8, 6, [40, 40, 40]);
        let mut window = xrgb_layer(&ink_data, &ink_geom, Rect::new(0, 0, 8, 6));
        window.style = LayerStyle {
            corner_radius: 2,
            ..LayerStyle::default()
        };
        render_with_clear(&output, &[window], (0, 0, 0, 0xFF))
    };
    let plain_corner = ((plain[0] >> 16) & 0xFF) as u8;
    let ringed_corner = ((fb[0] >> 16) & 0xFF) as u8;
    assert!(
        ringed_corner > plain_corner,
        "the stroke lightens the corner"
    );
}
