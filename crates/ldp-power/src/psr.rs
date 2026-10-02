//! Panel Self-Refresh — the sleeping-panel doctrine (Phase 35).
//!
//! The most expensive thing a display server does is *nothing visible*:
//! the display engine keeps scanning the framebuffer out of memory,
//! tens of thousands of times per second, forever, so the panel can
//! show a frame that never changes. Panel Self-Refresh (PSR) is the
//! escape: when the compositor knows the presented content is static,
//! it tells the panel to hold its own copy in the panel's GRAM — the
//! source stops scanning, the pixel clock quiets, the memory bus goes
//! idle, and the SoC's display domain drops to its floor. The machine
//! that sleeps when nothing changes.
//!
//! This module is the *decision* layer — the same purity doctrine as
//! the idle ladder: no clock reads (`observe_quiet`/`disturb` take the
//! injected `Mono`), no ioctls (the host applies the emitted property
//! writes and reports back), no policy about *which* connector (the
//! caller knows the connector's property inventory).
//!
//! The doctrine in three rules:
//!
//! * **Entry is earned by quiet.** One *quiet flip opportunity* is one
//!   pass of the frame choreography in which an output had nothing to
//!   render, nothing owed, and no flip in flight. The machine engages
//!   only after `hysteresis` consecutive quiet opportunities (default
//!   2 — roughly one to two refresh intervals of stillness on a
//!   vblank-paced path, half a second on a poll-paced path): a scene
//!   that bursts one empty frame between animations must never pay the
//!   exit cost to re-enter.
//! * **Exit is named.** Every disturbance carries its cause — damage
//!   arrived, the blanking rung, a brightness transition. Input is
//!   deliberately *not* an exit: touching the machine with no visual
//!   consequence leaves nothing to rescan, and the input→photon
//!   doctrine only counts frames that actually present. The exit cost
//!   itself is paid by the host (the panel must rescan before its next
//!   flip can land — modeled as one refresh interval of re-anchoring
//!   in the KMS layer), and this layer only *accounts* it.
//! * **The ledger is the proof.** The machine counts engagements,
//!   exits, and cumulative self-refresh nanoseconds — the raw material
//!   the energy ledger turns into the measured "static-scene energy"
//!   ratio the comparison table carries. Power claims that cannot be
//!   reproduced in CI are marketing; these are arithmetic.
//!
//! Interplay with the idle ladder: PSR engages far earlier than
//! `Dimmed` (stillness of *content*, not of the *user*), coexists with
//! it (a dimmed panel still benefits — the backlight is the LED, PSR
//! is the scan path; on real panels a brightness transition forces a
//! transient exit, so the ladder's `Dimmed` rung disturbs the machine
//! and the hysteresis re-earns entry), and is vacated by `Off` (a
//! blanked panel has no scan path at all; the machine restarts from
//! `Scanout` on the wake).

use ldp_core::time::Mono;

/// The machine's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PsrState {
    /// Normal scanout — the display engine reads the framebuffer.
    Scanout,
    /// Self-refresh engaged — the panel holds its own GRAM copy.
    SelfRefresh,
}

/// Why self-refresh ended (or was refused). Wire-stable for reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PsrExit {
    /// Damage arrived for this output — content to rescan.
    Damage,
    /// The ladder's blank rung — the scan path itself is going away.
    Blank,
    /// A brightness transition — the panel retrains its backlight
    /// path out of self-refresh (the `Dimmed` rung's ramp).
    Backlight,
    /// The panel's driver refused the engage (the host reported the
    /// property write rejected): the machine retires permanently — a
    /// power hint never retries into a rejecting device.
    Unsupported,
}

impl PsrExit {
    /// Report name (the ledger's exit histogram).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Damage => "damage",
            Self::Blank => "blank",
            Self::Backlight => "backlight",
            Self::Unsupported => "unsupported",
        }
    }
}

/// A transition the host must apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PsrEvent {
    /// Self-refresh engaged: write the connector property on.
    Engaged,
    /// Self-refresh exited for the named cause: the next flip on this
    /// output pays the rescan (the host's timeline re-anchor); write
    /// the property off if no flip follows immediately.
    Exited(PsrExit),
    /// The machine retired — the device refused self-refresh; never
    /// try again this session (the degraded-but-honest state).
    Retired,
}

/// The per-output panel self-refresh decision machine.
#[derive(Clone, Debug)]
pub struct PsrMachine {
    /// Quiet flip opportunities required before entry.
    hysteresis: u32,
    /// Consecutive quiet opportunities seen so far (reset by any
    /// disturbance, consumed by entry).
    quiet: u32,
    /// The current state.
    state: PsrState,
    /// Whether the device refused self-refresh (permanent retire).
    retired: bool,
    /// Total engagements since bring-up.
    engagements: u64,
    /// Exits per cause since bring-up (the ledger's histogram).
    exits: [u64; 4],
    /// When the current (or most recent) engagement started.
    engaged_at: Option<Mono>,
    /// Cumulative nanoseconds spent in self-refresh.
    selfrefresh_ns: u64,
}

impl PsrMachine {
    /// A machine requiring `hysteresis` consecutive quiet flip
    /// opportunities before entry. A hysteresis of zero is promoted
    /// to one — entry is never instantaneous (the frame that *just*
    /// presented is not yet proven static).
    #[must_use]
    pub const fn new(hysteresis: u32) -> Self {
        Self {
            hysteresis: if hysteresis == 0 { 1 } else { hysteresis },
            quiet: 0,
            state: PsrState::Scanout,
            retired: false,
            engagements: 0,
            exits: [0; 4],
            engaged_at: None,
            selfrefresh_ns: 0,
        }
    }

    /// The default entry hysteresis: two quiet opportunities.
    pub const DEFAULT_HYSTERESIS: u32 = 2;

    /// A machine with the default hysteresis.
    #[must_use]
    pub const fn default_machine() -> Self {
        Self::new(Self::DEFAULT_HYSTERESIS)
    }

    /// The current state.
    #[must_use]
    pub const fn state(&self) -> PsrState {
        self.state
    }

    /// Whether self-refresh is currently engaged.
    #[must_use]
    pub const fn engaged(&self) -> bool {
        matches!(self.state, PsrState::SelfRefresh)
    }

    /// Whether the machine retired (the device said no, permanently).
    #[must_use]
    pub const fn retired(&self) -> bool {
        self.retired
    }

    /// Total engagements since bring-up.
    #[must_use]
    pub const fn engagements(&self) -> u64 {
        self.engagements
    }

    /// Exits for the given cause since bring-up.
    #[must_use]
    pub const fn exits_for(&self, cause: PsrExit) -> u64 {
        match cause {
            PsrExit::Damage => self.exits[0],
            PsrExit::Blank => self.exits[1],
            PsrExit::Backlight => self.exits[2],
            PsrExit::Unsupported => self.exits[3],
        }
    }

    /// Cumulative nanoseconds spent in self-refresh — the sleeping
    /// time the energy ledger prices at the panel's floor.
    #[must_use]
    pub const fn selfrefresh_ns(&self) -> u64 {
        self.selfrefresh_ns
    }

    /// One more quiet flip opportunity passed with nothing to do for
    /// this output. Returns the transitions to apply (entry at the
    /// hysteresis point). A retired machine ignores quietness — a
    /// refusing device is never asked again.
    pub fn observe_quiet(&mut self, now: Mono) -> Vec<PsrEvent> {
        if self.retired || self.engaged() {
            return Vec::new();
        }
        self.quiet += 1;
        if self.quiet >= self.hysteresis {
            self.quiet = 0;
            self.state = PsrState::SelfRefresh;
            self.engagements += 1;
            self.engaged_at = Some(now);
            vec![PsrEvent::Engaged]
        } else {
            Vec::new()
        }
    }

    /// A disturbance with the named cause. While engaged, this exits
    /// self-refresh (closing the engagement's time account). While
    /// merely accumulating quietness, this *resets the count* — the
    /// hysteresis is earned by *consecutive* quietness, so a bursty
    /// scene (an empty frame between two animated ones) never
    /// engages. The unsupported cause additionally retires the
    /// machine (the host reported the engage rejected).
    pub fn disturb(&mut self, cause: PsrExit, now: Mono) -> Vec<PsrEvent> {
        self.quiet = 0;
        if cause == PsrExit::Unsupported {
            // The refusal retires the machine even mid-engagement
            // (the device's truth supersedes ours).
            self.retired = true;
        }
        let mut events = Vec::new();
        if self.engaged() {
            self.close_engagement(now);
            self.exits[Self::exit_index(cause)] += 1;
            self.state = PsrState::Scanout;
            events.push(PsrEvent::Exited(cause));
        }
        if self.retired {
            events.push(PsrEvent::Retired);
        }
        events
    }

    /// Close the open engagement's time account at `now`.
    fn close_engagement(&mut self, now: Mono) {
        if let Some(start) = self.engaged_at.take() {
            self.selfrefresh_ns = self
                .selfrefresh_ns
                .saturating_add(now.as_ns().saturating_sub(start.as_ns()));
        }
    }

    /// The exits-array slot of a cause.
    const fn exit_index(cause: PsrExit) -> usize {
        match cause {
            PsrExit::Damage => 0,
            PsrExit::Blank => 1,
            PsrExit::Backlight => 2,
            PsrExit::Unsupported => 3,
        }
    }
}

impl Default for PsrMachine {
    fn default() -> Self {
        Self::default_machine()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ldp_core::time::Mono;

    fn ms(m: u64) -> Mono {
        Mono::from_ns(m * 1_000_000)
    }

    #[test]
    fn entry_needs_consecutive_quiet() {
        let mut m = PsrMachine::default_machine();
        assert_eq!(m.observe_quiet(ms(16)), vec![]);
        assert!(!m.engaged());
        assert_eq!(m.observe_quiet(ms(33)), vec![PsrEvent::Engaged]);
        assert!(m.engaged());
        assert_eq!(m.engagements(), 1);
    }

    #[test]
    fn hysteresis_zero_is_promoted_to_one() {
        let mut m = PsrMachine::new(0);
        // One quiet opportunity engages — never instantaneous entry
        // would be zero, and the promotion guarantees ≥ 1.
        assert_eq!(m.observe_quiet(ms(1)), vec![PsrEvent::Engaged]);
    }

    #[test]
    fn a_disturbance_resets_the_count() {
        let mut m = PsrMachine::default_machine();
        assert_eq!(m.observe_quiet(ms(16)), vec![]);
        // An animated frame between two quiet ones: the count resets,
        // the machine must never engage through a burst.
        assert_eq!(m.disturb(PsrExit::Damage, ms(20)), vec![]);
        assert_eq!(m.observe_quiet(ms(36)), vec![]);
        assert!(!m.engaged());
        assert_eq!(m.observe_quiet(ms(53)), vec![PsrEvent::Engaged]);
    }

    #[test]
    fn damage_exits_and_closes_the_account() {
        let mut m = PsrMachine::default_machine();
        let _ = m.observe_quiet(ms(16));
        let _ = m.observe_quiet(ms(33));
        assert!(m.engaged());
        assert_eq!(
            m.disturb(PsrExit::Damage, ms(150)),
            vec![PsrEvent::Exited(PsrExit::Damage)]
        );
        assert!(!m.engaged());
        assert_eq!(m.exits_for(PsrExit::Damage), 1);
        // The engagement 33 ms → 150 ms: 117 ms of self-refresh.
        assert_eq!(m.selfrefresh_ns(), 117_000_000);
    }

    #[test]
    fn the_exit_histogram_counts_causes_separately() {
        let mut m = PsrMachine::default_machine();
        // First engagement: exit via backlight (the Dim ramp).
        let _ = m.observe_quiet(ms(16));
        let _ = m.observe_quiet(ms(33));
        let _ = m.disturb(PsrExit::Backlight, ms(200));
        // Second engagement: exit via blank (the Off rung).
        let _ = m.observe_quiet(ms(400));
        let _ = m.observe_quiet(ms(416));
        let _ = m.disturb(PsrExit::Blank, ms(500));
        assert_eq!(m.exits_for(PsrExit::Backlight), 1);
        assert_eq!(m.exits_for(PsrExit::Blank), 1);
        assert_eq!(m.exits_for(PsrExit::Damage), 0);
        assert_eq!(m.engagements(), 2);
        // 33→200 plus 416→500: 167 + 84 = 251 ms total.
        assert_eq!(m.selfrefresh_ns(), 251_000_000);
    }

    #[test]
    fn engaged_machines_ignore_further_quietness() {
        let mut m = PsrMachine::default_machine();
        let _ = m.observe_quiet(ms(16));
        let _ = m.observe_quiet(ms(33));
        // Quiet keeps passing while engaged: no re-entry events, no
        // count growth — engagement is idempotent.
        for k in 1..10 {
            assert_eq!(m.observe_quiet(ms(33 + k * 16)), vec![]);
        }
        assert_eq!(m.engagements(), 1);
    }

    #[test]
    fn the_refusal_retires_permanently() {
        let mut m = PsrMachine::default_machine();
        let _ = m.observe_quiet(ms(16));
        let _ = m.observe_quiet(ms(33));
        assert!(m.engaged());
        // The host reports the property write rejected.
        assert_eq!(
            m.disturb(PsrExit::Unsupported, ms(40)),
            vec![PsrEvent::Exited(PsrExit::Unsupported), PsrEvent::Retired]
        );
        assert!(m.retired());
        // Forever after: quietness is ignored.
        assert_eq!(m.observe_quiet(ms(1000)), vec![]);
        assert_eq!(m.observe_quiet(ms(1016)), vec![]);
        assert!(!m.engaged());
        assert_eq!(m.engagements(), 1);
    }

    #[test]
    fn retirement_while_accumulating_is_quiet() {
        let mut m = PsrMachine::default_machine();
        let _ = m.observe_quiet(ms(16));
        // Refused before ever engaging: just the retirement event.
        assert_eq!(
            m.disturb(PsrExit::Unsupported, ms(20)),
            vec![PsrEvent::Retired]
        );
        assert!(m.retired());
        assert_eq!(m.engagements(), 0);
    }

    #[test]
    fn the_re_engagement_after_an_exit_is_earned_again() {
        let mut m = PsrMachine::default_machine();
        let _ = m.observe_quiet(ms(16));
        let _ = m.observe_quiet(ms(33));
        let _ = m.disturb(PsrExit::Damage, ms(50));
        // The exit reset the count: full hysteresis again.
        assert_eq!(m.observe_quiet(ms(66)), vec![]);
        assert_eq!(m.observe_quiet(ms(83)), vec![PsrEvent::Engaged]);
        assert_eq!(m.engagements(), 2);
    }

    #[test]
    fn mono_saturating_close_never_invents_time() {
        let mut m = PsrMachine::default_machine();
        let _ = m.observe_quiet(ms(16));
        let _ = m.observe_quiet(ms(33));
        // A clock that went backwards (it cannot, but the account
        // must saturate, never wrap).
        let _ = m.disturb(PsrExit::Blank, Mono::from_ns(0));
        assert_eq!(m.selfrefresh_ns(), 0);
    }
}
