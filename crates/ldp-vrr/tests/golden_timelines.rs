//! Phase 15 exit criterion 2 — scheduler + VRR golden timelines.
//!
//! Two layers here (the panel-integration third lives in
//! `panel_integration.rs`), each pinned to the nanosecond:
//!
//! 1. **Selector timelines** — commit streams at 144/90/60/40 Hz
//!    cadences through the [`RefreshSelector`]; exact rulings, plus the
//!    flip-slip invariants (≥ min apart, ≤ max stretch) over a
//!    randomized LCG corpus.
//! 2. **Scheduler + window goldens** — the deadline scheduler running
//!    with `vrr_window_ns` exactly as the engine installs it
//!    (`max − nominal` under deadline policy): a commit past the plain
//!    deadline but inside the window presents; past the widened window
//!    it misses; below-min-rate frames under the fixed-rate fallback
//!    surface as engine `Throttled` events with grid retry hints.

use ldp_compositor::scheduler::SchedulerConfig;
use ldp_compositor::{SchedEvent, SchedInput, SurfaceId};
use ldp_core::time::{Mono, PresentationMode, RefreshInterval};
use ldp_vrr::caps::{OutputVrrSupport, VrrCaps};
use ldp_vrr::engine::{VrrEngine, VrrEvent};
use ldp_vrr::policy::VrrPolicy;
use ldp_vrr::refresh::{CommitRuling, RefreshSelector};
use ldp_vrr::window::{select_flip, FlipSelection, RefreshWindow};

const MIN: u64 = 1_000_000_000 / 144; // 6.94 ms — 144 Hz floor
const MAX: u64 = 1_000_000_000 / 48; // 20.83 ms — 48 Hz stretch
const NOM: u64 = 16_666_666; // 60 Hz mode
const COSTS: u64 = 400_000 + 600_000; // submit + flip latency

fn window() -> RefreshWindow {
    RefreshWindow::from_ns(MIN, MAX).unwrap()
}

fn support() -> OutputVrrSupport {
    OutputVrrSupport::new(
        MIN,
        MAX,
        VrrCaps {
            seamless: true,
            fixed_rate: true,
        },
        NOM,
    )
    .unwrap()
}

fn sixty() -> RefreshInterval {
    RefreshInterval::from_ns(NOM).unwrap()
}

fn t(ns: u64) -> Mono {
    Mono::from_ns(ns)
}

// ---------------------------------------------------------------------------
// 1. Selector timelines
// ---------------------------------------------------------------------------

#[test]
fn selector_144hz_cadence_clamps_to_the_floor() {
    // Content ready every 5 ms (200 Hz): every flip clamps onto the
    // min-interval grid — slip avoidance turns 5 ms readiness into a
    // 6.94 ms flip cadence.
    let mut s = RefreshSelector::new(window(), NOM, true, VrrPolicy::Deadline, true);
    s.observe_flip(t(0));
    let mut last = 0u64;
    for k in 1..=6u64 {
        let ready = k * 5_000_000;
        match s.commit_ready(t(ready)) {
            CommitRuling::FlipAt(at) => {
                assert_eq!(at.as_ns(), last + MIN, "flip {k} sits on the floor grid");
                last = at.as_ns();
                s.observe_flip(at);
            }
            other => panic!("144 Hz cadence cannot miss: {other:?}"),
        }
    }
    assert_eq!(last, 6 * MIN);
}

#[test]
fn selector_90hz_cadence_flips_at_ready() {
    // Content ready every 11.11 ms (90 Hz): inside the window, past the
    // floor — every flip lands exactly at readiness.
    let mut s = RefreshSelector::new(window(), NOM, true, VrrPolicy::Deadline, true);
    s.observe_flip(t(0));
    for k in 1..=6u64 {
        let ready = k * 11_111_111;
        match s.commit_ready(t(ready)) {
            CommitRuling::FlipAt(at) => {
                assert_eq!(at.as_ns(), ready, "flip {k} lands at ready");
                s.observe_flip(at);
            }
            other => panic!("90 Hz cadence cannot miss: {other:?}"),
        }
    }
}

#[test]
fn selector_40hz_cadence_defers_on_fixed_rate() {
    // Content ready every 25 ms (40 Hz): every commit misses the
    // 20.83 ms window; with fixed_rate the deadline policy defers each
    // onto the nominal grid.
    let mut s = RefreshSelector::new(window(), NOM, true, VrrPolicy::Deadline, true);
    s.observe_flip(t(0));
    for k in 1..=4u64 {
        let ready = k * 25_000_000;
        match s.commit_ready(t(ready)) {
            CommitRuling::Defer { retry_at } => {
                // Next 60 Hz grid point at-or-after ready.
                let periods = ready.div_ceil(NOM);
                assert_eq!(retry_at.as_ns(), periods * NOM, "defer {k}");
            }
            other => panic!("40 Hz on fixed_rate must defer: {other:?}"),
        }
    }
}

#[test]
fn selector_40hz_without_fixed_rate_catches_up() {
    let mut s = RefreshSelector::new(window(), NOM, false, VrrPolicy::Deadline, true);
    s.observe_flip(t(0));
    for k in 1..=3u64 {
        let ready = k * 25_000_000;
        assert_eq!(
            s.commit_ready(t(ready)),
            CommitRuling::Late(t(ready)),
            "no fixed_rate: catch up at readiness"
        );
    }
}

#[test]
fn selector_flip_slip_invariants_over_randomized_cadences() {
    // LCG corpus (the no-rand doctrine): random ready times; every
    // applied FlipAt/Late ruling respects the window against the
    // anchor it was computed from, and consecutive completed flips are
    // at least MIN apart.
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            self.0 >> 16
        }
    }
    let mut rng = Lcg(0x0F_15);
    for _case in 0..64 {
        let mut s = RefreshSelector::new(window(), NOM, true, VrrPolicy::Deadline, true);
        let mut last = 1 + rng.next() % 1_000_000;
        s.observe_flip(t(last));
        for _step in 0..48 {
            // Readiness anywhere from 1 ms to 30 ms past the last flip.
            let ready = last + 1_000_000 + rng.next() % 29_000_000;
            match s.commit_ready(t(ready)) {
                CommitRuling::FlipAt(at) | CommitRuling::Late(at) => {
                    assert!(at.as_ns() >= last + MIN, "slip floor violated");
                    assert!(at.as_ns() <= last + MAX, "stretch ceiling violated");
                    assert!(
                        at.as_ns() >= ready.min(last + MAX),
                        "content delayed illegally"
                    );
                    last = at.as_ns();
                    s.observe_flip(at);
                }
                CommitRuling::Defer { retry_at } => {
                    assert!(retry_at.as_ns() > ready, "retry must be later");
                    assert!(
                        retry_at.as_ns() % NOM == last % NOM,
                        "retry sits on the grid"
                    );
                }
            }
        }
    }
}

#[test]
fn selection_misses_only_past_expiry() {
    // The pure boundary: At at last+max exactly, Missed one ns later.
    let w = window();
    assert!(matches!(
        select_flip(t(1_000_000), t(1_000_000 + MAX), &w),
        FlipSelection::At(_)
    ));
    assert_eq!(
        select_flip(t(1_000_000), t(1_000_000 + MAX + 1), &w),
        FlipSelection::Missed
    );
}

// ---------------------------------------------------------------------------
// 2. Scheduler + window goldens
// ---------------------------------------------------------------------------

fn req(surface: u64, frame: u64, ts: u64) -> SchedInput {
    SchedInput::FrameRequest {
        surface: SurfaceId::from_raw(surface),
        frame,
        ts: Mono::from_ns(ts),
    }
}

fn commit(surface: u64, ts: u64) -> SchedInput {
    SchedInput::Commit {
        surface: SurfaceId::from_raw(surface),
        ts: Mono::from_ns(ts),
    }
}

fn flip(ts: u64) -> SchedInput {
    SchedInput::Flip {
        ts: Mono::from_ns(ts),
    }
}

fn mode(surface: u64, m: PresentationMode, ts: u64) -> SchedInput {
    SchedInput::SetMode {
        surface: SurfaceId::from_raw(surface),
        mode: m,
        ts: Mono::from_ns(ts),
    }
}

/// The engine-built scheduler config for a deadline-policy output with
/// adaptive demand (widening = MAX − NOM = 4_166_667).
fn windowed_config() -> SchedulerConfig {
    let mut e = VrrEngine::new(
        Some(support()),
        VrrPolicy::Deadline,
        SchedulerConfig::default(),
    );
    e.set_adaptive_demand(true);
    let cfg = e.scheduler_config();
    assert_eq!(cfg.vrr_window_ns, MAX - NOM);
    cfg
}

#[test]
fn scheduler_window_late_commit_still_presents() {
    // Anchored at a flip on the 60 Hz grid, an adaptive registration
    // targets vblank 2*NOM; a commit 2 ms past the plain deadline but
    // 2.16 ms inside the widened window satisfies — and under VRR the
    // panel stretch lets the flip land at the commit itself, so the
    // presentation reports at that flip: the window-late save.
    let cfg = windowed_config();
    let late = 2 * NOM - COSTS + 2_000_000; // inside the window
    assert!(late < 2 * NOM - COSTS + cfg.vrr_window_ns);
    let inputs = [
        flip(NOM),
        mode(1, PresentationMode::Adaptive, NOM + 1),
        req(1, 7, NOM + 1),
        commit(1, late),
        flip(late), // the stretched panel flip
    ];
    let events = ldp_compositor::replay::run(cfg, sixty(), &inputs).unwrap();
    let presented: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, SchedEvent::Presented { .. }))
        .collect();
    assert_eq!(presented.len(), 1, "the window-saved frame presents");
    match presented[0] {
        SchedEvent::Presented { timing, .. } => {
            assert_eq!(timing.frame, 7);
            assert_eq!(timing.presented_at.as_ns(), late);
            assert!(timing.flags.vblank);
            assert!(!timing.flags.torn, "VRR presentations are tear-free");
        }
        _ => unreachable!("filtered to Presented"),
    }
}

#[test]
fn scheduler_commit_past_the_widened_window_misses() {
    let cfg = windowed_config();
    // Deadline 2*NOM - COSTS ≈ 32_332_332; widened expiry adds
    // 4_166_667 → ≈ 36_498_999. A commit at 36_500_000 is past it.
    let deadline = 2 * NOM - COSTS;
    let expiry = deadline + cfg.vrr_window_ns;
    let inputs = [
        flip(NOM),
        mode(1, PresentationMode::Adaptive, NOM + 1),
        req(1, 8, NOM + 1),
        commit(1, expiry + 1),
        flip(expiry + 1),
    ];
    let events = ldp_compositor::replay::run(cfg, sixty(), &inputs).unwrap();
    let dropped: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            SchedEvent::FrameDropped { frame, reason, .. } => Some((*frame, *reason)),
            _ => None,
        })
        .collect();
    assert_eq!(
        dropped,
        vec![(8, ldp_core::time::FrameDropReason::DeadlineMissed)],
        "past the window the miss escalates"
    );
}

#[test]
fn scheduler_widening_boundary_is_exact() {
    let cfg = windowed_config();
    let deadline = 2 * NOM - COSTS;
    // Exactly at the widened expiry: still inside (the contract uses
    // strict-before; one ns past drops).
    let inputs = [
        flip(NOM),
        mode(1, PresentationMode::Adaptive, NOM + 1),
        req(1, 9, NOM + 1),
        commit(1, deadline + cfg.vrr_window_ns - 1),
        flip(deadline + cfg.vrr_window_ns),
    ];
    let events = ldp_compositor::replay::run(cfg, sixty(), &inputs).unwrap();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, SchedEvent::Presented { timing, .. } if timing.frame == 9)),
        "one ns before expiry still presents"
    );
    let inputs = [
        flip(NOM),
        mode(1, PresentationMode::Adaptive, NOM + 1),
        req(1, 10, NOM + 1),
        commit(1, deadline + cfg.vrr_window_ns),
        flip(deadline + cfg.vrr_window_ns + 1),
    ];
    let events = ldp_compositor::replay::run(cfg, sixty(), &inputs).unwrap();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, SchedEvent::FrameDropped { frame: 10, .. })),
        "at the expiry itself the miss fires"
    );
}

#[test]
fn always_policy_widening_spans_the_whole_window() {
    let mut e = VrrEngine::new(
        Some(support()),
        VrrPolicy::Always,
        SchedulerConfig::default(),
    );
    e.set_adaptive_demand(false); // the policy is the demand
    let cfg = e.scheduler_config();
    assert_eq!(cfg.vrr_window_ns, MAX - MIN);
    // A commit a full min-period past the deadline still presents.
    let deadline = 2 * NOM - COSTS;
    let late = deadline + MIN;
    let inputs = [
        flip(NOM),
        mode(1, PresentationMode::Adaptive, NOM + 1),
        req(1, 11, NOM + 1),
        commit(1, late),
        flip(late),
    ];
    let events = ldp_compositor::replay::run(cfg, sixty(), &inputs).unwrap();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, SchedEvent::Presented { timing, .. } if timing.frame == 11)),
        "full-span widening keeps in-cadence commits on time"
    );
}

#[test]
fn off_policy_widening_is_zero_and_unchanged() {
    // With VRR decided off the config passes through untouched: the
    // pre-Phase-15 scheduler behavior, byte for byte.
    let mut e = VrrEngine::new(Some(support()), VrrPolicy::Off, SchedulerConfig::default());
    e.set_adaptive_demand(true);
    assert_eq!(e.scheduler_config().vrr_window_ns, 0);
    let base = SchedulerConfig::default();
    let inputs = [
        flip(NOM),
        mode(1, PresentationMode::Adaptive, NOM + 1),
        req(1, 12, NOM + 1),
        commit(1, 2 * NOM - COSTS + 1), // one ns past the plain deadline
        flip(2 * NOM),
    ];
    let a = ldp_compositor::replay::run(e.scheduler_config(), sixty(), &inputs).unwrap();
    let b = ldp_compositor::replay::run(base, sixty(), &inputs).unwrap();
    assert_eq!(a, b, "no window, no behavior change");
}

#[test]
fn engine_throttled_events_carry_grid_retry_hints() {
    // The reserved drop reason surfaces through the engine: below-min
    // cadence with fixed_rate defers with the retry grid point.
    let mut e = VrrEngine::new(
        Some(support()),
        VrrPolicy::Deadline,
        SchedulerConfig::default(),
    );
    e.set_adaptive_demand(true);
    e.drain();
    let surface = SurfaceId::from_raw(3);
    e.observe_flip(t(0));
    let ruling = e.commit_ready(surface, 21, t(25_000_000));
    assert!(matches!(ruling, CommitRuling::Defer { .. }));
    assert_eq!(
        e.drain(),
        vec![VrrEvent::Throttled {
            surface,
            frame: 21,
            retry_at: t(2 * NOM) // ceil(25ms / 16.67ms) = 2 grid periods
        }]
    );
    // Without fixed_rate (same panel, caps stripped) the same cadence
    // catches up instead of throttling.
    let bare = OutputVrrSupport::new(
        MIN,
        MAX,
        VrrCaps {
            seamless: true,
            fixed_rate: false,
        },
        NOM,
    )
    .unwrap();
    let mut e = VrrEngine::new(Some(bare), VrrPolicy::Deadline, SchedulerConfig::default());
    e.set_adaptive_demand(true);
    e.drain();
    e.observe_flip(t(0));
    assert_eq!(
        e.commit_ready(surface, 22, t(25_000_000)),
        CommitRuling::Late(t(25_000_000))
    );
    assert!(e.drain().is_empty(), "no deferral, no Throttled event");
}

#[test]
fn lfc_repeat_rides_the_minimum_interval_grid() {
    // Phase 31's low-framerate compensation: with no content ready,
    // the display side repeats the front buffer at the next
    // `last + k*min` — the earliest in-window grid point. Content
    // slower than the window's minimum never drops the panel out of
    // adaptive sync.
    use ldp_core::time::Mono;
    use ldp_vrr::caps::{OutputVrrSupport, PanelWindow, VrrCaps};
    use ldp_vrr::policy::{PolicyInputs, VrrPolicy};
    use ldp_vrr::refresh::{CommitRuling, RefreshSelector};
    use ldp_vrr::window::RefreshWindow;

    // A 48-144 Hz panel (the reference preset's window).
    let window = RefreshWindow::from_panel(
        PanelWindow::from_millihz(48_000, 144_000).expect("valid window"),
    );
    let nominal_ns = 1_000_000_000_000 / 60_000; // 60 Hz nominal
    let support = OutputVrrSupport::new(
        window.min_ns(),
        window.max_ns(),
        VrrCaps::default(),
        nominal_ns,
    )
    .expect("nominal inside the window");
    let inputs = PolicyInputs {
        policy: VrrPolicy::Deadline,
        support: Some(&support),
        adaptive_demand: true,
        battery_saver: false,
    };
    let decision = ldp_vrr::policy::decide(&inputs);
    assert!(decision.vrr_enabled, "the deadline policy engages VRR");

    let mut selector = RefreshSelector::new(
        window,
        nominal_ns,
        true,
        VrrPolicy::Deadline,
        decision.vrr_enabled,
    );
    let t0 = Mono::from_ns(1_000_000);
    selector.observe_flip(t0);

    // The repeat grid: 144 Hz minimum interval = ~6.94 ms. From
    // t0 = 1 ms, the next grid multiple is ceil(1/6.944)*6.944 ms.
    let min_ns = window.min_ns();
    let repeat = selector.repeat_at();
    let expect = t0.as_ns().div_ceil(min_ns) * min_ns;
    assert_eq!(repeat.as_ns(), expect, "the earliest in-window repeat");
    assert!(repeat.as_ns() > t0.as_ns(), "strictly after the last flip");

    // The repeat is a floor, never a block: content arriving past
    // the panel's minimum interval takes its own ruling (it flips at
    // readiness, exactly when it would have without the repeat
    // policy); content arriving *inside* the floor is clamped by the
    // panel's physics — the earliest legal re-trigger — which is the
    // repeat grid's own answer.
    let content_ready = Mono::from_ns(t0.as_ns() + 8_000_000);
    match selector.commit_ready(content_ready) {
        CommitRuling::FlipAt(at) => assert_eq!(at, content_ready, "content wins past the floor"),
        other => panic!("content past the floor must flip at readiness: {other:?}"),
    }
    // Inside the floor: the earliest legal instant (the panel cannot
    // re-trigger sooner than min after the last flip).
    let mut selector2 = RefreshSelector::new(
        window,
        nominal_ns,
        true,
        VrrPolicy::Deadline,
        decision.vrr_enabled,
    );
    selector2.observe_flip(t0);
    let early = Mono::from_ns(t0.as_ns() + 2_000_000);
    match selector2.commit_ready(early) {
        CommitRuling::FlipAt(at) => {
            assert!(
                at.as_ns() >= t0.as_ns() + min_ns,
                "clamped to the panel's floor"
            );
        }
        other => panic!("early content takes the clamped flip: {other:?}"),
    }
}
