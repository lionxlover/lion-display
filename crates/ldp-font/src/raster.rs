//! The analytic coverage rasterizer (Phase 54): glyph outlines in em
//! units to 8-bit alpha coverage — exact per-pixel area integration,
//! the FreeType-smooth class of math in a few hundred honest lines.
//!
//! The pipeline is three stages, all deterministic (no hinting, no
//! device tuning tables, no environment: the same outline and pixel
//! size always produce byte-identical coverage — the oracle doctrine
//! every compositor test leans on):
//!
//! 1. **Walk & flatten** ([`flatten`]): every contour becomes line
//!    segments — on-curve pairs are lines, an off-curve point and
//!    its following on-curve partner a quadratic Bézier, flattened by
//!    recursive midpoint subdivision until the control point sits
//!    within [`FLATTEN_TOL`] of the chord. Lion Sans authors its arcs
//!    as many near-flat quads, but the rasterizer never trusts the
//!    author; it flattens everything.
//! 2. **Split** ([`split_rows`]): the polyline is cut at integer row
//!    boundaries so every piece lives wholly inside one pixel row.
//! 3. **Integrate** ([`column_area`]): each piece sweeps the exact
//!    signed area left of itself across each pixel column — the
//!    antiderivative of the clamped linear edge, one closed-form
//!    evaluation per (piece, column). The per-pixel accumulator
//!    clamps to `[0, 1]` under the nonzero winding rule.
//!
//! The integral's shape: a piece's edge is linear in y, so the area
//! it contributes to column `[cx, cx+1)` is
//! `∫ clip(x(y) − cx, 0, 1) dy` — the integral of a *continuous*
//! piecewise-linear clamp. Its antiderivative `G` is C¹, so the
//! chain rule holds across the clamp's kinks with no case splitting:
//! the integral over the whole piece is `(G(u₁) − G(u₀)) / (u₁ − u₀)`
//! times the piece's height — the whole rasterizer's exactness in
//! one fraction.
//!
//! The sign convention: an edge pointing *up* in bitmap space (the
//! reflection of a font-up edge) adds `+1`; a down-pointing edge
//! subtracts. Under that convention an outer contour authored
//! counter-clockwise in font space (the TrueType convention Lion
//! Sans follows — the reflection maps it to up-first winding) reads
//! positive inside; holes authored clockwise subtract, and a hole's
//! interior reads zero. [`rasterize`] clamps negative accumulations
//! to zero rather than taking absolute values: a mis-directed
//! contour fails visibly (missing ink), never silently.

use crate::outline::Contour;

/// The flattening tolerance (pixel units): a quadratic's control
/// point this close to its chord ends subdivision. 1/32 px is below
/// the 8-bit coverage quantization — flattening error can never
/// move a pixel's alpha by one step.
const FLATTEN_TOL: f32 = 1.0 / 32.0;

/// The maximum flattening depth: a safety valve for degenerate
/// outlines (midpoint subdivision converges quadratically; six
/// halvings shrink any em-scale segment far below tolerance).
const MAX_DEPTH: u8 = 6;

/// One flattened line segment in bitmap-relative pixel space.
#[derive(Clone, Copy, Debug)]
struct Seg {
    /// The start x.
    x0: f32,
    /// The start y.
    y0: f32,
    /// The end x.
    x1: f32,
    /// The end y.
    y1: f32,
}

/// One row-piece: a segment clipped to one pixel row, normalized so
/// `y0 ≤ y1` with the direction carried in `down` (the winding sign
/// — up-pointing edges add, down-pointing subtract; the convention
/// that reads font-CCW outer contours positive inside).
#[derive(Clone, Copy, Debug)]
struct Piece {
    /// The x at the piece's low-y end.
    x_lo: f32,
    /// The x at the piece's high-y end.
    x_hi: f32,
    /// The piece's low y, row-relative.
    y0: f32,
    /// The piece's high y, row-relative.
    y1: f32,
    /// The winding sign: `+1` when the original edge points *up*
    /// (decreasing bitmap y — a font-up edge under the reflection),
    /// `-1` when it points down.
    down: f32,
}

/// The antiderivative of `clip(u, 0, 1)`: `u²/2` on `[0, 1]`, `u − ½`
/// above, `0` below — C¹ at both knots, which is what lets
/// [`column_area`] evaluate the integral as one closed form.
#[inline]
fn g_of(u: f32) -> f32 {
    if u <= 0.0 {
        0.0
    } else if u >= 1.0 {
        u - 0.5
    } else {
        u * u * 0.5
    }
}

/// The exact area one piece sweeps left of itself across one pixel
/// column `[cx, cx+1)`, in row units: `∫ clip(x(y) − cx, 0, 1) dy`
/// over the piece's y-span. The edge is linear from `(x_lo, y0)` to
/// `(x_hi, y1)`; `u` runs linearly from `u_a = x_lo − cx` to
/// `u_b = x_hi − cx` with `du/dy` constant, and `G(u)` (C¹) is the
/// antiderivative of the continuous clamp — so the integral is the
/// chain-rule fraction, no case splitting at the kinks.
fn column_area(p: &Piece, cx: i32) -> f32 {
    let dy = p.y1 - p.y0;
    debug_assert!(dy > 0.0);
    let u_a = p.x_lo - cx as f32;
    let u_b = p.x_hi - cx as f32;
    let d = u_b - u_a;
    // ∫_0^dy clip(u(y) − cx) dy = dy · ∫_0^1 clip(u_a + s·d) ds,
    // and ∫_0^1 clip(u_a + s·d) ds = (G(u_b) − G(u_a)) / d.
    let per = if d == 0.0 {
        u_a.clamp(0.0, 1.0)
    } else {
        (g_of(u_b) - g_of(u_a)) / d
    };
    dy * per
}

/// Flatten one contour (already in bitmap-relative pixel space,
/// y-down) into straight segments.
///
/// The point stream follows the TrueType convention: on-curve points
/// are vertices; an off-curve point is a quadratic control that the
/// *next* on-curve point resolves. Two consecutive off-curve points
/// imply an on-curve midpoint between them. The contour closes from
/// its last resolved vertex back to its first point.
fn flatten(points: &[(f32, f32, bool)], segs: &mut Vec<Seg>) {
    let n = points.len();
    if n < 2 {
        return;
    }
    // The walk's current on-curve vertex (the contour starts on-curve
    // by table construction).
    debug_assert!(points[0].2, "the table starts every contour on-curve");
    let mut cur = (points[0].0, points[0].1);
    let mut pending: Option<(f32, f32)> = None;
    for &(x, y, on) in &points[1..] {
        if on {
            match pending.take() {
                Some((cx, cy)) => quad(cur, (cx, cy), (x, y), 0, segs),
                None => line(cur, (x, y), segs),
            }
            cur = (x, y);
        } else if let Some((pcx, pcy)) = pending {
            // Two consecutive off-curve points: the implied on-curve
            // midpoint splits them (the TrueType convention).
            let mid = ((pcx + x) * 0.5, (pcy + y) * 0.5);
            quad(cur, (pcx, pcy), mid, 0, segs);
            cur = mid;
            pending = Some((x, y));
        } else {
            pending = Some((x, y));
        }
    }
    // The closing edge back to the contour's start.
    let end = (points[0].0, points[0].1);
    match pending.take() {
        Some((cx, cy)) => quad(cur, (cx, cy), end, 0, segs),
        None => line(cur, end, segs),
    }
}

/// Emit one straight segment.
fn line(a: (f32, f32), b: (f32, f32), segs: &mut Vec<Seg>) {
    segs.push(Seg {
        x0: a.0,
        y0: a.1,
        x1: b.0,
        y1: b.1,
    });
}

/// Flatten one quadratic (p0 → control → p2) by midpoint subdivision
/// until the control point's distance from the chord is within
/// [`FLATTEN_TOL`] (or the depth valve trips — a linear bound on the
/// curve's deviation, the standard flatness test).
fn quad(p0: (f32, f32), p1: (f32, f32), p2: (f32, f32), depth: u8, segs: &mut Vec<Seg>) {
    let (ax, ay) = p0;
    let (bx, by) = p2;
    let mx = (ax + bx) * 0.5;
    let my = (ay + by) * 0.5;
    let dx = bx - ax;
    let dy = by - ay;
    let len = (dx * dx + dy * dy).sqrt();
    let dist = if len == 0.0 {
        ((p1.0 - mx).powi(2) + (p1.1 - my).powi(2)).sqrt()
    } else {
        ((p1.0 - mx) * dy - (p1.1 - my) * dx).abs() / len
    };
    if dist <= FLATTEN_TOL || depth >= MAX_DEPTH {
        line(p0, p2, segs);
        return;
    }
    let m01 = ((ax + p1.0) * 0.5, (ay + p1.1) * 0.5);
    let m12 = ((p1.0 + bx) * 0.5, (p1.1 + by) * 0.5);
    let mm = ((m01.0 + m12.0) * 0.5, (m01.1 + m12.1) * 0.5);
    quad(p0, m01, mm, depth + 1, segs);
    quad(mm, m12, p2, depth + 1, segs);
}

/// Split the flattened segments at integer row boundaries; each piece
/// lands wholly inside one row with `y0 ≤ y1` and its direction
/// carried in `down`. Horizontal segments contribute nothing (a
/// horizontal ray crosses no horizontal edge) and drop out here.
fn split_rows(segs: &[Seg]) -> Vec<(usize, Piece)> {
    let mut out = Vec::new();
    for s in segs {
        if s.y0 == s.y1 {
            continue;
        }
        // Up-pointing (decreasing bitmap y) adds: the reflection of a
        // font-up edge — the font-CCW convention's positive wind.
        let down = if s.y1 < s.y0 { 1.0 } else { -1.0 };
        let (ylo, yhi) = if s.y0 < s.y1 {
            (s.y0, s.y1)
        } else {
            (s.y1, s.y0)
        };
        // The y-parameter of the segment (linear): the t at each
        // integer row boundary strictly inside the span.
        let t = |y: f32| (y - s.y0) / (s.y1 - s.y0);
        let mut cuts: Vec<f32> = Vec::new();
        let first = ylo.ceil();
        let last = yhi.ceil();
        let mut boundary = first;
        while boundary < last {
            let cut = boundary;
            if cut > ylo && cut < yhi {
                cuts.push(cut);
            }
            boundary += 1.0;
        }
        let mut ys: Vec<f32> = vec![ylo];
        ys.extend(cuts);
        ys.push(yhi);
        for w in ys.windows(2) {
            let (a, b) = (w[0], w[1]);
            if b <= a {
                continue;
            }
            let xa = s.x0 + (s.x1 - s.x0) * t(a);
            let xb = s.x0 + (s.x1 - s.x0) * t(b);
            // The row owning [a, b): a's floor (a boundary value
            // belongs to the row it opens).
            let row = a.floor();
            let row_idx = row.max(0.0) as usize;
            let (x_lo, x_hi) = if xa <= xb { (xa, xb) } else { (xb, xa) };
            out.push((
                row_idx,
                Piece {
                    x_lo,
                    x_hi,
                    y0: a - row,
                    y1: b - row,
                    down,
                },
            ));
        }
    }
    out
}

/// One rasterized glyph's delivery: the bitmap's extent and its
/// alpha coverage.
///
/// The placement contract: the bitmap's top-left pixel lands at
/// `(pen_x + x_off, baseline_row − y_top)` — `x_off` measured from
/// the pen position (usually small, the glyph's left bearing),
/// `y_top` the rows the bitmap's top sits *above* the baseline
/// (negative for ink that never reaches the baseline).
#[derive(Clone, Debug)]
pub(crate) struct Bitmap {
    /// The bitmap's width (px).
    pub w: u32,
    /// The bitmap's height (px).
    pub h: u32,
    /// The left bearing from the pen (px).
    pub x_off: i32,
    /// The rows the bitmap's top sits above the baseline.
    pub y_top: i32,
    /// The coverage, row-major `w · h` bytes.
    pub alpha: Vec<u8>,
}

/// Rasterize one glyph's contours at `scale` (pixels per em unit)
/// into an alpha bitmap.
///
/// The bitmap box is the outline's ink extent, quantized outward
/// (ink never clips); degenerate outlines (no points, or extents
/// that collapse) yield an empty bitmap honestly.
pub(crate) fn rasterize(contours: &[Contour], scale: f32) -> Bitmap {
    if scale <= 0.0 {
        return Bitmap {
            w: 0,
            h: 0,
            x_off: 0,
            y_top: 0,
            alpha: Vec::new(),
        };
    }
    // The ink extent in font units.
    let mut min_x = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for c in contours {
        for p in &c.points {
            min_x = min_x.min(p.x as f32);
            max_x = max_x.max(p.x as f32);
            min_y = min_y.min(p.y as f32);
            max_y = max_y.max(p.y as f32);
        }
    }
    if min_x > max_x {
        return Bitmap {
            w: 0,
            h: 0,
            x_off: 0,
            y_top: 0,
            alpha: Vec::new(),
        };
    }
    // The bitmap box in baseline-origin pixel space (y down, the
    // reflection of the font's y-up): left/top quantized outward.
    let left = (min_x * scale).floor();
    let right = (max_x * scale).ceil();
    let top = (-max_y * scale).floor();
    let bottom = (-min_y * scale).ceil();
    let w = (right - left).max(0.0) as u32;
    let h = (bottom - top).max(0.0) as u32;
    let x_off = left as i32;
    let y_top = -top as i32;
    if w == 0 || h == 0 {
        return Bitmap {
            w: 0,
            h: 0,
            x_off,
            y_top,
            alpha: Vec::new(),
        };
    }
    // Flatten in bitmap-relative coordinates.
    let mut segs: Vec<Seg> = Vec::new();
    for contour in contours {
        let pts: Vec<(f32, f32, bool)> = contour
            .points
            .iter()
            .map(|p| (p.x as f32 * scale - left, -(p.y as f32) * scale - top, p.on))
            .collect();
        flatten(&pts, &mut segs);
    }
    // Accumulate the winding integral per pixel.
    let pieces = split_rows(&segs);
    let mut accum: Vec<f32> = vec![0.0; (w as usize) * (h as usize)];
    for (row, p) in pieces {
        if row >= h as usize {
            continue;
        }
        let base = row * w as usize;
        // The winding integral's column reach: an edge's sweep
        // contributes to EVERY column strictly left of its highest
        // x (the columns far left read the full span; the shape's
        // opposite edges cancel them there). The bitmap's own left
        // edge bounds the walk.
        if p.x_hi <= 0.0 {
            continue;
        }
        let cx_last = (p.x_hi.ceil() as i32).min(w as i32);
        for cx in 0..cx_last {
            let a = column_area(&p, cx);
            if a != 0.0 {
                accum[base + cx as usize] += p.down * a;
            }
        }
    }
    // The nonzero rule: clamp the accumulated winding into coverage.
    let alpha = accum
        .into_iter()
        .map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5).min(255.0) as u8)
        .collect();
    Bitmap {
        w,
        h,
        x_off,
        y_top,
        alpha,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outline::Contour;

    /// One counter-clockwise (font space) unit-square contour at the
    /// origin — the exact-coverage probe.
    fn square(units: i32) -> Contour {
        // CCW in y-up: (0,0) → (s,0) → (s,s) → (0,s).
        Contour::from_points(vec![
            (0, 0, true),
            (units, 0, true),
            (units, units, true),
            (0, units, true),
        ])
    }

    #[test]
    fn the_grid_aligned_square_is_exact() {
        // A 1-unit glyph at 1 px per unit: the single pixel is fully
        // covered — 255, the quantization's exact ceiling.
        let b = rasterize(&[square(1)], 1.0);
        assert_eq!((b.w, b.h), (1, 1));
        assert_eq!(b.alpha, vec![255]);
    }

    #[test]
    fn the_interior_of_a_big_square_is_exact() {
        // 4×4 at 1 px/unit: every interior and edge pixel is 255
        // (the boundary rides the grid exactly).
        let b = rasterize(&[square(4)], 1.0);
        assert_eq!((b.w, b.h), (4, 4));
        assert!(b.alpha.iter().all(|&a| a == 255));
        // Reversed direction (CW): the winding reads negative, the
        // clamp zeroes it — ink only from the CCW convention.
        let cw = Contour::from_points(vec![(0, 0, true), (0, 4, true), (4, 4, true), (4, 0, true)]);
        let b = rasterize(&[cw], 1.0);
        assert!(b.alpha.iter().all(|&a| a == 0));
    }

    #[test]
    fn the_fractional_square_is_area_exact() {
        // A 3-unit square at 0.5 px/unit: the ink is 1.5 px wide and
        // tall in a 2×2 box — the exact per-pixel fractions are half
        // quarters and fulls, pinned by hand geometry:
        // row 0 [128, 64], row 1 [255, 128].
        let b = rasterize(&[square(3)], 0.5);
        assert_eq!((b.w, b.h), (2, 2));
        assert_eq!(b.alpha, vec![128, 64, 255, 128]);
    }

    #[test]
    fn the_right_triangle_boundary_is_exact() {
        // The triangle (0,0), (2,0), (2,2) at 1 px/unit — the CCW
        // walk's interior reads positive (the inked side). Bitmap
        // row 0 is font y ∈ [1,2]: pixel (1,0) is cut corner to
        // corner by the hypotenuse (exactly half); row 1 is font
        // y ∈ [0,1]: pixel (0,1) half, pixel (1,1) full.
        let tri = Contour::from_points(vec![(0, 0, true), (2, 0, true), (2, 2, true)]);
        let b = rasterize(&[tri], 1.0);
        assert_eq!((b.w, b.h), (2, 2));
        assert_eq!(b.alpha, vec![0, 128, 128, 255]);
    }

    #[test]
    fn the_circle_area_matches_pi() {
        // A 100-unit-radius circle at 0.1 px/unit (10 px radius): the
        // total coverage ≈ π·r² within the antialiasing fringe's
        // slack (±2% — the fringe pixels' partial coverage sums to
        // the boundary's perimeter-scale error).
        let r = 100.0f32;
        let n = 32; // the circle as 32 quads — near-exact
        let mut pts: Vec<(i32, i32, bool)> = Vec::new();
        for k in 0..n {
            let a0 = (k as f32) * std::f32::consts::TAU / n as f32;
            let am = (k as f32 + 0.5) * std::f32::consts::TAU / n as f32;
            // On-curve at the octant point, off-curve at the tangent
            // intersection (r / cos(π/n) out at the mid-angle).
            let rc = r / ((std::f32::consts::PI / n as f32).cos());
            pts.push((
                (r * a0.cos()).round() as i32,
                (r * a0.sin()).round() as i32,
                true,
            ));
            pts.push((
                (rc * am.cos()).round() as i32,
                (rc * am.sin()).round() as i32,
                false,
            ));
        }
        let circle = Contour::from_points(pts);
        // CCW check: the parametrization above walks CCW in y-up
        // (angle increasing) — the interior must read positive.
        let b = rasterize(&[circle], 0.1);
        let total: f32 = b.alpha.iter().map(|&a| f32::from(a)).sum();
        let got = total / 255.0; // the covered area, pixels
        let expect = std::f32::consts::PI * 100.0; // (10 px)² · π
        assert!(
            (got - expect).abs() / expect < 0.02,
            "circle coverage {got} vs πr² {expect}"
        );
        // The center is solid, the corner of the box is empty.
        let cx = b.w as usize / 2;
        assert_eq!(b.alpha[cx * b.w as usize + cx], 255);
        assert_eq!(b.alpha[0], 0);
    }

    #[test]
    fn the_hole_subtracts_under_nonzero() {
        // A ring: a 4-unit CCW square with a 2-unit CW hole inside —
        // the hole's interior reads zero, the ring's body full.
        let outer =
            Contour::from_points(vec![(0, 0, true), (4, 0, true), (4, 4, true), (0, 4, true)]);
        let inner =
            Contour::from_points(vec![(1, 1, true), (1, 3, true), (3, 3, true), (3, 1, true)]);
        let b = rasterize(&[outer, inner], 1.0);
        assert_eq!((b.w, b.h), (4, 4));
        for y in 0..4 {
            for x in 0..4 {
                let inside_hole = (1..3).contains(&x) && (1..3).contains(&y);
                let in_body = x == 0 || x == 3 || y == 0 || y == 3;
                let a = b.alpha[y * 4 + x];
                if inside_hole {
                    assert_eq!(a, 0, "the hole at ({x},{y})");
                } else if in_body {
                    assert_eq!(a, 255, "the ring at ({x},{y})");
                }
            }
        }
    }

    #[test]
    fn the_bearing_contract_holds() {
        // A cap-height probe: ink [0, 720] in font units at 16/1000
        // px/unit — the bitmap's top sits above the baseline, the
        // bottom on it.
        let cap = square(720);
        let b = rasterize(&[cap], 16.0 / 1000.0);
        assert_eq!(b.y_top, (720.0_f32 * 16.0 / 1000.0).ceil() as i32);
        // The bitmap's bottom row = baseline − y_top + h = 0 rows
        // below the baseline (no descent).
        assert_eq!(b.y_top - b.h as i32, 0);
        // A descender: ink [−180, 0] — the top rides the baseline,
        // the bottom hangs below it.
        let desc = Contour::from_points(vec![
            (0, -180, true),
            (84, -180, true),
            (84, 0, true),
            (0, 0, true),
        ]);
        let b = rasterize(&[desc], 16.0 / 1000.0);
        assert_eq!(b.y_top, 0);
        assert_eq!(
            b.h as i32 - b.y_top,
            (180.0_f32 * 16.0 / 1000.0).ceil() as i32
        );
    }

    #[test]
    fn determinism_is_byte_exact() {
        // The same outline twice: byte-identical coverage.
        let a = rasterize(&[square(700)], 16.0 / 1000.0);
        let b = rasterize(&[square(700)], 16.0 / 1000.0);
        assert_eq!(a.alpha, b.alpha);
        assert_eq!((a.w, a.h, a.x_off, a.y_top), (b.w, b.h, b.x_off, b.y_top));
    }
}
