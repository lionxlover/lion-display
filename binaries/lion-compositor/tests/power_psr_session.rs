//! Phase 35 exit criteria — the static-frame power path, end to end.
//!
//! The session the doctrine promises, driven and measured: a client
//! presents its content, the content goes still, the panel *sleeps*
//! (the connector property on, the device emitting nothing — ten
//! seconds of mock time cross with zero vblanks), the energy ledger
//! prices the sleep, damage wakes the panel at exactly one refresh
//! interval of rescan cost, the GPU governor climbs under load and
//! decays in stillness, and the idle ladder's rungs vacate the sleep
//! by their names (the backlight transition retrains, the blank
//! kills the scan path). Every number asserted here is the number
//! the comparison table's power row carries.

mod testbench;

use std::io::Write;

use ldp_client::Proxy;
use ldp_core::geometry::Rect;
use ldp_power::governor::PState;
use ldp_power::ledger::LedgerState;
use ldp_power::psr::PsrState;
use testbench::*;

use lion_compositor::server::CompositorConfig;
use lion_compositor::World;

/// A memfd pool carrying explicit pixel bytes.
fn pool_bytes(pixels: &[u8]) -> std::os::fd::OwnedFd {
    let fd = lion_compositor::sys::memfd("power-psr").expect("memfd");
    let mut file = std::fs::File::from(fd);
    file.write_all(pixels).expect("write pixels");
    file.into()
}

/// Advance the mock's clock (the device's injected time).
fn advance(world: &mut World, ms: u64) {
    world
        .device
        .as_mock_mut()
        .expect("the mock driver")
        .advance_ns(ms * 1_000_000);
}

/// One power cadence tick with `ms` of clock before it (the serve
/// loop's poll turn: time passes, then the tick).
fn power_turn(world: &mut World, ms: u64) {
    advance(world, ms);
    world.psr_tick().expect("the power tick");
}

/// Present one frame of solid-color content and wait for *this*
/// frame's verdict (count-based: the collector already carries every
/// earlier presented — a name match alone would race ahead of the
/// pump).
fn present_solid(client: &mut TestClient, shm: &Proxy, comp: &Proxy, surface: &Proxy, color: u32) {
    let before = client
        .events
        .records
        .iter()
        .filter(|r| r.event == "presented")
        .count();
    let pixels = [color.to_le_bytes(); 4].concat();
    let pool = create_pool(client, shm, pool_bytes(&pixels), 16);
    let buffer = create_buffer(client, &pool, 0, 2, 2, 8, 0x3432_5258);
    frame(client, surface, 1);
    attach(client, surface, &buffer);
    damage(client, surface, &[Rect::new(0, 0, 2, 2)]);
    commit(client, surface, 0x3501);
    client.wait_until(|c| c.records.iter().filter(|r| r.event == "presented").count() > before);
    let _ = comp;
}

/// The bring-up every test shares: one client, one surface, one
/// presented frame — a lit, current, *still* desktop.
fn still_desktop(tag: &str) -> (Testbench, TestClient, Proxy, Proxy, Proxy) {
    let tb = Testbench::start(tag);
    let mut client = TestClient::connect(&tb.addr);
    let shm = client.bind("ldp.core.shm");
    let comp = client.bind("ldp.core.compositor");
    let surface = client
        .conn
        .create_object(&comp, "create_surface", vec![])
        .expect("surface");
    present_solid(&mut client, &shm, &comp, &surface, 0x00FF_0000);
    // Drain the bind cascades before the power path starts (the
    // quiescence the machine reasons about must be explicit).
    client.sync();
    (tb, client, shm, comp, surface)
}

#[test]
fn static_content_puts_the_panel_to_sleep() {
    // The doctrine's core: two quiet power turns earn the hysteresis,
    // the engage lands on the connector, and the device goes *silent*
    // — the frozen timeline emits nothing however long the clock
    // runs. The world's mirror (`psr_live`) and the mock's own
    // property read agree.
    let (tb, mut client, _shm, _comp, _surface) = still_desktop("psr-sleep");
    let _ = &mut client;
    tb.world_mut(|world| {
        assert!(
            world.outputs[0].psr_capable,
            "the mock's eDP connector carries the property"
        );
        assert_eq!(world.outputs[0].psr.state(), PsrState::Scanout);
        // Turn one: the count earns one of two.
        power_turn(world, 250);
        assert_eq!(world.outputs[0].psr.state(), PsrState::Scanout);
        // Turn two: engaged.
        power_turn(world, 250);
        assert_eq!(world.outputs[0].psr.state(), PsrState::SelfRefresh);
        assert!(world.outputs[0].psr_live, "the engage committed");
    });
    // The device's own truth: the property is on, and ten seconds of
    // wall time cross with NOTHING from the panel.
    let (vblanks, flips) = tb.world(|world| {
        let mock = world.device.as_mock().expect("the mock");
        (
            mock.vblank_count(world.outputs[0].crtc),
            mock.flip_count(world.outputs[0].crtc),
        )
    });
    tb.world_mut(|world| {
        advance(world, 10_000);
    });
    tb.world(|world| {
        let mock = world.device.as_mock().expect("the mock");
        assert_eq!(
            mock.vblank_count(world.outputs[0].crtc),
            vblanks,
            "a sleeping panel emits no vblanks"
        );
        assert_eq!(
            mock.flip_count(world.outputs[0].crtc),
            flips,
            "a sleeping panel completes no flips"
        );
        assert!(
            mock.psr_enabled(world.outputs[0].output.connector.id),
            "the property reads back on"
        );
    });
}

#[test]
fn the_ledger_prices_the_sleep_and_the_report_names_it() {
    // The accounting: a long still desktop spends its time in
    // self-refresh, and the static-scene ratio lands under the
    // model's floor-plus-prefix bound — the measured number, not a
    // marketing one.
    let (tb, mut client, _shm, _comp, _surface) = still_desktop("psr-ledger");
    let _ = &mut client;
    tb.world_mut(|world| {
        // The hysteresis prefix: two turns.
        power_turn(world, 250);
        power_turn(world, 250);
        assert!(world.outputs[0].psr.engaged());
        // Ten seconds of sleep at the cadence.
        for _ in 0..40 {
            power_turn(world, 250);
        }
        let slept = world.ledger.ns_in(LedgerState::SelfRefresh);
        assert!(
            slept >= 9_000_000_000,
            "ten seconds minus the prefix slept: {} ms",
            slept / 1_000_000
        );
        let ratio = world.ledger.static_scene_ratio();
        assert!(
            ratio < 0.25,
            "the static ratio beats 4:1 over always-scanout: {ratio}"
        );
        assert!(
            ratio > 0.10,
            "the prefix and the wakes keep it honest (never zero): {ratio}"
        );
        let report = world.power_report();
        assert!(report.contains("static-ratio"), "{report}");
        assert!(report.contains("self-refresh"), "{report}");
        // The wake discipline is priced: 42 turns at one wake each.
        assert_eq!(world.ledger.wakes(), 42);
    });
}

#[test]
fn damage_wakes_the_panel_at_one_refresh_interval() {
    // The exit: new content arrives, the machine exits by its name,
    // the flip's implicit release applies, and the frame lands within
    // one nominal of the submission — the rescan cost the ledger
    // paid. The presentation verdict still flows to the client.
    let (tb, mut client, shm, _comp, surface) = still_desktop("psr-wake");
    tb.world_mut(|world| {
        power_turn(world, 250);
        power_turn(world, 250);
        assert!(world.outputs[0].psr.engaged());
        assert_eq!(world.ledger.psr_exits(), 0);
    });
    // New damage: the client presents a different color.
    let pixels = [0x0000_FF00u32.to_le_bytes(); 4].concat();
    let pool = create_pool(&mut client, &shm, pool_bytes(&pixels), 16);
    let buffer = create_buffer(&mut client, &pool, 0, 2, 2, 8, 0x3432_5258);
    let submit_ns = tb.now_ns();
    frame(&mut client, &surface, 2);
    attach(&mut client, &surface, &buffer);
    damage(&mut client, &surface, &[Rect::new(0, 0, 2, 2)]);
    commit(&mut client, &surface, 0x3502);
    client.wait_until(|c| c.records.iter().filter(|r| r.event == "presented").count() >= 2);
    tb.world(|world| {
        // The machine is out, by the damage's name.
        assert!(!world.outputs[0].psr.engaged());
        assert_eq!(
            world.outputs[0]
                .psr
                .exits_for(ldp_power::psr::PsrExit::Damage),
            1
        );
        assert_eq!(world.ledger.psr_exits(), 1, "the rescan was paid once");
        assert!(!world.outputs[0].psr_live, "the mirror follows the release");
        let mock = world.device.as_mock().expect("the mock");
        assert!(
            !mock.psr_enabled(world.outputs[0].output.connector.id),
            "the flip's implicit release applied"
        );
        // The wake's frame landed — the counter moved.
        assert!(world.frames >= 1);
        let _ = world.outputs[0].flips;
    });
    // The rescan budget: the presented flip landed within one nominal
    // of the submission (the mock's timeline re-anchor).
    let landed_ns = tb.now_ns();
    assert!(
        landed_ns - submit_ns <= 16_666_666 + 4_000_000,
        "the rescan plus the frame within one period + slack: {} ns",
        landed_ns - submit_ns
    );
}

#[test]
fn the_governor_climbs_under_load_and_decays_in_stillness() {
    // The clock ladder: a burst of presented frames steps the rung up
    // immediately; stillness walks it back down patiently; the
    // residency histogram remembers both.
    let (tb, mut client, shm, comp, surface) = still_desktop("psr-governor");
    // Load: six back-to-back presented frames — a full-rate burst on
    // the pacing grid (each frame's flip lands one nominal after its
    // submission, so six frames span ~100 ms of device time: load
    // 6/6 = 1.0 against the window's capacity). One tick with no
    // dead time closes the window and reads the verdict.
    for i in 0..6 {
        present_solid(&mut client, &shm, &comp, &surface, 0x00FF_0000 + i);
    }
    tb.world_mut(|world| {
        // Close the load window (1 ms of clock; the frames themselves
        // carried the elapsed time).
        power_turn(world, 1);
        assert!(
            world.gpu_pstate() > PState::P0,
            "the load stepped the clock up: {:?}",
            world.gpu_pstate()
        );
    });
    // Stillness: enough quiet windows to walk back to the floor (one
    // step per three quiet intervals; climbing went to at most P3).
    tb.world_mut(|world| {
        for _ in 0..40 {
            power_turn(world, 250);
        }
        assert_eq!(
            world.gpu_pstate(),
            PState::P0,
            "stillness walked the clock back to the floor"
        );
        // The ledger priced the load window at a rung (the climb's own
        // cost — rendered time, not scanout time).
        let rendered_ns: u64 = [PState::P0, PState::P1, PState::P2, PState::P3]
            .iter()
            .map(|rung| world.ledger.ns_in(LedgerState::Render(*rung)))
            .sum();
        assert!(rendered_ns > 0, "the load window was priced as render time");
        // And the long stillness slept the panel.
        assert!(world.outputs[0].psr.engaged());
    });
}

#[test]
fn the_dim_rung_retrains_the_panel_by_name() {
    // The ladder interplay: the Dim rung's backlight transition exits
    // the sleep (a real panel retrains), paying the exit in the
    // ledger, and the hysteresis re-earns the entry once the ramp
    // settles.
    let config = CompositorConfig {
        socket: format!("lion-it-psr-dim-{}", std::process::id()),
        idle_ms: Some(500),
        ..CompositorConfig::default()
    };
    let tb = Testbench::start_with("psr-dim", config);
    let mut client = TestClient::connect(&tb.addr);
    let shm = client.bind("ldp.core.shm");
    let comp = client.bind("ldp.core.compositor");
    let surface = client
        .conn
        .create_object(&comp, "create_surface", vec![])
        .expect("surface");
    present_solid(&mut client, &shm, &comp, &surface, 0x00FF_0000);
    client.sync();
    // Sleep first (the ladder is only at 500 ms — the panel sleeps
    // far earlier than the user is considered gone).
    tb.world_mut(|world| {
        power_turn(world, 250);
        power_turn(world, 250);
        assert!(world.outputs[0].psr.engaged());
    });
    // The dim at 500 ms of inactivity: the backlight transition.
    tb.world_mut(|world| {
        let events = world.tick_idle().expect("the ladder tick");
        assert!(
            events
                .iter()
                .any(|e| matches!(e, lion_compositor::power::PowerEvent::Dim(true))),
            "dimmed at the timeout: {events:?}"
        );
        assert!(
            !world.outputs[0].psr.engaged(),
            "the backlight retrains the panel out of self-refresh"
        );
        assert_eq!(
            world.outputs[0]
                .psr
                .exits_for(ldp_power::psr::PsrExit::Backlight),
            1,
            "the exit is named"
        );
        // The explicit release applied (no flip carries this one).
        let mock = world.device.as_mock().expect("the mock");
        assert!(!mock.psr_enabled(world.outputs[0].output.connector.id));
    });
    // The ramp settles: stillness re-earns the entry (the dimmed
    // desktop keeps sleeping — the LED is dim, the scan path is
    // quiet).
    tb.world_mut(|world| {
        power_turn(world, 250);
        power_turn(world, 250);
        assert!(
            world.outputs[0].psr.engaged(),
            "the hysteresis re-earns the sleep"
        );
    });
}

#[test]
fn the_blank_vacates_the_sleep_and_the_wake_restarts_it() {
    // The Off rung: the DPMS commit carries the release, the machine
    // exits by the blank's name, and the wake restarts from Scanout —
    // the sleep is re-earned, never inherited.
    // The rungs in order (the ladder's spacing is the logind
    // doctrine — dim at the timeout, off at twice it): the dim fires
    // FIRST (the backlight retrain — the dim test's exit), the panel
    // re-earns its sleep *while dimmed* (the doctrine: the LED is
    // dim, the scan path is quiet — both savings stack), and THEN the
    // blank vacates the sleep by the blank's own name.
    let config = CompositorConfig {
        socket: format!("lion-it-psr-blank-{}", std::process::id()),
        idle_ms: Some(250),
        ..CompositorConfig::default()
    };
    let tb = Testbench::start_with("psr-blank", config);
    let mut client = TestClient::connect(&tb.addr);
    let shm = client.bind("ldp.core.shm");
    let comp = client.bind("ldp.core.compositor");
    let surface = client
        .conn
        .create_object(&comp, "create_surface", vec![])
        .expect("surface");
    present_solid(&mut client, &shm, &comp, &surface, 0x00FF_0000);
    client.sync();
    // 250 ms: the Dim rung (the backlight ramp).
    tb.world_mut(|world| {
        advance(world, 250);
        let events = world.tick_idle().expect("the ladder tick");
        assert!(
            events
                .iter()
                .any(|e| matches!(e, lion_compositor::power::PowerEvent::Dim(true))),
            "dimmed at the timeout: {events:?}"
        );
    });
    // The dimmed desktop still sleeps: two quiet turns.
    tb.world_mut(|world| {
        power_turn(world, 250);
        power_turn(world, 250);
        assert!(
            world.outputs[0].psr.engaged(),
            "a dimmed panel still self-refreshes"
        );
    });
    // 250 ms more: the Off rung (the blank).
    tb.world_mut(|world| {
        advance(world, 250);
        let events = world.tick_idle().expect("the ladder tick");
        assert!(
            events
                .iter()
                .any(|e| matches!(e, lion_compositor::power::PowerEvent::Dpms(false))),
            "blanked at twice the timeout: {events:?}"
        );
        assert!(world.blanked);
        assert!(
            !world.outputs[0].psr.engaged(),
            "the blank vacated the sleep"
        );
        assert_eq!(
            world.outputs[0]
                .psr
                .exits_for(ldp_power::psr::PsrExit::Blank),
            1,
            "the exit is named"
        );
        assert!(world.ledger.dpms_transitions() >= 1);
    });
    // The wake: the panels relight, the machine starts over.
    client.sync();
    tb.world_mut(|world| {
        assert!(!world.blanked);
        assert_eq!(world.outputs[0].psr.state(), PsrState::Scanout);
        // Re-earned, never inherited.
        power_turn(world, 250);
        assert_eq!(world.outputs[0].psr.state(), PsrState::Scanout);
        power_turn(world, 250);
        assert!(world.outputs[0].psr.engaged());
    });
}

#[test]
fn no_psr_keeps_scanning_the_static_frame() {
    // The operator's escape: with `--no-psr` the panel never sleeps —
    // the same still desktop, the timeline ticking on, the ledger
    // pricing pure scanout (the honest counterfactual the ratio is
    // measured against).
    let config = CompositorConfig {
        socket: format!("lion-it-psr-off-{}", std::process::id()),
        psr: false,
        ..CompositorConfig::default()
    };
    let tb = Testbench::start_with("psr-off", config);
    let mut client = TestClient::connect(&tb.addr);
    let shm = client.bind("ldp.core.shm");
    let comp = client.bind("ldp.core.compositor");
    let surface = client
        .conn
        .create_object(&comp, "create_surface", vec![])
        .expect("surface");
    present_solid(&mut client, &shm, &comp, &surface, 0x00FF_0000);
    client.sync();
    let vblanks = tb.world(|world| {
        world
            .device
            .as_mock()
            .expect("the mock")
            .vblank_count(world.outputs[0].crtc)
    });
    tb.world_mut(|world| {
        for _ in 0..6 {
            power_turn(world, 250);
        }
        assert_eq!(world.outputs[0].psr.state(), PsrState::Scanout);
        assert_eq!(
            world.ledger.ns_in(LedgerState::SelfRefresh),
            0,
            "no sleep without the doctrine"
        );
    });
    // The timeline never froze: the vblanks kept coming.
    let after = tb.world(|world| {
        world
            .device
            .as_mock()
            .expect("the mock")
            .vblank_count(world.outputs[0].crtc)
    });
    assert!(
        after > vblanks,
        "the escape keeps scanning: {vblanks} → {after}"
    );
}
