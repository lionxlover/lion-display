//! Golden format conformance: pixel-exact reference frames for all eleven
//! v1 fourcc formats (`docs/roadmap.md` Phase 8 EC).
//!
//! The reference model is **independent** of the production samplers: each
//! format gets a straight-line decoder in this file with its own addressing
//! arithmetic and hard-coded BT.709 constants, plus definitional anchors
//! (neutral gray, black, white, pure red) hand-derived from the standards.
//! RGB-family expectations are byte-exact; YUV expectations hold to ±1 LSB
//! per channel (the production path derives its coefficients from the
//! chromaticities, 0.212656 vs the rounded standard 0.2126).

use ldp_core::buffer::FourCC;
use ldp_core::color::ColorDescription;
use ldp_core::geometry::{Rect, Region, Transform};
use ldp_renderer::testkit::{
    full_damage, geometry_for, pattern_buffer, pattern_rgba, render_to_buffer,
};
use ldp_renderer::{BufferView, OutputDesc, SurfaceLayer};

const W: u32 = 13;
const H: u32 = 7;

fn layer_of<'a>(data: &'a [u8], geometry: &ldp_core::buffer::BufferGeometry) -> SurfaceLayer<'a> {
    let view = BufferView::new(1, data, geometry.clone()).expect("pattern geometry is valid");
    let (w, h) = (geometry.width(), geometry.height());
    SurfaceLayer::new(
        view,
        Rect::new(0, 0, w, h),
        Transform::Normal,
        ColorDescription::srgb_sdr(),
        1.0,
        Region::from_rect(Rect::new(0, 0, w, h)),
    )
}

fn output(fmt: FourCC) -> OutputDesc {
    OutputDesc::new(W, H, fmt, ColorDescription::srgb_sdr()).unwrap()
}

/// A small output matching the hand-planted 2x2 anchor buffers.
fn output2x2(fmt: FourCC) -> OutputDesc {
    OutputDesc::new(2, 2, fmt, ColorDescription::srgb_sdr()).unwrap()
}

// --- Independent reference decoders ---------------------------------------

/// Row bytes of a plane (4-byte padded), as `simple_layout` produces.
fn row_bytes(w: u32, bytes: u64) -> u32 {
    ((w as u64 * bytes).div_ceil(4) * 4) as u32
}

/// Reference-decode the pattern buffer of `format` at `(x, y)` into
/// premultiplied RGBA. Full range, BT.709 hard-coded.
// Pixel plumbing: x/y/a single-character coordinates are the domain's
// own names.
#[allow(clippy::many_single_char_names, clippy::similar_names)]
// XRGB/BGR888 (shared B,G,R order) and ARGB-over-black decode to the same
// expression by coincidence of the byte order — kept as separate arms for
// the per-format commentary.
#[allow(clippy::match_same_arms)]
fn ref_decode(data: &[u8], format: FourCC, x: u32, y: u32) -> [u8; 4] {
    match format {
        // XRGB and BGR888 share the B,G,R memory order (the X byte is
        // skipped), so their reference decode is literally the same math.
        // (BGR888 rows are 3 bytes per pixel; the X case 4.)
        FourCC::XRGB8888 | FourCC::BGR888 => {
            let (stride, bpp) = match format {
                FourCC::XRGB8888 => (row_bytes(W, 4) as usize, 4usize),
                _ => (row_bytes(W, 3) as usize, 3usize),
            };
            let a = stride * y as usize + x as usize * bpp;
            [data[a + 2], data[a + 1], data[a], 255]
        }
        FourCC::ARGB8888 => {
            let stride = row_bytes(W, 4) as usize;
            let a = stride * y as usize + x as usize * 4;
            // Stored premultiplied; compositing over the initial opaque
            // black leaves premul RGB with a fully opaque result.
            [data[a + 2], data[a + 1], data[a], 255]
        }
        FourCC::XBGR8888 => {
            let stride = row_bytes(W, 4) as usize;
            let a = stride * y as usize + x as usize * 4;
            [data[a], data[a + 1], data[a + 2], 255]
        }
        FourCC::ABGR8888 => {
            let stride = row_bytes(W, 4) as usize;
            let a = stride * y as usize + x as usize * 4;
            [data[a], data[a + 1], data[a + 2], 255]
        }
        FourCC::RGB888 => {
            let stride = row_bytes(W, 3) as usize;
            let a = stride * y as usize + x as usize * 3;
            [data[a], data[a + 1], data[a + 2], 255]
        }
        FourCC::RGB565 => {
            let stride = row_bytes(W, 2) as usize;
            let a = stride * y as usize + x as usize * 2;
            let v = u16::from_le_bytes([data[a], data[a + 1]]);
            let e5 = |c: u16| (((c << 3) | (c >> 2)) & 0xFF) as u8;
            let e6 = |c: u16| (((c << 2) | (c >> 4)) & 0xFF) as u8;
            [e5((v >> 11) & 0x1F), e6((v >> 5) & 0x3F), e5(v & 0x1F), 255]
        }
        FourCC::YUYV => {
            let stride = row_bytes(W.div_ceil(2), 4) as usize;
            let block = x as usize / 2;
            let a = stride * y as usize + block * 4;
            let yv = data[a + (x as usize & 1) * 2];
            let cb = data[a + 1];
            let cr = data[a + 3];
            ycbcr_to_rgb(yv, cb, cr)
        }
        FourCC::NV12 => {
            let luma_stride = row_bytes(W, 1) as usize;
            let luma_rows = H as usize;
            let chroma_stride = row_bytes(W.div_ceil(2), 2) as usize;
            let ya = luma_stride * y as usize + x as usize;
            let (cx, cy) = ((x / 2) as usize, (y / 2) as usize);
            let ca = luma_stride * luma_rows + chroma_stride * cy + cx * 2;
            ycbcr_to_rgb(data[ya], data[ca], data[ca + 1])
        }
        FourCC::YUV420 => {
            let luma_stride = row_bytes(W, 1) as usize;
            let chroma_stride = row_bytes(W.div_ceil(2), 1) as usize;
            let luma_rows = H as usize;
            let chroma_rows = H.div_ceil(2) as usize;
            let ya = luma_stride * y as usize + x as usize;
            let (cx, cy) = ((x / 2) as usize, (y / 2) as usize);
            let ua = luma_stride * luma_rows + chroma_stride * cy + cx;
            let va =
                luma_stride * luma_rows + chroma_stride * chroma_rows + chroma_stride * cy + cx;
            ycbcr_to_rgb(data[ya], data[ua], data[va])
        }
        FourCC::P010 => {
            let luma_stride = row_bytes(W, 2) as usize;
            let luma_rows = H as usize;
            let chroma_stride = row_bytes(W.div_ceil(2), 4) as usize;
            let ya = luma_stride * y as usize + x as usize * 2;
            let (cx, cy) = ((x / 2) as usize, (y / 2) as usize);
            let ca = luma_stride * luma_rows + chroma_stride * cy + cx * 4;
            let y10 = u16::from_le_bytes([data[ya], data[ya + 1]]) >> 6;
            let cb10 = u16::from_le_bytes([data[ca], data[ca + 1]]) >> 6;
            let cr10 = u16::from_le_bytes([data[ca + 2], data[ca + 3]]) >> 6;
            // Full range 10-bit decode with hard-coded constants,
            // integer pivot 512.
            let (kr, kb) = (0.2126f32, 0.0722f32);
            let yv = f32::from(y10) / 1023.0;
            let cb = (f32::from(cb10) - 512.0) / 1023.0;
            let cr = (f32::from(cr10) - 512.0) / 1023.0;
            let r = yv + 2.0 * (1.0 - kr) * cr;
            let b = yv + 2.0 * (1.0 - kb) * cb;
            let g = (yv - kr * r - kb * b) / (1.0 - kr - kb);
            let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5).floor() as u8;
            [q(r), q(g), q(b), 255]
        }
        _ => [0, 0, 0, 0],
    }
}

/// 8-bit full-range YCbCr → RGB with hard-coded BT.709 constants.
/// Integer chroma pivot 128 (decode-exact neutral).
#[allow(clippy::many_single_char_names)] // y/cb/cr/r/g/b: the standards' names
fn ycbcr_to_rgb(yv: u8, cb: u8, cr: u8) -> [u8; 4] {
    let (kr, kb) = (0.2126f32, 0.0722f32);
    let y = f32::from(yv) / 255.0;
    let cb = (f32::from(cb) - 128.0) / 255.0;
    let cr = (f32::from(cr) - 128.0) / 255.0;
    let r = y + 2.0 * (1.0 - kr) * cr;
    let b = y + 2.0 * (1.0 - kb) * cb;
    let g = (y - kr * r - kb * b) / (1.0 - kr - kb);
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5).floor() as u8;
    [q(r), q(g), q(b), 255]
}

/// Pack premul RGBA into an output word.
fn pack(fmt: FourCC, px: [u8; 4]) -> u32 {
    let (r, g, b, a) = (
        u32::from(px[0]),
        u32::from(px[1]),
        u32::from(px[2]),
        u32::from(px[3]),
    );
    match fmt {
        FourCC::ARGB8888 => (a << 24) | (r << 16) | (g << 8) | b,
        FourCC::XRGB8888 => 0xFF00_0000 | (r << 16) | (g << 8) | b,
        FourCC::ABGR8888 => (a << 24) | (b << 16) | (g << 8) | r,
        FourCC::XBGR8888 => 0xFF00_0000 | (b << 16) | (g << 8) | r,
        _ => 0,
    }
}

// --- The conformance sweep -------------------------------------------------

/// Every (format, output-format) pairing exercised by the suite.
fn cases() -> Vec<(FourCC, FourCC)> {
    vec![
        (FourCC::XRGB8888, FourCC::XRGB8888), // Copy path
        (FourCC::XBGR8888, FourCC::XBGR8888), // Copy path, B-order
        (FourCC::ARGB8888, FourCC::ARGB8888), // Alpha path
        (FourCC::ABGR8888, FourCC::ABGR8888), // Alpha path, B-order
        (FourCC::RGB888, FourCC::XRGB8888),   // Opaque path
        (FourCC::BGR888, FourCC::ABGR8888),   // Opaque path, swizzled output
        (FourCC::RGB565, FourCC::XRGB8888),
        (FourCC::XRGB8888, FourCC::ARGB8888), // X buffer onto ARGB output
        (FourCC::XBGR8888, FourCC::ARGB8888), // byte-order swap across paths
        (FourCC::YUYV, FourCC::XRGB8888),
        (FourCC::NV12, FourCC::XRGB8888),
        (FourCC::YUV420, FourCC::XRGB8888),
        (FourCC::P010, FourCC::XRGB8888),
    ]
}

#[test]
fn all_formats_match_the_reference_frame() {
    for (fmt, out_fmt) in cases() {
        let (data, geometry) = pattern_buffer(W, H, fmt);
        let expected: Vec<u32> = (0..H)
            .flat_map(|y| (0..W).map(move |x| (x, y)))
            .map(|(x, y)| pack(out_fmt, ref_decode(&data, fmt, x, y)))
            .collect();
        let got = render_to_buffer(
            &output(out_fmt),
            &full_damage(&output(out_fmt)),
            &[layer_of(&data, &geometry)],
        );
        let yuv = matches!(
            fmt,
            FourCC::YUYV | FourCC::NV12 | FourCC::P010 | FourCC::YUV420
        );
        let mut worst = 0u32;
        for (i, (&g, &e)) in got.iter().zip(expected.iter()).enumerate() {
            let (x, y) = ((i % W as usize) as u32, (i / W as usize) as u32);
            if g == e {
                continue;
            }
            let channel_diff =
                |shift: u32| ((g >> (shift * 8)) & 0xFF).abs_diff((e >> (shift * 8)) & 0xFF);
            let (dr, dg, db, da) = (
                channel_diff(2),
                channel_diff(1),
                channel_diff(0),
                channel_diff(3),
            );
            worst = worst.max(dr).max(dg).max(db).max(da);
            let limit = u32::from(yuv);
            assert!(
                dr <= limit && dg <= limit && db <= limit && da <= limit,
                "{fmt} -> {out_fmt} at ({x},{y}): got {g:#010x} want {e:#010x} \
                 (diff r{dr} g{dg} b{db} a{da})"
            );
        }
        let _ = worst; // worst is for breakpoint-free debugging reads
    }
}

#[test]
fn rgb_family_is_byte_exact_across_the_frame() {
    // The RGB cases above already assert zero tolerance; this makes the
    // intent explicit and independent of the tolerance switch.
    for (fmt, out_fmt) in cases() {
        if matches!(
            fmt,
            FourCC::YUYV | FourCC::NV12 | FourCC::P010 | FourCC::YUV420
        ) {
            continue;
        }
        let (data, geometry) = pattern_buffer(W, H, fmt);
        let expected: Vec<u32> = (0..H)
            .flat_map(|y| (0..W).map(move |x| (x, y)))
            .map(|(x, y)| pack(out_fmt, ref_decode(&data, fmt, x, y)))
            .collect();
        let got = render_to_buffer(
            &output(out_fmt),
            &full_damage(&output(out_fmt)),
            &[layer_of(&data, &geometry)],
        );
        assert_eq!(got, expected, "{fmt} -> {out_fmt}");
    }
}

#[test]
#[allow(clippy::many_single_char_names, clippy::similar_names)]
fn yuv_definitional_anchors_are_byte_exact() {
    // Hand-planted anchors: neutral gray (128,128,128) -> 128 exactly for
    // every YUV format; studio black (16,128,128) -> 0; white -> 235-ish is
    // range-specific so full-range 255 is used.
    for fmt in [FourCC::YUYV, FourCC::NV12, FourCC::YUV420, FourCC::P010] {
        let geometry = geometry_for(2, 2, fmt);
        let mut data = vec![0u8; geometry.spanned_bytes() as usize];
        let plant = |data: &mut [u8], y: u8, cb: u8, cr: u8| match fmt {
            FourCC::YUYV => {
                // 2x2: one 4-byte block per row.
                for row in 0..2usize {
                    let base = row * 4;
                    data[base] = y;
                    data[base + 1] = cb;
                    data[base + 2] = y;
                    data[base + 3] = cr;
                }
            }
            FourCC::NV12 => {
                // Luma 2 rows of 4 B; chroma one row at offset 8.
                for row in 0..2usize {
                    for x in 0..2usize {
                        data[row * 4 + x] = y;
                    }
                }
                data[8] = cb;
                data[9] = cr;
            }
            FourCC::YUV420 => {
                // Luma 2x4; U at offset 8, V at offset 12, one row each.
                for row in 0..2usize {
                    for x in 0..2usize {
                        data[row * 4 + x] = y;
                    }
                }
                data[8] = cb;
                data[12] = cr;
            }
            FourCC::P010 => {
                // Luma 2 rows of two u16 samples; chroma pair at offset 8.
                // Luma maps through the 1023 grid; the anchors always use
                // neutral chroma, which is the integer pivot 512.
                let (y10, cb10, cr10): (u16, u16, u16) =
                    (((f32::from(y) * 1023.0 / 255.0).round()) as u16, 512, 512);
                for row in 0..2usize {
                    for x in 0..2usize {
                        let a = row * 4 + x * 2;
                        data[a..a + 2].copy_from_slice(&(y10 << 6).to_le_bytes());
                    }
                }
                data[8..10].copy_from_slice(&(cb10 << 6).to_le_bytes());
                data[10..12].copy_from_slice(&(cr10 << 6).to_le_bytes());
            }
            _ => unreachable!("YUV-only loop"),
        };
        for (y, cb, cr, want) in [
            (128u8, 128u8, 128u8, 128u8), // neutral gray: identity of the matrix
            (0u8, 128u8, 128u8, 0u8),     // black
            (255u8, 128u8, 128u8, 255u8), // white
        ] {
            plant(&mut data, y, cb, cr);
            let out = output2x2(FourCC::XRGB8888);
            let got = render_to_buffer(&out, &full_damage(&out), &[layer_of(&data, &geometry)]);
            for px in &got {
                let r = (px >> 16) & 0xFF;
                let g = (px >> 8) & 0xFF;
                let b = px & 0xFF;
                assert_eq!(
                    (r, g, b),
                    (u32::from(want), u32::from(want), u32::from(want)),
                    "{fmt} anchor y={y}"
                );
            }
        }
    }
}

#[test]
fn yuv_pure_red_guard_against_uv_swap() {
    // Pure red from the DEFINITIONS: Y=Kr, Cb=-Kr/(2(1-Kb)), Cr=0.5 with
    // the rounded standard constants. A U/V swap would decode blue.
    let geometry = geometry_for(2, 2, FourCC::NV12);
    let mut data = vec![0u8; geometry.spanned_bytes() as usize];
    // Pure red from the definitions, quantized with the integer pivot.
    let y = (0.2126f32 * 255.0 + 0.5) as u8;
    let cb = ((-0.2126f32 / (2.0 * 0.9278) * 255.0) + 128.0).round() as u8;
    let cr = (0.5f32 * 255.0 + 128.0).round().min(255.0) as u8;
    for row in 0..2usize {
        for x in 0..2usize {
            data[row * 4 + x] = y;
        }
    }
    data[8] = cb;
    data[9] = cr;
    let out = output2x2(FourCC::XRGB8888);
    let got = render_to_buffer(&out, &full_damage(&out), &[layer_of(&data, &geometry)]);
    for px in &got {
        let (r, g, b) = ((px >> 16) & 0xFF, (px >> 8) & 0xFF, px & 0xFF);
        assert!(
            r >= 253 && g <= 2 && b <= 2,
            "pure red anchor: r={r} g={g} b={b}"
        );
    }
}

#[test]
fn x_byte_is_ignored_in_x_formats() {
    // The pattern fixture plants the pattern alpha into the X byte of XRGB;
    // the sampler must still force full alpha (checked implicitly by the
    // byte-exact sweep — this pins it explicitly on a planted dirty byte).
    let (mut data, geometry) = pattern_buffer(2, 1, FourCC::XRGB8888);
    // Pixel (1,0) lives at byte offset 4; its X byte at 7.
    data[7] = 0x00; // dirty X byte
    let out = OutputDesc::new(2, 1, FourCC::XRGB8888, ColorDescription::srgb_sdr()).unwrap();
    let got = render_to_buffer(&out, &full_damage(&out), &[layer_of(&data, &geometry)]);
    assert_eq!(got[1] >> 24, 0xFF);
    let [r, g, b, _] = pattern_rgba(1, 0);
    assert_eq!(
        got[1] & 0x00FF_FFFF,
        (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)
    );
}
