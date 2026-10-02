//! The Liquid effect kernels (Phase 27): the pure pixel mathematics of
//! the macOS-class visual language.
//!
//! Every function here is **backend-neutral**: it operates on
//! *canonical* premultiplied ARGB words (`a<<24 | r<<16 | g<<8 | b`)
//! and is shared by the software renderer and the GL path (whose
//! texture uploads, shadow draws and frost materials are all computed
//! through these same kernels — the byte-equality oracle of
//! `tests/gles_styled.rs`).
//!
//! Determinism doctrine (the crate's): no clocks, no hashmaps, and
//! every rounding rule named —
//!
//! * [`rounded_coverage`] uses only IEEE-754 exactly-specified
//!   operations (`+`, `*`, `abs`, `max`, `min`, correctly-rounded
//!   `sqrt`) on f32 — bit-stable on every platform that implements
//!   IEEE single precision (which is all of them, by law of the
//!   standard);
//! * the blend kernel [`over_premul`] is the crate's shared integer
//!   premultiplied `over` (`mul255`, round-half-up, alpha-clamped
//!   channel sums) — the *same rule* as the software composite path
//!   and the GL reference evaluator, factored here so the three call
//!   sites cannot drift;
//! * the box blur [`blur_words`] is a sliding-window integer mean with
//!   the named rule `out = (sum + r) / (2r + 1)` per channel.
//!
//! # The material doctrine
//!
//! The system owns the materials (macOS's rule): clients draw content,
//! the compositor adds the rounded corners
//! ([`rounded_coverage`]), the soft shadow
//! ([`shadow_material`]), the frosted backdrop
//! ([`frost_material`]) — Phase 40's vibrant domain included — and
//! the luminous hairline ([`edge_light_material`]) around it. See
//! `style.rs` for the vocabulary and the low-end quality tiers.

use ldp_core::geometry::Rect;

use crate::style::{BackdropParams, EdgeLightParams, ShadowParams};

/// Fixed-point multiply `v * a / 255` with round-half-up — the one
/// blending rounding rule every path in this crate shares (the
/// composite path's own `mul255`, restated).
#[inline]
#[must_use]
pub const fn mul255(v: u8, a: u8) -> u8 {
    // `as` casts: `From` is not const-callable (ldp-core's precedent).
    ((v as u32 * a as u32 + 127) / 255) as u8
}

/// Linear interpolation `a + (b - a) * t / 255` in the shared
/// fixed-point discipline. `t = 0` is `a`, `t = 255` is `b` exactly.
#[inline]
#[must_use]
pub const fn lerp255(a: u8, b: u8, t: u8) -> u8 {
    // Written as a weighted sum so negative differences never appear.
    ((a as u32 * (255 - t as u32) + b as u32 * t as u32 + 127) / 255) as u8
}

/// Pack a premultiplied RGBA quadruple into a canonical word.
#[inline]
#[must_use]
pub const fn pack_canonical(r: u8, g: u8, b: u8, a: u8) -> u32 {
    (a as u32) << 24 | (r as u32) << 16 | (g as u32) << 8 | b as u32
}

/// Unpack a canonical word into premultiplied RGBA.
#[inline]
#[must_use]
pub const fn unpack_canonical(word: u32) -> [u8; 4] {
    [
        ((word >> 16) & 0xFF) as u8,
        ((word >> 8) & 0xFF) as u8,
        (word & 0xFF) as u8,
        ((word >> 24) & 0xFF) as u8,
    ]
}

/// The shared integer premultiplied `over`: `px` over `d` with the
/// source scaled by `q` first.
///
/// This is the formula the software composite kernel, the GL reference
/// evaluator's `blend_word`, and this module's material draws all
/// execute — factored to one place so styled and unstyled paths cannot
/// drift apart. Channel sums live in `u32` and clamp to the result
/// alpha; every product rounds through the crate's `mul255` rule.
#[inline]
#[must_use]
pub fn over_premul(px: [u8; 4], d: [u8; 4], q: u8) -> [u8; 4] {
    let a2 = mul255(px[3], q);
    let inv = 255 - a2;
    let oa = u32::from(a2) + u32::from(mul255(d[3], inv));
    let chan =
        |i: usize| (u32::from(mul255(px[i], q)) + u32::from(mul255(d[i], inv))).min(oa) as u8;
    [chan(0), chan(1), chan(2), oa as u8]
}

/// Scale a premultiplied pixel by an antialiasing coverage factor —
/// the corner-clip fold. `cov = 255` is the identity (proven:
/// `mul255(v, 255) == v` for every `v`), so fully-covered interior
/// pixels keep their exact values.
#[inline]
#[must_use]
pub fn scale_premul(px: [u8; 4], cov: u8) -> [u8; 4] {
    [
        mul255(px[0], cov),
        mul255(px[1], cov),
        mul255(px[2], cov),
        mul255(px[3], cov),
    ]
}

/// The antialiased coverage of the rounded rectangle `rect` (radius
/// `radius`) at pixel `(x, y)` — `255` inside, `0` outside, the
/// fractional band across the edge.
///
/// The signed distance function is the canonical rounded-rect SDF:
/// with `q = |p - c| - (b - r)` (pixel center `p`, rect center `c`,
/// half-extents `b`, corner radius `r`),
/// `d = length(max(q, 0)) + min(max(q.x, q.y), 0) - r`, and coverage
/// is the half-pixel kernel `clamp(0.5 - d, 0, 1)` quantized with
/// round-half-up. Every operation is IEEE-754 exact or correctly
/// rounded (`sqrt` included) — bit-stable everywhere.
#[must_use]
pub fn rounded_coverage(x: i32, y: i32, rect: Rect, radius: u32) -> u8 {
    if radius == 0 {
        // The degenerate SDF: plain rectangle coverage against pixel
        // centers (still antialiased on the half-pixel kernel).
        let inside =
            x >= rect.x && y >= rect.y && x < rect.x + rect.w as i32 && y < rect.y + rect.h as i32;
        return if inside { 255 } else { 0 };
    }
    let (rw, rh) = (rect.w as f32, rect.h as f32);
    let r = radius as f32;
    let cx = rect.x as f32 + rw * 0.5;
    let cy = rect.y as f32 + rh * 0.5;
    // Half-extents of the straight part (b - r), floored at zero so a
    // radius exceeding min(w,h)/2 degrades to a stadium, never NaN.
    let hx = (rw * 0.5 - r).max(0.0);
    let hy = (rh * 0.5 - r).max(0.0);
    let px = x as f32 + 0.5;
    let py = y as f32 + 0.5;
    let qx = (px - cx).abs() - hx;
    let qy = (py - cy).abs() - hy;
    let ox = qx.max(0.0);
    let oy = qy.max(0.0);
    let outside = (ox * ox + oy * oy).sqrt();
    let inside = qx.max(qy).min(0.0);
    let d = outside + inside - r;
    let cov = (0.5 - d).clamp(0.0, 1.0);
    (cov * 255.0 + 0.5) as u8
}

/// How the box blur treats samples outside the buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlurEdge {
    /// Replicate the edge sample — the frost mode (the backdrop
    /// conceptually continues).
    Clamp,
    /// Treat outside as transparent zero — the shadow mode (the
    /// silhouette's energy spreads and fades).
    Transparent,
}

/// The x-intervals of one row where a rounded rectangle's coverage can
/// be fractional (anything but the bulk 255-inside / 0-outside).
///
/// The geometry (Phase 29's realized win): pixel centers sit at
/// `x + 0.5` against straight edges at integer coordinates, so the
/// distance to a straight edge is always exactly `k + 0.5` — the
/// coverage there is always exactly 255 or 0, never fractional.
/// Fractional coverage requires the *arcs*, whose reach is the corner
/// squares of side `radius + 2`. A 1080×2340 window at radius 60 has
/// 0.4% of its pixels in the bands — the other 99.6% never pay the
/// SDF's `sqrt` again.
///
/// The bands are deliberately conservative (the SDF stays the oracle
/// inside them), so byte-equality holds by construction: pixels
/// outside the bands take the bulk value
/// (containment ? 255 : 0), which is exactly what
/// `rounded_coverage` returns there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CornerXBand {
    /// The left corner band `[x0, x1)` (empty when `x0 >= x1`).
    pub left: (i32, i32),
    /// The right corner band `[x0, x1)` (empty when `x0 >= x1`).
    pub right: (i32, i32),
}

impl CornerXBand {
    /// Whether both bands are empty — the row carries no fractional
    /// coverage anywhere.
    pub(crate) fn is_empty(&self) -> bool {
        self.left.0 >= self.left.1 && self.right.0 >= self.right.1
    }

    /// Whether the column `x` falls into either band.
    pub(crate) fn contains(&self, x: i32) -> bool {
        (x >= self.left.0 && x < self.left.1) || (x >= self.right.0 && x < self.right.1)
    }

    /// The two bands as at most two **disjoint** intervals, ordered.
    ///
    /// On narrow rects the corner bands overlap or nest (their combined
    /// width `2r + 4` can exceed the row) — walking `left` then `right`
    /// naively would visit the intersection twice, double-applying any
    /// read-modify-write there. The merged walk visits every band
    /// column exactly once.
    pub(crate) fn merged(&self) -> [(i32, i32); 2] {
        let (a, b) = (self.left, self.right);
        let (lo, hi) = if a.0 <= b.0 { (a, b) } else { (b, a) };
        if hi.0 <= lo.1 {
            // Overlapping or touching: one merged interval (nested
            // bands collapse to the wider one).
            let end = lo.1.max(hi.1);
            if lo.0 >= end {
                [(0, 0), (0, 0)]
            } else {
                [(lo.0, end), (0, 0)]
            }
        } else {
            [lo, hi]
        }
    }
}

/// The corner-arc bands of row `y` for the rounded rect (`rect`,
/// `radius`): empty unless the row is within `radius + 2` of the top
/// or bottom edge (the arcs' vertical reach, with margin).
///
/// Rows outside the bands need no SDF at all — their coverage is the
/// bulk test (containment: 255 in, 0 out).
///
/// Two regimes, honestly split:
///
/// * **tight** (`2·radius ≤ min(w, h)` — every sanitized style): the
///   straight edges sit at integer coordinates (pixel centers are
///   always exactly half a pixel away — never fractional) and the
///   quarter-arcs stay inside their corner squares, so the tight bands
///   above carry every fractional pixel. This is the production shape.
/// * **degenerate** (a larger radius — reachable through the public
///   API, never through a sanitized style): the SDF clamps the
///   half-extents to zero, the shape becomes a capsule that can bulge
///   past the rect on every side, and on odd extents its straight
///   edges land on half-integer coordinates where coverage goes
///   fractional anywhere. The bands then cover the shape's whole
///   inflated bounding box — conservative, never wrong (the SDF stays
///   the oracle inside the bands).
#[must_use]
pub(crate) fn corner_band_xs(rect: Rect, radius: u32, y: i32) -> CornerXBand {
    let empty = CornerXBand {
        left: (0, 0),
        right: (0, 0),
    };
    if radius == 0 {
        return empty; // no arcs: the rect test is the coverage
    }
    let r = i64::from(radius);
    let top = i64::from(rect.y);
    let bottom = i64::from(rect.y) + i64::from(rect.h);
    let left = i64::from(rect.x);
    let right = i64::from(rect.x) + i64::from(rect.w);
    let row = i64::from(y);
    let clamp_i32 = |v: i64| v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
    if 2 * r > i64::from(rect.w.min(rect.h)) {
        // The degenerate regime: one full-width band over the shape's
        // inflated bounding box (the capsule bulges up to `radius`
        // past every side).
        let lo = top - r - 2;
        let hi = bottom + r + 2;
        if row < lo || row > hi {
            return empty;
        }
        return CornerXBand {
            left: (clamp_i32(left - r - 2), clamp_i32(right + r + 2)),
            right: (0, 0),
        };
    }
    // The tight regime: the arcs' vertical reach is the corner squares
    // (centers within 0.5 of an arc point, ±2 of margin).
    let near_top = row >= top - 2 && row <= top + r + 1;
    let near_bottom = row >= bottom - r - 2 && row <= bottom + 1;
    if !near_top && !near_bottom {
        return empty;
    }
    CornerXBand {
        left: (clamp_i32(left - 2), clamp_i32(left + r + 2)),
        right: (clamp_i32(right - r - 2), clamp_i32(right + 2)),
    }
}

/// Fold the rounded-corner coverage into a canonical-word material
/// (`w` × `h`): every word outside the corner bands keeps its value
/// (coverage 255 — the identity), band pixels scale by their
/// antialiased coverage (coverage 0 zeroes the word).
///
/// Byte-equal to the per-pixel fold the Phase 27 software and GL paths
/// ran (the golden suites pin it): the SDF is still the oracle inside
/// the bands, and outside them the coverage is exactly 255, where
/// `scale_premul(px, 255) == px`.
pub(crate) fn apply_corner_fold(words: &mut [u32], w: u32, h: u32, radius: u32) {
    if radius == 0 || w == 0 || h == 0 {
        return;
    }
    debug_assert_eq!(words.len(), (w as usize) * (h as usize));
    let rect = Rect::new(0, 0, w, h);
    for y in 0..h as i32 {
        let band = corner_band_xs(rect, radius, y);
        if band.is_empty() {
            continue;
        }
        let row_base = y as usize * w as usize;
        for (x0, x1) in band.merged() {
            let x0 = x0.max(0);
            let x1 = x1.min(w as i32);
            for x in x0..x1 {
                let cov = rounded_coverage(x, y, rect, radius);
                if cov == 255 {
                    continue;
                }
                let idx = row_base + x as usize;
                let px = unpack_canonical(words[idx]);
                let scaled = scale_premul(px, cov);
                words[idx] = pack_canonical(scaled[0], scaled[1], scaled[2], scaled[3]);
            }
        }
    }
}

/// Separable box blur over canonical premultiplied words.
///
/// `passes` rounds of (horizontal, vertical) sliding-window means of
/// half-extent `radius`; three passes approximate a Gaussian. The
/// per-channel rounding rule is `out = (sum + r) / (2r + 1)`.
///
/// Normalization is the **full window** (a true convolution):
/// [`BlurEdge::Transparent`] pads with zeros — energy spreads and
/// fades, the shadow semantics — while [`BlurEdge::Clamp`] replicates
/// edge samples — a constant field blurs to itself, the frost
/// semantics. The cost is O(pixels) per pass regardless of `radius`:
/// the window slides, it never rescans.
///
/// Blurring **premultiplied** channels is what makes translucent
/// regions blur correctly (straight-alpha blurs fringe toward black).
///
/// Phase 29 kernel notes (byte-equal to the original, proven by the
/// reference cross-suite in the tests): the interior of every row and
/// column takes an edge-free tight loop (the `Option`/clamp plumbing
/// only runs within `radius` of a border), the vertical pass walks
/// column strips so each row touch reads one contiguous slice (the
/// old column walk paid a cache line per row), the window mean's
/// division is an exact 48-bit reciprocal multiply for the sanitized
/// radius domain (`win ≤ 257`; larger radii keep the plain divide),
/// and callers with retained scratch can drive the crate's
/// in-place pass entry point directly for a zero-allocation steady
/// state.
#[must_use]
pub fn blur_words(
    words: &[u32],
    width: u32,
    height: u32,
    radius: u32,
    passes: u32,
    edge: BlurEdge,
) -> Vec<u32> {
    let n = words.len();
    if n == 0 || radius == 0 || passes == 0 || width == 0 || height == 0 {
        return words.to_vec();
    }
    debug_assert_eq!(n, (width as usize) * (height as usize));
    let mut a = words.to_vec();
    let mut b = vec![0u32; n];
    blur_passes(
        &mut a,
        &mut b,
        width as usize,
        height as usize,
        radius as usize,
        passes,
        edge,
    );
    a
}

/// The in-place separable blur over two caller-owned ping-pong buffers
/// (the retained-scratch entry point the frost memo and the shadow
/// builder use — no allocation per call).
///
/// `a` carries the input and receives the output (the pass count is
/// even on the ping-pong: horizontal + vertical per pass); `b` is
/// scratch of the same length. Both must be exactly `width * height`
/// words long and disjoint.
///
/// # Panics
/// Debug builds assert the two lengths and the disjointness (the
/// caller owns the buffers; the public [`blur_words`] satisfies this
/// by construction).
pub(crate) fn blur_passes(
    current: &mut Vec<u32>,
    scratch: &mut Vec<u32>,
    w: usize,
    h: usize,
    r: usize,
    passes: u32,
    edge: BlurEdge,
) {
    debug_assert_eq!(current.len(), w * h);
    debug_assert_eq!(scratch.len(), w * h);
    debug_assert_ne!(current.as_ptr(), scratch.as_ptr());
    let win = 2 * r + 1;
    let half = r as u32;
    // The exact 48-bit reciprocal of the (odd) window: `m = ceil(2^48 /
    // win)`, so `(n * m) >> 48 == n / win` for every `n ≤ 65663` — the
    // full sum domain of a sanitized window (`255 × 257 + 128`), proven
    // exhaustively by `reciprocal_division_is_exact` in the tests.
    // `win > 257` keeps the plain divide — only reachable through the
    // public API, never the sanitized material paths.
    let fast_div = if win <= 257 {
        Some(((1u64 << 48) - 1) / win as u64 + 1)
    } else {
        None
    };
    for _ in 0..passes {
        blur_horizontal(current, scratch, w, r, half, win, fast_div, edge);
        core::mem::swap(current, scratch);
        blur_vertical(current, scratch, w, h, r, half, win, fast_div, edge);
        core::mem::swap(current, scratch);
    }
}

/// The horizontal pass: rows read contiguously, written contiguously.
#[allow(clippy::too_many_arguments)] // the pass's shared invariants, hoisted once
fn blur_horizontal(
    src: &[u32],
    dst: &mut [u32],
    w: usize,
    r: usize,
    half: u32,
    win: usize,
    fast_div: Option<u64>,
    edge: BlurEdge,
) {
    for row in 0..(src.len() / w) {
        let base = row * w;
        let (mut sr, mut sg, mut sb, mut sa) = (0u32, 0u32, 0u32, 0u32);
        for i in 0..win {
            if let Some(sx) = sample_index(i as i64 - r as i64, w, edge) {
                add_window(&mut sr, &mut sg, &mut sb, &mut sa, src[base + sx]);
            }
        }
        for x in 0..w {
            dst[base + x] = mean_word_fast(sr, sg, sb, sa, half, win, fast_div);
            if x + 1 == w {
                break; // the last write needs no slide
            }
            let enter = x as i64 + r as i64 + 1;
            let leave = x as i64 - r as i64;
            if leave >= 0 && enter < w as i64 {
                // Interior: both samples are plain indices.
                add_window(
                    &mut sr,
                    &mut sg,
                    &mut sb,
                    &mut sa,
                    src[base + enter as usize],
                );
                sub_window(
                    &mut sr,
                    &mut sg,
                    &mut sb,
                    &mut sa,
                    src[base + leave as usize],
                );
            } else {
                let enter_s = sample_index(enter, w, edge);
                let leave_s = sample_index(leave, w, edge);
                if enter_s != leave_s {
                    if let Some(e) = enter_s {
                        add_window(&mut sr, &mut sg, &mut sb, &mut sa, src[base + e]);
                    }
                    if let Some(l) = leave_s {
                        sub_window(&mut sr, &mut sg, &mut sb, &mut sa, src[base + l]);
                    }
                }
            }
        }
    }
}

/// The number of columns one vertical-pass strip walks: 16 words is
/// two cache lines — each row touch of the strip reads one contiguous
/// slice instead of the old per-column stride (one cache line *per
/// row* per column).
const BLUR_STRIP: usize = 16;

/// The vertical pass, strip-wise: sixteen columns slide together, so
/// every row reads and writes contiguous 64-byte slices.
#[allow(clippy::too_many_arguments)] // the pass's shared invariants, hoisted once
fn blur_vertical(
    src: &[u32],
    dst: &mut [u32],
    w: usize,
    h: usize,
    r: usize,
    half: u32,
    win: usize,
    fast_div: Option<u64>,
    edge: BlurEdge,
) {
    let mut sr = [0u32; BLUR_STRIP];
    let mut sg = [0u32; BLUR_STRIP];
    let mut sb = [0u32; BLUR_STRIP];
    let mut sa = [0u32; BLUR_STRIP];
    for x0 in (0..w).step_by(BLUR_STRIP) {
        let x1 = (x0 + BLUR_STRIP).min(w);
        let cols = x1 - x0;
        for v in sr.iter_mut().take(cols) {
            *v = 0;
        }
        for v in sg.iter_mut().take(cols) {
            *v = 0;
        }
        for v in sb.iter_mut().take(cols) {
            *v = 0;
        }
        for v in sa.iter_mut().take(cols) {
            *v = 0;
        }
        for i in 0..win {
            if let Some(sy) = sample_index(i as i64 - r as i64, h, edge) {
                let row = &src[sy * w + x0..sy * w + x1];
                for (c, &word) in row.iter().enumerate() {
                    add_window(&mut sr[c], &mut sg[c], &mut sb[c], &mut sa[c], word);
                }
            }
        }
        for y in 0..h {
            let dst = &mut dst[y * w + x0..y * w + x1];
            for (c, d) in dst.iter_mut().enumerate() {
                *d = mean_word_fast(sr[c], sg[c], sb[c], sa[c], half, win, fast_div);
            }
            if y + 1 == h {
                break; // the last write needs no slide
            }
            let enter = y as i64 + r as i64 + 1;
            let leave = y as i64 - r as i64;
            if leave >= 0 && enter < h as i64 {
                let er = &src[enter as usize * w + x0..enter as usize * w + x1];
                let lr = &src[leave as usize * w + x0..leave as usize * w + x1];
                for c in 0..cols {
                    add_window(&mut sr[c], &mut sg[c], &mut sb[c], &mut sa[c], er[c]);
                    sub_window(&mut sr[c], &mut sg[c], &mut sb[c], &mut sa[c], lr[c]);
                }
            } else {
                let enter_s = sample_index(enter, h, edge);
                let leave_s = sample_index(leave, h, edge);
                if enter_s != leave_s {
                    if let Some(e) = enter_s {
                        let row = &src[e * w + x0..e * w + x1];
                        for (c, &word) in row.iter().enumerate() {
                            add_window(&mut sr[c], &mut sg[c], &mut sb[c], &mut sa[c], word);
                        }
                    }
                    if let Some(l) = leave_s {
                        let row = &src[l * w + x0..l * w + x1];
                        for (c, &word) in row.iter().enumerate() {
                            sub_window(&mut sr[c], &mut sg[c], &mut sb[c], &mut sa[c], word);
                        }
                    }
                }
            }
        }
    }
}

/// Add one window sample to the four channel sums.
#[inline]
fn add_window(sr: &mut u32, sg: &mut u32, sb: &mut u32, sa: &mut u32, word: u32) {
    *sr += (word >> 16) & 0xFF;
    *sg += (word >> 8) & 0xFF;
    *sb += word & 0xFF;
    *sa += (word >> 24) & 0xFF;
}

/// Remove one window sample from the four channel sums.
#[inline]
fn sub_window(sr: &mut u32, sg: &mut u32, sb: &mut u32, sa: &mut u32, word: u32) {
    *sr -= (word >> 16) & 0xFF;
    *sg -= (word >> 8) & 0xFF;
    *sb -= word & 0xFF;
    *sa -= (word >> 24) & 0xFF;
}

/// One blurred output word from the window's channel sums — the exact
/// `(sum + half) / win` rule, via the 48-bit reciprocal when the
/// window is small enough for the proof's domain.
#[inline]
fn mean_word_fast(
    sr: u32,
    sg: u32,
    sb: u32,
    sa: u32,
    half: u32,
    win: usize,
    fast_div: Option<u64>,
) -> u32 {
    match fast_div {
        Some(m48) => {
            let div = |n: u32| (((u64::from(n) * m48) >> 48) as u32) as u8;
            pack_canonical(
                div(sr + half),
                div(sg + half),
                div(sb + half),
                div(sa + half),
            )
        }
        None => mean_word(sr, sg, sb, sa, half, win),
    }
}

/// One blurred output word from the window's channel sums.
#[inline]
fn mean_word(sr: u32, sg: u32, sb: u32, sa: u32, half: u32, win: usize) -> u32 {
    pack_canonical(
        ((sr + half) / win as u32) as u8,
        ((sg + half) / win as u32) as u8,
        ((sb + half) / win as u32) as u8,
        ((sa + half) / win as u32) as u8,
    )
}

/// Resolve one blurred sample index under an edge policy; `None` is a
/// zero sample (transparent) — clamp never returns `None`.
#[inline]
fn sample_index(i: i64, extent: usize, edge: BlurEdge) -> Option<usize> {
    match edge {
        BlurEdge::Clamp => Some(i.clamp(0, extent as i64 - 1) as usize),
        BlurEdge::Transparent => u32::try_from(i)
            .ok()
            .and_then(|u| usize::try_from(u).ok())
            .filter(|&u| u < extent),
    }
}

/// The shadow material: a blurred rounded silhouette, holed where the
/// layer's own ink sits.
///
/// Returns `(words, box)` — the material over its output-space `box`
/// (the destination translated by the shadow offset and padded by
/// `blur * passes` on every side; the honest extent of the spread).
///
/// The hole is the layer's rounded ink shape: a translucent layer must
/// not see its own shadow through the material (the macOS rule), while
/// the corner gaps outside the ink still show the soft fringe. The
/// hole is subtracted **after** the blur so the fringe keeps the full
/// silhouette's energy.
///
/// Phase 29: both coverage passes are band-limited (the bulk interior
/// is a constant fill — what the per-pixel SDF returned there anyway —
/// and only the corner-arc bands run the SDF; see
/// the corner-band helper), and the blur runs in place over the material's
/// own words. Byte-equal to the Phase 27 loops (the reference
/// cross-suite in the tests pins it).
#[must_use]
pub fn shadow_material(params: &ShadowParams, dest: Rect, ink_radius: u32) -> (Vec<u32>, Rect) {
    let pad = params.blur.saturating_mul(params.passes);
    let box_x = dest.x + params.offset.0 - pad as i32;
    let box_y = dest.y + params.offset.1 - pad as i32;
    let box_w = dest.w + 2 * pad;
    let box_h = dest.h + 2 * pad;
    let box_rect = Rect::new(box_x, box_y, box_w, box_h);
    // The silhouette (and the ink hole) live in box-local coordinates.
    let silhouette = Rect::new(
        dest.x + params.offset.0 - box_x,
        dest.y + params.offset.1 - box_y,
        dest.w,
        dest.h,
    );
    let ink = Rect::new(dest.x - box_x, dest.y - box_y, dest.w, dest.h);

    // The bulk interior word: coverage 255 everywhere the SDF would
    // return it, so the fill and the SDF agree byte-for-byte.
    let interior = pack_canonical(
        mul255(params.color[0], params.alpha),
        mul255(params.color[1], params.alpha),
        mul255(params.color[2], params.alpha),
        params.alpha,
    );

    let mut words = vec![0u32; (box_w as usize) * (box_h as usize)];
    let bw = box_w as usize;
    // The silhouette pass: bulk fill per row, SDF only in the bands.
    for y in 0..box_h as i32 {
        let row = &mut words[y as usize * bw..(y + 1) as usize * bw];
        fill_bulk(row, silhouette, y, interior);
        let band = corner_band_xs(silhouette, params.radius, y);
        if band.is_empty() {
            continue;
        }
        for (x0, x1) in band.merged() {
            for x in x0.max(0)..x1.min(box_w as i32) {
                let cov = rounded_coverage(x, y, silhouette, params.radius);
                let a_eff = mul255(params.alpha, cov);
                row[x as usize] = pack_canonical(
                    mul255(params.color[0], a_eff),
                    mul255(params.color[1], a_eff),
                    mul255(params.color[2], a_eff),
                    a_eff,
                );
            }
        }
    }
    if params.blur > 0 && params.passes > 0 {
        let mut scratch = vec![0u32; words.len()];
        blur_passes(
            &mut words,
            &mut scratch,
            bw,
            box_h as usize,
            params.blur as usize,
            params.passes,
            BlurEdge::Transparent,
        );
    }
    // Hole the ink: the layer's own footprint carries no shadow. Bulk
    // rows zero the ink span; band rows zero the *non-band* span and
    // let the SDF decide the arcs (the bulk fill must not precede the
    // SDF on a band pixel — the fringe there scales the pre-hole word).
    for y in 0..box_h as i32 {
        let row = &mut words[y as usize * bw..(y + 1) as usize * bw];
        let band = corner_band_xs(ink, ink_radius, y);
        let row_in = y >= ink.y && y < ink.y + ink.h as i32;
        let in_x0 = ink.x.max(0);
        let in_x1 = (ink.x + ink.w as i32).min(box_w as i32);
        if band.is_empty() {
            if row_in && in_x0 < in_x1 {
                row[in_x0 as usize..in_x1 as usize].fill(0);
            }
            continue;
        }
        if row_in {
            for x in in_x0..in_x1 {
                if band.contains(x) {
                    continue; // The SDF decides below.
                }
                row[x as usize] = 0; // Bulk: ink coverage 255.
            }
        }
        for (x0, x1) in band.merged() {
            for x in x0.max(0)..x1.min(box_w as i32) {
                let cov = rounded_coverage(x, y, ink, ink_radius);
                if cov == 255 {
                    row[x as usize] = 0;
                } else if cov > 0 {
                    let px = unpack_canonical(row[x as usize]);
                    let kept = scale_premul(px, 255 - cov);
                    row[x as usize] = pack_canonical(kept[0], kept[1], kept[2], kept[3]);
                }
            }
        }
    }
    (words, box_rect)
}

/// Fill one row's bulk interior: `value` across `rect`'s x span on
/// rows the rect covers (what the per-pixel coverage loop wrote there).
#[inline]
fn fill_bulk(row: &mut [u32], rect: Rect, y: i32, value: u32) {
    if y < rect.y || y >= rect.y + rect.h as i32 {
        return;
    }
    let x0 = rect.x.max(0) as usize;
    let x1 = (rect.x + rect.w as i32).min(row.len() as i32) as usize;
    if x0 < x1 {
        row[x0..x1].fill(value);
    }
}

/// The frost material: the saved backdrop blurred, desaturated towards
/// its luminance, and veiled in the tint — what a translucent layer
/// shows through.
///
/// `saved` holds the backdrop's canonical words over exactly the
/// layer's destination (`w` x `h`). The result replaces those pixels'
/// view: on an opaque backdrop the material is opaque (the blur is all
/// the eye sees), which is the frosted-glass look; on a translucent
/// backdrop the replacement is proportional.
#[must_use]
pub fn frost_material(saved: &[u32], width: u32, height: u32, params: &BackdropParams) -> Vec<u32> {
    let mut out = Vec::new();
    let (mut blur_a, mut blur_b) = (Vec::new(), Vec::new());
    frost_material_into(
        saved,
        width,
        height,
        params,
        &mut blur_a,
        &mut blur_b,
        &mut out,
    );
    out
}

/// The frost material into caller-retained buffers (the zero-alloc
/// steady-state path the frost memo drives).
///
/// Phase 29 fast paths, byte-equal to the Phase 27 loop (the reference
/// cross-suite pins it):
///
/// * the **veil** is constant per material, so its three
///   `mul255(tint, 255)` products fold into constants (each channel
///   becomes one multiply instead of two — `mul255(v, 255) == v` is
///   the identity that makes it exact);
/// * **full saturation** (`saturation == 255`) skips the luma/lerp
///   entirely (`lerp255(a, b, 0) == a` exactly);
/// * a **transparent veil** (`tint_alpha == 0`) skips the over (an
///   invisible `over` returns its input exactly) — with no blur either
///   (the Low tier) the whole material is the backdrop, unchanged.
///
/// Phase 40's vibrant order — blur, **boost**, veil — keeps the
/// Phase 27 desaturation arm byte-identical (the reference cross-suite
/// pins that too) and adds the boost arm: `saturation > 255` extends
/// each channel *away* from its luma by `(saturation - 255) / 255` of
/// the distance, rounded half away from zero, clamped to the
/// premultiplied domain by the veil's existing channel clamps.
pub(crate) fn frost_material_into(
    saved: &[u32],
    width: u32,
    height: u32,
    params: &BackdropParams,
    blur_a: &mut Vec<u32>,
    blur_b: &mut Vec<u32>,
    out: &mut Vec<u32>,
) {
    out.clear();
    let n = saved.len();
    if n == 0 {
        return;
    }
    debug_assert_eq!(n, (width as usize) * (height as usize));
    // The blur: in place over the retained ping-pong pair.
    blur_a.clear();
    blur_a.extend_from_slice(saved);
    if params.blur > 0 && params.passes > 0 {
        blur_b.clear();
        blur_b.resize(n, 0);
        blur_passes(
            blur_a,
            blur_b,
            width as usize,
            height as usize,
            params.blur as usize,
            params.passes,
            BlurEdge::Clamp,
        );
    }
    // The saturation dial, both sides of identity (Phase 40): towards
    // luma by `255 - saturation` (the Phase 27 desaturation), or away
    // from it by `saturation - 255` (the vibrant boost — the acrylic
    // distance, the giants' backdrop-blur depth).
    let sat = u32::from(params.saturation);
    let (desat_amount, boost) = if sat <= 255 {
        (255 - sat, 0)
    } else {
        (0, sat - 255)
    };
    let desat = desat_amount as u8;
    let ta = params.tint_alpha;
    // The veil's constants: the veil pixel is the tint premultiplied by
    // its alpha (`mul255(tint_c, tint_alpha)`), and over_premul's
    // `mul255(veil_c, 255) == veil_c` keeps it whole — so each channel
    // contributes one hoisted constant instead of two multiplies.
    let inv = 255 - u32::from(ta);
    let (vr, vg, vb) = (
        u32::from(mul255(params.tint[0], ta)),
        u32::from(mul255(params.tint[1], ta)),
        u32::from(mul255(params.tint[2], ta)),
    );
    out.reserve(n);
    for &word in blur_a.iter() {
        let px = unpack_canonical(word);
        // BT.601 integer luma of the premultiplied channels.
        let m = if desat_amount == 0 && boost == 0 {
            px
        } else if boost > 0 {
            // The vibrant arm: each channel extends away from luma.
            let luma = bt601_luma(px);
            [
                extend_channel(px[0], luma, boost),
                extend_channel(px[1], luma, boost),
                extend_channel(px[2], luma, boost),
                px[3],
            ]
        } else {
            let luma = bt601_luma(px);
            [
                lerp255(px[0], luma, desat),
                lerp255(px[1], luma, desat),
                lerp255(px[2], luma, desat),
                px[3],
            ]
        };
        let o = if ta == 0 {
            // An invisible veil's `over` still clamps the channels to
            // the alpha (a no-op on premultiplied backdrops — the only
            // shape production ever feeds it — but the honest result
            // for garbage input, byte-equal to the Phase 27 loop).
            [m[0].min(m[3]), m[1].min(m[3]), m[2].min(m[3]), m[3]]
        } else {
            let oa = u32::from(ta) + u32::from(mul255(m[3], inv as u8));
            [
                (vr + u32::from(mul255(m[0], inv as u8))).min(oa) as u8,
                (vg + u32::from(mul255(m[1], inv as u8))).min(oa) as u8,
                (vb + u32::from(mul255(m[2], inv as u8))).min(oa) as u8,
                oa as u8,
            ]
        };
        out.push(pack_canonical(o[0], o[1], o[2], o[3]));
    }
}

/// The BT.601 integer luma of a premultiplied pixel (the frost's
/// saturation reference — the same weights every arm shares).
fn bt601_luma(px: [u8; 4]) -> u8 {
    ((77 * u32::from(px[0]) + 150 * u32::from(px[1]) + 29 * u32::from(px[2]) + 128) >> 8) as u8
}

/// Extend one channel away from its luma by `boost / 255` of the
/// distance (the vibrant boost): `c + round_half_away((c - l) *
/// boost / 255)`, clamped to `u8`. The identity at `boost == 0` and
/// at `c == l` returns the channel unchanged — the formula's own
/// fixed points, oracle-pinned.
fn extend_channel(c: u8, luma: u8, boost: u32) -> u8 {
    let delta = i32::from(c) - i32::from(luma);
    if delta == 0 {
        return c;
    }
    let v = delta * boost as i32;
    let adj = if v >= 0 {
        (v + 127) / 255
    } else {
        -((-v + 127) / 255)
    };
    (i32::from(c) + adj).clamp(0, 255) as u8
}

/// The edge-light material (Phase 40): the luminous hairline's
/// imagery — a `dest`-sized canonical word buffer holding the
/// 1-pixel inner stroke at the rounded silhouette.
///
/// The ring's coverage is exact by construction: it is the silhouette's
/// own antialiased coverage minus the coverage of the silhouette
/// **eroded by one pixel** (the rect shrunk 1 px per side, the radius
/// less 1 — the Minkowski erosion of a rounded rectangle, so the ring
/// follows the corners on the same coverage curve the ink clips
/// with). A degenerate destination (thinner than 2 px) keeps its full
/// coverage as the ring — the shape is all edge.
///
/// Each ring pixel carries the stroke premultiplied by
/// `alpha * ring_coverage`; interior pixels stay fully transparent
/// (the `apply_material` skip-transparent-runs optimization never
/// touches them). Like the shadow, the ring is *imagery* — memoized
/// per `(dest, radius, params)` by [`EdgeMemo`] and reused while the
/// window sits still.
#[must_use]
pub fn edge_light_material(params: EdgeLightParams, dest: Rect, radius: u32) -> Vec<u32> {
    let mut out = Vec::new();
    edge_light_material_into(params, dest, radius, &mut out);
    out
}

/// The edge-light material into a caller-retained buffer (the memo's
/// steady-state path).
pub(crate) fn edge_light_material_into(
    params: EdgeLightParams,
    dest: Rect,
    radius: u32,
    out: &mut Vec<u32>,
) {
    out.clear();
    if dest.is_empty() {
        return;
    }
    let rw = dest.w as usize;
    out.resize(rw * dest.h as usize, 0);
    // The erosion: shrink 1 px per side, radius less 1. A destination
    // too small to erode is all edge (the full coverage is the ring).
    let eroded = if dest.w >= 2 && dest.h >= 2 {
        Rect::new(dest.x + 1, dest.y + 1, dest.w - 2, dest.h - 2)
    } else {
        Rect::EMPTY
    };
    let inner_radius = radius.saturating_sub(1);
    for y in dest.y..dest.bottom() {
        for x in dest.x..dest.right() {
            let outer = rounded_coverage(x, y, dest, radius);
            let inner = if eroded.is_empty() {
                0
            } else {
                rounded_coverage(x, y, eroded, inner_radius)
            };
            let ring = outer.saturating_sub(inner);
            if ring == 0 {
                continue; // the transparent interior stays zeroed
            }
            let a = mul255(params.alpha, ring);
            let word = pack_canonical(
                mul255(params.color[0], a),
                mul255(params.color[1], a),
                mul255(params.color[2], a),
                a,
            );
            let idx = (y - dest.y) as usize * rw + (x - dest.x) as usize;
            out[idx] = word;
        }
    }
}

/// A memoized shadow material (Phase 29) — the macOS rule that shadows
/// are *imagery*: computed once per (params, geometry, ink) and reused
/// while the window sits still, instead of regenerated every frame.
///
/// Pure memoization: a hit serves exactly
/// [`shadow_material`]`(&params, dest, ink_radius)` — the same inputs
/// always produce the same bytes, so nothing observable changes (the
/// styled golden suites re-render frames and pin the bytes). Doctrine
/// intact: a linear scan (no hashmaps), a bounded word budget with
/// LRU eviction, and honest behavior past the cap — an over-budget
/// material is recomputed into a retained scratch slot, never cached.
#[derive(Debug, Default)]
pub struct MaterialCache {
    /// Most-recently-used first.
    entries: Vec<CacheEntry>,
    /// The retained slot for over-budget materials (recomputed per
    /// call, never admitted — the honest fallback).
    oversized: Vec<u32>,
    /// The over-budget path's texture bytes (converted per call).
    oversized_rgba: Vec<u8>,
    /// The live word budget (the floor is
    /// [`MATERIAL_CACHE_BUDGET_WORDS`]; `begin_frame` raises it for
    /// large outputs so a real 4K desktop's shadow working set fits —
    /// Phase 30: a 6 Mi cap thrashed exactly there, rebuilding every
    /// shadow every frame).
    budget_words: usize,
    /// How many materials were *built* (misses and over-budget
    /// recomputes) — the thrash oracle: a steady state must not grow
    /// it.
    rebuilds: u64,
}

/// One memoized material: the full key (every input
/// [`shadow_material`] reads) plus its words, box, and — lazily, for
/// the GL path — the texture-byte encoding of the same words.
#[derive(Debug)]
struct CacheEntry {
    params: ShadowParams,
    dest: Rect,
    ink_radius: u32,
    words: Vec<u32>,
    rect: Rect,
    /// The GL texture payload (`words_to_rgba` of `words`), filled on
    /// first GL use — one conversion per unique shadow, not per frame.
    rgba: Vec<u8>,
}

/// The cache's word budget floor: 6 Mi words (24 MiB) — roughly
/// thirteen phone-window shadows, the working set of a real session.
/// Past the live budget (this floor, or the scaled budget a large
/// output sets — [`MaterialCache::set_budget_words`]),
/// least-recently-used materials evict; a single material larger than
/// the whole budget takes the honest recompute path.
pub const MATERIAL_CACHE_BUDGET_WORDS: usize = 6 * 1024 * 1024;

/// The entry cap: beyond this many distinct styles, eviction happens
/// even under the word budget (a bounded scan keeps the lookup cheap).
pub const MATERIAL_CACHE_MAX_ENTRIES: usize = 16;

impl MaterialCache {
    /// The live word budget (the floor when never scaled).
    #[must_use]
    pub fn budget_words(&self) -> usize {
        if self.budget_words == 0 {
            MATERIAL_CACHE_BUDGET_WORDS
        } else {
            self.budget_words
        }
    }

    /// Raise the budget for a large output — never lower it below
    /// the floor — so the output's real shadow working set fits
    /// without eviction thrash. The renderer calls this at
    /// `begin_frame` with twice the output's pixel count: a full
    /// desktop's shadows (windows + dock) sit well under that, and a
    /// small output keeps the phone-era floor untouched (byte-exact
    /// Phase 29 behavior — only *larger* budgets ever change).
    pub fn set_budget_words(&mut self, words: usize) {
        self.budget_words = self.budget_words().max(words);
    }

    /// How many materials were built since construction (misses and
    /// over-budget recomputes) — the thrash oracle: rendering the
    /// same scene twice must not grow it, whatever the output size.
    #[must_use]
    pub const fn rebuilds(&self) -> u64 {
        self.rebuilds
    }

    /// The position of a matching entry, if any (the full key: every
    /// input [`shadow_material`] reads).
    fn hit_pos(&self, params: &ShadowParams, dest: Rect, ink_radius: u32) -> Option<usize> {
        self.entries
            .iter()
            .position(|e| e.params == *params && e.dest == dest && e.ink_radius == ink_radius)
    }

    /// Move a hit to the front (the LRU lives at the back).
    fn move_to_front(&mut self, pos: usize) {
        let entry = self.entries.remove(pos);
        self.entries.insert(0, entry);
    }

    /// Evict until the budget and entry cap admit `extra` words.
    fn evict_for(&mut self, extra: usize) {
        let mut total: usize = self.entries.iter().map(|e| e.words.len()).sum();
        while total + extra > self.budget_words()
            || self.entries.len() >= MATERIAL_CACHE_MAX_ENTRIES
        {
            let Some(evicted) = self.entries.pop() else {
                break;
            };
            total -= evicted.words.len();
        }
    }

    /// The memoized `shadow_material(params, dest, ink_radius)` —
    /// building on a miss, serving on a hit.
    pub fn get_or_build(
        &mut self,
        params: &ShadowParams,
        dest: Rect,
        ink_radius: u32,
    ) -> (&[u32], Rect) {
        if let Some(pos) = self.hit_pos(params, dest, ink_radius) {
            self.move_to_front(pos);
            let entry = &self.entries[0];
            return (&entry.words, entry.rect);
        }
        let (words, rect) = shadow_material(params, dest, ink_radius);
        self.rebuilds += 1;
        if words.len() > self.budget_words() {
            // The honest path: retained scratch, never cached.
            self.oversized = words;
            return (&self.oversized, rect);
        }
        self.evict_for(words.len());
        self.entries.insert(
            0,
            CacheEntry {
                params: *params,
                dest,
                ink_radius,
                words,
                rect,
                rgba: Vec::new(),
            },
        );
        let entry = &self.entries[0];
        (&entry.words, entry.rect)
    }

    /// The memoized material's GL texture bytes — converting once per
    /// unique shadow (the upload payload the GL stream draws).
    pub fn get_or_build_rgba(
        &mut self,
        params: &ShadowParams,
        dest: Rect,
        ink_radius: u32,
    ) -> (&[u8], Rect) {
        if let Some(pos) = self.hit_pos(params, dest, ink_radius) {
            self.move_to_front(pos);
            let entry = &mut self.entries[0];
            if entry.rgba.is_empty() {
                entry.rgba = words_to_texture_bytes(&entry.words);
            }
            let entry = &self.entries[0];
            return (&entry.rgba, entry.rect);
        }
        let (words, rect) = shadow_material(params, dest, ink_radius);
        self.rebuilds += 1;
        if words.len() > self.budget_words() {
            // The over-budget path: the words live in the retained
            // scratch slot; the texture bytes convert per call (never
            // admitted — the honest fallback).
            self.oversized = words;
            self.oversized_rgba = words_to_texture_bytes(&self.oversized);
            return (&self.oversized_rgba, rect);
        }
        let rgba = words_to_texture_bytes(&words);
        self.evict_for(words.len());
        self.entries.insert(
            0,
            CacheEntry {
                params: *params,
                dest,
                ink_radius,
                words,
                rect,
                rgba,
            },
        );
        let entry = &self.entries[0];
        (&entry.rgba, entry.rect)
    }

    /// How many distinct materials are memoized (the honest report
    /// line — tests and introspection).
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing is memoized.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Canonical premultiplied words → tightly packed RGBA bytes (the
/// texture-upload layout the GL seam mandates) — the shared converter
/// for memoized materials.
fn words_to_texture_bytes(words: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(words.len() * 4);
    for &w in words {
        out.extend_from_slice(&unpack_canonical(w));
    }
    out
}

/// A memoized frost material (Phase 29): the material is a pure
/// function of (saved backdrop words, parameters), so an unchanged
/// backdrop — the steady state: static windows, damage elsewhere —
/// reuses the bytes instead of re-blurring.
///
/// The comparison is a full word equality (deterministic by
/// construction — no hashing, no clocks); a changed backdrop recomputes
/// into the same retained entry. Two entries cover the real phone
/// scene (the dock plus one frosted panel).
#[derive(Debug, Default)]
pub struct FrostMemo {
    entries: Vec<FrostEntry>,
}

#[derive(Debug)]
struct FrostEntry {
    dest: Rect,
    params: BackdropParams,
    saved: Vec<u32>,
    material: Vec<u32>,
    blur_a: Vec<u32>,
    blur_b: Vec<u32>,
}

/// How many distinct frosted surfaces the memo tracks (the dock plus a
/// panel; more frosted layers than this thrash — each recomputes, which
/// is exactly what Phase 28 did for every one of them).
pub const FROST_MEMO_ENTRIES: usize = 2;

impl FrostMemo {
    /// The memoized `frost_material(saved, dest.w, dest.h, params)` —
    /// recomputing when the backdrop changed, serving when it did not.
    pub fn get_or_build(&mut self, dest: Rect, params: &BackdropParams, saved: &[u32]) -> &[u32] {
        let pos = self
            .entries
            .iter()
            .position(|e| e.dest == dest && e.params == *params);
        let pos = if let Some(p) = pos {
            p
        } else {
            while self.entries.len() >= FROST_MEMO_ENTRIES {
                self.entries.pop();
            }
            self.entries.insert(
                0,
                FrostEntry {
                    dest,
                    params: *params,
                    saved: Vec::new(),
                    material: Vec::new(),
                    blur_a: Vec::new(),
                    blur_b: Vec::new(),
                },
            );
            0
        };
        let entry = &mut self.entries[pos];
        if entry.saved != saved || entry.material.is_empty() {
            frost_material_into(
                saved,
                dest.w,
                dest.h,
                params,
                &mut entry.blur_a,
                &mut entry.blur_b,
                &mut entry.material,
            );
            entry.saved.clear();
            entry.saved.extend_from_slice(saved);
        }
        let entry = &self.entries[pos];
        &entry.material
    }

    /// How many frosted surfaces are memoized (tests, introspection).
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing is memoized.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// A memoized edge-light material (Phase 40): the hairline is *imagery*
/// exactly like the shadow — a pure function of
/// `(dest, radius, params)`, so a window that sits still serves the
/// cached words (and, for the GL stream, the once-converted texture
/// bytes) instead of re-walking the silhouette.
///
/// The discipline is the frost memo's: a linear scan (no hashing), a
/// small entry cap with honest eviction (the oldest ring falls out —
/// menus come and go), deterministic by construction. Four entries
/// cover the real scene (the dock plus a menu, a tooltip, and a
/// sheet); beyond that the rings recompute, which is the Phase 27
/// behavior for every one of them.
#[derive(Debug, Default)]
pub struct EdgeMemo {
    entries: Vec<EdgeEntry>,
}

#[derive(Debug)]
struct EdgeEntry {
    dest: Rect,
    radius: u32,
    params: EdgeLightParams,
    words: Vec<u32>,
    /// The GL texture payload (`words_to_texture_bytes` of `words`),
    /// filled on first GL use — one conversion per unique ring.
    rgba: Vec<u8>,
}

/// How many distinct hairlines the memo tracks (the dock plus three
/// transient menus).
pub const EDGE_MEMO_ENTRIES: usize = 4;

impl EdgeMemo {
    fn hit_pos(&self, dest: Rect, radius: u32, params: EdgeLightParams) -> Option<usize> {
        self.entries
            .iter()
            .position(|e| e.dest == dest && e.radius == radius && e.params == params)
    }

    fn admit(&mut self, dest: Rect, radius: u32, params: EdgeLightParams) -> usize {
        while self.entries.len() >= EDGE_MEMO_ENTRIES {
            self.entries.pop(); // the oldest ring evicts
        }
        self.entries.insert(
            0,
            EdgeEntry {
                dest,
                radius,
                params,
                words: Vec::new(),
                rgba: Vec::new(),
            },
        );
        0
    }

    /// The memoized `edge_light_material(params, dest, radius)` words —
    /// building on a miss, serving the cached bytes on a hit.
    pub fn get_or_build(&mut self, dest: Rect, radius: u32, params: EdgeLightParams) -> &[u32] {
        let pos = match self.hit_pos(dest, radius, params) {
            Some(pos) => pos,
            None => self.admit(dest, radius, params),
        };
        let entry = &mut self.entries[pos];
        if entry.words.is_empty() {
            edge_light_material_into(params, dest, radius, &mut entry.words);
        }
        &self.entries[pos].words
    }

    /// The memoized ring's GL texture bytes — converting once per
    /// unique hairline (the upload payload the GL stream draws).
    pub fn get_or_build_rgba(&mut self, dest: Rect, radius: u32, params: EdgeLightParams) -> &[u8] {
        let pos = match self.hit_pos(dest, radius, params) {
            Some(pos) => pos,
            None => self.admit(dest, radius, params),
        };
        let entry = &mut self.entries[pos];
        if entry.words.is_empty() {
            edge_light_material_into(params, dest, radius, &mut entry.words);
        }
        if entry.rgba.is_empty() {
            entry.rgba = words_to_texture_bytes(&entry.words);
        }
        &self.entries[pos].rgba
    }

    /// How many hairlines are memoized (tests, introspection).
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing is memoized.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mul255_identity_at_full_coverage() {
        for v in [0u8, 1, 7, 64, 128, 200, 254, 255] {
            assert_eq!(mul255(v, 255), v, "v={v}");
        }
        assert_eq!(mul255(255, 128), 128, "half rounds up");
        assert_eq!(mul255(255, 127), 127);
    }

    #[test]
    fn lerp255_endpoints_are_exact() {
        assert_eq!(lerp255(10, 200, 0), 10);
        assert_eq!(lerp255(10, 200, 255), 200);
        assert_eq!(lerp255(0, 255, 128), 128);
    }

    #[test]
    fn scale_premul_identity_at_255() {
        let px = [3, 200, 250, 17];
        assert_eq!(scale_premul(px, 255), px);
        let half = scale_premul(px, 128);
        assert_eq!(half, [2, 100, 125, 9], "channels halve together");
    }

    #[test]
    fn over_premul_matches_the_composite_rule() {
        // The hand case the composite golden suite pins: 50% red over
        // opaque blue in encoded space.
        let out = over_premul([128, 0, 0, 128], [0, 0, 255, 255], 255);
        // a2=128, inv=127, oa=128+mul255(255,127)=128+127=255
        assert_eq!(out[3], 255);
        // r = mul255(128,255)=128 + mul255(0,127)=0 -> 128
        assert_eq!(out[0], 128);
        // b = 0 + mul255(255,127)=127
        assert_eq!(out[2], 127);
        // Opacity folds the source: q=128 halves the source first.
        let out = over_premul([255, 255, 255, 255], [0, 0, 0, 255], 128);
        assert_eq!(out, [128, 128, 128, 255]);
    }

    #[test]
    fn coverage_hand_oracle_radius_two() {
        // Rect (0,0,4,4) radius 2 (a stadium): the corner pixel's
        // center is sqrt(4.5) from the arc center; coverage =
        // 0.5 - (sqrt(4.5) - 2) = 0.37868 -> 97.
        let rect = Rect::new(0, 0, 4, 4);
        assert_eq!(rounded_coverage(0, 0, rect, 2), 97);
        assert_eq!(rounded_coverage(3, 3, rect, 2), 97, "symmetry");
        // Pixel (0,1): distance sqrt(2.5) -> cov 0.91886 -> 234.
        assert_eq!(rounded_coverage(0, 1, rect, 2), 234);
        assert_eq!(rounded_coverage(1, 1, rect, 2), 255, "interior full");
        assert_eq!(rounded_coverage(2, 2, rect, 2), 255);
        // Outside.
        assert_eq!(rounded_coverage(-1, -1, rect, 2), 0);
        assert_eq!(rounded_coverage(4, 4, rect, 2), 0);
    }

    #[test]
    fn coverage_degrades_to_the_rectangle() {
        let rect = Rect::new(0, 0, 10, 6);
        for x in -1..11 {
            for y in -1..7 {
                assert_eq!(
                    rounded_coverage(x, y, rect, 0),
                    rounded_coverage(x, y, rect, 0),
                    "deterministic"
                );
                let inside = x >= 0 && y >= 0 && x < 10 && y < 6;
                assert_eq!(
                    rounded_coverage(x, y, rect, 0),
                    if inside { 255 } else { 0 },
                    "({x},{y})"
                );
            }
        }
    }

    #[test]
    fn coverage_is_monotone_along_the_diagonal() {
        let rect = Rect::new(0, 0, 20, 20);
        let mut last = 255u8;
        for step in 0..30 {
            // March outward from the arc center along the diagonal:
            // coverage may only decrease (or hold).
            let t = (step as f32) * 0.1;
            let x = (10.0 + 8.0 * t - 0.5).round() as i32;
            let y = x;
            let c = rounded_coverage(x, y, rect, 10);
            assert!(c <= last, "coverage increased at step {step}");
            last = c;
        }
    }

    #[test]
    fn box_blur_of_a_constant_is_the_constant() {
        let (w, h) = (9u32, 9u32);
        let words: Vec<u32> = (0..w * h)
            .map(|_| pack_canonical(10, 20, 30, 255))
            .collect();
        let out = blur_words(&words, w, h, 2, 3, BlurEdge::Clamp);
        assert!(out.iter().all(|&v| v == pack_canonical(10, 20, 30, 255)));
        // Transparent edge too: interior of a large constant field.
        let out = blur_words(&words, w, h, 1, 1, BlurEdge::Transparent);
        assert_eq!(
            out[4 * w as usize + 4],
            pack_canonical(10, 20, 30, 255),
            "the center keeps the constant"
        );
    }

    #[test]
    fn box_blur_step_edge_hand_oracle() {
        // A 7-wide alpha step [0,0,0,255,255,255,255] blurred with
        // radius 1 (window 3, rule (sum+1)/3), padded transparently.
        // The middle row's windows are fully in-bounds vertically:
        // [0, 0, 85, 170, 255, 255, 170] — note the honest tail at the
        // canvas edge (the last pixel's window hangs off: 510/3). The
        // outer rows attenuate further (their vertical windows hang
        // off too): x=2 reads (85*2+1)/3 = 57.
        let (w, h) = (7usize, 3usize);
        let row: Vec<u32> = (0..w)
            .map(|x| {
                if x >= 3 {
                    pack_canonical(255, 255, 255, 255)
                } else {
                    0
                }
            })
            .collect();
        let words: Vec<u32> = row.iter().cycle().take(w * h).copied().collect();
        let out = blur_words(&words, w as u32, h as u32, 1, 1, BlurEdge::Transparent);
        let expect = [0u8, 0, 85, 170, 255, 255, 170];
        for (x, e) in expect.iter().enumerate() {
            let px = unpack_canonical(out[w + x]);
            assert_eq!(px[3], *e, "alpha at ({x},1)");
            assert_eq!(px[0], *e, "premul rgb tracks alpha at ({x},1)");
        }
        // The top row's vertical window covers rows {0,1} only:
        // the 85 at x=2 reads (85+85+1)/3 = 57 there.
        let top = unpack_canonical(out[2]);
        assert_eq!(top[3], 57, "the transparent edge attenuates outer rows");
    }

    #[test]
    fn blur_spreads_energy_and_fades() {
        // A single opaque pixel in a 9x3 field, blurred radius 2 once:
        // horizontally the impulse smears across the 5-wide window
        // (255/5 = 51), vertically the 3-row window of the center row
        // column catches it once — every row of the column then reads
        // (51 + 2) / 5 = 10 (full-window normalization: the energy off
        // the canvas is honestly gone).
        let (w, h) = (9, 3);
        let mut words = vec![0u32; w * h];
        words[w + 4] = pack_canonical(255, 255, 255, 255);
        let out = blur_words(&words, w as u32, h as u32, 2, 1, BlurEdge::Transparent);
        for y in 0..h {
            let center = unpack_canonical(out[y * w + 4]);
            assert_eq!(center[3], 10, "row {y}");
            let near = unpack_canonical(out[y * w + 2]);
            assert_eq!(near[3], 10, "the window reaches 2 out (row {y})");
            let beyond = unpack_canonical(out[y * w + 1]);
            assert_eq!(beyond[3], 0, "and no further (row {y})");
        }
    }

    #[test]
    fn shadow_holes_the_ink() {
        // A 4x4 window, shadow radius 0 blur 0 passes 1 offset 0: the
        // material equals the silhouette, holed by the ink.
        let params = ShadowParams {
            radius: 0,
            blur: 0,
            passes: 1,
            color: [0, 0, 0],
            alpha: 255,
            offset: (0, 0),
        };
        let dest = Rect::new(10, 10, 4, 4);
        let (words, box_rect) = shadow_material(&params, dest, 0);
        assert_eq!(box_rect, dest, "no blur, no offset: box == dest");
        // Every ink pixel is holed (radius 0 ink = the full rect).
        assert!(words.iter().all(|&v| v == 0), "the ink hole empties it");
    }

    #[test]
    fn shadow_fringe_survives_the_hole() {
        // With a corner radius on the ink, the corner pixels outside
        // the ink keep shadow energy.
        let params = ShadowParams {
            radius: 2,
            blur: 0,
            passes: 1,
            color: [0, 0, 0],
            alpha: 255,
            offset: (0, 0),
        };
        let dest = Rect::new(0, 0, 6, 6);
        let (words, box_rect) = shadow_material(&params, dest, 2);
        assert_eq!(box_rect, dest);
        // Corner (0,0): silhouette coverage 97 (the hand oracle) — the
        // ink hole is 255 minus nothing there... ink coverage at the
        // corner is also 97, so the hole keeps 255-97 of it:
        // kept alpha = mul255(97, 255-97) = mul255(97, 158) = 60.
        let corner = unpack_canonical(words[0]);
        assert_eq!(corner[3], 60, "corner fringe keeps partial energy");
        // Center pixel: ink coverage 255 -> fully holed.
        let center = unpack_canonical(words[3 * 6 + 3]);
        assert_eq!(center[3], 0);
    }

    #[test]
    fn frost_blurs_desaturates_and_tints() {
        // Half red, half blue backdrop, no blur: the material veils it.
        let w = 2;
        let saved = vec![
            pack_canonical(255, 0, 0, 255),
            pack_canonical(0, 0, 255, 255),
        ];
        let params = BackdropParams {
            blur: 0,
            passes: 0,
            saturation: 255, // no desaturation
            tint: [255, 255, 255],
            tint_alpha: 128, // a 50% white veil
        };
        let out = frost_material(&saved, w, 1, &params);
        // veil = premul white at 128 over red: r = 128 + mul255(255,127)
        // = 255, g = b = 128 + 0 = 128 — the red shows through the
        // half veil (pink), the opaque backdrop stays opaque.
        let left = unpack_canonical(out[0]);
        assert_eq!(left[0], 255, "red shows through the half veil");
        assert_eq!(left[1], 128, "green takes the veil");
        assert_eq!(left[2], 128, "blue takes the veil");
        assert_eq!(left[3], 255, "opaque backdrop stays opaque");
        // Desaturation alone pulls color towards luma.
        let params = BackdropParams {
            blur: 0,
            passes: 0,
            saturation: 0,
            tint: [0, 0, 0],
            tint_alpha: 0,
        };
        let out = frost_material(&saved, w, 1, &params);
        let left = unpack_canonical(out[0]);
        // Full desaturation is grayscale: luma of premul (255,0,0) is
        // (77*255+128)>>8 = 77, and every channel lerps fully to it.
        assert_eq!(left[0], 77);
        assert_eq!(left[1], 77);
        assert_eq!(left[2], 77);
    }

    // ---- Phase 29: the optimization oracles ---------------------------

    /// The exact-division proof, exhaustively: for every odd window the
    /// sanitized domain can produce and every sum it can carry, the
    /// 48-bit reciprocal multiply equals the plain division.
    #[test]
    fn reciprocal_division_is_exact() {
        for win in (3..=257usize).step_by(2) {
            let m48 = ((1u64 << 48) - 1) / win as u64 + 1;
            for n in 0..=65_663u32 {
                let fast = ((u64::from(n) * m48) >> 48) as u32;
                assert_eq!(fast, n / win as u32, "win={win} n={n}");
            }
        }
    }

    /// The Phase 27 blur, verbatim — the reference the rewritten kernel
    /// must match byte-for-byte.
    fn reference_blur(
        words: &[u32],
        width: u32,
        height: u32,
        radius: u32,
        passes: u32,
        edge: BlurEdge,
    ) -> Vec<u32> {
        let n = words.len();
        if n == 0 || radius == 0 || passes == 0 || width == 0 || height == 0 {
            return words.to_vec();
        }
        let (w, r) = (width as usize, radius as usize);
        let h = height as usize;
        let win = 2 * r + 1;
        let half = r as u32;
        let mut src = words.to_vec();
        let mut dst = vec![0u32; n];
        for _ in 0..passes {
            for row in 0..h {
                let base = row * w;
                let (mut sr, mut sg, mut sb, mut sa) = (0u32, 0u32, 0u32, 0u32);
                for i in 0..win {
                    if let Some(sx) = sample_index(i as i64 - r as i64, w, edge) {
                        let px = unpack_canonical(src[base + sx]);
                        sr += u32::from(px[0]);
                        sg += u32::from(px[1]);
                        sb += u32::from(px[2]);
                        sa += u32::from(px[3]);
                    }
                }
                for x in 0..w {
                    dst[base + x] = mean_word(sr, sg, sb, sa, half, win);
                    let enter = sample_index(x as i64 + r as i64 + 1, w, edge);
                    let leave = sample_index(x as i64 - r as i64, w, edge);
                    if enter != leave {
                        if let Some(e) = enter {
                            let px = unpack_canonical(src[base + e]);
                            sr += u32::from(px[0]);
                            sg += u32::from(px[1]);
                            sb += u32::from(px[2]);
                            sa += u32::from(px[3]);
                        }
                        if let Some(l) = leave {
                            let px = unpack_canonical(src[base + l]);
                            sr -= u32::from(px[0]);
                            sg -= u32::from(px[1]);
                            sb -= u32::from(px[2]);
                            sa -= u32::from(px[3]);
                        }
                    }
                }
            }
            core::mem::swap(&mut src, &mut dst);
            for col in 0..w {
                let (mut sr, mut sg, mut sb, mut sa) = (0u32, 0u32, 0u32, 0u32);
                for i in 0..win {
                    if let Some(sy) = sample_index(i as i64 - r as i64, h, edge) {
                        let px = unpack_canonical(src[sy * w + col]);
                        sr += u32::from(px[0]);
                        sg += u32::from(px[1]);
                        sb += u32::from(px[2]);
                        sa += u32::from(px[3]);
                    }
                }
                for y in 0..h {
                    dst[y * w + col] = mean_word(sr, sg, sb, sa, half, win);
                    let enter = sample_index(y as i64 + r as i64 + 1, h, edge);
                    let leave = sample_index(y as i64 - r as i64, h, edge);
                    if enter != leave {
                        if let Some(e) = enter {
                            let px = unpack_canonical(src[e * w + col]);
                            sr += u32::from(px[0]);
                            sg += u32::from(px[1]);
                            sb += u32::from(px[2]);
                            sa += u32::from(px[3]);
                        }
                        if let Some(l) = leave {
                            let px = unpack_canonical(src[l * w + col]);
                            sr -= u32::from(px[0]);
                            sg -= u32::from(px[1]);
                            sb -= u32::from(px[2]);
                            sa -= u32::from(px[3]);
                        }
                    }
                }
            }
            core::mem::swap(&mut src, &mut dst);
        }
        src
    }

    /// The rewritten kernel equals the Phase 27 kernel byte-for-byte
    /// across randomized shapes, radii, pass counts, and both edge
    /// policies (narrow strips, tall strips, radius-wider-than-canvas).
    #[test]
    fn blur_kernel_matches_the_reference_over_randomized_shapes() {
        let mut rng = crate::test_rng();
        for _ in 0..160 {
            let w = 1 + (rng.next_u64() % 40) as u32;
            let h = 1 + (rng.next_u64() % 40) as u32;
            let radius = (rng.next_u64() % 12) as u32;
            let passes = (rng.next_u64() % 4) as u32;
            let edge = if rng.next_u64() & 1 == 0 {
                BlurEdge::Clamp
            } else {
                BlurEdge::Transparent
            };
            let words: Vec<u32> = (0..w * h)
                .map(|_| {
                    pack_canonical(
                        (rng.next_u64() % 256) as u8,
                        (rng.next_u64() % 256) as u8,
                        (rng.next_u64() % 256) as u8,
                        (rng.next_u64() % 256) as u8,
                    )
                })
                .collect();
            let fast = blur_words(&words, w, h, radius, passes, edge);
            let reference = reference_blur(&words, w, h, radius, passes, edge);
            assert_eq!(
                fast, reference,
                "w={w} h={h} r={radius} passes={passes} edge={edge:?}"
            );
            // The retained-scratch entry point agrees with both.
            let mut a = words.clone();
            let mut b = vec![0u32; words.len()];
            blur_passes(
                &mut a,
                &mut b,
                w as usize,
                h as usize,
                radius as usize,
                passes,
                edge,
            );
            assert_eq!(a, fast, "scratch path: w={w} h={h} r={radius}");
        }
    }

    /// The corner bands are *sound*: every pixel outside the bands has
    /// exactly the bulk coverage (255 inside the rect, 0 outside) —
    /// skipping the SDF there changes nothing. Randomized rects and
    /// radii, including the overlap-heavy narrow shapes.
    #[test]
    fn corner_bands_cover_every_fractional_pixel() {
        let mut rng = crate::test_rng();
        for _ in 0..240 {
            let w = 1 + (rng.next_u64() % 30) as u32;
            let h = 1 + (rng.next_u64() % 30) as u32;
            let radius = (rng.next_u64() % 20) as u32;
            let x = (rng.next_u64() % 8) as i32 - 4;
            let y = (rng.next_u64() % 8) as i32 - 4;
            let rect = Rect::new(x, y, w, h);
            for py in y - 3..y + h as i32 + 3 {
                let band = corner_band_xs(rect, radius, py);
                for px in x - 3..x + w as i32 + 3 {
                    if band.contains(px) {
                        continue; // Inside the band: the SDF decides.
                    }
                    let cov = rounded_coverage(px, py, rect, radius);
                    let inside = px >= rect.x
                        && py >= rect.y
                        && px < rect.x + rect.w as i32
                        && py < rect.y + rect.h as i32;
                    let bulk = if inside { 255 } else { 0 };
                    assert_eq!(cov, bulk, "rect={rect:?} r={radius} pixel=({px},{py})");
                }
            }
        }
    }

    /// The merged band walk visits every band column exactly once
    /// (the overlapping narrow-rect shapes are the regression the GL
    /// corpus caught).
    #[test]
    fn merged_bands_visit_each_column_once() {
        let mut rng = crate::test_rng();
        for _ in 0..300 {
            let w = 1 + (rng.next_u64() % 24) as u32;
            let radius = (rng.next_u64() % 16) as u32;
            let rect = Rect::new(0, 0, w, 8);
            let band = corner_band_xs(rect, radius, 0);
            let merged = band.merged();
            let mut seen: Vec<i32> = Vec::new();
            for (x0, x1) in merged {
                assert!(x0 < x1 || (x0 == 0 && x1 == 0), "empty must be (0,0)");
                seen.extend(x0..x1);
            }
            // Sorted + disjoint + identical to the union membership.
            let mut sorted = seen.clone();
            sorted.sort_unstable();
            sorted.dedup();
            assert_eq!(seen, sorted, "no column visited twice: w={w} r={radius}");
            for x in -4..w as i32 + 4 {
                assert_eq!(
                    band.contains(x),
                    merged.iter().any(|&(a, b)| x >= a && x < b),
                    "membership preserved: w={w} r={radius} x={x}"
                );
            }
        }
    }

    /// The Phase 27 shadow builder, verbatim — the band-limited rewrite
    /// must match it byte-for-byte.
    fn reference_shadow(params: &ShadowParams, dest: Rect, ink_radius: u32) -> (Vec<u32>, Rect) {
        let pad = params.blur.saturating_mul(params.passes);
        let box_x = dest.x + params.offset.0 - pad as i32;
        let box_y = dest.y + params.offset.1 - pad as i32;
        let box_w = dest.w + 2 * pad;
        let box_h = dest.h + 2 * pad;
        let box_rect = Rect::new(box_x, box_y, box_w, box_h);
        let silhouette = Rect::new(
            dest.x + params.offset.0 - box_x,
            dest.y + params.offset.1 - box_y,
            dest.w,
            dest.h,
        );
        let ink = Rect::new(dest.x - box_x, dest.y - box_y, dest.w, dest.h);
        let mut words = Vec::with_capacity((box_w * box_h) as usize);
        for y in 0..box_h as i32 {
            for x in 0..box_w as i32 {
                let cov = rounded_coverage(x, y, silhouette, params.radius);
                let a_eff = mul255(params.alpha, cov);
                words.push(pack_canonical(
                    mul255(params.color[0], a_eff),
                    mul255(params.color[1], a_eff),
                    mul255(params.color[2], a_eff),
                    a_eff,
                ));
            }
        }
        if params.blur > 0 && params.passes > 0 {
            words = reference_blur(
                &words,
                box_w,
                box_h,
                params.blur,
                params.passes,
                BlurEdge::Transparent,
            );
        }
        for y in 0..box_h as i32 {
            for x in 0..box_w as i32 {
                let ink_cov = rounded_coverage(x, y, ink, ink_radius);
                if ink_cov == 255 {
                    words[y as usize * box_w as usize + x as usize] = 0;
                } else if ink_cov > 0 {
                    let idx = y as usize * box_w as usize + x as usize;
                    let px = unpack_canonical(words[idx]);
                    let kept = scale_premul(px, 255 - ink_cov);
                    words[idx] = pack_canonical(kept[0], kept[1], kept[2], kept[3]);
                }
            }
        }
        (words, box_rect)
    }

    /// The band-limited shadow builder equals the Phase 27 builder
    /// byte-for-byte over randomized shapes — including the
    /// bands-overlap narrow rects and radius > half-extent stadiums.
    #[test]
    fn shadow_material_matches_the_reference_over_randomized_shapes() {
        let mut rng = crate::test_rng();
        for _ in 0..140 {
            let dest = Rect::new(
                (rng.next_u64() % 16) as i32 - 8,
                (rng.next_u64() % 16) as i32 - 8,
                1 + (rng.next_u64() % 22) as u32,
                1 + (rng.next_u64() % 22) as u32,
            );
            let params = ShadowParams {
                radius: (rng.next_u64() % 14) as u32,
                blur: (rng.next_u64() % 8) as u32,
                passes: (rng.next_u64() % 4) as u32,
                color: [
                    (rng.next_u64() % 256) as u8,
                    (rng.next_u64() % 256) as u8,
                    (rng.next_u64() % 256) as u8,
                ],
                alpha: (rng.next_u64() % 256) as u8,
                offset: (
                    (rng.next_u64() % 9) as i32 - 4,
                    (rng.next_u64() % 9) as i32 - 4,
                ),
            };
            let ink_radius = (rng.next_u64() % 16) as u32;
            let (words, rect) = shadow_material(&params, dest, ink_radius);
            let (ref_words, ref_rect) = reference_shadow(&params, dest, ink_radius);
            assert_eq!(rect, ref_rect);
            assert_eq!(words, ref_words, "dest={dest:?} params={params:?}");
        }
    }

    /// The Phase 27 frost loop, verbatim, with Phase 40's boost arm
    /// beside it — the fast-path rewrite must match byte-for-byte on
    /// both sides of the saturation identity.
    fn reference_frost(
        saved: &[u32],
        width: u32,
        height: u32,
        params: &BackdropParams,
    ) -> Vec<u32> {
        let blurred = reference_blur(
            saved,
            width,
            height,
            params.blur,
            params.passes,
            BlurEdge::Clamp,
        );
        let sat = u32::from(params.saturation);
        let (desat_amount, boost) = if sat <= 255 {
            (255 - sat, 0)
        } else {
            (0, sat - 255)
        };
        let desat = desat_amount as u8;
        // The reference's own channel extension, written from the
        // documented rule (round half away from zero) — independent
        // of the production `extend_channel` by construction site.
        let ext = |c: u8, luma: u8| -> u8 {
            let delta = i32::from(c) - i32::from(luma);
            if delta == 0 {
                return c;
            }
            let v = delta * boost as i32;
            let adj = if v >= 0 {
                (v + 127) / 255
            } else {
                -((-v + 127) / 255)
            };
            (i32::from(c) + adj).clamp(0, 255) as u8
        };
        let mut out = Vec::with_capacity(blurred.len());
        for &word in &blurred {
            let px = unpack_canonical(word);
            let m = if desat_amount == 0 && boost == 0 {
                px
            } else {
                let luma =
                    ((77 * u32::from(px[0]) + 150 * u32::from(px[1]) + 29 * u32::from(px[2]) + 128)
                        >> 8) as u8;
                if boost > 0 {
                    [ext(px[0], luma), ext(px[1], luma), ext(px[2], luma), px[3]]
                } else {
                    [
                        lerp255(px[0], luma, desat),
                        lerp255(px[1], luma, desat),
                        lerp255(px[2], luma, desat),
                        px[3],
                    ]
                }
            };
            let veil = [
                mul255(params.tint[0], params.tint_alpha),
                mul255(params.tint[1], params.tint_alpha),
                mul255(params.tint[2], params.tint_alpha),
                params.tint_alpha,
            ];
            let o = over_premul(veil, m, 255);
            out.push(pack_canonical(o[0], o[1], o[2], o[3]));
        }
        out
    }

    /// The fast-path frost equals the Phase 27 loop byte-for-byte —
    /// across the saturation and veil corners (full saturation,
    /// transparent veil, both, neither).
    #[test]
    fn frost_material_matches_the_reference_over_randomized_params() {
        let mut rng = crate::test_rng();
        for _ in 0..140 {
            let w = 1 + (rng.next_u64() % 30) as u32;
            let h = 1 + (rng.next_u64() % 20) as u32;
            let saved: Vec<u32> = (0..w * h)
                .map(|_| {
                    pack_canonical(
                        (rng.next_u64() % 256) as u8,
                        (rng.next_u64() % 256) as u8,
                        (rng.next_u64() % 256) as u8,
                        (rng.next_u64() % 256) as u8,
                    )
                })
                .collect();
            // Cover the fast-path corners deterministically — the
            // identity, the full desaturation, the menu's boost, and
            // the whole vibrant domain.
            let saturation = match rng.next_u64() % 4 {
                0 => 255,
                1 => 0,
                2 => 383,
                _ => (rng.next_u64() % 511) as u16,
            };
            let tint_alpha = match rng.next_u64() % 3 {
                0 => 0,
                1 => 255,
                _ => (rng.next_u64() % 200) as u8,
            };
            let params = BackdropParams {
                blur: (rng.next_u64() % 10) as u32,
                passes: (rng.next_u64() % 4) as u32,
                saturation,
                tint: [
                    (rng.next_u64() % 256) as u8,
                    (rng.next_u64() % 256) as u8,
                    (rng.next_u64() % 256) as u8,
                ],
                tint_alpha,
            };
            let fast = frost_material(&saved, w, h, &params);
            let reference = reference_frost(&saved, w, h, &params);
            assert_eq!(fast, reference, "w={w} h={h} params={params:?}");
        }
    }

    /// The corner fold equals the per-pixel fold byte-for-byte,
    /// including the overlapping-band narrow rects.
    #[test]
    fn corner_fold_matches_the_per_pixel_fold() {
        let mut rng = crate::test_rng();
        for _ in 0..200 {
            let w = 1 + (rng.next_u64() % 26) as u32;
            let h = 1 + (rng.next_u64() % 26) as u32;
            let radius = (rng.next_u64() % 18) as u32;
            let mut words: Vec<u32> = (0..w * h)
                .map(|_| {
                    pack_canonical(
                        (rng.next_u64() % 256) as u8,
                        (rng.next_u64() % 256) as u8,
                        (rng.next_u64() % 256) as u8,
                        (rng.next_u64() % 256) as u8,
                    )
                })
                .collect();
            // The reference: fold every pixel.
            let mut reference = words.clone();
            let local = Rect::new(0, 0, w, h);
            for y in 0..h as i32 {
                for x in 0..w as i32 {
                    let cov = rounded_coverage(x, y, local, radius);
                    if cov == 255 {
                        continue;
                    }
                    let idx = y as usize * w as usize + x as usize;
                    let px = unpack_canonical(reference[idx]);
                    let scaled = scale_premul(px, cov);
                    reference[idx] = pack_canonical(scaled[0], scaled[1], scaled[2], scaled[3]);
                }
            }
            apply_corner_fold(&mut words, w, h, radius);
            assert_eq!(words, reference, "w={w} h={h} r={radius}");
        }
    }

    /// The shadow memo serves exactly the builder's bytes: hits and
    /// misses agree, distinct keys stay distinct, and the LRU budget
    /// evicts honestly.
    #[test]
    fn material_cache_serves_the_builders_bytes() {
        let params = ShadowParams::panel();
        let dest = Rect::new(10, 10, 24, 18);
        let (built, rect) = shadow_material(&params, dest, 6);
        let mut cache = MaterialCache::default();
        let (served, srect) = cache.get_or_build(&params, dest, 6);
        assert_eq!((served, srect), (built.as_slice(), rect));
        assert_eq!(cache.len(), 1);
        // The hit serves the same bytes.
        let (again, arect) = cache.get_or_build(&params, dest, 6);
        assert_eq!((again, arect), (built.as_slice(), rect));
        assert_eq!(cache.len(), 1);
        // A different key builds its own material.
        let other = Rect::new(4, 4, 12, 9);
        let (built2, _) = shadow_material(&params, other, 6);
        let (served2, _) = cache.get_or_build(&params, other, 6);
        assert_eq!(served2, built2.as_slice());
        assert_eq!(cache.len(), 2);
        // The texture-byte form is the words' RGBA encoding.
        let (texture, texture_rect) = cache.get_or_build_rgba(&params, dest, 6);
        assert_eq!(texture_rect, rect);
        assert_eq!(texture.len(), built.len() * 4);
        for (i, &w) in built.iter().enumerate() {
            let px = unpack_canonical(w);
            assert_eq!(&texture[i * 4..i * 4 + 4], &px);
        }
        // The build counter: one per distinct key, zero on hits.
        assert_eq!(cache.rebuilds(), 2, "two distinct keys built");
        let _ = cache.get_or_build(&params, dest, 6);
        assert_eq!(cache.rebuilds(), 2, "the hit did not build");
    }

    /// The Phase 30 budget scaling: the floor by default, raised
    /// never lowered, and the build counter counts misses only —
    /// every property the steady-state thrash oracle stands on.
    #[test]
    fn material_cache_budget_scales_with_the_output() {
        // The default is the floor.
        let mut cache = MaterialCache::default();
        assert_eq!(cache.budget_words(), MATERIAL_CACHE_BUDGET_WORDS);
        // Raising: a large output's budget (twice 4K's pixels ≈ 16 Mi).
        cache.set_budget_words(16 * 1024 * 1024);
        assert_eq!(cache.budget_words(), 16 * 1024 * 1024);
        // Never lowered — not by a small output, not below the floor.
        cache.set_budget_words(1024);
        assert_eq!(cache.budget_words(), 16 * 1024 * 1024);
        // The build counter: one per miss, zero per hit.
        let params = ShadowParams::panel();
        let dest = Rect::new(0, 0, 32, 32);
        let before = cache.rebuilds();
        let _ = cache.get_or_build(&params, dest, 6);
        let after = cache.rebuilds();
        assert_eq!(after, before + 1, "a miss builds exactly once");
        let _ = cache.get_or_build(&params, dest, 6);
        assert_eq!(cache.rebuilds(), after, "the hit builds nothing");
    }

    /// The thrash regression the 4K matrix caught: a working set
    /// larger than the *floor* but well under the *scaled* budget
    /// stays resident once `set_budget_words` has seen the output —
    /// before Phase 30, the fixed cap evicted and rebuilt every
    /// material every frame (the 194 ms steady state).
    #[test]
    fn scaled_budget_keeps_a_large_working_set_resident() {
        let params = ShadowParams::panel();
        // A 2048x2048 destination materializes ~4.2 Mi words with the
        // panel shadow's blur margins — two of them overflow the 6 Mi
        // floor together (8.4 Mi) but fit a 16 Mi budget.
        let dest_a = Rect::new(0, 0, 2048, 2048);
        let dest_b = Rect::new(8, 8, 2048, 2048);
        let mut cache = MaterialCache::default();
        let _ = cache.get_or_build(&params, dest_a, 6);
        let _ = cache.get_or_build(&params, dest_b, 6);
        // At the floor, the two entries cannot coexist: the LRU
        // evicted the older one.
        assert_eq!(cache.len(), 1, "the floor evicts the older entry");
        // A third access rebuilds — the thrash.
        let rebuilt_before = cache.rebuilds();
        let _ = cache.get_or_build(&params, dest_a, 6);
        assert_eq!(cache.rebuilds(), rebuilt_before + 1, "the thrash rebuilds");
        // With the scaled budget, both stay resident and serve.
        let mut scaled = MaterialCache::default();
        scaled.set_budget_words(16 * 1024 * 1024);
        let _ = scaled.get_or_build(&params, dest_a, 6);
        let _ = scaled.get_or_build(&params, dest_b, 6);
        assert_eq!(scaled.len(), 2, "the scaled budget keeps both");
        let stable = scaled.rebuilds();
        let _ = scaled.get_or_build(&params, dest_a, 6);
        let _ = scaled.get_or_build(&params, dest_b, 6);
        assert_eq!(scaled.rebuilds(), stable, "no rebuild in the steady state");
    }

    /// The frost memo recomputes when the backdrop changes and serves
    /// when it does not — the steady-state contract.
    #[test]
    fn frost_memo_tracks_the_backdrop() {
        let params = BackdropParams::frosted_light();
        let dest = Rect::new(0, 0, 20, 10);
        let saved: Vec<u32> = (0..200)
            .map(|i| pack_canonical(i as u8, 255 - i as u8, 128, 255))
            .collect();
        let mut memo = FrostMemo::default();
        let first = memo.get_or_build(dest, &params, &saved).to_vec();
        assert_eq!(memo.len(), 1);
        // Unchanged backdrop: the same bytes (and no rebuild).
        let again = memo.get_or_build(dest, &params, &saved).to_vec();
        assert_eq!(first, again);
        // A changed backdrop rebuilds.
        let mut changed = saved.clone();
        changed[7] = pack_canonical(250, 10, 10, 255);
        let rebuilt = memo.get_or_build(dest, &params, &changed).to_vec();
        assert_ne!(first, rebuilt);
        // ...and the fresh value equals a direct build.
        let direct = frost_material(&changed, dest.w, dest.h, &params);
        assert_eq!(rebuilt, direct);
        // A second frosted surface gets its own entry.
        let dest2 = Rect::new(5, 5, 8, 8);
        let saved2 = vec![pack_canonical(9, 9, 9, 255); 64];
        let _ = memo.get_or_build(dest2, &params, &saved2);
        assert_eq!(memo.len(), 2);
    }

    // ---- Phase 40: the edge light and the vibrant domain -------------

    /// THE ring coverage oracle, from the SDF rules: an 8x6
    /// destination at the origin, radius 2 (the vertical straight
    /// band survives: half-height 3 > radius 2). The eroded silhouette
    /// is (1,1,6,4) radius 1.
    #[test]
    fn edge_light_ring_is_the_eroded_coverage_difference() {
        let params = EdgeLightParams {
            color: [255, 255, 255],
            alpha: 255,
        };
        let dest = Rect::new(0, 0, 8, 6);
        let ring = edge_light_material(params, dest, 2);
        assert_eq!(ring.len(), 48, "the ring covers the destination");
        let word_at = |x: i32, y: i32| ring[(y * 8 + x) as usize];
        // Straight left edge, middle row: full stroke.
        let full = word_at(0, 3);
        assert_eq!(full >> 24, 0xFF, "the straight edge carries alpha 255");
        // The interior: transparent.
        assert_eq!(word_at(4, 3) >> 24, 0x00, "the interior stays clear");
        // The corner: outer 97, inner 0 — alpha 97, white premul.
        let corner = word_at(0, 0);
        assert_eq!(corner >> 24, 97, "the corner arc's coverage");
        assert_eq!((corner >> 16) & 0xFF, 97, "premul white at alpha 97");
        // The erosion's exactness: every ring coverage equals the
        // coverage difference, recomputed here from the oracle SDF.
        let eroded = Rect::new(1, 1, 6, 4);
        for y in 0..6 {
            for x in 0..8 {
                let outer = rounded_coverage(x, y, dest, 2);
                let inner = rounded_coverage(x, y, eroded, 1);
                let expect = outer.saturating_sub(inner);
                let got = ((word_at(x, y) >> 24) & 0xFF) as u8;
                assert_eq!(
                    got, expect,
                    "ring at ({x},{y}): the erosion difference, exactly"
                );
            }
        }
    }

    /// The ring's alpha scales the stroke; a zero-alpha ring is fully
    /// transparent (the identity).
    #[test]
    fn edge_light_alpha_scales_the_stroke() {
        let dest = Rect::new(0, 0, 8, 6);
        let half = edge_light_material(
            EdgeLightParams {
                color: [200, 100, 50],
                alpha: 128,
            },
            dest,
            2,
        );
        let full = edge_light_material(
            EdgeLightParams {
                color: [200, 100, 50],
                alpha: 255,
            },
            dest,
            2,
        );
        // The straight-edge pixel (0, 3) — ring coverage 255: half
        // alpha vs full.
        let half_px = half[(3 * 8) as usize];
        let full_px = full[(3 * 8) as usize];
        assert_eq!(half_px >> 24, 128);
        assert_eq!(full_px >> 24, 255);
        // Premultiplied channels follow the alpha (mul255 rule).
        assert_eq!((half_px >> 16) & 0xFF, u32::from(mul255(200, 128)));
        // Zero alpha: everything transparent.
        let off = edge_light_material(
            EdgeLightParams {
                color: [255, 255, 255],
                alpha: 0,
            },
            dest,
            2,
        );
        assert!(off.iter().all(|&w| w == 0));
    }

    /// Degenerate destinations (thinner than the stroke) are all edge:
    /// the full coverage is the ring.
    #[test]
    fn degenerate_destinations_are_all_edge() {
        let params = EdgeLightParams {
            color: [255, 255, 255],
            alpha: 255,
        };
        // 1x1: the single pixel is its own ring.
        let one = edge_light_material(params, Rect::new(0, 0, 1, 1), 0);
        assert_eq!(one.len(), 1);
        assert_eq!(one[0] >> 24, 0xFF, "the 1x1 is all edge");
        // 1xN columns: every pixel rings (the erosion is empty).
        let tall = edge_light_material(params, Rect::new(0, 0, 1, 8), 0);
        assert!(tall.iter().all(|&w| w >> 24 == 0xFF));
        // Empty: nothing.
        assert!(edge_light_material(params, Rect::EMPTY, 2).is_empty());
    }

    /// The edge memo serves hits, builds misses, and evicts past the
    /// cap — the imagery doctrine.
    #[test]
    fn edge_memo_serves_and_evicts() {
        let params = EdgeLightParams::light();
        let mut memo = EdgeMemo::default();
        let a = Rect::new(0, 0, 6, 4);
        let first = memo.get_or_build(a, 2, params).to_vec();
        assert_eq!(memo.len(), 1);
        // A hit: the same bytes, no growth.
        let again = memo.get_or_build(a, 2, params).to_vec();
        assert_eq!(first, again);
        assert_eq!(memo.len(), 1);
        // A hit equals the direct build.
        assert_eq!(first, edge_light_material(params, a, 2));
        // Different geometry or params: a new entry.
        let _ = memo.get_or_build(Rect::new(3, 3, 6, 4), 2, params);
        let _ = memo.get_or_build(a, 5, params);
        let _ = memo.get_or_build(
            a,
            2,
            EdgeLightParams {
                color: [9, 9, 9],
                alpha: 9,
            },
        );
        assert_eq!(memo.len(), 4, "the cap holds four");
        // The fifth evicts the oldest (the first-built rect).
        let _ = memo.get_or_build(Rect::new(20, 20, 3, 3), 2, params);
        assert_eq!(memo.len(), 4);
        assert_ne!(
            memo.get_or_build(a, 2, params).to_vec(),
            Vec::new(),
            "the evicted ring rebuilds on demand"
        );
        // The GL arm converts once: the rgba length is 4x the words
        // (both borrows end before the assertion).
        let rgba_len = {
            let rgba = memo.get_or_build_rgba(a, 2, params);
            rgba.len()
        };
        let words_len = {
            let words = memo.get_or_build(a, 2, params);
            words.len()
        };
        assert_eq!(rgba_len, words_len * 4);
    }
}
