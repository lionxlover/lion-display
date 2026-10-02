//! Phase 31 exit criteria — adaptive sync in the running compositor.
//!
//! The VRR window was always probed and advertised; Phase 31 arms it:
//! `--vrr` carries `VRR_ENABLED` on every flip of a VRR-capable
//! output (the panel stretches inside its window — the mock's
//! timeline clamps exactly like the kernel), and the deadline
//! scheduler's window widens by the ldp-vrr policy's decision.

mod testbench;

use std::io::Write;

use ldp_core::geometry::Rect;
use testbench::*;

use lion_compositor::server::CompositorConfig;

/// A memfd pool carrying explicit pixel bytes.
fn pool_bytes(pixels: &[u8]) -> std::os::fd::OwnedFd {
    let fd = lion_compositor::sys::memfd("vrr").expect("memfd");
    let mut file = std::fs::File::from(fd);
    file.write_all(pixels).expect("write pixels");
    file.into()
}

fn start_vrr(tag: &str) -> Testbench {
    let config = CompositorConfig {
        socket: format!("lion-it-vrr-{tag}-{}", std::process::id()),
        vrr: true,
        ..CompositorConfig::default()
    };
    Testbench::start_with(tag, config)
}

#[test]
fn vrr_flips_arm_the_crtcs_enablement() {
    let tb = start_vrr("arm");
    let mut client = TestClient::connect(&tb.addr);
    let shm = client.bind("ldp.core.shm");
    let comp = client.bind("ldp.core.compositor");
    let surface = client
        .conn
        .create_object(&comp, "create_surface", vec![])
        .expect("surface");
    let pixels = [0x00FF_0000u32.to_le_bytes(); 4].concat();
    let pool = create_pool(&mut client, &shm, pool_bytes(&pixels), 16);
    let buffer = create_buffer(&mut client, &pool, 0, 2, 2, 8, 0x3432_5258);
    // One commit cycle. The VRR-aware presentation clock is Phase 39's
    // truth (see latency_session.rs: an adaptive-mode surface's
    // `presented` feedback reports the cadence the panel actually
    // ran); the ARMING and the scanout delivery are the Phase 31
    // truths pinned here.
    attach(&mut client, &surface, &buffer);
    damage(&mut client, &surface, &[Rect::new(0, 0, 2, 2)]);
    commit(&mut client, &surface, 0x3401);
    client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "committed" && r.args[0] == ldp_core::wire::Value::Uint32(0x3401))
    });
    // The primary (eDP's CRTC 42, the VRR-capable one) has VRR armed
    // on the device — the flips carried VRR_ENABLED.
    tb.world(|world| {
        let mock = world.device.as_mock().expect("the mock driver");
        let crtc = world.outputs[0].crtc;
        assert!(mock.vrr_enabled(crtc), "the primary's VRR is armed");
        // The scheduler's window widened by the ldp-vrr deadline
        // policy's decision: the panel's max stretch (the 48 Hz end
        // of the 48-144 window) minus the 60 Hz nominal — a late
        // commit may consume those 4.17 ms inside its window.
        assert_eq!(
            world.scene.scheduler.config().vrr_window_ns,
            20_833_333 - 16_666_666
        );
        // The scanout carries the window's ink — the VRR flip
        // delivered (the panel re-triggered at its minimum interval,
        // the earliest legal instant).
        let scanout = world.scanout_words().expect("lit");
        assert_eq!(
            scanout[0], 0xFFFF_0000,
            "the red quad scanned out under VRR"
        );
    });
    let _ = buffer;
}

#[test]
fn the_default_keeps_the_fixed_grid() {
    // No `--vrr`: the flips carry no VRR enablement — the Phase 25
    // bytes (the fixed nominal grid).
    let tb = Testbench::start("vrr-off");
    let mut client = TestClient::connect(&tb.addr);
    let shm = client.bind("ldp.core.shm");
    let comp = client.bind("ldp.core.compositor");
    let surface = client
        .conn
        .create_object(&comp, "create_surface", vec![])
        .expect("surface");
    let pixels = [0x00FF_0000u32.to_le_bytes(); 4].concat();
    let pool = create_pool(&mut client, &shm, pool_bytes(&pixels), 16);
    let buffer = create_buffer(&mut client, &pool, 0, 2, 2, 8, 0x3432_5258);
    frame(&mut client, &surface, 1);
    attach(&mut client, &surface, &buffer);
    damage(&mut client, &surface, &[Rect::new(0, 0, 2, 2)]);
    commit(&mut client, &surface, 0x3402);
    client.wait_until(|c| c.records.iter().any(|r| r.event == "presented"));
    tb.world(|world| {
        let mock = world.device.as_mock().expect("the mock driver");
        let crtc = world.outputs[0].crtc;
        assert!(!mock.vrr_enabled(crtc), "VRR is not armed by default");
    });
    let _ = buffer;
}
