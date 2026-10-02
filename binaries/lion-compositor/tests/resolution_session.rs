//! Phase 30's exit criterion: the sized output doctrine (`--resolution
//! WxH`), end to end through the real bring-up and the real protocol.
//!
//! * **The forced size that is offered serves it** — the compositor
//!   comes up on exactly that mode, the client sees that geometry,
//!   and the desktop renders on it.
//! * **The forced size nothing offers fails honestly** — the typed
//!   startup failure naming every size the panel *does* offer (the
//!   operator's menu, never a silent nearest-neighbor guess).
//! * **The migration honors the forced size** — a hotplug swap picks
//!   the next display *at the operator's size* when it offers one.
//! * **The migration falls back honestly** — a display that does not
//!   offer the size takes the unsized doctrine (the migration never
//!   dies on the operator's preference); the world's mode and the
//!   scanout geometry follow.

mod testbench;

use std::io::Write;

use ldp_client::Proxy;
use ldp_core::geometry::Rect;
use ldp_core::wire::Value;
use testbench::*;

use lion_compositor::rearrange::Rearrange;
use lion_compositor::server::{Compositor, CompositorConfig};
use lion_compositor::World;

/// The connectors of the mock preset.
const EDP: u32 = 91;
const HDMI: u32 = 93;

fn connector(id: u32) -> ldp_display::ids::ConnectorId {
    ldp_display::ids::ConnectorId::new(id).expect("preset connector id")
}

/// Recorded events of one name.
fn events_of<'a>(c: &'a Collector, name: &str) -> Vec<&'a Recorded> {
    c.records.iter().filter(|r| r.event == name).collect()
}

/// A memfd pool carrying explicit pixel bytes.
fn pool_bytes(pixels: &[u8]) -> std::os::fd::OwnedFd {
    let fd = lion_compositor::sys::memfd("resolution").expect("memfd");
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

/// Inject a topology change and service it once — the summary.
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

/// A 540x960@60 portrait mode (a display without the forced size).
fn phone_mode() -> ldp_display::mode::Mode {
    ldp_display::mode::Mode::new(
        42_000,
        540,
        560,
        580,
        700,
        960,
        970,
        975,
        1000,
        ldp_display::mode::ModeFlags::PVSYNC | ldp_display::mode::ModeFlags::NHSYNC,
        ldp_display::mode::ModeType::PREFERRED | ldp_display::mode::ModeType::DRIVER,
    )
}

fn unique_config(resolution: Option<ldp_display::serve::ModeSize>) -> CompositorConfig {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    CompositorConfig {
        socket: format!("lion-it-res-{n}-{}", std::process::id()),
        resolution,
        ..CompositorConfig::default()
    }
}

/// THE bring-up criterion: the forced size the panel offers is the
/// mode the whole compositor serves — the client binds and hears the
/// geometry, the desktop presents, and the scanout carries the pixels
/// at exactly that geometry.
#[test]
fn forced_size_that_is_offered_serves_it() {
    let config = unique_config(Some(ldp_display::serve::ModeSize {
        width: 1920,
        height: 1080,
    }));
    let tb = Testbench::start_with("res-hit", config);

    // The world's truth: the forced mode is active.
    tb.world(|world| {
        let mode = world.output().expect("lit").mode();
        assert_eq!(
            (u32::from(mode.hdisplay), u32::from(mode.vdisplay)),
            (1920, 1080)
        );
        assert_eq!(mode.refresh_hz(), 60, "the best 1080p: the preferred 60 Hz");
    });

    // The protocol truth: a bound client hears the geometry.
    let mut client = TestClient::connect(&tb.addr);
    let shm = client.bind("ldp.core.shm");
    let compositor = client.bind("ldp.core.compositor");
    let surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("create_surface");
    let _output = client.bind("ldp.core.output");
    client.wait_until(|c| !events_of(c, "name").is_empty());
    client.wait_until(|c| !events_of(c, "mode").is_empty());
    // The mode list is advertised with the current mode flagged
    // (bit 1) — the forced 1080p is the active truth. The 59.94 Hz
    // sibling carries the same size, so the flagged one must be the
    // 60 Hz preferred pick.
    let modes = client.events_of("mode");
    let current: Vec<&Recorded> = modes
        .iter()
        .copied()
        .filter(|r| matches!(&r.args[1], Value::Bitset(b) if b.test(1)))
        .collect();
    assert_eq!(current.len(), 1, "exactly one flagged current mode");
    assert_eq!(current[0].args[2], Value::Uint32(1920), "the forced width");
    assert_eq!(current[0].args[3], Value::Uint32(1080), "the forced height");
    assert_eq!(
        current[0].args[4],
        Value::Uint32(60_000),
        "the best 1080p: the preferred 60 Hz, not the 59.94 sibling"
    );

    // The desktop presents on the forced geometry.
    present_one_frame(&mut client, &shm, &surface, 1, 0x5301);
    let scanout = tb.scanout();
    assert_eq!(scanout.len(), 1920 * 1080);
    assert_eq!(scanout[0], 0xFFFF_0000, "red, opaque");
    assert_eq!(scanout[1], 0xFF00_FF00, "green, opaque");
    assert_eq!(scanout[1920], 0xFF00_00FF, "blue, opaque");
    assert_eq!(scanout[1921], 0xFFFF_FFFF, "white, opaque");
    assert_eq!(scanout[1920 * 1079 + 1919], 0xFF00_0000, "black desktop");
}

/// THE honesty criterion: a forced size nothing offers is the typed
/// startup failure carrying the offered menu — never a silent
/// nearest-neighbor guess.
#[test]
fn forced_size_that_is_not_offered_fails_with_the_menu() {
    let config = unique_config(Some(ldp_display::serve::ModeSize {
        width: 2560,
        height: 1440,
    }));
    let Err(err) = Compositor::headless(config) else {
        panic!("the miss must fail bring-up")
    };
    let text = err.to_string();
    assert!(text.contains("no 2560x1440 mode"), "names the miss: {text}");
    assert!(
        text.contains("offered: 1920x1080"),
        "teaches the menu: {text}"
    );
}

/// THE migration criterion: a hotplug swap prefers the next display
/// **at the operator's size** when it offers one — the forced size
/// rides every selection, not just bring-up.
#[test]
fn migration_honors_the_forced_size() {
    let config = unique_config(Some(ldp_display::serve::ModeSize {
        width: 1920,
        height: 1080,
    }));
    let tb = Testbench::start_with("res-migrate", config);

    // A second display appears offering only 720p — not the forced
    // size. While eDP lives, it is topology noise.
    {
        let mut world = tb.shared.world.lock().expect("world lock");
        world
            .device
            .as_mock_mut()
            .expect("the mock driver")
            .hotplug_connect(
                connector(HDMI),
                vec![ldp_display::mode::Mode::panel_1080p60(), phone_mode()],
                vec![0x5Au8; 16],
            );
    }
    assert!(matches!(service_hotplug(&tb), Rearrange::Spurious));

    // The cable pulls: eDP dies; HDMI is next in line and it offers
    // the forced size — the migration lands on 1080p, not the
    // portrait mode also in its list.
    {
        let mut world = tb.shared.world.lock().expect("world lock");
        unplug(&mut world, EDP);
    }
    match service_hotplug(&tb) {
        Rearrange::Migrated { from, to } => {
            assert_eq!(from.name, "eDP-1");
            assert_eq!((from.width, from.height), (1920, 1080));
            assert_eq!(to.name, "HDMI-A-1");
            assert_eq!((to.width, to.height), (1920, 1080), "the forced size wins");
        }
        other => panic!("expected a migration, got {other:?}"),
    }
    tb.world(|world| {
        let mode = world.output().expect("lit").mode();
        assert_eq!(
            (u32::from(mode.hdisplay), u32::from(mode.vdisplay)),
            (1920, 1080)
        );
    });

    // HDMI re-negotiates to a phone-only list: the forced size is
    // gone, and the migration must not die on the operator's
    // preference — the unsized doctrine (the preferred mode) stands.
    {
        let mut world = tb.shared.world.lock().expect("world lock");
        world
            .device
            .as_mock_mut()
            .expect("the mock driver")
            .hotplug_connect(connector(HDMI), vec![phone_mode()], vec![]);
    }
    match service_hotplug(&tb) {
        Rearrange::Migrated { to, .. } => {
            assert_eq!((to.width, to.height), (540, 960), "the honest fallback");
        }
        other => panic!("expected a mode migration, got {other:?}"),
    }
    tb.world(|world| {
        let mode = world.output().expect("lit").mode();
        assert_eq!(
            (u32::from(mode.hdisplay), u32::from(mode.vdisplay)),
            (540, 960)
        );
        // The world remembers the operator's preference for the next
        // display that does offer it.
        assert_eq!(
            world.resolution,
            Some(ldp_display::serve::ModeSize {
                width: 1920,
                height: 1080
            })
        );
    });
}
