//! The configure serial clock.
//!
//! Every role object that sends `configure` proposals numbers them
//! with a serial drawn from its own clock. The two-phase commit needs
//! two properties that a plain counter does not survive wrap with:
//!
//! * the serial of the *live* proposal (the only one an `ack` may
//!   reference) is never confused with a stale one, and
//! * monotonicity is defined in wrapping `u32` order, so the clock
//!   runs forever without restarts.
//!
//! [`SerialClock`] is that clock: [`SerialClock::issue`] advances in
//! wrapping order and skips the one reserved value (a live serial),
//! which keeps the "stale ack" test exact even across the 2^32 wrap.

#![forbid(unsafe_code)]

/// A configure serial (wrapping `u32` domain).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct Serial(pub u32);

impl Serial {
    /// The zero serial (never issued; a clock starts at 1).
    pub const ZERO: Serial = Serial(0);

    /// Ordering in wrapping `u32` space: `self` is strictly after
    /// `origin` (distance in `1..=2^31-1`), i.e. the half-window that
    /// follows `origin`.
    #[must_use]
    pub const fn after(self, origin: Serial) -> bool {
        let d = self.0.wrapping_sub(origin.0);
        d > 0 && d < 0x8000_0000
    }

    /// The distance from `origin` to `self` in wrapping space when
    /// `self` is after `origin` (`None` otherwise).
    #[must_use]
    pub const fn distance(self, origin: Serial) -> Option<u32> {
        let d = self.0.wrapping_sub(origin.0);
        if d > 0 && d < 0x8000_0000 {
            Some(d)
        } else {
            None
        }
    }
}

/// Monotonic wrapping serial allocation for one role object.
#[derive(Clone, Copy, Debug)]
pub struct SerialClock {
    last_issued: u32,
    /// The one serial that must not be re-issued (the live proposal).
    reserved: u32,
}

impl Default for SerialClock {
    fn default() -> Self {
        Self::new()
    }
}

impl SerialClock {
    /// A fresh clock; the first serial issued is 1 (0 is never used so
    /// a default-constructed [`Serial`] can never collide).
    #[must_use]
    pub const fn new() -> SerialClock {
        SerialClock {
            last_issued: 0,
            reserved: 0,
        }
    }

    /// The last serial issued (0 before the first draw).
    #[must_use]
    pub const fn last(&self) -> Serial {
        Serial(self.last_issued)
    }

    /// Reserve a serial (the live proposal); the next issue skips it
    /// under wrap. Reserving 0 clears the reservation.
    pub fn reserve(&mut self, live: Serial) {
        self.reserved = live.0;
    }

    /// The reserved (live) serial.
    #[must_use]
    pub const fn reserved(&self) -> Serial {
        Serial(self.reserved)
    }

    /// Issue the next serial, skipping the reserved value so a future
    /// issue can never be confused with the live proposal. (Named
    /// `issue`, not `next`, so it cannot be mistaken for
    /// [`Iterator::next`].)
    ///
    /// # Panics
    ///
    /// Never: with one reservation the skip always terminates within
    /// two steps.
    #[must_use]
    pub fn issue(&mut self) -> Serial {
        loop {
            let candidate = self.last_issued.wrapping_add(1);
            self.last_issued = candidate;
            if candidate != self.reserved || self.reserved == 0 {
                return Serial(candidate);
            }
        }
    }

    /// Resume from a session watermark (crash recovery): the clock
    /// jumps forward — never backward — so post-restart serials can
    /// never collide with pre-crash acks still in flight. The
    /// reservation survives.
    pub fn resume_from(&mut self, watermark: Serial) {
        if watermark.after(Serial(self.last_issued)) || watermark.0 == self.last_issued {
            self.last_issued = watermark.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_serial_is_one_and_monotonic() {
        let mut c = SerialClock::new();
        assert_eq!(c.last(), Serial::ZERO);
        assert_eq!(c.issue(), Serial(1));
        assert_eq!(c.issue(), Serial(2));
        assert_eq!(c.last(), Serial(2));
    }

    #[test]
    fn wrapping_order_is_half_window() {
        assert!(Serial(5).after(Serial(3)));
        assert!(!Serial(3).after(Serial(5)));
        assert!(!Serial(3).after(Serial(3)));
        assert!(Serial(0).after(Serial(0xFFFF_FFFF)));
        assert!(Serial(1).after(Serial(0xFFFF_FFFF)));
        assert!(!Serial(0xFFFF_FFF0).after(Serial(5)));
        assert_eq!(Serial(5).distance(Serial(3)), Some(2));
        assert_eq!(Serial(3).distance(Serial(5)), None);
        assert_eq!(Serial(0).distance(Serial(0xFFFF_FFFF)), Some(1));
    }

    #[test]
    fn reservation_is_skipped_on_wrap() {
        // Drive the clock to the wrap with serial 1 reserved: the wrap
        // issues 0, then 1 is skipped in favor of 2.
        let mut c = SerialClock {
            last_issued: 0xFFFF_FFFF,
            reserved: 0,
        };
        c.reserve(Serial(1));
        assert_eq!(c.issue(), Serial(0));
        assert_eq!(c.issue(), Serial(2)); // 1 skipped
        assert_eq!(c.issue(), Serial(3));
        // Reserving 0 clears the reservation (0 is never issued by a
        // fresh clock, so it can never be live from one).
        let mut c2 = SerialClock {
            last_issued: 0xFFFF_FFFF,
            reserved: 5,
        };
        c2.reserve(Serial(0));
        assert_eq!(c2.issue(), Serial(0));
    }

    #[test]
    fn reservation_is_skipped_mid_stream() {
        let mut c = SerialClock::new();
        c.reserve(Serial(5));
        assert_eq!(c.issue(), Serial(1));
        assert_eq!(c.issue(), Serial(2));
        assert_eq!(c.issue(), Serial(3));
        assert_eq!(c.issue(), Serial(4));
        assert_eq!(c.issue(), Serial(6)); // 5 skipped
        assert_eq!(c.issue(), Serial(7));
    }

    #[test]
    fn reservation_follows_the_clock_when_updated() {
        let mut c = SerialClock::new();
        let a = c.issue();
        c.reserve(a);
        let b = c.issue();
        assert!(b.after(a));
        c.reserve(b);
        let d = c.issue();
        assert!(d.after(b));
        // The stale reservation is gone: issuing past it is fine.
        assert_ne!(d, a);
    }

    #[test]
    fn full_cycle_never_repeats_the_reserved_serial() {
        let mut c = SerialClock {
            last_issued: 0xFFFF_FF00,
            reserved: 0,
        };
        c.reserve(Serial(0x1234_5678));
        let mut seen = 0u128; // bitset over a window of 127 draws
        for _ in 0..127 {
            let s = c.issue();
            assert_ne!(s, Serial(0x1234_5678));
            if s.0 < 128 {
                let bit = 1u128 << s.0;
                assert_eq!(seen & bit, 0, "serial {} repeated", s.0);
                seen |= bit;
            }
        }
    }
}
