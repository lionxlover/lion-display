//! Shape rasterization: arcs (outline and filled) and polygons.
//!
//! X arc semantics: the arc's bounding rectangle is `(x, y, width,
//! height)`, the ellipse's center is the rectangle's center, and
//! angles measured in 1/64 degree units grow **clockwise from the
//! 3 o'clock position** (screen Y grows downward). `angle1` is the
//! start, `angle2` the signed extent; a full circle is `angle2 =
//! 360*64`. Zero width or height draws nothing.
//!
//! * Outline arcs sample the parametric curve so no gap between
//!   consecutive samples exceeds half a pixel along the larger radius
//!   — a full circle comes out as a closed ring with no holes.
//! * Filled arcs cover pixel **centers**: inside the ellipse, and
//!   either inside the center wedge (pie mode) or on the same side of
//!   the chord as the arc's midpoint (chord mode). The same-side test
//!   is exact for both minor and major arcs, which the tests pin.
//! * Polygons fill by scanlines over pixel centers with the even-odd
//!   or the winding rule.

#![forbid(unsafe_code)]

use crate::render::Target;

/// One X arc: bounding box plus start/extent angles in 1/64 degrees.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Arc {
    /// Left edge of the bounding rectangle.
    pub x: i32,
    /// Top edge.
    pub y: i32,
    /// Bounding rectangle width (zero draws nothing).
    pub width: u32,
    /// Bounding rectangle height.
    pub height: u32,
    /// Start angle in 1/64 degrees, clockwise from 3 o'clock.
    pub angle1: i32,
    /// Signed extent in 1/64 degrees.
    pub angle2: i32,
}

impl Arc {
    /// The arc's center and radii as floats.
    #[must_use]
    fn geometry(self) -> (f64, f64, f64, f64) {
        let cx = f64::from(self.x) + f64::from(self.width) / 2.0;
        let cy = f64::from(self.y) + f64::from(self.height) / 2.0;
        (
            cx,
            cy,
            f64::from(self.width) / 2.0,
            f64::from(self.height) / 2.0,
        )
    }

    /// The point on the arc at parameter `t` (1/64 degrees).
    #[must_use]
    fn point_at(self, t: i64) -> (f64, f64) {
        let (cx, cy, rx, ry) = self.geometry();
        let rad = (t as f64) * (core::f64::consts::PI / 11_520.0); // /64 deg
        (cx + rx * rad.cos(), cy + ry * rad.sin())
    }

    /// The arc midpoint parameter.
    #[must_use]
    fn mid_param(self) -> i64 {
        i64::from(self.angle1) + i64::from(self.angle2) / 2
    }

    /// Bounding rectangle inflated by one pixel (rasterization reach).
    #[must_use]
    fn reach(self) -> (i32, i32, i32, i32) {
        (
            self.x.saturating_sub(1),
            self.y.saturating_sub(1),
            self.x.saturating_add(self.width as i32 + 1),
            self.y.saturating_add(self.height as i32 + 1),
        )
    }
}

/// Angular sweep test: is parameter `t` inside `[start, start+extent]`
/// sweeping in the extent's direction?
fn in_sweep(t: i64, start: i64, extent: i64) -> bool {
    let wrap = 360 * 64;
    if extent >= 0 {
        let d = (t - start).rem_euclid(wrap);
        d <= extent
    } else {
        let d = (start - t).rem_euclid(wrap);
        d <= -extent
    }
}

impl Target<'_> {
    /// Plot one float sample rounded to the nearest pixel.
    fn plot_rounded(&mut self, px: f64, py: f64, src: u32) {
        self.put(px.round() as i32, py.round() as i32, src);
    }

    /// Draw an arc outline (PolyArc).
    pub fn arc_outline(&mut self, arc: Arc, src: u32) {
        if arc.width == 0 || arc.height == 0 || arc.angle2 == 0 {
            return;
        }
        let (_, _, rx, ry) = arc.geometry();
        let rmax = rx.max(ry);
        if rmax <= 0.0 {
            return;
        }
        // Step so the arc distance between samples stays under half a
        // pixel along the larger radius.
        let extent_rad = (f64::from(arc.angle2)).abs() * (core::f64::consts::PI / 11_520.0);
        let max_step = 0.5 / rmax;
        let steps = ((extent_rad / max_step).ceil() as usize).clamp(1, 262_144);
        let dir = if arc.angle2 > 0 { 1 } else { -1 };
        let a1 = i64::from(arc.angle1);
        let a2 = i64::from(arc.angle2);
        for k in 0..=steps {
            let t = a1 + dir * (i64::from(k as u32) * a2.abs() / steps.max(1) as i64);
            let (px, py) = arc.point_at(t);
            self.plot_rounded(px, py, src);
        }
    }

    /// Fill an arc (PolyFillArc) in pie or chord mode.
    ///
    /// `pie` selects the center wedge; otherwise the region closes on
    /// the chord (same side as the arc midpoint — exact for any extent).
    pub fn fill_arc(&mut self, arc: Arc, src: u32, pie: bool) {
        if arc.width == 0 || arc.height == 0 || arc.angle2 == 0 {
            return;
        }
        let (cx, cy, rx, ry) = arc.geometry();
        if rx <= 0.0 || ry <= 0.0 {
            return;
        }
        // Chord geometry: endpoints and midpoint.
        let (x1, y1) = arc.point_at(i64::from(arc.angle1));
        let (x2, y2) = arc.point_at(i64::from(arc.angle1) + i64::from(arc.angle2));
        let (mx, my) = arc.point_at(arc.mid_param());
        let chord_cross = |px: f64, py: f64| -> bool {
            // Sign of the cross product (endpoint2-endpoint1) x (p-endpoint1).
            ((x2 - x1) * (py - y1) - (y2 - y1) * (px - x1)) >= 0.0
        };
        let mid_side = chord_cross(mx, my);
        let (bx0, by0, bx1, by1) = arc.reach();
        let (bx0, by0, bx1, by1) = self.clamp_box(bx0, by0, bx1, by1);
        for py in by0..by1 {
            for px in bx0..bx1 {
                let pxc = f64::from(px) + 0.5;
                let pyc = f64::from(py) + 0.5;
                let ex = (pxc - cx) / rx;
                let ey = (pyc - cy) / ry;
                if ex * ex + ey * ey > 1.0 {
                    continue;
                }
                let inside = if pie {
                    let t = angle64(ex, ey);
                    in_sweep(t, i64::from(arc.angle1), i64::from(arc.angle2))
                } else {
                    chord_cross(pxc, pyc) == mid_side
                };
                if inside {
                    self.put(px, py, src);
                }
            }
        }
    }

    /// Fill a polygon (FillPoly) with the given rule.
    ///
    /// `winding` selects the nonzero winding rule; otherwise even-odd.
    pub fn fill_polygon(&mut self, pts: &[(i32, i32)], src: u32, winding: bool) {
        if pts.len() < 3 {
            for &(x, y) in pts {
                self.put(x, y, src);
            }
            return;
        }
        let min_x = pts.iter().map(|p| p.0).min().unwrap_or(0);
        let max_x = pts.iter().map(|p| p.0).max().unwrap_or(0);
        let min_y = pts.iter().map(|p| p.1).min().unwrap_or(0);
        let max_y = pts.iter().map(|p| p.1).max().unwrap_or(0);
        let (sx0, sy0, sx1, sy1) = self.clamp_box(min_x, min_y, max_x + 1, max_y + 1);
        for y in sy0..sy1 {
            let yc = f64::from(y) + 0.5;
            // Crossings of the polygon boundary with the scanline.
            let mut xs: Vec<(f64, i32)> = Vec::new();
            let n = pts.len();
            for i in 0..n {
                let (ax, ay) = (f64::from(pts[i].0), f64::from(pts[i].1));
                let (bx, by) = (f64::from(pts[(i + 1) % n].0), f64::from(pts[(i + 1) % n].1));
                if (ay <= yc && by > yc) || (by <= yc && ay > yc) {
                    let t = (yc - ay) / (by - ay);
                    let x = ax + t * (bx - ax);
                    let dir = if by > ay { 1 } else { -1 };
                    xs.push((x, dir));
                }
            }
            xs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(core::cmp::Ordering::Equal));
            if winding {
                let mut wind = 0i32;
                let mut spans: Vec<(i32, i32)> = Vec::new();
                let mut span_start: Option<i32> = None;
                for &(x, dir) in &xs {
                    let was_zero = wind == 0;
                    wind += dir;
                    if wind != 0 && was_zero {
                        span_start = Some(x.floor() as i32);
                    } else if wind == 0 {
                        if let Some(s) = span_start.take() {
                            spans.push((s, x.ceil() as i32));
                        }
                    }
                }
                for &(s, e) in &spans {
                    for px in s.max(sx0)..e.min(sx1) {
                        self.put(px, y, src);
                    }
                }
            } else {
                let mut idx = 0;
                while idx + 1 < xs.len() {
                    let (xa, _) = xs[idx];
                    let (xb, _) = xs[idx + 1];
                    let s = xa.floor() as i32;
                    let e = xb.ceil() as i32;
                    for px in s.max(sx0)..e.min(sx1) {
                        self.put(px, y, src);
                    }
                    idx += 2;
                }
            }
        }
    }
}

/// The parameter angle (1/64 deg) of an offset from the ellipse center.
fn angle64(ex: f64, ey: f64) -> i64 {
    let mut deg = ey.atan2(ex).to_degrees() * 64.0;
    deg = deg.rem_euclid(360.0 * 64.0);
    deg.round() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::{Gx, Store};

    #[test]
    fn full_circle_outline_is_a_closed_ring() {
        let mut s = Store::filled(16, 16, 0);
        {
            let mut t = Target::full(&mut s, Gx::Copy, 0x00ff_ffff);
            t.arc_outline(
                Arc {
                    x: 0,
                    y: 0,
                    width: 15,
                    height: 15,
                    angle1: 0,
                    angle2: 360 * 64,
                },
                1,
            );
        }
        // The ring's pixels all sit within a pixel of radius 7.5.
        let mut count = 0;
        for y in 0..16 {
            for x in 0..16 {
                if s.pixel(x, y) == Some(1) {
                    count += 1;
                    let dx = f64::from(x) + 0.5 - 7.5;
                    let dy = f64::from(y) + 0.5 - 7.5;
                    let d = (dx * dx + dy * dy).sqrt();
                    assert!(d < 8.75 && d > 6.0, "ring pixel at ({x},{y}) dist {d}");
                }
            }
        }
        assert!(count >= 24, "ring must have samples, got {count}");
    }

    #[test]
    fn quarter_arc_runs_three_oclock_to_six() {
        let mut s = Store::filled(20, 20, 0);
        {
            let mut t = Target::full(&mut s, Gx::Copy, 0x00ff_ffff);
            // 90 degrees clockwise from 3 o'clock: right -> bottom.
            t.arc_outline(
                Arc {
                    x: 0,
                    y: 0,
                    width: 18,
                    height: 18,
                    angle1: 0,
                    angle2: 90 * 64,
                },
                1,
            );
        }
        assert_eq!(s.pixel(18, 9), Some(1), "3 o'clock start of the arc");
        assert_eq!(s.pixel(9, 18), Some(1), "6 o'clock end (clockwise)");
        assert_eq!(s.pixel(0, 9), Some(0), "9 o'clock not drawn");
        assert_eq!(s.pixel(9, 0), Some(0), "12 o'clock not drawn");
    }

    #[test]
    fn filled_pie_quarter_covers_the_wedge() {
        let mut s = Store::filled(12, 12, 0);
        {
            let mut t = Target::full(&mut s, Gx::Copy, 0x00ff_ffff);
            t.fill_arc(
                Arc {
                    x: 0,
                    y: 0,
                    width: 11,
                    height: 11,
                    angle1: 0,
                    angle2: 90 * 64,
                },
                1,
                true,
            );
        }
        assert_eq!(s.pixel(5, 5), Some(1), "center apex of the wedge");
        assert_eq!(s.pixel(10, 5), Some(1), "3 o'clock edge region");
        assert_eq!(s.pixel(5, 10), Some(1), "6 o'clock edge region");
        assert_eq!(s.pixel(5, 0), Some(0), "12 o'clock (opposite the sweep)");
        assert_eq!(s.pixel(0, 5), Some(0), "9 o'clock (before the start)");
    }

    #[test]
    fn chord_mode_excludes_the_pie_apex() {
        let mut s = Store::filled(12, 12, 0);
        {
            let mut t = Target::full(&mut s, Gx::Copy, 0x00ff_ffff);
            t.fill_arc(
                Arc {
                    x: 0,
                    y: 0,
                    width: 11,
                    height: 11,
                    angle1: 0,
                    angle2: 90 * 64,
                },
                1,
                false,
            );
        }
        // The chord runs from (11, 5.5) to (5.5, 11): the region is the
        // bulge between chord and arc, not the center wedge.
        assert_eq!(s.pixel(5, 5), Some(0), "pie apex excluded in chord mode");
        assert_eq!(s.pixel(9, 8), Some(1), "arc-side segment filled");
        assert_eq!(s.pixel(10, 10), Some(0), "outside the ellipse");
    }

    #[test]
    fn chord_mode_major_arc_keeps_the_far_side() {
        let mut s = Store::filled(12, 12, 0);
        {
            let mut t = Target::full(&mut s, Gx::Copy, 0x00ff_ffff);
            t.fill_arc(
                Arc {
                    x: 0,
                    y: 0,
                    width: 11,
                    height: 11,
                    angle1: 0,
                    angle2: 270 * 64,
                },
                1,
                false,
            );
        }
        // A 270-degree chord closes the major arc: the region is
        // everything except the 90-degree bulge.
        assert_eq!(s.pixel(5, 5), Some(1), "center inside the major segment");
        assert_eq!(s.pixel(10, 10), Some(0), "inside the excluded bulge");
    }

    #[test]
    fn even_odd_and_winding_differ_on_nested_squares() {
        // Outer square and inner square with the same winding direction,
        // joined by a zero-width cut that cancels on both rules.
        let pts = [
            (0, 0),
            (12, 0),
            (12, 12),
            (0, 12),
            (0, 0),
            (4, 4),
            (8, 4),
            (8, 8),
            (4, 8),
            (4, 4),
        ];
        let mut eo = Store::filled(14, 14, 0);
        {
            let mut t = Target::full(&mut eo, Gx::Copy, 0x00ff_ffff);
            t.fill_polygon(&pts, 1, false);
        }
        let mut wz = Store::filled(14, 14, 0);
        {
            let mut t = Target::full(&mut wz, Gx::Copy, 0x00ff_ffff);
            t.fill_polygon(&pts, 1, true);
        }
        // Outer body filled under both rules.
        assert_eq!(eo.pixel(2, 6), Some(1));
        assert_eq!(wz.pixel(2, 6), Some(1));
        assert_eq!(eo.pixel(10, 6), Some(1));
        // Inner square: a hole for even-odd, doubly-wound (filled) for
        // the nonzero rule.
        assert_eq!(eo.pixel(6, 6), Some(0), "even-odd leaves the hole");
        assert_eq!(wz.pixel(6, 6), Some(1), "winding fills the nested loop");
    }

    #[test]
    fn rectangle_polygon_matches_fill_rect() {
        // A polygon's corner vertices are exclusive: (9,9) closes the
        // pixel box that FillRectangle(2,2,7,7) covers.
        let pts = [(2, 2), (9, 2), (9, 9), (2, 9)];
        let mut a = Store::filled(12, 12, 0);
        let mut b = Store::filled(12, 12, 0);
        {
            let mut t = Target::full(&mut a, Gx::Copy, 0x00ff_ffff);
            t.fill_polygon(&pts, 1, false);
        }
        {
            let mut t = Target::full(&mut b, Gx::Copy, 0x00ff_ffff);
            t.fill_rect(2, 2, 7, 7, 1);
        }
        for y in 0..12 {
            for x in 0..12 {
                assert_eq!(a.pixel(x, y), b.pixel(x, y), "({x},{y})");
            }
        }
    }
}
