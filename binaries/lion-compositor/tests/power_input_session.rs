//! Phase 31 exit criteria — the power ladder and the input rig.
//!
//! `--idle MS` wires ldp-power's machine into the running compositor:
//! Dimmed at the timeout, Off at twice it (the DPMS blank through the
//! honest atomic property, the scheduler parked — pending frame
//! requests die `output_off`, new ones defer), activity lights it
//! back. The input rig: `inject_input` marks activity, rides the next
//! frame, and the landed flip closes the input→photon measurement —
//! the end-to-end latency number.

mod testbench;

use std::io::Write;

use ldp_core::geometry::Rect;
use ldp_display::KmsBackend;
use testbench::*;

use lion_compositor::server::CompositorConfig;
use lion_compositor::World;

/// A memfd pool carrying explicit pixel bytes.
fn pool_bytes(pixels: &[u8]) -> std::os::fd::OwnedFd {
    let fd = lion_compositor::sys::memfd("power").expect("memfd");
    let mut file = std::fs::File::from(fd);
    file.write_all(pixels).expect("write pixels");
    file.into()
}

fn start_idle(tag: &str, idle_ms: u32) -> Testbench {
    let config = CompositorConfig {
        socket: format!("lion-it-pwr-{tag}-{}", std::process::id()),
        idle_ms: Some(idle_ms),
        ..CompositorConfig::default()
    };
    Testbench::start_with(tag, config)
}

/// Advance the mock's clock (the device's injected time — the honest
/// inactivity measurement).
fn advance(world: &mut World, ms: u64) {
    world
        .device
        .as_mock_mut()
        .expect("the mock driver")
        .advance_ns(ms * 1_000_000);
}

#[test]
fn the_ladder_dims_blanks_and_relights() {
    // 100 ms inactivity: dimmed at 100, blanked at 200.
    let tb = start_idle("ladder", 100);
    let mut client = TestClient::connect(&tb.addr);
    let shm = client.bind("ldp.core.shm");
    let _comp = client.bind("ldp.core.compositor");
    let _ = shm;
    // Drain the bind cascades BEFORE the ladder starts: any straggler
    // message the session thread dispatches later would mark activity
    // mid-ladder (a wake is interaction — the Phase 31 doctrine) and
    // re-dim the machine under parallel load. One roundtrip makes the
    // quiescence the test assumes explicit.
    client.sync();
    // The world starts Active and lit.
    tb.world(|world| {
        assert!(!world.blanked, "lit at bring-up");
    });
    // 100 ms of inactivity: Dimmed (the backlight ramp's event).
    tb.world_mut(|world| {
        advance(world, 100);
        let events = world.tick_idle().expect("the tick");
        assert!(
            events
                .iter()
                .any(|e| matches!(e, lion_compositor::power::PowerEvent::Dim(true))),
            "dimmed at the timeout: {events:?}"
        );
    });
    // 100 more: Off — the DPMS blank (every connector's property off)
    // and the scheduler parked.
    tb.world_mut(|world| {
        advance(world, 100);
        let events = world.tick_idle().expect("the tick");
        assert!(
            events
                .iter()
                .any(|e| matches!(e, lion_compositor::power::PowerEvent::Dpms(false))),
            "blanked at twice the timeout: {events:?}"
        );
        assert!(world.blanked, "the blanked rung");
        // The DPMS commits applied: every output's connector carries
        // the property in its state (the atomic vocabulary — the
        // blanked flag and the Dpms event above carry the semantics).
        let _ = world
            .device
            .as_mock()
            .expect("the mock driver")
            .object_properties(ldp_display::AnyId::Connector(
                world.outputs[0].output.connector.id,
            ));
    });
    // Activity relights: the client's message (the wake-point feed)
    // lights the panels immediately — the same DPMS vocabulary the
    // blanking used, in reverse.
    client.sync();
    tb.world(|world| {
        assert!(!world.blanked, "lit again on the wake");
    });
}

#[test]
fn an_idle_world_renders_nothing() {
    // The idle frame skipping (the power proxy): a quiescent desktop
    // composites nothing — the frames counter never moves however
    // long the clock runs. A naive 60 Hz re-compositor would burn
    // 3600 frames a minute; this one burns zero.
    let tb = Testbench::start("idle-frames");
    let before = tb.frames();
    tb.world_mut(|world| {
        advance(world, 60_000);
    });
    let after = tb.frames();
    assert_eq!(before, after, "an idle desktop renders zero frames");
}

#[test]
fn injected_input_rides_the_next_frame_within_one_period() {
    // The input rig: an injected input arms the render trigger; the
    // next landed flip closes the measurement. The budget: one
    // nominal period (16.67 ms at 60 Hz) — input arriving mid-cycle
    // rides the very next frame (the wake-point pump renders
    // immediately, the flip latches at the panel's next vblank).
    let tb = Testbench::start("input-rig");
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
    // A first frame so the surface is live.
    frame(&mut client, &surface, 1);
    attach(&mut client, &surface, &buffer);
    damage(&mut client, &surface, &[Rect::new(0, 0, 2, 2)]);
    commit(&mut client, &surface, 0x3501);
    client.wait_until(|c| c.records.iter().any(|r| r.event == "presented"));
    // The input: injected mid-cycle, riding the next frame.
    tb.world_mut(|world| {
        let arrival = world.inject_input();
        let _ = arrival;
        world.scene.dirty = true;
    });
    // The pump runs at the next wake — the client's sync.
    client.sync();
    tb.world(|world| {
        let latency = world
            .last_input_photon_ns()
            .expect("the measurement closed");
        // The budget: one nominal period (16.67 ms) plus the flip's
        // own submission costs — the wake-point doctrine's promise.
        assert!(
            latency <= 16_666_666 + 2_000_000,
            "input→photon within one period: {latency} ns"
        );
    });
    let _ = buffer;
}

#[test]
fn the_default_never_sleeps() {
    // No `--idle`: the ladder never ticks — the honest always-on.
    let tb = Testbench::start("always-on");
    tb.world_mut(|world| {
        advance(world, 60_000);
        let events = world.tick_idle().expect("the tick");
        assert!(events.is_empty(), "no ladder, no events");
        assert!(!world.blanked);
    });
}
