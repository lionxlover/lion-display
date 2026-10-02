//! 128-bit capability bitsets.
//!
//! The wire encodes a bitset as four little-endian u32 words
//! (`docs/protocol.md` §4). [`Bitset128`] is the decoded form shared by
//! schema tables, capability events, and the security scope model.

use core::fmt;

/// A 128-bit set of named flags.
///
/// Bits are addressed 0..=127. The zero set is "no capabilities"; unknown
/// bits set by a peer must be ignored (forward compatibility), which the
/// set operations here support naturally via masking.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(transparent)]
pub struct Bitset128 {
    bits: u128,
}

impl fmt::Debug for Bitset128 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Bitset128({:#034x})", self.bits)
    }
}

impl fmt::LowerHex for Bitset128 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:034x}", self.bits)
    }
}

impl Bitset128 {
    /// The empty set.
    pub const EMPTY: Bitset128 = Bitset128 { bits: 0 };

    /// Set containing exactly bit `index` (0..=127).
    #[must_use]
    pub const fn single(index: u32) -> Bitset128 {
        Bitset128 {
            bits: 1u128 << (index & 127),
        }
    }

    /// Whether bit `index` is set.
    #[must_use]
    pub const fn test(self, index: u32) -> bool {
        (self.bits & (1u128 << (index & 127))) != 0
    }

    /// With bit `index` set (immutably).
    #[must_use]
    pub const fn with(self, index: u32) -> Bitset128 {
        Bitset128 {
            bits: self.bits | (1u128 << (index & 127)),
        }
    }

    /// With bit `index` cleared.
    #[must_use]
    pub const fn without(self, index: u32) -> Bitset128 {
        Bitset128 {
            bits: self.bits & !(1u128 << (index & 127)),
        }
    }

    /// Set or clear a bit in place.
    pub fn set(&mut self, index: u32, value: bool) {
        if value {
            self.bits |= 1u128 << (index & 127);
        } else {
            self.bits &= !(1u128 << (index & 127));
        }
    }

    /// Number of set bits.
    #[must_use]
    pub const fn count(self) -> u32 {
        self.bits.count_ones()
    }

    /// Whether no bit is set.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.bits == 0
    }

    /// Whether every bit of `other` is also set here.
    #[must_use]
    pub const fn contains(self, other: Bitset128) -> bool {
        (self.bits & other.bits) == other.bits
    }

    /// Union.
    #[must_use]
    pub const fn union(self, other: Bitset128) -> Bitset128 {
        Bitset128 {
            bits: self.bits | other.bits,
        }
    }

    /// Intersection.
    #[must_use]
    pub const fn intersect(self, other: Bitset128) -> Bitset128 {
        Bitset128 {
            bits: self.bits & other.bits,
        }
    }

    /// Difference (`self` minus `other`).
    #[must_use]
    pub const fn minus(self, other: Bitset128) -> Bitset128 {
        Bitset128 {
            bits: self.bits & !other.bits,
        }
    }

    /// Encode as four little-endian u32 words (wire form).
    #[must_use]
    pub const fn to_words(self) -> [u32; 4] {
        [
            self.bits as u32,
            (self.bits >> 32) as u32,
            (self.bits >> 64) as u32,
            (self.bits >> 96) as u32,
        ]
    }

    /// Decode from four little-endian u32 words.
    #[must_use]
    pub const fn from_words(words: [u32; 4]) -> Bitset128 {
        Bitset128 {
            bits: (words[0] as u128)
                | ((words[1] as u128) << 32)
                | ((words[2] as u128) << 64)
                | ((words[3] as u128) << 96),
        }
    }

    /// Mask to the low `width` bits (drop bits a peer must ignore).
    #[must_use]
    pub const fn masked_to(self, width: u32) -> Bitset128 {
        if width >= 128 {
            self
        } else if width == 0 {
            Self::EMPTY
        } else {
            Bitset128 {
                bits: self.bits & ((1u128 << width) - 1),
            }
        }
    }

    /// Iterate the indices of set bits, ascending.
    pub fn iter_indices(self) -> impl Iterator<Item = u32> {
        (0..128u32).filter(move |i| self.test(*i))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_round_trip() {
        let words = [0xDEAD_BEEFu32, 0x1234_5678, 0x0, 0x8000_0001];
        let b = Bitset128::from_words(words);
        assert_eq!(b.to_words(), words);
        assert!(b.test(31));
        assert!(b.test(127));
        assert!(!b.test(32));
    }

    #[test]
    fn set_operations() {
        let a = Bitset128::single(0).with(1).with(5);
        let b = Bitset128::single(1).with(6);
        assert!(a.contains(Bitset128::single(1)));
        assert!(!a.contains(b));
        assert_eq!(a.union(b).count(), 4);
        assert_eq!(a.intersect(b), Bitset128::single(1));
        assert_eq!(a.minus(b), Bitset128::single(0).with(5));
        assert!(Bitset128::EMPTY.is_empty());
        assert_eq!(Bitset128::EMPTY.count(), 0);
    }

    #[test]
    fn bit_index_wraps_safely() {
        // Indices are masked into range rather than panicking.
        assert_eq!(Bitset128::single(128), Bitset128::single(0));
        assert!(Bitset128::EMPTY.with(129).test(1));
    }

    #[test]
    fn masking_drops_high_bits() {
        let b = Bitset128::single(3).with(70);
        assert_eq!(b.masked_to(64), Bitset128::single(3));
        assert_eq!(b.masked_to(4), Bitset128::single(3));
        assert_eq!(b.masked_to(0), Bitset128::EMPTY);
        assert_eq!(b.masked_to(128), b);
    }

    #[test]
    fn iteration_is_ascending() {
        let b = Bitset128::single(9).with(2).with(127);
        let idx: Vec<u32> = b.iter_indices().collect();
        assert_eq!(idx, vec![2, 9, 127]);
    }

    #[test]
    fn formatting() {
        let b = Bitset128::single(0);
        // 128-bit value: 32 hex digits, prefixed debug form is 34 wide.
        assert_eq!(
            format!("{b:?}"),
            "Bitset128(0x00000000000000000000000000000001)"
        );
        assert_eq!(format!("{b:x}"), "0000000000000000000000000000000001");
    }
}
