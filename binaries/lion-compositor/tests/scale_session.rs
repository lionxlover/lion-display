//! Phase 31 exit criteria — the fractional scale doctrine, CI-proven.
//!
//! The wire vocabulary always existed (`output.scale` in Q8.8,
//! `surface.set_buffer_scale`, `surface.preferred_scale`); Phase 31
//! makes it *true*: the operator's `--scale F` advertises the real
//! factor on every output, the positioning shell resolves its layout
//! doctrine on the *logical* canvas (a 1080p panel at 2x serves a
//! 960x540 phone-density desktop), placement lands in physical pixels,
//! and clients learn the rendering truth at bind *and* at
//! `enter_output` (the `preferred_scale` hint).

mod testbench;

use std::io::Write;

use ldp_core::geometry::Rect;
use ldp_core::wire::Value;
use testbench::*;

use lion_compositor::server::CompositorConfig;

/// Recorded events of one name.
fn events_of<'a>(c: &'a Collector, name: &str) -> Vec<&'a Recorded> {
    c.records.iter().filter(|r| r.event == name).collect()
}

/// A memfd pool carrying explicit pixel bytes.
fn pool_bytes(pixels: &[u8]) -> std::os::fd::OwnedFd {
    let fd = lion_compositor::sys::memfd("scale").expect("memfd");
    let mut file = std::fs::File::from(fd);
    file.write_all(pixels).expect("write pixels");
    file.into()
}

fn start_scale(tag: &str, scale: f32, dock: lion_compositor::shell::DockMode) -> Testbench {
    let config = CompositorConfig {
        socket: format!("lion-it-scale-{tag}-{}", std::process::id()),
        scale: ldp_core::scale::ScaleFactor::from_f32_lossy(scale).expect("valid scale"),
        shell: lion_compositor::shell::ShellConfig {
            dock,
            dock_thickness: 84,
        },
        ..CompositorConfig::default()
    };
    Testbench::start_with(tag, config)
}

#[test]
fn the_cascade_advertises_the_forced_factor() {
    let tb = start_scale("advertise", 1.5, lion_compositor::shell::DockMode::Off);
    let mut client = TestClient::connect(&tb.addr);
    let _out = client.bind("ldp.core.output");
    let scale = client.last("scale");
    // 1.5 in Q8.8: the fractional truth, no longer pinned to 256.
    assert_eq!(scale.args[0], Value::Uint32(384));
    // The logical canvas: 1920x1080 at 1.5x is a 1280x720 desktop.
    tb.world(|world| {
        assert_eq!(world.output().unwrap().logical_size(), (1280, 720));
    });
}

#[test]
fn twice_scale_lays_out_and_places_on_the_logical_canvas() {
    // The layout doctrine resolves on the logical canvas: a 1080p
    // panel at 2x is a 960x540 logical desktop — the cascade's step
    // is 24 *logical* px (48 physical), and the dock reserves 84
    // *logical* px of thickness (168 physical).
    let tb = start_scale("logical", 2.0, lion_compositor::shell::DockMode::Auto);
    tb.world(|world| {
        // The dock's physical reservation: 84 logical * 2 = 168 px at
        // the bottom of the 1920x1080 panel.
        let dock = world.shell.dock_rect().expect("the dock serves");
        assert_eq!((dock.x, dock.y, dock.w, dock.h), (0, 1080 - 168, 1920, 168));
    });
    // A client's 800x600 buffer (physical pixels) places on the
    // logical cascade: 800x600 physical = 400x300 logical, the first
    // desktop cascade step (0,0) — filling from the physical origin.
    let mut client = TestClient::connect(&tb.addr);
    let shm = client.bind("ldp.core.shm");
    let comp = client.bind("ldp.core.compositor");
    let surface = client
        .conn
        .create_object(&comp, "create_surface", vec![])
        .expect("surface");
    let pixels = vec![0x00FF_0000u32.to_le_bytes(); 800 * 600].concat();
    let pool = create_pool(
        &mut client,
        &shm,
        pool_bytes(&pixels),
        (800 * 600 * 4) as i64,
    );
    let buffer = create_buffer(&mut client, &pool, 0, 800, 600, 3200, 0x3432_5258);
    attach(&mut client, &surface, &buffer);
    damage(&mut client, &surface, &[Rect::new(0, 0, 800, 600)]);
    commit(&mut client, &surface, 0x3201);
    client.wait_until(|c| c.records.iter().any(|r| r.event == "committed"));
    tb.world(|world| {
        let scanout = world.scanout_words().expect("lit");
        let at = |x: usize, y: usize| scanout[y * 1920 + x];
        // The first cascade step is the origin; the window spans its
        // full 800 physical pixels of ink.
        assert_eq!(at(0, 0), 0xFFFF_0000, "the window fills from the origin");
        assert_eq!(at(799, 0), 0xFFFF_0000, "the window's right edge inside");
        assert_eq!(
            at(800, 0),
            0xFF00_0000,
            "past the right edge is the desktop"
        );
        // The second placement would step 24 *logical* px (48
        // physical) — the cascade's geometry rides the scale.
    });
    let _ = buffer;
}

#[test]
fn preferred_scale_hints_arrive_with_enter_output() {
    let tb = start_scale("hint", 1.25, lion_compositor::shell::DockMode::Off);
    let mut client = TestClient::connect(&tb.addr);
    let shm = client.bind("ldp.core.shm");
    let comp = client.bind("ldp.core.compositor");
    let _out = client.bind("ldp.core.output");
    let surface = client
        .conn
        .create_object(&comp, "create_surface", vec![])
        .expect("surface");
    let pixels = [0x0000_FF00u32.to_le_bytes(); 4].concat();
    let pool = create_pool(&mut client, &shm, pool_bytes(&pixels), 16);
    let buffer = create_buffer(&mut client, &pool, 0, 2, 2, 8, 0x3432_5258);
    attach(&mut client, &surface, &buffer);
    damage(&mut client, &surface, &[Rect::new(0, 0, 2, 2)]);
    commit(&mut client, &surface, 0x3202);
    client.wait_until(|c| events_of(c, "enter_output").len() == 1);
    // The rendering truth rides the visibility transition: 1.25x in
    // Q8.8 is 320.
    let hints = client.events_of("preferred_scale");
    assert_eq!(hints.len(), 1, "one hint with the enter");
    assert_eq!(hints[0].args[0], Value::Uint32(320));
    let _ = buffer;
}

#[test]
fn identity_scale_keeps_the_phase_28_geometry() {
    // The default: 256 (1.0x) advertised, the logical canvas equals
    // the physical one, the dock at its logical thickness — every
    // Phase 28 byte.
    let tb = start_scale("identity", 1.0, lion_compositor::shell::DockMode::Auto);
    let mut client = TestClient::connect(&tb.addr);
    let _out = client.bind("ldp.core.output");
    assert_eq!(client.last("scale").args[0], Value::Uint32(256));
    tb.world(|world| {
        assert_eq!(world.output().unwrap().logical_size(), (1920, 1080));
        let dock = world.shell.dock_rect().expect("the dock serves");
        assert_eq!((dock.x, dock.y, dock.w, dock.h), (0, 1080 - 84, 1920, 84));
    });
}
