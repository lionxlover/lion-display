//! Phase 25 exit criteria — the serve choreography, proven applied on
//! the mock device.
//!
//! The rehearsal (Phase 24) validated the enable request under
//! `TEST_ONLY` and unwound; the serve loop *applies* it and keeps
//! serving. These tests pin the applied-state evidence: after
//! [`ldp_display::serve::enable`] the connector is bound, the CRTC
//! runs the chosen mode, the primary plane scans out the committed
//! framebuffer; after [`ldp_display::serve::disable`] all three are
//! released. The same request sequence runs for real on `DrmBackend`
//! — this is the CI half of that proof.
//!
//! The mapping half: [`ldp_display::drm::sys::anon_mapping`] mints a
//! real `mmap` region with the dumb-buffer lifetime discipline, so
//! the pitch-honoring delivery pass exercises genuine mapped memory
//! (write, read-back, drop) without a DRM node.

use core::time::Duration;

use std::os::fd::AsRawFd;

use ldp_core::buffer::{FourCC, Modifier};
use ldp_display::driver::DisplayDriver;
use ldp_display::events::DeviceEvent;
use ldp_display::serve::{self, Pipeline};
use ldp_display::{FbSpec, KmsBackend, MockDevice};

/// Register a mode-sized XRGB framebuffer (GEM handle 1) and return
/// its id.
fn registered_fb(dev: &mut MockDevice) -> ldp_display::FbId {
    let pipeline = serve::select_pipeline(dev).unwrap();
    let (w, h) = (pipeline.width(), pipeline.height());
    dev.add_fb(
        &FbSpec::single(w, h, FourCC::XRGB8888, 1, w * 4, 0),
        Some(Modifier::LINEAR),
    )
    .unwrap()
}

#[test]
fn enable_applies_the_full_pipeline() {
    let mut dev = MockDevice::laptop_dual();
    let pipeline = serve::select_pipeline(&dev).expect("eDP-1 present");
    let fb = registered_fb(&mut dev);
    serve::enable(&mut dev, &pipeline, fb).expect("the applied enable commits");

    // Applied evidence — TEST_ONLY would have left every one of these
    // untouched.
    assert_eq!(
        dev.connector_binding(pipeline.connector),
        Some(pipeline.crtc)
    );
    assert_eq!(
        dev.crtc_mode_now(pipeline.crtc).expect("mode active"),
        pipeline.mode
    );
    let plane = dev.plane_state(pipeline.plane).expect("plane present");
    assert_eq!(plane.fb, u64::from(fb.raw()));
    assert_eq!(plane.crtc, u64::from(pipeline.crtc.raw()));
    assert_eq!(plane.src_w, u64::from(pipeline.width()) << 16);
    assert_eq!(plane.crtc_w, u64::from(pipeline.width()));

    // The flip-landing discipline: the enable flip is in flight, a
    // driver wait lands it at the first vblank.
    assert_eq!(dev.pending_flips(pipeline.crtc), 1);
    let events = dev.wait_events(None).unwrap();
    assert!(events
        .iter()
        .any(|e| matches!(e, DeviceEvent::PageFlip(f) if f.crtc == pipeline.crtc)));
    assert_eq!(dev.flip_count(pipeline.crtc), 1);
}

#[test]
fn disable_releases_the_pipeline() {
    let mut dev = MockDevice::laptop_dual();
    let pipeline = serve::select_pipeline(&dev).unwrap();
    let fb = registered_fb(&mut dev);
    serve::enable(&mut dev, &pipeline, fb).unwrap();
    // Land the bring-up flip so the pipeline is steady, then disable.
    let _ = dev.wait_events(None).unwrap();

    serve::disable(&mut dev, &pipeline).expect("the applied disable commits");

    assert_eq!(dev.connector_binding(pipeline.connector), None);
    assert_eq!(dev.crtc_mode_now(pipeline.crtc), None);
    let plane = dev.plane_state(pipeline.plane).unwrap();
    assert_eq!(plane.fb, 0);
    assert_eq!(plane.crtc, 0);
}

#[test]
fn select_pipeline_prefers_the_connected_panel_and_its_preferred_mode() {
    let dev = MockDevice::laptop_dual();
    let Pipeline {
        connector,
        crtc,
        plane,
        mode,
    } = serve::select_pipeline(&dev).unwrap();
    // The preset's eDP-1 (connector 91) on CRTC 42 with the 1080p60
    // preferred mode, scanned out by a primary plane.
    assert_eq!(connector.raw(), 91);
    assert_eq!(crtc.raw(), 42);
    assert_eq!(
        (u32::from(mode.hdisplay), u32::from(mode.vdisplay)),
        (1920, 1080)
    );
    let info = dev.plane_info(plane).unwrap();
    assert_eq!(info.kind, ldp_display::PlaneType::Primary);
}

#[test]
fn anon_mapping_round_trips_a_frame_with_pitch() {
    // A real mmap region, the real delivery pass, the real reader.
    let (w, h) = (16usize, 9usize);
    let pitch = w * 4 + 16; // kernel-style padded pitch
    let mut map = ldp_display::drm::sys::anon_mapping(pitch * h).unwrap();
    assert_eq!(map.len(), pitch * h);
    let frame: Vec<u32> = (0..w * h).map(|i| 0x0A00_0000 + i as u32).collect();
    serve::write_frame_rows(map.as_mut_slice(), pitch, w, h, &frame);
    let back = serve::read_frame_rows(map.as_slice(), pitch, w, h);
    assert_eq!(back, frame);
    // Drop unmaps without complaint — the lifetime discipline.
    drop(map);
}

#[test]
fn poll_reports_readable_fds_honestly() {
    // /dev/null is permanently readable (EOF counts for POLLIN): the
    // poll must report it immediately, and a zero timeout must not
    // hang on it.
    let nul = std::fs::File::open("/dev/null").unwrap();
    assert!(ldp_display::drm::sys::poll_readable(nul.as_raw_fd(), 0).unwrap());
}

#[test]
fn monotonic_now_advances() {
    let a = ldp_display::drm::sys::monotonic_now().unwrap();
    let b = ldp_display::drm::sys::monotonic_now().unwrap();
    assert!(b.as_ns() >= a.as_ns(), "CLOCK_MONOTONIC never regresses");
}

#[test]
fn zero_timeout_wait_drains_hotplug_without_moving_the_clock() {
    let mut dev = MockDevice::laptop_dual();
    let before = dev.now();
    // Inject a topology event on the disconnected DP-1 connector
    // (id 92 in the preset).
    let dp = ldp_display::ConnectorId::new(92).unwrap();
    dev.hotplug_connect(dp, vec![], vec![]);
    let events = dev.wait_events(Some(Duration::ZERO)).unwrap();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, DeviceEvent::Hotplug(h) if h.connector == dp)),
        "the queued hotplug drains: {events:?}"
    );
    // And the clock did not move.
    assert_eq!(dev.now(), before);
}
