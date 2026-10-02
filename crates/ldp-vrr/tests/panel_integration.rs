//! Phase 15 exit criterion 2 (part 3) — panel integration goldens.
//!
//! The mock KMS device closes the loop: the engine's `WindowApplied`
//! emission programs `VRR_ENABLED` through a plain atomic commit; flips
//! submitted per the engine's rulings land at exactly the ruled times
//! (the panel model and the selector agree to the nanosecond); the
//! seamless toggle round-trips; and the windowed scheduler's
//! `presented` feedback lands at the panel's page-flip completions with
//! the *measured* VRR interval — never a torn flag.

use ldp_compositor::scheduler::SchedulerConfig;
use ldp_compositor::{SchedEvent, SurfaceId};
use ldp_core::time::{Mono, PresentationMode, RefreshInterval};
use ldp_vrr::caps::{OutputVrrSupport, VrrCaps};
use ldp_vrr::engine::{VrrEngine, VrrEvent};
use ldp_vrr::policy::VrrPolicy;
use ldp_vrr::refresh::CommitRuling;

const MIN: u64 = 1_000_000_000 / 144; // 6.94 ms — 144 Hz floor
const MAX: u64 = 1_000_000_000 / 48; // 20.83 ms — 48 Hz stretch
const NOM: u64 = 16_666_666; // 60 Hz mode
const COSTS: u64 = 400_000 + 600_000; // submit + flip latency

fn sixty() -> RefreshInterval {
    RefreshInterval::from_ns(NOM).unwrap()
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

fn t(ns: u64) -> Mono {
    Mono::from_ns(ns)
}

#[test]
fn engine_rulings_match_the_mock_panel_flip_times() {
    use ldp_core::buffer::{FourCC, Modifier};
    use ldp_display::atomic::AtomicRequest;
    use ldp_display::backend::KmsBackend;
    use ldp_display::commit::{CommitFlags, DstRect, SrcRect};
    use ldp_display::fb::FbSpec;
    use ldp_display::ids::{ConnectorId, CrtcId, PlaneId};
    use ldp_display::mode::Mode;
    use ldp_display::props::{prop, PropValue};
    use ldp_display::MockDevice;

    let edp = ConnectorId::new(91).unwrap();
    let crtc = CrtcId::new(42).unwrap();
    let primary = PlaneId::new(50).unwrap();

    let mut dev = MockDevice::laptop_dual();
    let fb = dev
        .add_fb(
            &FbSpec::single(1920, 1080, FourCC::XRGB8888, 1, 1920 * 4, 0),
            Some(Modifier::LINEAR),
        )
        .expect("fb registers");

    // The engine says: adaptive demand under deadline policy → enable.
    let mut engine = VrrEngine::new(
        Some(support()),
        VrrPolicy::Deadline,
        SchedulerConfig::default(),
    );
    engine.drain(); // the initial (disabled) state
    engine.set_adaptive_demand(true);
    match engine.drain().as_slice() {
        [VrrEvent::WindowApplied {
            enable: true,
            window: Some(_),
            seamless: true,
        }] => {}
        other => panic!("expected the enable emission, got {other:?}"),
    }

    // Pipeline up with VRR programmed from the engine decision.
    let req = AtomicRequest::new()
        .flag(CommitFlags::ALLOW_MODESET)
        .flag(CommitFlags::PAGE_FLIP_EVENT)
        .connector_bind(edp, crtc)
        .crtc_active(crtc, true)
        .crtc_mode(crtc, &Mode::panel_1080p60())
        .crtc_vrr(crtc, true)
        .plane_on(
            primary,
            crtc,
            fb,
            SrcRect::new(0, 0, 1920, 1080).unwrap(),
            DstRect::new(0, 0, 1920, 1080).unwrap(),
        );
    dev.commit(&req).expect("pipeline commits");
    assert!(dev.vrr_enabled(crtc), "VRR_ENABLED programmed");

    // The first pipeline flip lands at max(commit, last+min) = MIN
    // (t=0 commit, last=0).
    let flip = next_flip(&mut dev, crtc);
    assert_eq!(flip.timestamp.as_ns(), MIN);
    engine.observe_flip(flip.timestamp);

    // A 90 Hz cadence: content ready every 11.11 ms. Each ruling says
    // "flip at ready"; the device must agree to the nanosecond.
    for k in 1..=5u64 {
        let ready = MIN + k * 11_111_111;
        let ruling = engine.commit_ready(SurfaceId::from_raw(1), k, t(ready));
        match ruling {
            CommitRuling::FlipAt(at) => {
                assert_eq!(at.as_ns(), ready, "ruling {k}: at ready");
                // Submit the flip at readiness (the device clock is
                // already there) and let it mature.
                dev.advance_to(t(ready));
                let flip_req = AtomicRequest::new().flag(CommitFlags::PAGE_FLIP_EVENT).set(
                    ldp_display::ids::AnyId::Plane(primary),
                    prop::FB_ID,
                    PropValue::U64(u64::from(fb.raw())),
                );
                dev.commit(&flip_req).expect("flip commits");
                let flip = next_flip(&mut dev, crtc);
                assert_eq!(
                    flip.timestamp.as_ns(),
                    ready,
                    "device flip {k} matches the ruling"
                );
                engine.observe_flip(flip.timestamp);
            }
            other => panic!("90 Hz cadence ruling {k}: {other:?}"),
        }
    }
    // Interval invariants held across the whole run.
    let flips = dev.flip_count(crtc);
    assert!(flips >= 6);
}

#[test]
fn vrr_toggle_round_trips_through_the_engine() {
    use ldp_display::atomic::AtomicRequest;
    use ldp_display::backend::KmsBackend;
    use ldp_display::commit::CommitFlags;
    use ldp_display::ids::CrtcId;
    use ldp_display::MockDevice;

    let crtc = CrtcId::new(42).unwrap();
    let mut dev = MockDevice::laptop_dual();
    bring_up(&mut dev, crtc, false);
    assert!(!dev.vrr_enabled(crtc));

    let mut engine = VrrEngine::new(
        Some(support()),
        VrrPolicy::Deadline,
        SchedulerConfig::default(),
    );
    assert!(
        matches!(
            engine.drain().as_slice(),
            [VrrEvent::WindowApplied { enable: false, .. }]
        ),
        "no demand yet"
    );

    // Demand arrives: the engine's emission programs the CRTC through
    // a plain (seamless) commit — no modeset flag needed.
    engine.set_adaptive_demand(true);
    match engine.drain().as_slice() {
        [VrrEvent::WindowApplied {
            enable: true,
            window: Some(w),
            seamless: true,
        }] => {
            assert_eq!((w.min_ns(), w.max_ns()), (MIN, MAX));
        }
        other => panic!("expected enable, got {other:?}"),
    }
    let on = AtomicRequest::new()
        .flag(CommitFlags::PAGE_FLIP_EVENT)
        .crtc_vrr(crtc, true);
    dev.commit(&on).expect("seamless enable");
    assert!(dev.vrr_enabled(crtc));
    assert_eq!(dev.vrr_window(crtc), Some((MIN, MAX)));

    // Battery saver engages: disable, same seamless path.
    engine.set_battery_saver(true);
    assert!(matches!(
        engine.drain().as_slice(),
        [VrrEvent::WindowApplied { enable: false, .. }]
    ));
    let off = AtomicRequest::new().crtc_vrr(crtc, false);
    dev.commit(&off).expect("seamless disable");
    assert!(!dev.vrr_enabled(crtc));
}

/// Bring the pipeline up (helper for the toggle test).
fn bring_up(dev: &mut ldp_display::MockDevice, crtc: ldp_display::ids::CrtcId, vrr: bool) {
    use ldp_core::buffer::{FourCC, Modifier};
    use ldp_display::atomic::AtomicRequest;
    use ldp_display::backend::KmsBackend;
    use ldp_display::commit::{CommitFlags, DstRect, SrcRect};
    use ldp_display::fb::FbSpec;
    use ldp_display::ids::{ConnectorId, PlaneId};
    use ldp_display::mode::Mode;

    let edp = ConnectorId::new(91).unwrap();
    let primary = PlaneId::new(50).unwrap();
    let fb = dev
        .add_fb(
            &FbSpec::single(1920, 1080, FourCC::XRGB8888, 1, 1920 * 4, 0),
            Some(Modifier::LINEAR),
        )
        .expect("fb registers");
    let req = AtomicRequest::new()
        .flag(CommitFlags::ALLOW_MODESET)
        .connector_bind(edp, crtc)
        .crtc_active(crtc, true)
        .crtc_mode(crtc, &Mode::panel_1080p60())
        .crtc_vrr(crtc, vrr)
        .plane_on(
            primary,
            crtc,
            fb,
            SrcRect::new(0, 0, 1920, 1080).unwrap(),
            DstRect::new(0, 0, 1920, 1080).unwrap(),
        );
    dev.commit(&req).expect("pipeline commits");
}

/// Advance exactly to each next event until a page flip matures.
fn next_flip(
    dev: &mut ldp_display::MockDevice,
    crtc: ldp_display::ids::CrtcId,
) -> ldp_display::events::PageFlipEvent {
    use ldp_display::events::DeviceEvent;
    loop {
        let at = dev
            .next_event_at()
            .expect("a live timeline always has a next event");
        for e in dev.advance_to(at) {
            if let DeviceEvent::PageFlip(f) = e {
                assert_eq!(f.crtc, crtc);
                return f;
            }
        }
    }
}

#[test]
fn scheduler_presented_aligns_with_panel_vrr_flips() {
    // The full loop: engine rulings drive device flips; device flips
    // feed the windowed scheduler; the presented feedback carries the
    // *measured* VRR interval (the 90 Hz cadence), and the vblank flag
    // — never torn.
    use ldp_core::buffer::{FourCC, Modifier};
    use ldp_display::atomic::AtomicRequest;
    use ldp_display::backend::KmsBackend;
    use ldp_display::commit::{CommitFlags, DstRect, SrcRect};
    use ldp_display::fb::FbSpec;
    use ldp_display::ids::{AnyId, ConnectorId, CrtcId, PlaneId};
    use ldp_display::mode::Mode;
    use ldp_display::props::{prop, PropValue};
    use ldp_display::MockDevice;

    let edp = ConnectorId::new(91).unwrap();
    let crtc = CrtcId::new(42).unwrap();
    let primary = PlaneId::new(50).unwrap();
    let surface = SurfaceId::from_raw(1);

    let mut dev = MockDevice::laptop_dual();
    let fb = dev
        .add_fb(
            &FbSpec::single(1920, 1080, FourCC::XRGB8888, 1, 1920 * 4, 0),
            Some(Modifier::LINEAR),
        )
        .expect("fb registers");

    let mut engine = VrrEngine::new(
        Some(support()),
        VrrPolicy::Deadline,
        SchedulerConfig::default(),
    );
    engine.set_adaptive_demand(true);
    engine.drain();

    let req = AtomicRequest::new()
        .flag(CommitFlags::ALLOW_MODESET)
        .flag(CommitFlags::PAGE_FLIP_EVENT)
        .connector_bind(edp, crtc)
        .crtc_active(crtc, true)
        .crtc_mode(crtc, &Mode::panel_1080p60())
        .crtc_vrr(crtc, true)
        .plane_on(
            primary,
            crtc,
            fb,
            SrcRect::new(0, 0, 1920, 1080).unwrap(),
            DstRect::new(0, 0, 1920, 1080).unwrap(),
        );
    dev.commit(&req).expect("pipeline commits");

    // The scheduler runs the engine's config.
    let mut sched =
        ldp_compositor::scheduler::FrameScheduler::new(sixty(), engine.scheduler_config()).unwrap();
    sched.set_mode(surface, PresentationMode::Adaptive, Mono::ZERO);

    // First flip at MIN anchors both timelines.
    let first = next_flip(&mut dev, crtc);
    assert_eq!(first.timestamp.as_ns(), MIN);
    engine.observe_flip(first.timestamp);
    sched.observe_flip(first.timestamp);
    sched.drain();

    // Register interest; the reply targets the PLL grid ahead: anchored
    // at MIN (re-anchored off the 60 Hz origin), the next grid vblank
    // is MIN + NOM and the deadline one costs-bundle earlier.
    let target = MIN + NOM;
    let deadline = target - COSTS;
    let expiry = deadline + (MAX - NOM);
    sched.frame_request(surface, 1, first.timestamp);
    let events = sched.drain();
    match events.as_slice() {
        [SchedEvent::FrameTarget {
            deadline: reply, ..
        }] => {
            assert_eq!(reply.mode, PresentationMode::Adaptive);
            assert_eq!(reply.target_vblank.as_ns(), target);
            assert_eq!(reply.deadline.as_ns(), deadline);
            assert_eq!(reply.budget_ns, deadline - MIN);
        }
        other => panic!("expected the frame target, got {other:?}"),
    }

    // The window-late save: content committed past the plain deadline
    // but inside the widened window still satisfies, and the VRR panel
    // stretch lets the flip land at the commit itself.
    let ready = deadline + 2_000_000;
    assert!(ready < expiry, "the commit must be inside the window");
    sched.commit(surface, t(ready));
    assert!(
        sched.drain().is_empty(),
        "satisfaction emits nothing; presentation waits for the flip"
    );

    // The engine rules the flip slot: ready is past the min floor and
    // inside the stretch — at readiness.
    let ruling = engine.commit_ready(surface, 1, t(ready));
    match ruling {
        CommitRuling::FlipAt(at) => assert_eq!(at.as_ns(), ready),
        other => panic!("expected at-ready ruling, got {other:?}"),
    }
    dev.advance_to(t(ready));
    let flip_req = AtomicRequest::new().flag(CommitFlags::PAGE_FLIP_EVENT).set(
        AnyId::Plane(primary),
        prop::FB_ID,
        PropValue::U64(u64::from(fb.raw())),
    );
    dev.commit(&flip_req).expect("flip commits");
    let flip = next_flip(&mut dev, crtc);
    assert_eq!(flip.timestamp.as_ns(), ready);

    // The flip feeds both; the scheduler presents the satisfied frame
    // at the panel flip, with the *measured* VRR interval (MIN→ready)
    // in the feedback — and never a torn flag.
    engine.observe_flip(flip.timestamp);
    sched.observe_flip(flip.timestamp);
    let events = sched.drain();
    match events.as_slice() {
        [SchedEvent::Presented { timing, .. }] => {
            assert_eq!(timing.frame, 1);
            assert_eq!(timing.presented_at.as_ns(), ready);
            assert_eq!(timing.refresh.as_ns(), ready - MIN);
            assert!(timing.flags.vblank);
            assert!(!timing.flags.torn);
        }
        other => panic!("expected presentation, got {other:?}"),
    }
}
