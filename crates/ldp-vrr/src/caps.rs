//! Adaptive-sync support of one output.
//!
//! The wire vocabulary mirrors `ldp.core.output`'s `vrr` event: a
//! `[min_refresh_millihz, max_refresh_millihz]` window plus the
//! `vrr_caps` bitset (`seamless`, `fixed_rate`). This module wraps both
//! into validated value types and checks the one structural invariant
//! the kernel also enforces before a mode can run adaptive sync: the
//! **nominal mode period must sit inside the window** —
//! `min_period ≤ nominal_period ≤ max_period` — otherwise the fixed
//! grid the deadline scheduler extrapolates would fall outside the
//! panel's legal stretch range and every windowed deadline would be a
//! lie.

use ldp_core::time::RefreshInterval;

/// The `vrr_caps` wire bits (`ldp.core.vrr_caps`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct VrrCaps {
    /// VRR can be toggled without a modeset (`seamless`, bit 0).
    pub seamless: bool,
    /// Fixed-rate fallback available below the minimum refresh rate
    /// (`fixed_rate`, bit 1).
    pub fixed_rate: bool,
}

impl VrrCaps {
    /// Encode to the wire bitset (bits 0 and 1).
    #[must_use]
    pub const fn to_wire(self) -> u32 {
        (self.seamless as u32) | ((self.fixed_rate as u32) << 1)
    }

    /// Decode from a wire bitset; unknown bits are ignored by receivers,
    /// so only the defined bits are read.
    #[must_use]
    pub const fn from_wire(word: u32) -> Self {
        Self {
            seamless: (word & 0x1) != 0,
            fixed_rate: (word & 0x2) != 0,
        }
    }
}

/// A panel's adaptive-sync window `[min, max]` as refresh intervals.
///
/// `min` is the **shortest** period (highest refresh rate) the panel can
/// re-trigger at; `max` is the longest stretch (lowest rate). Stored as
/// periods because every scheduling decision is period arithmetic.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PanelWindow {
    min: RefreshInterval,
    max: RefreshInterval,
}

impl PanelWindow {
    /// Build from periods; rejects zero or inverted windows.
    ///
    /// # Errors
    /// [`WindowError::Inverted`] when `min_ns == 0`, `max_ns == 0`, or
    /// `min_ns > max_ns`.
    pub const fn from_ns(min_ns: u64, max_ns: u64) -> Result<Self, WindowError> {
        if min_ns == 0 || max_ns == 0 || min_ns > max_ns {
            return Err(WindowError::Inverted);
        }
        match (
            RefreshInterval::from_ns(min_ns),
            RefreshInterval::from_ns(max_ns),
        ) {
            (Some(min), Some(max)) => Ok(Self { min, max }),
            _ => Err(WindowError::Inverted),
        }
    }

    /// Build from the wire millihertz pair (`output.vrr` event fields:
    /// the lowest and highest refresh *rates* the panel accepts).
    ///
    /// # Errors
    /// [`WindowError::Inverted`] when either rate is zero or
    /// `min_refresh_millihz > max_refresh_millihz`.
    pub const fn from_millihz(
        min_refresh_millihz: u32,
        max_refresh_millihz: u32,
    ) -> Result<Self, WindowError> {
        if min_refresh_millihz == 0
            || max_refresh_millihz == 0
            || min_refresh_millihz > max_refresh_millihz
        {
            return Err(WindowError::Inverted);
        }
        // Rate inversion: the highest rate owns the shortest period
        // and the lowest rate the longest stretch.
        let min_ns = 1_000_000_000_000 / max_refresh_millihz as u64;
        let max_ns = 1_000_000_000_000 / min_refresh_millihz as u64;
        Self::from_ns(min_ns, max_ns)
    }

    /// The shortest period (highest rate), nanoseconds.
    #[must_use]
    pub const fn min_ns(self) -> u64 {
        self.min.as_ns()
    }

    /// The longest stretch (lowest rate), nanoseconds.
    #[must_use]
    pub const fn max_ns(self) -> u64 {
        self.max.as_ns()
    }

    /// The minimum refresh interval as a value type.
    #[must_use]
    pub const fn min(self) -> RefreshInterval {
        self.min
    }

    /// The maximum refresh interval as a value type.
    #[must_use]
    pub const fn max(self) -> RefreshInterval {
        self.max
    }

    /// The window as wire millihertz
    /// `(min_refresh_rate, max_refresh_rate)` — note the order flip:
    /// the stored `min` is the shortest period, i.e. the highest rate.
    #[must_use]
    pub const fn as_millihz(self) -> (u64, u64) {
        (self.max.as_millihz(), self.min.as_millihz())
    }

    /// Clamp a candidate period into the window.
    #[must_use]
    pub const fn clamp_ns(self, period_ns: u64) -> u64 {
        if period_ns < self.min_ns() {
            self.min_ns()
        } else if period_ns > self.max_ns() {
            self.max_ns()
        } else {
            period_ns
        }
    }

    /// Whether a period sits inside the window (inclusive).
    #[must_use]
    pub const fn contains_ns(self, period_ns: u64) -> bool {
        period_ns >= self.min_ns() && period_ns <= self.max_ns()
    }
}

/// A malformed window.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WindowError {
    /// Zero extent, or minimum period above the maximum.
    Inverted,
    /// The nominal mode period lies outside the panel window: the mode
    /// cannot run adaptive sync against this window.
    NominalOutside,
    /// A zero nominal period.
    ZeroNominal,
}

/// One output's complete adaptive-sync support: the panel window, the
/// capability bits, and the nominal mode period the deadline scheduler
/// grids against.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct OutputVrrSupport {
    window: PanelWindow,
    caps: VrrCaps,
    nominal: RefreshInterval,
}

impl OutputVrrSupport {
    /// Validate and build.
    ///
    /// # Errors
    /// [`WindowError::NominalOutside`] unless
    /// `window.min ≤ nominal ≤ window.max`; [`WindowError::ZeroNominal`]
    /// for a zero nominal period; window construction errors pass
    /// through.
    pub const fn new(
        min_ns: u64,
        max_ns: u64,
        caps: VrrCaps,
        nominal_ns: u64,
    ) -> Result<Self, WindowError> {
        let window = match PanelWindow::from_ns(min_ns, max_ns) {
            Ok(w) => w,
            Err(e) => return Err(e),
        };
        let Some(nominal) = RefreshInterval::from_ns(nominal_ns) else {
            return Err(WindowError::ZeroNominal);
        };
        if !window.contains_ns(nominal_ns) {
            return Err(WindowError::NominalOutside);
        }
        Ok(Self {
            window,
            caps,
            nominal,
        })
    }

    /// The panel window.
    #[must_use]
    pub const fn window(&self) -> PanelWindow {
        self.window
    }

    /// The capability bits.
    #[must_use]
    pub const fn caps(&self) -> VrrCaps {
        self.caps
    }

    /// The nominal mode period, nanoseconds.
    #[must_use]
    pub const fn nominal_ns(&self) -> u64 {
        self.nominal.as_ns()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 48–144 Hz around a 60 Hz mode (the mock panel's window).
    const MIN: u64 = 1_000_000_000 / 144;
    const MAX: u64 = 1_000_000_000 / 48;
    const NOM: u64 = 16_666_666;

    #[test]
    fn caps_wire_round_trip() {
        for word in 0..4u32 {
            let caps = VrrCaps::from_wire(word);
            assert_eq!(caps.to_wire(), word);
        }
        // Unknown bits ignored.
        assert_eq!(
            VrrCaps::from_wire(0b1111_0000),
            VrrCaps {
                seamless: false,
                fixed_rate: false
            }
        );
    }

    #[test]
    fn window_construction_rejects_inversion() {
        assert!(PanelWindow::from_ns(0, MAX).is_err());
        assert!(PanelWindow::from_ns(MIN, 0).is_err());
        assert!(PanelWindow::from_ns(MAX, MIN).is_err());
        assert!(PanelWindow::from_ns(MIN, MIN).is_ok()); // degenerate but legal
        assert!(PanelWindow::from_millihz(48_000, 144_000).is_ok());
        assert!(PanelWindow::from_millihz(0, 144_000).is_err());
        assert!(PanelWindow::from_millihz(144_000, 48_000).is_err());
    }

    #[test]
    fn window_period_conversions() {
        let w = PanelWindow::from_millihz(48_000, 144_000).unwrap();
        assert_eq!(w.min_ns(), 1_000_000_000_000 / 144_000);
        assert_eq!(w.max_ns(), 1_000_000_000_000 / 48_000);
        let (lo, hi) = w.as_millihz();
        assert_eq!((lo, hi), (48_000, 144_000));
    }

    #[test]
    fn window_clamp_and_contains() {
        let w = PanelWindow::from_ns(MIN, MAX).unwrap();
        assert_eq!(w.clamp_ns(1), MIN);
        assert_eq!(w.clamp_ns(MIN), MIN);
        assert_eq!(w.clamp_ns(NOM), NOM);
        assert_eq!(w.clamp_ns(u64::MAX), MAX);
        assert!(w.contains_ns(NOM));
        assert!(!w.contains_ns(MIN - 1));
        assert!(!w.contains_ns(MAX + 1));
    }

    #[test]
    fn support_requires_nominal_inside_window() {
        let caps = VrrCaps {
            seamless: true,
            fixed_rate: true,
        };
        assert!(OutputVrrSupport::new(MIN, MAX, caps, NOM).is_ok());
        // A 165 Hz mode on a 48–144 Hz panel: nominal period 6.06 ms is
        // below the 6.94 ms minimum — the mode cannot run VRR.
        assert_eq!(
            OutputVrrSupport::new(MIN, MAX, caps, 6_060_606),
            Err(WindowError::NominalOutside)
        );
        // A 40 Hz mode stretches past the 20.83 ms maximum.
        assert_eq!(
            OutputVrrSupport::new(MIN, MAX, caps, 25_000_000),
            Err(WindowError::NominalOutside)
        );
        assert_eq!(
            OutputVrrSupport::new(MIN, MAX, caps, 0),
            Err(WindowError::ZeroNominal)
        );
        assert_eq!(
            OutputVrrSupport::new(MAX, MIN, caps, NOM),
            Err(WindowError::Inverted)
        );
    }

    #[test]
    fn support_accessors() {
        let caps = VrrCaps {
            seamless: false,
            fixed_rate: true,
        };
        let s = OutputVrrSupport::new(MIN, MAX, caps, NOM).unwrap();
        assert_eq!(s.window().min_ns(), MIN);
        assert_eq!(s.window().max_ns(), MAX);
        assert_eq!(s.caps(), caps);
        assert_eq!(s.nominal_ns(), NOM);
    }
}
