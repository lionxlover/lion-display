//! Scheduler contract tests at the drain-API level.
//!
//! These exercise the [`FrameScheduler`] state machine directly (feed one
//! input, drain, assert) — the golden suite pins whole-session event
//! vectors through the pure `run` driver; this file keeps the
//! step-by-step view of the same contract, including paths the golden
//! streams cannot show (mid-session drains, expiry checkpoints firing
//! at commit vs. at flip).

use ldp_compositor::scheduler::{FrameScheduler, SchedEvent, SchedulerConfig};
use ldp_compositor::SurfaceId;
use ldp_core::time::{FrameDropReason, Mono, PresentationMode, RefreshInterval};

const SIXTY: u64 = 16_666_666;

fn sixty() -> RefreshInterval {
    RefreshInterval::from_ns(SIXTY).unwrap()
}

fn live_scheduler() -> FrameScheduler {
    let mut s = FrameScheduler::new(sixty(), SchedulerConfig::default()).unwrap();
    s.observe_flip(Mono::from_ns(SIXTY));
    s.drain();
    s
}

#[test]
fn inputs_must_be_ordered() {
    let mut s = live_scheduler();
    s.commit(SurfaceId::from_raw(1), Mono::from_ns(20_000_000));
    let hook = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        s.commit(SurfaceId::from_raw(1), Mono::from_ns(19_999_999));
    }));
    assert!(hook.is_err(), "out-of-order input must panic");
}

#[test]
fn frame_request_before_first_flip_defers_reply() {
    let mut s = FrameScheduler::new(sixty(), SchedulerConfig::default()).unwrap();
    s.frame_request(SurfaceId::from_raw(1), 7, Mono::from_ns(1_000));
    assert!(s.drain().is_empty());
    // First flip answers the deferred registration.
    s.observe_flip(Mono::from_ns(SIXTY));
    let events = s.drain();
    assert_eq!(events.len(), 1);
    match events[0] {
        SchedEvent::FrameTarget {
            frame, deadline, ..
        } => {
            assert_eq!(frame, 7);
            assert_eq!(deadline.target_vblank.as_ns(), 2 * SIXTY);
            assert_eq!(deadline.deadline.as_ns(), 2 * SIXTY - 1_000_000);
            assert_eq!(deadline.budget_ns, 2 * SIXTY - 1_000_000 - SIXTY);
            assert_eq!(deadline.mode, PresentationMode::Vsync);
        }
        ref other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn steady_frame_hits_target() {
    let mut s = live_scheduler();
    let surface = SurfaceId::from_raw(1);
    s.frame_request(surface, 1, Mono::from_ns(17_000_000));
    let target = match s.drain().remove(0) {
        SchedEvent::FrameTarget { deadline, .. } => deadline,
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(target.target_vblank.as_ns(), 2 * SIXTY);
    // Commit inside the window satisfies.
    s.commit(surface, Mono::from_ns(31_000_000));
    assert!(s.drain().is_empty());
    // Flip at the target vblank presents.
    s.observe_flip(Mono::from_ns(2 * SIXTY));
    let events = s.drain();
    assert_eq!(events.len(), 1);
    match events[0] {
        SchedEvent::Presented { timing, .. } => {
            assert_eq!(timing.frame, 1);
            assert_eq!(timing.presented_at.as_ns(), 2 * SIXTY);
            assert!(timing.flags.vblank);
            assert_eq!(timing.refresh.as_ns(), SIXTY);
        }
        ref other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn late_commit_drops_and_carries() {
    let mut s = live_scheduler();
    let surface = SurfaceId::from_raw(1);
    s.frame_request(surface, 3, Mono::from_ns(17_000_000));
    s.drain();
    // Deadline is 2*SIXTY - 1_000_000; commit after it.
    s.commit(surface, Mono::from_ns(2 * SIXTY - 500_000));
    assert_eq!(
        s.drain(),
        vec![SchedEvent::FrameDropped {
            surface,
            frame: 3,
            reason: FrameDropReason::DeadlineMissed,
        }]
    );
    // The flip presents nothing (the late content is silent state).
    s.observe_flip(Mono::from_ns(2 * SIXTY));
    assert!(s.drain().is_empty());
}

#[test]
fn newer_registration_supersedes() {
    let mut s = live_scheduler();
    let surface = SurfaceId::from_raw(1);
    s.frame_request(surface, 1, Mono::from_ns(17_000_000));
    s.drain();
    s.frame_request(surface, 2, Mono::from_ns(18_000_000));
    let events = s.drain();
    assert!(events.contains(&SchedEvent::FrameDropped {
        surface,
        frame: 1,
        reason: FrameDropReason::Superseded,
    }));
    assert_eq!(events.len(), 2); // supersede drop + new target
}

#[test]
fn hidden_surface_uses_surface_hidden_reason() {
    let mut s = live_scheduler();
    let surface = SurfaceId::from_raw(1);
    s.frame_request(surface, 5, Mono::from_ns(17_000_000));
    s.drain();
    s.set_visibility(surface, true, Mono::from_ns(18_000_000));
    assert_eq!(
        s.drain(),
        vec![SchedEvent::FrameDropped {
            surface,
            frame: 5,
            reason: FrameDropReason::SurfaceHidden,
        }]
    );
    // Phase 45, the occlusion quiescing contract: a request while
    // hidden parks — no deadline is handed to a surface the server
    // cannot show — and the parking survives flips (the timeline
    // being live says nothing about the surface's visibility).
    s.frame_request(surface, 6, Mono::from_ns(19_000_000));
    assert!(s.drain().is_empty());
    s.observe_flip(Mono::from_ns(2 * SIXTY));
    assert!(s.drain().is_empty());
    // The unhide is the answer point: the parked request draws its
    // deadline at the moment the surface can present again.
    s.set_visibility(surface, false, Mono::from_ns(2 * SIXTY + 100));
    let events = s.drain();
    assert_eq!(events.len(), 1);
    match events[0] {
        SchedEvent::FrameTarget { frame, .. } => assert_eq!(frame, 6),
        ref other => panic!("unexpected {other:?}"),
    }
    // The answered registration behaves like any live one: commit
    // inside the window, flip at the target, frame 6 presents.
    s.commit(surface, Mono::from_ns(2 * SIXTY + 200));
    s.observe_flip(Mono::from_ns(4 * SIXTY));
    let events = s.drain();
    assert_eq!(events.len(), 1);
    match events[0] {
        SchedEvent::Presented { timing, .. } => assert_eq!(timing.frame, 6),
        ref other => panic!("unexpected {other:?}"),
    }
    // Committing while hidden also terminates with SurfaceHidden.
    s.set_visibility(surface, true, Mono::from_ns(4 * SIXTY + 100));
    assert!(s.drain().is_empty());
    s.frame_request(surface, 7, Mono::from_ns(4 * SIXTY + 200));
    assert!(s.drain().is_empty());
    s.commit(surface, Mono::from_ns(4 * SIXTY + 300));
    assert_eq!(
        s.drain(),
        vec![SchedEvent::FrameDropped {
            surface,
            frame: 7,
            reason: FrameDropReason::SurfaceHidden,
        }]
    );
}

/// Phase 45's addition to the contract: a surface that never showed
/// (request arriving already-hidden) parks just the same — and a
/// *later* flip does not answer it, only the unhide does.
#[test]
fn request_arriving_hidden_parks_until_unhide() {
    let mut s = live_scheduler();
    let surface = SurfaceId::from_raw(2);
    s.set_visibility(surface, true, Mono::from_ns(17_000_000));
    s.frame_request(surface, 9, Mono::from_ns(17_500_000));
    assert!(s.drain().is_empty(), "no target while hidden");
    // Two flips pass: still parked (the quiesced surface costs
    // nothing — no deadline, no expiry, no presentation).
    s.observe_flip(Mono::from_ns(2 * SIXTY));
    s.observe_flip(Mono::from_ns(3 * SIXTY));
    assert!(s.drain().is_empty(), "flips do not answer hidden requests");
    s.set_visibility(surface, false, Mono::from_ns(3 * SIXTY + 5));
    let events = s.drain();
    assert_eq!(events.len(), 1);
    match events[0] {
        SchedEvent::FrameTarget {
            frame, deadline, ..
        } => {
            assert_eq!(frame, 9);
            assert_eq!(deadline.target_vblank.as_ns(), 4 * SIXTY);
        }
        ref other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn park_drops_everything_output_off() {
    let mut s = live_scheduler();
    let a = SurfaceId::from_raw(1);
    let b = SurfaceId::from_raw(2);
    s.frame_request(a, 1, Mono::from_ns(17_000_000));
    s.frame_request(b, 2, Mono::from_ns(17_000_000));
    s.drain();
    s.park(Mono::from_ns(20_000_000));
    let mut events = s.drain();
    events.sort_by_key(|event| match *event {
        SchedEvent::FrameDropped { surface, .. } => surface,
        _ => SurfaceId::from_raw(0),
    });
    assert_eq!(events.len(), 2);
    for event in events {
        match event {
            SchedEvent::FrameDropped { reason, .. } => {
                assert_eq!(reason, FrameDropReason::OutputOff);
            }
            ref other => panic!("unexpected {other:?}"),
        }
    }
    // While parked, requests defer their replies.
    s.frame_request(a, 9, Mono::from_ns(21_000_000));
    assert!(s.drain().is_empty());
    s.resume(Mono::from_ns(22_000_000));
    let events = s.drain();
    assert_eq!(events.len(), 1);
    assert!(matches!(
        events[0],
        SchedEvent::FrameTarget { frame: 9, .. }
    ));
}

#[test]
fn commit_while_unanchored_satisfies_pending() {
    let mut s = FrameScheduler::new(sixty(), SchedulerConfig::default()).unwrap();
    s.frame_request(SurfaceId::from_raw(1), 4, Mono::from_ns(1_000));
    s.commit(SurfaceId::from_raw(1), Mono::from_ns(2_000));
    s.observe_flip(Mono::from_ns(SIXTY));
    let events = s.drain();
    // Answered target + (not yet) presented: target is the second
    // vblank, so this flip only answers.
    assert_eq!(events.len(), 1);
    assert!(matches!(
        events[0],
        SchedEvent::FrameTarget { frame: 4, .. }
    ));
    s.observe_flip(Mono::from_ns(2 * SIXTY));
    let events = s.drain();
    assert_eq!(events.len(), 1);
    assert!(matches!(events[0], SchedEvent::Presented { .. }));
}

#[test]
fn immediate_mode_never_misses_and_marks_torn() {
    let mut s = live_scheduler();
    let surface = SurfaceId::from_raw(1);
    s.set_mode(
        surface,
        PresentationMode::Immediate,
        Mono::from_ns(17_000_000),
    );
    s.frame_request(surface, 1, Mono::from_ns(17_000_000));
    let target = match s.drain().remove(0) {
        SchedEvent::FrameTarget { deadline, .. } => deadline,
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(target.budget_ns, 0);
    assert_eq!(target.deadline.as_ns(), 17_000_000);
    assert_eq!(target.mode, PresentationMode::Immediate);
    // Even an absurdly late commit satisfies immediate mode.
    s.commit(surface, Mono::from_ns(30_000_000));
    assert!(s.drain().is_empty());
    s.observe_flip(Mono::from_ns(2 * SIXTY));
    let events = s.drain();
    assert_eq!(events.len(), 1);
    match events[0] {
        SchedEvent::Presented { timing, .. } => {
            assert!(timing.flags.torn);
            assert!(!timing.flags.vblank);
        }
        ref other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn adaptive_window_absorbs_lateness() {
    let config = SchedulerConfig {
        vrr_window_ns: 2_000_000,
        ..SchedulerConfig::default()
    };
    let mut s = FrameScheduler::new(sixty(), config).unwrap();
    s.observe_flip(Mono::from_ns(SIXTY));
    s.drain();
    let surface = SurfaceId::from_raw(1);
    s.set_mode(
        surface,
        PresentationMode::Adaptive,
        Mono::from_ns(17_000_000),
    );
    s.frame_request(surface, 1, Mono::from_ns(17_000_000));
    s.drain();
    // Commit past the plain deadline but inside the widened window.
    s.commit(surface, Mono::from_ns(2 * SIXTY - 333_332));
    assert!(s.drain().is_empty());
    s.observe_flip(Mono::from_ns(2 * SIXTY));
    assert_eq!(s.drain().len(), 1);
}

#[test]
fn adaptive_window_expiry_still_bounded() {
    let config = SchedulerConfig {
        vrr_window_ns: 2_000_000,
        ..SchedulerConfig::default()
    };
    let mut s = FrameScheduler::new(sixty(), config).unwrap();
    s.observe_flip(Mono::from_ns(SIXTY));
    s.drain();
    let surface = SurfaceId::from_raw(1);
    s.set_mode(
        surface,
        PresentationMode::Adaptive,
        Mono::from_ns(17_000_000),
    );
    s.frame_request(surface, 1, Mono::from_ns(17_000_000));
    s.drain();
    // The widened window (deadline + 2 ms) is still open at the
    // target vblank: the flip must not present (nothing satisfied)
    // nor expire it.
    s.observe_flip(Mono::from_ns(2 * SIXTY));
    assert!(s.drain().is_empty());
    // Past the widened window: the expiry checkpoint at the commit
    // drops the registration as a deadline miss.
    s.commit(surface, Mono::from_ns(2 * SIXTY + 1_200_000));
    assert_eq!(
        s.drain(),
        vec![SchedEvent::FrameDropped {
            surface,
            frame: 1,
            reason: FrameDropReason::DeadlineMissed,
        }]
    );
}

#[test]
fn adaptive_commit_after_flip_still_presents_next() {
    let config = SchedulerConfig {
        vrr_window_ns: 2_000_000,
        ..SchedulerConfig::default()
    };
    let mut s = FrameScheduler::new(sixty(), config).unwrap();
    s.observe_flip(Mono::from_ns(SIXTY));
    s.drain();
    let surface = SurfaceId::from_raw(1);
    s.set_mode(
        surface,
        PresentationMode::Adaptive,
        Mono::from_ns(17_000_000),
    );
    s.frame_request(surface, 1, Mono::from_ns(17_000_000));
    s.drain();
    // Satisfying commit lands after the target flip but inside the
    // window (the panel could have waited): presents at the NEXT flip.
    s.observe_flip(Mono::from_ns(2 * SIXTY));
    assert!(s.drain().is_empty());
    s.commit(surface, Mono::from_ns(2 * SIXTY + 500_000));
    assert!(s.drain().is_empty());
    s.observe_flip(Mono::from_ns(3 * SIXTY));
    assert_eq!(s.drain().len(), 1);
}

#[test]
fn miss_escalation_widens_deadlines() {
    let config = SchedulerConfig {
        escalate_after: 1,
        max_extra_lead: 2,
        deescalate_hits: 1,
        ..SchedulerConfig::default()
    };
    let mut s = FrameScheduler::new(sixty(), config).unwrap();
    s.observe_flip(Mono::from_ns(SIXTY));
    s.drain();
    let surface = SurfaceId::from_raw(1);
    // Frame 1: lead 0 miss -> streak 1.
    s.frame_request(surface, 1, Mono::from_ns(17_000_000));
    s.drain();
    s.observe_flip(Mono::from_ns(2 * SIXTY));
    assert_eq!(
        s.drain(),
        vec![SchedEvent::FrameDropped {
            surface,
            frame: 1,
            reason: FrameDropReason::DeadlineMissed,
        }]
    );
    // Frame 2: lead 1 (streak 1) targets two vblanks ahead.
    s.frame_request(surface, 2, Mono::from_ns(2 * SIXTY + 333_334));
    let deadline = match s.drain().remove(0) {
        SchedEvent::FrameTarget { deadline, .. } => deadline,
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(deadline.target_vblank.as_ns(), 4 * SIXTY);
    // Commit in time (deadline is 4*SIXTY - 1 ms), flip presents,
    // streak decays to 0.
    s.commit(surface, Mono::from_ns(40_000_000));
    s.drain();
    s.observe_flip(Mono::from_ns(3 * SIXTY)); // hold: not due yet
    assert!(s.drain().is_empty());
    s.observe_flip(Mono::from_ns(4 * SIXTY));
    assert_eq!(s.drain().len(), 1);
    // Frame 3: back to lead 0.
    s.frame_request(surface, 3, Mono::from_ns(4 * SIXTY + 1));
    let deadline = match s.drain().remove(0) {
        SchedEvent::FrameTarget { deadline, .. } => deadline,
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(deadline.target_vblank.as_ns(), 5 * SIXTY);
}

#[test]
fn min_lead_retargets_next_vblank() {
    let config = SchedulerConfig {
        min_commit_lead_ns: 500_000,
        ..SchedulerConfig::default()
    };
    let mut s = FrameScheduler::new(sixty(), config).unwrap();
    s.observe_flip(Mono::from_ns(SIXTY));
    s.drain();
    let surface = SurfaceId::from_raw(1);
    // So late in the window that the remaining budget (333_332 ns)
    // is below the 500_000 ns lead floor: retarget one vblank out.
    s.frame_request(surface, 1, Mono::from_ns(32_000_000));
    let deadline = match s.drain().remove(0) {
        SchedEvent::FrameTarget { deadline, .. } => deadline,
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(deadline.target_vblank.as_ns(), 3 * SIXTY);
    assert_eq!(deadline.budget_ns, 3 * SIXTY - 1_000_000 - 32_000_000);
}

#[test]
fn presentation_silent_for_unregistered_content() {
    let mut s = live_scheduler();
    let surface = SurfaceId::from_raw(1);
    // Commit without any registration: flips silently.
    s.commit(surface, Mono::from_ns(20_000_000));
    s.observe_flip(Mono::from_ns(2 * SIXTY));
    assert!(s.drain().is_empty());
}

#[test]
fn deeper_pipeline_holds_until_due() {
    let config = SchedulerConfig {
        vsync_depth: 2,
        ..SchedulerConfig::default()
    };
    let mut s = FrameScheduler::new(sixty(), config).unwrap();
    s.observe_flip(Mono::from_ns(SIXTY));
    s.drain();
    let surface = SurfaceId::from_raw(1);
    s.frame_request(surface, 1, Mono::from_ns(17_000_000));
    let deadline = match s.drain().remove(0) {
        SchedEvent::FrameTarget { deadline, .. } => deadline,
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(deadline.target_vblank.as_ns(), 3 * SIXTY);
    s.commit(surface, Mono::from_ns(20_000_000));
    // The intermediate flip must not present (content not due).
    s.observe_flip(Mono::from_ns(2 * SIXTY));
    assert!(s.drain().is_empty());
    s.observe_flip(Mono::from_ns(3 * SIXTY));
    assert_eq!(s.drain().len(), 1);
}
