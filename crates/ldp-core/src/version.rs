//! Protocol version negotiation.
//!
//! Interfaces are advertised with a *range* `[min, max]`
//! (`docs/protocol.md` §5); a bind pins one version inside the
//! intersection of the client's and server's ranges.

use core::fmt;

/// A single wire protocol version of an interface or module. Versions are
/// positive integers; 1 is the baseline of every module.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[repr(transparent)]
pub struct Version {
    v: u32,
}

impl Version {
    /// The baseline version.
    pub const V1: Version = Version { v: 1 };

    /// Construct from a raw positive integer; zero or negative-equivalents
    /// are rejected (the wire type is unsigned).
    pub const fn new(v: u32) -> Option<Version> {
        if v == 0 {
            None
        } else {
            Some(Version { v })
        }
    }

    /// Raw numeric value.
    pub const fn as_u32(self) -> u32 {
        self.v
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "v{}", self.v)
    }
}

/// An inclusive range of supported versions, `[min, max]`.
///
/// ```
/// use ldp_core::{Version, VersionRange};
/// let server = VersionRange::new(1, 3).unwrap();
/// let client = VersionRange::new(2, 5).unwrap();
/// let negotiated = server.intersect(client).unwrap();
/// assert_eq!(negotiated.max(), Version::new(3).unwrap());
/// let chosen = server.choose(client, None).unwrap();
/// assert_eq!(chosen, Version::new(3).unwrap());
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct VersionRange {
    min: Version,
    max: Version,
}

impl VersionRange {
    /// Construct `[min, max]`; requires `1 <= min <= max`.
    pub const fn new(min: u32, max: u32) -> Option<VersionRange> {
        if min == 0 || min > max {
            None
        } else {
            Some(VersionRange {
                min: Version { v: min },
                max: Version { v: max },
            })
        }
    }

    /// A single-version range.
    pub const fn exact(v: u32) -> Option<VersionRange> {
        VersionRange::new(v, v)
    }

    /// Lowest supported version.
    pub const fn min(self) -> Version {
        self.min
    }

    /// Highest supported version.
    pub const fn max(self) -> Version {
        self.max
    }

    /// Whether `v` lies inside the range.
    pub const fn contains(self, v: Version) -> bool {
        v.v >= self.min.v && v.v <= self.max.v
    }

    /// Intersection of two ranges: the versions both sides support.
    /// `None` means the peers are incompatible for this interface.
    pub const fn intersect(self, other: VersionRange) -> Option<VersionRange> {
        let lo = if self.min.v >= other.min.v {
            self.min
        } else {
            other.min
        };
        let hi = if self.max.v <= other.max.v {
            self.max
        } else {
            other.max
        };
        if lo.v <= hi.v {
            Some(VersionRange { min: lo, max: hi })
        } else {
            None
        }
    }

    /// Pick the version to bind: the highest common version, optionally
    /// capped by `ceiling` (a client that wants to avoid a too-new
    /// version). `None` if no common version exists.
    pub const fn choose(self, other: VersionRange, ceiling: Option<Version>) -> Option<Version> {
        let Some(mut intersection) = self.intersect(other) else {
            return None;
        };
        if let Some(c) = ceiling {
            if c.v < intersection.min.v {
                return None;
            }
            if c.v < intersection.max.v {
                intersection.max = c;
            }
        }
        Some(intersection.max)
    }
}

impl fmt::Display for VersionRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}, {}]", self.min.v, self.max.v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_degenerate_ranges() {
        assert!(VersionRange::new(0, 1).is_none());
        assert!(VersionRange::new(2, 1).is_none());
        assert!(VersionRange::new(1, 1).is_some());
        assert!(Version::new(0).is_none());
    }

    #[test]
    fn intersection_math() {
        let a = VersionRange::new(1, 3).unwrap();
        let b = VersionRange::new(2, 5).unwrap();
        let i = a.intersect(b).unwrap();
        assert_eq!(i.min().as_u32(), 2);
        assert_eq!(i.max().as_u32(), 3);

        // Disjoint.
        let c = VersionRange::new(4, 9).unwrap();
        assert!(a.intersect(c).is_none());

        // Identical.
        assert_eq!(a.intersect(a), Some(a));
    }

    #[test]
    fn choose_picks_highest_common_with_ceiling() {
        let server = VersionRange::new(1, 6).unwrap();
        let client = VersionRange::new(2, 4).unwrap();
        assert_eq!(server.choose(client, None).unwrap().as_u32(), 4);

        // Ceiling below the common minimum fails.
        assert!(server
            .choose(client, Some(Version::new(1).unwrap()))
            .is_none());
        // Ceiling inside the range caps the pick.
        assert_eq!(
            server
                .choose(client, Some(Version::new(3).unwrap()))
                .unwrap()
                .as_u32(),
            3
        );
    }

    #[test]
    fn contains_and_display() {
        let r = VersionRange::new(1, 3).unwrap();
        assert!(r.contains(Version::V1));
        assert!(!r.contains(Version::new(4).unwrap()));
        assert_eq!(r.to_string(), "[1, 3]");
        assert_eq!(Version::V1.to_string(), "v1");
    }
}
