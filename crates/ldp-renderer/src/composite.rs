//! Span compositors: the per-row pixel loops.
//!
//! Five paths, selected once per layer (see [`Mode`]):
//!
//! * **Copy** — 1:1 untransformed opaque X/XBGR onto the same output
//!   format: a row word-copy with forced alpha (the scanline
//!   `memcpy`-equivalent; the 4K single-surface EC lives here),
//! * **CopyPremul** — 1:1 untransformed ARGB/ABGR onto the same output
//!   format at full opacity: per 64-pixel chunk, fully-opaque chunks
//!   word-copy (window interiors — the most common real shape) and
//!   mixed chunks take the per-pixel blend (the Phase 22 fast path),
//! * **Opaque** — no-alpha formats at full opacity: sample and write,
//!   never reading the destination,
//! * **Alpha** — premultiplied integer `over` in the target's *encoded*
//!   space (pixman/GL-fast-path math, bit-stable, no libm), with an
//!   opaque-write shortcut,
//! * **Pipeline** — cross-description composites: decode to scene-linear,
//!   blend linear, re-encode.
//!
//! All paths consume the merged damage spans of [`crate::spans::RowSpans`]
//! (served per row through the per-frame [`crate::spans::RowIndex`]),
//! so writes land exactly on damaged pixels.

use ldp_core::buffer::FourCC;
use ldp_core::geometry::Rect;

use crate::effects::{
    corner_band_xs, over_premul, rounded_coverage, scale_premul, unpack_canonical,
};
use crate::mapping::{FrameTarget, Mapping, RowSpan};

use crate::pipeline::ColorPipeline;
use crate::sample::{format_has_alpha, sample_premul_u8, sample_straight_f32};
use crate::spans::RowSpans;
use crate::view::BufferView;
use crate::yuv::YuvCoefficients;

/// Statistics of one frame's compositing work.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderStats {
    /// Layers submitted this frame.
    pub layers: u32,
    /// Layers whose clipped spans were non-empty (actually touched pixels).
    pub layers_rendered: u32,
    /// Damaged-pixel coverage summed over layers (overlapping layers each
    /// count their coverage).
    pub pixels_damaged: u64,
    /// Pixels written without reading the destination.
    pub pixels_opaque: u64,
    /// Pixels composited with a read-modify-write blend.
    pub pixels_blended: u64,
    /// Pixels touched by the Liquid effect passes (Phase 27): shadow
    /// fringes, frost materials, corner-coverage blends. Zero on plain
    /// frames — the Phase 26 behavior exactly.
    pub pixels_effect: u64,
}

/// The resolved pixel path of a layer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Mode {
    /// Row word-copy (same X-family format, 1:1, Normal, opaque).
    Copy,
    /// Chunked word-copy with per-pixel fallback (same A-family format,
    /// 1:1, Normal, full opacity): opaque chunks copy, mixed chunks blend.
    CopyPremul,
    /// Write-only sampling (no-alpha format, full opacity, identity color).
    Opaque,
    /// Encoded-space premultiplied `over` (identity color).
    Alpha,
    /// Scene-linear decode/blend/encode (color mismatch).
    Pipeline,
}

/// A layer fully resolved for compositing.
pub(crate) struct LayerPlan<'a> {
    /// The buffer.
    pub view: &'a BufferView<'a>,
    /// Output-space placement.
    pub dest: Rect,
    /// Inverse pixel mapping.
    pub mapping: Mapping,
    /// Quantized opacity.
    pub opacity_q: u8,
    /// YCbCr matrix (YUV formats).
    pub coeffs: YuvCoefficients,
    /// Source color range.
    pub range: ldp_core::color::ColorRange,
    /// Resolved color pipeline.
    pub pipeline: ColorPipeline,
    /// Pixel path.
    pub mode: Mode,
    /// Antialiased corner radius (Phase 27); `0` keeps the fast paths.
    pub corner_radius: u32,
}

/// Composite one layer into the framebuffer, clipped to damage.
///
/// `index` is the per-frame damage row index (built once per frame in
/// `begin_frame`); each row's damage intervals are served in O(1) and
/// clipped to the layer's destination — the per-layer cost no longer
/// scales with the damage rectangle count.
///
/// Phase 29: styled layers dispatch **per row** — a row outside the
/// corner-arc bands (the whole straight interior; see
/// [`crate::effects::corner_band_xs`]) carries coverage 255 on every
/// pixel, so it takes the *plain* path (the word copies included)
/// while paying the styled statistics exactly as before. Only the
/// band rows run the per-pixel coverage fold.
pub(crate) fn composite_layer(
    target: &mut FrameTarget<'_>,
    plan: &LayerPlan<'_>,
    index: &crate::spans::RowIndex,
    spans: &mut RowSpans,
    stats: &mut RenderStats,
) {
    if plan.dest.is_empty() {
        return;
    }
    let y0 = plan.dest.y.max(0);
    let y1 = plan.dest.bottom().min(target.height as i32);
    let mut rendered = false;
    for y in y0..y1 {
        let row = index.row(y);
        if row.is_empty() {
            continue;
        }
        RowSpans::clip(row, &plan.dest, spans);
        if spans.is_empty() {
            continue;
        }
        rendered = true;
        let row_base = (y as u32 * target.width) as usize;
        // A styled layer's full rows (coverage 255 everywhere) take
        // the plain path — byte-identical to the coverage-255 branch
        // the styled span itself runs.
        let styled_row =
            plan.corner_radius > 0 && !corner_band_xs(plan.dest, plan.corner_radius, y).is_empty();
        for &(x0, x1) in spans.intervals() {
            stats.pixels_damaged += (x1 - x0) as u64;
            let span = RowSpan {
                row_base,
                y,
                x0,
                x1,
            };
            if plan.corner_radius > 0 {
                stats.pixels_effect += (x1 - x0) as u64;
                if !styled_row {
                    // The row's bulk: every pixel's coverage is 255.
                    dispatch_plain(target, &span, plan, stats);
                } else if plan.mode == Mode::Pipeline {
                    styled_pipeline_span(target, &span, plan, stats);
                } else {
                    styled_alpha_span(target, &span, plan, stats);
                }
            } else {
                dispatch_plain(target, &span, plan, stats);
            }
        }
    }
    if rendered {
        stats.layers_rendered += 1;
    }
}

/// The plain (unstyled) span dispatch — the Phase 26 behavior, and
/// the path styled layers' full rows now share.
fn dispatch_plain(
    target: &mut FrameTarget<'_>,
    span: &RowSpan,
    plan: &LayerPlan<'_>,
    stats: &mut RenderStats,
) {
    match plan.mode {
        Mode::Copy => {
            copy_span(target, span, plan);
            stats.pixels_opaque += (span.x1 - span.x0) as u64;
        }
        Mode::CopyPremul => {
            copy_premul_span(target, span, plan, stats);
        }
        Mode::Opaque => {
            sample_span::<false>(target, span, plan, stats);
        }
        Mode::Alpha => {
            sample_span::<true>(target, span, plan, stats);
        }
        Mode::Pipeline => {
            pipeline_span(target, span, plan, stats);
        }
    }
}

/// The 1:1 word-copy path. Only reached with an X-family buffer whose
/// format equals the output format, untransformed and opaque.
fn copy_span(target: &mut FrameTarget<'_>, span: &RowSpan, plan: &LayerPlan<'_>) {
    let layout = plan.view.geometry().planes()[0];
    let stride = layout.stride as usize;
    // Source coordinates are destination-relative: the span lives in
    // output space, so both axes subtract the layer's destination
    // origin. (The x origin was missing — found by the Phase 10
    // vertical slice placing a layer away from (0,0).)
    let src_row = layout.offset as usize + stride * (span.y - plan.dest.y) as usize;
    let data = plan.view.data();
    let s0 = src_row + (span.x0 - plan.dest.x) as usize * 4;
    let s1 = src_row + (span.x1 - plan.dest.x) as usize * 4;
    let dst = &mut target.fb[span.row_base + span.x0 as usize..span.row_base + span.x1 as usize];
    for (src, d) in data[s0..s1].chunks_exact(4).zip(dst.iter_mut()) {
        let word = u32::from_le_bytes([src[0], src[1], src[2], src[3]]);
        *d = word | 0xFF00_0000;
    }
}

/// The chunk granularity of the premultiplied copy path: fully-opaque
/// chunks word-copy, mixed chunks blend per pixel. 64 pixels (256 bytes)
/// bounds the per-pixel fallback of an edge-intersecting chunk while
/// keeping the opaque-scan overhead amortized.
const PREMUL_CHUNK_PX: usize = 64;

/// The 1:1 premultiplied-copy path (Phase 22).
///
/// Only reached with an A-family buffer (ARGB8888/ABGR8888) whose format
/// equals the output format, untransformed and at full opacity — the shape
/// of a real window interior. Per [`PREMUL_CHUNK_PX`] chunk: if every
/// pixel's alpha is 255, the chunk word-copies (the `over` of an opaque
/// source is the source itself, and `pack_out ∘ unpack` is the identity on
/// a same-format word — so the copy is *bit-identical* to the blend); mixed
/// chunks take the per-pixel [`sample_span`] blend, so translucent edges
/// and shadows blend exactly as before.
fn copy_premul_span(
    target: &mut FrameTarget<'_>,
    span: &RowSpan,
    plan: &LayerPlan<'_>,
    stats: &mut RenderStats,
) {
    let layout = plan.view.geometry().planes()[0];
    let stride = layout.stride as usize;
    let src_row = layout.offset as usize + stride * (span.y - plan.dest.y) as usize;
    let data = plan.view.data();
    let src_x0 = (span.x0 - plan.dest.x) as usize;
    let mut x = span.x0;
    while x < span.x1 {
        let x_end = (x + PREMUL_CHUNK_PX as i32).min(span.x1);
        let s0 = src_row + (src_x0 + (x - span.x0) as usize) * 4;
        let s1 = s0 + (x_end - x) as usize * 4;
        let chunk = &data[s0..s1];
        // The alpha byte sits at offset 3 in both A-family layouts (the
        // high byte of the little-endian word).
        if chunk.chunks_exact(4).all(|px| px[3] == 0xFF) {
            let dst = &mut target.fb[span.row_base + x as usize..span.row_base + x_end as usize];
            for (src, d) in chunk.chunks_exact(4).zip(dst.iter_mut()) {
                *d = u32::from_le_bytes([src[0], src[1], src[2], src[3]]);
            }
            stats.pixels_opaque += (x_end - x) as u64;
        } else {
            let sub = RowSpan {
                row_base: span.row_base,
                y: span.y,
                x0: x,
                x1: x_end,
            };
            sample_span::<true>(target, &sub, plan, stats);
        }
        x = x_end;
    }
}

/// The encoded-space integer path (Opaque and Alpha modes).
///
/// `BLEND` selects opaque write-only versus the per-pixel `over` blend with
/// the opaque shortcut. All arithmetic is u32 fixed-point with
/// round-half-up — bit-stable on every platform.
///
/// Typestate generic instead of a runtime flag: the branch sits outside
/// the pixel loop.
///
/// Phase 29: a 1:1 untransformed layer over a 32-bit RGB-family buffer
/// takes the tight [`direct_span`] loop — no per-pixel mapping match,
/// no per-pixel format dispatch, the source word's channels extracted
/// by compile-time constants (the translucent-interior hot path).
// Pixel plumbing: q/x/d/a2 are the blend kernel's standard short names.
#[allow(clippy::many_single_char_names)]
fn sample_span<const BLEND: bool>(
    target: &mut FrameTarget<'_>,
    span: &RowSpan,
    plan: &LayerPlan<'_>,
    stats: &mut RenderStats,
) {
    if let Mapping::Direct { ox, oy } = plan.mapping {
        match plan.view.geometry().format() {
            FourCC::ARGB8888 => {
                direct_span::<BLEND, { SRC_ARGB }>(target, span, plan, ox, oy, stats);
                return;
            }
            FourCC::XRGB8888 => {
                direct_span::<BLEND, { SRC_XRGB }>(target, span, plan, ox, oy, stats);
                return;
            }
            FourCC::ABGR8888 => {
                direct_span::<BLEND, { SRC_ABGR }>(target, span, plan, ox, oy, stats);
                return;
            }
            FourCC::XBGR8888 => {
                direct_span::<BLEND, { SRC_XBGR }>(target, span, plan, ox, oy, stats);
                return;
            }
            // YUV, RGB888/BGR888, RGB565: the general sampler below.
            _ => {}
        }
    }
    let q = plan.opacity_q;
    for x in span.x0..span.x1 {
        let Some((bx, by)) = plan.mapping.map(x, span.y) else {
            continue; // Scaled-edge miss: leave the destination alone.
        };
        let px = sample_premul_u8(plan.view, bx, by, plan.coeffs, plan.range);
        let idx = span.row_base + x as usize;
        if !BLEND {
            debug_assert_eq!(px[3], 255, "opaque path on an alpha format");
            target.fb[idx] = pack_out(target.format, px[0], px[1], px[2], 255);
            stats.pixels_opaque += 1;
            continue;
        }
        if px[3] == 255 && q == 255 {
            target.fb[idx] = pack_out(target.format, px[0], px[1], px[2], 255);
            stats.pixels_opaque += 1;
            continue;
        }
        if px[3] == 0 {
            continue; // Fully transparent source: keep the destination.
        }
        // Premultiplied over with the opacity factor folded in — the
        // crate's one shared integer rule (effects::over_premul, the
        // same kernel the GL reference evaluator runs).
        let d = unpack_out(target.format, target.fb[idx]);
        let out = over_premul(px, d, q);
        target.fb[idx] = pack_out(target.format, out[0], out[1], out[2], out[3]);
        stats.pixels_blended += 1;
    }
}

/// Source format selectors for [`direct_span`] (const-generic dispatch:
/// the channel extraction folds to shifts at compile time).
const SRC_ARGB: u8 = 0;
const SRC_XRGB: u8 = 1;
const SRC_ABGR: u8 = 2;
const SRC_XBGR: u8 = 3;

/// The 1:1 direct-mapping loop over a 32-bit RGB-family source —
/// byte-equal to [`sample_span`]'s general path (the same skips, the
/// same blend, the same statistics), with the mapping and format
/// dispatch hoisted out of the pixel loop.
#[allow(clippy::many_single_char_names)] // the blend kernel's standard short names
fn direct_span<const BLEND: bool, const SRC: u8>(
    target: &mut FrameTarget<'_>,
    span: &RowSpan,
    plan: &LayerPlan<'_>,
    ox: i32,
    oy: i32,
    stats: &mut RenderStats,
) {
    let q = plan.opacity_q;
    let layout = plan.view.geometry().planes()[0];
    let stride = layout.stride as usize;
    let src_row = layout.offset as usize + stride * (span.y - oy) as usize;
    let data = plan.view.data();
    for x in span.x0..span.x1 {
        let s = src_row + (x - ox) as usize * 4;
        let word = u32::from_le_bytes([data[s], data[s + 1], data[s + 2], data[s + 3]]);
        let px = match SRC {
            SRC_ARGB => [
                ((word >> 16) & 0xFF) as u8,
                ((word >> 8) & 0xFF) as u8,
                (word & 0xFF) as u8,
                ((word >> 24) & 0xFF) as u8,
            ],
            SRC_XRGB => [
                ((word >> 16) & 0xFF) as u8,
                ((word >> 8) & 0xFF) as u8,
                (word & 0xFF) as u8,
                255,
            ],
            SRC_ABGR => [
                (word & 0xFF) as u8,
                ((word >> 8) & 0xFF) as u8,
                ((word >> 16) & 0xFF) as u8,
                ((word >> 24) & 0xFF) as u8,
            ],
            _ => [
                (word & 0xFF) as u8,
                ((word >> 8) & 0xFF) as u8,
                ((word >> 16) & 0xFF) as u8,
                255,
            ],
        };
        let idx = span.row_base + x as usize;
        if !BLEND {
            debug_assert_eq!(px[3], 255, "opaque path on an alpha format");
            target.fb[idx] = pack_out(target.format, px[0], px[1], px[2], 255);
            stats.pixels_opaque += 1;
            continue;
        }
        if px[3] == 255 && q == 255 {
            target.fb[idx] = pack_out(target.format, px[0], px[1], px[2], 255);
            stats.pixels_opaque += 1;
            continue;
        }
        if px[3] == 0 {
            continue; // Fully transparent source: keep the destination.
        }
        let d = unpack_out(target.format, target.fb[idx]);
        let out = over_premul(px, d, q);
        target.fb[idx] = pack_out(target.format, out[0], out[1], out[2], out[3]);
        stats.pixels_blended += 1;
    }
}

/// The scene-linear pipeline path (cross-description composites).
fn pipeline_span(
    target: &mut FrameTarget<'_>,
    span: &RowSpan,
    plan: &LayerPlan<'_>,
    stats: &mut RenderStats,
) {
    let qf = f32::from(plan.opacity_q) * (1.0 / 255.0);
    for x in span.x0..span.x1 {
        let Some((bx, by)) = plan.mapping.map(x, span.y) else {
            continue;
        };
        let idx = span.row_base + x as usize;
        let s = sample_straight_f32(plan.view, bx, by, plan.coeffs, plan.range);
        let sa = s[3] * qf;
        if sa <= 0.0 {
            continue; // Nothing contributes: keep the destination.
        }
        // Source: decode straight color to output-relative linear.
        let lin = plan.pipeline.decode_straight([s[0], s[1], s[2]]);
        let src = [lin[0] * sa, lin[1] * sa, lin[2] * sa, sa];
        // Destination: decode through the *target* side of the pipeline.
        let d = unpack_out(target.format, target.fb[idx]);
        let d_straight = [
            f32::from(d[0]) * (1.0 / 255.0),
            f32::from(d[1]) * (1.0 / 255.0),
            f32::from(d[2]) * (1.0 / 255.0),
        ];
        let d_lin = plan.pipeline.decode_target_straight(d_straight);
        let da = f32::from(d[3]) * (1.0 / 255.0);
        let dst = [d_lin[0] * da, d_lin[1] * da, d_lin[2] * da, da];
        // Premultiplied over in scene-linear.
        let inv = 1.0 - src[3];
        let oa = src[3] + dst[3] * inv;
        let o_rgb = [
            (src[0] + dst[0] * inv).clamp(0.0, 1.0),
            (src[1] + dst[1] * inv).clamp(0.0, 1.0),
            (src[2] + dst[2] * inv).clamp(0.0, 1.0),
        ];
        // Un-premultiply, encode, quantize; alpha stays linear.
        let enc = if oa > 0.0 {
            let inv_a = 1.0 / oa;
            let straight = [
                (o_rgb[0] * inv_a).clamp(0.0, 1.0),
                (o_rgb[1] * inv_a).clamp(0.0, 1.0),
                (o_rgb[2] * inv_a).clamp(0.0, 1.0),
            ];
            plan.pipeline.encode_straight(straight)
        } else {
            [0.0; 3]
        };
        target.fb[idx] = pack_out(
            target.format,
            quantize_f32(enc[0]),
            quantize_f32(enc[1]),
            quantize_f32(enc[2]),
            quantize_f32(oa),
        );
        stats.pixels_blended += 1;
    }
}

/// The Liquid corner path (Phase 27): the encoded-space blend with the
/// antialiased corner coverage folded into the source alpha first.
///
/// The fold order is pinned to the GL path's (coverage at upload, then
/// the opacity inside the blend) — `scale_premul` then `over_premul` —
/// so the two backends produce identical bytes on every pixel.
fn styled_alpha_span(
    target: &mut FrameTarget<'_>,
    span: &RowSpan,
    plan: &LayerPlan<'_>,
    stats: &mut RenderStats,
) {
    let q = plan.opacity_q;
    // Phase 29: the SDF runs only inside the row's corner bands; every
    // other pixel's coverage is exactly 255 (the straight-edge bulk).
    let band = corner_band_xs(plan.dest, plan.corner_radius, span.y);
    for x in span.x0..span.x1 {
        let Some((bx, by)) = plan.mapping.map(x, span.y) else {
            continue; // Scaled-edge miss: leave the destination alone.
        };
        let cov = if band.contains(x) {
            rounded_coverage(x, span.y, plan.dest, plan.corner_radius)
        } else {
            255
        };
        if cov == 0 {
            continue; // Outside the rounded ink: keep the destination.
        }
        let px = sample_premul_u8(plan.view, bx, by, plan.coeffs, plan.range);
        let idx = span.row_base + x as usize;
        if cov == 255 {
            // Interior pixel: the plain blend shortcuts, byte-identical.
            if px[3] == 255 && q == 255 {
                target.fb[idx] = pack_out(target.format, px[0], px[1], px[2], 255);
                stats.pixels_opaque += 1;
                continue;
            }
            if px[3] == 0 {
                continue;
            }
        }
        let scaled = scale_premul(px, cov);
        let d = unpack_out(target.format, target.fb[idx]);
        let out = over_premul(scaled, d, q);
        target.fb[idx] = pack_out(target.format, out[0], out[1], out[2], out[3]);
        stats.pixels_blended += 1;
    }
}

/// The Liquid corner path over the scene-linear pipeline: coverage
/// scales the source's straight alpha before the decode (a named
/// rounding difference from the encoded-space fold — the GL v1 subset
/// never runs this path, documented).
fn styled_pipeline_span(
    target: &mut FrameTarget<'_>,
    span: &RowSpan,
    plan: &LayerPlan<'_>,
    stats: &mut RenderStats,
) {
    let qf = f32::from(plan.opacity_q) * (1.0 / 255.0);
    // Phase 29: the SDF runs only inside the row's corner bands; every
    // other pixel's coverage is exactly 255 (the straight-edge bulk).
    let band = corner_band_xs(plan.dest, plan.corner_radius, span.y);
    for x in span.x0..span.x1 {
        let Some((bx, by)) = plan.mapping.map(x, span.y) else {
            continue;
        };
        let cov = if band.contains(x) {
            rounded_coverage(x, span.y, plan.dest, plan.corner_radius)
        } else {
            255
        };
        if cov == 0 {
            continue;
        }
        let cov_f = f32::from(cov) * (1.0 / 255.0);
        let idx = span.row_base + x as usize;
        let s = sample_straight_f32(plan.view, bx, by, plan.coeffs, plan.range);
        let sa = s[3] * qf * cov_f;
        if sa <= 0.0 {
            continue; // Nothing contributes: keep the destination.
        }
        // Source: decode straight color to output-relative linear.
        let lin = plan.pipeline.decode_straight([s[0], s[1], s[2]]);
        let src = [lin[0] * sa, lin[1] * sa, lin[2] * sa, sa];
        // Destination: decode through the *target* side of the pipeline.
        let d = unpack_out(target.format, target.fb[idx]);
        let d_straight = [
            f32::from(d[0]) * (1.0 / 255.0),
            f32::from(d[1]) * (1.0 / 255.0),
            f32::from(d[2]) * (1.0 / 255.0),
        ];
        let d_lin = plan.pipeline.decode_target_straight(d_straight);
        let da = f32::from(d[3]) * (1.0 / 255.0);
        let dst = [d_lin[0] * da, d_lin[1] * da, d_lin[2] * da, da];
        // Premultiplied over in scene-linear.
        let inv = 1.0 - src[3];
        let oa = src[3] + dst[3] * inv;
        let o_rgb = [
            (src[0] + dst[0] * inv).clamp(0.0, 1.0),
            (src[1] + dst[1] * inv).clamp(0.0, 1.0),
            (src[2] + dst[2] * inv).clamp(0.0, 1.0),
        ];
        // Un-premultiply, encode, quantize; alpha stays linear.
        let enc = if oa > 0.0 {
            let inv_a = 1.0 / oa;
            let straight = [
                (o_rgb[0] * inv_a).clamp(0.0, 1.0),
                (o_rgb[1] * inv_a).clamp(0.0, 1.0),
                (o_rgb[2] * inv_a).clamp(0.0, 1.0),
            ];
            plan.pipeline.encode_straight(straight)
        } else {
            [0.0; 3]
        };
        target.fb[idx] = pack_out(
            target.format,
            quantize_f32(enc[0]),
            quantize_f32(enc[1]),
            quantize_f32(enc[2]),
            quantize_f32(oa),
        );
        stats.pixels_blended += 1;
    }
}

/// Save a framebuffer region as canonical premultiplied words — the
/// frost pass's snapshot of what sits beneath the layer. Pixels outside
/// the output read as transparent (the blur's honest boundary).
///
/// Phase 29: the snapshot is a per-row word operation, not a per-pixel
/// unpack/pack round trip — the canonical word *is* the ARGB-family
/// output word (byte-equal by the field orders), the BGR family swaps
/// two channels, and the X family forces alpha; every other pixel of
/// the region reads as transparent zero.
pub(crate) fn save_region_into(target: &FrameTarget<'_>, rect: Rect, out: &mut Vec<u32>) {
    out.clear();
    out.resize((rect.w as usize) * (rect.h as usize), 0);
    let y0 = rect.y.max(0);
    let y1 = rect.bottom().min(target.height as i32);
    let x0 = rect.x.max(0);
    let x1 = rect.right().min(target.width as i32);
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let rw = rect.w as usize;
    // The region's rows above the output (a negative `rect.y`) read as
    // transparent: the output rows start at `y0 - rect.y`.
    let row_skip = (y0 - rect.y) as usize;
    for (y, out_row) in (y0..y1).zip(out.chunks_mut(rw).skip(row_skip)) {
        let src_row = &target.fb[(y as u32 * target.width) as usize..];
        let src_span = &src_row[x0 as usize..x1 as usize];
        let off = (x0 - rect.x) as usize;
        match target.format {
            FourCC::ARGB8888 => {
                // The canonical word is the ARGB word.
                out_row[off..off + src_span.len()].copy_from_slice(src_span);
            }
            FourCC::XRGB8888 => {
                // Alpha forced (the stored word already carries 0xFF in
                // the field; the OR is the same value).
                for (d, &s) in out_row[off..off + src_span.len()].iter_mut().zip(src_span) {
                    *d = s | 0xFF00_0000;
                }
            }
            FourCC::ABGR8888 => {
                // Swap R and B fields.
                for (d, &s) in out_row[off..off + src_span.len()].iter_mut().zip(src_span) {
                    *d = (s & 0xFF00_FF00) | ((s & 0x00FF_0000) >> 16) | ((s & 0x0000_00FF) << 16);
                }
            }
            FourCC::XBGR8888 => {
                for (d, &s) in out_row[off..off + src_span.len()].iter_mut().zip(src_span) {
                    let sw =
                        (s & 0xFF00_FF00) | ((s & 0x00FF_0000) >> 16) | ((s & 0x0000_00FF) << 16);
                    *d = sw | 0xFF00_0000;
                }
            }
            // Output formats are validated to the 32-bit family.
            _ => {}
        }
    }
}

/// Composite a prepared material (canonical premultiplied words over
/// `rect`, its own extent) into the framebuffer, clipped to damage —
/// the shadow and frost passes' one write path. The blend is the
/// shared integer `over` at full opacity, so the GL texture draw of the
/// same material lands on identical bytes.
///
/// Phase 29: the material row is served as a slice (no per-pixel
/// bounds check), leading and trailing fully-transparent runs are
/// skipped by scan (the holed ink's interior — the shadow's zero
/// middle — never touches the blend loop), and fully-opaque material
/// pixels word-write (an `over` with source alpha 255 returns the
/// source exactly, and the canonical word is the output-format word
/// for the A-family, a channel swap for the B family). Statistics keep
/// their Phase 27 meaning: every non-transparent pixel counts as
/// blended, exactly as the per-pixel loop counted it.
pub(crate) fn apply_material(
    target: &mut FrameTarget<'_>,
    index: &crate::spans::RowIndex,
    spans: &mut RowSpans,
    stats: &mut RenderStats,
    rect: Rect,
    material: &[u32],
) {
    if rect.is_empty() || material.is_empty() {
        return;
    }
    let rw = rect.w as usize;
    // The material covers its rect exactly (every in-crate caller
    // builds it that way); an undersized material draws nothing rather
    // than indexing past its end.
    debug_assert!(
        material.len() >= rw * rect.h as usize,
        "material covers its rect"
    );
    if material.len() < rw * rect.h as usize {
        return;
    }
    let y0 = rect.y.max(0);
    let y1 = rect.bottom().min(target.height as i32);
    for y in y0..y1 {
        let row = index.row(y);
        if row.is_empty() {
            continue;
        }
        RowSpans::clip(row, &rect, spans);
        if spans.is_empty() {
            continue;
        }
        let row_base = (y as u32 * target.width) as usize;
        let mat_row = &material[(y - rect.y) as usize * rw..(y - rect.y + 1) as usize * rw];
        let fb_row = &mut target.fb[row_base..row_base + target.width as usize];
        for &(x0, x1) in spans.intervals() {
            stats.pixels_effect += (x1 - x0) as u64;
            // The material-local span (in-bounds by the clip above).
            let m0 = (x0 - rect.x) as usize;
            let m1 = (x1 - rect.x) as usize;
            let seg = &mat_row[m0..m1];
            // Skip the leading and trailing transparent runs (the
            // holed ink) — transparent pixels never write.
            let Some(first) = seg.iter().position(|&w| w >= 0x0100_0000) else {
                continue; // The whole span is the hole.
            };
            let last = seg
                .iter()
                .rposition(|&w| w >= 0x0100_0000)
                .expect("first found implies last exists");
            for (i, &word) in seg[first..=last].iter().enumerate() {
                let x = x0 as usize + first + i;
                let a = (word >> 24) & 0xFF;
                if a == 0 {
                    continue; // Interior hole between the runs.
                }
                if a == 0xFF {
                    // Opaque material pixel: the word-write form of
                    // over_premul(px, d, 255) — the source itself.
                    fb_row[x] = pack_out(
                        target.format,
                        ((word >> 16) & 0xFF) as u8,
                        ((word >> 8) & 0xFF) as u8,
                        (word & 0xFF) as u8,
                        255,
                    );
                    stats.pixels_blended += 1;
                    continue;
                }
                let px = unpack_canonical(word);
                let d = unpack_out(target.format, fb_row[x]);
                let out = over_premul(px, d, 255);
                fb_row[x] = pack_out(target.format, out[0], out[1], out[2], out[3]);
                stats.pixels_blended += 1;
            }
        }
    }
}

/// Quantize a clamped `0..=1` float to u8 (round-half-up).
#[inline]
fn quantize_f32(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5).floor() as u8
}

/// Pack premultiplied RGBA into an output-format word.
#[inline]
#[must_use]
pub(crate) fn pack_out(fmt: FourCC, r: u8, g: u8, b: u8, a: u8) -> u32 {
    let (r, g, b, a) = (u32::from(r), u32::from(g), u32::from(b), u32::from(a));
    match fmt {
        FourCC::ARGB8888 => (a << 24) | (r << 16) | (g << 8) | b,
        FourCC::XRGB8888 => 0xFF00_0000 | (r << 16) | (g << 8) | b,
        FourCC::ABGR8888 => (a << 24) | (b << 16) | (g << 8) | r,
        FourCC::XBGR8888 => 0xFF00_0000 | (b << 16) | (g << 8) | r,
        // Output formats are validated to the 32-bit family.
        _ => 0,
    }
}

/// Unpack an output-format word into premultiplied RGBA.
#[inline]
#[must_use]
pub(crate) fn unpack_out(fmt: FourCC, v: u32) -> [u8; 4] {
    let chan = |shift: u32| ((v >> shift) & 0xFF) as u8;
    match fmt {
        FourCC::ARGB8888 => [chan(16), chan(8), chan(0), chan(24)],
        FourCC::XRGB8888 => [chan(16), chan(8), chan(0), 255],
        FourCC::ABGR8888 => [chan(0), chan(8), chan(16), chan(24)],
        FourCC::XBGR8888 => [chan(0), chan(8), chan(16), 255],
        _ => [0, 0, 0, 0],
    }
}

/// Whether a layer can take the write-only opaque path.
pub(crate) fn opaque_path_ok(format: FourCC, opacity_q: u8) -> bool {
    opacity_q == 255 && !format_has_alpha(format)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_unpack_round_trips_all_output_formats() {
        for fmt in [
            FourCC::XRGB8888,
            FourCC::ARGB8888,
            FourCC::XBGR8888,
            FourCC::ABGR8888,
        ] {
            let v = pack_out(fmt, 0x12, 0x34, 0x56, 0x78);
            let px = unpack_out(fmt, v);
            if fmt == FourCC::XRGB8888 || fmt == FourCC::XBGR8888 {
                assert_eq!(px[3], 255, "{fmt:?} forces alpha");
            } else {
                assert_eq!(px[3], 0x78);
            }
            assert_eq!(px[0], 0x12, "{fmt:?} r");
            assert_eq!(px[1], 0x34, "{fmt:?} g");
            assert_eq!(px[2], 0x56, "{fmt:?} b");
        }
    }
}
