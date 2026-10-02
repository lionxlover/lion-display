//! Phase 41 exit criteria — the quirk ledger, CI-proven.
//!
//! The comparison row's named remainder ("the multi-year driver
//! quirk table, not the mechanism") is answered with the *structure*
//! the decades fill: the effective floor (`--vrr-floor N` — the
//! honesty knob for panels whose advertised range flickers at the
//! bottom), the per-output CSV grammar (the `--scale` mirror), the
//! uniform collapse (`--vrr-uniform` — the mixed-desktop escape for
//! cross-CRTC clock coupling), each pinned end to end over the real
//! socket against the mock panel's own advertised 48-144 Hz window.
//!
//! The mock's device-side window keeps its advertised truth (the
//! hardware's claim, exactly where the giants' overrides live — in
//! the display stack above the driver, never rewriting it): what the
//! floor clamps is every *consumer* — the scheduler's widened
//! deadline, the `output.vrr` advertisement clients pace against, the
//! LFC cadence's stretch ceiling.

mod testbench;

use std::io::Write;

use ldp_client::Proxy;
use ldp_core::geometry::Rect;
use ldp_core::wire::Value;
use testbench::*;

use lion_compositor::server::{Compositor, CompositorConfig};

/// A memfd pool carrying explicit pixel bytes.
fn pool_bytes(pixels: &[u8]) -> std::os::fd::OwnedFd {
    let fd = lion_compositor::sys::memfd("vrrfloor").expect("memfd");
    let mut file = std::fs::File::from(fd);
    file.write_all(pixels).expect("write pixels");
    file.into()
}

/// The mock panel's own advertisement (integer nanoseconds).
const AD_MIN_NS: u64 = 1_000_000_000 / 144;
const AD_MAX_NS: u64 = 1_000_000_000 / 48;
/// The 60 Hz mode's period — the grid the deadline scheduler paces.
const NOMINAL_NS: u64 = 16_666_666;

/// Bring up a VRR compositor under a floor doctrine: `floors` is the
/// CSV the operator typed (a lone rate blankets every output).
fn start_vrr_floor(tag: &str, floors: &[u32]) -> Testbench {
    let config = CompositorConfig {
        socket: format!("lion-it-vrrfloor-{tag}-{}", std::process::id()),
        vrr: true,
        vrr_floor: (floors.len() == 1).then_some(floors[0]),
        vrr_floors: if floors.len() == 1 {
            Vec::new()
        } else {
            floors.to_vec()
        },
        ..CompositorConfig::default()
    };
    Testbench::start_with(tag, config)
}

/// One commit cycle: a red quad onto the primary.
fn commit_red_quad(client: &mut TestClient, shm: &Proxy, comp: &Proxy, cookie: u32) {
    let surface = client
        .conn
        .create_object(comp, "create_surface", vec![])
        .expect("surface");
    let pixels = [0x00FF_0000u32.to_le_bytes(); 4].concat();
    let pool = create_pool(client, shm, pool_bytes(&pixels), 16);
    let buffer = create_buffer(client, &pool, 0, 2, 2, 8, 0x3432_5258);
    attach(client, &surface, &buffer);
    damage(client, &surface, &[Rect::new(0, 0, 2, 2)]);
    commit(client, &surface, cookie);
    client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "committed" && r.args[0] == Value::Uint32(cookie))
    });
}

#[test]
fn the_floor_clamps_every_consumer_of_the_window() {
    // `--vrr --vrr-floor 57`: the panel advertises 48-144 Hz but
    // flickers below 57 — the operator's honest floor.
    let floor_ns = 1_000_000_000 / 57;
    let tb = start_vrr_floor("clamp", &[57]);
    let mut client = TestClient::connect(&tb.addr);
    let shm = client.bind("ldp.core.shm");
    let comp = client.bind("ldp.core.compositor");
    let _out = client.bind("ldp.core.output");
    // Drain every bind's burst (the collector's `bind` waits only for
    // the first "bound" — later binds return immediately, their
    // advertisements still queued).
    client.sync();

    // The advertisement clients see is the *effective* window: the
    // minimum rate is the floor (57,000 millihz), the maximum the
    // panel's own (144,000) — the operator narrows the stretch they
    // do not trust, never the panel's own slip floor.
    let vrr = client.last("vrr");
    assert_eq!(vrr.args[0], Value::Uint32(57_000), "the floor's rate");
    assert_eq!(
        vrr.args[1],
        Value::Uint32(144_000),
        "the panel's own ceiling"
    );

    // The scheduler's widened deadline shrank by exactly the
    // flickering band: the effective stretch a late commit may
    // consume, no longer the advertised one.
    tb.world(|world| {
        assert_eq!(
            world.scene.scheduler.config().vrr_window_ns,
            floor_ns - NOMINAL_NS
        );
        assert_eq!(
            world.outputs[0].output.vrr,
            Some((AD_MIN_NS, floor_ns)),
            "the slot's window is the effective one"
        );
        // The audit trail the startup line prints: the clamp, from
        // the advertised stretch to the floor's.
        assert_eq!(
            world.vrr_floor_outcomes,
            vec![ldp_vrr::FloorOutcome::Clamped {
                from_max_ns: AD_MAX_NS,
                to_max_ns: floor_ns,
            }]
        );
        assert!(world.vrr_quirk_report().contains("17543859 ns"));
    });

    // The device-side truth is untouched (the hardware's own claim —
    // the mock's timeline still stretches inside its advertised
    // window; the giants' overrides live above the driver), and the
    // CRTC still arms: the floor is scheduling honesty, not a refusal
    // to serve.
    commit_red_quad(&mut client, &shm, &comp, 0x3401);
    tb.world(|world| {
        let mock = world.device.as_mock().expect("the mock driver");
        let crtc = world.outputs[0].crtc;
        assert!(mock.vrr_enabled(crtc), "the floor still arms VRR");
        assert_eq!(
            mock.vrr_window(crtc),
            Some((AD_MIN_NS, AD_MAX_NS)),
            "the device's own advertisement stands"
        );
    });
}

#[test]
fn a_floor_below_the_advertised_minimum_is_a_no_op() {
    // 40 Hz is below the advertised 48 Hz minimum: the panel already
    // claims a deeper stretch than the operator asks — the window
    // stands (the Phase 31 bytes).
    let tb = start_vrr_floor("noop", &[40]);
    let mut client = TestClient::connect(&tb.addr);
    let _out = client.bind("ldp.core.output");
    let vrr = client.last("vrr");
    assert_eq!(vrr.args[0], Value::Uint32(48_000));
    assert_eq!(vrr.args[1], Value::Uint32(144_000));
    tb.world(|world| {
        assert_eq!(
            world.scene.scheduler.config().vrr_window_ns,
            AD_MAX_NS - NOMINAL_NS
        );
        assert!(matches!(
            world.vrr_floor_outcomes.first(),
            Some(ldp_vrr::FloorOutcome::BelowAdvertised)
        ));
    });
}

#[test]
fn the_per_output_passthrough_lets_the_window_stand() {
    // `--vrr-floor 0,57`: output 0's entry is the passthrough — its
    // advertised window stands untouched (the grammar's per-output
    // `0`, the `--scale` mirror); the stretch rule sends 57 only to
    // the outputs past the list's end (none here — the mock's second
    // output carries no window at all).
    let tb = start_vrr_floor("passthrough", &[0, 57]);
    let mut client = TestClient::connect(&tb.addr);
    let _out = client.bind("ldp.core.output");
    let vrr = client.last("vrr");
    assert_eq!(vrr.args[0], Value::Uint32(48_000), "the passthrough stands");
    tb.world(|world| {
        assert_eq!(world.outputs[0].output.vrr, Some((AD_MIN_NS, AD_MAX_NS)));
        assert_eq!(
            world.scene.scheduler.config().vrr_window_ns,
            AD_MAX_NS - NOMINAL_NS
        );
        // The audit trail names the passthrough for output 0.
        assert!(matches!(
            world.vrr_floor_outcomes.first(),
            Some(ldp_vrr::FloorOutcome::Passthrough)
        ));
    });
}

#[test]
fn an_unservable_floor_refuses_boot_with_the_fixable_message() {
    // 61 Hz on a 60 Hz mode: the fixed grid itself would fall outside
    // the effective window — every deadline a lie. The honest knob
    // refuses at boot, naming the operator's mistake.
    let config = CompositorConfig {
        socket: format!("lion-it-vrrfloor-refuse-{}", std::process::id()),
        vrr: true,
        vrr_floor: Some(61),
        ..CompositorConfig::default()
    };
    let Err(refused) = Compositor::headless(config) else {
        panic!("the floor must refuse boot")
    };
    let message = refused.to_string();
    assert!(
        message.contains("--vrr-floor 61"),
        "names the knob: {message}"
    );
    assert!(
        message.contains("mode's refresh rate"),
        "names the mistake: {message}"
    );
}

#[test]
fn the_uniform_collapse_disarms_the_mixed_desktop() {
    // The mock's dual topology is the mixed desktop: output 0 carries
    // the 48-144 Hz window, output 1 none. Under `--vrr-uniform` no
    // output arms — one fixed clock across the seam (the
    // `vrr-sibling-flicker` escape); without it, the per-output
    // doctrine arms the capable one.
    fn mixed(tag: &str, uniform: bool) -> Testbench {
        let config = CompositorConfig {
            socket: format!("lion-it-vrruniform-{tag}-{}", std::process::id()),
            vrr: true,
            vrr_uniform: uniform,
            multi_output: true,
            ..CompositorConfig::default()
        };
        Testbench::start_with(tag, config)
    }
    // The doctrine (the default): the capable output arms.
    let tb = mixed("per-output", false);
    let mut client = TestClient::connect(&tb.addr);
    let shm = client.bind("ldp.core.shm");
    let comp = client.bind("ldp.core.compositor");
    commit_red_quad(&mut client, &shm, &comp, 0x3402);
    tb.world(|world| {
        let mock = world.device.as_mock().expect("the mock driver");
        assert!(
            mock.vrr_enabled(world.outputs[0].crtc),
            "the per-output doctrine arms the capable one"
        );
        assert!(!world.vrr_collapse);
        assert_eq!(
            world.scene.scheduler.config().vrr_window_ns,
            AD_MAX_NS - NOMINAL_NS
        );
    });

    // The escape (`--vrr-uniform`): the mixed desktop collapses to
    // one fixed sync — no output arms, whatever its own capability.
    let tb = mixed("uniform", true);
    let mut client = TestClient::connect(&tb.addr);
    let shm = client.bind("ldp.core.shm");
    let comp = client.bind("ldp.core.compositor");
    commit_red_quad(&mut client, &shm, &comp, 0x3403);
    tb.world(|world| {
        let mock = world.device.as_mock().expect("the mock driver");
        assert!(
            !mock.vrr_enabled(world.outputs[0].crtc),
            "the collapse disarms even the capable output"
        );
        assert!(world.vrr_collapse, "the collapse holds");
        assert_eq!(
            world.scene.scheduler.config().vrr_window_ns,
            0,
            "no widening under the collapse"
        );
        let report = world.vrr_quirk_report();
        assert!(
            report.contains("uniform fixed sync"),
            "the report names the collapse: {report}"
        );
    });
}

#[test]
fn the_floor_and_the_collapse_compose() {
    // `--vrr --vrr-uniform --vrr-floor 57` on the mixed desktop: the
    // floor pass still clamps the advertisement (the operator's truth
    // about the panel is the truth regardless of arming), and the
    // collapse keeps the desktop fixed — the audit trail carries both.
    let floor_ns = 1_000_000_000 / 57;
    let config = CompositorConfig {
        socket: format!("lion-it-vrrboth-{}", std::process::id()),
        vrr: true,
        vrr_uniform: true,
        vrr_floor: Some(57),
        multi_output: true,
        ..CompositorConfig::default()
    };
    let tb = Testbench::start_with("both", config);
    let mut client = TestClient::connect(&tb.addr);
    let _out = client.bind("ldp.core.output");
    // The effective window on the wire: the floor, not the collapse,
    // owns the advertisement.
    let vrr = client.last("vrr");
    assert_eq!(vrr.args[0], Value::Uint32(57_000));
    tb.world(|world| {
        assert!(world.vrr_collapse);
        assert_eq!(world.outputs[0].output.vrr, Some((AD_MIN_NS, floor_ns)));
        // The collapse owns the arming decision: no widening.
        assert_eq!(world.scene.scheduler.config().vrr_window_ns, 0);
    });
}
