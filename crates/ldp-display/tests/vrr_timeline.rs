//! Phase 9 exit criteria — VRR timeline proofs at the device level.
//!
//! CRTC 42 carries the preset's adaptive-sync window (48-144 Hz); every
//! test enables the pipeline with `VRR_ENABLED` and asserts the exact
//! completion times the mock doctrine prescribes: a flip lands at
//! `max(commit, last + min)` clamped into `[last + min, last + max]`,
//! and an idle panel stretches to the `max` period.

use ldp_core::buffer::{FourCC, Modifier};
use ldp_display::atomic::AtomicRequest;
use ldp_display::backend::KmsBackend;
use ldp_display::commit::{CommitFlags, DstRect, SrcRect};
use ldp_display::events::DeviceEvent;
use ldp_display::fb::FbSpec;
use ldp_display::ids::{AnyId, ConnectorId, CrtcId, PlaneId};
use ldp_display::mode::Mode;
use ldp_display::props::{prop, PropValue};
use ldp_display::MockDevice;

const MIN: u64 = 1_000_000_000 / 144; // 6_944_444 ns
const MAX: u64 = 1_000_000_000 / 48; // 20_833_333 ns
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

/// Bring the pipeline up on CRTC 42 with VRR enabled (or not).
fn vrr_pipeline(dev: &mut MockDevice, vrr: bool) -> ldp_display::ids::FbId {
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
        .crtc_vrr(CRTC0, vrr)
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

fn flip_fb(dev: &mut MockDevice, fb: ldp_display::ids::FbId) {
    let req = AtomicRequest::new().flag(CommitFlags::PAGE_FLIP_EVENT).set(
        AnyId::Plane(PRIMARY),
        prop::FB_ID,
        PropValue::U64(u64::from(fb.raw())),
    );
    dev.commit(&req).expect("flip commits");
}

/// Advance exactly to each next event until a page flip matures.
///
/// Stepping (rather than bulk-advancing) keeps the commit instants on
/// known times, which is what the assertions below reason about.
fn next_flip(dev: &mut MockDevice) -> ldp_display::events::PageFlipEvent {
    loop {
        let at = dev
            .next_event_at()
            .expect("a live timeline always has a next event");
        for e in dev.advance_to(at) {
            if let DeviceEvent::PageFlip(f) = e {
                return f;
            }
        }
    }
}

#[test]
fn vrr_property_round_trips_through_the_catalog() {
    let mut dev = MockDevice::laptop_dual();
    assert!(!dev.vrr_enabled(CRTC0));
    vrr_pipeline(&mut dev, true);
    assert!(dev.vrr_enabled(CRTC0));
    // The toggle is itself a plain commit — no modeset needed.
    let off = AtomicRequest::new().crtc_vrr(CRTC0, false);
    dev.commit(&off).expect("vrr toggle applies");
    assert!(!dev.vrr_enabled(CRTC0));
    // Fixed-sync CRTCs reject nothing at the property level, but the
    // window stays absent: flips sit on the nominal grid.
    assert_eq!(dev.frame_period_ns(CRTC0), Some(NOMINAL));
}

#[test]
fn vrr_first_flip_lands_at_the_min_window() {
    let mut dev = MockDevice::laptop_dual();
    vrr_pipeline(&mut dev, true);
    // Committed at t=0 with last=0: earliest is max(0, MIN) = MIN.
    let flip = next_flip(&mut dev);
    assert_eq!(flip.timestamp.as_ns(), MIN, "early flip at last+min");
    assert_eq!(flip.sequence, 1);
}

#[test]
fn vrr_flip_too_early_waits_for_min() {
    let mut dev = MockDevice::laptop_dual();
    let fb = vrr_pipeline(&mut dev, true);
    // The first flip completes at MIN; a flip committed exactly at that
    // completion sequences off it: it cannot land before 2*MIN.
    let first = next_flip(&mut dev);
    assert_eq!(first.timestamp.as_ns(), MIN);
    flip_fb(&mut dev, fb);
    let second = next_flip(&mut dev);
    assert_eq!(
        second.timestamp.as_ns(),
        2 * MIN,
        "chained flip respects the min window"
    );
    assert_eq!(second.sequence, 2);
}

#[test]
fn vrr_late_commit_lands_at_commit_time() {
    let mut dev = MockDevice::laptop_dual();
    let fb = vrr_pipeline(&mut dev, true);
    // Let the first flip complete, idle to just before the next max
    // stretch, then commit: the flip lands at max(commit, last+min),
    // and with the panel idled past min the commit instant itself wins.
    let first = next_flip(&mut dev);
    let _ = dev.advance_ns(MAX - 1);
    let now = dev.now().as_ns();
    assert!(
        now > first.timestamp.as_ns() + MIN,
        "commit is past last+min"
    );
    flip_fb(&mut dev, fb);
    let second = next_flip(&mut dev);
    assert_eq!(
        second.timestamp.as_ns(),
        now,
        "late commit lands immediately"
    );
}

#[test]
fn vrr_idle_vblanks_stretch_to_max() {
    let mut dev = MockDevice::laptop_dual();
    vrr_pipeline(&mut dev, true);
    // The first flip completes at MIN, then the panel idles: the next
    // bare vblank sits at MIN + MAX (idle stretch), not MIN + NOMINAL.
    let first = next_flip(&mut dev);
    let last = first.timestamp.as_ns();
    assert_eq!(last, MIN);
    let next = dev
        .next_event_at()
        .expect("a live timeline always has a next event")
        .as_ns();
    assert_eq!(next, last + MAX, "idle vblank stretches to the max period");
    // Advancing exactly to it produces exactly one vblank at that time.
    let events = dev.advance_ns(MAX);
    let vblanks: Vec<_> = events
        .into_iter()
        .filter_map(|e| match e {
            DeviceEvent::Vblank(v) => Some(v),
            _ => None,
        })
        .collect();
    assert_eq!(vblanks.len(), 1);
    assert_eq!(vblanks[0].timestamp.as_ns(), last + MAX);
    assert!(vblanks[0].sequence >= 1);
}

#[test]
fn fixed_sync_stays_on_the_nominal_grid() {
    let mut dev = MockDevice::laptop_dual();
    let fb = vrr_pipeline(&mut dev, false);
    assert!(!dev.vrr_enabled(CRTC0));
    // Three flips in a row, each committed at the previous completion:
    // frames 1, 2, 3 on the strict grid.
    let a = next_flip(&mut dev);
    flip_fb(&mut dev, fb);
    let b = next_flip(&mut dev);
    flip_fb(&mut dev, fb);
    let c = next_flip(&mut dev);
    assert_eq!(a.timestamp.as_ns(), NOMINAL);
    assert_eq!(b.timestamp.as_ns(), 2 * NOMINAL);
    assert_eq!(c.timestamp.as_ns(), 3 * NOMINAL);
    // Vblanks between flips stay nominal too.
    let idle = dev.advance_ns(5 * NOMINAL);
    assert!(
        idle.iter().all(|e| match e {
            DeviceEvent::Vblank(v) => v.timestamp.as_ns() % NOMINAL == 0,
            _ => false,
        }),
        "every fixed-sync tick is on the grid"
    );
}

#[test]
fn two_crtcs_interleave_in_timeline_order() {
    use ldp_display::commit::CommitFlags as Flags;
    let mut dev = MockDevice::laptop_dual();
    // CRTC 42 on eDP (VRR), CRTC 43 on HDMI (fixed 60 Hz).
    let fb0 = dev
        .add_fb(
            &FbSpec::single(1920, 1080, FourCC::XRGB8888, 1, 1920 * 4, 0),
            Some(Modifier::LINEAR),
        )
        .unwrap();
    let fb1 = dev
        .add_fb(
            &FbSpec::single(1280, 720, FourCC::XRGB8888, 2, 1280 * 4, 0),
            Some(Modifier::LINEAR),
        )
        .unwrap();
    let req = AtomicRequest::new()
        .flag(Flags::ALLOW_MODESET)
        .flag(Flags::PAGE_FLIP_EVENT)
        .connector_bind(EDP, CRTC0)
        .crtc_active(CRTC0, true)
        .crtc_mode(CRTC0, &Mode::panel_1080p60())
        .crtc_vrr(CRTC0, true)
        .plane_on(
            PRIMARY,
            CRTC0,
            fb0,
            SrcRect::new(0, 0, 1920, 1080).unwrap(),
            DstRect::new(0, 0, 1920, 1080).unwrap(),
        )
        // HDMI-A-1 (93) offers 1080p60 and 720p60; drive it at 60 Hz.
        .connector_bind(ConnectorId::new(93).unwrap(), CrtcId::new(43).unwrap())
        .crtc_active(CrtcId::new(43).unwrap(), true)
        .crtc_mode(CrtcId::new(43).unwrap(), &Mode::panel_1080p60())
        .plane_on(
            PlaneId::new(53).unwrap(),
            CrtcId::new(43).unwrap(),
            fb1,
            SrcRect::new(0, 0, 1280, 720).unwrap(),
            DstRect::new(0, 0, 1280, 720).unwrap(),
        );
    dev.commit(&req).expect("dual pipeline commits");

    // CRTC 42's VRR flip lands at MIN; CRTC 43's fixed flip at NOMINAL.
    let events = dev.advance_ns(NOMINAL + MIN);
    let mut seen = Vec::new();
    for e in &events {
        if let DeviceEvent::PageFlip(f) = e {
            seen.push((f.crtc.raw(), f.timestamp.as_ns()));
        }
    }
    assert!(seen.contains(&(42, MIN)), "VRR flip is early: {seen:?}");
    assert!(
        seen.contains(&(43, NOMINAL)),
        "fixed flip is on grid: {seen:?}"
    );
    // Events arrive in time order per CRTC; across CRTCs the device
    // emits in resource order at equal timestamps.
    let mut times: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            DeviceEvent::PageFlip(f) => Some(f.timestamp.as_ns()),
            _ => None,
        })
        .collect();
    let mut sorted = times.clone();
    sorted.sort_unstable();
    assert_eq!(times, sorted, "flip events are emitted in time order");
    times.clear();
}

#[test]
fn vrr_trace_is_reproducible() {
    let run = || {
        let mut dev = MockDevice::laptop_dual();
        let fb = vrr_pipeline(&mut dev, true);
        let mut trace = Vec::new();
        for i in 0..4u64 {
            if i > 0 {
                // Commit each subsequent flip a fixed skew after the
                // previous completion.
                let _ = dev.advance_ns(MIN / 2);
                flip_fb(&mut dev, fb);
            }
            loop {
                let at = dev.next_event_at().expect("live timeline");
                let got = dev.advance_to(at);
                let done = got.iter().any(|e| matches!(e, DeviceEvent::PageFlip(_)));
                trace.extend(got.into_iter().map(|e| match e {
                    DeviceEvent::PageFlip(f) => (1u8, f.sequence, f.timestamp.as_ns()),
                    DeviceEvent::Vblank(v) => (0u8, v.sequence, v.timestamp.as_ns()),
                    _ => (2u8, 0, 0),
                }));
                if done {
                    break;
                }
            }
        }
        trace
    };
    assert_eq!(run(), run());
}
