//! Idle stages — the inhibitor-aware progression behind the
//! `ldp.session.session.idle` events.
//!
//! Stages advance monotonically (active → dimmed → off → suspend) as
//! time-without-activity accumulates; user activity and resume reset to
//! active. Each transition is *delayed* (not cancelled) by its
//! inhibitor bit: with the inhibitor held the stage holds; on release
//! the elapsed-time check re-runs and the transition fires at the next
//! tick — logind's delay semantics.
//!
//! | transition | delayed by |
//! |------------|------------|
//! | active → dimmed | `blur` |
//! | dimmed → off | `display` |
//! | off → suspend | `idle` |
//!
//! (The `suspend` bit gates manual/lid/low-battery sleep in the session
//! layer, not this machine — see `suspend.rs`.)
//!
//! The machine never reads a clock: `tick(now, inhibitors)` is driven
//! by the host's event loop, and every emitted event is data (the DPMS
//! and backlight actions are policy the host applies).

use ldp_core::time::Mono;

/// Idle progression stage (wire: `ldp.session.idle_stage`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
pub enum IdleStage {
    /// User activity current; everything on.
    Active,
    /// Dimmed (backlight ramp down; `blur` inhibitor expired).
    Dimmed,
    /// Display off (DPMS; `display` inhibitor expired).
    Off,
    /// Idle suspend entered (`idle` inhibitor expired).
    Suspend,
}

impl IdleStage {
    /// Wire value (`spec/session.toml`).
    #[must_use]
    pub const fn to_wire(self) -> u32 {
        match self {
            Self::Active => 1,
            Self::Dimmed => 2,
            Self::Off => 3,
            Self::Suspend => 4,
        }
    }

    /// Parse a wire value.
    #[must_use]
    pub const fn from_wire(v: u32) -> Option<Self> {
        match v {
            1 => Some(Self::Active),
            2 => Some(Self::Dimmed),
            3 => Some(Self::Off),
            4 => Some(Self::Suspend),
            _ => None,
        }
    }
}

/// Which behaviors session inhibitors delay
/// (wire: `ldp.session.inhibit_bits` bitset).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InhibitMask {
    bits: u32,
}

impl InhibitMask {
    /// No inhibitors.
    pub const NONE: InhibitMask = InhibitMask { bits: 0 };
    /// Delay dimming/blur of the lock screen (bit 0).
    pub const BLUR: InhibitMask = InhibitMask { bits: 1 << 0 };
    /// Delay display DPMS-off (bit 1).
    pub const DISPLAY: InhibitMask = InhibitMask { bits: 1 << 1 };
    /// Delay idle suspend (bit 2).
    pub const IDLE: InhibitMask = InhibitMask { bits: 1 << 2 };
    /// Delay system suspend (bit 3) — the session layer's manual-sleep
    /// gate; kept here so the mask is one type.
    pub const SUSPEND: InhibitMask = InhibitMask { bits: 1 << 3 };

    /// Union of two masks.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        InhibitMask {
            bits: self.bits | other.bits,
        }
    }

    /// Whether `other`'s bits are all held.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.bits & other.bits == other.bits
    }

    /// The wire word.
    #[must_use]
    pub const fn to_wire(self) -> u32 {
        self.bits
    }

    /// Parse a wire word (unknown bits preserved for forward compat).
    #[must_use]
    pub const fn from_wire(bits: u32) -> Self {
        InhibitMask { bits }
    }
}

/// Stage timeouts, each measured from the *previous* stage's entry
/// (deltas, not cumulative).
#[derive(Clone, Copy, Debug)]
pub struct IdleTimeouts {
    /// active → dimmed after this much inactivity (the
    /// `set_idle_timeout` request's value).
    pub dim_ms: u64,
    /// dimmed → off after this much further time.
    pub off_ms: u64,
    /// off → suspend after this much further time.
    pub suspend_ms: u64,
}

impl Default for IdleTimeouts {
    fn default() -> Self {
        // A desktop-typical ladder: 5 min dim, 1 min to off, 2 min to
        // idle suspend.
        IdleTimeouts {
            dim_ms: 300_000,
            off_ms: 60_000,
            suspend_ms: 120_000,
        }
    }
}

/// One observable idle transition (data only — the host maps these to
/// session events and display actions).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdleEvent {
    /// The `idle` session event payload.
    Stage(IdleStage),
    /// DPMS state change for the display layer.
    Dpms(bool),
    /// Backlight dim (true) / restore (false) request.
    Dim(bool),
}

/// The idle stage machine.
#[derive(Clone, Debug)]
pub struct IdleMachine {
    stage: IdleStage,
    last_activity: Mono,
    timeouts: IdleTimeouts,
}

impl IdleMachine {
    /// A machine starting active at `t0` with the given timeouts.
    #[must_use]
    pub fn new(t0: Mono, timeouts: IdleTimeouts) -> Self {
        IdleMachine {
            stage: IdleStage::Active,
            last_activity: t0,
            timeouts,
        }
    }

    /// The current stage.
    #[must_use]
    pub const fn stage(&self) -> IdleStage {
        self.stage
    }

    /// The last activity timestamp.
    #[must_use]
    pub const fn last_activity(&self) -> Mono {
        self.last_activity
    }

    /// The configured timeouts.
    #[must_use]
    pub const fn timeouts(&self) -> IdleTimeouts {
        self.timeouts
    }

    /// The `set_idle_timeout` request: updates the dim timeout in
    /// place (the running ladder re-evaluates on the next tick).
    pub fn set_idle_timeout(&mut self, timeout_ms: u64) {
        self.timeouts.dim_ms = timeout_ms;
    }

    /// User activity at `now`: resets to active (emitting the wake-up
    /// events only when a stage had advanced).
    pub fn activity(&mut self, now: Mono) -> Vec<IdleEvent> {
        let mut events = Vec::new();
        if self.stage != IdleStage::Active {
            events.push(IdleEvent::Stage(IdleStage::Active));
            events.push(IdleEvent::Dpms(true));
            events.push(IdleEvent::Dim(false));
        }
        self.stage = IdleStage::Active;
        self.last_activity = now;
        events
    }

    /// Advance the machine. Transitions whose inhibitors are held wait;
    /// releasing them lets the (already overdue) transition fire at the
    /// next tick. Returns the transition events in order.
    pub fn tick(&mut self, now: Mono, inhibitors: InhibitMask) -> Vec<IdleEvent> {
        let elapsed_ms = Mono::from_ms(now.ms_since(self.last_activity)).as_ms();
        let mut events = Vec::new();
        if self.stage == IdleStage::Active
            && elapsed_ms >= self.timeouts.dim_ms
            && !inhibitors.contains(InhibitMask::BLUR)
        {
            self.stage = IdleStage::Dimmed;
            events.push(IdleEvent::Stage(IdleStage::Dimmed));
            events.push(IdleEvent::Dim(true));
        }
        if self.stage == IdleStage::Dimmed
            && elapsed_ms >= self.timeouts.dim_ms + self.timeouts.off_ms
            && !inhibitors.contains(InhibitMask::DISPLAY)
        {
            self.stage = IdleStage::Off;
            events.push(IdleEvent::Stage(IdleStage::Off));
            events.push(IdleEvent::Dpms(false));
        }
        if self.stage == IdleStage::Off
            && elapsed_ms >= self.timeouts.dim_ms + self.timeouts.off_ms + self.timeouts.suspend_ms
            && !inhibitors.contains(InhibitMask::IDLE)
        {
            self.stage = IdleStage::Suspend;
            events.push(IdleEvent::Stage(IdleStage::Suspend));
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(m: u64) -> Mono {
        Mono::from_ms(m)
    }

    #[test]
    fn wire_values() {
        assert_eq!(IdleStage::Active.to_wire(), 1);
        assert_eq!(IdleStage::Suspend.to_wire(), 4);
        assert_eq!(IdleStage::from_wire(3), Some(IdleStage::Off));
        assert_eq!(IdleStage::from_wire(5), None);
        assert_eq!(InhibitMask::DISPLAY.to_wire(), 2);
        assert_eq!(InhibitMask::from_wire(8), InhibitMask::SUSPEND);
    }

    #[test]
    fn full_ladder_walk() {
        let timeouts = IdleTimeouts {
            dim_ms: 100,
            off_ms: 50,
            suspend_ms: 200,
        };
        let mut m = IdleMachine::new(ms(0), timeouts);
        assert!(m.tick(ms(99), InhibitMask::NONE).is_empty());
        let events = m.tick(ms(100), InhibitMask::NONE);
        assert_eq!(
            events,
            vec![IdleEvent::Stage(IdleStage::Dimmed), IdleEvent::Dim(true)]
        );
        assert!(m.tick(ms(149), InhibitMask::NONE).is_empty());
        let events = m.tick(ms(150), InhibitMask::NONE);
        assert_eq!(
            events,
            vec![IdleEvent::Stage(IdleStage::Off), IdleEvent::Dpms(false)]
        );
        assert!(m.tick(ms(349), InhibitMask::NONE).is_empty());
        let events = m.tick(ms(350), InhibitMask::NONE);
        assert_eq!(events, vec![IdleEvent::Stage(IdleStage::Suspend)]);
    }

    #[test]
    fn activity_resets_with_wake_events() {
        let timeouts = IdleTimeouts {
            dim_ms: 10,
            off_ms: 10,
            suspend_ms: 10,
        };
        let mut m = IdleMachine::new(ms(0), timeouts);
        m.tick(ms(30), InhibitMask::NONE);
        assert_eq!(m.stage(), IdleStage::Suspend);
        let events = m.activity(ms(31));
        assert_eq!(
            events,
            vec![
                IdleEvent::Stage(IdleStage::Active),
                IdleEvent::Dpms(true),
                IdleEvent::Dim(false)
            ]
        );
        // A second activity while active emits nothing.
        assert!(m.activity(ms(32)).is_empty());
        assert!(m.tick(ms(32), InhibitMask::NONE).is_empty());
    }

    #[test]
    fn inhibitors_delay_then_release_fires() {
        let timeouts = IdleTimeouts {
            dim_ms: 10,
            off_ms: 10,
            suspend_ms: 10,
        };
        let mut m = IdleMachine::new(ms(0), timeouts);
        // BLUR held: no dimming.
        assert!(m.tick(ms(500), InhibitMask::BLUR).is_empty());
        assert_eq!(m.stage(), IdleStage::Active);
        // Released (DISPLAY+IDLE held to isolate the dim step): the
        // overdue dim fires immediately, nothing else.
        let events = m.tick(ms(501), InhibitMask::DISPLAY.union(InhibitMask::IDLE));
        assert_eq!(
            events,
            vec![IdleEvent::Stage(IdleStage::Dimmed), IdleEvent::Dim(true)]
        );
        // DISPLAY held: stuck at dimmed even deep past off.
        assert!(m.tick(ms(900), InhibitMask::DISPLAY).is_empty());
        // Released (IDLE held): off fires, suspend gated.
        let events = m.tick(ms(901), InhibitMask::IDLE);
        assert_eq!(
            events,
            vec![IdleEvent::Stage(IdleStage::Off), IdleEvent::Dpms(false)]
        );
        assert_eq!(m.stage(), IdleStage::Off);
        assert!(m.tick(ms(9999), InhibitMask::IDLE).is_empty());
        // IDLE released: the overdue suspend fires.
        let events = m.tick(ms(10_000), InhibitMask::NONE);
        assert_eq!(events, vec![IdleEvent::Stage(IdleStage::Suspend)]);
    }

    #[test]
    fn long_gap_cascades_through_overdue_stages() {
        // A single tick far past every threshold walks the whole ladder
        // (the machine catches up; stages stay monotonic).
        let timeouts = IdleTimeouts {
            dim_ms: 10,
            off_ms: 10,
            suspend_ms: 10,
        };
        let mut m = IdleMachine::new(ms(0), timeouts);
        let events = m.tick(ms(1000), InhibitMask::NONE);
        assert_eq!(
            events,
            vec![
                IdleEvent::Stage(IdleStage::Dimmed),
                IdleEvent::Dim(true),
                IdleEvent::Stage(IdleStage::Off),
                IdleEvent::Dpms(false),
                IdleEvent::Stage(IdleStage::Suspend),
            ]
        );
    }

    #[test]
    fn set_idle_timeout_retargets_the_ladder() {
        let mut m = IdleMachine::new(
            ms(0),
            IdleTimeouts {
                dim_ms: 100,
                off_ms: 100,
                suspend_ms: 100,
            },
        );
        m.set_idle_timeout(50);
        assert!(m.tick(ms(49), InhibitMask::NONE).is_empty());
        assert_eq!(
            m.tick(ms(50), InhibitMask::NONE)[0],
            IdleEvent::Stage(IdleStage::Dimmed)
        );
    }
}
