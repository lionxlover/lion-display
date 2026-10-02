//! Output-to-buffer pixel mappings.
//!
//! Three shapes: [`Mapping::Direct`] (1:1 untransformed — pure integer
//! offsets), [`Mapping::Integer`] (1:1 rotated/mirrored — the exact integer
//! inverse of the core's forward `transform_point`), and [`Mapping::Scaled`]
//! (continuous inverse on the pixel-center grid, nearest-sample). The
//! `FrameTarget`/`RowSpan` bundles carry the span kernels' shared state.

use ldp_core::buffer::FourCC;
use ldp_core::geometry::Transform;

/// The output-pixel → buffer-pixel inverse mapping of a layer.
pub(crate) enum Mapping {
    /// 1:1 untransformed: `bx = x - ox`, `by = y - oy`.
    Direct {
        /// Destination origin x.
        ox: i32,
        /// Destination origin y.
        oy: i32,
    },
    /// 1:1 rotated/mirrored: integer inverse of the transform.
    Integer {
        /// The transform.
        t: Transform,
        /// Buffer width.
        w: i32,
        /// Buffer height.
        h: i32,
        /// Destination origin x.
        ox: i32,
        /// Destination origin y.
        oy: i32,
    },
    /// Scaled: continuous inverse mapping, nearest-sample.
    Scaled {
        /// The transform.
        t: Transform,
        /// Buffer width.
        w: i32,
        /// Buffer height.
        h: i32,
        /// Destination origin x.
        ox: f32,
        /// Destination origin y.
        oy: f32,
        /// Surface-x per output-x factor (`surface_w / dest.w`).
        ax: f32,
        /// Surface-y per output-y factor (`surface_h / dest.h`).
        ay: f32,
    },
}

impl Mapping {
    /// Map an output pixel to its buffer pixel; `None` for coordinates that
    /// round outside the buffer (possible only on scaled edges).
    #[inline]
    #[allow(clippy::similar_names)] // coordinate plumbing: x/y axes are the domain's names
    pub(crate) fn map(&self, x: i32, y: i32) -> Option<(u32, u32)> {
        match *self {
            Mapping::Direct { ox, oy } => Some(((x - ox) as u32, (y - oy) as u32)),
            Mapping::Integer { t, w, h, ox, oy } => {
                let (bx, by) = inverse_1to1(t, x - ox, y - oy, w, h);
                Some((bx as u32, by as u32))
            }
            Mapping::Scaled {
                t,
                w,
                h,
                ox,
                oy,
                ax,
                ay,
            } => {
                let surf_x = ((x as f32) + 0.5 - ox) * ax;
                let surf_y = ((y as f32) + 0.5 - oy) * ay;
                let (buf_xc, buf_yc) = inverse_cont(t, surf_x, surf_y, w as f32, h as f32);
                let buf_x = buf_xc.floor();
                let buf_y = buf_yc.floor();
                if buf_x >= 0.0 && buf_x < w as f32 && buf_y >= 0.0 && buf_y < h as f32 {
                    Some((buf_x as u32, buf_y as u32))
                } else {
                    None
                }
            }
        }
    }
}

/// Integer 1:1 inverse of [`Transform`] (pixel-index space).
///
/// `Transform` is `#[non_exhaustive]`; future variants fall back to the
/// identity mapping until the renderer teaches them (deterministic, never
/// a panic).
pub(crate) fn inverse_1to1(t: Transform, sx: i32, sy: i32, w: i32, h: i32) -> (i32, i32) {
    match t {
        Transform::Rot90 => (sy, h - 1 - sx),
        Transform::Rot180 => (w - 1 - sx, h - 1 - sy),
        Transform::Rot270 => (w - 1 - sy, sx),
        Transform::Flipped => (w - 1 - sx, sy),
        Transform::Flipped90 => (w - 1 - sy, h - 1 - sx),
        Transform::Flipped180 => (sx, h - 1 - sy),
        Transform::Flipped270 => (sy, sx),
        // Normal (and future variants) is the identity.
        _ => (sx, sy),
    }
}

/// Continuous inverse of [`Transform`] (pixel-center space, pixel `i` covers
/// `[i, i+1)`); the scaled path's sampling grid. Same identity fallback for
/// future variants as [`inverse_1to1`].
pub(crate) fn inverse_cont(t: Transform, sx: f32, sy: f32, w: f32, h: f32) -> (f32, f32) {
    match t {
        Transform::Rot90 => (sy, h - sx),
        Transform::Rot180 => (w - sx, h - sy),
        Transform::Rot270 => (w - sy, sx),
        Transform::Flipped => (w - sx, sy),
        Transform::Flipped90 => (w - sy, h - sx),
        Transform::Flipped180 => (sx, h - sy),
        Transform::Flipped270 => (sy, sx),
        // Normal (and future variants) is the identity.
        _ => (sx, sy),
    }
}

/// The frame's write target: framebuffer, its geometry, and the output
/// format (bundles the span kernels' shared arguments).
pub(crate) struct FrameTarget<'a> {
    /// The framebuffer, row-major words.
    pub fb: &'a mut [u32],
    /// Framebuffer width.
    pub width: u32,
    /// Framebuffer height.
    pub height: u32,
    /// Output format.
    pub format: FourCC,
}

/// One merged row span to composite (`[x0, x1)` of row `y`).
pub(crate) struct RowSpan {
    /// Row-major word offset of the row's start.
    pub row_base: usize,
    /// The row's y coordinate (output space).
    pub y: i32,
    /// Span start (inclusive).
    pub x0: i32,
    /// Span end (exclusive).
    pub x1: i32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inverse_1to1_undoes_forward() {
        let (w, h) = (7i32, 5i32);
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
            for by in 0..h {
                for bx in 0..w {
                    let (fx, fy) = t.transform_point(bx, by, w as u32, h as u32);
                    let back = inverse_1to1(t, fx, fy, w, h);
                    assert_eq!(back, (bx, by), "{t:?} ({bx},{by})");
                }
            }
        }
    }

    #[test]
    fn integer_and_scaled_mappings_agree_at_exact_centers() {
        // For a 1:1 dest the continuous inverse must land exactly on the
        // integer inverse (pixel centers map to pixel centers).
        let (w, h) = (6i32, 4i32);
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
            let m_int = Mapping::Integer {
                t,
                w,
                h,
                ox: 0,
                oy: 0,
            };
            let m_scaled = Mapping::Scaled {
                t,
                w,
                h,
                ox: 0.0,
                oy: 0.0,
                ax: 1.0,
                ay: 1.0,
            };
            // The surface (and a 1:1 dest) is transform-swapped.
            let (sw, sh) = if t.swaps_axes() { (h, w) } else { (w, h) };
            for y in 0..sh {
                for x in 0..sw {
                    assert_eq!(
                        m_int.map(x, y).map(|v| (v.0 as i32, v.1 as i32)),
                        m_scaled.map(x, y).map(|v| (v.0 as i32, v.1 as i32)),
                        "{t:?} ({x},{y})"
                    );
                }
            }
        }
    }
}
