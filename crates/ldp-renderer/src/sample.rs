//! Per-pixel sampling: format dispatch and the RGB-family unpackers.
//!
//! Every sample leaves here as **premultiplied encoded RGBA** (u8 fast path)
//! or **straight encoded RGBA** (f32 pipeline path). Channel order follows
//! the DRM fourcc convention: the named order is the u32 field order
//! (MSB→LSB) and memory is little-endian — `ARGB8888` reads the u32 as
//! `0xAARRGGBB` (memory bytes `B,G,R,A`), byte-identical to `wl_shm`, which
//! is what the Phase 16/17 bridges require.

use ldp_core::buffer::FourCC;
use ldp_core::color::ColorRange;

use crate::view::BufferView;
use crate::yuv::{self, YuvCoefficients};

/// Whether the format carries per-pixel alpha (premultiplied on the wire).
#[must_use]
pub fn format_has_alpha(format: FourCC) -> bool {
    matches!(format, FourCC::ARGB8888 | FourCC::ABGR8888)
}

/// Whether the format is YUV-family (decoded through the YCbCr matrix).
#[must_use]
pub fn format_is_yuv(format: FourCC) -> bool {
    matches!(
        format,
        FourCC::YUYV | FourCC::NV12 | FourCC::P010 | FourCC::YUV420
    )
}

/// Sample one pixel as premultiplied encoded u8 RGBA.
///
/// YUV formats decode through the primaries-derived YCbCr matrix and come
/// out opaque; ARGB/ABGR are premultiplied as stored; X-formats force full
/// alpha. `coeffs` is the layer's precomputed YCbCr matrix (the 3×3 solve
/// is far too expensive per pixel).
pub fn sample_premul_u8(
    view: &BufferView<'_>,
    bx: u32,
    by: u32,
    coeffs: YuvCoefficients,
    range: ColorRange,
) -> [u8; 4] {
    let format = view.geometry().format();
    if format_is_yuv(format) {
        let (y, cb, cr) = yuv::fetch_sample(view.data(), view.geometry(), bx, by, range);
        let [r, g, b] = coeffs.to_rgb(y, cb, cr);
        return [quantize(r), quantize(g), quantize(b), 255];
    }
    let data = view.data();
    match format {
        FourCC::XRGB8888 => {
            let v = read_u32(view, bx, by);
            [chan(v, 16), chan(v, 8), chan(v, 0), 255]
        }
        FourCC::ARGB8888 => {
            let v = read_u32(view, bx, by);
            [chan(v, 16), chan(v, 8), chan(v, 0), chan(v, 24)]
        }
        FourCC::XBGR8888 => {
            let v = read_u32(view, bx, by);
            // u32 fields MSB→LSB: X, B, G, R (memory bytes R, G, B, X).
            [chan(v, 0), chan(v, 8), chan(v, 16), 255]
        }
        FourCC::ABGR8888 => {
            let v = read_u32(view, bx, by);
            [chan(v, 0), chan(v, 8), chan(v, 16), chan(v, 24)]
        }
        FourCC::RGB888 => {
            let a = addr3(view, bx, by);
            [data[a], data[a + 1], data[a + 2], 255]
        }
        FourCC::BGR888 => {
            let a = addr3(view, bx, by);
            [data[a + 2], data[a + 1], data[a], 255]
        }
        FourCC::RGB565 => {
            let a = addr565(view, bx, by);
            let v = u16::from_le_bytes([data[a], data[a + 1]]);
            let r5 = (v >> 11) & 0x1f;
            let g6 = (v >> 5) & 0x3f;
            let b5 = v & 0x1f;
            [expand5(r5), expand6(g6), expand5(b5), 255]
        }
        // YUV handled above; unsupported formats never reach a layer
        // (BufferGeometry validation gates the format family).
        _ => [0, 0, 0, 0],
    }
}

/// Sample one pixel as straight (non-premultiplied) encoded f32 RGBA.
///
/// The pipeline path needs straight components because transfer functions
/// and primaries matrices are defined on straight color; premultiplication
/// happens after the decode.
pub(crate) fn sample_straight_f32(
    view: &BufferView<'_>,
    bx: u32,
    by: u32,
    coeffs: YuvCoefficients,
    range: ColorRange,
) -> [f32; 4] {
    let format = view.geometry().format();
    if format_is_yuv(format) {
        let (y, cb, cr) = yuv::fetch_sample(view.data(), view.geometry(), bx, by, range);
        let [r, g, b] = coeffs.to_rgb(y, cb, cr);
        return [r, g, b, 1.0];
    }
    let px = sample_premul_u8(view, bx, by, coeffs, range);
    let [r, g, b, a] = px.map(f32::from);
    let nf = 1.0 / 255.0;
    if a == 0.0 {
        return [0.0; 4];
    }
    let af = a / 255.0;
    // Un-premultiply: straight = premul / alpha (clamped to 1).
    [
        ((r * nf) / af).min(1.0),
        ((g * nf) / af).min(1.0),
        ((b * nf) / af).min(1.0),
        af,
    ]
}

/// 5-bit → 8-bit by bit replication.
const fn expand5(v: u16) -> u8 {
    (((v << 3) | (v >> 2)) & 0xFF) as u8
}

/// 6-bit → 8-bit by bit replication.
const fn expand6(v: u16) -> u8 {
    (((v << 2) | (v >> 4)) & 0xFF) as u8
}

/// Quantize an encoded-domain `0..=1` float to u8 (round-half-up).
fn quantize(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5).floor() as u8
}

/// Extract one byte channel of a u32 sample.
const fn chan(v: u32, shift: u32) -> u8 {
    ((v >> shift) & 0xFF) as u8
}

/// Read the 32-bit pixel at `(bx, by)` of a 32-bit RGB-family buffer.
fn read_u32(view: &BufferView<'_>, bx: u32, by: u32) -> u32 {
    let layout = view.geometry().planes()[0];
    let addr = layout.offset as usize + layout.stride as usize * by as usize + bx as usize * 4;
    let d = view.data();
    u32::from_le_bytes([d[addr], d[addr + 1], d[addr + 2], d[addr + 3]])
}

/// Byte address of a 24-bit pixel.
fn addr3(view: &BufferView<'_>, bx: u32, by: u32) -> usize {
    let layout = view.geometry().planes()[0];
    layout.offset as usize + layout.stride as usize * by as usize + bx as usize * 3
}

/// Byte address of a 16-bit pixel.
fn addr565(view: &BufferView<'_>, bx: u32, by: u32) -> usize {
    let layout = view.geometry().planes()[0];
    layout.offset as usize + layout.stride as usize * by as usize + bx as usize * 2
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit;
    use ldp_core::buffer::BufferGeometry;

    fn view_of<'a>(data: &'a [u8], g: &BufferGeometry) -> BufferView<'a> {
        BufferView::new(0, data, g.clone()).unwrap()
    }

    fn coeffs() -> YuvCoefficients {
        YuvCoefficients {
            kr: 0.2126,
            kb: 0.0722,
        }
    }

    #[test]
    fn xrgb_reads_fields_in_drm_order() {
        let (data, g) = testkit::pattern_buffer(2, 1, FourCC::XRGB8888);
        let v = view_of(&data, &g);
        // Pattern pixel (0,0): r=13, g=5, b=41 (see testkit::pattern_rgba).
        let px = sample_premul_u8(&v, 0, 0, coeffs(), ColorRange::Full);
        assert_eq!(px, [13, 5, 41, 255]);
        // Hand-plant a pixel with the X byte dirty: alpha stays forced.
        let mut data = data.clone();
        let layout = g.planes()[0];
        let addr = layout.offset as usize + 4;
        data[addr + 3] = 0x42; // X byte
        let v = view_of(&data, &g);
        let px = sample_premul_u8(&v, 1, 0, coeffs(), ColorRange::Full);
        assert_eq!(px[3], 255);
    }

    #[test]
    fn abgr_swaps_r_and_b_fields() {
        // One pixel: ARGB u32 0x8040C020 → premul r=0x40, g=0xC0, b=0x20.
        let layout = ldp_core::buffer::PlaneLayout {
            offset: 0,
            stride: 4,
        };
        let g = BufferGeometry::new(
            1,
            1,
            FourCC::ABGR8888,
            ldp_core::buffer::Modifier::LINEAR,
            &[layout],
            4,
        )
        .unwrap();
        let abgr = 0x8020_C040u32; // A=0x80, B=0x20, G=0xC0, R=0x40
        let data = abgr.to_le_bytes().to_vec();
        let v = view_of(&data, &g);
        let px = sample_premul_u8(&v, 0, 0, coeffs(), ColorRange::Full);
        assert_eq!(px, [0x40, 0xC0, 0x20, 0x80]);
        let s = sample_straight_f32(&v, 0, 0, coeffs(), ColorRange::Full);
        // Straight: premul / (128/255) ≈ ×1.9922.
        assert!((s[0] - (0x40 as f32 / 128.0)).abs() < 1e-3);
        assert!((s[3] - 128.0 / 255.0).abs() < 1e-6);
    }

    #[test]
    fn rgb565_bit_replication() {
        let layout = ldp_core::buffer::PlaneLayout {
            offset: 0,
            stride: 4,
        };
        let g = BufferGeometry::new(
            1,
            1,
            FourCC::RGB565,
            ldp_core::buffer::Modifier::LINEAR,
            &[layout],
            4,
        )
        .unwrap();
        // Pure white and pure black replicate exactly.
        for (v565, want) in [(0xFFFFu16, [255u8, 255, 255]), (0x0000, [0, 0, 0])] {
            let data = v565.to_le_bytes().to_vec();
            let view = view_of(&data, &g);
            let px = sample_premul_u8(&view, 0, 0, coeffs(), ColorRange::Full);
            assert_eq!(&px[..3], &want);
        }
        // Mid values: 5-bit 16 -> 10000100b = 132; 6-bit 32 -> 10000010b = 130.
        let v: u16 = (16 << 11) | (32 << 5) | 16;
        let data = v.to_le_bytes().to_vec();
        let view = view_of(&data, &g);
        let px = sample_premul_u8(&view, 0, 0, coeffs(), ColorRange::Full);
        assert_eq!(&px[..3], &[132, 130, 132]);
    }

    #[test]
    fn straight_unpremul_handles_zero_alpha() {
        let layout = ldp_core::buffer::PlaneLayout {
            offset: 0,
            stride: 4,
        };
        let g = BufferGeometry::new(
            1,
            1,
            FourCC::ARGB8888,
            ldp_core::buffer::Modifier::LINEAR,
            &[layout],
            4,
        )
        .unwrap();
        let data = 0x00_40_30_20u32.to_le_bytes().to_vec();
        let v = view_of(&data, &g);
        let s = sample_straight_f32(&v, 0, 0, coeffs(), ColorRange::Full);
        assert_eq!(s, [0.0; 4]);
    }
}
