//! Conformance helpers: the golden suites' shared fixtures.
//!
//! These are **inputs**, not oracles: [`pattern_rgba`] is the canonical
//! deterministic pixel rule and [`pattern_buffer`] materializes it in every
//! v1 format (YUV through the primaries-derived forward transform). The
//! expected *outputs* are always derived independently inside the test
//! files — that separation is what makes the suites conformance tests
//! rather than tautologies.

use ldp_core::buffer::{BufferGeometry, FourCC, PlaneLayout};
use ldp_core::color::{ColorRange, Primaries};
use ldp_core::geometry::{Rect, Region};

use crate::layer::{OutputDesc, SurfaceLayer};
use crate::yuv::YuvCoefficients;
use crate::{Renderer, SoftwareRenderer};

/// The canonical pattern pixel (straight RGBA).
///
/// Chosen so every channel exercises its full 0..255 range over modest
/// frame sizes and no two nearby pixels agree.
#[must_use]
#[allow(clippy::many_single_char_names)] // r/g/b/a/x/y are the pixel domain's names
pub fn pattern_rgba(x: u32, y: u32) -> [u8; 4] {
    let r = ((x * 7 + 13) & 0xFF) as u8;
    let g = ((y * 11 + 5) & 0xFF) as u8;
    let b = (((x + y) * 29 + 41) & 0xFF) as u8;
    let a = ((x * 3 + y * 17 + 89) & 0xFF) as u8;
    [r, g, b, a]
}

/// A validated geometry with `simple_layout` and the exact storage bytes.
///
/// # Panics
///
/// Panics if `simple_layout` ever produced an invalid layout (it cannot by
/// construction; the panic is a guard against regressions).
#[must_use]
pub fn geometry_for(w: u32, h: u32, format: FourCC) -> BufferGeometry {
    let layout = BufferGeometry::simple_layout(w, h, format);
    let storage = storage_of(&layout, w, h, format);
    BufferGeometry::new(
        w,
        h,
        format,
        ldp_core::buffer::Modifier::LINEAR,
        &layout,
        storage,
    )
    .expect("simple_layout is always valid")
}

/// Storage bytes spanned by a layout.
fn storage_of(layout: &[PlaneLayout], _w: u32, h: u32, format: FourCC) -> u64 {
    layout
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let (_, _, sub_h) = format.plane_block(i as u32);
            let rows = u64::from(h.div_ceil(sub_h).max(1));
            u64::from(p.offset) + u64::from(p.stride) * rows
        })
        .max()
        .unwrap_or(0)
}

/// Materialize the pattern in `format` (full range, BT.709 for YUV).
#[must_use]
#[allow(clippy::many_single_char_names)]
pub fn pattern_buffer(w: u32, h: u32, format: FourCC) -> (Vec<u8>, BufferGeometry) {
    let geometry = geometry_for(w, h, format);
    let mut data = vec![0u8; geometry.spanned_bytes() as usize];
    let coeffs = YuvCoefficients {
        kr: 0.2126,
        kb: 0.0722,
    };
    for y in 0..h {
        for x in 0..w {
            let [r, g, b, a] = pattern_rgba(x, y);
            let rgb = [
                f32::from(r) / 255.0,
                f32::from(g) / 255.0,
                f32::from(b) / 255.0,
            ];
            store_pixel(&mut data, &geometry, x, y, rgb, a, coeffs);
        }
    }
    (data, geometry)
}

/// Store one pixel of any format into `data` (full range).
#[allow(
    clippy::too_many_arguments,
    clippy::similar_names,
    clippy::many_single_char_names
)]
fn store_pixel(
    data: &mut [u8],
    geometry: &BufferGeometry,
    x: u32,
    y: u32,
    rgb: [f32; 3],
    a: u8,
    coeffs: YuvCoefficients,
) {
    let planes = geometry.planes();
    let px = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5).floor() as u8;
    // ARGB/ABGR store PREMULTIPLIED components (the wire convention); the
    // same fixed-point rule the blend kernels use, so golden expectations
    // are byte-stable.
    let premul = |c: u8, a: u8| ((u32::from(c) * u32::from(a) + 127) / 255) as u8;
    match geometry.format() {
        FourCC::XRGB8888 => {
            let layout = planes[0];
            let addr =
                layout.offset as usize + layout.stride as usize * y as usize + x as usize * 4;
            data[addr] = px(rgb[2]);
            data[addr + 1] = px(rgb[1]);
            data[addr + 2] = px(rgb[0]);
            // The X byte carries the pattern alpha: samplers must ignore it.
            data[addr + 3] = a;
        }
        FourCC::ARGB8888 => {
            let layout = planes[0];
            let addr =
                layout.offset as usize + layout.stride as usize * y as usize + x as usize * 4;
            let [r, g, b] = [px(rgb[0]), px(rgb[1]), px(rgb[2])];
            data[addr] = premul(b, a);
            data[addr + 1] = premul(g, a);
            data[addr + 2] = premul(r, a);
            data[addr + 3] = a;
        }
        FourCC::XBGR8888 => {
            let layout = planes[0];
            let addr =
                layout.offset as usize + layout.stride as usize * y as usize + x as usize * 4;
            data[addr] = px(rgb[0]);
            data[addr + 1] = px(rgb[1]);
            data[addr + 2] = px(rgb[2]);
            data[addr + 3] = a;
        }
        FourCC::ABGR8888 => {
            let layout = planes[0];
            let addr =
                layout.offset as usize + layout.stride as usize * y as usize + x as usize * 4;
            let [r, g, b] = [px(rgb[0]), px(rgb[1]), px(rgb[2])];
            data[addr] = premul(r, a);
            data[addr + 1] = premul(g, a);
            data[addr + 2] = premul(b, a);
            data[addr + 3] = a;
        }
        FourCC::RGB888 => {
            let layout = planes[0];
            let addr =
                layout.offset as usize + layout.stride as usize * y as usize + x as usize * 3;
            data[addr] = px(rgb[0]);
            data[addr + 1] = px(rgb[1]);
            data[addr + 2] = px(rgb[2]);
        }
        FourCC::BGR888 => {
            let layout = planes[0];
            let addr =
                layout.offset as usize + layout.stride as usize * y as usize + x as usize * 3;
            data[addr] = px(rgb[2]);
            data[addr + 1] = px(rgb[1]);
            data[addr + 2] = px(rgb[0]);
        }
        FourCC::RGB565 => {
            let layout = planes[0];
            let addr =
                layout.offset as usize + layout.stride as usize * y as usize + x as usize * 2;
            let r5 = (px(rgb[0]) >> 3) as u16;
            let g6 = (px(rgb[1]) >> 2) as u16;
            let b5 = (px(rgb[2]) >> 3) as u16;
            let v = (r5 << 11) | (g6 << 5) | b5;
            data[addr..addr + 2].copy_from_slice(&v.to_le_bytes());
        }
        _ => {
            // YUV family: forward-encode through the coefficients.
            if geometry.format() == FourCC::P010 {
                let (luma10, chroma_b10, chroma_r10) = encode_yuv10(coeffs, rgb);
                store_p010(data, geometry, x, y, luma10, chroma_b10, chroma_r10);
            } else {
                let (luma, chroma_b, chroma_r) = encode_yuv(coeffs, rgb);
                store_yuv(data, geometry, x, y, luma, chroma_b, chroma_r);
            }
        }
    }
}

/// Forward-encode straight RGB into full-range 8-bit YCbCr.
#[must_use]
pub fn encode_yuv(coeffs: YuvCoefficients, rgb: [f32; 3]) -> (u8, u8, u8) {
    let kg = 1.0 - coeffs.kr - coeffs.kb;
    let y = coeffs.kr * rgb[0] + kg * rgb[1] + coeffs.kb * rgb[2];
    let cb = (rgb[2] - y) / (2.0 * (1.0 - coeffs.kb));
    let cr = (rgb[0] - y) / (2.0 * (1.0 - coeffs.kr));
    // Chroma pivot: integer 128 (decode-exact for neutral chroma).
    let q = |v: f32| ((v.clamp(-0.5, 0.5) * 255.0 + 128.0).round() as i32).clamp(0, 255) as u8;
    let yq = ((y.clamp(0.0, 1.0) * 255.0 + 0.5).floor()) as u8;
    (yq, q(cb), q(cr))
}

/// Forward-encode straight RGB into full-range 10-bit YCbCr (P010 values).
#[must_use]
pub fn encode_yuv10(coeffs: YuvCoefficients, rgb: [f32; 3]) -> (u16, u16, u16) {
    let kg = 1.0 - coeffs.kr - coeffs.kb;
    let y = coeffs.kr * rgb[0] + kg * rgb[1] + coeffs.kb * rgb[2];
    let cb = (rgb[2] - y) / (2.0 * (1.0 - coeffs.kb));
    let cr = (rgb[0] - y) / (2.0 * (1.0 - coeffs.kr));
    // Chroma pivot: integer 512.
    let q = |v: f32| (((v.clamp(-0.5, 0.5) * 1023.0) + 512.0).round() as i32).clamp(0, 1023) as u16;
    let yq = ((y.clamp(0.0, 1.0) * 1023.0 + 0.5).floor() as i32).clamp(0, 1023) as u16;
    (yq, q(cb), q(cr))
}

/// Store a decoded-sample YUV pixel (8-bit values, full range).
#[allow(
    clippy::too_many_arguments,
    clippy::similar_names,
    clippy::many_single_char_names
)]
fn store_yuv(data: &mut [u8], geometry: &BufferGeometry, x: u32, y: u32, yv: u8, cb: u8, cr: u8) {
    let planes = geometry.planes();
    match geometry.format() {
        FourCC::YUYV => {
            let layout = planes[0];
            let block = (x / 2) as usize;
            let addr = layout.offset as usize + layout.stride as usize * y as usize + block * 4;
            data[addr + (x as usize & 1) * 2] = yv;
            data[addr + 1] = cb;
            data[addr + 3] = cr;
        }
        FourCC::NV12 => {
            let (y_layout, c_layout) = (planes[0], planes[1]);
            let y_addr =
                y_layout.offset as usize + y_layout.stride as usize * y as usize + x as usize;
            let (cx, cy) = ((x / 2) as usize, (y / 2) as usize);
            let c_addr = c_layout.offset as usize + c_layout.stride as usize * cy + cx * 2;
            data[y_addr] = yv;
            data[c_addr] = cb;
            data[c_addr + 1] = cr;
        }
        FourCC::YUV420 => {
            let (y_layout, u_layout, v_layout) = (planes[0], planes[1], planes[2]);
            let y_addr =
                y_layout.offset as usize + y_layout.stride as usize * y as usize + x as usize;
            let (cx, cy) = ((x / 2) as usize, (y / 2) as usize);
            data[y_addr] = yv;
            data[u_layout.offset as usize + u_layout.stride as usize * cy + cx] = cb;
            data[v_layout.offset as usize + v_layout.stride as usize * cy + cx] = cr;
        }
        _ => {}
    }
}

/// Store a P010 pixel (10-bit values, full range, high bits of u16).
// Pixel-coordinate plumbing: allow the domain's short names.
#[allow(clippy::similar_names, clippy::many_single_char_names)]
fn store_p010(
    data: &mut [u8],
    geometry: &BufferGeometry,
    col: u32,
    row: u32,
    luma10: u16,
    chroma_b10: u16,
    chroma_r10: u16,
) {
    let planes = geometry.planes();
    if geometry.format() != FourCC::P010 {
        return;
    }
    let (y_layout, c_layout) = (planes[0], planes[1]);
    let luma_addr =
        y_layout.offset as usize + y_layout.stride as usize * row as usize + col as usize * 2;
    let (cx, cy) = ((col / 2) as usize, (row / 2) as usize);
    let c_addr = c_layout.offset as usize + c_layout.stride as usize * cy + cx * 4;
    data[luma_addr..luma_addr + 2].copy_from_slice(&(luma10 << 6).to_le_bytes());
    data[c_addr..c_addr + 2].copy_from_slice(&(chroma_b10 << 6).to_le_bytes());
    data[c_addr + 2..c_addr + 4].copy_from_slice(&(chroma_r10 << 6).to_le_bytes());
}

/// A pattern buffer in P010 (separate because it needs 10-bit encoding).
#[must_use]
#[allow(clippy::similar_names, clippy::many_single_char_names)]
pub fn pattern_buffer_p010(w: u32, h: u32, range: ColorRange) -> (Vec<u8>, BufferGeometry) {
    let geometry = geometry_for(w, h, FourCC::P010);
    let mut data = vec![0u8; geometry.spanned_bytes() as usize];
    let coeffs = YuvCoefficients {
        kr: 0.2126,
        kb: 0.0722,
    };
    for row in 0..h {
        for col in 0..w {
            let [r, g, b, _a] = pattern_rgba(col, row);
            let rgb = [
                f32::from(r) / 255.0,
                f32::from(g) / 255.0,
                f32::from(b) / 255.0,
            ];
            let (mut luma10, mut chroma_b10, mut chroma_r10) = encode_yuv10(coeffs, rgb);
            if range == ColorRange::Studio {
                // Remap the full-range 10-bit values into the studio
                // excursion so the decoder's expansion returns the original.
                let remap = |v: u16, lo: i32, hi: i32| -> u16 {
                    let f = f32::from(v) / 1023.0;
                    (((hi - lo) as f32 * f + lo as f32).round() as i32).clamp(lo, hi) as u16
                };
                luma10 = remap(luma10, 64, 940);
                chroma_b10 = remap(chroma_b10, 512 - 448, 512 + 448);
                chroma_r10 = remap(chroma_r10, 512 - 448, 512 + 448);
            }
            store_p010(
                &mut data, &geometry, col, row, luma10, chroma_b10, chroma_r10,
            );
        }
    }
    (data, geometry)
}

/// Render layers into a fresh framebuffer and return it (the golden
/// suites' one-liner).
///
/// # Panics
///
/// Panics on any renderer contract violation — golden fixtures are valid by
/// construction, so a panic here is a production bug.
#[must_use]
pub fn render_to_buffer(
    output: &OutputDesc,
    damage: &Region,
    layers: &[SurfaceLayer<'_>],
) -> Vec<u32> {
    let mut renderer = SoftwareRenderer::new();
    renderer.begin_frame(output, damage).expect("valid output");
    renderer.submit(layers).expect("open frame");
    renderer.end_frame().expect("open frame");
    renderer.readout().to_vec()
}

/// A full-output damage region.
#[must_use]
pub fn full_damage(output: &OutputDesc) -> Region {
    Region::from_rect(Rect::new(0, 0, output.width, output.height))
}

/// The default test color description (sRGB, full range).
#[must_use]
pub fn srgb_full() -> ldp_core::color::ColorDescription {
    ldp_core::color::ColorDescription::srgb_sdr()
}

/// BT.709 full-range coefficients (hard-coded, independent of production
/// derivation — the golden anchors' reference constants).
#[must_use]
pub fn bt709_constants() -> YuvCoefficients {
    YuvCoefficients {
        kr: 0.2126,
        kb: 0.0722,
    }
}

/// Primaries used by the pattern buffers' YUV encoding.
#[must_use]
pub fn pattern_primaries() -> Primaries {
    Primaries::Bt709
}
