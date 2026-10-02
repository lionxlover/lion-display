//! Suspend/resume orchestration and timeline re-anchoring.
//!
//! The controller models the sleep transition pair behind
//! `ldp.session.session.sleeping` / `resumed`: entering sleep (with the
//! wire's sleep kinds) and waking with the report clients see — the
//! approximate suspended duration and the **re-anchor** instant.
//!
//! ## Why re-anchoring exists
//!
//! A display timeline is a vblank grid anchored on observed flips. The
//! panel hardware does not keep the grid through suspend: modesets on
//! resume, PLLs restart, the first post-resume vblank lands at an
//! effectively arbitrary phase. Deadlines extrapolated from the
//! pre-suspend grid are therefore *stale* — the compositor must
//! re-anchor on the first post-resume flip and re-derive every pending
//! deadline from it.
//!
//! [`ReAnchor::grid_align`] computes the target for that first flip:
//! the first grid boundary (phase-preserving: multiples of the nominal
//! interval from the last pre-suspend flip) at or after the wake
//! timestamp. Feeding exactly that flip to the compositor's frame
//! clock (the `FrameClock` of `ldp-compositor`, exercised as a
//! dev-dependency in `tests/reanchor.rs`) re-anchors the PLL with zero
//! phase error at nominal lock — the property the Phase 16
//! exit-criterion suite proves.
//!
//! The controller itself is deliberately blind to inhibitors (the
//! session layer gates manual sleeps), to fences (the GPU layer drains
//! them when it sees the report), and to which clock domain jumped —
//! the host feeds timestamps.

use ldp_core::time::{Mono, RefreshInterval};

/// Why the session is going to sleep (wire: `ldp.session.sleep_kind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SleepKind {
    /// Idle progression reached the suspend stage.
    Idle,
    /// Explicit user/system request.
    Manual,
    /// Laptop lid closed.
    Lid,
    /// Battery critical.
    LowBattery,
}

impl SleepKind {
    /// Wire value (`spec/session.toml`).
    #[must_use]
    pub const fn to_wire(self) -> u32 {
        match self {
            Self::Idle => 1,
            Self::Manual => 2,
            Self::Lid => 3,
            Self::LowBattery => 4,
        }
    }

    /// Parse a wire value.
    #[must_use]
    pub const fn from_wire(v: u32) -> Option<Self> {
        match v {
            1 => Some(Self::Idle),
            2 => Some(Self::Manual),
            3 => Some(Self::Lid),
            4 => Some(Self::LowBattery),
            _ => None,
        }
    }
}

/// The wake report (the `resumed` event's payload plus the
/// compositor-internal re-anchor).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResumeReport {
    /// Approximate sleep duration in milliseconds (the wire's
    /// `suspended_ms`).
    pub suspended_ms: u64,
    /// The first post-resume vblank target: feed this flip to the
    /// output's frame clock to re-anchor.
    pub re_anchor: Mono,
}

/// Sleep state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SuspendState {
    /// Session awake.
    Awake,
    /// Sleeping since the given instant, for the given reason.
    Sleeping {
        /// Why.
        kind: SleepKind,
        /// When sleep began.
        since: Mono,
    },
}

/// The re-anchor math, isolated so the exit-criterion suite can drive
/// it against the real frame clock.
pub struct ReAnchor;

impl ReAnchor {
    /// The first phase-preserving grid boundary strictly after `wake`:
    /// `last_flip + k * nominal` for the smallest `k` that lands
    /// strictly after the wake (at least one interval ahead when the wake
    /// lands exactly on a boundary — a flip at the wake instant has not
    /// happened yet).
    ///
    /// If the pre-suspend anchor is unknown (`last_flip` = 0, or a
    /// wake that precedes it — a clock-domain jump), there is no phase
    /// to preserve: the first sensible flip is one interval after the
    /// later of the two.
    #[must_use]
    pub fn grid_align(wake: Mono, nominal: RefreshInterval, last_flip: Mono) -> Mono {
        let interval = nominal.as_ns().max(1);
        let wake_ns = wake.as_ns();
        let last_ns = last_flip.as_ns();
        // Unknown anchor (0), or a wake that precedes the last flip
        // (clock-domain jump): no phase to preserve — one interval
        // after the later instant.
        if last_ns == 0 || wake_ns <= last_ns {
            return Mono::from_ns(wake_ns.max(last_ns).saturating_add(interval));
        }
        let delta = wake_ns - last_ns;
        let k = delta / interval + 1; // strictly after the wake
        Mono::from_ns(last_ns + k * interval)
    }
}

/// The suspend/resume controller.
#[derive(Clone, Debug)]
pub struct SuspendController {
    state: SuspendState,
}

impl Default for SuspendController {
    fn default() -> Self {
        Self::new()
    }
}

impl SuspendController {
    /// An awake controller.
    #[must_use]
    pub const fn new() -> Self {
        SuspendController {
            state: SuspendState::Awake,
        }
    }

    /// The current state.
    #[must_use]
    pub const fn state(&self) -> SuspendState {
        self.state
    }

    /// Enter sleep (`PrepareForSleep(true)`). Double-sleep is a no-op
    /// returning false (logind does not nest sleeps).
    pub fn sleep(&mut self, kind: SleepKind, now: Mono) -> bool {
        if matches!(self.state, SuspendState::Sleeping { .. }) {
            return false;
        }
        self.state = SuspendState::Sleeping { kind, since: now };
        true
    }

    /// Wake (`PrepareForSleep(false)`). Returns the report with the
    /// suspended duration and the re-anchor target; `nominal` is the
    /// output's refresh and `last_flip` its last pre-suspend flip.
    ///
    /// Waking while awake is a no-op returning `None`.
    pub fn wake(
        &mut self,
        now: Mono,
        nominal: RefreshInterval,
        last_flip: Mono,
    ) -> Option<ResumeReport> {
        let SuspendState::Sleeping { since, .. } = self.state else {
            return None;
        };
        self.state = SuspendState::Awake;
        Some(ResumeReport {
            suspended_ms: now.ms_since(since),
            re_anchor: ReAnchor::grid_align(now, nominal, last_flip),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(m: u64) -> Mono {
        Mono::from_ms(m)
    }

    fn hz60() -> RefreshInterval {
        RefreshInterval::from_millihz(60_000).unwrap()
    }

    #[test]
    fn sleep_wake_round_trip() {
        let mut c = SuspendController::new();
        assert!(c.sleep(SleepKind::Idle, ms(1000)));
        assert!(!c.sleep(SleepKind::Manual, ms(1001)), "no nested sleep");
        let report = c.wake(ms(6000), hz60(), ms(990)).unwrap();
        assert_eq!(report.suspended_ms, 5000);
        // Waking again: nothing.
        assert!(c.wake(ms(7000), hz60(), ms(990)).is_none());
        assert_eq!(c.state(), SuspendState::Awake);
    }

    #[test]
    fn grid_align_is_phase_preserving() {
        let nominal = hz60();
        let interval = nominal.as_ns();
        let last_flip = Mono::from_ns(1_000_000);
        // Wake exactly on a boundary: the next boundary (strictly after).
        let wake = Mono::from_ns(last_flip.as_ns() + 10 * interval);
        let anchor = ReAnchor::grid_align(wake, nominal, last_flip);
        assert_eq!(anchor.as_ns() - last_flip.as_ns(), 11 * interval);
        // Wake between boundaries: round up to the next.
        let wake = Mono::from_ns(last_flip.as_ns() + 10 * interval + interval / 2);
        let anchor = ReAnchor::grid_align(wake, nominal, last_flip);
        assert_eq!(anchor.as_ns() - last_flip.as_ns(), 11 * interval);
        // Wake before the last flip (clock-domain jump): one interval
        // past the flip.
        let wake = Mono::from_ns(1);
        let anchor = ReAnchor::grid_align(wake, nominal, last_flip);
        assert_eq!(anchor.as_ns(), last_flip.as_ns() + interval);
        // Unknown pre-suspend anchor: one interval after wake.
        let anchor = ReAnchor::grid_align(ms(500), nominal, Mono::ZERO);
        assert_eq!(anchor.as_ns(), ms(500).as_ns() + interval);
    }

    #[test]
    fn sleep_kinds_wire() {
        assert_eq!(SleepKind::Idle.to_wire(), 1);
        assert_eq!(SleepKind::LowBattery.to_wire(), 4);
        assert_eq!(SleepKind::from_wire(3), Some(SleepKind::Lid));
        assert_eq!(SleepKind::from_wire(5), None);
    }
}
