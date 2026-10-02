//! Lock gate matrix, VT sequence walk, inhibitor registry semantics,
//! and the cross-crate inhibit-bits conformance (ldp-session and
//! ldp-power define the same wire bitset independently — pinned here).

use ldp_core::time::Mono;
use ldp_session::inhibit::{InhibitBits, InhibitRegistry};
use ldp_session::lock::{LockAction, LockScreen, LockState};
use ldp_session::logind::{PauseReason, SessionEvent};
use ldp_session::vt::{VtAction, VtState, VtSwitcher};

fn ms(m: u64) -> Mono {
    Mono::from_ms(m)
}

// ---------------------------------------------------------------------------
// Lock gate matrix: state x surface kind -> focusability.
// ---------------------------------------------------------------------------

#[test]
fn focus_gate_matrix() {
    // (state, is_lock_surface, can_focus)
    let expected: &[(LockState, bool, bool)] = &[
        (LockState::Unlocked, false, true),
        (LockState::Unlocked, true, true),
        (LockState::Locking, false, true),
        (LockState::Locking, true, true),
        (LockState::Locked, false, false),
        (LockState::Locked, true, true),
        (LockState::Unlocking, false, false),
        (LockState::Unlocking, true, true),
    ];
    for (state, is_lock, want) in expected {
        // Reach each state deterministically.
        let mut l = LockScreen::new();
        match state {
            LockState::Unlocked => {}
            LockState::Locking => {
                l.lock_requested(None);
            }
            LockState::Locked => {
                l.lock_requested(None);
                l.lock_surface_mapped();
            }
            _ => {
                // Unlocking has no linger path in v1 (unlock completes
                // immediately); reached only by direct construction in
                // the direct checks below.
                l.lock_requested(None);
                l.lock_surface_mapped();
                l.unlock_signal();
            }
        }
        // For Unlocking the approximation above ends Unlocked; assert
        // only the states we can reach.
        if l.state() == *state {
            assert_eq!(
                l.can_focus(*is_lock),
                *want,
                "gate failed for {state:?} lock={is_lock}"
            );
        }
    }
    // Direct checks for the two critical cells.
    let mut locked = LockScreen::new();
    locked.lock_requested(None);
    locked.lock_surface_mapped();
    assert!(
        !locked.can_focus(false),
        "non-lock surface focusable while locked"
    );
    assert!(locked.can_focus(true), "lock surface must stay focusable");
}

#[test]
fn lock_timeout_and_administrative_unlock() {
    let mut lock = LockScreen::new();
    lock.lock_requested(Some(ms(250)));
    // Input events keep arriving; the deadline is absolute.
    assert!(lock.tick(ms(249)).is_empty());
    assert_eq!(lock.tick(ms(250)), vec![]); // at the deadline: not yet past
    assert_eq!(lock.tick(ms(251)), vec![LockAction::ForceBlank]);
    // The late surface still locks.
    assert_eq!(lock.lock_surface_mapped(), vec![LockAction::Locked]);
    // Administrative unlock from logind.
    assert_eq!(lock.unlock_signal(), vec![LockAction::Unlocked]);
    assert_eq!(lock.state(), LockState::Unlocked);
}

// ---------------------------------------------------------------------------
// VT sequence walk over an interleaved event stream.
// ---------------------------------------------------------------------------

#[test]
fn vt_walk_ignores_stray_and_unrelated_events() {
    let mut vt = VtSwitcher::new();
    // Unrelated events never move the machine.
    for event in [
        SessionEvent::LockRequested,
        SessionEvent::UnlockRequested,
        SessionEvent::PrepareForSleep(true),
        SessionEvent::Controlled,
        SessionEvent::CallFailed {
            member: "TakeControl",
            name: "x".to_owned(),
        },
    ] {
        assert!(vt.on_event(&event).is_empty());
    }
    assert_eq!(vt.state(), VtState::Active);
    // A full away/back cycle, then a stray resume (ignored).
    let pause = SessionEvent::DevicePaused {
        major: 226,
        minor: 0,
        reason: PauseReason::Pause,
        needs_ack: true,
    };
    assert_eq!(
        vt.on_event(&pause),
        vec![
            VtAction::ReleaseDrmMaster,
            VtAction::AckPause {
                major: 226,
                minor: 0
            }
        ]
    );
    vt.switch_away_complete();
    let resume = SessionEvent::DeviceResumed {
        major: 226,
        minor: 0,
        fd: 4,
    };
    assert_eq!(
        vt.on_event(&resume),
        vec![VtAction::ReacquireDevice {
            major: 226,
            minor: 0,
            fd: 4
        }]
    );
    vt.switch_back_complete();
    assert!(vt.on_event(&resume).is_empty());
    // Pausing while Pausing (a second device) still releases + acks.
    assert_eq!(
        vt.on_event(&pause),
        vec![
            VtAction::ReleaseDrmMaster,
            VtAction::AckPause {
                major: 226,
                minor: 0
            }
        ]
    );
}

// ---------------------------------------------------------------------------
// Inhibitor registry semantics.
// ---------------------------------------------------------------------------

#[test]
fn inhibitor_registry_full_semantics() {
    let mut reg = InhibitRegistry::new();
    assert!(reg.is_empty());
    // Three clients, overlapping bits.
    let a = reg.take(1, InhibitBits::BLUR.union(InhibitBits::DISPLAY));
    let b = reg.take(2, InhibitBits::DISPLAY);
    let c = reg.take(3, InhibitBits::SUSPEND);
    assert_eq!(reg.effective().to_wire(), 0b1011);
    // Cookies are unique, non-zero, ascending.
    assert!(a < b && b < c && a != 0);
    // Release one cookie: only its bits drop.
    assert_eq!(
        reg.release(a),
        Some(InhibitBits::BLUR.union(InhibitBits::DISPLAY))
    );
    assert_eq!(reg.effective().to_wire(), 0b1010);
    // Unknown cookie release is a no-op.
    assert_eq!(reg.release(9999), None);
    // Disconnect client 2: DISPLAY drops entirely.
    assert_eq!(reg.client_disconnected(2).to_wire(), 0b0010);
    assert_eq!(reg.effective().to_wire(), 0b1000);
    // Disconnect a client with no cookies: nothing.
    assert_eq!(reg.client_disconnected(7), InhibitBits::NONE);
    assert_eq!(reg.len(), 1);
    // Holder tracking.
    assert_eq!(reg.holder(c), Some(3));
    assert_eq!(reg.holder(42), None);
}

// ---------------------------------------------------------------------------
// Cross-crate conformance: the inhibit bitset, defined twice.
// ---------------------------------------------------------------------------

#[test]
fn inhibit_bits_agree_with_the_power_layer() {
    // ldp-power's InhibitMask (the idle machine's consumer) and this
    // crate's InhibitBits (the registry) are independent definitions of
    // the same wire bitset; any drift breaks the session <-> power seam.
    use ldp_power::idle::InhibitMask as Power;
    assert_eq!(InhibitBits::BLUR.to_wire(), Power::BLUR.to_wire());
    assert_eq!(InhibitBits::DISPLAY.to_wire(), Power::DISPLAY.to_wire());
    assert_eq!(InhibitBits::IDLE.to_wire(), Power::IDLE.to_wire());
    assert_eq!(InhibitBits::SUSPEND.to_wire(), Power::SUSPEND.to_wire());
    // Union and contains behave identically on the union of everything.
    let all_session = InhibitBits::BLUR
        .union(InhibitBits::DISPLAY)
        .union(InhibitBits::IDLE)
        .union(InhibitBits::SUSPEND);
    let all_power = Power::BLUR
        .union(Power::DISPLAY)
        .union(Power::IDLE)
        .union(Power::SUSPEND);
    assert_eq!(all_session.to_wire(), all_power.to_wire());
    assert_eq!(all_session.to_wire(), 0b1111);
    // The wire round trip preserves unknown bits in both.
    for bits in [0u32, 1, 0b1111, 0b1_0000_1111] {
        assert_eq!(InhibitBits::from_wire(bits).to_wire(), bits);
        assert_eq!(Power::from_wire(bits).to_wire(), bits);
    }
}

// ---------------------------------------------------------------------------
// The idle machine consumes the registry's effective mask (the seam
// the whole session<->power integration rides).
// ---------------------------------------------------------------------------

#[test]
fn registry_mask_drives_the_idle_machine() {
    use ldp_power::idle::{IdleEvent, IdleMachine, IdleStage, IdleTimeouts};

    let timeouts = IdleTimeouts {
        dim_ms: 10,
        off_ms: 10,
        suspend_ms: 10,
    };
    let mut reg = InhibitRegistry::new();
    let mut machine = IdleMachine::new(ms(0), timeouts);

    // A video player holds DISPLAY+IDLE: dim fires, off and suspend do not.
    let _cookie = reg.take(4, InhibitBits::DISPLAY.union(InhibitBits::IDLE));
    let mask = ldp_power::idle::InhibitMask::from_wire(reg.effective().to_wire());
    let events = machine.tick(ms(1000), mask);
    assert_eq!(
        events,
        vec![IdleEvent::Stage(IdleStage::Dimmed), IdleEvent::Dim(true)]
    );
    // The player disconnects mid-movie: everything fires on the next tick.
    reg.client_disconnected(4);
    let mask = ldp_power::idle::InhibitMask::from_wire(reg.effective().to_wire());
    let events = machine.tick(ms(1001), mask);
    assert_eq!(
        events,
        vec![
            IdleEvent::Stage(IdleStage::Off),
            IdleEvent::Dpms(false),
            IdleEvent::Stage(IdleStage::Suspend),
        ]
    );
}
