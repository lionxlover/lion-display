//! The energy ledger — power as accounting, not assertion (Phase 35).
//!
//! "Power efficient" is unfalsifiable until it is *counted*. This
//! module is the counting: a documented parametric model of what each
//! compositor state costs, and a ledger that accumulates observed
//! time-in-state into an energy figure and a headline ratio. The
//! model's numbers are deliberately public, deliberate, and
//! *labeled as a model*: the ballparks of a modern laptop SoC's
//! display domain, from public vendor documentation and measured
//! teardowns of comparable silicon — not a claim about any specific
//! machine. The point is the *ratios*: they are arithmetic over
//! states the CI can drive deterministically, so a regression that
//! keeps the panel awake through a static scene is a red test, not a
//! warmer lap.
//!
//! The states (milliwatts, the model's defaults — see
//! [`PowerModel::DEFAULT`]):
//!
//! | state | draw | what it is |
//! |---|---|---|
//! | [`LedgerState::Render`] at P0 | 420 | compositing at the deepest clock |
//! | [`LedgerState::Render`] at P3 | 1850 | compositing at the ceiling clock |
//! | [`LedgerState::Scanout`] | 350 | the display engine scanning a static framebuffer — the "asleep" a damage-driven compositor still pays |
//! | [`LedgerState::SelfRefresh`] | 45 | the panel holding its own GRAM: the pixel clock quiet, the memory bus idle |
//! | [`LedgerState::Dimmed`] | 190 | scanout with the backlight at the dim rung (the LED at ~55%) |
//! | [`LedgerState::Blanked`] | 8 | DPMS off — the pipeline enabled, the panel dark |
//!
//! Transitions cost energy too (the PSR exit's rescan, the DPMS
//! wake), and wakes cost per event — the poll-cadence discipline the
//! serve loop practices shows up here as *wakes per interval*, priced
//! honestly so a 60 Hz vblank-driven idle loop cannot hide behind a
//! "but nothing renders" defense.
//!
//! The ledger's inputs come from the world's own truth: the PSR
//! machines' cumulative self-refresh time, the governor's residency,
//! the idle ladder's stage timeline, the flip counters. The session
//! test drives a scripted day (animated, then static-with-PSR, then
//! static-without) and the ledger returns the verdict the comparison
//! table carries: **the static-scene ratio** — static-with-PSR energy
//! over the static-without counterfactual.

use crate::governor::PState;
use ldp_core::time::Mono;

/// One accounted compositor state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum LedgerState {
    /// Compositing — flips landing, passes rendering. The draw
    /// depends on the governor's rung.
    Render(PState),
    /// Static without self-refresh: nothing renders, but the display
    /// engine keeps scanning the unchanged framebuffer out of memory.
    Scanout,
    /// Static *with* self-refresh: the panel holds its own copy.
    SelfRefresh,
    /// The ladder's dim rung: scanout (or self-refresh) at the dimmed
    /// backlight — priced at the dimmed level.
    Dimmed,
    /// The ladder's off rung: DPMS blank.
    Blanked,
}

impl LedgerState {
    /// The state's name (the report line).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Render(PState::P0) => "render-p0",
            Self::Render(PState::P1) => "render-p1",
            Self::Render(PState::P2) => "render-p2",
            Self::Render(PState::P3) => "render-p3",
            Self::Scanout => "scanout",
            Self::SelfRefresh => "self-refresh",
            Self::Dimmed => "dimmed",
            Self::Blanked => "blanked",
        }
    }
}

/// The parametric cost model. Defaults are the documented ballpark
/// figures; a host with better numbers (a sysfs power meter, a
/// vendor's DVFS table) overrides them — the ledger's arithmetic is
/// unchanged.
#[derive(Clone, Copy, Debug)]
pub struct PowerModel {
    /// Milliwatts compositing at each P-state rung `[p0, p1, p2, p3]`.
    pub render_mw: [f64; 4],
    /// Milliwatts scanning a static framebuffer (no PSR).
    pub scanout_mw: f64,
    /// Milliwatts in panel self-refresh.
    pub selfrefresh_mw: f64,
    /// Milliwatts at the dim rung.
    pub dimmed_mw: f64,
    /// Milliwatts blanked (DPMS off).
    pub blanked_mw: f64,
    /// Millijoules per PSR exit (the rescan's panel retraining).
    pub psr_exit_mj: f64,
    /// Millijoules per DPMS transition (blank or wake).
    pub dpms_transition_mj: f64,
    /// Millijoules per wake (a device event or poll timeout that
    /// reaches the compositor's loop).
    pub wake_mj: f64,
}

impl PowerModel {
    /// The documented default model (see the module table).
    pub const DEFAULT: PowerModel = PowerModel {
        render_mw: [420.0, 780.0, 1250.0, 1850.0],
        scanout_mw: 350.0,
        selfrefresh_mw: 45.0,
        dimmed_mw: 190.0,
        blanked_mw: 8.0,
        psr_exit_mj: 2.4,
        dpms_transition_mj: 6.0,
        wake_mj: 0.9,
    };
}

/// The accumulated account.
#[derive(Clone, Debug)]
pub struct EnergyLedger {
    /// The cost model.
    model: PowerModel,
    /// Nanoseconds accumulated per [`LedgerState`] (render states by
    /// rung — the governor's residency translated).
    ns_in_state: [u64; 8],
    /// PSR exits paid.
    psr_exits: u64,
    /// DPMS transitions paid.
    dpms_transitions: u64,
    /// Wakes paid.
    wakes: u64,
}

impl EnergyLedger {
    /// The slot of a state in `ns_in_state`.
    const fn slot(state: LedgerState) -> usize {
        match state {
            LedgerState::Render(PState::P0) => 0,
            LedgerState::Render(PState::P1) => 1,
            LedgerState::Render(PState::P2) => 2,
            LedgerState::Render(PState::P3) => 3,
            LedgerState::Scanout => 4,
            LedgerState::SelfRefresh => 5,
            LedgerState::Dimmed => 6,
            LedgerState::Blanked => 7,
        }
    }

    /// An empty ledger over the default model.
    #[must_use]
    pub const fn new() -> Self {
        Self::with_model(PowerModel::DEFAULT)
    }

    /// An empty ledger over the given model.
    #[must_use]
    pub const fn with_model(model: PowerModel) -> Self {
        Self {
            model,
            ns_in_state: [0; 8],
            psr_exits: 0,
            dpms_transitions: 0,
            wakes: 0,
        }
    }

    /// Account `ns` in `state` (the residency sample).
    pub fn account(&mut self, state: LedgerState, ns: u64) {
        self.ns_in_state[Self::slot(state)] =
            self.ns_in_state[Self::slot(state)].saturating_add(ns);
    }

    /// Account a residency interval from `since` to `now` in `state`.
    pub fn account_since(&mut self, state: LedgerState, since: Mono, now: Mono) {
        let ns = now.as_ns().saturating_sub(since.as_ns());
        self.account(state, ns);
    }

    /// One PSR exit (the rescan's retraining cost).
    pub fn psr_exit(&mut self) {
        self.psr_exits += 1;
    }

    /// One DPMS transition (blank or wake).
    pub fn dpms_transition(&mut self) {
        self.dpms_transitions += 1;
    }

    /// Record `count` wakes (the loop's own poll/vblank discipline).
    pub fn add_wakes(&mut self, count: u64) {
        self.wakes = self.wakes.saturating_add(count);
    }

    /// Nanoseconds accumulated in a state.
    #[must_use]
    pub const fn ns_in(&self, state: LedgerState) -> u64 {
        self.ns_in_state[Self::slot(state)]
    }

    /// Total transitions and wakes paid.
    #[must_use]
    pub const fn psr_exits(&self) -> u64 {
        self.psr_exits
    }

    /// DPMS transitions paid.
    #[must_use]
    pub const fn dpms_transitions(&self) -> u64 {
        self.dpms_transitions
    }

    /// Wakes paid.
    #[must_use]
    pub const fn wakes(&self) -> u64 {
        self.wakes
    }

    /// The model the ledger prices with.
    #[must_use]
    pub const fn model(&self) -> &PowerModel {
        &self.model
    }

    /// The total energy in millijoules: time-in-state priced by the
    /// model, plus transitions and wakes.
    #[must_use]
    pub fn total_mj(&self) -> f64 {
        let mut mj = 0.0;
        for (slot, ns) in self.ns_in_state.iter().enumerate() {
            if *ns == 0 {
                continue;
            }
            let state = match slot {
                0 => LedgerState::Render(PState::P0),
                1 => LedgerState::Render(PState::P1),
                2 => LedgerState::Render(PState::P2),
                3 => LedgerState::Render(PState::P3),
                4 => LedgerState::Scanout,
                5 => LedgerState::SelfRefresh,
                6 => LedgerState::Dimmed,
                _ => LedgerState::Blanked,
            };
            let mw = self.draw(state);
            mj += mw * (*ns as f64) / 1e9;
        }
        mj += self.psr_exits as f64 * self.model.psr_exit_mj;
        mj += self.dpms_transitions as f64 * self.model.dpms_transition_mj;
        mj += self.wakes as f64 * self.model.wake_mj;
        mj
    }

    /// The draw of a state under this ledger's model.
    fn draw(&self, state: LedgerState) -> f64 {
        match state {
            LedgerState::Render(rung) => self.model.render_mw[rung.index()],
            LedgerState::Scanout => self.model.scanout_mw,
            LedgerState::SelfRefresh => self.model.selfrefresh_mw,
            LedgerState::Dimmed => self.model.dimmed_mw,
            LedgerState::Blanked => self.model.blanked_mw,
        }
    }

    /// The headline: the static-scene ratio — this ledger's energy
    /// over the *always-scanning counterfactual* (the same timeline
    /// with every nanosecond priced at [`LedgerState::Scanout`]'s
    /// draw, no transitions, no wakes). A fully static session with
    /// self-refresh engaged for all but the hysteresis prefix returns
    /// the model's floor ratio (~13% at the defaults: 45/350, plus
    /// the prefix); a session that fails to engage returns ~1.0.
    ///
    /// This is the number the comparison table's power row carries:
    /// not "efficient" but *what fraction of the naive compositor's
    /// static-scene energy this machine draws* — reproduced in CI by
    /// driving the session and reading the ledger.
    #[must_use]
    pub fn static_scene_ratio(&self) -> f64 {
        let total_ns: u64 = self.ns_in_state.iter().sum();
        if total_ns == 0 {
            return 1.0;
        }
        let counterfactual_mj = self.model.scanout_mw * (total_ns as f64) / 1e9;
        let actual = self.total_mj();
        if counterfactual_mj <= 0.0 {
            return 1.0;
        }
        actual / counterfactual_mj
    }

    /// The one-line report (the selftest and teardown print).
    #[must_use]
    pub fn report_line(&self) -> String {
        format!(
            "ledger: {:.1} mJ, static-ratio {:.3}, self-refresh {:.1}s of {:.1}s, \
             exits {} ({} rescan), wakes {}",
            self.total_mj(),
            self.static_scene_ratio(),
            self.ns_in(LedgerState::SelfRefresh) as f64 / 1e9,
            self.ns_in_state.iter().sum::<u64>() as f64 / 1e9,
            self.psr_exits + self.dpms_transitions,
            self.psr_exits,
            self.wakes
        )
    }
}

impl Default for EnergyLedger {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::governor::PState;
    use ldp_core::time::Mono;

    fn ms(m: u64) -> Mono {
        Mono::from_ns(m * 1_000_000)
    }

    #[test]
    fn the_floor_ratio_of_a_perfect_static_session() {
        let mut l = EnergyLedger::new();
        // 500 ms of scanout hysteresis, then 9.5 s of self-refresh.
        l.account(LedgerState::Scanout, 500_000_000);
        l.account(LedgerState::SelfRefresh, 9_500_000_000);
        // One exit (the damage that ends the session).
        l.psr_exit();
        // The poll-paced wake discipline: 4 Hz over 10 s.
        l.add_wakes(40);
        // Expected: 0.5 s at 350 mW + 9.5 s at 45 mW + 2.4 + 40*0.9.
        let expected = 350.0 * 0.5 + 45.0 * 9.5 + 2.4 + 36.0;
        assert!((l.total_mj() - expected).abs() < 1e-6, "{}", l.total_mj());
        // Ratio over the 10 s all-scanout counterfactual (3500 mJ).
        let ratio = l.static_scene_ratio();
        assert!(ratio < 0.25, "the floor must beat 4:1, got {ratio}");
        assert!(ratio > 0.10, "the hysteresis prefix keeps it honest");
    }

    #[test]
    fn the_session_that_never_sleeps_pays_full_price() {
        let mut l = EnergyLedger::new();
        l.account(LedgerState::Scanout, 10_000_000_000);
        let ratio = l.static_scene_ratio();
        assert!((ratio - 1.0).abs() < 1e-9);
    }

    #[test]
    fn render_states_price_by_rung() {
        let mut l = EnergyLedger::new();
        l.account(LedgerState::Render(PState::P0), 1_000_000_000);
        l.account(LedgerState::Render(PState::P3), 1_000_000_000);
        // One second each at 420 mW and 1850 mW.
        assert!((l.total_mj() - 2270.0).abs() < 1e-6);
    }

    #[test]
    fn the_ladder_rungs_price_their_own() {
        let mut l = EnergyLedger::new();
        l.account(LedgerState::Dimmed, 1_000_000_000);
        l.account(LedgerState::Blanked, 1_000_000_000);
        assert!((l.total_mj() - 198.0).abs() < 1e-6);
        // The ladder's transitions are paid too.
        l.dpms_transition();
        l.dpms_transition();
        assert!((l.total_mj() - 210.0).abs() < 1e-6);
    }

    #[test]
    fn account_since_measures_wall_intervals() {
        let mut l = EnergyLedger::new();
        l.account_since(LedgerState::SelfRefresh, ms(100), ms(350));
        assert_eq!(l.ns_in(LedgerState::SelfRefresh), 250_000_000);
    }

    #[test]
    fn the_report_line_names_the_verdict() {
        let mut l = EnergyLedger::new();
        l.account(LedgerState::SelfRefresh, 1_000_000_000);
        l.psr_exit();
        l.add_wakes(4);
        let line = l.report_line();
        assert!(line.contains("static-ratio"), "{line}");
        assert!(line.contains("self-refresh 1.0s"), "{line}");
        assert!(line.contains("exits 1 (1 rescan)"), "{line}");
    }

    #[test]
    fn saturating_arithmetic_never_invents_energy() {
        let mut l = EnergyLedger::new();
        l.account_since(LedgerState::Scanout, ms(500), ms(100));
        assert_eq!(l.ns_in(LedgerState::Scanout), 0);
        l.add_wakes(u64::MAX);
        l.add_wakes(1);
        assert_eq!(l.wakes(), u64::MAX);
    }

    #[test]
    fn the_model_is_overridable() {
        let model = PowerModel {
            render_mw: [100.0, 200.0, 300.0, 400.0],
            scanout_mw: 100.0,
            selfrefresh_mw: 10.0,
            dimmed_mw: 50.0,
            blanked_mw: 1.0,
            psr_exit_mj: 1.0,
            dpms_transition_mj: 2.0,
            wake_mj: 0.5,
        };
        let mut l = EnergyLedger::with_model(model);
        l.account(LedgerState::SelfRefresh, 1_000_000_000);
        assert!((l.total_mj() - 10.0).abs() < 1e-6);
    }

    #[test]
    fn state_names_are_stable_for_reports() {
        assert_eq!(LedgerState::Render(PState::P2).name(), "render-p2");
        assert_eq!(LedgerState::SelfRefresh.name(), "self-refresh");
        assert_eq!(LedgerState::Blanked.name(), "blanked");
    }
}
