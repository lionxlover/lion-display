//! Phase 35 exit criteria — panel self-refresh proofs at the device
//! level.
//!
//! The mock models the kernel's PSR semantics exactly as this phase
//! documents them: the property write engages (the panel holds its
//! GRAM), an engaged CRTC's timeline *freezes* (no vblanks, no flip
//! landings — advancing time past a sleeping panel is free, which is
//! the entire point), and either an explicit property release or a
//! submitted page flip (the kernel's implicit rescan) re-anchors the
//! grid at the commit's own time — so the next flip lands one full
//! nominal period later, never free. Every assertion below is the
//! device-side half of the sleeping-panel doctrine; the compositor's
//! decision layer is proven in the session suite.

use ldp_core::buffer::{FourCC, Modifier};
use ldp_display::atomic::AtomicRequest;
use ldp_display::backend::KmsBackend;
use ldp_display::commit::{CommitFlags, DstRect, SrcRect};
use ldp_display::events::DeviceEvent;
use ldp_display::fb::FbSpec;
use ldp_display::ids::{AnyId, ConnectorId, CrtcId, PlaneId};
use ldp_display::mode::Mode;
use ldp_display::props::prop;
use ldp_display::MockDevice;

const NOMINAL: u64 = 16_666_666; // the 60 Hz mode period

macro_rules! preset_id {
    ($name:ident, $t:ty, $raw:literal) => {
        const $name: $t = match <$t>::new($raw) {
            Some(id) => id,
            None => panic!("preset id is nonzero"),
        };
    };
}
preset_id!(EDP, ConnectorId, 91);
preset_id!(CRTC0, CrtcId, 42);
preset_id!(PRIMARY, PlaneId, 50);

/// Bring the pipeline up on CRTC 42 (fixed sync — the PSR doctrine is
/// orthogonal to VRR).
fn pipeline(dev: &mut MockDevice) -> ldp_display::ids::FbId {
    let fb = dev
        .add_fb(
            &FbSpec::single(1920, 1080, FourCC::XRGB8888, 1, 1920 * 4, 0),
            Some(Modifier::LINEAR),
        )
        .expect("fb registers");
    let req = AtomicRequest::new()
        .flag(CommitFlags::ALLOW_MODESET)
        .flag(CommitFlags::PAGE_FLIP_EVENT)
        .connector_bind(EDP, CRTC0)
        .crtc_active(CRTC0, true)
        .crtc_mode(CRTC0, &Mode::panel_1080p60())
        .plane_on(
            PRIMARY,
            CRTC0,
            fb,
            SrcRect::new(0, 0, 1920, 1080).unwrap(),
            DstRect::new(0, 0, 1920, 1080).unwrap(),
        );
    dev.commit(&req).expect("pipeline commits");
    fb
}

/// Land the pipeline's bring-up flip (t=0 anchor).
fn land_first_flip(dev: &mut MockDevice) {
    let at = dev.next_event_at().expect("the bring-up flip is due");
    let events = dev.advance_to(at);
    assert!(
        events.iter().any(|e| matches!(e, DeviceEvent::PageFlip(_))),
        "the bring-up flip lands"
    );
}

/// Engage (or release) the panel's self-refresh.
fn psr(dev: &mut MockDevice, on: bool) {
    let req = AtomicRequest::new()
        .flag(CommitFlags::NONBLOCK)
        .connector_psr(EDP, on);
    dev.commit(&req).expect("the psr property write applies");
}

/// Submit a flip (the implicit rescan vocabulary).
fn flip(dev: &mut MockDevice, fb: ldp_display::ids::FbId) {
    let req = AtomicRequest::new().flag(CommitFlags::PAGE_FLIP_EVENT).set(
        AnyId::Plane(PRIMARY),
        prop::FB_ID,
        ldp_display::props::PropValue::U64(u64::from(fb.raw())),
    );
    dev.commit(&req).expect("flip commits");
}

#[test]
fn the_property_round_trips_and_starts_disengaged() {
    let mut dev = MockDevice::laptop_dual();
    pipeline(&mut dev);
    land_first_flip(&mut dev);
    assert!(!dev.psr_enabled(EDP), "a fresh panel is scanning");
    psr(&mut dev, true);
    assert!(dev.psr_enabled(EDP), "the write engages");
    psr(&mut dev, false);
    assert!(!dev.psr_enabled(EDP), "the write releases");
}

#[test]
fn a_sleeping_panel_advances_time_for_free() {
    // The doctrine's core: with self-refresh engaged the timeline is
    // frozen — ten seconds of wall time cross the device with zero
    // vblanks, zero flips, zero events. The display engine is dark;
    // the panel holds its own GRAM.
    let mut dev = MockDevice::laptop_dual();
    pipeline(&mut dev);
    land_first_flip(&mut dev);
    let vblanks_before = dev.vblank_count(CRTC0);
    let flips_before = dev.flip_count(CRTC0);
    psr(&mut dev, true);
    // The freeze is invisible to next_event_at: nothing is due.
    assert!(
        dev.next_event_at().is_none(),
        "a sleeping panel has nothing due"
    );
    let events = dev.advance_ns(10_000_000_000);
    assert!(events.is_empty(), "ten seconds, zero events: {events:?}");
    assert_eq!(dev.vblank_count(CRTC0), vblanks_before);
    assert_eq!(dev.flip_count(CRTC0), flips_before);
    // And the release still works after the long sleep.
    psr(&mut dev, false);
    assert!(!dev.psr_enabled(EDP));
}

#[test]
fn an_explicit_release_pays_one_nominal_of_rescan() {
    // Releasing through the property write re-anchors the grid at the
    // commit's time: the next flip submitted at t lands one full
    // nominal later — the honest rescan cost.
    let mut dev = MockDevice::laptop_dual();
    let fb = pipeline(&mut dev);
    land_first_flip(&mut dev);
    psr(&mut dev, true);
    // Sleep a while (the clock is free while frozen).
    let _ = dev.advance_ns(5_000_000_000);
    let now = dev.now();
    psr(&mut dev, false);
    // A flip submitted immediately after the release.
    flip(&mut dev, fb);
    let at = dev.next_event_at().expect("the rescan flip is scheduled");
    assert_eq!(
        at.as_ns() - now.as_ns(),
        NOMINAL,
        "the rescan costs exactly one refresh interval"
    );
}

#[test]
fn a_flip_is_the_implicit_release_and_pays_the_same_rescan() {
    // The kernel's own semantics: submitting a page flip to a
    // self-refreshing CRTC releases the panel (the property reads
    // back off) and the flip lands one nominal after the submission —
    // no free exits, whichever vocabulary releases.
    let mut dev = MockDevice::laptop_dual();
    let fb = pipeline(&mut dev);
    land_first_flip(&mut dev);
    psr(&mut dev, true);
    let _ = dev.advance_ns(2_000_000_000);
    let now = dev.now();
    flip(&mut dev, fb);
    assert!(
        !dev.psr_enabled(EDP),
        "the flip releases the property implicitly"
    );
    let at = dev.next_event_at().expect("the implicit release schedules");
    assert_eq!(
        at.as_ns() - now.as_ns(),
        NOMINAL,
        "the implicit rescan costs one refresh interval too"
    );
    // The panel went back to scanning: vblanks resume on the grid.
    let events = dev.advance_to(at);
    assert!(
        events.iter().any(|e| matches!(e, DeviceEvent::PageFlip(_))),
        "the flip lands after the rescan"
    );
}

#[test]
fn a_disengaged_panel_flips_on_the_untouched_grid() {
    // The control: without self-refresh, the same flip at the same
    // clock rides the nominal grid — the rescan cost above is the
    // PSR exit's own, not an artifact of the timeline arithmetic.
    let mut dev = MockDevice::laptop_dual();
    let fb = pipeline(&mut dev);
    land_first_flip(&mut dev);
    let _ = dev.advance_ns(2_000_000_000);
    let now = dev.now();
    flip(&mut dev, fb);
    let at = dev.next_event_at().expect("the flip is scheduled");
    // On the strict grid: the first point strictly after `now`.
    let elapsed = now.as_ns() % NOMINAL;
    let expect = now.as_ns() + (NOMINAL - elapsed);
    assert_eq!(at.as_ns(), expect, "no rescan without self-refresh");
    let _ = fb;
}

#[test]
fn the_second_flip_after_release_rides_the_grid_again() {
    // The rescan is paid once: after the release's re-anchor, the
    // grid continues normally — a *second* flip submitted after the
    // first's landing carries no exit cost.
    let mut dev = MockDevice::laptop_dual();
    let fb = pipeline(&mut dev);
    land_first_flip(&mut dev);
    psr(&mut dev, true);
    let _ = dev.advance_ns(1_000_000_000);
    psr(&mut dev, false);
    flip(&mut dev, fb);
    // Land it (the rescan interval).
    let at = dev.next_event_at().expect("the rescan flip");
    let _ = dev.advance_to(at);
    // The next flip rides the grid.
    let now = dev.now();
    flip(&mut dev, fb);
    let at2 = dev.next_event_at().expect("the second flip");
    assert!(
        at2.as_ns() - now.as_ns() <= NOMINAL,
        "the grid is live again: {} ns after submission",
        at2.as_ns() - now.as_ns()
    );
}
