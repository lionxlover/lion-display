//! Adaptive PNG row filtering.
//!
//! Each scanline is trialed through the five RFC-filtered forms
//! (None/Sub/Up/Average/Paeth); the filter whose output has the smallest
//! sum of absolute values — interpreted as *signed* bytes, the standard
//! heuristic — wins, and its byte plus filtered row are appended to the
//! scanline stream. The heuristic is pure integer arithmetic, so the
//! chosen filter per row (and therefore the whole stream) is
//! platform-stable.

/// The five filter byte values.
const F_NONE: u8 = 0;
const F_SUB: u8 = 1;
const F_UP: u8 = 2;
const F_AVG: u8 = 3;
const F_PAETH: u8 = 4;
/// Every filter, in byte order (the trial order — first hit wins ties,
/// so the choice is deterministic).
const ALL_FILTERS: [u8; 5] = [F_NONE, F_SUB, F_UP, F_AVG, F_PAETH];

/// The Paeth predictor (RFC 2083 §`filter-type-4`): the nearest of the
/// three neighbors, with the documented tie order (a, b, c).
fn paeth_predictor(a: u8, b: u8, c: u8) -> u8 {
    // The 16-bit intermediate is exactly representable; the sum of two
    // bytes cannot overflow i32.
    let p = i32::from(a) + i32::from(b) - i32::from(c);
    let pa = (p - i32::from(a)).abs();
    let pb = (p - i32::from(b)).abs();
    let pc = (p - i32::from(c)).abs();
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

/// Retained scratch of the filter stage: the trial buffer (filled per
/// candidate filter), the best-so-far buffer, and the previous row. All
/// capacity is reused across rows and images — the steady state
/// allocates nothing.
#[derive(Default)]
pub(crate) struct FilterScratch {
    trial: Vec<u8>,
    best: Vec<u8>,
    prev: Vec<u8>,
}

impl FilterScratch {
    /// Filter `row` into `self.trial` with `filter` and return the
    /// heuristic score (sum of absolute byte-as-signed values). The
    /// previous row is read from `self.prev`.
    fn filter_row(&mut self, filter: u8, row: &[u8], bpp: usize) -> u64 {
        let n = row.len();
        self.trial.clear();
        self.trial.reserve(n);
        let mut score: u64 = 0;
        for (x, &raw) in row.iter().enumerate() {
            let left = if x >= bpp { row[x - bpp] } else { 0 };
            let up = self.prev[x];
            let upleft = if x >= bpp { self.prev[x - bpp] } else { 0 };
            let v = match filter {
                F_SUB => raw.wrapping_sub(left),
                F_UP => raw.wrapping_sub(up),
                F_AVG => raw.wrapping_sub(
                    // floor((left + up) / 2) — the RFC's exact form.
                    ((u16::from(left) + u16::from(up)) >> 1) as u8,
                ),
                F_PAETH => raw.wrapping_sub(paeth_predictor(left, up, upleft)),
                _ => raw, // F_NONE
            };
            self.trial.push(v);
            // Absolute value as a signed byte (the heuristic's definition).
            score += u64::from((v as i8).unsigned_abs());
        }
        score
    }
}

/// Filter a whole image into `out` (the scanline stream the deflate stage
/// consumes: one filter byte + the filtered row, per row).
pub(crate) fn filter_image(
    scratch: &mut FilterScratch,
    out: &mut Vec<u8>,
    width: usize,
    height: usize,
    bpp: usize,
    data: &[u8],
) {
    out.clear();
    let stride = width * bpp;
    scratch.trial.clear();
    scratch.best.clear();
    scratch.prev.clear();
    scratch.prev.resize(stride, 0);
    scratch.best.reserve(stride);
    for y in 0..height {
        let row = &data[y * stride..(y + 1) * stride];
        let mut best_filter = F_NONE;
        let mut best_score = u64::MAX;
        for &f in &ALL_FILTERS {
            let score = scratch.filter_row(f, row, bpp);
            // Strict `<` keeps the earliest filter on ties (None before
            // Sub before Up ...): deterministic and stable.
            if score < best_score {
                best_score = score;
                best_filter = f;
                core::mem::swap(&mut scratch.trial, &mut scratch.best);
            }
        }
        out.push(best_filter);
        out.extend_from_slice(&scratch.best);
        // The previous row advances to the current raw row.
        scratch.prev.copy_from_slice(row);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paeth_matches_the_reference_tie_order() {
        assert_eq!(paeth_predictor(0, 0, 0), 0);
        assert_eq!(paeth_predictor(255, 0, 0), 255); // pa small
        assert_eq!(paeth_predictor(0, 255, 0), 255); // pb small
        assert_eq!(paeth_predictor(0, 0, 255), 0); // pc small but a ties
        assert_eq!(paeth_predictor(10, 10, 10), 10);
        // p = a + b - c; when all distances tie, a wins.
        assert_eq!(paeth_predictor(100, 100, 100), 100);
    }

    #[test]
    fn all_filters_wrap_not_panic() {
        let mut scratch = FilterScratch {
            prev: vec![10, 240, 30, 200, 128],
            ..Default::default()
        };
        let row = [200u8, 100, 50, 250, 5];
        for &f in &ALL_FILTERS {
            let score = scratch.filter_row(f, &row, 1);
            assert_eq!(scratch.trial.len(), row.len());
            assert!(score <= u64::from(u8::MAX) * row.len() as u64);
            // None is the identity; the heuristic reads bytes as signed,
            // so 200 contributes |−56| = 56 and 250 contributes 6.
            if f == F_NONE {
                assert_eq!(scratch.trial, row);
                assert_eq!(score, 56 + 100 + 50 + 6 + 5);
            }
        }
    }

    #[test]
    fn sub_up_and_avg_are_hand_computed() {
        let mut scratch = FilterScratch {
            prev: vec![5, 5, 5, 5],
            ..Default::default()
        };
        let row = [10u8, 20, 30, 40];
        // Sub with bpp=2: x0/x1 have left=0; x2/x3 subtract row[x-2].
        scratch.filter_row(F_SUB, &row, 2);
        assert_eq!(scratch.trial, [10, 20, 20, 20]);
        // Up subtracts the previous row.
        scratch.filter_row(F_UP, &row, 2);
        assert_eq!(scratch.trial, [5, 15, 25, 35]);
        // Average: floor((left + up) / 2). x0/x1 have left=0 (x < bpp);
        // x2's left is row[0]=10, x3's left is row[1]=20.
        scratch.filter_row(F_AVG, &row, 2);
        assert_eq!(scratch.trial, [8, 18, 23, 28]);
    }

    #[test]
    fn stream_shape_is_filter_byte_per_row() {
        let mut scratch = FilterScratch::default();
        let mut out = Vec::new();
        // 4x2 RGB.
        let data: Vec<u8> = (0..24).collect();
        filter_image(&mut scratch, &mut out, 4, 2, 3, &data);
        assert_eq!(out.len(), 2 * (1 + 12));
        // Filter bytes are in range.
        assert!(out[0] <= 4);
        assert!(out[13] <= 4);
    }

    #[test]
    fn identical_rows_pick_up() {
        // Rows that repeat exactly: after row 0, "Up" filters them to all
        // zeros — score 0, the minimum possible — which must beat every
        // other filter. The heuristic is thereby pinned on a case where
        // the answer is known exactly.
        let mut scratch = FilterScratch::default();
        let mut out = Vec::new();
        let mut data = Vec::new();
        for _y in 0..4 {
            for x in 0..8 {
                let v = if x % 2 == 0 { 200u8 } else { 30 };
                data.extend_from_slice(&[v, v.wrapping_add(1), v.wrapping_add(2)]);
            }
        }
        filter_image(&mut scratch, &mut out, 8, 4, 3, &data);
        let picks: Vec<u8> = (0..4).map(|y| out[y * (1 + 24)]).collect();
        assert_eq!(
            picks,
            vec![F_NONE, F_UP, F_UP, F_UP],
            "row 0 has no prior; identical rows after must pick Up"
        );
    }
}
