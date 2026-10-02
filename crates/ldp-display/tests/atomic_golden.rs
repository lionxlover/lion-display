//! Phase 9 exit criteria — atomic-commit goldens against the mock KMS.
//!
//! Every test addresses the preset topology by its fixed ids (connectors
//! 91/92/93, CRTCs 42/43, planes 50-55) and asserts the *typed*
//! outcome of each commit, the all-or-nothing contract, and the exact
//! event trace the deterministic timeline emits.

#![allow(clippy::too_many_lines)]

use ldp_core::buffer::{FourCC, Modifier};
use ldp_core::time::Mono;
use ldp_display::atomic::AtomicRequest;
use ldp_display::backend::KmsBackend;
use ldp_display::commit::{CommitFlags, DstRect, SrcRect};
use ldp_display::error::{DisplayError, RejectReason};
use ldp_display::events::{DeviceEvent, OutFence};
use ldp_display::fb::FbSpec;
use ldp_display::ids::{AnyId, ConnectorId, CrtcId, PlaneId};
use ldp_display::mode::Mode;
use ldp_display::props::{prop, PropValue};
use ldp_display::MockDevice;

/// Const-constructible preset ids (0 is rejected by `new`, so the
/// constants are always valid).
macro_rules! preset_id {
    ($name:ident, $t:ty, $raw:literal) => {
        const $name: $t = match <$t>::new($raw) {
            Some(id) => id,
            None => panic!("preset id is nonzero"),
        };
    };
}
preset_id!(EDP, ConnectorId, 91);
preset_id!(HDMI, ConnectorId, 93);
preset_id!(CRTC0, CrtcId, 42);
preset_id!(CRTC1, CrtcId, 43);
preset_id!(PRIMARY, PlaneId, 50);

fn frame_fb(dev: &mut MockDevice) -> ldp_display::ids::FbId {
    dev.add_fb(
        &FbSpec::single(1920, 1080, FourCC::XRGB8888, 1, 1920 * 4, 0),
        Some(Modifier::LINEAR),
    )
    .expect("linear XRGB8888 fb registers")
}

/// The full pipeline bring-up request: bind, enable, mode, plane on.
fn bring_up(fb: ldp_display::ids::FbId) -> AtomicRequest {
    AtomicRequest::new()
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
        )
}

#[test]
fn golden_pipeline_bring_up_and_first_flip() {
    let mut dev = MockDevice::laptop_dual();
    let fb = frame_fb(&mut dev);
    let out = dev.commit(&bring_up(fb)).expect("pipeline commits");
    assert!(out.applied);
    assert!(out.out_fences.is_empty());

    // State is queryable through the trait surface.
    assert_eq!(dev.connector_binding(EDP), Some(CRTC0));
    let mode = dev.crtc_mode_now(CRTC0).expect("mode is live");
    assert_eq!(mode.refresh_millihz(), 60_000);
    let plane = dev.plane_state(PRIMARY).expect("plane exists");
    assert_eq!(plane.fb, u64::from(fb.raw()));
    assert_eq!(plane.crtc, u64::from(CRTC0.raw()));
    assert_eq!(
        dev.plane_kind_of(PRIMARY),
        Some(ldp_display::plane::PlaneType::Primary)
    );

    // 60 Hz: the first flip lands exactly one nominal frame after the
    // commit at t=0, sequence 1.
    assert_eq!(dev.frame_period_ns(CRTC0), Some(16_666_666));
    let events = dev.advance_ns(16_666_666);
    assert_eq!(events.len(), 1);
    match &events[0] {
        DeviceEvent::PageFlip(f) => {
            assert_eq!(f.crtc, CRTC0);
            assert_eq!(f.sequence, 1);
            assert_eq!(f.timestamp, Mono::from_ns(16_666_666));
            assert!(f.out_fence.is_none());
        }
        other => panic!("expected a page flip, got {other:?}"),
    }
    assert_eq!(dev.flip_count(CRTC0), 1);
}

#[test]
fn golden_page_flip_chain_is_deterministic() {
    // The same script over two fresh devices must produce byte-equal
    // traces — the Phase 9 reproducibility contract.
    let run = || {
        let mut dev = MockDevice::laptop_dual();
        let fb = frame_fb(&mut dev);
        dev.commit(&bring_up(fb)).unwrap();
        let mut trace = Vec::new();
        for i in 0..5u64 {
            let req = AtomicRequest::new()
                .flag(CommitFlags::PAGE_FLIP_EVENT)
                .set(
                    AnyId::Plane(PRIMARY),
                    prop::FB_ID,
                    PropValue::U64(u64::from(fb.raw())),
                )
                .set(AnyId::Plane(PRIMARY), prop::SRC_X, PropValue::U64(i << 16))
                .set(
                    AnyId::Plane(PRIMARY),
                    prop::SRC_W,
                    PropValue::U64((1920 - i) << 16),
                );
            dev.commit(&req).unwrap();
            // Step exactly to the next event, twice per frame boundary.
            for _ in 0..2 {
                let at = dev.next_event_at().expect("timeline is live");
                let mut got = dev.advance_to(at);
                trace.append(&mut got);
            }
        }
        trace
            .into_iter()
            .map(|e| match e {
                DeviceEvent::PageFlip(f) => (1u8, f.crtc.raw(), f.sequence, f.timestamp.as_ns()),
                DeviceEvent::Vblank(v) => (0u8, v.crtc.raw(), v.sequence, v.timestamp.as_ns()),
                _ => (2u8, 0, 0, 0),
            })
            .collect::<Vec<_>>()
    };
    let a = run();
    let b = run();
    assert_eq!(a, b);
    assert!(
        a.len() >= 5,
        "five flips must mature, got {} events",
        a.len()
    );
    // Every flip lands on the nominal 60 Hz grid (vblanks may
    // interleave between commits, so flips need not be consecutive
    // frames — but they are always grid points).
    for (kind, _, _, ts) in &a {
        if *kind == 1 {
            assert_eq!(ts % 16_666_666, 0, "flip off the nominal grid: {ts}");
        }
    }
}

#[test]
fn rejection_matrix_covers_the_typed_taxonomy() {
    let mut dev = MockDevice::laptop_dual();
    let fb = frame_fb(&mut dev);

    let reason_of = |req: &AtomicRequest, dev: &mut MockDevice| match dev.commit(req) {
        Err(DisplayError::AtomicReject { reason, .. }) => reason,
        other => panic!("expected a typed reject, got {other:?}"),
    };

    // Malformed: duplicate set of the same property.
    let dup = AtomicRequest::new()
        .crtc_active(CRTC0, true)
        .crtc_active(CRTC0, false);
    assert_eq!(reason_of(&dup, &mut dev), RejectReason::Malformed);

    // Unknown object: a CRTC id the device never had.
    let ghost = AtomicRequest::new().crtc_active(CrtcId::new(999).unwrap(), true);
    assert_eq!(reason_of(&ghost, &mut dev), RejectReason::UnknownObject);

    // Unknown property: a name no catalog carries.
    let weird = AtomicRequest::new().set(AnyId::Crtc(CRTC0), "NOT_A_PROP", PropValue::U64(1));
    assert_eq!(reason_of(&weird, &mut dev), RejectReason::UnknownProperty);

    // Immutable: the EDID blob property rejects writes.
    let edid = AtomicRequest::new().set(AnyId::Connector(EDP), prop::EDID, PropValue::U64(1));
    assert_eq!(reason_of(&edid, &mut dev), RejectReason::Immutable);

    // Property type: a boolean payload into a range property.
    let bad_type =
        AtomicRequest::new().set(AnyId::Connector(EDP), prop::MAX_BPC, PropValue::Bool(true));
    assert_eq!(reason_of(&bad_type, &mut dev), RejectReason::PropertyType);

    // Value out of range: max bpc 99 into [6, 16].
    let oor = AtomicRequest::new().set(AnyId::Connector(EDP), prop::MAX_BPC, PropValue::U64(99));
    assert_eq!(reason_of(&oor, &mut dev), RejectReason::PropertyType);

    // Disconnected connector: binding DP-1 (preset leaves it empty).
    let dp = ConnectorId::new(92).unwrap();
    let disc = AtomicRequest::new()
        .flag(CommitFlags::ALLOW_MODESET)
        .connector_bind(dp, CRTC0);
    assert_eq!(
        reason_of(&disc, &mut dev),
        RejectReason::ConnectorDisconnected
    );

    // Bring the pipeline up; the rest of the matrix mutates around it.
    dev.commit(&bring_up(fb)).unwrap();

    // NeedsModeset: a mode change without ALLOW_MODESET.
    let remode = AtomicRequest::new()
        .flag(CommitFlags::PAGE_FLIP_EVENT)
        .crtc_mode(CRTC0, &Mode::panel_1080p_59_94());
    assert_eq!(reason_of(&remode, &mut dev), RejectReason::NeedsModeset);

    // Busy: NONBLOCK while a flip is pending on the touched CRTC.
    let busy = AtomicRequest::new().flag(CommitFlags::NONBLOCK).set(
        AnyId::Plane(PRIMARY),
        prop::ZPOS,
        PropValue::U64(3),
    );
    assert_eq!(reason_of(&busy, &mut dev), RejectReason::Busy);

    // Plane not possible: plane 50 cannot feed CRTC 43.
    let theft = AtomicRequest::new()
        .flag(CommitFlags::ALLOW_MODESET)
        .plane_on(
            PRIMARY,
            CRTC1,
            fb,
            SrcRect::new(0, 0, 64, 64).unwrap(),
            DstRect::new(0, 0, 64, 64).unwrap(),
        );
    assert_eq!(reason_of(&theft, &mut dev), RejectReason::PlaneNotPossible);

    // Format unsupported: NV12 on plane 53 (RGB-only caps). The fb
    // store accepts a well-formed two-plane NV12 spec.
    let mut nv12_spec = FbSpec::single(64, 64, FourCC::NV12, 1, 64, 0);
    nv12_spec.handles[1] = 2;
    nv12_spec.pitches[1] = 64;
    nv12_spec.offsets[1] = 64 * 64;
    let nv12 = dev
        .add_fb(&nv12_spec, Some(Modifier::LINEAR))
        .expect("the fb store itself accepts NV12");
    let rgb53 = PlaneId::new(53).unwrap();
    let fmt = AtomicRequest::new().plane_on(
        rgb53,
        CRTC1,
        nv12,
        SrcRect::new(0, 0, 64, 64).unwrap(),
        DstRect::new(0, 0, 64, 64).unwrap(),
    );
    assert_eq!(reason_of(&fmt, &mut dev), RejectReason::FormatUnsupported);

    // Modifier unsupported: XRGB8888+INTEL_X on plane 53.
    let tiled = dev
        .add_fb(
            &FbSpec::single(64, 64, FourCC::XRGB8888, 1, 256, 0),
            Some(Modifier::INTEL_X),
        )
        .unwrap();
    // A linear twin for the checks that must reach past the modifier
    // stage (bounds and zero-size run after format/modifier).
    let linear64 = dev
        .add_fb(
            &FbSpec::single(64, 64, FourCC::XRGB8888, 1, 256, 0),
            Some(Modifier::LINEAR),
        )
        .unwrap();
    let modreq = AtomicRequest::new().plane_on(
        rgb53,
        CRTC1,
        tiled,
        SrcRect::new(0, 0, 64, 64).unwrap(),
        DstRect::new(0, 0, 64, 64).unwrap(),
    );
    assert_eq!(
        reason_of(&modreq, &mut dev),
        RejectReason::ModifierUnsupported
    );

    // Source out of bounds: crop wider than the framebuffer.
    let over = AtomicRequest::new().plane_on(
        rgb53,
        CRTC1,
        linear64,
        SrcRect::new(0, 0, 65, 64).unwrap(),
        DstRect::new(0, 0, 65, 64).unwrap(),
    );
    assert_eq!(reason_of(&over, &mut dev), RejectReason::SrcOutOfBounds);

    // Zero size: raw property writes bypass the builder's guard.
    let zero = AtomicRequest::new()
        .set(
            AnyId::Plane(rgb53),
            prop::CRTC_ID,
            PropValue::U64(u64::from(CRTC1.raw())),
        )
        .set(
            AnyId::Plane(rgb53),
            prop::FB_ID,
            PropValue::U64(u64::from(linear64.raw())),
        )
        .set(AnyId::Plane(rgb53), prop::SRC_W, PropValue::U64(0))
        .set(AnyId::Plane(rgb53), prop::SRC_H, PropValue::U64(64 << 16))
        .set(AnyId::Plane(rgb53), prop::CRTC_W, PropValue::U64(64))
        .set(AnyId::Plane(rgb53), prop::CRTC_H, PropValue::U64(64));
    assert_eq!(reason_of(&zero, &mut dev), RejectReason::ZeroSize);

    // Invalid in-fence: an fd the device never registered.
    let fence = AtomicRequest::new().plane_in_fence(PRIMARY, 77);
    assert_eq!(reason_of(&fence, &mut dev), RejectReason::InvalidInFence);

    // FB not found.
    let ghostfb =
        AtomicRequest::new().set(AnyId::Plane(PRIMARY), prop::FB_ID, PropValue::U64(9999));
    assert_eq!(reason_of(&ghostfb, &mut dev), RejectReason::FbNotFound);

    // Mode not supported: a mode the sink never offered.
    let odd = AtomicRequest::new()
        .flag(CommitFlags::ALLOW_MODESET)
        .connector_bind(HDMI, CRTC1)
        .crtc_active(CRTC1, true)
        .crtc_mode(CRTC1, &Mode::panel_1440p144())
        .plane_on(
            PlaneId::new(53).unwrap(),
            CRTC1,
            linear64,
            SrcRect::new(0, 0, 64, 64).unwrap(),
            DstRect::new(0, 0, 64, 64).unwrap(),
        );
    assert_eq!(reason_of(&odd, &mut dev), RejectReason::ModeNotSupported);

    // Missing connector: an active CRTC with no binding.
    let orphan = AtomicRequest::new()
        .flag(CommitFlags::ALLOW_MODESET)
        .crtc_active(CRTC1, true)
        .crtc_mode(CRTC1, &Mode::panel_1080p60());
    assert_eq!(reason_of(&orphan, &mut dev), RejectReason::MissingConnector);

    // Blob not found: MODE_ID pointing nowhere.
    let ghost_blob = AtomicRequest::new()
        .flag(CommitFlags::ALLOW_MODESET)
        .connector_bind(HDMI, CRTC1)
        .crtc_active(CRTC1, true)
        .crtc_mode_blob(CRTC1, ldp_display::ids::BlobId::new(2999).unwrap());
    assert_eq!(reason_of(&ghost_blob, &mut dev), RejectReason::BlobNotFound);
}

#[test]
fn commits_are_all_or_nothing() {
    let mut dev = MockDevice::laptop_dual();
    let fb = frame_fb(&mut dev);
    dev.commit(&bring_up(fb)).unwrap();
    let before = dev.plane_state(PRIMARY).unwrap();

    // A request whose *last* op is invalid must not apply its earlier
    // ops either: ZPOS writes before an unknown property.
    let torn = AtomicRequest::new()
        .set(AnyId::Plane(PRIMARY), prop::ZPOS, PropValue::U64(7))
        .set(AnyId::Plane(PRIMARY), "NOPE", PropValue::U64(1));
    assert!(dev.commit(&torn).is_err());
    assert_eq!(dev.plane_state(PRIMARY).unwrap().zpos, before.zpos);
}

#[test]
fn test_only_validates_but_leaves_no_residue() {
    let mut dev = MockDevice::laptop_dual();
    let fb = frame_fb(&mut dev);
    let probe = bring_up(fb).flag(CommitFlags::TEST_ONLY);

    let out = dev.commit(&probe).expect("the pipeline shape is valid");
    assert!(!out.applied);
    assert_eq!(dev.connector_binding(EDP), None);
    assert!(dev.crtc_mode_now(CRTC0).is_none());
    // A TEST_ONLY request that would fail validation still fails
    // (an acquire fence the device never registered).
    let bad = bring_up(fb)
        .flag(CommitFlags::TEST_ONLY)
        .plane_in_fence(PRIMARY, 77);
    let err = dev.commit(&bad).unwrap_err();
    assert!(matches!(
        err,
        DisplayError::AtomicReject {
            reason: RejectReason::InvalidInFence,
            ..
        }
    ));
    // And the real commit afterwards still works.
    dev.commit(&bring_up(fb))
        .expect("device state is untouched");
}

#[test]
fn out_fence_lifecycle() {
    let mut dev = MockDevice::laptop_dual();
    let fb = frame_fb(&mut dev);
    let req = bring_up(fb).crtc_out_fence(CRTC0);
    let out = dev.commit(&req).expect("out-fence requests commit");
    assert_eq!(out.out_fences.len(), 1);
    assert_eq!(out.out_fences[0].0, CRTC0);
    let OutFence::Token(token) = &out.out_fences[0].1 else {
        panic!("mock mints tokens");
    };

    // Not signaled before the flip lands; signaled at the vblank.
    assert!(!dev.out_fence_ready(*token));
    assert!(!dev
        .advance_ns(16_666_665)
        .iter()
        .any(|e| matches!(e, DeviceEvent::PageFlip(_))));
    assert!(!dev.out_fence_ready(*token));
    let events = dev.advance_ns(1);
    assert!(events.iter().any(|e| matches!(e, DeviceEvent::PageFlip(_))));
    assert!(dev.out_fence_ready(*token));
    assert_eq!(dev.out_fence_time(*token), Some(Mono::from_ns(16_666_666)));
}

#[test]
fn in_fence_holds_the_flip_until_ready() {
    let mut dev = MockDevice::laptop_dual();
    let fb = frame_fb(&mut dev);
    // The acquire fence matures a frame and a half after the commit.
    dev.add_in_fence(7, Mono::from_ns(16_666_666 + 8_000_000));
    let req = bring_up(fb).plane_in_fence(PRIMARY, 7);
    dev.commit(&req).expect("fenced pipeline commits");

    // Past the nominal vblank: nothing fires (fence not ready).
    let events = dev.advance_ns(16_666_666);
    assert!(events.is_empty(), "fence holds the queue: {events:?}");
    assert_eq!(dev.pending_flips(CRTC0), 1);

    // At fence readiness the flip completes with the fence timestamp.
    let events = dev.advance_ns(8_000_000);
    assert_eq!(events.len(), 1);
    match &events[0] {
        DeviceEvent::PageFlip(f) => {
            assert_eq!(f.timestamp, Mono::from_ns(24_666_666));
            assert_eq!(f.sequence, 1);
        }
        other => panic!("expected the gated flip, got {other:?}"),
    }
}

#[test]
fn blob_and_fb_lifecycles() {
    let mut dev = MockDevice::laptop_dual();

    // Blob create/read/destroy through the trait surface.
    let payload = Mode::panel_1080p60().to_blob().to_vec();
    let blob = dev.create_blob(&payload).expect("blob mints");
    assert_eq!(dev.blob(blob.id).expect("blob reads"), payload);
    dev.destroy_blob(blob.id).expect("blob destroys");
    assert!(dev.destroy_blob(blob.id).is_err(), "double destroy fails");

    // FB register/unregister, with modifier tracking.
    let fb = dev
        .add_fb(
            &FbSpec::single(64, 64, FourCC::ARGB8888, 3, 256, 0),
            Some(Modifier::LINEAR),
        )
        .expect("fb registers");
    dev.rm_fb(fb).expect("fb unregisters");
    assert!(dev.rm_fb(fb).is_err(), "double unregister fails");

    // Malformed FB specs fail before touching the store.
    assert!(dev
        .add_fb(&FbSpec::single(0, 64, FourCC::XRGB8888, 1, 4, 0), None)
        .is_err());
}

#[test]
fn deactivation_drops_pending_flips_and_dpms_writes() {
    let mut dev = MockDevice::laptop_dual();
    let fb = frame_fb(&mut dev);
    dev.commit(&bring_up(fb)).unwrap();
    assert_eq!(dev.pending_flips(CRTC0), 1);

    // DPMS write on the bound connector lands without a modeset.
    let dpms = AtomicRequest::new().connector_dpms(EDP, ldp_display::commit::DpmsState::Off);
    dev.commit(&dpms).expect("dpms write applies");

    // Full teardown: plane off + CRTC off + unbind, with ALLOW_MODESET.
    let teardown = AtomicRequest::new()
        .flag(CommitFlags::ALLOW_MODESET)
        .plane_off(PRIMARY)
        .crtc_active(CRTC0, false)
        .connector_unbind(EDP);
    dev.commit(&teardown).expect("teardown applies");
    assert_eq!(dev.pending_flips(CRTC0), 0, "deactivation cancels flips");
    assert!(dev.crtc_mode_now(CRTC0).is_none());
    assert_eq!(dev.connector_binding(EDP), None);
    // A CRTC with no timeline yields no events however far time runs.
    assert!(dev.advance_ns(100_000_000).is_empty());
}

#[test]
fn hotplug_reprobe_round_trip() {
    let mut dev = MockDevice::laptop_dual();
    let dp = ConnectorId::new(92).unwrap();
    // A monitor lands on DP-1; the event surfaces on the next drain.
    dev.hotplug_connect(
        dp,
        vec![Mode::panel_1080p60()],
        ldp_display::edid::EdidIdentity::synthesize("DEL", 0xA305, 9, "Dock Panel", 2026).to_vec(),
    );
    let events = dev.advance_ns(0);
    assert!(matches!(events.as_slice(), [DeviceEvent::Hotplug(h)] if h.connector == dp));

    let info = dev.connector_info(dp).expect("connector lives");
    assert_eq!(
        info.status,
        ldp_display::connector::ConnectorStatus::Connected
    );
    let identity = info.identity().expect("edid parses");
    assert_eq!(identity.manufacturer, "DEL");
    assert_eq!(identity.monitor_name, "Dock Panel");
    // best_mode now resolves through the standard policy.
    assert_eq!(info.best_mode().map(Mode::refresh_millihz), Some(60_000));

    // Unplug clears it again.
    dev.hotplug_disconnect(dp);
    assert!(matches!(
        dev.advance_ns(0).as_slice(),
        [DeviceEvent::Hotplug(h)] if h.connector == dp
    ));
    let info = dev.connector_info(dp).unwrap();
    assert_eq!(
        info.status,
        ldp_display::connector::ConnectorStatus::Disconnected
    );
    assert!(info.modes.is_empty());
}

#[test]
fn object_safety_polymorphism() {
    // The whole point of the backend seam: one function, any device.
    fn describe(backend: &dyn KmsBackend) -> (String, usize, usize, usize) {
        let version = backend.version().expect("version");
        let topo = backend.topology().expect("topology");
        (
            version.name,
            topo.connectors.len(),
            topo.crtcs.len(),
            topo.planes.len(),
        )
    }
    let dev: &dyn KmsBackend = &MockDevice::laptop_dual();
    assert_eq!(describe(dev), ("mockdrm".into(), 3, 2, 6));
}
