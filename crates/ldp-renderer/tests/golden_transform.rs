//! Golden transform conformance: all eight buffer transforms plus the
//! nearest-sample scaled mapping.
//!
//! The reference model is built **forward**: for every buffer pixel, the
//! expected image receives the pattern at `Transform::transform_point` plus
//! the destination offset (core's own forward mapping, golden-tested in
//! Phase 1) — the renderer's inverse mapping must agree with it exactly.
//! Scaled cases extend the same forward construction to whole source-pixel
//! footprints.

use ldp_core::buffer::FourCC;
use ldp_core::color::ColorDescription;
use ldp_core::geometry::{Rect, Region, Size, Transform};
use ldp_renderer::testkit::{full_damage, pattern_buffer, pattern_rgba, render_to_buffer};
use ldp_renderer::{BufferView, OutputDesc, SurfaceLayer};

const BW: u32 = 4;
const BH: u32 = 3;

fn layer_of<'a>(
    data: &'a [u8],
    geometry: &ldp_core::buffer::BufferGeometry,
    dest: Rect,
    t: Transform,
) -> SurfaceLayer<'a> {
    let view = BufferView::new(1, data, geometry.clone()).unwrap();
    SurfaceLayer::new(
        view,
        dest,
        t,
        ColorDescription::srgb_sdr(),
        1.0,
        Region::new(),
    )
}

fn pack_rgb(px: [u8; 4]) -> u32 {
    0xFF00_0000 | (u32::from(px[0]) << 16) | (u32::from(px[1]) << 8) | u32::from(px[2])
}

#[test]
fn all_eight_transforms_match_the_forward_reference() {
    let (data, geometry) = pattern_buffer(BW, BH, FourCC::RGB888);
    for t in [
        Transform::Normal,
        Transform::Rot90,
        Transform::Rot180,
        Transform::Rot270,
        Transform::Flipped,
        Transform::Flipped90,
        Transform::Flipped180,
        Transform::Flipped270,
    ] {
        let swapped = t.transform_size(Size::new(BW, BH));
        let (dw, dh) = (swapped.w, swapped.h);
        // Place at a nonzero origin so the offset math is exercised.
        let dest = Rect::new(3, 2, dw, dh);
        let out_w = dw + 6;
        let out_h = dh + 5;
        let output =
            OutputDesc::new(out_w, out_h, FourCC::XRGB8888, ColorDescription::srgb_sdr()).unwrap();
        // Forward construction: every buffer pixel lands at
        // transform_point + dest origin.
        let mut expected = vec![0xFF00_0000u32; (out_w * out_h) as usize];
        for by in 0..BH {
            for bx in 0..BW {
                let (sx, sy) = t.transform_point(bx as i32, by as i32, BW, BH);
                let (ox, oy) = (dest.x + sx, dest.y + sy);
                expected[(oy * out_w as i32 + ox) as usize] = pack_rgb(pattern_rgba(bx, by));
            }
        }
        let got = render_to_buffer(
            &output,
            &full_damage(&output),
            &[layer_of(&data, &geometry, dest, t)],
        );
        assert_eq!(got, expected, "transform {t:?}");
    }
}

#[test]
fn rot90_corners_are_hand_placed() {
    // 3x2 buffer (w=3, h=2) rotated 90°: output is 2x3.
    // Output (0,0) shows buffer (0, h-1); output (w-1, 0) shows (0, 0).
    let (data, geometry) = pattern_buffer(3, 2, FourCC::RGB888);
    let output = OutputDesc::new(2, 3, FourCC::XRGB8888, ColorDescription::srgb_sdr()).unwrap();
    let got = render_to_buffer(
        &output,
        &full_damage(&output),
        &[layer_of(
            &data,
            &geometry,
            Rect::new(0, 0, 2, 3),
            Transform::Rot90,
        )],
    );
    let px = |x: u32, y: u32| got[(y * 2 + x) as usize];
    assert_eq!(px(0, 0), pack_rgb(pattern_rgba(0, 1))); // buffer (0, h-1)
    assert_eq!(px(1, 0), pack_rgb(pattern_rgba(0, 0))); // buffer (0, 0)
    assert_eq!(px(0, 2), pack_rgb(pattern_rgba(2, 1))); // buffer (w-1, h-1)
    assert_eq!(px(1, 2), pack_rgb(pattern_rgba(2, 0))); // buffer (w-1, 0)
}

#[test]
fn flipped_mirrors_columns_only() {
    let (data, geometry) = pattern_buffer(4, 3, FourCC::RGB888);
    let output = OutputDesc::new(4, 3, FourCC::XRGB8888, ColorDescription::srgb_sdr()).unwrap();
    let got = render_to_buffer(
        &output,
        &full_damage(&output),
        &[layer_of(
            &data,
            &geometry,
            Rect::new(0, 0, 4, 3),
            Transform::Flipped,
        )],
    );
    let normal = render_to_buffer(
        &output,
        &full_damage(&output),
        &[layer_of(
            &data,
            &geometry,
            Rect::new(0, 0, 4, 3),
            Transform::Normal,
        )],
    );
    for y in 0..3u32 {
        for x in 0..4u32 {
            assert_eq!(
                got[(y * 4 + x) as usize],
                normal[(y * 4 + (3 - x)) as usize],
                "flipped({x},{y})"
            );
        }
    }
}

#[test]
fn double_rotation_is_rotation_of_rotation() {
    // Rot90 twice == Rot180; Flipped90 then Rot90... use the composition
    // identities the core defines: R90 ∘ R90 = R180, R90 ∘ R180 = R270.
    let (data, geometry) = pattern_buffer(4, 3, FourCC::RGB888);
    let output = OutputDesc::new(4, 3, FourCC::XRGB8888, ColorDescription::srgb_sdr()).unwrap();
    let direct = render_to_buffer(
        &output,
        &full_damage(&output),
        &[layer_of(
            &data,
            &geometry,
            Rect::new(0, 0, 4, 3),
            Transform::Rot180,
        )],
    );
    // Render Rot90 onto an intermediate 3x4, then rotate that buffer again.
    let mid_out = OutputDesc::new(3, 4, FourCC::XRGB8888, ColorDescription::srgb_sdr()).unwrap();
    let mid = render_to_buffer(
        &mid_out,
        &full_damage(&mid_out),
        &[layer_of(
            &data,
            &geometry,
            Rect::new(0, 0, 3, 4),
            Transform::Rot90,
        )],
    );
    // Turn the intermediate framebuffer into an XRGB buffer layer.
    let mut bytes = Vec::with_capacity(mid.len() * 4);
    for px in &mid {
        bytes.extend_from_slice(&px.to_le_bytes());
    }
    let layout = ldp_core::buffer::PlaneLayout {
        offset: 0,
        stride: 3 * 4,
    };
    let mid_geom = ldp_core::buffer::BufferGeometry::new(
        3,
        4,
        FourCC::XRGB8888,
        ldp_core::buffer::Modifier::LINEAR,
        &[layout],
        bytes.len() as u64,
    )
    .unwrap();
    let composed = render_to_buffer(
        &output,
        &full_damage(&output),
        &[layer_of(
            &bytes,
            &mid_geom,
            Rect::new(0, 0, 4, 3),
            Transform::Rot90,
        )],
    );
    assert_eq!(direct, composed);
}

#[test]
fn integer_upscale_repeats_source_pixels() {
    // 2x1 buffer scaled to 4x2: each source pixel covers a 2x2 block.
    let (data, geometry) = pattern_buffer(2, 1, FourCC::RGB888);
    let output = OutputDesc::new(4, 2, FourCC::XRGB8888, ColorDescription::srgb_sdr()).unwrap();
    let got = render_to_buffer(
        &output,
        &full_damage(&output),
        &[layer_of(
            &data,
            &geometry,
            Rect::new(0, 0, 4, 2),
            Transform::Normal,
        )],
    );
    for oy in 0..2u32 {
        for ox in 0..4u32 {
            let (bx, by) = (ox / 2, oy / 2);
            assert_eq!(
                got[(oy * 4 + ox) as usize],
                pack_rgb(pattern_rgba(bx, by)),
                "scale({ox},{oy})"
            );
        }
    }
}

#[test]
fn upscale_with_transform_composes() {
    // 2x3 buffer, Rot90 (surface 3x2) scaled 2x into a 6x4 dest.
    let (data, geometry) = pattern_buffer(2, 3, FourCC::RGB888);
    let output = OutputDesc::new(6, 4, FourCC::XRGB8888, ColorDescription::srgb_sdr()).unwrap();
    let got = render_to_buffer(
        &output,
        &full_damage(&output),
        &[layer_of(
            &data,
            &geometry,
            Rect::new(0, 0, 6, 4),
            Transform::Rot90,
        )],
    );
    // Forward construction: source pixel (bx,by) -> surface (sx,sy) ->
    // 2x2 block at (2*sx, 2*sy).
    for by in 0..3u32 {
        for bx in 0..2u32 {
            let (sx, sy) = Transform::Rot90.transform_point(bx as i32, by as i32, 2, 3);
            for dy in 0..2u32 {
                for dx in 0..2u32 {
                    let (ox, oy) = (2 * sx as u32 + dx, 2 * sy as u32 + dy);
                    assert_eq!(
                        got[(oy * 6 + ox) as usize],
                        pack_rgb(pattern_rgba(bx, by)),
                        "rot+scale({ox},{oy}) <- ({bx},{by})"
                    );
                }
            }
        }
    }
}

#[test]
fn downscale_samples_nearest_on_the_forward_grid() {
    // 4x2 buffer into a 2x1 dest (half size): output pixel centers map back
    // to even source pixels on the forward grid.
    let (data, geometry) = pattern_buffer(4, 2, FourCC::RGB888);
    let output = OutputDesc::new(2, 1, FourCC::XRGB8888, ColorDescription::srgb_sdr()).unwrap();
    let got = render_to_buffer(
        &output,
        &full_damage(&output),
        &[layer_of(
            &data,
            &geometry,
            Rect::new(0, 0, 2, 1),
            Transform::Normal,
        )],
    );
    // Output center mapping: sx_c = (x+0.5)*4/2 = 2x+1 -> floor {1, 3};
    // sy_c = (0+0.5)*2/1 = 1 -> row 1.
    assert_eq!(got[0], pack_rgb(pattern_rgba(1, 1)));
    assert_eq!(got[1], pack_rgb(pattern_rgba(3, 1)));
}

#[test]
fn dest_clipped_at_output_edges() {
    // A dest extending past the output on every side: only the intersection
    // renders; the 1:1 mapping still aligns inside it.
    let (data, geometry) = pattern_buffer(4, 3, FourCC::RGB888);
    let output = OutputDesc::new(4, 3, FourCC::XRGB8888, ColorDescription::srgb_sdr()).unwrap();
    let got = render_to_buffer(
        &output,
        &full_damage(&output),
        &[layer_of(
            &data,
            &geometry,
            Rect::new(-2, -1, 4, 3),
            Transform::Normal,
        )],
    );
    // Dest covers output pixels [0..2) x [0..2) mapping to buffer (2..4, 1..3).
    for y in 0..2u32 {
        for x in 0..2u32 {
            assert_eq!(
                got[(y * 4 + x) as usize],
                pack_rgb(pattern_rgba(x + 2, y + 1)),
                "clipped({x},{y})"
            );
        }
    }
}

#[test]
fn scaled_edges_never_write_outside_the_footprint() {
    // Odd scaling (3 -> 5): rounding at the right edge must not miss the
    // last source pixel or leak outside the dest.
    let (data, geometry) = pattern_buffer(3, 1, FourCC::RGB888);
    let output = OutputDesc::new(5, 1, FourCC::XRGB8888, ColorDescription::srgb_sdr()).unwrap();
    let got = render_to_buffer(
        &output,
        &full_damage(&output),
        &[layer_of(
            &data,
            &geometry,
            Rect::new(0, 0, 5, 1),
            Transform::Normal,
        )],
    );
    // sx_c = (x+0.5)*3/5: x=0 -> 0.3 -> 0; x=1 -> 0.9 -> 0; x=2 -> 1.5 -> 1;
    // x=3 -> 2.1 -> 2; x=4 -> 2.7 -> 2.
    let want = [0u32, 0, 1, 2, 2].map(|b| pack_rgb(pattern_rgba(b, 0)));
    assert_eq!(got, want);
}

#[test]
#[allow(clippy::many_single_char_names)] // y/cb/cr/r/g/b: the standards' names
fn yuv_transforms_sample_chroma_consistently() {
    // Rot90 of an NV12 pattern: the reference decodes the buffer with its
    // own straight-line NV12 reader (luma at the pixel, chroma from the
    // buffer's own 2x2 grid — chroma is spatial, it does not rotate with
    // the luma sample) at the inverse-mapped coordinates.
    let (data, _geometry) = pattern_buffer(4, 4, FourCC::NV12);
    // Independent layout: luma 4B rows x4, chroma at offset 16, 4B rows x2.
    let luma_stride = 4usize;
    let chroma_off = 16usize;
    let chroma_stride = 4usize;
    let ref_nv12 = |bx: usize, by: usize| -> u32 {
        let y = data[by * luma_stride + bx];
        let (cx, cy) = (bx / 2, by / 2);
        let cb = data[chroma_off + cy * chroma_stride + cx * 2];
        let cr = data[chroma_off + cy * chroma_stride + cx * 2 + 1];
        let (kr, kb) = (0.2126f32, 0.0722f32);
        let yv = f32::from(y) / 255.0;
        let cbv = (f32::from(cb) - 128.0) / 255.0;
        let crv = (f32::from(cr) - 128.0) / 255.0;
        let r = yv + 2.0 * (1.0 - kr) * crv;
        let b = yv + 2.0 * (1.0 - kb) * cbv;
        let g = (yv - kr * r - kb * b) / (1.0 - kr - kb);
        let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5).floor() as u32;
        0xFF00_0000 | (q(r) << 16) | (q(g) << 8) | q(b)
    };
    let geometry = ldp_renderer::testkit::geometry_for(4, 4, FourCC::NV12);
    let output = OutputDesc::new(4, 4, FourCC::XRGB8888, ColorDescription::srgb_sdr()).unwrap();
    let got = render_to_buffer(
        &output,
        &full_damage(&output),
        &[layer_of(
            &data,
            &geometry,
            Rect::new(0, 0, 4, 4),
            Transform::Rot90,
        )],
    );
    for oy in 0..4u32 {
        for ox in 0..4u32 {
            // Inverse of the forward map: (bx, by) = (oy, 3 - ox).
            let (bx, by) = (oy as usize, (3 - ox) as usize);
            let want = ref_nv12(bx, by);
            let g = got[(oy * 4 + ox) as usize];
            let dr = ((g >> 16) & 0xFF).abs_diff((want >> 16) & 0xFF);
            let dg = ((g >> 8) & 0xFF).abs_diff((want >> 8) & 0xFF);
            let db = (g & 0xFF).abs_diff(want & 0xFF);
            assert!(
                dr <= 1 && dg <= 1 && db <= 1,
                "nv12 rot90 ({ox},{oy}): {g:#010x} vs {want:#010x}"
            );
        }
    }
}
