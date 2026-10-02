//! The shared deterministic PRNG.
//!
//! [`SplitMix64`] is the one random source every LDP harness uses: the
//! fuzzers, the conformance boundary sweeps, the stress schedule, and
//! the benchmark input generation. It is 5 lines of arithmetic with a
//! 2^128-period-equivalent mixing function, has no platform-dependent
//! behavior, and fails no statistical smoke test that matters for
//! mutation-driven harnessing. The point is not cryptographic strength
//! — it is **seed-addressed reproducibility**: a failure report that
//! names a seed replays exactly on any machine.

/// The SplitMix64 generator (Steele & Marsaglia).
///
/// Deterministic across platforms: pure `u64` wrapping arithmetic, no
/// state beyond the 64-bit seed, no allocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    /// A generator seeded with `seed`.
    ///
    /// The all-zero seed is fine: the mixer escapes it on the first
    /// draw (unlike e.g. a plain LCG).
    #[must_use]
    pub const fn new(seed: u64) -> SplitMix64 {
        SplitMix64 { state: seed }
    }

    /// The current state (the seed that would reproduce the *next*
    /// draw exactly).
    #[must_use]
    pub const fn state(&self) -> u64 {
        self.state
    }

    /// Next raw 64-bit value.
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Next 32-bit value (drawn from the high half — the well-mixed
    /// end).
    pub fn next_u32(&mut self) -> u32 {
        (self.next_u64() >> 32) as u32
    }

    /// Next byte.
    pub fn byte(&mut self) -> u8 {
        (self.next_u64() >> 56) as u8
    }

    /// Next boolean.
    pub fn next_bool(&mut self) -> bool {
        (self.next_u64() & 1) == 1
    }

    /// Uniform-ish value below `n`.
    ///
    /// # Panics
    ///
    /// Panics when `n` is zero — there is no answer, and callers are
    /// harness code that should crash loudly rather than silently skip.
    pub fn below(&mut self, n: usize) -> usize {
        assert!(n > 0, "below(0)");
        // Modulo bias exists but is bounded by 2^-32 for every
        // realistic table size — irrelevant for mutation harnesses.
        (self.next_u64() % n as u64) as usize
    }

    /// A uniform `f64` in `[0, 1)` (53-bit mantissa).
    #[must_use]
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// `n` fresh bytes.
    pub fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.byte()).collect()
    }

    /// A reference to one uniformly picked element of `items`.
    ///
    /// # Panics
    ///
    /// Panics when `items` is empty — same loud-failure doctrine as
    /// [`Self::below`].
    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_stream() {
        let mut a = SplitMix64::new(0xDEAD_BEEF);
        let mut b = SplitMix64::new(0xDEAD_BEEF);
        for _ in 0..1000 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn different_seeds_diverge() {
        let mut a = SplitMix64::new(1);
        let mut b = SplitMix64::new(2);
        let agree = (0..100).filter(|_| a.next_u64() == b.next_u64()).count();
        assert_eq!(agree, 0);
    }

    #[test]
    fn zero_seed_is_not_degenerate() {
        let mut r = SplitMix64::new(0);
        let first = r.next_u64();
        assert_ne!(first, 0);
        let second = r.next_u64();
        assert_ne!(first, second);
    }

    #[test]
    fn byte_histogram_is_flat_enough() {
        // 256k draws over 256 buckets: expectation 1024, tolerance
        // ±3 sigma ~ sqrt(1024)*3 ≈ 96 (and then some margin).
        let mut r = SplitMix64::new(0x1234);
        let mut hist = [0usize; 256];
        for _ in 0..256 * 1024 {
            hist[r.byte() as usize] += 1;
        }
        for (b, count) in hist.iter().enumerate() {
            assert!(
                (800..=1250).contains(count),
                "byte {b}: {count} (expected ~1024)"
            );
        }
    }

    #[test]
    fn below_stays_in_range_and_covers() {
        let mut r = SplitMix64::new(7);
        let mut seen = [false; 5];
        for _ in 0..1000 {
            let v = r.below(5);
            assert!(v < 5);
            seen[v] = true;
        }
        assert!(seen.iter().all(|s| *s), "all residues must appear");
    }

    #[test]
    fn f64_in_unit_interval() {
        let mut r = SplitMix64::new(99);
        for _ in 0..10_000 {
            let v = r.next_f64();
            assert!((0.0..1.0).contains(&v));
        }
    }

    #[test]
    fn state_snapshot_reproduces() {
        let mut a = SplitMix64::new(42);
        a.next_u64();
        a.next_u64();
        let snap = a.state();
        let x = a.next_u64();
        let mut b = SplitMix64::new(snap);
        assert_eq!(b.next_u64(), x);
    }

    #[test]
    fn pick_and_bytes_shapes() {
        let mut r = SplitMix64::new(5);
        let items = [1u32, 2, 3];
        for _ in 0..100 {
            assert!(items.contains(r.pick(&items)));
        }
        assert_eq!(r.bytes(0).len(), 0);
        assert_eq!(r.bytes(17).len(), 17);
    }
}
