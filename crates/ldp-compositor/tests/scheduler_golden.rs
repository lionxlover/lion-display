//! Phase 7 exit criterion: simulated-timeline conformance (golden
//! schedules).
//!
//! Every scenario feeds a hand-built input stream through the pure
//! [`run`] driver and asserts the **exact** emission vector — deadlines,
//! target vblanks, budgets, refresh intervals, flags, reasons, and
//! ordering — pinning the scheduler's observable behavior to the
//! nanosecond. Any semantic change shows up here as a reviewable diff.

#[path = "sched/mod.rs"]
mod sched;

use ldp_compositor::replay::{run, SchedInput};
use ldp_compositor::scheduler::SchedulerConfig;
use ldp_compositor::SurfaceId;
use ldp_core::time::{FrameDropReason, Mono, PresentationMode, RefreshInterval};
use sched::{fd, ft, pr, COSTS, SIXTY};

fn sixty() -> RefreshInterval {
    RefreshInterval::from_ns(SIXTY).unwrap()
}

fn flip(ts: u64) -> SchedInput {
    SchedInput::Flip {
        ts: Mono::from_ns(ts),
    }
}

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

fn hide(surface: u64, hidden: bool, ts: u64) -> SchedInput {
    SchedInput::SetVisibility {
        surface: SurfaceId::from_raw(surface),
        hidden,
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

const V: PresentationMode = PresentationMode::Vsync;
const I: PresentationMode = PresentationMode::Immediate;
const A: PresentationMode = PresentationMode::Adaptive;

/// A client that requests right after the first flip, commits in the
/// window, and presents at the target vblank.
#[test]
fn golden_steady_state() {
    let inputs = vec![
        flip(SIXTY),
        req(1, 1, SIXTY + 1_000_000),
        commit(1, SIXTY + 8_000_000),
        flip(2 * SIXTY),
    ];
    let out = run(SchedulerConfig::default(), sixty(), &inputs).unwrap();
    assert_eq!(
        out,
        vec![
            ft(1, 1, 2 * SIXTY - COSTS, 2 * SIXTY, SIXTY, 14_666_666, V),
            pr(1, 1, 2 * SIXTY, SIXTY, false),
        ]
    );
}

/// A late commit drops the registration; the retargeted next
/// registration gets a widened (one-vblank) deadline.
#[test]
fn golden_miss_then_retarget() {
    let inputs = vec![
        flip(SIXTY),
        req(1, 1, 17_000_000),
        commit(1, 33_000_000), // past the deadline 32_333_332
        req(1, 2, 33_000_001),
        commit(1, 40_000_000),
        flip(49_999_998),
    ];
    let out = run(SchedulerConfig::default(), sixty(), &inputs).unwrap();
    assert_eq!(
        out,
        vec![
            ft(1, 1, 2 * SIXTY - COSTS, 2 * SIXTY, SIXTY, 15_333_332, V),
            fd(1, 1, FrameDropReason::DeadlineMissed),
            ft(1, 2, 3 * SIXTY - COSTS, 3 * SIXTY, SIXTY, 15_999_997, V),
            pr(1, 2, 49_999_998, SIXTY, false),
        ]
    );
}

/// The full escalation ladder: three misses step the target out
/// (1, 2, 3, 3 vblanks of lead), then presentations walk it back
/// (3, 2, 1) under one-hit de-escalation.
#[test]
fn golden_escalation_ladder() {
    let config = SchedulerConfig {
        escalate_after: 1,
        max_extra_lead: 2,
        deescalate_hits: 1,
        ..SchedulerConfig::default()
    };
    let ms = 1_000_000u64;
    let mut inputs = Vec::new();
    for k in 1..=16u64 {
        inputs.push(flip(k * SIXTY));
    }
    // Misses: requests only, no commits.
    inputs.push(req(1, 1, SIXTY + ms));
    inputs.push(req(1, 2, 2 * SIXTY + ms));
    inputs.push(req(1, 3, 4 * SIXTY + ms));
    // Hits from frame 4 on.
    inputs.push(req(1, 4, 7 * SIXTY + ms));
    inputs.push(commit(1, 7 * SIXTY + 2 * ms));
    inputs.push(req(1, 5, 10 * SIXTY + ms));
    inputs.push(commit(1, 10 * SIXTY + 2 * ms));
    inputs.push(req(1, 6, 13 * SIXTY + ms));
    inputs.push(commit(1, 13 * SIXTY + 2 * ms));
    inputs.push(req(1, 7, 15 * SIXTY + ms));
    inputs.push(commit(1, 15 * SIXTY + 2 * ms));
    inputs.sort_by_key(|i| match *i {
        SchedInput::Flip { ts }
        | SchedInput::FrameRequest { ts, .. }
        | SchedInput::Commit { ts, .. }
        | SchedInput::SetVisibility { ts, .. }
        | SchedInput::SetMode { ts, .. }
        | SchedInput::SetProfile { ts, .. }
        | SchedInput::Park { ts }
        | SchedInput::Resume { ts } => ts.as_ns(),
    });
    let out = run(config, sixty(), &inputs).unwrap();
    assert_eq!(
        out,
        vec![
            ft(1, 1, 2 * SIXTY - COSTS, 2 * SIXTY, SIXTY, 14_666_666, V),
            fd(1, 1, FrameDropReason::DeadlineMissed),
            ft(1, 2, 4 * SIXTY - COSTS, 4 * SIXTY, SIXTY, 31_333_332, V),
            fd(1, 2, FrameDropReason::DeadlineMissed),
            ft(1, 3, 7 * SIXTY - COSTS, 7 * SIXTY, SIXTY, 47_999_998, V),
            fd(1, 3, FrameDropReason::DeadlineMissed),
            ft(1, 4, 10 * SIXTY - COSTS, 10 * SIXTY, SIXTY, 47_999_998, V),
            pr(1, 4, 10 * SIXTY, SIXTY, false),
            ft(1, 5, 13 * SIXTY - COSTS, 13 * SIXTY, SIXTY, 47_999_998, V),
            pr(1, 5, 13 * SIXTY, SIXTY, false),
            ft(1, 6, 15 * SIXTY - COSTS, 15 * SIXTY, SIXTY, 31_333_332, V),
            pr(1, 6, 15 * SIXTY, SIXTY, false),
            ft(1, 7, 16 * SIXTY - COSTS, 16 * SIXTY, SIXTY, 14_666_666, V),
            pr(1, 7, 16 * SIXTY, SIXTY, false),
        ]
    );
}

/// A newer registration supersedes the live one; the content of the
/// superseded frame never presents.
#[test]
fn golden_supersede() {
    let inputs = vec![
        flip(SIXTY),
        req(1, 1, SIXTY + 1_000_000),
        req(1, 2, SIXTY + 2_000_000),
        commit(1, SIXTY + 3_000_000),
        flip(2 * SIXTY),
    ];
    let out = run(SchedulerConfig::default(), sixty(), &inputs).unwrap();
    assert_eq!(
        out,
        vec![
            ft(1, 1, 2 * SIXTY - COSTS, 2 * SIXTY, SIXTY, 14_666_666, V),
            fd(1, 1, FrameDropReason::Superseded),
            ft(1, 2, 2 * SIXTY - COSTS, 2 * SIXTY, SIXTY, 13_666_666, V),
            pr(1, 2, 2 * SIXTY, SIXTY, false),
        ]
    );
}

/// Hiding terminates with SurfaceHidden; Phase 45, the occlusion
/// quiescing contract: a request while hidden **parks** (no deadline
/// for a surface the server cannot show) and is answered at the
/// unhide — the budget then measured from the answer moment (one ms
/// later than the request, the honest remaining time), the same
/// target vblank, and the content presents once visible again.
#[test]
fn golden_hidden_then_visible() {
    let inputs = vec![
        flip(SIXTY),
        req(1, 1, SIXTY + 1_000_000),
        hide(1, true, SIXTY + 2_000_000),
        req(1, 2, SIXTY + 3_000_000),
        hide(1, false, SIXTY + 4_000_000),
        commit(1, SIXTY + 5_000_000),
        flip(2 * SIXTY),
    ];
    let out = run(SchedulerConfig::default(), sixty(), &inputs).unwrap();
    assert_eq!(
        out,
        vec![
            ft(1, 1, 2 * SIXTY - COSTS, 2 * SIXTY, SIXTY, 14_666_666, V),
            fd(1, 1, FrameDropReason::SurfaceHidden),
            ft(1, 2, 2 * SIXTY - COSTS, 2 * SIXTY, SIXTY, 11_666_666, V),
            pr(1, 2, 2 * SIXTY, SIXTY, false),
        ]
    );
}

/// Parking kills live registrations with OutputOff; requests while
/// parked defer their replies until resume.
#[test]
fn golden_park_resume() {
    let inputs = vec![
        flip(SIXTY),
        req(1, 1, SIXTY + 1_000_000),
        SchedInput::Park {
            ts: Mono::from_ns(SIXTY + 2_000_000),
        },
        req(1, 2, SIXTY + 3_000_000), // deferred while parked
        commit(1, SIXTY + 4_000_000), // satisfies the pending frame
        SchedInput::Resume {
            ts: Mono::from_ns(SIXTY + 5_000_000),
        },
        flip(2 * SIXTY),
    ];
    let out = run(SchedulerConfig::default(), sixty(), &inputs).unwrap();
    assert_eq!(
        out,
        vec![
            ft(1, 1, 2 * SIXTY - COSTS, 2 * SIXTY, SIXTY, 14_666_666, V),
            fd(1, 1, FrameDropReason::OutputOff),
            // The deferred reply is answered at resume, anchored on the
            // resumed timeline.
            ft(1, 2, 2 * SIXTY - COSTS, 2 * SIXTY, SIXTY, 10_666_666, V),
            pr(1, 2, 2 * SIXTY, SIXTY, false),
        ]
    );
}

/// A registration before the first flip defers until the timeline
/// anchors, then behaves normally.
#[test]
fn golden_unanchored_start() {
    let inputs = vec![
        req(1, 1, 1_000),
        commit(1, 2_000),
        flip(SIXTY),
        flip(2 * SIXTY),
    ];
    let out = run(SchedulerConfig::default(), sixty(), &inputs).unwrap();
    assert_eq!(
        out,
        vec![
            ft(1, 1, 2 * SIXTY - COSTS, 2 * SIXTY, SIXTY, SIXTY - COSTS, V),
            pr(1, 1, 2 * SIXTY, SIXTY, false),
        ]
    );
}

/// Depth-2 pipeline: the target is two vblanks out; intermediate flips
/// hold the content.
#[test]
fn golden_depth_two_pipeline() {
    let config = SchedulerConfig {
        vsync_depth: 2,
        ..SchedulerConfig::default()
    };
    let inputs = vec![
        flip(SIXTY),
        req(1, 1, SIXTY + 1_000_000),
        commit(1, SIXTY + 2_000_000),
        flip(2 * SIXTY),
        flip(3 * SIXTY),
    ];
    let out = run(config, sixty(), &inputs).unwrap();
    assert_eq!(
        out,
        vec![
            ft(1, 1, 3 * SIXTY - COSTS, 3 * SIXTY, SIXTY, 31_333_332, V),
            pr(1, 1, 3 * SIXTY, SIXTY, false),
        ]
    );
}

/// Immediate mode: the deadline is "now", commits can never miss, and
/// the presentation carries the torn flag.
#[test]
fn golden_immediate_torn() {
    let inputs = vec![
        flip(SIXTY),
        mode(1, I, SIXTY + 1_000_000),
        req(1, 1, SIXTY + 1_000_000),
        commit(1, SIXTY + 2_000_000),
        flip(2 * SIXTY),
    ];
    let out = run(SchedulerConfig::default(), sixty(), &inputs).unwrap();
    assert_eq!(
        out,
        vec![
            ft(1, 1, SIXTY + 1_000_000, 2 * SIXTY, SIXTY, 0, I),
            pr(1, 1, 2 * SIXTY, SIXTY, true),
        ]
    );
}

/// Adaptive mode: a commit past the plain deadline but inside the
/// widened window still makes the frame (it presents at the *next*
/// flip after the commit).
#[test]
fn golden_adaptive_window() {
    let config = SchedulerConfig {
        vrr_window_ns: 2_000_000,
        ..SchedulerConfig::default()
    };
    let inputs = vec![
        flip(SIXTY),
        mode(1, A, SIXTY + 1_000_000),
        req(1, 1, SIXTY + 1_000_000),
        flip(2 * SIXTY),                // window still open
        commit(1, 2 * SIXTY + 500_000), // inside the window
        flip(3 * SIXTY),
    ];
    let out = run(config, sixty(), &inputs).unwrap();
    assert_eq!(
        out,
        vec![
            ft(1, 1, 2 * SIXTY - COSTS, 2 * SIXTY, SIXTY, 14_666_666, A),
            pr(1, 1, 3 * SIXTY, SIXTY, false),
        ]
    );
}

/// Adaptive mode: a commit past the widened window is a deadline miss.
#[test]
fn golden_adaptive_expiry_bounded() {
    let config = SchedulerConfig {
        vrr_window_ns: 2_000_000,
        ..SchedulerConfig::default()
    };
    let inputs = vec![
        flip(SIXTY),
        mode(1, A, SIXTY + 1_000_000),
        req(1, 1, SIXTY + 1_000_000),
        commit(1, 2 * SIXTY + 2_000_000), // past deadline + window
    ];
    let out = run(config, sixty(), &inputs).unwrap();
    assert_eq!(
        out,
        vec![
            ft(1, 1, 2 * SIXTY - COSTS, 2 * SIXTY, SIXTY, 14_666_666, A),
            fd(1, 1, FrameDropReason::DeadlineMissed),
        ]
    );
}

/// A request late in the window retargets the next vblank when the
/// remaining budget is below the minimum lead.
#[test]
fn golden_min_lead_retarget() {
    let inputs = vec![
        flip(SIXTY),
        req(1, 1, 32_000_000), // only 333_332 ns of budget left
    ];
    let out = run(SchedulerConfig::default(), sixty(), &inputs).unwrap();
    assert_eq!(
        out,
        vec![ft(1, 1, 3 * SIXTY - COSTS, 3 * SIXTY, SIXTY, 16_999_998, V)]
    );
}

/// Two surfaces presenting at the same flip emit in surface order —
/// the cross-process-visible determinism contract.
#[test]
fn golden_multi_surface_order() {
    let inputs = vec![
        flip(SIXTY),
        req(1, 1, SIXTY + 1_000_000),
        req(2, 1, SIXTY + 1_000_001),
        commit(1, SIXTY + 2_000_000),
        commit(2, SIXTY + 2_000_000),
        flip(2 * SIXTY),
    ];
    let out = run(SchedulerConfig::default(), sixty(), &inputs).unwrap();
    assert_eq!(
        out,
        vec![
            ft(1, 1, 2 * SIXTY - COSTS, 2 * SIXTY, SIXTY, 14_666_666, V),
            ft(2, 1, 2 * SIXTY - COSTS, 2 * SIXTY, SIXTY, 14_666_665, V),
            pr(1, 1, 2 * SIXTY, SIXTY, false),
            pr(2, 1, 2 * SIXTY, SIXTY, false),
        ]
    );
}

/// Jittered flip reports (late, then early) still land on the right
/// vblank within the arrival slack; the measured refresh in the
/// feedback reflects the actual flip intervals.
#[test]
fn golden_jittered_flips_within_slack() {
    let flip_late = 2 * SIXTY + 100_000;
    let flip_early = 3 * SIXTY - 100_000;
    let inputs = vec![
        flip(SIXTY),
        req(1, 1, SIXTY + 1_000_000),
        commit(1, SIXTY + 2_000_000),
        flip(flip_late),
        req(1, 2, flip_late + 1_000_000),
        commit(1, flip_late + 2_000_000),
        flip(flip_early),
    ];
    let out = run(SchedulerConfig::default(), sixty(), &inputs).unwrap();
    assert_eq!(
        out,
        vec![
            ft(1, 1, 2 * SIXTY - COSTS, 2 * SIXTY, SIXTY, 14_666_666, V),
            pr(1, 1, flip_late, 16_766_666, false),
            // The PLL folded the late flip into a small correction; the
            // early report still counts as the target vblank (slack).
            ft(1, 2, 49_106_248, 50_106_248, 16_766_666, 14_672_916, V),
            pr(1, 2, flip_early, 16_466_666, false),
        ]
    );
}

/// Unregistered commits present nothing — state changes stay silent.
#[test]
fn golden_silent_content() {
    let inputs = vec![
        flip(SIXTY),
        commit(1, SIXTY + 2_000_000),
        flip(2 * SIXTY),
        flip(3 * SIXTY),
    ];
    let out = run(SchedulerConfig::default(), sixty(), &inputs).unwrap();
    assert_eq!(out, vec![]);
}
