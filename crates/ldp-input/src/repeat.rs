//! Server-side key repeat: the schedule model.
//!
//! Repeat is modeled in the server, never taken from the kernel (the
//! normalizer drops `EV_KEY` value-2 events). The model is pure
//! arithmetic over timestamps: given a press time, the delay before
//! the first repeat, and the rate in keys per second,
//! [`RepeatModel::ticks`] enumerates the instants a repeat `key`
//! event should fire within a query window.
//!
//! The compositor drives it from its wake points (the same doctrine
//! as the frame scheduler): at each wake, it asks for the ticks since
//! the last wake and emits at most those — late wakes coalesce, they
//! never queue an unbounded burst. [`RepeatModel::ticks_bounded`] is
//! that shape.

#![forbid(unsafe_code)]

use ldp_core::time::Mono;

/// The repeat parameters (the wire's `repeat_info` event).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RepeatModel {
    /// Repeats per second (0 = repeat disabled).
    pub rate_hz: i32,
    /// Milliseconds of hold before the first repeat.
    pub delay_ms: i32,
}

impl Default for RepeatModel {
    fn default() -> Self {
        // The classic desktop default: 33 cps after a 500 ms delay.
        RepeatModel {
            rate_hz: 33,
            delay_ms: 500,
        }
    }
}

impl RepeatModel {
    /// Whether repeat is active at all.
    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.rate_hz > 0 && self.delay_ms >= 0
    }

    /// The interval between repeats, nanoseconds (0 when disabled).
    #[must_use]
    pub fn interval_ns(&self) -> u64 {
        if !self.enabled() {
            return 0;
        }
        (1e9 / f64::from(self.rate_hz.max(1))) as u64
    }

    /// Repeat instants for a key pressed at `press` within
    /// `(press, until]`.
    ///
    /// The first tick lands at `press + delay`; subsequent ticks are
    /// spaced by the rate interval, aligned to the press time —
    /// deterministic and independent of query windows. Repeated calls
    /// with growing windows return prefixes of the same sequence
    /// (that is the property the wake-point driver relies on).
    #[must_use]
    pub fn ticks(&self, press: Mono, until: Mono) -> Vec<Mono> {
        if !self.enabled() {
            return Vec::new();
        }
        let mut out = Vec::new();
        let interval = self.interval_ns().max(1);
        let first_ns = press
            .as_ns()
            .saturating_add(self.delay_ms as u64 * 1_000_000);
        if until.as_ns() <= first_ns {
            return out;
        }
        // Tick k (k ≥ 0) lands at first + k * interval.
        let mut k = 0u64;
        loop {
            let t_ns = first_ns.saturating_add(k.saturating_mul(interval));
            if t_ns >= until.as_ns() {
                break;
            }
            out.push(Mono::from_ns(t_ns));
            k += 1;
            if k > 100_000 {
                // A degenerate rate would spin forever; the cap is a
                // guard, not a policy (0.001 cps × 100k ≈ 3 years).
                break;
            }
        }
        out
    }

    /// The bounded form: at most `cap` ticks, for wake-point driving.
    ///
    /// When the window contains more ticks than the cap, the *latest*
    /// `cap` are returned — late wakes coalesce rather than burst.
    #[must_use]
    pub fn ticks_bounded(&self, press: Mono, until: Mono, cap: usize) -> Vec<Mono> {
        let all = self.ticks(press, until);
        if all.len() > cap {
            all[all.len() - cap..].to_vec()
        } else {
            all
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: u64 = 1_000_000_000;

    #[test]
    fn schedule_shape() {
        let m = RepeatModel {
            rate_hz: 10,
            delay_ms: 500,
        };
        let press = Mono::from_ns(0);
        // Window ends before the delay: nothing.
        assert!(m.ticks(press, Mono::from_ns(S / 2)).is_empty());
        // Window exactly at the first tick: still nothing (the window
        // is half-open — the tick at exactly `until` belongs to the
        // next window).
        assert!(m.ticks(press, Mono::from_ns(500 * S / 1000)).is_empty());
        // One nanosecond later: the first tick.
        let t = m.ticks(press, Mono::from_ns(500 * S / 1000 + 1));
        assert_eq!(t, vec![Mono::from_ns(500 * S / 1000)]);
        // Full second after delay: first tick + 9 more at 100 ms.
        let t = m.ticks(press, Mono::from_ns(S + 500 * S / 1000));
        assert_eq!(t.len(), 10);
        assert_eq!(t[0], Mono::from_ns(500 * S / 1000));
        assert_eq!(t[1], Mono::from_ns(600 * S / 1000));
        assert_eq!(t[9], Mono::from_ns(1_400 * S / 1000));
    }

    #[test]
    fn windows_are_prefixes_of_one_sequence() {
        let m = RepeatModel {
            rate_hz: 20,
            delay_ms: 100,
        };
        let press = Mono::from_ns(0);
        let early = m.ticks(press, Mono::from_ns(S));
        let late = m.ticks(press, Mono::from_ns(5 * S));
        assert!(late.len() > early.len());
        for (i, t) in early.iter().enumerate() {
            assert_eq!(*t, late[i]);
        }
    }

    #[test]
    fn bounded_form_coalesces_the_latest() {
        let m = RepeatModel {
            rate_hz: 100,
            delay_ms: 0,
        };
        let press = Mono::from_ns(0);
        let bounded = m.ticks_bounded(press, Mono::from_ns(S), 5);
        assert_eq!(bounded.len(), 5);
        let all = m.ticks(press, Mono::from_ns(S));
        // The latest 5, not the earliest 5.
        assert_eq!(bounded, all[all.len() - 5..]);
    }

    #[test]
    fn disabled_repeat_is_empty() {
        let m = RepeatModel {
            rate_hz: 0,
            delay_ms: 500,
        };
        assert!(!m.enabled());
        assert!(m.ticks(Mono::from_ns(0), Mono::from_ns(100 * S)).is_empty());
    }

    #[test]
    fn interval_math() {
        assert_eq!(
            RepeatModel {
                rate_hz: 50,
                delay_ms: 200
            }
            .interval_ns(),
            20_000_000
        );
        assert_eq!(
            RepeatModel {
                rate_hz: 0,
                delay_ms: 200
            }
            .interval_ns(),
            0
        );
    }

    #[test]
    fn nonzero_press_origin() {
        let m = RepeatModel {
            rate_hz: 2,
            delay_ms: 250,
        };
        let press = Mono::from_ms(10_000);
        let t = m.ticks(press, Mono::from_ms(11_000));
        // First repeat at 10.25 s, then 10.75 s — both within 11 s.
        assert_eq!(t, vec![Mono::from_ms(10_250), Mono::from_ms(10_750)]);
    }
}
