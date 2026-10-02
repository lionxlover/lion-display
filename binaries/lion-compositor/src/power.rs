//! The power ladder, the input rig (Phase 31), and the static-frame
//! power path (Phase 35): panel self-refresh, the GPU clock
//! governor, and the energy ledger.
//!
//! Phase 35's doctrine — the machine that sleeps when nothing
//! changes: [`World::psr_tick`] is the serve loop's power cadence
//! (every poll turn, ≤ 250 ms). Each tick samples the energy ledger's
//! state (blanked / dimmed / self-refresh / render / scanout), feeds
//! the governor's decision interval (the landed-flip load), and steps
//! every output's PSR machine — a quiet output earns its entry
//! hysteresis, an engaged one holds, a busy one's count resets. The
//! engage is a connector property write; the exit is usually *no
//! write at all* — the next flip's implicit rescan (the kernel's own
//! semantics, modeled by the mock) releases the panel, and the world
//! pays the exit's cost in the ledger. Only the backlight transition
//! (the ladder's Dim rung — the panel retrains) writes the release
//! explicitly.
//!
//! The exits are named and paid: [`PsrExit::Damage`] (a flip was
//! submitted — the render path disturbs at `render_slot`),
//! [`PsrExit::Blank`] (the Off rung — the DPMS commit carries the
//! release), [`PsrExit::Backlight`] (the Dim rung's ramp),
//! [`PsrExit::Unsupported`] (the device refused the engage — the
//! machine retires, a power hint never retries into a device that
//! said no once). Input is deliberately *not* an exit: touching the
//! machine with no visual consequence leaves nothing to rescan.
//!
//! The idle machinery always existed as a library (`ldp-power`'s
//! `IdleMachine` — Active → Dimmed → Off with inhibitors and the
//! logind delay semantics); Phase 31 wires it into the *running
//! compositor*: every wake marks activity, the serve loop ticks the
//! ladder at the device's clock, `Dimmed` reports the backlight ramp,
//! and `Off` blanks the panel through the DPMS property (the honest
//! atomic vocabulary — the pipeline stays enabled, the scanout
//! content survives, the scheduler parks: pending frame requests die
//! with `output_off`, new ones defer until the wake). Activity lights
//! it back up.
//!
//! The input rig: [`World::inject_input`] — the input-arrival path
//! the latency rig measures. Input marks activity (the ladder's
//! feed), sets the render trigger (input-driven damage rides the next
//! frame — the pump's own commit cycle), and records the arrival so
//! the next *landed flip* measures the input→photon latency — the
//! end-to-end number the comparison's "input latency" row carries.

#![forbid(unsafe_code)]

use ldp_core::time::Mono;
use ldp_display::atomic::AtomicRequest;
use ldp_display::commit::{CommitFlags, DpmsState};
use ldp_power::governor::PState;
use ldp_power::idle::{IdleEvent, IdleStage, InhibitMask};
use ldp_power::ledger::LedgerState;
use ldp_power::psr::{PsrEvent, PsrExit};

pub use ldp_power::idle::IdleEvent as PowerEvent;

use crate::scene::World;

impl World {
    /// Tick the idle ladder at the device's clock — the serve loop's
    /// periodic arm (and the tests' driver). Returns the transition
    /// events (`Stage`/`Dpms`/`Dim`) in order; the world acts on
    /// them: `Off` blanks every output through the DPMS property and
    /// parks the scheduler, the wake lights everything back.
    ///
    /// # Errors
    ///
    /// [`crate::frame_loop::FrameError`] — a DPMS commit rejected by
    /// the device (fatal for the caller, the display-doctrine rule).
    pub fn tick_idle(&mut self) -> Result<Vec<IdleEvent>, crate::frame_loop::FrameError> {
        let Some(machine) = self.idle.as_mut() else {
            return Ok(Vec::new());
        };
        let now = self.device.now();
        let events = machine.tick(now, InhibitMask::default());
        for event in &events {
            match event {
                IdleEvent::Stage(IdleStage::Off) => self.blank_all()?,
                IdleEvent::Stage(IdleStage::Active) => self.unblank_all()?,
                // The Dim rung's backlight ramp (Phase 35): a real
                // panel retrains its brightness path out of
                // self-refresh — every sleeping output wakes for the
                // transition, then re-earns entry through the
                // hysteresis once the ramp settles.
                IdleEvent::Stage(IdleStage::Dimmed) | IdleEvent::Dim(true) => {
                    self.psr_disturb_all(PsrExit::Backlight);
                }
                _ => {}
            }
        }
        Ok(events)
    }

    /// Mark activity (the ladder's feed — every client message and
    /// every input arrival). A wake from `Off`/`Dimmed` returns the
    /// ladder to `Active` immediately (the events ride the next
    /// `tick_idle`; the un-blanking happens there).
    ///
    /// # Errors
    ///
    /// [`crate::frame_loop::FrameError`] — the DPMS wake commit was
    /// rejected by the device (fatal for the caller, the
    /// display-doctrine rule).
    pub fn note_activity(&mut self) -> Result<(), crate::frame_loop::FrameError> {
        let now = self.device.now();
        if let Some(machine) = self.idle.as_mut() {
            machine.activity(now);
        }
        if self.blanked {
            // The wake: light the panels immediately — the DPMS
            // commits and the scheduler's resume, the same vocabulary
            // the blanking used in reverse.
            self.unblank_all()?;
        }
        Ok(())
    }

    /// Blank every output: the DPMS property off (the pipeline stays
    /// enabled — the scanout content survives the blank), the
    /// scheduler parked (pending frame requests die `output_off`,
    /// new ones defer until the wake).
    fn blank_all(&mut self) -> Result<(), crate::frame_loop::FrameError> {
        if self.blanked {
            return Ok(());
        }
        for slot in &self.outputs {
            // The scan path dies at the blank — a sleeping panel is
            // released in the same commit (the property rides the
            // DPMS write, one atomic turn). Incapable connectors
            // write DPMS alone: the property list said no, and a
            // power hint must never fail the blank.
            let request = AtomicRequest::new()
                .flag(CommitFlags::NONBLOCK)
                .connector_dpms(slot.output.connector.id, DpmsState::Off);
            let request = if slot.psr_capable {
                request.connector_psr(slot.output.connector.id, false)
            } else {
                request
            };
            self.device.commit(&request)?;
        }
        let now = self.device.now();
        self.scene.scheduler.park(now);
        self.blanked = true;
        // The machines learn: the Blank exit closes every open
        // engagement's account (the ledger pays the exit; the wake
        // re-earns entry from zero).
        for index in 0..self.outputs.len() {
            self.psr_disturb_slot(index, PsrExit::Blank, false);
        }
        // The ladder's own transition cost.
        self.ledger.dpms_transition();
        Ok(())
    }

    /// Light every output back: the DPMS property on, the scheduler
    /// resumed at the device clock.
    fn unblank_all(&mut self) -> Result<(), crate::frame_loop::FrameError> {
        if !self.blanked {
            return Ok(());
        }
        for slot in &self.outputs {
            let request = AtomicRequest::new()
                .flag(CommitFlags::NONBLOCK)
                .connector_dpms(slot.output.connector.id, DpmsState::On);
            self.device.commit(&request)?;
        }
        let now = self.device.now();
        self.scene.scheduler.resume(now);
        self.blanked = false;
        // The ladder's own transition cost (the wake side).
        self.ledger.dpms_transition();
        Ok(())
    }

    /// The serve loop's power cadence (Phase 35): one tick per poll
    /// turn (≤ 250 ms of wall time on the real path; the test driver
    /// calls it directly at its own cadence). Three clocks advance:
    ///
    /// * the **energy ledger** — the interval since the last tick is
    ///   priced in the folded power state (blanked > dimmed >
    ///   all-asleep self-refresh > render-at-the-governor's-rung >
    ///   scanout), and the wake itself is paid (the poll discipline
    ///   is honest about its own cost);
    /// * the **governor** — one decision interval: the landed-flip
    ///   load against the pacing grid's capacity (up instant, down
    ///   patient);
    /// * every output's **PSR machine** — a quiet output (no pending
    ///   damage, nothing owed, no flip in flight, the scene clean,
    ///   not blanked) earns one quiet flip opportunity toward its
    ///   entry hysteresis; a busy output's count resets (the
    ///   consecutive-quiet contract); an engaged busy output is
    ///   disturbed (its damage's flip is coming).
    ///
    /// Engagements apply as connector property commits immediately —
    /// a rejection retires the machine for the session (a power hint
    /// degrades, never dies).
    ///
    /// # Errors
    ///
    /// [`crate::frame_loop::FrameError`] — an engage/release commit
    /// path failed fatally (the DPMS doctrine's error family; the
    /// property writes themselves degrade, never propagate).
    pub fn psr_tick(&mut self) -> Result<(), crate::frame_loop::FrameError> {
        if !self.psr_enabled {
            return Ok(());
        }
        let now = self.device.now();
        // The wake itself: one poll turn of the compositor's loop.
        self.ledger.add_wakes(1);
        // The ledger's interval closes in the state that held.
        self.power_sample(now);
        // The governor's decision interval.
        self.governor_window(now);
        // The machines.
        let dirty = self.scene.dirty;
        let blanked = self.blanked;
        let mut engages: Vec<usize> = Vec::new();
        let mut engaged_busy: Vec<usize> = Vec::new();
        for (index, slot) in self.outputs.iter_mut().enumerate() {
            if !slot.psr_capable || slot.psr.retired() {
                continue;
            }
            let quiet = slot.pending.is_empty()
                && !slot.owes
                && !slot.scanout.flip_pending
                && !dirty
                && !blanked;
            if quiet {
                let events = slot.psr.observe_quiet(now);
                if events
                    .iter()
                    .any(|event| matches!(event, PsrEvent::Engaged))
                {
                    engages.push(index);
                }
            } else if slot.psr.engaged() {
                // Busy while asleep: the damage's flip is coming — the
                // machine must exit now (the device will release at
                // the flip's submission regardless of what we write).
                engaged_busy.push(index);
            } else {
                // Busy and awake: the count resets — quietness is
                // earned consecutively.
                let _ = slot.psr.disturb(PsrExit::Damage, now);
            }
        }
        // Apply the engagements: the connector property writes.
        for index in engages {
            let connector = self.outputs[index].output.connector.id;
            let request = AtomicRequest::new()
                .flag(CommitFlags::NONBLOCK)
                .connector_psr(connector, true);
            if self.device.commit(&request).is_ok() {
                self.outputs[index].psr_live = true;
            } else {
                // The refusal retires: a power hint never retries into
                // a device that said no once.
                let _ = self.outputs[index].psr.disturb(PsrExit::Unsupported, now);
                self.outputs[index].psr_live = false;
            }
        }
        // Disturb the busy-engaged (the Damage exit rides the coming
        // flip — no property write).
        for index in engaged_busy {
            self.psr_disturb_slot(index, PsrExit::Damage, false);
        }
        Ok(())
    }

    /// Disturb one output's PSR machine. The first disturbance of an
    /// *engaged* machine closes the engagement (the ledger pays the
    /// exit); later ones only reset the quiet count. The explicit
    /// release write (the backlight transition — no flip follows) is
    /// `release`; the Damage and Blank exits ride the flip and the
    /// DPMS commit respectively.
    pub(crate) fn psr_disturb_slot(&mut self, index: usize, cause: PsrExit, release: bool) {
        let now = self.device.now();
        let Some(slot) = self.outputs.get_mut(index) else {
            return;
        };
        let events = slot.psr.disturb(cause, now);
        if events.is_empty() {
            return;
        }
        let was_live = slot.psr_live;
        let capable = slot.psr_capable;
        let connector = slot.output.connector.id;
        slot.psr_live = false;
        for event in events {
            if matches!(event, PsrEvent::Exited(_)) {
                // The rescan's retraining cost — paid once per exit.
                self.ledger.psr_exit();
            }
        }
        if release && was_live && capable {
            // The explicit release: the panel wakes with no flip
            // coming (the backlight retrain). A rejection is honest
            // to ignore — the next flip's implicit release covers it.
            let request = AtomicRequest::new()
                .flag(CommitFlags::NONBLOCK)
                .connector_psr(connector, false);
            let _ = self.device.commit(&request);
        }
    }

    /// Disturb every output's machine (the ladder's rung events).
    fn psr_disturb_all(&mut self, cause: PsrExit) {
        for index in 0..self.outputs.len() {
            self.psr_disturb_slot(index, cause, cause == PsrExit::Backlight);
        }
    }

    /// Close the ledger's interval in the state that held it.
    fn power_sample(&mut self, now: Mono) {
        let state = self.power_state();
        self.ledger.account_since(state, self.power_sample_at, now);
        self.power_sample_at = now;
    }

    /// The folded power state of the whole world (the ledger's
    /// pricing key): blanked outranks dimmed, dimmed outranks
    /// self-refresh, all-capable-asleep is self-refresh, a window
    /// with landed flips renders at the governor's rung, and the rest
    /// is the static-scanout baseline — the "asleep" a
    /// damage-driven compositor still pays when the panel cannot
    /// self-refresh.
    fn power_state(&self) -> LedgerState {
        if self.blanked {
            return LedgerState::Blanked;
        }
        if self
            .idle
            .as_ref()
            .is_some_and(|machine| machine.stage() == IdleStage::Dimmed)
        {
            return LedgerState::Dimmed;
        }
        let capable: Vec<bool> = self.outputs.iter().map(|slot| slot.psr_capable).collect();
        if capable.iter().any(|c| *c)
            && self
                .outputs
                .iter()
                .zip(capable)
                .filter(|(_, capable)| *capable)
                .all(|(slot, _)| slot.psr.engaged())
        {
            return LedgerState::SelfRefresh;
        }
        if self.governor_flips > 0 {
            return LedgerState::Render(self.governor.state());
        }
        LedgerState::Scanout
    }

    /// The governor's decision interval: the landed-flip load against
    /// the pacing grid's capacity over the elapsed window.
    fn governor_window(&mut self, now: Mono) {
        let since_last = now.as_ns().saturating_sub(self.governor_at.as_ns());
        let window_ms = since_last / 1_000_000;
        if window_ms == 0 {
            return;
        }
        let nominal = self.outputs.first().map_or_else(
            || ldp_core::time::RefreshInterval::from_millihz(60_000).expect("60 Hz is positive"),
            |slot| slot.output.refresh(),
        );
        let _ = self
            .governor
            .interval(self.governor_flips, nominal, window_ms);
        self.governor_flips = 0;
        self.governor_at = now;
    }

    /// The power report (Phase 35): the ledger's verdict line — the
    /// operator reads it at selftest and teardown; the session tests
    /// assert it.
    #[must_use]
    pub fn power_report(&self) -> String {
        self.ledger.report_line()
    }

    /// The governor's current rung (the report's clock face).
    #[must_use]
    pub fn gpu_pstate(&self) -> PState {
        self.governor.state()
    }

    /// The input-arrival path (Phase 31's latency rig): marks activity
    /// (the ladder's feed), arms the render trigger (input-driven
    /// damage rides the next frame — the pump's commit cycle; the
    /// desktop's own chrome, the cursor of the future), and records
    /// the arrival so the next *landed flip* measures the
    /// input→photon latency. Returns the arrival timestamp (the
    /// rig's reference).
    pub fn inject_input(&mut self) -> Mono {
        let now = self.device.now();
        if let Err(e) = self.note_activity() {
            eprintln!("lion-compositor: input wake failed: {e}");
        }
        self.last_input = Some(now);
        now
    }

    /// The measured input→photon latency of the most recent input
    /// that rode a landed flip — the end-to-end number (nanoseconds):
    /// input arrival to the panel's scanout flip. `None` until an
    /// injected input has ridden a flip.
    #[must_use]
    pub fn last_input_photon_ns(&self) -> Option<u64> {
        self.input_photon_ns
    }

    /// The landed-flip arm of the latency rig (called from the frame
    /// loop's landing): the flip that carries an injected input's
    /// frame closes the measurement.
    pub(crate) fn measure_input_photon(&mut self, flip_ts: Mono) {
        if let Some(arrival) = self.last_input.take() {
            self.input_photon_ns = Some(flip_ts.as_ns().saturating_sub(arrival.as_ns()));
        }
    }
}
