//! Refresh-window arithmetic: the scheduling half of the panel window.
//!
//! Two responsibilities:
//!
//! * **Deadline widening** ([`RefreshWindow::scheduler_window`]) — how
//!   much grace the deadline scheduler's adaptive registrations get.
//!   Under deadline policy a commit misses only when it is later than
//!   `grid_target − costs + (max − nominal)`, and the panel can absorb
//!   exactly that much stretch, so a window-satisfied commit presents
//!   at a flip that is still inside the panel's window.
//! * **Flip-slip avoidance** ([`select_flip`]) — the rule that keeps a
//!   VRR panel stable: a flip never lands closer than `min` after the
//!   previous scanout (the slip floor; violating it is what causes
//!   visible flicker on real panels) and never later than `max` (the
//!   stretch ceiling; past it the window has expired). The selection is
//!   `clamp(max(ready_at, last + min), last + min, last + max)` — the
//!   same formula the mock KMS timeline implements, pinned against it
//!   by the integration suite.

use ldp_core::time::Mono;

use crate::caps::{OutputVrrSupport, PanelWindow, WindowError};

/// The window as the scheduler sees it (periods, nanoseconds).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RefreshWindow {
    min_ns: u64,
    max_ns: u64,
}

impl RefreshWindow {
    /// From raw period bounds; rejects zero or inverted windows.
    ///
    /// # Errors
    /// [`WindowError::Inverted`] when `min_ns == 0`, `max_ns == 0`, or
    /// `min_ns > max_ns`.
    pub const fn from_ns(min_ns: u64, max_ns: u64) -> Result<Self, WindowError> {
        if min_ns == 0 || max_ns == 0 || min_ns > max_ns {
            return Err(WindowError::Inverted);
        }
        Ok(Self { min_ns, max_ns })
    }

    /// From a validated panel window.
    #[must_use]
    pub const fn from_panel(panel: PanelWindow) -> Self {
        Self {
            min_ns: panel.min_ns(),
            max_ns: panel.max_ns(),
        }
    }

    /// From validated output support.
    #[must_use]
    pub const fn from_support(support: &OutputVrrSupport) -> Self {
        Self::from_panel(support.window())
    }

    /// The slip floor: the shortest legal interval between flips.
    #[must_use]
    pub const fn min_ns(&self) -> u64 {
        self.min_ns
    }

    /// The stretch ceiling: the longest interval before the window
    /// expires.
    #[must_use]
    pub const fn max_ns(&self) -> u64 {
        self.max_ns
    }

    /// The deadline-widening grace for the scheduler under the given
    /// policy: `max − nominal` (deadline) or `max − min` (always).
    ///
    /// Saturating: a degenerate window at the nominal period yields
    /// zero grace, which is correct — the panel cannot stretch at all.
    #[must_use]
    pub const fn scheduler_window(&self, nominal_ns: u64, always: bool) -> u64 {
        if always {
            self.max_ns.saturating_sub(self.min_ns)
        } else {
            self.max_ns.saturating_sub(nominal_ns)
        }
    }

    /// The earliest legal flip after `last_flip`.
    #[must_use]
    pub const fn earliest_after(&self, last_flip: Mono) -> Mono {
        last_flip.saturating_add_ns(self.min_ns)
    }

    /// The latest flip still inside the window that opened at
    /// `last_flip`.
    #[must_use]
    pub const fn latest_after(&self, last_flip: Mono) -> Mono {
        last_flip.saturating_add_ns(self.max_ns)
    }
}

/// The result of the flip-slip-avoidance selection.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FlipSelection {
    /// The flip lands at this time (≥ ready, ≥ last + min, ≤ last + max).
    At(Mono),
    /// The commit arrived after the whole window had already stretched
    /// past `max`: there is no legal flip slot for it in the window
    /// that opened at `last_flip` (the caller decides between the
    /// fixed-rate fallback and an immediate catch-up flip — see
    /// [`crate::refresh::RefreshSelector`]).
    Missed,
}

/// Select the flip time for content that became ready at `ready_at`,
/// given the previous scanout completed at `last_flip`.
///
/// The **latency optimizer**: the selected time is the earliest legal
/// slot — never later than the content, never violating the slip floor.
/// Returns [`FlipSelection::Missed`] only when `ready_at` itself is
/// later than the window ceiling `last_flip + max`.
#[must_use]
pub const fn select_flip(last_flip: Mono, ready_at: Mono, window: &RefreshWindow) -> FlipSelection {
    let earliest = window.earliest_after(last_flip);
    let latest = window.latest_after(last_flip);
    let chosen = if ready_at.as_ns() > earliest.as_ns() {
        ready_at
    } else {
        earliest
    };
    if chosen.as_ns() > latest.as_ns() {
        FlipSelection::Missed
    } else {
        FlipSelection::At(chosen)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: u64 = 1_000_000_000 / 144; // 6.94 ms
    const MAX: u64 = 1_000_000_000 / 48; // 20.83 ms
    const NOM: u64 = 16_666_666; // 60 Hz

    fn window() -> RefreshWindow {
        RefreshWindow::from_ns(MIN, MAX).unwrap()
    }

    fn t(ns: u64) -> Mono {
        Mono::from_ns(ns)
    }

    #[test]
    fn scheduler_window_math() {
        // Deadline: the stretch beyond the nominal grid point.
        assert_eq!(window().scheduler_window(NOM, false), MAX - NOM);
        // Always: the full selectable span.
        assert_eq!(window().scheduler_window(NOM, true), MAX - MIN);
        // Degenerate panel at exactly the nominal period: no grace.
        assert_eq!(
            RefreshWindow::from_ns(NOM, NOM)
                .unwrap()
                .scheduler_window(NOM, false),
            0
        );
        // Window below nominal (min = max < nominal): saturates to zero
        // rather than underflowing.
        assert_eq!(
            RefreshWindow::from_ns(MIN, MIN)
                .unwrap()
                .scheduler_window(NOM, false),
            0
        );
    }

    #[test]
    fn raw_construction_validates() {
        assert!(RefreshWindow::from_ns(0, MAX).is_err());
        assert!(RefreshWindow::from_ns(MIN, 0).is_err());
        assert!(RefreshWindow::from_ns(MAX, MIN).is_err());
        assert!(RefreshWindow::from_ns(MIN, MAX).is_ok());
    }

    #[test]
    fn bounds_helpers() {
        let w = window();
        assert_eq!(w.earliest_after(t(1_000)).as_ns(), 1_000 + MIN);
        assert_eq!(w.latest_after(t(1_000)).as_ns(), 1_000 + MAX);
    }

    #[test]
    fn early_content_waits_for_the_slip_floor() {
        // Content ready 1 ms after the last flip: the panel cannot
        // re-trigger for MIN total; the flip waits.
        match select_flip(t(1_000_000), t(2_000_000), &window()) {
            FlipSelection::At(at) => assert_eq!(at.as_ns(), 1_000_000 + MIN),
            FlipSelection::Missed => panic!("early content cannot miss"),
        }
    }

    #[test]
    fn ready_content_flips_at_ready() {
        // Content ready past the floor: flips exactly when ready.
        match select_flip(t(1_000_000), t(1_000_000 + MIN + 5), &window()) {
            FlipSelection::At(at) => assert_eq!(at.as_ns(), 1_000_000 + MIN + 5),
            FlipSelection::Missed => panic!("in-window content cannot miss"),
        }
        // Exactly at the floor: legal, immediate.
        match select_flip(t(1_000_000), t(1_000_000 + MIN), &window()) {
            FlipSelection::At(at) => assert_eq!(at.as_ns(), 1_000_000 + MIN),
            FlipSelection::Missed => panic!("floor content cannot miss"),
        }
    }

    #[test]
    fn late_content_misses_only_past_the_ceiling() {
        // Ready exactly at last + max: still inside (inclusive).
        match select_flip(t(1_000_000), t(1_000_000 + MAX), &window()) {
            FlipSelection::At(at) => assert_eq!(at.as_ns(), 1_000_000 + MAX),
            FlipSelection::Missed => panic!("ceiling content is inside"),
        }
        // One nanosecond later: the window has expired.
        assert_eq!(
            select_flip(t(1_000_000), t(1_000_000 + MAX + 1), &window()),
            FlipSelection::Missed
        );
        // Much later (below-min-rate content): missed.
        assert_eq!(
            select_flip(t(1_000_000), t(1_000_000 + 40_000_000), &window()),
            FlipSelection::Missed
        );
    }

    #[test]
    fn selection_always_respects_the_window() {
        // Property: for every ready time, a hit lands within
        // [last + min, last + max] and is >= ready.
        let last = 7_777_777u64;
        for k in 0..=(MAX + 2 * MIN) / 1_000 {
            let ready = last + k * 1_000;
            match select_flip(t(last), t(ready), &window()) {
                FlipSelection::At(at) => {
                    assert!(at.as_ns() >= ready, "selection cannot delay content");
                    assert!(at.as_ns() >= last + MIN, "slip floor violated");
                    assert!(at.as_ns() <= last + MAX, "stretch ceiling violated");
                }
                FlipSelection::Missed => {
                    assert!(ready > last + MAX, "miss requires window expiry");
                }
            }
        }
    }
}
