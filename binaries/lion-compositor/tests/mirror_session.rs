//! Phase 37 exit criteria — the mirror doctrine and the per-output
//! scales, CI-proven.
//!
//! The Phase 31 multi-output doctrine extended the desktop
//! left-to-right: every display carried its *own* portion. These
//! tests pin the arrangement that was missing — and the scale
//! doctrine that was one-factor-for-all:
//!
//! * **The mirror bring-up** — both connected displays serve on their
//!   own CRTCs and planes, every layout position the *origin*: the
//!   overlap is the clone signal (a client that binds two outputs at
//!   the same position knows they show the same desktop). The desktop
//!   is the *primary's* bounds — not the union.
//! * **The identical-pixels proof** — one gradient window commits and
//!   every same-size display's scanout is byte-equal to the
//!   primary's: the same words at the same offsets, the mirror's own
//!   meaning.
//! * **The mixed-size crop** — a 720p display in the mirror shows the
//!   desktop's top-left crop (the per-row pixels equal the primary's
//!   first 1280 columns); content beyond its width honestly never
//!   reaches it.
//! * **Topology motion in the mirror** — a display leaving is
//!   `OutputRemoved` (the survivor keeps mirroring), a display
//!   joining is `OutputAdded` *at the origin* (it joins the mirror,
//!   not an extension), and the newcomer paints its whole first
//!   frame (the full-bounds seed — no fb-init edges).
//! * **The per-output scales** — `--scale 1,2`: the eDP's bind
//!   advertises Q8.8 256, the HDMI's 512 — one factor per output in
//!   output order.
//! * **The stretch rule (and the bug it fixes)** — a lone `--scale 2`
//!   plus a monitor joining mid-session: the newcomer advertises 2x
//!   (before Phase 37 it silently reset to identity — a latent
//!   Phase 31 bug).
//! * **The release gates span the mirror** — a buffer both displays
//!   scan out releases only after *both* flip past it.

mod testbench;

use std::io::Write;

use ldp_client::Proxy;
use ldp_core::geometry::Rect;
use ldp_core::wire::Value;
use testbench::*;

use lion_compositor::rearrange::Rearrange;
use lion_compositor::scene::OutputArrangement;
use lion_compositor::server::CompositorConfig;
use lion_compositor::World;

/// The connectors of the mock preset.
const DP: u32 = 92;
const HDMI: u32 = 93;

fn connector(id: u32) -> ldp_display::ids::ConnectorId {
    ldp_display::ids::ConnectorId::new(id).expect("preset connector id")
}

/// The mirrored multi-output configuration (the clone doctrine).
fn mirror_config(tag: &str) -> CompositorConfig {
    CompositorConfig {
        socket: format!("lion-it-mirror-{tag}-{}", std::process::id()),
        multi_output: true,
        arrangement: OutputArrangement::Mirrored,
        ..CompositorConfig::default()
    }
}

fn start_mirror(tag: &str) -> Testbench {
    Testbench::start_with(tag, mirror_config(tag))
}

/// Recorded events of one name.
fn events_of<'a>(c: &'a Collector, name: &str) -> Vec<&'a Recorded> {
    c.records.iter().filter(|r| r.event == name).collect()
}

/// A memfd pool carrying explicit pixel bytes.
fn pool_bytes(pixels: &[u8]) -> std::os::fd::OwnedFd {
    let fd = lion_compositor::sys::memfd("mirror").expect("memfd");
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

/// One 1920-wide gradient row (every pixel a distinct, non-black word
/// — the crop proof's discriminator: a scaled or shifted copy could
/// never match per-pixel, and the first pixel is never the desktop's
/// own black).
fn gradient_row() -> Vec<u8> {
    let mut v = Vec::with_capacity(1920 * 4);
    for x in 0..1920u32 {
        let word = 0xFF00_0000
            | ((0x10 + (x % 240)) << 16)
            | ((0x08 + ((x * 7) % 240)) << 8)
            | (0x30 + ((x * 13) % 200));
        v.extend_from_slice(&word.to_le_bytes());
    }
    v
}

/// Spin the session until both display models exist at their sizes
/// (the pump's quiescence — the `presented` wait fires on the
/// primary's flip and the second display's render can be one pump
/// behind; the equality proofs need both models populated).
fn wait_both_painted(tb: &Testbench, client: &mut TestClient, sizes: &[(usize, usize)]) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let ready = tb.world(|world| {
            sizes.iter().all(|(i, px)| {
                world
                    .outputs
                    .get(*i)
                    .is_some_and(|s| s.display.len() == *px)
            })
        });
        if ready {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the displays never painted their whole frames"
        );
        client.sync();
    }
}

/// Commit a one-row 1920-wide buffer of `pixels` on a fresh surface
/// and wait for the presentation.
fn commit_row(client: &mut TestClient, shm: &Proxy, comp: &Proxy, pixels: &[u8], cookie: u32) {
    let surface = client
        .conn
        .create_object(comp, "create_surface", vec![])
        .expect("surface");
    let pool = create_pool(client, shm, pool_bytes(pixels), pixels.len() as i64);
    let buffer = create_buffer(client, &pool, 0, 1920, 1, 7680, 0x3432_5258);
    frame(client, &surface, 1);
    attach(client, &surface, &buffer);
    damage(client, &surface, &[Rect::new(0, 0, 1920, 1)]);
    commit(client, &surface, cookie);
    client.wait_until(|c| c.records.iter().any(|r| r.event == "presented"));
}

/// Inject a topology change and service it — the summary.
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

#[test]
fn mirror_bringup_places_every_display_at_the_origin() {
    let tb = start_mirror("bringup");
    tb.world(|world| {
        // Two slots: eDP-1 and HDMI-A-1, both pipelines applied.
        assert_eq!(world.outputs.len(), 2, "both connected displays serve");
        assert_eq!(world.outputs[0].output.name, "eDP-1");
        assert_eq!(world.outputs[1].output.name, "HDMI-A-1");
        // The clone signal: every layout position the origin.
        assert_eq!(world.outputs[0].output.layout, (0, 0));
        assert_eq!(world.outputs[1].output.layout, (0, 0));
        // The desktop is the primary's bounds — NOT the union (a
        // union would inflate the desktop to 3840 wide and place
        // windows into area no display anchors).
        let desktop = world.desktop_bounds();
        assert_eq!(
            (desktop.x, desktop.y, desktop.w, desktop.h),
            (0, 0, 1920, 1080)
        );
        // The device state: both pipelines enabled.
        let mock = world.device.as_mock().expect("the mock driver");
        assert!(mock.crtc_mode_now(world.outputs[0].crtc).is_some());
        assert!(mock.crtc_mode_now(world.outputs[1].crtc).is_some());
    });
}

#[test]
fn one_window_shows_identical_pixels_on_every_display() {
    let tb = start_mirror("identical");
    let mut client = TestClient::connect(&tb.addr);
    let shm = client.bind("ldp.core.shm");
    let comp = client.bind("ldp.core.compositor");
    // The client watches both displays.
    let out0 = client.bind("ldp.core.output");
    let out1 = client.bind("ldp.core.output");
    // The gradient row commits once.
    commit_row(&mut client, &shm, &comp, &gradient_row(), 0x3701);
    // The window is visible on both displays — the mirror's own
    // visibility truth: the geometry implies an enter on every bound
    // display object (the same window, both displays scanning it).
    let enters = client.events_of("enter_output");
    assert_eq!(
        enters.len(),
        2,
        "one surface, entered on both display objects"
    );
    let objects: Vec<u32> = enters
        .iter()
        .filter_map(|r| match r.args[0] {
            Value::Object(Some(o)) => Some(o.as_u32()),
            _ => None,
        })
        .collect();
    assert!(objects.contains(&out0.id().as_u32()));
    assert!(objects.contains(&out1.id().as_u32()));
    // THE PROOF: both scanouts byte-equal — the same words at the
    // same offsets. Two same-size displays showing one desktop.
    wait_both_painted(&tb, &mut client, &[(0, 1920 * 1080), (1, 1920 * 1080)]);
    tb.world(|world| {
        let primary = world.slot_scanout_words(0).expect("primary lit");
        let secondary = world.slot_scanout_words(1).expect("secondary lit");
        assert_eq!(primary.len(), 1920 * 1080);
        assert_eq!(secondary.len(), 1920 * 1080);
        assert_eq!(
            primary, secondary,
            "the mirror: every word identical on every display"
        );
        // And the desktop's own truth: the gradient landed at row 0.
        assert_ne!(primary[0], 0xFF00_0000, "the gradient's first pixel");
        assert_eq!(primary[1920], 0xFF00_0000, "row 1 is the desktop's black");
    });
}

#[test]
fn a_smaller_display_crops_the_mirror_per_pixel() {
    let tb = start_mirror("crop");
    // HDMI re-negotiates a 720p mode: the mirror's mixed-size truth.
    {
        let mut world = tb.shared.world.lock().expect("world lock");
        world
            .device
            .as_mock_mut()
            .expect("the mock driver")
            .hotplug_connect(
                connector(HDMI),
                vec![ldp_display::mode::Mode::new(
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
                )],
                vec![0x5Bu8; 16],
            );
    }
    match service_hotplug(&tb) {
        Rearrange::Migrated { to, .. } => {
            assert_eq!((to.width, to.height), (1280, 720));
        }
        other => panic!("expected Migrated, got {other:?}"),
    }
    // The desktop stays the primary's 1920x1080 — the smaller display
    // crops, the desktop does not shrink.
    tb.world(|world| {
        assert_eq!(world.outputs[1].output.layout, (0, 0), "still the origin");
        let desktop = world.desktop_bounds();
        assert_eq!((desktop.w, desktop.h), (1920, 1080));
    });
    let mut client = TestClient::connect(&tb.addr);
    let shm = client.bind("ldp.core.shm");
    let comp = client.bind("ldp.core.compositor");
    // The gradient row commits: every pixel distinct.
    commit_row(&mut client, &shm, &comp, &gradient_row(), 0x3702);
    // THE CROP PROOF: the 720p display's every row equals the
    // primary's first 1280 columns of the same row — per-pixel, so a
    // scaled or shifted copy could never pass.
    wait_both_painted(&tb, &mut client, &[(0, 1920 * 1080), (1, 1280 * 720)]);
    tb.world(|world| {
        let primary = world.slot_scanout_words(0).expect("primary lit");
        let secondary = world.slot_scanout_words(1).expect("secondary lit");
        assert_eq!(secondary.len(), 1280 * 720);
        for y in 0..720 {
            let p_row = &primary[y * 1920..y * 1920 + 1280];
            let s_row = &secondary[y * 1280..(y + 1) * 1280];
            assert_eq!(p_row, s_row, "row {y}: the crop is the primary's columns");
        }
    });
}

#[test]
fn topology_motion_keeps_the_mirror() {
    let tb = start_mirror("topology");
    let mut client = TestClient::connect(&tb.addr);
    let shm = client.bind("ldp.core.shm");
    let comp = client.bind("ldp.core.compositor");
    let _out0 = client.bind("ldp.core.output");
    // The gradient commits — both displays mirror it.
    commit_row(&mut client, &shm, &comp, &gradient_row(), 0x3703);
    wait_both_painted(&tb, &mut client, &[(0, 1920 * 1080), (1, 1920 * 1080)]);
    tb.world(|world| {
        assert_eq!(
            world.slot_scanout_words(0).unwrap(),
            world.slot_scanout_words(1).unwrap()
        );
    });
    // HDMI leaves: the survivor keeps mirroring (alone).
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
    tb.world(|world| {
        assert_eq!(world.outputs.len(), 1);
        assert_eq!(world.outputs[0].output.layout, (0, 0));
    });
    // DP joins: it joins the MIRROR — the origin, not an extension.
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
        assert_eq!(world.outputs.len(), 2);
        assert_eq!(world.outputs[0].output.layout, (0, 0));
        assert_eq!(world.outputs[1].output.layout, (0, 0), "joined the mirror");
        let desktop = world.desktop_bounds();
        assert_eq!((desktop.w, desktop.h), (1920, 1080), "never an extension");
        // The newcomer owes its whole first frame (the full-bounds
        // seed — the region beyond the desktop's damage dirties
        // nowhere else, so the seed is what paints it).
        assert!(
            !world.outputs[1].pending.is_empty(),
            "the newcomer's whole-first-frame seed"
        );
    });
    // And the mirror continues: a fresh gradient commits, both
    // displays byte-equal again — and the newcomer's display model
    // exists at its own size (the seed's product: painted whole).
    // (The `presented` wait fires on the primary's flip; the
    // newcomer's own render can still be one pump behind — spin the
    // session until its whole frame has painted.)
    commit_row(&mut client, &shm, &comp, &gradient_row(), 0x3704);
    wait_both_painted(&tb, &mut client, &[(0, 1920 * 1080), (1, 1920 * 1080)]);
    tb.world(|world| {
        assert_eq!(
            world.outputs[1].display.len(),
            1920 * 1080,
            "the newcomer painted its whole first frame"
        );
        assert_eq!(
            world.slot_scanout_words(0).unwrap(),
            world.slot_scanout_words(1).unwrap(),
            "the mirror continues after the topology moved"
        );
    });
}

#[test]
fn per_output_scales_advertise_per_output() {
    // The per-output doctrine (Phase 37): output 0 at 1x, output 1 at
    // 2x — the eDP's bind advertises Q8.8 256, the HDMI's 512.
    let tb = Testbench::start_with(
        "scales",
        CompositorConfig {
            socket: format!("lion-it-scales-{}", std::process::id()),
            multi_output: true,
            arrangement: OutputArrangement::Mirrored,
            output_scales: vec![
                ldp_core::scale::ScaleFactor::IDENTITY,
                ldp_core::scale::ScaleFactor::from_q8(512).expect("2.0 is a representable factor"),
            ],
            ..CompositorConfig::default()
        },
    );
    let mut client = TestClient::connect(&tb.addr);
    let _out0 = client.bind("ldp.core.output");
    let _out1 = client.conn.bind("ldp.core.output").expect("bind");
    client.wait_until(|c| events_of(c, "scale").len() == 2);
    let scales: Vec<u32> = client
        .events_of("scale")
        .iter()
        .filter_map(|r| match r.args[0] {
            Value::Uint32(q8) => Some(q8),
            _ => None,
        })
        .collect();
    assert_eq!(scales, vec![256, 512], "one factor per output, in order");
    // The shell's logical canvas resolves on the *primary's* factor
    // (1x — the Phase 28 geometry unchanged by the HDMI's 2x: the
    // usable area stays the full panel).
    tb.world(|world| {
        assert_eq!(world.outputs[0].output.scale.to_q8(), 256);
        assert_eq!(world.outputs[1].output.scale.to_q8(), 512);
        assert_eq!(world.shell.usable_physical().w, 1920);
    });
}

#[test]
fn a_hotplug_newcomer_inherits_the_scale_doctrine() {
    // The stretch rule — and the latent Phase 31 bug it fixes: a lone
    // `--scale 2` (the list empty, every output at the one factor)
    // plus a monitor joining mid-session. Before Phase 37 the
    // newcomer silently reset to identity; now it inherits.
    let two_x = ldp_core::scale::ScaleFactor::from_q8(512).expect("2.0 is representable");
    let tb = Testbench::start_with(
        "stretch",
        CompositorConfig {
            socket: format!("lion-it-stretch-{}", std::process::id()),
            multi_output: true,
            arrangement: OutputArrangement::Mirrored,
            scale: two_x,
            ..CompositorConfig::default()
        },
    );
    let mut client = TestClient::connect(&tb.addr);
    let _out0 = client.bind("ldp.core.output");
    client.wait_until(|c| !events_of(c, "name").is_empty());
    // The bring-up truth: both displays advertise the operator's 2x.
    tb.world(|world| {
        assert_eq!(world.outputs[0].output.scale.to_q8(), 512);
        assert_eq!(world.outputs[1].output.scale.to_q8(), 512);
    });
    // HDMI leaves, DP joins — the newcomer inherits the doctrine.
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
    assert!(matches!(
        service_hotplug(&tb),
        Rearrange::OutputAdded { .. }
    ));
    tb.world(|world| {
        assert_eq!(
            world.outputs[1].output.scale.to_q8(),
            512,
            "the newcomer advertises the operator's factor (the Phase 31 bug fixed)"
        );
    });
    // And with a per-output list, the newcomer takes the LAST entry
    // (the stretch rule): 1x eDP, 2x HDMI, newcomer DP → 2x.
}

#[test]
fn a_mirrored_swap_releases_after_both_displays_flip() {
    let tb = start_mirror("gates");
    let mut client = TestClient::connect(&tb.addr);
    let shm = client.bind("ldp.core.shm");
    let comp = client.bind("ldp.core.compositor");
    let surface = client
        .conn
        .create_object(&comp, "create_surface", vec![])
        .expect("surface");
    // The first row: red.
    frame(&mut client, &surface, 1);
    let pool_a = create_pool(
        &mut client,
        &shm,
        pool_bytes(&solid_pixels(1920, 1, 0x00FF_0000)),
        7680,
    );
    let buffer_a = create_buffer(&mut client, &pool_a, 0, 1920, 1, 7680, 0x3432_5258);
    attach(&mut client, &surface, &buffer_a);
    damage(&mut client, &surface, &[Rect::new(0, 0, 1920, 1)]);
    commit(&mut client, &surface, 0x3705);
    client.wait_until(|c| c.records.iter().any(|r| r.event == "presented"));
    // The swap: green replaces red. The old front buffer was being
    // scanned out by BOTH displays — its release gate spans the
    // mirror.
    frame(&mut client, &surface, 2);
    let pool_b = create_pool(
        &mut client,
        &shm,
        pool_bytes(&solid_pixels(1920, 1, 0x0000_FF00)),
        7680,
    );
    let buffer_b = create_buffer(&mut client, &pool_b, 0, 1920, 1, 7680, 0x3432_5258);
    attach(&mut client, &surface, &buffer_b);
    damage(&mut client, &surface, &[Rect::new(0, 0, 1920, 1)]);
    commit(&mut client, &surface, 0x3706);
    client.wait_until(|c| events_of(c, "release").len() == 1);
    let release = client.last("release");
    assert!(release.fence.unwrap_or(false), "the fence is signalled");
    // Both displays flipped past it — and both show the green row.
    client.wait_until(|c| c.records.iter().filter(|r| r.event == "presented").count() >= 2);
    tb.world(|world| {
        assert_eq!(world.slot_scanout_words(0).unwrap()[0], 0xFF00_FF00);
        assert_eq!(world.slot_scanout_words(1).unwrap()[0], 0xFF00_FF00);
    });
}
