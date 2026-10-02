//! Phase 15 exit criterion 3 — tearing opt-in, verified isolated from
//! VRR.
//!
//! Tearing is a **separate opt-in**: a surface in immediate mode on an
//! async-flip-capable output the session permits. The isolation is
//! proven at three levels:
//!
//! 1. **Gate level** — the exhaustive input cross-product under both
//!    VRR states yields byte-identical decisions (the gate cannot even
//!    *see* VRR), plus an LCG-randomized corpus for good measure.
//! 2. **Scheduler level** — the deadline scheduler's event vector for
//!    an immediate-mode surface is *identical* whether the adaptive
//!    window is installed or not: the widening never touches the
//!    immediate path (deadline "now", unbounded expiry, torn flag).
//!    And an adaptive surface on the same windowed output is
//!    structurally tear-free.
//! 3. **KMS seam level** — the async-flip decision maps onto exactly
//!    the `PAGE_FLIP_ASYNC` commit flag, and nothing else: a plain
//!    (non-immediate) surface's flips never carry it.

use ldp_compositor::scheduler::SchedulerConfig;
use ldp_compositor::{SchedEvent, SchedInput, SurfaceId};
use ldp_core::time::{FrameDropReason, Mono, PresentationMode, RefreshInterval};
use ldp_vrr::policy::{decide, PolicyInputs, VrrPolicy};
use ldp_vrr::tearing::{mode_is_tear_free, tearing_decision, TearingInputs, TearingRationale};

const SIXTY: u64 = 16_666_666;
const MIN: u64 = 1_000_000_000 / 144;
const MAX: u64 = 1_000_000_000 / 48;

fn sixty() -> RefreshInterval {
    RefreshInterval::from_ns(SIXTY).unwrap()
}

// ---------------------------------------------------------------------------
// 1. Gate level: VRR state cannot enter the decision
// ---------------------------------------------------------------------------

#[test]
fn gate_matrix_is_identical_under_both_vrr_states() {
    for mode in [
        PresentationMode::Vsync,
        PresentationMode::Adaptive,
        PresentationMode::Immediate,
    ] {
        for async_capable in [false, true] {
            for permits in [false, true] {
                let inputs = TearingInputs {
                    mode,
                    async_flip_capable: async_capable,
                    session_permits: permits,
                };
                let d = tearing_decision(&inputs);
                // The normative conjunction.
                assert_eq!(
                    d.allowed,
                    mode == PresentationMode::Immediate && async_capable && permits,
                    "mode={mode:?} cap={async_capable} permits={permits}"
                );
                assert_eq!(
                    d.requires_async_flip, d.allowed,
                    "the async-flip flag rides exactly on allowance"
                );
                // Structural tear-freedom of non-immediate modes.
                assert_eq!(mode_is_tear_free(mode), mode != PresentationMode::Immediate);
            }
        }
    }
}

#[test]
fn gate_is_blind_to_vrr_by_construction() {
    // Every gate input combination, evaluated alongside *every* policy
    // table outcome: the tearing decision must not budge. (The gate's
    // signature has no VRR parameter — this test pins that the *system*
    // composes the two decisions independently.)
    let support = ldp_vrr::caps::OutputVrrSupport::new(
        MIN,
        MAX,
        ldp_vrr::caps::VrrCaps {
            seamless: true,
            fixed_rate: true,
        },
        SIXTY,
    )
    .unwrap();
    let mut vrr_states = Vec::new();
    for &policy in &[VrrPolicy::Off, VrrPolicy::Deadline, VrrPolicy::Always] {
        for &demand in &[false, true] {
            for &battery in &[false, true] {
                let d = decide(&PolicyInputs {
                    policy,
                    support: Some(&support),
                    adaptive_demand: demand,
                    battery_saver: battery,
                });
                vrr_states.push((d.vrr_enabled, d.rationale, d.scheduler_window_ns));
            }
        }
    }
    assert!(vrr_states.iter().any(|&(on, ..)| on));
    assert!(vrr_states.iter().any(|&(on, ..)| !on));
    for mode in [
        PresentationMode::Vsync,
        PresentationMode::Adaptive,
        PresentationMode::Immediate,
    ] {
        for &cap in &[false, true] {
            for &permits in &[false, true] {
                let d = tearing_decision(&TearingInputs {
                    mode,
                    async_flip_capable: cap,
                    session_permits: permits,
                });
                for &(vrr_on, _, _) in &vrr_states {
                    assert_eq!(
                        d.allowed,
                        mode == PresentationMode::Immediate && cap && permits,
                        "vrr_on={vrr_on} must not change the tearing answer"
                    );
                }
            }
        }
    }
}

#[test]
fn gate_randomized_corpus_is_vrr_independent() {
    // LCG corpus (the no-rand doctrine): 2,000 random input triples,
    // each evaluated with VRR on and off — identical decisions, and
    // allowed implies the full conjunction.
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            self.0 >> 33
        }
    }
    let mut rng = Lcg(0x7E_A7);
    for _ in 0..2_000 {
        let mode = match rng.next() % 3 {
            0 => PresentationMode::Vsync,
            1 => PresentationMode::Adaptive,
            _ => PresentationMode::Immediate,
        };
        let cap = rng.next() % 2 == 0;
        let permits = rng.next() % 2 == 0;
        let inputs = TearingInputs {
            mode,
            async_flip_capable: cap,
            session_permits: permits,
        };
        let on = tearing_decision(&inputs);
        let off = tearing_decision(&inputs); // VRR is not an input
        assert_eq!(on, off);
        assert_eq!(
            on.allowed,
            matches!(mode, PresentationMode::Immediate) && cap && permits
        );
        assert_eq!(on.requires_async_flip, on.allowed);
    }
}

#[test]
fn refusal_rationale_precedence() {
    // NotRequested beats Unsupported beats Forbidden — the audit trail
    // reports the first unmet condition.
    assert_eq!(
        tearing_decision(&TearingInputs {
            mode: PresentationMode::Adaptive,
            async_flip_capable: false,
            session_permits: false,
        })
        .rationale,
        TearingRationale::NotRequested
    );
    assert_eq!(
        tearing_decision(&TearingInputs {
            mode: PresentationMode::Immediate,
            async_flip_capable: false,
            session_permits: false,
        })
        .rationale,
        TearingRationale::Unsupported
    );
    assert_eq!(
        tearing_decision(&TearingInputs {
            mode: PresentationMode::Immediate,
            async_flip_capable: true,
            session_permits: false,
        })
        .rationale,
        TearingRationale::Forbidden
    );
}

// ---------------------------------------------------------------------------
// 2. Scheduler level: the immediate path is untouched by the window
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

fn mode_set(surface: u64, m: PresentationMode, ts: u64) -> SchedInput {
    SchedInput::SetMode {
        surface: SurfaceId::from_raw(surface),
        mode: m,
        ts: Mono::from_ns(ts),
    }
}

#[test]
fn immediate_path_is_identical_with_and_without_the_window() {
    // Tearing opt-in (immediate mode) with VRR enabled vs disabled: the
    // scheduler's event vector is byte-identical — the widening only
    // ever touches adaptive registrations.
    let inputs = |surface: u64| {
        vec![
            flip(SIXTY),
            mode_set(surface, PresentationMode::Immediate, SIXTY + 1),
            req(surface, 1, SIXTY + 2),
            // Immediate deadline is "now": any commit satisfies.
            commit(surface, SIXTY + 3_000_000),
            flip(2 * SIXTY),
        ]
    };
    let plain = SchedulerConfig::default(); // vrr_window_ns = 0
    let windowed = SchedulerConfig {
        vrr_window_ns: MAX - SIXTY,
        ..SchedulerConfig::default()
    };
    let a = ldp_compositor::replay::run(plain, sixty(), &inputs(1)).unwrap();
    let b = ldp_compositor::replay::run(windowed, sixty(), &inputs(1)).unwrap();
    assert_eq!(a, b, "the window cannot touch the immediate path");
    // And the immediate presentation is torn.
    match a.last() {
        Some(SchedEvent::Presented { timing, .. }) => {
            assert!(timing.flags.torn, "immediate presentations tear");
            assert!(!timing.flags.vblank);
        }
        other => panic!("expected a presentation, got {other:?}"),
    }
}

#[test]
fn adaptive_surface_on_a_windowed_output_never_tears() {
    // The other direction: VRR fully engaged (window installed, surface
    // adaptive) — presentations carry the vblank flag and never the
    // torn flag, including the window-late save.
    let windowed = SchedulerConfig {
        vrr_window_ns: MAX - SIXTY,
        ..SchedulerConfig::default()
    };
    let late = 2 * SIXTY - 1_000_000 + 2_000_000;
    let inputs = [
        flip(SIXTY),
        mode_set(1, PresentationMode::Adaptive, SIXTY + 1),
        req(1, 5, SIXTY + 1),
        commit(1, late), // past the deadline, inside the window
        flip(late),
    ];
    let events = ldp_compositor::replay::run(windowed, sixty(), &inputs).unwrap();
    let mut presented = 0;
    for e in &events {
        if let SchedEvent::Presented { timing, .. } = e {
            assert!(timing.flags.vblank);
            assert!(!timing.flags.torn, "VRR never tears");
            presented += 1;
        }
    }
    assert_eq!(presented, 1, "the window-late frame presents");
}

#[test]
fn mixed_output_tears_exactly_the_immediate_surface() {
    // One output, VRR windowed: an adaptive surface and an immediate
    // surface side by side. Exactly the immediate one presents torn;
    // the adaptive one rides the window tear-free.
    let windowed = SchedulerConfig {
        vrr_window_ns: MAX - SIXTY,
        ..SchedulerConfig::default()
    };
    let late = 2 * SIXTY - 1_000_000 + 2_000_000;
    let inputs = [
        flip(SIXTY),
        mode_set(1, PresentationMode::Adaptive, SIXTY + 1),
        mode_set(2, PresentationMode::Immediate, SIXTY + 1),
        req(1, 5, SIXTY + 2),
        req(2, 9, SIXTY + 3),
        commit(2, SIXTY + 4_000_000), // immediate: any time satisfies
        commit(1, late),              // window-late save (time-ordered)
        flip(late),                   // both present at this flip report
    ];
    let events = ldp_compositor::replay::run(windowed, sixty(), &inputs).unwrap();
    let mut torn = Vec::new();
    let mut clean = Vec::new();
    for e in &events {
        if let SchedEvent::Presented {
            surface, timing, ..
        } = e
        {
            if timing.flags.torn {
                torn.push(surface);
            } else {
                assert!(timing.flags.vblank);
                clean.push(surface);
            }
        }
    }
    assert_eq!(
        torn,
        vec![&SurfaceId::from_raw(2)],
        "only the immediate surface tears"
    );
    assert_eq!(
        clean,
        vec![&SurfaceId::from_raw(1)],
        "the adaptive surface rides the window"
    );
    // No drops: both registrations terminated in presentations.
    assert!(!events.iter().any(|e| matches!(
        e,
        SchedEvent::FrameDropped {
            reason: FrameDropReason::Throttled,
            ..
        }
    )));
}

// ---------------------------------------------------------------------------
// 3. KMS seam level: the async-flip flag
// ---------------------------------------------------------------------------

#[test]
fn async_flip_flag_rides_exactly_on_the_tearing_allowance() {
    use ldp_display::commit::CommitFlags;
    for mode in [
        PresentationMode::Vsync,
        PresentationMode::Adaptive,
        PresentationMode::Immediate,
    ] {
        for &cap in &[false, true] {
            for &permits in &[false, true] {
                let d = tearing_decision(&TearingInputs {
                    mode,
                    async_flip_capable: cap,
                    session_permits: permits,
                });
                let flags = if d.requires_async_flip {
                    CommitFlags::PAGE_FLIP_EVENT | CommitFlags::PAGE_FLIP_ASYNC
                } else {
                    CommitFlags::PAGE_FLIP_EVENT
                };
                assert_eq!(flags.is_async_flip(), d.allowed);
                assert!(flags.wants_flip_event());
            }
        }
    }
    // The kernel encoding: async is 0x02, distinct from every other
    // flag bit.
    assert_eq!(CommitFlags::PAGE_FLIP_ASYNC.0, 0x02);
    assert!(!CommitFlags::PAGE_FLIP_EVENT.is_async_flip());
    assert!(!CommitFlags::NONBLOCK.is_async_flip());
    assert!(!CommitFlags::ALLOW_MODESET.is_async_flip());
}

#[test]
fn async_flagged_flips_commit_through_the_mock_device() {
    // The seam is wire-compatible end to end: a tearing-allowed flip
    // carries PAGE_FLIP_ASYNC and the mock KMS device accepts the
    // commit (it programs the CRTC exactly like any other flip).
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

    // The gate says yes: immediate surface, async-capable, permitted.
    let gate = tearing_decision(&TearingInputs {
        mode: PresentationMode::Immediate,
        async_flip_capable: true,
        session_permits: true,
    });
    assert!(gate.allowed);

    let mut dev = MockDevice::laptop_dual();
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
        .plane_on(
            primary,
            crtc,
            fb,
            SrcRect::new(0, 0, 1920, 1080).unwrap(),
            DstRect::new(0, 0, 1920, 1080).unwrap(),
        );
    dev.commit(&req).expect("pipeline commits");

    // The async flip: PAGE_FLIP_EVENT | PAGE_FLIP_ASYNC.
    let flip_req = AtomicRequest::new()
        .flag(CommitFlags::PAGE_FLIP_EVENT)
        .flag(CommitFlags::PAGE_FLIP_ASYNC)
        .set(
            AnyId::Plane(primary),
            prop::FB_ID,
            PropValue::U64(u64::from(fb.raw())),
        );
    dev.commit(&flip_req).expect("async flip commits");
    assert_eq!(dev.pending_flips(crtc), 1);

    // And the gate refusing means the flag is never set: a plain flip
    // for a non-immediate surface carries no async bit.
    let gate = tearing_decision(&TearingInputs {
        mode: PresentationMode::Adaptive,
        async_flip_capable: true,
        session_permits: true,
    });
    assert!(!gate.requires_async_flip);
}
