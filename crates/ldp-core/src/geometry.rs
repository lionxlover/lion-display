//! Geometry and damage algebra.
//!
//! Layout coordinates are [`i32`] logical pixels; sizes are [`u32`].
//! [`Region`] is an ordered rect list with the operations the compositor
//! needs for damage propagation and occlusion subtraction — including full
//! [`Region::subtract`], the workhorse of damage tracking.

use core::fmt;

/// Integer point in logical coordinates.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default, Debug)]
pub struct Point {
    /// X coordinate.
    pub x: i32,
    /// Y coordinate.
    pub y: i32,
}

impl Point {
    /// Construct from coordinates.
    pub const fn new(x: i32, y: i32) -> Point {
        Point { x, y }
    }
}

/// Subpixel point for input event coordinates (pointer, touch, stylus).
#[derive(Clone, Copy, PartialEq, Default, Debug)]
pub struct PointF {
    /// X coordinate.
    pub x: f32,
    /// Y coordinate.
    pub y: f32,
}

impl PointF {
    /// Construct from coordinates.
    pub const fn new(x: f32, y: f32) -> PointF {
        PointF { x, y }
    }

    /// Round to the nearest integer point (half away from zero).
    #[must_use]
    pub fn round(&self) -> Point {
        Point::new(self.x.round() as i32, self.y.round() as i32)
    }
}

/// Size in logical pixels. Non-negative.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default, Debug)]
pub struct Size {
    /// Width in pixels.
    pub w: u32,
    /// Height in pixels.
    pub h: u32,
}

impl Size {
    /// Construct from width/height.
    pub const fn new(w: u32, h: u32) -> Size {
        Size { w, h }
    }

    /// Whether either dimension is zero.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.w == 0 || self.h == 0
    }

    /// Area in pixels, saturating at [`u64::MAX`].
    #[must_use]
    pub const fn area(self) -> u64 {
        (self.w as u64).saturating_mul(self.h as u64)
    }
}

/// Axis-aligned rectangle. `w`/`h` of zero mark the empty rectangle;
/// coordinates may be negative (layouts can start off-origin).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Rect {
    /// Left edge.
    pub x: i32,
    /// Top edge.
    pub y: i32,
    /// Width; 0 = empty.
    pub w: u32,
    /// Height; 0 = empty.
    pub h: u32,
}

impl fmt::Display for Rect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}x{}+{}+{}", self.w, self.h, self.x, self.y)
    }
}

impl Rect {
    /// Construct from position and size.
    pub const fn new(x: i32, y: i32, w: u32, h: u32) -> Rect {
        Rect { x, y, w, h }
    }

    /// The empty rectangle.
    pub const EMPTY: Rect = Rect {
        x: 0,
        y: 0,
        w: 0,
        h: 0,
    };

    /// Whether width or height is zero.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.w == 0 || self.h == 0
    }

    /// Exclusive right edge; saturates at [`i32::MAX`] for huge widths.
    #[must_use]
    pub fn right(self) -> i32 {
        self.x.saturating_add(self.w.min(i32::MAX as u32) as i32)
    }

    /// Exclusive bottom edge.
    #[must_use]
    pub fn bottom(self) -> i32 {
        self.y.saturating_add(self.h.min(i32::MAX as u32) as i32)
    }

    /// Size as a [`Size`].
    #[must_use]
    pub const fn size(self) -> Size {
        Size {
            w: self.w,
            h: self.h,
        }
    }

    /// Whether `p` lies inside (right/bottom exclusive).
    #[must_use]
    pub fn contains_point(self, p: Point) -> bool {
        !self.is_empty()
            && p.x >= self.x
            && p.y >= self.y
            && p.x < self.right()
            && p.y < self.bottom()
    }

    /// Overlap of two rectangles; `None` when disjoint or either is empty.
    #[must_use]
    pub fn intersect(self, other: Rect) -> Option<Rect> {
        if self.is_empty() || other.is_empty() {
            return None;
        }
        let left = self.x.max(other.x);
        let top = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        if right <= left || bottom <= top {
            return None;
        }
        // i64 differences: i32 edge arithmetic can overflow near extremes.
        Some(Rect::new(
            left,
            top,
            (right as i64 - left as i64) as u32,
            (bottom as i64 - top as i64) as u32,
        ))
    }

    /// Bounding box of two rectangles; empty when both are empty. A single
    /// empty rectangle contributes nothing.
    #[must_use]
    pub fn union(self, other: Rect) -> Rect {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return self;
        }
        let left = self.x.min(other.x);
        let top = self.y.min(other.y);
        let right = self.right().max(other.right());
        let bottom = self.bottom().max(other.bottom());
        Rect::new(
            left,
            top,
            (right as i64 - left as i64).max(0) as u32,
            (bottom as i64 - top as i64).max(0) as u32,
        )
    }

    /// Translate by a delta.
    #[must_use]
    pub const fn translate(self, dx: i32, dy: i32) -> Rect {
        Rect {
            x: self.x.saturating_add(dx),
            y: self.y.saturating_add(dy),
            w: self.w,
            h: self.h,
        }
    }

    /// Inset by fixed amounts on each side; saturates (an inset larger than
    /// the rect yields empty).
    #[must_use]
    pub fn inset(self, left: i32, top: i32, right: i32, bottom: i32) -> Rect {
        let x = self.x.saturating_add(left);
        let y = self.y.saturating_add(top);
        let w = (self.w as i64 - left as i64 - right as i64).max(0) as u32;
        let h = (self.h as i64 - top as i64 - bottom as i64).max(0) as u32;
        Rect { x, y, w, h }
    }

    /// Subtract `cutter` from this rectangle, yielding 0..4 rectangles that
    /// cover `self \ cutter` exactly. This is the primitive underneath
    /// [`Region::subtract`].
    #[must_use]
    pub fn subtract(self, cutter: Rect) -> SmallRects {
        let mut out = SmallRects::default();
        let Some(hole) = self.intersect(cutter) else {
            if !self.is_empty() {
                out.push(self);
            }
            return out;
        };
        // Top band.
        if hole.y > self.y {
            out.push(Rect::new(
                self.x,
                self.y,
                self.w,
                (hole.y as i64 - self.y as i64) as u32,
            ));
        }
        // Bottom band.
        let hole_bottom = hole.bottom();
        let self_bottom = self.bottom();
        if hole_bottom < self_bottom {
            out.push(Rect::new(
                self.x,
                hole_bottom,
                self.w,
                (self_bottom as i64 - hole_bottom as i64) as u32,
            ));
        }
        // Left strip (between the bands' vertical range).
        if hole.x > self.x {
            out.push(Rect::new(
                self.x,
                hole.y,
                (hole.x as i64 - self.x as i64) as u32,
                hole.h,
            ));
        }
        // Right strip.
        let hole_right = hole.right();
        let self_right = self.right();
        if hole_right < self_right {
            out.push(Rect::new(
                hole_right,
                hole.y,
                (self_right as i64 - hole_right as i64) as u32,
                hole.h,
            ));
        }
        out
    }
}

/// Fixed-capacity rect list (≤4) returned by [`Rect::subtract`].
#[derive(Clone, Copy)]
pub struct SmallRects {
    items: [Rect; 4],
    len: usize,
}

impl Default for SmallRects {
    fn default() -> Self {
        SmallRects {
            items: [Rect::EMPTY; 4],
            len: 0,
        }
    }
}

impl SmallRects {
    fn push(&mut self, r: Rect) {
        if self.len < 4 && !r.is_empty() {
            self.items[self.len] = r;
            self.len += 1;
        }
    }

    /// Number of rectangles produced.
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Whether no rectangles were produced.
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Iterate the produced rectangles.
    pub fn iter(&self) -> core::slice::Iter<'_, Rect> {
        self.items[..self.len].iter()
    }
}

impl<'a> IntoIterator for &'a SmallRects {
    type Item = &'a Rect;
    type IntoIter = core::slice::Iter<'a, Rect>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// A region: an ordered list of rectangles (possibly overlapping — regions
/// are used as damage sets where union-approximation is acceptable; exact
/// subtraction is provided for occlusion math).
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Region {
    rects: Vec<Rect>,
}

impl<'a> IntoIterator for &'a Region {
    type Item = &'a Rect;
    type IntoIter = core::slice::Iter<'a, Rect>;
    fn into_iter(self) -> Self::IntoIter {
        self.rects.iter()
    }
}

impl Region {
    /// An empty region.
    pub const fn new() -> Region {
        Region { rects: Vec::new() }
    }

    /// Region of a single rectangle.
    pub fn from_rect(r: Rect) -> Region {
        let mut region = Region::new();
        region.add(r);
        region
    }

    /// Add a rectangle (union-append; keeps `other` rects for fidelity).
    pub fn add(&mut self, r: Rect) {
        if !r.is_empty() {
            self.rects.push(r);
        }
    }

    /// Union another region in.
    pub fn add_region(&mut self, other: &Region) {
        self.rects.extend_from_slice(&other.rects);
    }

    /// Whether the region contains no rectangles.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rects.is_empty()
    }

    /// Number of rectangles.
    #[must_use]
    pub fn len(&self) -> usize {
        self.rects.len()
    }

    /// Iterate rectangles.
    pub fn iter(&self) -> core::slice::Iter<'_, Rect> {
        self.rects.iter()
    }

    /// Bounding box of all rectangles (empty region → empty rect).
    #[must_use]
    pub fn bounds(&self) -> Rect {
        let mut acc = Rect::EMPTY;
        for r in &self.rects {
            acc = acc.union(*r);
        }
        acc
    }

    /// Whether any rectangle contains the point.
    #[must_use]
    pub fn contains_point(&self, p: Point) -> bool {
        self.rects.iter().any(|r| r.contains_point(p))
    }

    /// Translate every rectangle.
    #[must_use]
    pub fn translate(&self, dx: i32, dy: i32) -> Region {
        Region {
            rects: self.rects.iter().map(|r| r.translate(dx, dy)).collect(),
        }
    }

    /// Clip every rectangle to `clip`, dropping empty results.
    #[must_use]
    pub fn clipped_to(&self, clip: Rect) -> Region {
        let mut out = Region::new();
        for r in &self.rects {
            if let Some(c) = r.intersect(clip) {
                out.add(c);
            }
        }
        out
    }

    /// Exact set difference `self \ other`, computed by rect subtraction.
    /// Output rectangle count is bounded by
    /// `len(self) * (1 + 4 * len(other))` — callers enforce the protocol
    /// damage-rect limit on *inputs*, which bounds this.
    ///
    /// Two scratch buffers are swapped per cutter, so the subtraction
    /// performs O(1) allocations regardless of how many cutters apply
    /// (the swapped buffers retain the high-water capacity), and
    /// rectangles disjoint from a cutter pass through without entering
    /// the four-way split machinery — the common case when damage
    /// holes sit beside, not on, the region.
    #[must_use]
    pub fn subtract(&self, other: &Region) -> Region {
        let mut current: Vec<Rect> = Vec::with_capacity(self.rects.len());
        current.extend_from_slice(&self.rects);
        let mut scratch: Vec<Rect> = Vec::with_capacity(self.rects.len() * 2 + 4);
        for cutter in &other.rects {
            scratch.clear();
            for r in &current {
                if r.intersect(*cutter).is_none() {
                    scratch.push(*r);
                    continue;
                }
                for piece in &r.subtract(*cutter) {
                    scratch.push(*piece);
                }
            }
            std::mem::swap(&mut current, &mut scratch);
        }
        Region { rects: current }
    }

    /// Exact difference against a single rectangle.
    #[must_use]
    pub fn subtract_rect(&self, cutter: Rect) -> Region {
        self.subtract(&Region::from_rect(cutter))
    }

    /// Clear the region.
    pub fn clear(&mut self) {
        self.rects.clear();
    }
}

/// Buffer-to-surface orientation (`ldp.core.transform` on the wire).
///
/// Rotations are clockwise when reading the buffer onto the surface.
/// Mirroring composes: `flipped90` = flip horizontally, then rotate 90°.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
#[non_exhaustive]
pub enum Transform {
    /// Identity.
    #[default]
    Normal,
    /// 90° clockwise.
    Rot90,
    /// 180°.
    Rot180,
    /// 270° clockwise (90° counter-clockwise).
    Rot270,
    /// Horizontal mirror.
    Flipped,
    /// Mirror then 90° clockwise.
    Flipped90,
    /// Mirror then 180°.
    Flipped180,
    /// Mirror then 270° clockwise.
    Flipped270,
}

impl Transform {
    /// Wire value (`ldp.core.transform` enum, dense 1..=8).
    #[must_use]
    pub const fn to_wire(self) -> u32 {
        match self {
            Self::Normal => 1,
            Self::Rot90 => 2,
            Self::Rot180 => 3,
            Self::Rot270 => 4,
            Self::Flipped => 5,
            Self::Flipped90 => 6,
            Self::Flipped180 => 7,
            Self::Flipped270 => 8,
        }
    }

    /// Parse a wire value; `None` for unknown codes.
    #[must_use]
    pub const fn from_wire(v: u32) -> Option<Transform> {
        match v {
            1 => Some(Self::Normal),
            2 => Some(Self::Rot90),
            3 => Some(Self::Rot180),
            4 => Some(Self::Rot270),
            5 => Some(Self::Flipped),
            6 => Some(Self::Flipped90),
            7 => Some(Self::Flipped180),
            8 => Some(Self::Flipped270),
            _ => None,
        }
    }

    /// The inverse transform (undoes `self`).
    ///
    /// Rotations pair up (`Rot90` ↔ `Rot270`); `Rot180` and every mirror
    /// variant are involutions — applying them twice is the identity —
    /// which follows from the mirror-then-rotate composition convention.
    #[must_use]
    pub const fn inverse(self) -> Transform {
        match self {
            Self::Normal | Self::Rot180 | Self::Flipped | Self::Flipped180 => self,
            Self::Rot90 => Self::Rot270,
            Self::Rot270 => Self::Rot90,
            Self::Flipped90 => Self::Flipped90,
            Self::Flipped270 => Self::Flipped270,
        }
    }

    /// Whether the transform swaps width and height.
    #[must_use]
    pub const fn swaps_axes(self) -> bool {
        matches!(
            self,
            Self::Rot90 | Self::Rot270 | Self::Flipped90 | Self::Flipped270
        )
    }

    /// Map a buffer-space pixel-center `(x, y)` of a `w x h` buffer into
    /// surface space. Coordinates are half-open `[0, w)`/`[0, h)`; the
    /// mapping is the standard DRM-style scanout transform (mirror first,
    /// then rotate clockwise).
    #[must_use]
    pub fn transform_point(self, x: i32, y: i32, w: u32, h: u32) -> (i32, i32) {
        let (w, h) = (w as i32, h as i32);
        match self {
            Self::Normal => (x, y),
            Self::Rot90 => (h - 1 - y, x),
            Self::Rot180 => (w - 1 - x, h - 1 - y),
            Self::Rot270 => (y, w - 1 - x),
            Self::Flipped => (w - 1 - x, y),
            Self::Flipped90 => (h - 1 - y, w - 1 - x),
            Self::Flipped180 => (x, h - 1 - y),
            Self::Flipped270 => (y, x), // transpose: mirror + 270° clockwise
        }
    }

    /// Map a rectangle through the transform by mapping corners and taking
    /// the bounding box. `w`/`h` are the *source* buffer dimensions.
    #[must_use]
    pub fn transform_rect(self, r: Rect, w: u32, h: u32) -> Rect {
        if r.is_empty() || w == 0 || h == 0 {
            return Rect::EMPTY;
        }
        let corners = [
            self.transform_point(r.x, r.y, w, h),
            self.transform_point(r.right() - 1, r.y, w, h),
            self.transform_point(r.x, r.bottom() - 1, w, h),
            self.transform_point(r.right() - 1, r.bottom() - 1, w, h),
        ];
        let min_x = corners.iter().map(|c| c.0).min().unwrap_or(0);
        let max_x = corners.iter().map(|c| c.0).max().unwrap_or(0);
        let min_y = corners.iter().map(|c| c.1).min().unwrap_or(0);
        let max_y = corners.iter().map(|c| c.1).max().unwrap_or(0);
        Rect::new(
            min_x,
            min_y,
            (max_x - min_x + 1) as u32,
            (max_y - min_y + 1) as u32,
        )
    }

    /// The surface-space size of a `w x h` buffer under this transform.
    #[must_use]
    pub fn transform_size(self, s: Size) -> Size {
        if self.swaps_axes() {
            Size::new(s.h, s.w)
        } else {
            s
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn containment_and_edges() {
        let r = Rect::new(10, 20, 30, 40);
        assert_eq!(r.right(), 40);
        assert_eq!(r.bottom(), 60);
        assert!(r.contains_point(Point::new(10, 20)));
        assert!(r.contains_point(Point::new(39, 59)));
        assert!(!r.contains_point(Point::new(40, 59)));
        assert!(!r.contains_point(Point::new(39, 60)));
        assert!(!Rect::EMPTY.contains_point(Point::new(0, 0)));
    }

    #[test]
    fn intersect_union_basics() {
        let a = Rect::new(0, 0, 10, 10);
        let b = Rect::new(5, 5, 10, 10);
        assert_eq!(a.intersect(b), Some(Rect::new(5, 5, 5, 5)));
        assert_eq!(a.intersect(Rect::new(20, 20, 5, 5)), None);
        assert_eq!(a.intersect(Rect::EMPTY), None);
        assert_eq!(a.union(b), Rect::new(0, 0, 15, 15));
        assert_eq!(a.union(Rect::EMPTY), a);
        assert_eq!(Rect::EMPTY.union(a), a);
    }

    #[test]
    fn inset_saturates_to_empty() {
        let r = Rect::new(0, 0, 10, 10);
        assert_eq!(r.inset(2, 2, 2, 2), Rect::new(2, 2, 6, 6));
        assert!(r.inset(100, 0, 0, 0).is_empty());
        assert_eq!(r.inset(0, 0, 0, 0), r);
    }

    #[test]
    fn subtract_fully_covered_yields_nothing() {
        let a = Rect::new(0, 0, 10, 10);
        let cover = Rect::new(-5, -5, 20, 20);
        assert!(a.subtract(cover).is_empty());
        assert_eq!(a.subtract(a).len(), 0);
    }

    #[test]
    fn subtract_disjoint_yields_self() {
        let a = Rect::new(0, 0, 10, 10);
        let pieces = a.subtract(Rect::new(20, 20, 5, 5));
        assert_eq!(pieces.len(), 1);
        assert_eq!(*pieces.iter().next().unwrap(), a);
    }

    #[test]
    fn subtract_center_hole_yields_four() {
        let a = Rect::new(0, 0, 10, 10);
        let hole = Rect::new(3, 3, 4, 4);
        let pieces: Vec<Rect> = a.subtract(hole).iter().copied().collect();
        assert_eq!(pieces.len(), 4);
        // Re-assembling the pieces plus the hole must cover the original.
        let mut covered = Region::new();
        for p in &pieces {
            covered.add(*p);
        }
        covered.add(hole);
        assert_eq!(covered.bounds(), a);
        // And each piece must avoid the hole.
        for p in &pieces {
            assert!(p.intersect(hole).is_none(), "piece {p:?} overlaps hole");
        }
    }

    #[test]
    fn subtract_edge_cases() {
        // Cutter flush with the left edge: 3 pieces (top, bottom, right).
        let a = Rect::new(0, 0, 10, 10);
        let cutter = Rect::new(0, 2, 3, 3);
        assert_eq!(a.subtract(cutter).len(), 3);
        // Cutter flush with top and left: 1 piece (bottom-right remainder
        // splits into bottom band + right strip => 2).
        let cutter = Rect::new(0, 0, 3, 3);
        assert_eq!(a.subtract(cutter).len(), 2);
    }

    #[test]
    fn region_subtract_exactness() {
        // Property: region.subtract(other) ∪ other ⊇ region, and no output
        // rect intersects `other`.
        let region = Region::from_rect(Rect::new(0, 0, 100, 100));
        let mut holes = Region::new();
        holes.add(Rect::new(10, 10, 20, 20));
        holes.add(Rect::new(50, 50, 30, 30));
        holes.add(Rect::new(15, 15, 10, 10)); // nested hole
        let diff = region.subtract(&holes);
        for r in &diff {
            for h in &holes {
                assert!(
                    r.intersect(*h).is_none(),
                    "diff rect {r:?} overlaps hole {h:?}"
                );
            }
        }
        // Area conservation: area(region) == area(diff) + area(region ∩ holes)
        let area = |reg: &Region| -> u64 { reg.iter().map(|r| r.size().area()).sum() };
        // holes overlap each other; compute covered area exactly by rasterizing.
        let mut covered: u64 = 0;
        for y in 0..100 {
            for x in 0..100 {
                if holes.contains_point(Point::new(x, y)) {
                    covered += 1;
                }
            }
        }
        assert_eq!(area(&region), area(&diff) + covered);
    }

    #[test]
    fn region_ops() {
        let mut r = Region::new();
        r.add(Rect::new(0, 0, 5, 5));
        r.add(Rect::new(10, 10, 5, 5));
        r.add(Rect::EMPTY);
        assert_eq!(r.len(), 2);
        assert_eq!(r.bounds(), Rect::new(0, 0, 15, 15));
        assert!(r.contains_point(Point::new(12, 12)));
        let t = r.translate(1, 1);
        assert!(t.contains_point(Point::new(13, 13)));
        let clipped = r.clipped_to(Rect::new(0, 0, 6, 6));
        assert_eq!(clipped.len(), 1);
        r.clear();
        assert!(r.is_empty());
    }

    #[test]
    fn transform_point_round_trips_through_inverse() {
        let cases = [
            (Transform::Rot90, 17u32, 29u32, 7i32, 11i32),
            (Transform::Rot180, 17, 29, 7, 11),
            (Transform::Rot270, 17, 29, 7, 11),
            (Transform::Flipped, 17, 29, 7, 11),
            (Transform::Flipped90, 17, 29, 7, 11),
            (Transform::Flipped180, 17, 29, 7, 11),
            (Transform::Flipped270, 17, 29, 7, 11),
        ];
        for (t, w, h, x, y) in cases {
            let (sx, sy) = t.transform_point(x, y, w, h);
            let (bx, by) = t.inverse().transform_point(
                sx,
                sy,
                t.transform_size(Size::new(w, h)).w,
                t.transform_size(Size::new(w, h)).h,
            );
            assert_eq!((bx, by), (x, y), "transform {t:?} round trip failed");
        }
    }

    #[test]
    fn transform_wire_round_trip() {
        for v in 1..=8u32 {
            let t = Transform::from_wire(v).unwrap();
            assert_eq!(t.to_wire(), v);
        }
        assert!(Transform::from_wire(0).is_none());
        assert!(Transform::from_wire(9).is_none());
        assert_eq!(Transform::default(), Transform::Normal);
    }

    #[test]
    fn transform_rect_identity_and_swap() {
        let r = Rect::new(1, 2, 8, 6);
        assert_eq!(Transform::Normal.transform_rect(r, 10, 10), r);
        let swapped = Transform::Rot90.transform_rect(r, 10, 10);
        assert_eq!(swapped.size(), Size::new(6, 8));
        assert!(Transform::Rot90.swaps_axes());
        assert!(!Transform::Flipped.swaps_axes());
    }

    #[test]
    fn saturating_edges_do_not_panic() {
        let r = Rect::new(i32::MIN, i32::MIN, u32::MAX, u32::MAX);
        let _ = r.right();
        let _ = r.bottom();
        let u = r.union(Rect::new(0, 0, 1, 1));
        let _ = u.right();
        let _ = r.translate(i32::MAX, i32::MAX);
    }
}
