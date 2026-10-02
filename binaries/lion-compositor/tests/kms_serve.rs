//! Phase 25 exit criteria — the real-KMS serve loop, CI-proven.
//!
//! The serve loop's device seam runs the *same* choreography on the
//! mock and the real backend; these tests pin the mock half plus the
//! real delivery surface:
//!
//! * **Mapped-store parity** — the identical client session driven
//!   through the shadow store (headless default) and the *mapped*
//!   store (real `mmap` regions, pitch-honoring writes — the DRM
//!   path's delivery vehicle, exercised here through anonymous
//!   mappings) produces byte-equal scanout, over two frames.
//! * **Teardown** — the world's applied disable commit releases the
//!   mock pipeline (plane, CRTC, connector), the same request
//!   sequence the DRM path submits on exit.
//! * **Serve-loop service** — device events outside any session's
//!   pump (hotplug) drain through `service_device_events` without
//!   moving the deterministic clock.
//! * **Honest DRM mode** — the binary's `--mode drm` on a machine
//!   without DRM nodes fails typed and loud, never silently serving
//!   headless.

mod testbench;

use std::io::Write;

use ldp_core::geometry::Rect;
use ldp_core::wire::Value;
use testbench::*;

use lion_compositor::server::CompositorConfig;
use lion_compositor::World;

/// A memfd pool carrying explicit pixel bytes.
fn pool_bytes(pixels: &[u8]) -> std::os::fd::OwnedFd {
    let fd = lion_compositor::sys::memfd("kms-serve").expect("memfd");
    let mut file = std::fs::File::from(fd);
    file.write_all(pixels).expect("write pixels");
    file.into()
}

/// 2x2 XRGB8888 (stride 8): red, green / blue, white.
fn quad_pixels() -> Vec<u8> {
    let mut v = Vec::new();
    for word in [0x00FF_0000u32, 0x0000_FF00, 0x0000_00FF, 0x00FF_FFFF] {
        v.extend_from_slice(&word.to_le_bytes());
    }
    v
}

/// 2x2 replacement content for the second frame: amber, cyan /
/// magenta, grey.
fn quad_pixels_two() -> Vec<u8> {
    let mut v = Vec::new();
    for word in [0x00FF_A000u32, 0x0000_FFA0, 0x00FF_00A0, 0x00A0_A0A0] {
        v.extend_from_slice(&word.to_le_bytes());
    }
    v
}

/// Drive the scripted two-frame session against a live compositor:
/// pool + buffer, attach, damage, commit, presented — then new pool
/// bytes, re-attach, damage, commit, presented.
fn drive_two_frame_session(tb: &Testbench) {
    let mut client = TestClient::connect(&tb.addr);
    let shm = client.bind("ldp.core.shm");
    client.wait_until(|c| c.records.iter().filter(|r| r.event == "format").count() >= 2);
    let compositor = client.bind("ldp.core.compositor");
    let surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("create_surface");

    // Frame one: request the presentation feedback, then commit.
    frame(&mut client, &surface, 1);
    let pool = create_pool(&mut client, &shm, pool_bytes(&quad_pixels()), 16);
    let buffer = create_buffer(&mut client, &pool, 0, 2, 2, 8, 0x3432_5258);
    attach(&mut client, &surface, &buffer);
    damage(&mut client, &surface, &[Rect::new(0, 0, 2, 2)]);
    commit(&mut client, &surface, 0xA001);
    client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "committed" && r.args[0] == Value::Uint32(0xA001))
    });
    client.wait_until(|c| c.records.iter().any(|r| r.event == "presented"));

    // Frame two: fresh content in a second pool, a new frame request.
    frame(&mut client, &surface, 2);
    let pool2 = create_pool(&mut client, &shm, pool_bytes(&quad_pixels_two()), 16);
    let buffer2 = create_buffer(&mut client, &pool2, 0, 2, 2, 8, 0x3432_5258);
    attach(&mut client, &surface, &buffer2);
    damage(&mut client, &surface, &[Rect::new(0, 0, 2, 2)]);
    commit(&mut client, &surface, 0xA002);
    client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "committed" && r.args[0] == Value::Uint32(0xA002))
    });
    client.wait_until(|c| c.records.iter().filter(|r| r.event == "presented").count() >= 2);
}

/// THE parity criterion: the mapped store (real mmap regions, the DRM
/// delivery path) and the shadow store (the headless oracle) produce
/// byte-equal scanout for the identical session — two frames deep.
#[test]
fn mapped_store_matches_shadow_store_byte_for_byte() {
    let shadow_tb = Testbench::start("kms-shadow");
    drive_two_frame_session(&shadow_tb);
    let shadow_scanout = shadow_tb.scanout();
    let shadow_frames = shadow_tb.frames();

    let mapped_tb = Testbench::start_with(
        "kms-mapped",
        CompositorConfig {
            scanout_mapped: true,
            ..CompositorConfig::default()
        },
    );
    drive_two_frame_session(&mapped_tb);
    let mapped_scanout = mapped_tb.scanout();
    let mapped_frames = mapped_tb.frames();

    // The same render work happened on both paths.
    assert_eq!(shadow_frames, mapped_frames, "frame counts match");
    assert_eq!(shadow_frames, 2, "two frames rendered");
    // Byte-for-byte: the whole 1920x1080 front buffer.
    assert_eq!(
        shadow_scanout.len(),
        mapped_scanout.len(),
        "scanout sizes match"
    );
    assert_eq!(
        shadow_scanout, mapped_scanout,
        "the mapped delivery is byte-equal"
    );
    // And the content is the second frame's quad (the known truth).
    assert_eq!(mapped_scanout[0], 0xFFFF_A000, "top-left amber");
    assert_eq!(mapped_scanout[1], 0xFF00_FFA0, "top-right cyan");
    assert_eq!(mapped_scanout[1920], 0xFFFF_00A0, "bottom-left magenta");
    assert_eq!(mapped_scanout[1921], 0xFFA0_A0A0, "bottom-right grey");
    assert_eq!(
        mapped_scanout[2], 0xFF00_0000,
        "the desktop behind is black"
    );
}

/// Teardown: the world's disable commit releases the whole mock
/// pipeline — the same request the DRM path submits on exit.
#[test]
fn teardown_disables_the_pipeline() {
    let tb = Testbench::start("kms-teardown");
    let mut client = TestClient::connect(&tb.addr);
    let shm = client.bind("ldp.core.shm");
    client.wait_until(|c| c.records.iter().filter(|r| r.event == "format").count() >= 2);
    let compositor = client.bind("ldp.core.compositor");
    let surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("create_surface");
    frame(&mut client, &surface, 1);
    let pool = create_pool(&mut client, &shm, pool_bytes(&quad_pixels()), 16);
    let buffer = create_buffer(&mut client, &pool, 0, 2, 2, 8, 0x3432_5258);
    attach(&mut client, &surface, &buffer);
    damage(&mut client, &surface, &[Rect::new(0, 0, 2, 2)]);
    commit(&mut client, &surface, 0xB001);
    client.wait_until(|c| c.records.iter().any(|r| r.event == "presented"));
    drop(client);

    // The applied state before: the pipeline serves.
    {
        let world = tb.shared.world.lock().expect("world lock");
        let mock = world.device.as_mock().expect("the mock driver");
        assert!(
            mock.crtc_mode_now(world.crtc().expect("lit")).is_some(),
            "mode active"
        );
        let connector = world.output().expect("lit after the session").connector.id;
        assert_eq!(
            mock.connector_binding(connector),
            Some(world.crtc().expect("lit"))
        );
        let plane = mock
            .plane_state(world.primary().expect("lit").plane)
            .expect("plane present");
        assert_eq!(
            plane.crtc,
            u64::from(world.crtc().expect("lit").raw()),
            "plane fed"
        );
    }
    // Teardown through the world (the serve loop's exit path).
    {
        let mut world = tb.shared.world.lock().expect("world lock");
        world.teardown().expect("the disable commit applies");
    }
    // The applied state after: everything released.
    {
        let world = tb.shared.world.lock().expect("world lock");
        let mock = world.device.as_mock().expect("the mock driver");
        let connector = world
            .output()
            .expect("teardown keeps the output model")
            .connector
            .id;
        assert_eq!(mock.connector_binding(connector), None, "connector unbound");
        assert_eq!(
            world
                .outputs
                .iter()
                .filter(|slot| mock.crtc_mode_now(slot.crtc).is_some())
                .count(),
            0,
            "CRTC off"
        );
        let plane = mock
            .plane_state(world.primary().expect("lit").plane)
            .expect("plane present");
        assert_eq!(plane.fb, 0, "plane's FB released");
        assert_eq!(plane.crtc, 0, "plane unfed");
    }
}

/// The serve loop's own device service: hotplug drains without moving
/// the deterministic clock (the zero-timeout doctrine).
#[test]
fn service_device_events_drains_hotplug_without_clock_motion() {
    let tb = Testbench::start("kms-service");
    let (before_ns, before_events) = {
        let world = tb.shared.world.lock().expect("world lock");
        (world.now().as_ns(), world.device_events)
    };
    // Inject a topology change on the disconnected DP-1 (id 92).
    {
        let mut world = tb.shared.world.lock().expect("world lock");
        world
            .device
            .as_mock_mut()
            .expect("the mock driver")
            .hotplug_connect(ldp_display::ConnectorId::new(92).unwrap(), vec![], vec![]);
    }
    {
        let mut world = tb.shared.world.lock().expect("world lock");
        world.service_device_events().expect("the service pass");
    }
    let (after_ns, after_events) = {
        let world = tb.shared.world.lock().expect("world lock");
        (world.now().as_ns(), world.device_events)
    };
    assert!(after_events > before_events, "the hotplug was serviced");
    assert_eq!(after_ns, before_ns, "the clock never moved");
}

/// The honest DRM mode: no nodes, a typed loud failure — the binary
/// never silently serves headless under an explicit `--mode drm`.
#[test]
fn drm_mode_fails_loudly_without_nodes() {
    use std::process::Command;
    let out = Command::new(env!("CARGO_BIN_EXE_lion-compositor"))
        .args(["--mode", "drm", "--quiet"])
        .output()
        .expect("run the compositor binary");
    assert!(
        !out.status.success(),
        "--mode drm must fail without a DRM node"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no DRM device could be opened"),
        "the honest reason, printed: {stderr}"
    );
    assert!(
        stderr.contains("no silent fallback"),
        "the explicit-mode doctrine, stated: {stderr}"
    );
}

/// The zero-risk pre-flight: `--probe` on a nodeless machine also
/// fails honestly (the rehearsal has nothing to rehearse on).
#[test]
fn probe_reports_honestly_without_nodes() {
    use std::process::Command;
    let out = Command::new(env!("CARGO_BIN_EXE_lion-compositor"))
        .args(["--probe", "--quiet"])
        .output()
        .expect("run the compositor binary");
    assert!(
        !out.status.success(),
        "--probe must fail without a DRM node"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no DRM device could be opened"),
        "the honest reason, printed: {stderr}"
    );
}

/// Keep the `World` import honest (the suite locks it directly).
#[allow(dead_code)]
fn _world_type_witness(_: &World) {}
