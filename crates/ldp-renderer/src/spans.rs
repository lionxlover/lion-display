//! Damage-clipped row spans.
//!
//! Compositing must write *exactly* the damaged pixels: a pixel outside the
//! damage rect holds older framebuffer content that is only correct because
//! undamaged layers were never re-submitted — writing a partial layer stack
//! there would corrupt it. Per output row, the destination rectangle is
//! intersected with every damage rect and the resulting intervals are sorted
//! and merged, so overlapping damage rectangles never double-blend (blending
//! is idempotent only for opaque writes, not for the `over` operator).
//!
//! Two shapes serve that contract:
//!
//! * [`RowIndex`] — the Phase 22 per-frame index: the damage's merged
//!   x-intervals *per row*, built once per frame and shared by every layer,
//!   so a layer's row work drops from "rescan every damage rect" to
//!   "clip the row's ready intervals to the destination" (O(intervals)).
//! * `RowSpans::collect` — the standalone collector (damage ∩ dest ∩ row,
//!   merged), retained under `#[cfg(test)]` as the *reference* semantics
//!   the index is cross-validated against.

use ldp_core::geometry::{Rect, Region};

/// The merged half-open x-intervals of one output row.
///
/// The buffer is reused across rows and layers (the renderer owns one
/// scratch instance); rows are short-lived by construction.
#[derive(Default)]
pub(crate) struct RowSpans {
    intervals: Vec<(i32, i32)>,
}

impl RowSpans {
    /// Drop all intervals (capacity is retained).
    pub(crate) fn clear(&mut self) {
        self.intervals.clear();
    }

    /// Whether the row has no coverage.
    pub(crate) fn is_empty(&self) -> bool {
        self.intervals.is_empty()
    }

    /// The merged intervals as `[(x0, x1))` (sorted, disjoint).
    pub(crate) fn intervals(&self) -> &[(i32, i32)] {
        &self.intervals
    }

    /// Total pixel coverage of the row.
    #[cfg(test)]
    pub(crate) fn coverage(&self) -> u32 {
        self.intervals
            .iter()
            .map(|&(a, b)| (b - a).max(0) as u32)
            .sum()
    }

    /// Collect `damage ∩ dest ∩ row y` as merged intervals.
    ///
    /// Damage rectangles overlapping the row are clipped to the destination
    /// rectangle; overlapping or adjacent intervals are merged so each
    /// destination pixel is composited at most once per layer.
    ///
    /// Production compositing goes through the row index + `clip`
    /// (Phase 22); this collector stays as the reference semantics the
    /// index's tests cross-validate against.
    #[cfg(test)]
    pub(crate) fn collect(damage: &Region, dest: &Rect, y: i32, out: &mut RowSpans) {
        out.clear();
        if dest.is_empty() || !damage.iter().any(|r| r.h > 0) {
            return;
        }
        let dx0 = dest.x;
        let dx1 = dest.right();
        for r in damage {
            if r.h == 0 || y < r.y || y >= r.bottom() {
                continue;
            }
            let x0 = r.x.max(dx0);
            let x1 = r.right().min(dx1);
            if x0 < x1 {
                out.intervals.push((x0, x1));
            }
        }
        if out.intervals.len() > 1 {
            out.intervals.sort_unstable();
            // In-place two-pointer merge of overlapping/touching intervals:
            // `write_idx` trails `read`, compacting as it goes.
            #[allow(clippy::needless_range_loop)] // two-pointer indices, not element iteration
            let mut write_idx = 0;
            for read in 1..out.intervals.len() {
                let (cur0, cur1) = out.intervals[write_idx];
                let (next0, next1) = out.intervals[read];
                if next0 <= cur1 {
                    // Overlapping or adjacent: extend the current interval.
                    out.intervals[write_idx] = (cur0, cur1.max(next1));
                } else {
                    write_idx += 1;
                    out.intervals[write_idx] = (next0, next1);
                }
            }
            out.intervals.truncate(write_idx + 1);
        }
    }

    /// Clip pre-merged row intervals (from [`RowIndex::row`]) to the
    /// destination rectangle's x range.
    ///
    /// The input intervals are already sorted and disjoint (the index merged
    /// them per row), and clipping by a common window preserves both
    /// properties, so no re-sort or re-merge is needed.
    pub(crate) fn clip(intervals: &[(i32, i32)], dest: &Rect, out: &mut RowSpans) {
        out.clear();
        if dest.is_empty() {
            return;
        }
        let dx0 = dest.x;
        let dx1 = dest.right();
        for &(a, b) in intervals {
            let x0 = a.max(dx0);
            let x1 = b.min(dx1);
            if x0 < x1 {
                out.intervals.push((x0, x1));
            }
        }
    }
}

/// The per-frame damage row index: for every row of the output bounds, the
/// merged damage x-intervals, built once per frame and served per row.
///
/// Before the index, every layer rescanned every damage rectangle for every
/// row it covered — O(rects × rows × layers) rect visits per frame, which
/// dominates real desktop frames (many windows, many small damage rects).
/// The index costs one O(rect coverage) build per *frame* and each
/// `(layer, row)` lookup becomes an offset slice plus a cheap clip.
///
/// All buffers are retained across frames (the Phase 21
/// `Region::subtract` precedent: no allocation in the steady state).
#[derive(Default)]
pub(crate) struct RowIndex {
    /// Per-row `(offset, count)` into `arena`, indexed by `y - base_y`.
    offsets: Vec<(u32, u32)>,
    /// Per-row interval counts while building (reused as scatter cursor).
    counts: Vec<u32>,
    /// The merged intervals of each covered row, back to back.
    arena: Vec<(i32, i32)>,
    /// The bounds' y origin (`row(y)` indexes by `y - base_y`).
    base_y: i32,
}

impl RowIndex {
    /// Build the index for `damage` clipped to `bounds`.
    ///
    /// Damage rectangles are clipped to the bounds first; each covered row
    /// receives the rectangle's x interval; per row the intervals are sorted
    /// and merged (the same merge rule as the test-only reference
    /// collector, applied at
    /// the row level before any destination clipping).
    pub(crate) fn build(&mut self, damage: &Region, bounds: &Rect) {
        self.offsets.clear();
        self.counts.clear();
        self.arena.clear();
        self.base_y = bounds.y;
        if bounds.w == 0 || bounds.h == 0 {
            return;
        }
        let bh = bounds.h as usize;
        self.counts.resize(bh, 0);
        // Pass 1: count each row's intervals (rects clipped to bounds).
        for r in damage {
            let y0 = r.y.max(bounds.y);
            let y1 = r.bottom().min(bounds.bottom());
            if y0 >= y1 {
                continue;
            }
            let x0 = r.x.max(bounds.x);
            let x1 = r.right().min(bounds.right());
            if x0 >= x1 {
                continue;
            }
            for y in y0..y1 {
                self.counts[(y - bounds.y) as usize] += 1;
            }
        }
        // Prefix sums turn the counts into arena windows.
        let mut total = 0u32;
        self.offsets.reserve(bh);
        for &c in &self.counts {
            self.offsets.push((total, c));
            total += c;
        }
        self.arena.resize(total as usize, (0, 0));
        // Pass 2: scatter each interval into its row's window, filling from
        // the back while `counts[y]` counts down to zero (order within the
        // window is arbitrary — pass 3 sorts).
        for r in damage {
            let y0 = r.y.max(bounds.y);
            let y1 = r.bottom().min(bounds.bottom());
            if y0 >= y1 {
                continue;
            }
            let x0 = r.x.max(bounds.x);
            let x1 = r.right().min(bounds.right());
            if x0 >= x1 {
                continue;
            }
            for y in y0..y1 {
                let idx = (y - bounds.y) as usize;
                let (off, _) = self.offsets[idx];
                let slot = off + self.counts[idx] - 1;
                self.arena[slot as usize] = (x0, x1);
                self.counts[idx] -= 1;
            }
        }
        // Pass 3: sort + merge each row's window in place; disjoint field
        // borrows keep this a single pass over the arena.
        let (arena, offsets) = (&mut self.arena, &mut self.offsets);
        for entry in offsets.iter_mut() {
            let (off, cnt) = *entry;
            if cnt <= 1 {
                continue;
            }
            let window = &mut arena[off as usize..(off + cnt) as usize];
            window.sort_unstable();
            let mut write = 0;
            #[allow(clippy::needless_range_loop)] // two-pointer indices, not element iteration
            for read in 1..window.len() {
                let (cur0, cur1) = window[write];
                let (next0, next1) = window[read];
                if next0 <= cur1 {
                    window[write] = (cur0, cur1.max(next1));
                } else {
                    write += 1;
                    window[write] = (next0, next1);
                }
            }
            entry.1 = write as u32 + 1;
        }
    }

    /// The merged damage intervals of row `y` (clipped to the build bounds).
    ///
    /// Rows outside the bounds or without damage return an empty slice;
    /// the lookup is O(1).
    pub(crate) fn row(&self, y: i32) -> &[(i32, i32)] {
        let idx = y - self.base_y;
        if idx < 0 || idx as usize >= self.offsets.len() {
            return &[];
        }
        let (off, cnt) = self.offsets[idx as usize];
        &self.arena[off as usize..(off + cnt) as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(rects: &[(i32, i32, u32, u32)]) -> Region {
        let mut r = Region::new();
        for &(x, y, w, h) in rects {
            r.add(Rect::new(x, y, w, h));
        }
        r
    }

    #[test]
    fn empty_inputs_stay_empty() {
        let mut spans = RowSpans::default();
        let dest = Rect::new(10, 10, 20, 20);
        RowSpans::collect(&Region::new(), &dest, 15, &mut spans);
        assert!(spans.is_empty());
        RowSpans::collect(&region(&[(0, 0, 5, 5)]), &dest, 2, &mut spans);
        assert!(spans.is_empty());
        assert_eq!(spans.coverage(), 0);
    }

    #[test]
    fn clipping_and_coverage() {
        let mut spans = RowSpans::default();
        let dest = Rect::new(10, 10, 20, 20);
        // Damage partially outside dest on the left.
        RowSpans::collect(&region(&[(0, 12, 15, 1)]), &dest, 12, &mut spans);
        assert_eq!(spans.intervals(), &[(10, 15)]);
        assert_eq!(spans.coverage(), 5);
        // Row outside the damage rect.
        RowSpans::collect(&region(&[(0, 12, 15, 1)]), &dest, 13, &mut spans);
        assert!(spans.is_empty());
    }

    #[test]
    fn overlapping_damage_merges_to_one_pass() {
        let mut spans = RowSpans::default();
        let dest = Rect::new(0, 0, 100, 10);
        RowSpans::collect(
            &region(&[(10, 1, 30, 1), (30, 1, 30, 1), (0, 1, 15, 1)]),
            &dest,
            1,
            &mut spans,
        );
        // Touching at 30..30 and overlapping 10..15 merge into [0, 60).
        assert_eq!(spans.intervals(), &[(0, 60)]);
        assert_eq!(spans.coverage(), 60);
    }

    #[test]
    fn disjoint_intervals_stay_split() {
        let mut spans = RowSpans::default();
        let dest = Rect::new(0, 0, 100, 10);
        RowSpans::collect(
            &region(&[(5, 0, 5, 1), (20, 0, 5, 1), (50, 0, 5, 1)]),
            &dest,
            0,
            &mut spans,
        );
        assert_eq!(spans.intervals(), &[(5, 10), (20, 25), (50, 55)]);
        assert_eq!(spans.coverage(), 15);
    }

    #[test]
    fn coverage_never_double_counts() {
        let mut spans = RowSpans::default();
        let dest = Rect::new(0, 0, 50, 1);
        // Same rect twice: merged once.
        RowSpans::collect(
            &region(&[(0, 0, 50, 1), (0, 0, 50, 1)]),
            &dest,
            0,
            &mut spans,
        );
        assert_eq!(spans.coverage(), 50);
    }

    // ---- RowIndex (Phase 22) -------------------------------------------

    #[test]
    fn index_serves_merged_intervals_per_row() {
        let mut index = RowIndex::default();
        let bounds = Rect::new(0, 0, 100, 10);
        index.build(
            &region(&[(10, 1, 30, 1), (30, 1, 30, 1), (0, 1, 15, 1)]),
            &bounds,
        );
        // Touching at 30 and overlapping 10..15 merge into [0, 60).
        assert_eq!(index.row(1), &[(0, 60)]);
        // Rows without damage are empty.
        assert!(index.row(0).is_empty());
        assert!(index.row(2).is_empty());
        // Rows outside the bounds are empty (not panics).
        assert!(index.row(-1).is_empty());
        assert!(index.row(10).is_empty());
    }

    #[test]
    fn index_clips_damage_to_bounds() {
        let mut index = RowIndex::default();
        let bounds = Rect::new(10, 10, 20, 20);
        // Damage extends past every edge of the bounds.
        index.build(&region(&[(0, 0, 100, 100)]), &bounds);
        assert_eq!(index.row(10), &[(10, 30)]);
        assert_eq!(index.row(29), &[(10, 30)]);
        assert!(index.row(9).is_empty());
        assert!(index.row(30).is_empty());
    }

    #[test]
    fn index_retains_buffers_across_rebuilds() {
        let mut index = RowIndex::default();
        let bounds = Rect::new(0, 0, 8, 8);
        index.build(&region(&[(0, 0, 8, 8)]), &bounds);
        let arena_cap = index.arena.capacity();
        let offsets_cap = index.offsets.capacity();
        // Rebuild with different content: capacity retained, content replaced.
        index.build(&region(&[(2, 2, 2, 2)]), &bounds);
        assert_eq!(index.arena.capacity(), arena_cap);
        assert_eq!(index.offsets.capacity(), offsets_cap);
        // The 2x2 rect covers rows 2 and 3.
        assert_eq!(index.row(2), &[(2, 4)]);
        assert_eq!(index.row(3), &[(2, 4)]);
        assert!(index.row(1).is_empty());
        assert!(index.row(4).is_empty());
        // Degenerate bounds: the index serves nothing.
        index.build(&region(&[(0, 0, 8, 8)]), &Rect::new(0, 0, 0, 0));
        assert!(index.row(0).is_empty());
    }

    /// The index must agree with the reference collector on every row the
    /// production caller queries — the destination's rows — for any damage
    /// clipped to the bounds: same intervals after clipping to the same
    /// dest. (The reference collector itself does not clip y to the dest;
    /// `composite_layer` only ever asks for dest rows, so that is the
    /// equivalence that matters.)
    #[test]
    #[allow(clippy::many_single_char_names)] // x/y/n are the corpus domain's names
    fn index_matches_the_reference_collector_on_a_corpus() {
        let mut rng = crate::test_rng();
        let bounds = Rect::new(0, 0, 64, 48);
        let dest = Rect::new(5, 7, 40, 30);
        let mut index = RowIndex::default();
        let mut spans = RowSpans::default();
        for case in 0..64 {
            // Deterministic scattered rect corpus.
            let mut damage = Region::new();
            let n = 1 + (rng.next_u64() % 8) as usize;
            for _ in 0..n {
                let x = (rng.next_u64() % 64) as i32;
                let y = (rng.next_u64() % 48) as i32;
                let w = 1 + (rng.next_u64() % 16) as u32;
                let h = 1 + (rng.next_u64() % 8) as u32;
                damage.add(Rect::new(x, y, w, h));
            }
            index.build(&damage, &bounds);
            for y in dest.y..dest.bottom() {
                // Reference: collect(damage, dest, y) — clips to dest inline.
                RowSpans::collect(&damage, &dest, y, &mut spans);
                let reference = spans.intervals().to_vec();
                // Index path: row intervals, then clip to the same dest.
                let row = index.row(y);
                RowSpans::clip(row, &dest, &mut spans);
                assert_eq!(
                    spans.intervals(),
                    reference.as_slice(),
                    "case {case}, row {y}: index path diverged from the reference"
                );
            }
        }
    }

    /// Clipping pre-merged intervals keeps them sorted and disjoint —
    /// never overlapping or doubled.
    #[test]
    fn clip_never_produces_overlap() {
        let mut index = RowIndex::default();
        let bounds = Rect::new(0, 0, 100, 4);
        index.build(
            &region(&[(0, 1, 40, 1), (30, 1, 40, 1), (80, 1, 5, 1)]),
            &bounds,
        );
        let mut spans = RowSpans::default();
        let dest = Rect::new(20, 0, 60, 4);
        RowSpans::clip(index.row(1), &dest, &mut spans);
        // 80..85 lies past the dest's right edge (20..80): clipped away.
        assert_eq!(spans.intervals(), &[(20, 70)]);
        let ivs = spans.intervals();
        for w in ivs.windows(2) {
            assert!(w[0].1 <= w[1].0, "intervals must stay disjoint: {ivs:?}");
        }
    }
}
