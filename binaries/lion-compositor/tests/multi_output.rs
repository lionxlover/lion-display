//! Phase 31 exit criteria — the multi-output doctrine, CI-proven.
//!
//! The serve loop of Phase 25/26 served one pipeline; these tests pin
//! the compositor that serves *every* pipeline the allocator finds:
//!
//! * **The dual bring-up** — the mock's two connected displays serve
//!   on their own CRTCs and planes, enabled and scanning out, laid out
//!   left-to-right into one logical desktop.
//! * **The bind round-robin** — a client binds `ldp.core.output` once
//!   per display it cares about; each bind mirrors the next output
//!   (its own geometry, its own VRR truth).
//! * **Per-output visibility** — a window living on the primary
//!   enters only the primary; a window *spanning* the seam enters both
//!   outputs, and each output's scanout carries its own portion of the
//!   desktop.
//! * **Topology motion** — a non-primary leaving is `OutputRemoved`
//!   (the survivor re-flows, nothing restarts); a display joining is
//!   `OutputAdded` (the desktop extends); mixed sizes lay out
//!   left-to-right at their own geometries.
//! * **The release gates** — a buffer spanning two outputs is released
//!   once *both* have flipped past it; an output leaving satisfies its
//!   own gate (that output stopped reading).
//! * **The library default** — the single-output doctrine is
//!   untouched: two connected monitors, one served pipeline.

mod testbench;

use std::io::Write;

use ldp_core::geometry::Rect;
use ldp_core::wire::Value;
use testbench::*;

use lion_compositor::rearrange::Rearrange;
use lion_compositor::server::CompositorConfig;
use lion_compositor::World;

/// The connectors of the mock preset.
const EDP: u32 = 91;
const DP: u32 = 92;
const HDMI: u32 = 93;

fn connector(id: u32) -> ldp_display::ids::ConnectorId {
    ldp_display::ids::ConnectorId::new(id).expect("preset connector id")
}

/// The multi-output configuration (the library default stays single —
/// these tests opt the doctrine in).
fn multi_config(tag: &str) -> CompositorConfig {
    CompositorConfig {
        socket: format!("lion-it-multi-{tag}-{}", std::process::id()),
        multi_output: true,
        ..CompositorConfig::default()
    }
}

fn start_multi(tag: &str) -> Testbench {
    Testbench::start_with(tag, multi_config(tag))
}

/// Recorded events of one name (the Collector-side helper the
/// wait-until closures use).
fn events_of<'a>(c: &'a Collector, name: &str) -> Vec<&'a Recorded> {
    c.records.iter().filter(|r| r.event == name).collect()
}

/// A memfd pool carrying explicit pixel bytes.
fn pool_bytes(pixels: &[u8]) -> std::os::fd::OwnedFd {
    let fd = lion_compositor::sys::memfd("multi").expect("memfd");
    let mut file = std::fs::File::from(fd);
    file.write_all(pixels).expect("write pixels");
    file.into()
}

/// A `w * h` XRGB8888 pool of one color word (stride `w * 4`).
fn solid_pixels(w: u32, h: u32, word: u32) -> Vec<u8> {
    let mut v = Vec::with_capacity((w * h * 4) as usize);
    for _ in 0..w * h {
        v.extend_from_slice(&word.to_le_bytes());
    }
    v
}

/// Inject a topology change and service it (the serve loop's hotplug
/// arm, run once) — returns the re-arrangement's summary.
fn service_hotplug(tb: &Testbench) -> Rearrange {
    let mut world = tb.shared.world.lock().expect("world lock");
    world
        .service_device_events()
        .expect("the service pass")
        .expect("a hotplug event was queued")
}

/// Unplug one connector (queues the event; the service pass acts).
fn unplug(world: &mut World, id: u32) {
    world
        .device
        .as_mock_mut()
        .expect("the mock driver")
        .hotplug_disconnect(connector(id));
}

/// Plug a connector in with a 1080p60 panel mode and a synthesized
/// EDID identity.
fn plug_1080p(world: &mut World, id: u32) {
    world
        .device
        .as_mock_mut()
        .expect("the mock driver")
        .hotplug_connect(
            connector(id),
            vec![ldp_display::mode::Mode::panel_1080p60()],
            vec![0xA5u8; 16],
        );
}

/// A 720p60 mode (the mixed-size probe).
fn mode_720p60() -> ldp_display::mode::Mode {
    ldp_display::mode::Mode::new(
        74_250,
        1280,
        1390,
        1430,
        1650,
        720,
        725,
        730,
        750,
        ldp_display::mode::ModeFlags::PVSYNC | ldp_display::mode::ModeFlags::NHSYNC,
        ldp_display::mode::ModeType::DRIVER,
    )
}

/// The served summaries as `(name, w, h)` triples, in slot order.
fn served_triples(world: &World) -> Vec<(String, u32, u32)> {
    world
        .served_all()
        .into_iter()
        .map(|s| (s.name, s.width, s.height))
        .collect()
}

#[test]
fn dual_bringup_serves_both_outputs() {
    let tb = start_multi("bringup");
    // Two slots: eDP-1 on CRTC 42/plane 50, HDMI-A-1 on CRTC 43/plane 53.
    tb.world(|world| {
        assert_eq!(world.outputs.len(), 2, "both connected displays serve");
        assert_eq!(world.outputs[0].output.name, "eDP-1");
        assert_eq!(world.outputs[0].crtc.raw(), 42);
        assert_eq!(world.outputs[0].plane.raw(), 50);
        assert_eq!(world.outputs[1].output.name, "HDMI-A-1");
        assert_eq!(world.outputs[1].crtc.raw(), 43);
        assert_eq!(world.outputs[1].plane.raw(), 53);
        // The logical layout: the primary at the origin, HDMI appended
        // right (one 3840x1080 desktop).
        assert_eq!(world.outputs[0].output.layout, (0, 0));
        assert_eq!(world.outputs[1].output.layout, (1920, 0));
        let desktop = world.desktop_bounds();
        assert_eq!(
            (desktop.x, desktop.y, desktop.w, desktop.h),
            (0, 0, 3840, 1080)
        );
        // The device state: both pipelines applied.
        let mock = world.device.as_mock().expect("the mock driver");
        assert!(
            mock.crtc_mode_now(world.outputs[0].crtc).is_some(),
            "eDP active"
        );
        assert!(
            mock.crtc_mode_now(world.outputs[1].crtc).is_some(),
            "HDMI active"
        );
        assert_eq!(
            mock.connector_binding(connector(EDP)),
            Some(world.outputs[0].crtc)
        );
        assert_eq!(
            mock.connector_binding(connector(HDMI)),
            Some(world.outputs[1].crtc)
        );
    });
}

#[test]
fn library_default_stays_single_with_two_monitors() {
    // The Phase 25 spine: the library default serves one pipeline even
    // on the dual-screen mock — every equivalence oracle's meaning.
    let tb = Testbench::start("single-default");
    tb.world(|world| {
        assert_eq!(world.outputs.len(), 1);
        assert_eq!(world.outputs[0].output.name, "eDP-1");
        assert_eq!(world.outputs[0].output.layout, (0, 0));
    });
}

#[test]
fn two_output_binds_mirror_two_displays() {
    let tb = start_multi("binds");
    let mut client = TestClient::connect(&tb.addr);
    // The first bind mirrors eDP-1 (the primary, at the origin, with
    // the VRR window of its CRTC); the second mirrors HDMI-A-1 (at
    // x=1920, no VRR — its CRTC is fixed-sync).
    let _out0 = client.bind("ldp.core.output");
    let geometry0: Vec<Value> = client.last("geometry").args.clone();
    assert_eq!(geometry0[0], Value::Int32(0));
    assert_eq!(client.last("name").args[0], Value::String("eDP-1".into()));
    let (_, vrr_max) = (0u32, 0u32);
    let _ = vrr_max;
    let vrr0 = client.last("vrr").args.clone();
    let Value::Uint32(min0) = vrr0[0] else {
        panic!("vrr min")
    };
    let Value::Uint32(max0) = vrr0[1] else {
        panic!("vrr max")
    };
    assert_eq!((min0, max0), (48_000, 144_000), "the primary's VRR window");

    let _out1 = client.conn.bind("ldp.core.output").expect("bind");
    // (The helper's `bound` wait fires on the first bind's confirmation
    // already — wait for the *second* cascade by its content.)
    client.wait_until(|c| events_of(c, "name").len() == 2);
    // The second bind's cascade: HDMI-A-1's geometry (x=1920) and the
    // fixed-sync VRR truth.
    let geoms = client.events_of("geometry");
    assert_eq!(geoms.len(), 2, "two geometries — one per bind");
    assert_eq!(geoms[1].args[0], Value::Int32(1920), "HDMI at x=1920");
    let names: Vec<&str> = client
        .events_of("name")
        .iter()
        .map(|r| match &r.args[0] {
            Value::String(s) => &**s,
            _ => "",
        })
        .collect();
    assert_eq!(names, vec!["eDP-1", "HDMI-A-1"]);
    let caps_events = client.events_of("vrr");
    assert_eq!(caps_events.len(), 2);
    // The fixed-sync CRTC advertises no window.
    let Value::Bitset(bits1) = &caps_events[1].args[2] else {
        panic!("vrr caps");
    };
    assert_eq!(bits1.to_words()[0], 0, "no VRR on the fixed-sync CRTC");
}

#[test]
fn a_window_on_the_primary_enters_only_the_primary() {
    let tb = start_multi("enter-one");
    let mut client = TestClient::connect(&tb.addr);
    let shm = client.bind("ldp.core.shm");
    let comp = client.bind("ldp.core.compositor");
    // Two output objects: the client watches both displays.
    let _primary_out = client.bind("ldp.core.output");
    let out1 = client.bind("ldp.core.output");
    // A 2x2 window at the origin: purely on the primary.
    let surface = client
        .conn
        .create_object(&comp, "create_surface", vec![])
        .expect("surface");
    let pool = create_pool(
        &mut client,
        &shm,
        pool_bytes(&solid_pixels(2, 2, 0x00FF_0000)),
        16,
    );
    let buffer = create_buffer(&mut client, &pool, 0, 2, 2, 8, 0x3432_5258);
    frame(&mut client, &surface, 1);
    attach(&mut client, &surface, &buffer);
    damage(&mut client, &surface, &[Rect::new(0, 0, 2, 2)]);
    commit(&mut client, &surface, 0x3101);
    client.wait_until(|c| c.records.iter().any(|r| r.event == "presented"));
    // Exactly one enter_output — the primary's object (the window
    // never crosses the seam; HDMI's object stays un-entered).
    let enters = client.events_of("enter_output");
    assert_eq!(enters.len(), 1, "one enter, one output");
    let Value::Object(Some(entered)) = enters[0].args[0] else {
        panic!("enter_output object");
    };
    assert_ne!(entered, out1.id(), "the entered object is the primary's");
}

#[test]
fn a_window_spanning_the_seam_enters_both_and_shows_on_both() {
    let tb = start_multi("span");
    let mut client = TestClient::connect(&tb.addr);
    let shm = client.bind("ldp.core.shm");
    let comp = client.bind("ldp.core.compositor");
    let out0 = client.bind("ldp.core.output");
    let out1 = client.bind("ldp.core.output");
    // A 2200x2 window at the origin: 1920 px on the primary, 280 px
    // past the seam on HDMI.
    let surface = client
        .conn
        .create_object(&comp, "create_surface", vec![])
        .expect("surface");
    let pixels = solid_pixels(2200, 2, 0x00FF_0000);
    let pool = create_pool(&mut client, &shm, pool_bytes(&pixels), 17_600);
    let buffer = create_buffer(&mut client, &pool, 0, 2200, 2, 8800, 0x3432_5258);
    frame(&mut client, &surface, 1);
    attach(&mut client, &surface, &buffer);
    damage(&mut client, &surface, &[Rect::new(0, 0, 2200, 2)]);
    commit(&mut client, &surface, 0x3102);
    client.wait_until(|c| c.records.iter().any(|r| r.event == "presented"));
    // The window is visible on both outputs: two enter_output events,
    // one per bound object.
    let enters = client.events_of("enter_output");
    assert_eq!(enters.len(), 2, "the seam-spanning window enters both");
    let objects: Vec<u32> = enters
        .iter()
        .filter_map(|r| match r.args[0] {
            Value::Object(Some(o)) => Some(o.as_u32()),
            _ => None,
        })
        .collect();
    assert!(objects.contains(&out0.id().as_u32()));
    assert!(objects.contains(&out1.id().as_u32()));
    // Each output's scanout carries its own portion: the primary's
    // first pixel is the window's x=0; HDMI's first pixel is the
    // window's x=1920 — both red, and the pixels *past* the window's
    // right edge on HDMI are the desktop's black.
    tb.world(|world| {
        let primary = world.slot_scanout_words(0).expect("primary lit");
        let secondary = world.slot_scanout_words(1).expect("secondary lit");
        assert_eq!(primary.len(), 1920 * 1080);
        assert_eq!(secondary.len(), 1920 * 1080);
        // The seam: the window's row continues across outputs.
        assert_eq!(
            primary[0], 0xFFFF_0000,
            "primary x=0 is window red (opaque)"
        );
        assert_eq!(
            secondary[0], 0xFFFF_0000,
            "secondary x=0 is window x=1920 (red, opaque)"
        );
        assert_eq!(
            secondary[280], 0xFF00_0000,
            "secondary x=280 is past the window's right edge — desktop black"
        );
    });
}

#[test]
fn a_non_primary_leaving_is_output_removed_and_the_survivor_reflows() {
    let tb = start_multi("remove");
    let mut client = TestClient::connect(&tb.addr);
    let _shm = client.bind("ldp.core.shm");
    let _out0 = client.bind("ldp.core.output");
    let _out1 = client.conn.bind("ldp.core.output").expect("bind");
    client.wait_until(|c| events_of(c, "name").len() == 2);
    // HDMI (the non-primary) leaves.
    {
        let mut world = tb.shared.world.lock().expect("world lock");
        unplug(&mut world, HDMI);
    }
    match service_hotplug(&tb) {
        Rearrange::OutputRemoved { from } => {
            assert_eq!(from.name, "HDMI-A-1");
        }
        other => panic!("expected OutputRemoved, got {other:?}"),
    }
    // One output remains — the primary, unmoved (its CRTC, its chain,
    // its session never restarted).
    tb.world(|world| {
        assert_eq!(served_triples(world), vec![("eDP-1".into(), 1920, 1080)]);
        assert_eq!(world.outputs[0].output.layout, (0, 0));
        let desktop = world.desktop_bounds();
        assert_eq!((desktop.w, desktop.h), (1920, 1080), "the desktop shrank");
    });
    // The HDMI bind was revoked; the primary's object still serves
    // (the client learns at its next wake).
    client.wait_until(|c| !events_of(c, "revoked").is_empty());
}

#[test]
fn the_primary_leaving_promotes_the_survivor() {
    let tb = start_multi("promote");
    let mut client = TestClient::connect(&tb.addr);
    let _out0 = client.bind("ldp.core.output");
    let _out1 = client.conn.bind("ldp.core.output").expect("bind");
    client.wait_until(|c| events_of(c, "name").len() == 2);
    // eDP (the primary) leaves: HDMI is promoted — the desktop
    // re-flows onto it as the new primary at the origin.
    {
        let mut world = tb.shared.world.lock().expect("world lock");
        unplug(&mut world, EDP);
    }
    match service_hotplug(&tb) {
        Rearrange::OutputRemoved { from } => {
            assert_eq!(from.name, "eDP-1");
        }
        other => panic!("expected OutputRemoved, got {other:?}"),
    }
    tb.world(|world| {
        assert_eq!(served_triples(world), vec![("HDMI-A-1".into(), 1920, 1080)]);
        assert_eq!(
            world.outputs[0].output.layout,
            (0, 0),
            "the survivor is the primary now"
        );
    });
    // Both binds revoked (every layout rode the cascades).
    client.wait_until(|c| events_of(c, "revoked").len() == 2);
    // The session still serves: a re-bind mirrors the promoted primary.
    let _rebound = client.bind("ldp.core.output");
    client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "name" && r.args[0] == Value::String("HDMI-A-1".into()))
    });
}

#[test]
fn a_display_joining_extends_the_desktop() {
    let tb = start_multi("add");
    let mut client = TestClient::connect(&tb.addr);
    let _out0 = client.bind("ldp.core.output");
    client.wait_until(|c| !events_of(c, "name").is_empty());
    // Free a pipeline (the device has two CRTCs, both serving), then
    // plug DP-1 in: the desktop extends.
    {
        let mut world = tb.shared.world.lock().expect("world lock");
        unplug(&mut world, HDMI);
    }
    assert!(matches!(
        service_hotplug(&tb),
        Rearrange::OutputRemoved { .. }
    ));
    {
        let mut world = tb.shared.world.lock().expect("world lock");
        plug_1080p(&mut world, DP);
    }
    match service_hotplug(&tb) {
        Rearrange::OutputAdded { to } => {
            assert_eq!(to.name, "DP-1");
            assert_eq!((to.width, to.height), (1920, 1080));
        }
        other => panic!("expected OutputAdded, got {other:?}"),
    }
    tb.world(|world| {
        assert_eq!(
            served_triples(world),
            vec![("eDP-1".into(), 1920, 1080), ("DP-1".into(), 1920, 1080)]
        );
        assert_eq!(world.outputs[1].output.layout, (1920, 0));
        let desktop = world.desktop_bounds();
        assert_eq!(desktop.w, 3840, "the desktop extended");
    });
    // The living session presents onto the extended desktop.
    let shm = client.bind("ldp.core.shm");
    let comp = client.bind("ldp.core.compositor");
    let surface = client
        .conn
        .create_object(&comp, "create_surface", vec![])
        .expect("surface");
    let pool = create_pool(
        &mut client,
        &shm,
        pool_bytes(&solid_pixels(2, 2, 0x0000_FF00)),
        16,
    );
    let buffer = create_buffer(&mut client, &pool, 0, 2, 2, 8, 0x3432_5258);
    frame(&mut client, &surface, 1);
    attach(&mut client, &surface, &buffer);
    damage(&mut client, &surface, &[Rect::new(0, 0, 2, 2)]);
    commit(&mut client, &surface, 0x3103);
    client.wait_until(|c| c.records.iter().any(|r| r.event == "presented"));
}

#[test]
fn mixed_sizes_lay_out_left_to_right() {
    let tb = start_multi("mixed");
    // HDMI re-negotiates a 720p mode: the desktop is a 1080p primary
    // plus a 720p secondary at x=1920, top-aligned.
    {
        let mut world = tb.shared.world.lock().expect("world lock");
        world
            .device
            .as_mock_mut()
            .expect("the mock driver")
            .hotplug_connect(connector(HDMI), vec![mode_720p60()], vec![0x5Au8; 16]);
    }
    match service_hotplug(&tb) {
        Rearrange::Migrated { from, to } => {
            // The same connector re-negotiating a different truth is a
            // migration (Phase 26's rule, per-output now).
            assert_eq!(from.name, "HDMI-A-1");
            assert_eq!((from.width, from.height), (1920, 1080));
            assert_eq!((to.width, to.height), (1280, 720));
        }
        other => panic!("expected Migrated, got {other:?}"),
    }
    tb.world(|world| {
        assert_eq!(
            served_triples(world),
            vec![("eDP-1".into(), 1920, 1080), ("HDMI-A-1".into(), 1280, 720)]
        );
        assert_eq!(world.outputs[1].output.layout, (1920, 0));
        let desktop = world.desktop_bounds();
        assert_eq!((desktop.w, desktop.h), (3200, 1080), "the union bounds");
    });
}

#[test]
fn a_spanning_buffer_releases_after_both_outputs_flip_past_it() {
    let tb = start_multi("gates");
    let mut client = TestClient::connect(&tb.addr);
    let shm = client.bind("ldp.core.shm");
    let comp = client.bind("ldp.core.compositor");
    let surface = client
        .conn
        .create_object(&comp, "create_surface", vec![])
        .expect("surface");
    // A spanning window, first buffer.
    frame(&mut client, &surface, 1);
    let pool_a = create_pool(
        &mut client,
        &shm,
        pool_bytes(&solid_pixels(2200, 2, 0x00FF_0000)),
        17_600,
    );
    let buffer_a = create_buffer(&mut client, &pool_a, 0, 2200, 2, 8800, 0x3432_5258);
    attach(&mut client, &surface, &buffer_a);
    damage(&mut client, &surface, &[Rect::new(0, 0, 2200, 2)]);
    commit(&mut client, &surface, 0x3104);
    client.wait_until(|c| c.records.iter().any(|r| r.event == "presented"));
    // The swap: buffer B replaces A. A's release gate names *both*
    // outputs (each was scanning its portion).
    frame(&mut client, &surface, 2);
    let pool_b = create_pool(
        &mut client,
        &shm,
        pool_bytes(&solid_pixels(2200, 2, 0x0000_FF00)),
        17_600,
    );
    let buffer_b = create_buffer(&mut client, &pool_b, 0, 2200, 2, 8800, 0x3432_5258);
    attach(&mut client, &surface, &buffer_b);
    damage(&mut client, &surface, &[Rect::new(0, 0, 2200, 2)]);
    commit(&mut client, &surface, 0x3105);
    // The release lands once both outputs flipped past the old front
    // buffers — the spanning swap's own renders.
    client.wait_until(|c| events_of(c, "release").len() == 1);
    let release = client.last("release");
    assert!(release.fence.unwrap_or(false), "the fence is signalled");
    // And the second presentation completed on both outputs' renders.
    client.wait_until(|c| c.records.iter().filter(|r| r.event == "presented").count() >= 2);
    tb.world(|world| {
        // Both scanouts now carry buffer B's green.
        assert_eq!(world.slot_scanout_words(0).unwrap()[0], 0xFF00_FF00);
        assert_eq!(world.slot_scanout_words(1).unwrap()[0], 0xFF00_FF00);
    });
}

#[test]
fn an_output_leaving_satisfies_its_own_release_gate() {
    let tb = start_multi("gate-satisfy");
    let mut client = TestClient::connect(&tb.addr);
    let shm = client.bind("ldp.core.shm");
    let comp = client.bind("ldp.core.compositor");
    let surface = client
        .conn
        .create_object(&comp, "create_surface", vec![])
        .expect("surface");
    // A spanning window, first buffer.
    frame(&mut client, &surface, 1);
    let pool_a = create_pool(
        &mut client,
        &shm,
        pool_bytes(&solid_pixels(2200, 2, 0x00FF_0000)),
        17_600,
    );
    let buffer_a = create_buffer(&mut client, &pool_a, 0, 2200, 2, 8800, 0x3432_5258);
    attach(&mut client, &surface, &buffer_a);
    damage(&mut client, &surface, &[Rect::new(0, 0, 2200, 2)]);
    commit(&mut client, &surface, 0x3106);
    client.wait_until(|c| c.records.iter().any(|r| r.event == "presented"));
    // The swap queues the release gated on both outputs — and HDMI
    // leaves before its flip: its gate satisfies (that output stopped
    // reading), the primary's gate passes with the re-flow's flip.
    frame(&mut client, &surface, 2);
    let pool_b = create_pool(
        &mut client,
        &shm,
        pool_bytes(&solid_pixels(2200, 2, 0x0000_FF00)),
        17_600,
    );
    let buffer_b = create_buffer(&mut client, &pool_b, 0, 2200, 2, 8800, 0x3432_5258);
    attach(&mut client, &surface, &buffer_b);
    damage(&mut client, &surface, &[Rect::new(0, 0, 2200, 2)]);
    commit(&mut client, &surface, 0x3107);
    {
        let mut world = tb.shared.world.lock().expect("world lock");
        unplug(&mut world, HDMI);
    }
    assert!(matches!(
        service_hotplug(&tb),
        Rearrange::OutputRemoved { .. }
    ));
    client.wait_until(|c| events_of(c, "release").len() == 1);
}

#[test]
fn the_last_two_leaving_is_dark_regardless_of_count() {
    let tb = start_multi("dark");
    let mut client = TestClient::connect(&tb.addr);
    let _out0 = client.bind("ldp.core.output");
    client.wait_until(|c| !events_of(c, "name").is_empty());
    {
        let mut world = tb.shared.world.lock().expect("world lock");
        unplug(&mut world, EDP);
        unplug(&mut world, HDMI);
    }
    match service_hotplug(&tb) {
        Rearrange::Dark { from } => {
            assert_eq!(from.name, "eDP-1", "the primary leads the report");
        }
        other => panic!("expected Dark, got {other:?}"),
    }
    tb.world(|world| {
        assert!(world.outputs.is_empty(), "the honest dark state");
    });
    // The session keeps serving (the Phase 26 doctrine).
    client.wait_until(|c| events_of(c, "revoked").len() == 1);
    // And the relight brings the desktop back on both.
    {
        let mut world = tb.shared.world.lock().expect("world lock");
        plug_1080p(&mut world, EDP);
        plug_1080p(&mut world, HDMI);
    }
    match service_hotplug(&tb) {
        Rearrange::Relit { to } => {
            assert_eq!(to.name, "eDP-1");
        }
        other => panic!("expected Relit, got {other:?}"),
    }
    tb.world(|world| {
        assert_eq!(world.outputs.len(), 2, "the desktop returned on both");
    });
}
