//! Software rasterizer for the bridge's window backing stores.
//!
//! One [`Store`] is a depth-24 ZPixmap at 32 bits per pixel: memory
//! bytes `B, G, R, X` per pixel — byte-identical to LDP's XRGB8888 shm
//! format, so an LDP buffer view over a window's store needs no pixel
//! conversion. All drawing goes through a [`Target`], which carries the
//! clip rectangles (GC clip list intersected with the drawable bounds
//! and subwindow clipping), the raster operation and the plane mask,
//! and answers every op with the damaged bounding box.
//!
//! Rasterization rules (pinned by unit tests with hand-computed
//! vectors):
//!
//! * **Lines** — integer Bresenham between endpoints; a polyline is one
//!   path (dash state and joins continue across segments), and the
//!   default `NotLast` cap skips the path's final pixel. Line width
//!   above zero stamps a square brush centered on each path pixel
//!   (documented approximation; cap/join styles are not modeled).
//! * **Rectangles** — outline uses four thin lines (without `NotLast`
//!   so corners land); fill covers pixel centers inside.
//! * **Arcs** — the arc rect's center is `(x + w/2, y + h/2)`; angles
//!   run clockwise from 3 o'clock (screen Y grows down), in 1/64
//!   degree units. Outline samples the arc so no step exceeds half a
//!   pixel along the larger radius; fills cover pixel centers inside
//!   the ellipse, in the wedge (pie) or on the arc's side of the chord
//!   (chord mode).
//! * **Polygons** — scanline fill over pixel centers with both
//!   even-odd and winding rules.
//! * **Raster ops** — all 16 GX functions, then the plane mask:
//!   `out = (rop(src, dst) & mask) | (dst & !mask)`.

#![forbid(unsafe_code)]

use ldp_core::geometry::Rect;

/// A depth-24 ZPixmap backing store (32 bpp, rows tightly packed).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Store {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Pixel data, `width * height * 4` bytes.
    pub data: Vec<u8>,
}

impl Store {
    /// A store filled with one pixel value.
    #[must_use]
    pub fn filled(width: u32, height: u32, pixel: u32) -> Store {
        Store {
            width,
            height,
            data: px_bytes(pixel).repeat((width as usize) * (height as usize)),
        }
    }

    /// Read one pixel (XRGB8888 value) if in bounds.
    #[must_use]
    pub fn pixel(&self, x: i32, y: i32) -> Option<u32> {
        if x < 0 || y < 0 {
            return None;
        }
        let (x, y) = (x as usize, y as usize);
        if x >= self.width as usize || y >= self.height as usize {
            return None;
        }
        let at = (y * self.width as usize + x) * 4;
        Some(
            u32::from(self.data[at])
                | (u32::from(self.data[at + 1]) << 8)
                | (u32::from(self.data[at + 2]) << 16),
        )
    }

    /// The store's byte size (pool sizing for the LDP export).
    #[must_use]
    pub fn byte_len(&self) -> usize {
        self.data.len()
    }

    /// Bytes for one pixel in memory order.
    #[must_use]
    pub fn row_bytes(&self) -> usize {
        self.width as usize * 4
    }
}

fn px_bytes(pixel: u32) -> [u8; 4] {
    [pixel as u8, (pixel >> 8) as u8, (pixel >> 16) as u8, 0]
}

/// The 16 GX raster operations.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[allow(missing_docs)] // the names are the protocol's
pub enum Gx {
    Clear,
    And,
    AndReverse,
    Copy,
    AndInverted,
    Noop,
    Xor,
    Or,
    Nor,
    Equiv,
    Invert,
    OrReverse,
    CopyInverted,
    OrInverted,
    Nand,
    Set,
}

impl Gx {
    /// Map a wire value (0..=15) to a GX function.
    #[must_use]
    pub const fn from_wire(v: u8) -> Option<Gx> {
        Some(match v {
            0 => Gx::Clear,
            1 => Gx::And,
            2 => Gx::AndReverse,
            3 => Gx::Copy,
            4 => Gx::AndInverted,
            5 => Gx::Noop,
            6 => Gx::Xor,
            7 => Gx::Or,
            8 => Gx::Nor,
            9 => Gx::Equiv,
            10 => Gx::Invert,
            11 => Gx::OrReverse,
            12 => Gx::CopyInverted,
            13 => Gx::OrInverted,
            14 => Gx::Nand,
            15 => Gx::Set,
            _ => return None,
        })
    }

    /// The wire value.
    #[must_use]
    pub const fn to_wire(self) -> u8 {
        self as u8
    }

    /// Apply to 24-bit pixels (the result is always masked into the
    /// depth-24 value space; the plane-mask step in [`Target`] then
    /// merges it with the destination).
    #[must_use]
    pub const fn apply(self, src: u32, dst: u32) -> u32 {
        match self {
            Gx::Clear => 0,
            Gx::And => src & dst,
            Gx::AndReverse => src & !dst,
            Gx::Copy => src,
            Gx::AndInverted => !src & dst,
            Gx::Noop => dst,
            Gx::Xor => src ^ dst,
            Gx::Or => src | dst,
            Gx::Nor => !(src | dst) & 0x00ff_ffff,
            Gx::Equiv => (!src ^ dst) & 0x00ff_ffff,
            Gx::Invert => !dst & 0x00ff_ffff,
            Gx::OrReverse => src | !dst & 0x00ff_ffff,
            Gx::CopyInverted => !src & 0x00ff_ffff,
            Gx::OrInverted => !src & 0x00ff_ffff | dst,
            Gx::Nand => !(src & dst) & 0x00ff_ffff,
            Gx::Set => 0x00ff_ffff,
        }
    }
}

/// A drawing target: store + clip + raster op + plane mask + damage.
pub struct Target<'a> {
    store: &'a mut Store,
    /// Clip rectangles (already intersected; empty list draws nothing).
    pub clip: Vec<Rect>,
    gx: Gx,
    mask: u32,
    damage: Option<Rect>,
}

impl<'a> Target<'a> {
    /// Bind a target to a store with full-bounds clip.
    #[must_use]
    pub fn full(store: &'a mut Store, gx: Gx, mask: u32) -> Target<'a> {
        let bounds = Rect::new(0, 0, store.width, store.height);
        Target {
            store,
            clip: vec![bounds],
            gx,
            mask,
            damage: None,
        }
    }

    /// Restrict to an explicit clip list (GC clip rects + subwindow
    /// clipping computed by the caller).
    #[must_use]
    pub fn with_clip(mut self, clip: Vec<Rect>) -> Target<'a> {
        let bounds = Rect::new(0, 0, self.store.width, self.store.height);
        let mut kept = Vec::with_capacity(clip.len());
        for r in clip {
            if let Some(i) = bounds.intersect(r) {
                kept.push(i);
            }
        }
        self.clip = kept;
        self
    }

    /// Clip a requested op rectangle down to the store bounds so loops
    /// always run over drawable pixels only (a `width=65535` fill on an
    /// 8-pixel store iterates eight pixels, not four billion). Returns
    /// the half-open `(x0, y0, x1, y1)` span; empty when fully outside.
    #[must_use]
    fn span(&self, x: i32, y: i32, w: u32, h: u32) -> (i32, i32, i32, i32) {
        let sw = i64::from(self.store.width);
        let sh = i64::from(self.store.height);
        let x1 = (i64::from(x) + i64::from(w)).min(sw);
        let y1 = (i64::from(y) + i64::from(h)).min(sh);
        let x0 = i64::from(x).clamp(0, sw);
        let y0 = i64::from(y).clamp(0, sh);
        (x0 as i32, y0 as i32, x1.max(x0) as i32, y1.max(y0) as i32)
    }

    /// Clip a half-open pixel box `(x0, y0, x1, y1)` to the store
    /// bounds (shared by the shape rasterizer).
    #[must_use]
    pub(crate) fn clamp_box(&self, x0: i32, y0: i32, x1: i32, y1: i32) -> (i32, i32, i32, i32) {
        let w = self.store.width as i32;
        let h = self.store.height as i32;
        (
            x0.clamp(0, w),
            y0.clamp(0, h),
            x1.clamp(0, w),
            y1.clamp(0, h),
        )
    }

    /// Write one pixel through the GX function and plane mask.
    pub(crate) fn put(&mut self, x: i32, y: i32, src: u32) {
        if !self.visible(x, y) {
            return;
        }
        let (xu, yu) = (x as usize, y as usize);
        let at = (yu * self.store.width as usize + xu) * 4;
        let dst = u32::from(self.store.data[at])
            | (u32::from(self.store.data[at + 1]) << 8)
            | (u32::from(self.store.data[at + 2]) << 16);
        let raw = self.gx.apply(src, dst);
        let out = (raw & self.mask) | (dst & !self.mask) & 0x00ff_ffff;
        let b = px_bytes(out);
        self.store.data[at..at + 4].copy_from_slice(&b);
        let r = Rect::new(x, y, 1, 1);
        self.damage = Some(match self.damage {
            Some(prev) => prev.union(r),
            None => r,
        });
    }

    /// Whether a pixel passes bounds and clip.
    #[must_use]
    pub fn visible(&self, x: i32, y: i32) -> bool {
        let p = ldp_core::geometry::Point::new(x, y);
        self.clip.iter().any(|r| r.contains_point(p))
    }

    /// Take the damage accumulated by the ops so far.
    #[must_use]
    pub fn take_damage(&mut self) -> Option<Rect> {
        self.damage.take()
    }

    /// Plot individual points (PolyPoint).
    pub fn points(&mut self, pts: &[(i32, i32)], src: u32) {
        for &(x, y) in pts {
            self.put(x, y, src);
        }
    }

    /// One Bresenham segment into `acc` (no clipping applied here).
    fn bresenham(acc: &mut Vec<(i32, i32)>, x0: i32, y0: i32, x1: i32, y1: i32) {
        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;
        let (mut x, mut y) = (x0, y0);
        loop {
            acc.push((x, y));
            if x == x1 && y == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x += sx;
            }
            if e2 <= dx {
                err += dx;
                y += sy;
            }
        }
    }

    /// A whole path (polyline): joins continue; `not_last` drops the
    /// final pixel (the `NotLast` cap). Returns the walked pixels.
    #[must_use]
    fn path_pixels(pts: &[(i32, i32)]) -> Vec<(i32, i32)> {
        let mut acc = Vec::new();
        for w in pts.windows(2) {
            Self::bresenham(&mut acc, w[0].0, w[0].1, w[1].0, w[1].1);
        }
        acc
    }

    /// Thin polyline with dash state and optional `NotLast` cap.
    ///
    /// `dashes` is (offset, pattern); an empty pattern means solid.
    pub fn polyline(
        &mut self,
        pts: &[(i32, i32)],
        src: u32,
        not_last: bool,
        dashes: Option<(u32, &[u8])>,
        dash_bg: Option<u32>,
    ) {
        if pts.len() < 2 {
            if let Some(&(x, y)) = pts.first() {
                self.put(x, y, src);
            }
            return;
        }
        let mut acc = Self::path_pixels(pts);
        if not_last {
            acc.pop();
        }
        self.dash_walk(&acc, src, dashes, dash_bg);
    }

    /// One segment request: each segment is its own path.
    pub fn segments(&mut self, segs: &[(i32, i32, i32, i32)], src: u32, not_last: bool) {
        for &(x0, y0, x1, y1) in segs {
            let mut acc = Vec::new();
            Self::bresenham(&mut acc, x0, y0, x1, y1);
            if not_last && acc.len() > 1 {
                acc.pop();
            }
            for &(x, y) in &acc {
                self.put(x, y, src);
            }
        }
    }

    /// Walk path pixels applying a dash pattern (double-dash draws the
    /// gaps with `dash_bg`).
    fn dash_walk(
        &mut self,
        acc: &[(i32, i32)],
        src: u32,
        dashes: Option<(u32, &[u8])>,
        dash_bg: Option<u32>,
    ) {
        let Some((off, pattern)) = dashes else {
            for &(x, y) in acc {
                self.put(x, y, src);
            }
            return;
        };
        let total: usize = pattern.iter().map(|&d| d as usize).sum();
        if total == 0 {
            // All-zero pattern: solid (SetDashes rejects this, GC
            // defaults never produce it).
            for &(x, y) in acc {
                self.put(x, y, src);
            }
            return;
        }
        let pattern: Vec<u8> = pattern.to_vec();
        let len = pattern.len();
        let next_nonzero = |mut i: usize| -> usize {
            loop {
                i = (i + 1) % len;
                if pattern[i] > 0 {
                    return i;
                }
            }
        };
        // Advance the pattern state by the dash offset.
        let mut idx = 0usize;
        while pattern[idx] == 0 {
            idx = next_nonzero(idx);
        }
        let mut skip = (off as usize) % total;
        while skip >= pattern[idx] as usize {
            skip -= pattern[idx] as usize;
            idx = next_nonzero(idx);
        }
        let mut rem = pattern[idx] as usize - skip;
        for &(x, y) in acc {
            if idx % 2 == 0 {
                self.put(x, y, src);
            } else if let Some(bg) = dash_bg {
                self.put(x, y, bg);
            }
            rem -= 1;
            if rem == 0 {
                idx = next_nonzero(idx);
                rem = pattern[idx] as usize;
            }
        }
    }

    /// Rectangle outline: four edges, corners inclusive.
    pub fn rect_outline(&mut self, x: i32, y: i32, w: u32, h: u32, src: u32) {
        if w == 0 || h == 0 {
            return;
        }
        let (w, h) = (w as i32, h as i32);
        for px in x..x + w {
            self.put(px, y, src);
            self.put(px, y + h - 1, src);
        }
        for py in y..y + h {
            self.put(x, py, src);
            self.put(x + w - 1, py, src);
        }
    }

    /// Filled rectangle covering pixel centers.
    pub fn fill_rect(&mut self, x: i32, y: i32, w: u32, h: u32, src: u32) {
        if w == 0 || h == 0 {
            return;
        }
        let (x0, y0, x1, y1) = self.span(x, y, w, h);
        for py in y0..y1 {
            for px in x0..x1 {
                self.put(px, py, src);
            }
        }
    }

    /// Wide-line square-brush stamping for one path.
    pub fn wide_polyline(&mut self, pts: &[(i32, i32)], width: u32, src: u32) {
        if pts.len() < 2 {
            if let Some(&(x, y)) = pts.first() {
                self.stamp(x, y, width, src);
            }
            return;
        }
        let acc = Self::path_pixels(pts);
        for &(x, y) in &acc {
            self.stamp(x, y, width, src);
        }
    }

    /// Stamp a `width x width` square centered on (x, y).
    fn stamp(&mut self, x: i32, y: i32, width: u32, src: u32) {
        if width == 0 {
            self.put(x, y, src);
            return;
        }
        let half = width as i32 / 2;
        let (x0, y0, x1, y1) = self.span(x - half, y - half, width, width);
        for py in y0..y1 {
            for px in x0..x1 {
                self.put(px, py, src);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_layout_is_xrgb8888_bytes() {
        let s = Store::filled(2, 1, 0x00ff_8000);
        assert_eq!(s.data, vec![0x00, 0x80, 0xff, 0, 0x00, 0x80, 0xff, 0]);
        assert_eq!(s.pixel(1, 0), Some(0x00ff_8000));
        assert_eq!(s.pixel(2, 0), None);
        assert_eq!(s.pixel(-1, 0), None);
    }

    #[test]
    fn gx_functions_match_truth_table() {
        let s = 0b1100u32;
        let d = 0b1010u32;
        assert_eq!(Gx::Clear.apply(s, d), 0);
        assert_eq!(Gx::And.apply(s, d), 0b1000);
        assert_eq!(Gx::AndReverse.apply(s, d), 0b0100);
        assert_eq!(Gx::Copy.apply(s, d), 0b1100);
        assert_eq!(Gx::AndInverted.apply(s, d), 0b0010);
        assert_eq!(Gx::Noop.apply(s, d), 0b1010);
        assert_eq!(Gx::Xor.apply(s, d), 0b0110);
        assert_eq!(Gx::Or.apply(s, d), 0b1110);
        assert_eq!(Gx::Nor.apply(s, d), !0b1110u32 & 0x00ff_ffff);
        assert_eq!(Gx::Equiv.apply(s, d), (!s ^ d) & 0x00ff_ffff);
        assert_eq!(Gx::Invert.apply(s, d), !d & 0x00ff_ffff);
        assert_eq!(Gx::OrReverse.apply(s, d), (s | !d) & 0x00ff_ffff);
        assert_eq!(Gx::CopyInverted.apply(s, d), !s & 0x00ff_ffff);
        assert_eq!(Gx::OrInverted.apply(s, d), (!s | d) & 0x00ff_ffff);
        assert_eq!(Gx::Nand.apply(s, d), !(0b1000u32) & 0x00ff_ffff);
        assert_eq!(Gx::Set.apply(s, d), 0x00ff_ffff);
        assert_eq!(Gx::from_wire(16), None);
        // Every function's output stays in the 24-bit value space.
        for v in 0u8..16 {
            let g = Gx::from_wire(v).unwrap();
            assert_eq!(g.apply(s, d) & !0x00ff_ffff, 0);
        }
    }

    #[test]
    fn bresenham_diagonal_is_exact() {
        let mut acc = Vec::new();
        Target::bresenham(&mut acc, 0, 0, 3, 3);
        assert_eq!(acc, vec![(0, 0), (1, 1), (2, 2), (3, 3)]);
        let mut acc = Vec::new();
        Target::bresenham(&mut acc, 0, 0, 4, 2);
        assert_eq!(acc, vec![(0, 0), (1, 1), (2, 1), (3, 2), (4, 2)]);
    }

    #[test]
    fn polyline_not_last_drops_final_pixel() {
        let mut s = Store::filled(8, 8, 0);
        {
            let mut t = Target::full(&mut s, Gx::Copy, 0x00ff_ffff);
            t.polyline(&[(1, 1), (5, 1)], 1, true, None, None);
        }
        assert_eq!(s.pixel(1, 1), Some(1));
        assert_eq!(s.pixel(4, 1), Some(1));
        assert_eq!(s.pixel(5, 1), Some(0), "NotLast cap skips the endpoint");
        {
            let mut t = Target::full(&mut s, Gx::Copy, 0x00ff_ffff);
            t.polyline(&[(1, 3), (5, 3)], 1, false, None, None);
        }
        assert_eq!(s.pixel(5, 3), Some(1));
    }

    #[test]
    fn plane_mask_preserves_dst() {
        let mut s = Store::filled(4, 4, 0x00ff_0000);
        let mut t = Target::full(&mut s, Gx::Copy, 0x0000_ffff);
        t.fill_rect(0, 0, 4, 4, 0x00ff_00ff);
        // Red channel kept from dst; green+blue taken from src.
        assert_eq!(s.pixel(1, 1), Some(0x00ff_00ff));
    }

    #[test]
    fn xor_flips_only_drawn_region() {
        let mut s = Store::filled(4, 4, 0x00ff_00ff);
        let d = {
            let mut t = Target::full(&mut s, Gx::Xor, 0x00ff_ffff);
            t.fill_rect(1, 1, 2, 2, 0x0000_00ff);
            t.take_damage().unwrap()
        };
        assert_eq!(s.pixel(0, 0), Some(0x00ff_00ff));
        assert_eq!(s.pixel(1, 1), Some(0x00ff_0000));
        assert_eq!((d.x, d.y, d.w, d.h), (1, 1, 2, 2));
    }

    #[test]
    fn clip_restricts_drawing() {
        let mut s = Store::filled(8, 8, 0);
        let d = {
            let mut t =
                Target::full(&mut s, Gx::Copy, 0x00ff_ffff).with_clip(vec![Rect::new(2, 2, 4, 4)]);
            t.fill_rect(0, 0, 8, 8, 9);
            t.take_damage().unwrap()
        };
        assert_eq!(s.pixel(0, 0), Some(0));
        assert_eq!(s.pixel(1, 1), Some(0));
        assert_eq!(s.pixel(2, 2), Some(9));
        assert_eq!(s.pixel(5, 5), Some(9));
        assert_eq!(s.pixel(6, 6), Some(0));
        assert_eq!((d.x, d.y, d.w, d.h), (2, 2, 4, 4));
    }

    #[test]
    fn rect_outline_hits_corners() {
        let mut s = Store::filled(6, 6, 0);
        let mut t = Target::full(&mut s, Gx::Copy, 0x00ff_ffff);
        t.rect_outline(1, 1, 4, 4, 5);
        assert_eq!(s.pixel(1, 1), Some(5));
        assert_eq!(s.pixel(4, 1), Some(5));
        assert_eq!(s.pixel(1, 4), Some(5));
        assert_eq!(s.pixel(4, 4), Some(5));
        assert_eq!(s.pixel(2, 2), Some(0));
        assert_eq!(s.pixel(2, 1), Some(5));
    }

    #[test]
    fn huge_dimensions_are_clipped_not_looped() {
        // A 65535-wide fill on a 4-pixel store touches four pixels.
        let mut s = Store::filled(4, 1, 0);
        let mut t = Target::full(&mut s, Gx::Copy, 0x00ff_ffff);
        t.fill_rect(2, 0, u32::MAX, 1, 7);
        assert_eq!(s.pixel(3, 0), Some(7));
        assert_eq!(s.data.len(), 16);
    }

    #[test]
    fn dashes_alternate_and_offset_advances() {
        let mut s = Store::filled(16, 1, 0);
        {
            let mut t = Target::full(&mut s, Gx::Copy, 0x00ff_ffff);
            t.polyline(&[(0, 0), (15, 0)], 1, false, Some((0, &[2, 2])), None);
        }
        // Pattern 2 on, 2 off starting at 0.
        let on: Vec<i32> = (0..16).filter(|x| s.pixel(*x, 0) == Some(1)).collect();
        assert_eq!(on, vec![0, 1, 4, 5, 8, 9, 12, 13]);
        // Offset 3 starts one pixel into the off dash: pixel 0 off,
        // then the 2-on/2-off cycle from pixel 1.
        let mut s2 = Store::filled(16, 1, 0);
        {
            let mut t2 = Target::full(&mut s2, Gx::Copy, 0x00ff_ffff);
            t2.polyline(&[(0, 0), (15, 0)], 1, false, Some((3, &[2, 2])), None);
        }
        let on2: Vec<i32> = (0..16).filter(|x| s2.pixel(*x, 0) == Some(1)).collect();
        assert_eq!(on2, vec![1, 2, 5, 6, 9, 10, 13, 14]);
    }
}
