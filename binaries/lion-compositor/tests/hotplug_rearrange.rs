//! Phase 26 exit criteria — live output re-arrangement, CI-proven.
//!
//! The serve loop of Phase 25 selected its pipeline once; these tests
//! pin the compositor that *follows the topology* instead:
//!
//! * **The swap** — the served connector dies mid-session; the
//!   pipeline migrates to the next connected one (applied disable,
//!   objects released, fresh chain, applied enable, bring-up flip
//!   latched, full re-render), the client's output object is revoked,
//!   a re-bind delivers the new cascade, and the very same surface
//!   presents its next frame on the new display — the desktop
//!   *survives* the swap.
//! * **The dark state** — the last display goes: the pipeline is
//!   disabled honestly, the client learns (`revoked`), the session
//!   keeps serving (commits still commit, attach swaps release their
//!   fences immediately), a grab answers `failed(no_output)`, and a
//!   frame request made in the dark defers (no `frame_target`, no
//!   drop).
//! * **The relight** — the first display after dark: the bring-up
//!   replays, the deferred frame request is answered on the new
//!   timeline, and the waiting desktop paints again.
//! * **Stability** — a connector appearing while the served one lives
//!   is topology noise: no migration, no revocation.
//! * **Re-anchoring** — a swap to a different refresh answers the
//!   next frame request with the *new* nominal rate.
//! * **Mode renegotiation** — the same connector re-negotiating a
//!   different preferred mode migrates (a status-only diff would be
//!   empty; the pipeline comparison catches what the diff cannot).

mod testbench;

use std::io::Write;

use ldp_client::Proxy;
use ldp_core::geometry::Rect;
use ldp_core::wire::Value;
use testbench::*;

use lion_compositor::rearrange::Rearrange;
use lion_compositor::World;

/// The connectors of the mock preset.
const EDP: u32 = 91;
const HDMI: u32 = 93;

fn connector(id: u32) -> ldp_display::ids::ConnectorId {
    ldp_display::ids::ConnectorId::new(id).expect("preset connector id")
}

/// Recorded events of one name (the Collector-side helper the
/// wait-until closures use).
fn events_of<'a>(c: &'a Collector, name: &str) -> Vec<&'a Recorded> {
    c.records.iter().filter(|r| r.event == name).collect()
}

/// A memfd pool carrying explicit pixel bytes.
fn pool_bytes(pixels: &[u8]) -> std::os::fd::OwnedFd {
    let fd = lion_compositor::sys::memfd("hotplug").expect("memfd");
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

/// Drive one presentation cycle: frame request, pool, buffer, attach,
/// damage, commit, presented.
fn present_one_frame(
    client: &mut TestClient,
    shm: &Proxy,
    surface: &Proxy,
    frame_id: u64,
    cookie: u32,
) {
    frame(client, surface, frame_id);
    let pool = create_pool(client, shm, pool_bytes(&quad_pixels()), 16);
    let buffer = create_buffer(client, &pool, 0, 2, 2, 8, 0x3432_5258);
    attach(client, surface, &buffer);
    damage(client, surface, &[Rect::new(0, 0, 2, 2)]);
    commit(client, surface, cookie);
    client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "committed" && r.args[0] == Value::Uint32(cookie))
    });
    client.wait_until(|c| c.records.iter().any(|r| r.event == "presented"));
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

/// A 720p144 mode (the re-anchoring probe: a different nominal).
fn mode_720p144() -> ldp_display::mode::Mode {
    ldp_display::mode::Mode::new(
        178_200,
        1280,
        1390,
        1430,
        1650,
        720,
        725,
        730,
        750,
        ldp_display::mode::ModeFlags::PVSYNC | ldp_display::mode::ModeFlags::NHSYNC,
        ldp_display::mode::ModeType::PREFERRED | ldp_display::mode::ModeType::DRIVER,
    )
}

/// THE exit criterion: the served connector dies mid-session, the
/// pipeline migrates to the next connected one, the client's output
/// object is revoked, the re-bind delivers the new cascade, and the
/// same surface presents its next frame on the new display.
#[test]
fn monitor_swap_migrates_the_live_pipeline() {
    let tb = Testbench::start("hotplug-swap");
    let mut client = TestClient::connect(&tb.addr);
    let shm = client.bind("ldp.core.shm");
    let compositor = client.bind("ldp.core.compositor");
    let surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("create_surface");
    let output = client.bind("ldp.core.output");
    client.wait_until(|c| !events_of(c, "name").is_empty());
    assert_eq!(client.last("name").args[0], Value::String("eDP-1".into()));

    // The desktop is live: one presented frame.
    present_one_frame(&mut client, &shm, &surface, 1, 0xC001);
    let frames_before = tb.frames();

    // The cable pulls: eDP-1 dies, HDMI-A-1 is next in line.
    {
        let mut world = tb.shared.world.lock().expect("world lock");
        unplug(&mut world, EDP);
    }
    match service_hotplug(&tb) {
        Rearrange::Migrated { from, to } => {
            assert_eq!(from.name, "eDP-1");
            assert_eq!((from.width, from.height), (1920, 1080));
            assert_eq!(to.name, "HDMI-A-1");
            assert_eq!((to.width, to.height), (1920, 1080));
        }
        other => panic!("expected a migration, got {other:?}"),
    }

    // The device followed: HDMI bound to the CRTC, eDP unbound, the
    // plane fed, the mode active — the *applied* state, not a claim.
    tb.world(|world| {
        let mock = world.device.as_mock().expect("the mock driver");
        assert_eq!(
            mock.connector_binding(connector(HDMI)),
            Some(world.crtc().expect("lit"))
        );
        assert_eq!(mock.connector_binding(connector(EDP)), None);
        assert!(
            mock.crtc_mode_now(world.crtc().expect("lit")).is_some(),
            "mode active"
        );
        let plane = mock
            .plane_state(world.primary().expect("lit").plane)
            .expect("plane present");
        assert_eq!(
            plane.crtc,
            u64::from(world.crtc().expect("lit").raw()),
            "plane fed"
        );
        assert!(plane.fb != 0, "a fresh framebuffer scans out");
    });

    // The client learned: the old output object is revoked
    // (capability_revoked), nothing else died. The event rides the
    // connection object; its first argument names the revoked object.
    client.wait_until(|c| events_of(c, "revoked").len() == 1);
    {
        let revoked = client.last("revoked");
        assert_eq!(revoked.args[0], Value::Uint32(output.id().as_u32()));
        assert_eq!(revoked.args[1], Value::Enum(2), "capability_revoked");
    }
    // The session is intact — the same surface commits and presents
    // on the new display without reconnecting.
    present_one_frame(&mut client, &shm, &surface, 2, 0xC002);
    assert!(tb.frames() > frames_before, "the desktop renders again");

    // The re-bind delivers the new truth.
    let _output2 = client.bind("ldp.core.output");
    client.wait_until(|c| events_of(c, "name").len() >= 2);
    let names: Vec<&Value> = client
        .events_of("name")
        .iter()
        .map(|r| &r.args[0])
        .collect();
    assert_eq!(
        names.last(),
        Some(&&Value::String("HDMI-A-1".into())),
        "the re-bind cascade names the new output"
    );

    // And the new scanout carries the frame's content: the 2x2 quad
    // at the origin (opaque premultiplied — XRGB input composites
    // opaque) over the black desktop, at the mode's geometry.
    let scanout = tb.scanout();
    assert_eq!(scanout.len(), 1920 * 1080);
    assert_eq!(scanout[0], 0xFFFF_0000, "red, opaque");
    assert_eq!(scanout[1], 0xFF00_FF00, "green, opaque");
    assert_eq!(scanout[1920], 0xFF00_00FF, "blue, opaque");
    assert_eq!(scanout[1921], 0xFFFF_FFFF, "white, opaque");
    assert!(
        scanout[2..1920].iter().all(|w| *w == 0xFF00_0000),
        "black beyond the quad"
    );
    assert!(
        scanout[1922..].iter().all(|w| *w == 0xFF00_0000),
        "black beyond the quad"
    );
}

/// The last display going away is the honest dark state: the pipeline
/// disabled and released, the client told, the session *still
/// serving*, attach swaps releasing their fences immediately, grabs
/// refused with `no_output`, frame requests deferring.
#[test]
fn last_display_unplug_goes_dark_and_keeps_serving() {
    let tb = Testbench::start("hotplug-dark");
    let mut client = TestClient::connect(&tb.addr);
    let shm = client.bind("ldp.core.shm");
    let compositor = client.bind("ldp.core.compositor");
    let surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("create_surface");
    client.bind("ldp.core.output");

    // A live desktop first.
    present_one_frame(&mut client, &shm, &surface, 1, 0xD001);

    // Both connected displays go in one topology event.
    // The primary's plane, remembered across the dark (its id
    // outlives the slot — the object-level assertions read it back).
    let primary_plane = tb.world(|world| world.primary().expect("lit").plane);
    {
        let mut world = tb.shared.world.lock().expect("world lock");
        unplug(&mut world, EDP);
        unplug(&mut world, HDMI);
    }
    match service_hotplug(&tb) {
        Rearrange::Dark { from } => {
            assert_eq!(from.name, "eDP-1");
        }
        other => panic!("expected the dark state, got {other:?}"),
    }

    // The honest device state: everything released.
    tb.world(|world| {
        assert!(world.outputs.is_empty(), "no output model");
        assert!(world.outputs.is_empty(), "no scanout chain");
        assert!(
            world.outputs.iter().all(|slot| slot.lease.is_none()),
            "no lease"
        );
        let mock = world.device.as_mock().expect("the mock driver");
        assert_eq!(mock.connector_binding(connector(EDP)), None);
        assert_eq!(mock.connector_binding(connector(HDMI)), None);
        assert_eq!(
            world
                .outputs
                .iter()
                .filter(|slot| mock.crtc_mode_now(slot.crtc).is_some())
                .count(),
            0,
            "CRTC off"
        );
        let plane = mock.plane_state(primary_plane).expect("plane present");
        assert_eq!(plane.fb, 0, "FB released");
        assert_eq!(plane.crtc, 0, "plane unfed");
    });

    // The client learned the output died...
    client.wait_until(|c| events_of(c, "revoked").len() == 1);
    // ...and the session still serves: a commit commits (no display,
    // no presentation, no error).
    damage(&mut client, &surface, &[Rect::new(0, 0, 2, 2)]);
    commit(&mut client, &surface, 0xD002);
    client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "committed" && r.args[0] == Value::Uint32(0xD002))
    });

    // An attach swap made in the dark releases its superseded buffer
    // immediately — nothing is being read anymore, so the fence does
    // not wait for a flip that will never land.
    let pool = create_pool(&mut client, &shm, pool_bytes(&quad_pixels()), 16);
    let buffer = create_buffer(&mut client, &pool, 0, 2, 2, 8, 0x3432_5258);
    attach(&mut client, &surface, &buffer);
    commit(&mut client, &surface, 0xD003);
    client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "committed" && r.args[0] == Value::Uint32(0xD003))
    });
    client.wait_until(|c| !events_of(c, "release").is_empty());
    assert!(
        client
            .events_of("release")
            .iter()
            .all(|r| r.fence == Some(true)),
        "the dark flush's fences read signalled"
    );

    // A frame request made in the dark defers: no frame_target, no
    // drop — the reply waits for light (the DPMS-off mirror).
    let targets_before = client.events_of("frame_target").len();
    let drops_before = client.events_of("frame_dropped").len();
    frame(&mut client, &surface, 77);
    client.sync();
    assert_eq!(client.events_of("frame_target").len(), targets_before);
    assert_eq!(client.events_of("frame_dropped").len(), drops_before);

    // A grab answers the honest failure.
    let capture = client.bind("ldp.capture.capture_manager");
    client
        .conn
        .send_request(&capture, "grab", vec![])
        .expect("grab");
    client.wait_until(|c| events_of(c, "failed").len() == 1);
    assert_eq!(client.last("failed").args[0], Value::Enum(2), "no_output");
}

/// The first display after dark: the relight replays the bring-up,
/// answers the deferred frame request on the new timeline, and the
/// waiting desktop paints again — the whole cycle without a session
/// lost.
#[test]
fn first_replug_relights_and_the_desktop_paints() {
    let tb = Testbench::start("hotplug-relight");
    let mut client = TestClient::connect(&tb.addr);
    let shm = client.bind("ldp.core.shm");
    let compositor = client.bind("ldp.core.compositor");
    let surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("create_surface");
    client.bind("ldp.core.output");

    // Present, go dark (both displays), then ask for a frame in the
    // dark (the deferred request).
    present_one_frame(&mut client, &shm, &surface, 1, 0xE001);
    {
        let mut world = tb.shared.world.lock().expect("world lock");
        unplug(&mut world, EDP);
        unplug(&mut world, HDMI);
    }
    service_hotplug(&tb);
    client.wait_until(|c| events_of(c, "revoked").len() == 1);
    frame(&mut client, &surface, 2);
    client.sync();
    assert!(
        !client
            .events_of("frame_target")
            .iter()
            .any(|r| r.args[0] == Value::Uint64(2)),
        "the dark request defers"
    );

    // The dock lands: DP-1 comes up.
    {
        let mut world = tb.shared.world.lock().expect("world lock");
        plug_1080p(&mut world, 92);
    }
    match service_hotplug(&tb) {
        Rearrange::Relit { to } => {
            assert_eq!(to.name, "DP-1");
            assert_eq!((to.width, to.height), (1920, 1080));
        }
        other => panic!("expected the relight, got {other:?}"),
    }

    // The deferred request is answered on the new timeline...
    client.wait_until(|c| {
        events_of(c, "frame_target")
            .iter()
            .any(|r| r.args[0] == Value::Uint64(2))
    });
    // ...the re-bind delivers the cascade...
    let _output2 = client.bind("ldp.core.output");
    client.wait_until(|c| events_of(c, "name").len() >= 2);
    assert_eq!(client.last("name").args[0], Value::String("DP-1".into()));
    // ...and the desktop paints again: the same surface, fresh
    // content, presented on the relit display.
    present_one_frame(&mut client, &shm, &surface, 3, 0xE003);

    let scanout = tb.scanout();
    assert_eq!(scanout.len(), 1920 * 1080);
    assert_eq!(scanout[0], 0xFFFF_0000, "red, opaque");
    assert_eq!(scanout[1], 0xFF00_FF00, "green, opaque");
    assert_eq!(scanout[1920], 0xFF00_00FF, "blue, opaque");
    assert_eq!(scanout[1921], 0xFFFF_FFFF, "white, opaque");
}

/// The stability rule: a connector appearing while the served one
/// lives is topology noise — no migration, no revocation, no frames.
#[test]
fn a_new_connector_while_served_is_noise() {
    let tb = Testbench::start("hotplug-noise");
    let mut client = TestClient::connect(&tb.addr);
    let shm = client.bind("ldp.core.shm");
    let compositor = client.bind("ldp.core.compositor");
    let surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("create_surface");
    present_one_frame(&mut client, &shm, &surface, 1, 0xF001);
    let frames_before = tb.frames();

    // DP-1 gains a monitor while eDP-1 still serves.
    {
        let mut world = tb.shared.world.lock().expect("world lock");
        plug_1080p(&mut world, 92);
    }
    match service_hotplug(&tb) {
        Rearrange::Spurious => {}
        other => panic!("expected stability, got {other:?}"),
    }
    client.sync();
    assert_eq!(client.events_of("revoked").len(), 0, "no revocation");
    assert_eq!(tb.frames(), frames_before, "no re-render");
    tb.world(|world| {
        assert_eq!(
            world
                .device
                .as_mock()
                .expect("mock")
                .connector_binding(connector(EDP)),
            Some(world.crtc().expect("lit")),
            "the served pipeline holds"
        );
    });
}

/// A swap to a different refresh re-anchors the scheduler: the next
/// frame request is answered with the *new* nominal rate.
#[test]
fn a_refresh_change_reanchors_the_scheduler() {
    let tb = Testbench::start("hotplug-reanchor");
    let mut client = TestClient::connect(&tb.addr);
    let shm = client.bind("ldp.core.shm");
    let compositor = client.bind("ldp.core.compositor");
    let surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("create_surface");
    present_one_frame(&mut client, &shm, &surface, 1, 0x1001);

    // eDP-1 dies; the dock offers a 144 Hz panel.
    {
        let mut world = tb.shared.world.lock().expect("world lock");
        unplug(&mut world, EDP);
        world
            .device
            .as_mock_mut()
            .expect("the mock driver")
            .hotplug_connect(connector(HDMI), vec![mode_720p144()], vec![0x5Au8; 16]);
    }
    match service_hotplug(&tb) {
        Rearrange::Migrated { from, to } => {
            assert_eq!(from.name, "eDP-1");
            assert_eq!((to.width, to.height), (1280, 720));
        }
        other => panic!("expected a migration, got {other:?}"),
    }

    // The next frame request is answered at 144 Hz (6,944,444 ns
    // nominal), not the 60 Hz the old timeline carried.
    frame(&mut client, &surface, 2);
    client.wait_until(|c| {
        events_of(c, "frame_target")
            .iter()
            .any(|r| r.args[0] == Value::Uint64(2))
    });
    let target = client
        .events_of("frame_target")
        .into_iter()
        .find(|r| r.args[0] == Value::Uint64(2))
        .expect("the answered request");
    match &target.args[2] {
        Value::Uint64(ns) => assert_eq!(*ns, 6_944_444, "the new nominal answers"),
        other => panic!("refresh argument, got {other:?}"),
    }
}

/// A mode renegotiation on the *same* connector (status unchanged,
/// mode list changed) migrates: the pipeline comparison catches what
/// a status-only diff cannot.
#[test]
fn mode_renegotiation_on_the_same_connector_migrates() {
    let tb = Testbench::start("hotplug-renegotiate");
    let mut client = TestClient::connect(&tb.addr);
    let shm = client.bind("ldp.core.shm");
    let compositor = client.bind("ldp.core.compositor");
    let surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("create_surface");
    client.bind("ldp.core.output");
    present_one_frame(&mut client, &shm, &surface, 1, 0x1101);

    // The sink re-negotiates: eDP-1 stays Connected, its mode list
    // becomes a single 720p144 preferred mode.
    {
        let mut world = tb.shared.world.lock().expect("world lock");
        world
            .device
            .as_mock_mut()
            .expect("the mock driver")
            .hotplug_connect(connector(EDP), vec![mode_720p144()], vec![0xA5u8; 16]);
    }
    match service_hotplug(&tb) {
        Rearrange::Migrated { from, to } => {
            assert_eq!(from.connector.raw(), EDP);
            assert_eq!(to.connector.raw(), EDP, "same connector, new truth");
            assert_eq!((from.width, from.height), (1920, 1080));
            assert_eq!((to.width, to.height), (1280, 720));
        }
        other => panic!("expected a renegotiation migration, got {other:?}"),
    }
    // The client re-binds and sees the renegotiated geometry (the
    // eDP panel's physical extents, unchanged by the renegotiation).
    client.wait_until(|c| events_of(c, "revoked").len() == 1);
    let _output2 = client.bind("ldp.core.output");
    client.wait_until(|c| events_of(c, "geometry").len() >= 2);
    let geo = client.last("geometry");
    assert_eq!(geo.args[2], Value::Int32(309), "the renegotiated cascade");
    // The scanout follows the new mode (the migration's re-render or
    // the re-bind's wake — either way, the new geometry).
    assert_eq!(tb.scanout().len(), 1280 * 720);
}

/// Binding the output while dark revokes the fresh object — the
/// client learns "no display right now" instead of waiting for a
/// cascade that cannot come.
///
/// Phase 44 refines the reason: the dark state *withdraws* the output
/// global (every live registry received `global_remove`), so a bind
/// that races or ignores the withdrawal is answered with
/// `interface_removed` — the same vocabulary the registry barrier
/// carried, not the capability vocabulary the pre-withdrawal dark
/// state used.
#[test]
fn binding_while_dark_revokes_the_fresh_object() {
    let tb = Testbench::start("hotplug-bind-dark");
    let mut client = TestClient::connect(&tb.addr);
    {
        let mut world = tb.shared.world.lock().expect("world lock");
        unplug(&mut world, EDP);
        unplug(&mut world, HDMI);
    }
    service_hotplug(&tb);
    // The global is withdrawn (the registry said `global_remove`);
    // the bind still succeeds at the static server layer; the object
    // dies immediately with the honest reason.
    let output = client.bind("ldp.core.output");
    client.wait_until(|c| events_of(c, "revoked").len() == 1);
    let revoked = client.last("revoked");
    assert_eq!(revoked.args[0], Value::Uint32(output.id().as_u32()));
    assert_eq!(revoked.args[1], Value::Enum(1), "interface_removed");
}

/// Phase 44: the registry tells the truth about the display set. The
/// dark state **withdraws** the output global — every live output
/// bind revoked first (the capability vocabulary), then the
/// `global_remove` barrier on the registry (the interface vocabulary)
/// — and the relight re-advertises it, so a fresh bind serves the
/// cascade again. The client's registry stream *is* the display
/// topology's changelog; the races a static advertisement set left
/// (a bind that never learns) are closed.
#[test]
fn the_registry_tracks_the_display_set_honestly() {
    let tb = Testbench::start("dynamic-globals");
    let mut client = TestClient::connect(&tb.addr);
    // The first bind mints the registry (the fan-out target).
    client.bind("ldp.core.shm");
    let registry = client
        .conn
        .registry_proxy()
        .expect("the registry exists after the first bind")
        .id()
        .as_u32();
    let output = client.bind("ldp.core.output");
    client.sync();
    // Baseline: the replay advertised the output global once, and
    // nothing has been withdrawn.
    let advertised = |c: &Collector| {
        c.records
            .iter()
            .filter(|r| r.event == "global" && r.target == registry)
            .filter(|r| r.args.first() == Some(&Value::String("ldp.core.output".into())))
            .count()
    };
    assert_eq!(
        advertised(&client.events),
        1,
        "the replay advertised it once"
    );
    assert!(client.events_of("global_remove").is_empty());

    // Go dark: both displays unplugged.
    {
        let mut world = tb.shared.world.lock().expect("world lock");
        unplug(&mut world, EDP);
        unplug(&mut world, HDMI);
    }
    service_hotplug(&tb);
    // The withdrawal's ordering, as the spec's own sentence demands:
    // the bound output object revoked BEFORE the removal barrier.
    client.wait_until(|c| events_of(c, "global_remove").len() == 1);
    let remove = client.last("global_remove");
    assert_eq!(remove.target, registry, "the barrier lands on the registry");
    assert!(remove.args.is_empty(), "the v1 wire carries zero arguments");
    let revoked_pos = client
        .events
        .records
        .iter()
        .position(|r| r.event == "revoked" && r.args[0] == Value::Uint32(output.id().as_u32()))
        .expect("the live output bind was revoked by the sweep");
    let remove_pos = client
        .events
        .records
        .iter()
        .rposition(|r| r.event == "global_remove")
        .expect("checked above");
    assert!(
        revoked_pos < remove_pos,
        "objects of the interface are revoked before global_remove"
    );
    assert_eq!(
        client.events.records[revoked_pos].args[1],
        Value::Enum(2),
        "the live bind's own revocation keeps the capability vocabulary"
    );

    // A bind while withdrawn: accepted at the static layer, answered
    // with the withdrawal's own vocabulary.
    let late = client.bind("ldp.core.output");
    client.wait_until(|c| events_of(c, "revoked").len() == 2);
    assert_eq!(
        client.last("revoked").args[0],
        Value::Uint32(late.id().as_u32())
    );
    assert_eq!(
        client.last("revoked").args[1],
        Value::Enum(1),
        "interface_removed"
    );

    // The relight: the registry re-advertises the output global, and a
    // fresh bind serves the geometry cascade again.
    {
        let mut world = tb.shared.world.lock().expect("world lock");
        plug_1080p(&mut world, 92);
    }
    service_hotplug(&tb);
    client.wait_until(|c| advertised(c) == 2);
    let fresh = client.bind("ldp.core.output");
    client.wait_until(|c| {
        events_of(c, "mode")
            .iter()
            .any(|r| r.target == fresh.id().as_u32())
    });
    // And the withdrawn set is clean — a second dark round works.
    {
        let mut world = tb.shared.world.lock().expect("world lock");
        unplug(&mut world, 92);
    }
    service_hotplug(&tb);
    client.wait_until(|c| events_of(c, "global_remove").len() == 2);
}
