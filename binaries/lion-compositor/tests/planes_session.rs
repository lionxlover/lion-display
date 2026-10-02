//! Phase 34 exit criteria — the hardware compositing path, end to
//! end through the real wire.
//!
//! The plane engine's session truths: a fullscreen opaque client
//! scans out **untouched** (the zero-composite frame — the renderer
//! emits no pass, the primary plane carries the client's own
//! framebuffer); the underlay split (a window above the fullscreen
//! base rides an overlay plane, the composite canvas beneath it);
//! the honest demotions (Liquid-styled layers composite because
//! their pixels exist only in the render path); plane retirement
//! when the plan shrinks; and the release gates under direct
//! scanout (a superseded buffer frees after the flip that replaces
//! it lands).

mod testbench;

use std::io::Write;

use ldp_client::Proxy;
use ldp_core::geometry::Rect;
use ldp_core::wire::{Primitive, Value};
use ldp_display::ids::PlaneId;
use testbench::*;

use lion_compositor::server::CompositorConfig;
use lion_compositor::World;

const W: u32 = 1920;
const H: u32 = 1080;

/// A memfd pool carrying explicit pixel bytes.
fn pool_bytes(pixels: &[u8]) -> std::os::fd::OwnedFd {
    let fd = lion_compositor::sys::memfd("planes").expect("memfd");
    let mut file = std::fs::File::from(fd);
    file.write_all(pixels).expect("write pixels");
    file.into()
}

/// A fullscreen XRGB pool of one color.
fn fullscreen_pool(client: &mut TestClient, shm: &Proxy, word: u32) -> Proxy {
    // Heap-built (a `[bytes; 2M]` literal would live on the stack —
    // 8 MiB and an overflow on the default test-thread stack).
    let mut pixels = Vec::with_capacity((W * H * 4) as usize);
    for _ in 0..W * H {
        pixels.extend_from_slice(&word.to_le_bytes());
    }
    create_pool(client, shm, pool_bytes(&pixels), (W * H * 4) as i64)
}

fn start(tag: &str) -> Testbench {
    let config = CompositorConfig {
        socket: format!("lion-it-planes-{tag}-{}", std::process::id()),
        ..CompositorConfig::default()
    };
    Testbench::start_with(tag, config)
}

fn start_tiered(tag: &str, tier: ldp_renderer::EffectTier) -> Testbench {
    let config = CompositorConfig {
        socket: format!("lion-it-planes-{tag}-{}", std::process::id()),
        effects: ldp_renderer::EffectChoice::Tier(tier),
        ..CompositorConfig::default()
    };
    Testbench::start_with(tag, config)
}

fn set_opaque(client: &mut TestClient, surface: &Proxy, rect: Rect) {
    client
        .conn
        .send_request(
            surface,
            "set_opaque_region",
            vec![Value::Array {
                element: ldp_core::wire::ArgType::Rect,
                items: vec![Primitive::Rect(rect)].into(),
            }],
        )
        .expect("set_opaque_region");
}

/// Create a fullscreen surface, present it, and wait for the verdict.
fn present_fullscreen(
    tb: &Testbench,
    client: &mut TestClient,
    comp: &Proxy,
    shm: &Proxy,
    word: u32,
    cookie: u32,
) -> Proxy {
    let surface = client
        .conn
        .create_object(comp, "create_surface", vec![])
        .expect("surface");
    let pool = fullscreen_pool(client, shm, word);
    let buffer = create_buffer(
        client,
        &pool,
        0,
        W as i32,
        H as i32,
        (W * 4) as i32,
        0x3432_5258,
    );
    set_opaque(client, &surface, Rect::new(0, 0, W, H));
    frame(client, &surface, 1);
    attach(client, &surface, &buffer);
    damage(client, &surface, &[Rect::new(0, 0, W, H)]);
    commit(client, &surface, cookie);
    // The presented verdict carries the *frame id* (`frame()`'s
    // argument), not the commit cookie.
    client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "presented" && r.args[0] == Value::Uint64(1))
    });
    let _ = (tb, cookie);
    buffer
}

#[test]
fn a_fullscreen_opaque_client_scans_out_untouched() {
    let tb = start("zero");
    let mut client = TestClient::connect(&tb.addr);
    let shm = client.bind("ldp.core.shm");
    let comp = client.bind("ldp.core.compositor");

    let blue = 0x0040_80FFu32; // XRGB: B=0x40? — XRGB words are B,G,R,X on the wire.
    let buffer = present_fullscreen(&tb, &mut client, &comp, &shm, blue, 0x5001);

    tb.world(|world| {
        // The zero-composite frame: no renderer pass ever opened.
        assert!(
            world.zero_pass_frames >= 1,
            "the fullscreen frame was zero-composite (got {})",
            world.zero_pass_frames
        );
        // The import walk registered the client's buffer.
        let imported: Vec<u64> = world
            .imports
            .values()
            .flatten()
            .map(|fb| u64::from(fb.raw()))
            .collect();
        assert_eq!(imported.len(), 1, "one buffer imported");
        // The primary plane carries the CLIENT's framebuffer — not
        // the canvas. The hardware is scanning the client's memory.
        let mock = world.device.as_mock().expect("the mock driver");
        let primary = mock
            .plane_state(PlaneId::new(50).unwrap())
            .expect("plane 50");
        assert_eq!(primary.fb, imported[0], "the client's fb is on the primary");
        assert_eq!(primary.crtc, u64::from(world.outputs[0].crtc.raw()));
        // The visible truth: the client's own pixels everywhere.
        let scanout = world.scanout_words().expect("lit");
        assert_eq!(scanout.len(), (W * H) as usize);
        assert!(scanout.iter().all(|&w| w == blue | 0xFF00_0000));
        // No overlay carries anything.
        assert!(world.outputs[0].active_planes.iter().all(|p| p.raw() == 50));
    });
    let _ = buffer;
}

#[test]
fn the_underlay_split_rides_the_overlay_above_the_canvas() {
    let tb = start("split");
    let mut client = TestClient::connect(&tb.addr);
    let shm = client.bind("ldp.core.shm");
    let comp = client.bind("ldp.core.compositor");

    // The fullscreen base: ARGB with **no opaque coverage claimed** —
    // per-pixel-opaque pixels but a region that leaves the output
    // uncovered, so the base is Overlay-grade rather than Bottom: the
    // zero-composite shape is barred and the frame takes the SPLIT
    // (the base composites onto the canvas, the window rides the
    // overlay above it — the underlay shape).
    let base_word = 0xFF66_33FFu32; // ARGB: opaque blue.
    let mut pixels = Vec::with_capacity((W * H * 4) as usize);
    for _ in 0..W * H {
        pixels.extend_from_slice(&base_word.to_le_bytes());
    }
    let base_pool = create_pool(&mut client, &shm, pool_bytes(&pixels), (W * H * 4) as i64);
    let base_buffer = create_buffer(
        &mut client,
        &base_pool,
        0,
        W as i32,
        H as i32,
        (W * 4) as i32,
        0x3432_5241,
    );
    let base = client
        .conn
        .create_object(&comp, "create_surface", vec![])
        .expect("base surface");
    frame(&mut client, &base, 1);
    attach(&mut client, &base, &base_buffer);
    damage(&mut client, &base, &[Rect::new(0, 0, W, H)]);
    commit(&mut client, &base, 0x5101);
    client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "presented" && r.args[0] == Value::Uint64(1))
    });
    let _ = &base_buffer;

    // A 64x64 opaque window at (100, 100): red.
    let window_word = 0x0000_11FFu32;
    let pixels = [window_word.to_le_bytes(); 64 * 64].concat();
    let pool = create_pool(&mut client, &shm, pool_bytes(&pixels), 64 * 64 * 4);
    let buffer = create_buffer(&mut client, &pool, 0, 64, 64, 256, 0x3432_5258);
    let top = client
        .conn
        .create_object(&comp, "create_surface", vec![])
        .expect("top surface");
    set_opaque(&mut client, &top, Rect::new(0, 0, 64, 64));
    // A distinct frame id (the base's presented already carried 1 —
    // waiting on 1 again would race past the top's own frame).
    frame(&mut client, &top, 7);
    attach(&mut client, &top, &buffer);
    damage(&mut client, &top, &[Rect::new(0, 0, 64, 64)]);
    commit(&mut client, &top, 0x5102);
    client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "presented" && r.args[0] == Value::Uint64(7))
    });

    tb.world(|world| {
        // The split shape: the base composites onto the canvas (the
        // window above it is not fullscreen, so the base does not
        // direct-scan), the window rides overlay 51.
        let mock = world.device.as_mock().expect("the mock driver");
        let overlay51 = mock
            .plane_state(PlaneId::new(51).unwrap())
            .expect("overlay 51");
        assert_ne!(overlay51.fb, 0, "the window rides overlay 51");
        assert_eq!(overlay51.crtc, u64::from(world.outputs[0].crtc.raw()));
        assert_eq!(overlay51.crtc_x, 0);
        assert_eq!(overlay51.crtc_y, 0);
        assert_eq!(overlay51.crtc_w, 64);
        assert_eq!(overlay51.crtc_h, 64);
        assert_eq!(overlay51.zpos, 1, "the stacking zpos");
        // The primary carries the canvas (the composite arm's own
        // framebuffer — the scanout chain's back buffer).
        let primary = mock
            .plane_state(PlaneId::new(50).unwrap())
            .expect("plane 50");
        let canvas_fb = u64::from(world.outputs[0].scanout.fbs[0].raw());
        assert!(
            primary.fb == canvas_fb
                || primary.fb == u64::from(world.outputs[0].scanout.fbs[1].raw()),
            "the canvas is on the primary (fb {})",
            primary.fb
        );
        // The visible truth: blue base with the red window at the
        // origin (the display model blends the overlay over the
        // canvas — the panel's own blend order).
        let scanout = world.scanout_words().expect("lit");
        assert_eq!(
            scanout[0],
            window_word | 0xFF00_0000,
            "the window at the origin"
        );
        let mid = (H / 2) as usize * W as usize + (W / 2) as usize;
        assert_eq!(scanout[mid], base_word, "the base beyond the window");
    });
}

#[test]
fn styled_layers_composite_because_their_pixels_live_in_the_render_path() {
    // The High tier dresses the fullscreen window with rounded
    // corners: the plane would scan the buffer out square — the
    // solver demotes with the reason named.
    let tb = start_tiered("styled", ldp_renderer::EffectTier::High);
    let mut client = TestClient::connect(&tb.addr);
    let shm = client.bind("ldp.core.shm");
    let comp = client.bind("ldp.core.compositor");

    // A 512x512 opaque window: the High tier dresses it (rounded
    // corners, a soft shadow) — the styling that must demote.
    let surface = client
        .conn
        .create_object(&comp, "create_surface", vec![])
        .expect("surface");
    set_opaque(&mut client, &surface, Rect::new(0, 0, 512, 512));
    let mut pixels = Vec::with_capacity(512 * 512 * 4);
    for _ in 0..512 * 512 {
        pixels.extend_from_slice(&0x00FF_00FFu32.to_le_bytes());
    }
    let pool = create_pool(&mut client, &shm, pool_bytes(&pixels), 512 * 512 * 4);
    let buffer = create_buffer(&mut client, &pool, 0, 512, 512, 512 * 4, 0x3432_5258);
    frame(&mut client, &surface, 1);
    attach(&mut client, &surface, &buffer);
    damage(&mut client, &surface, &[Rect::new(0, 0, 512, 512)]);
    commit(&mut client, &surface, 0x5201);
    client.wait_until(|c| c.records.iter().any(|r| r.event == "presented"));

    tb.world(|world| {
        assert_eq!(
            world.zero_pass_frames, 0,
            "a styled frame never zero-composites"
        );
        let mock = world.device.as_mock().expect("the mock driver");
        let overlay51 = mock
            .plane_state(PlaneId::new(51).unwrap())
            .expect("overlay 51");
        assert_eq!(overlay51.fb, 0, "no overlay carries the styled window");
        // The renderer produced the styled frame (the composite arm).
        assert!(world.frames >= 1);
    });
}

#[test]
fn the_overlay_retires_when_the_plan_shrinks() {
    let tb = start("retire");
    let mut client = TestClient::connect(&tb.addr);
    let shm = client.bind("ldp.core.shm");
    let comp = client.bind("ldp.core.compositor");

    // The base + the top window (the underlay split).
    present_fullscreen(&tb, &mut client, &comp, &shm, 0x0066_33FFu32, 0x5301);
    let pixels = [0x0000_11FFu32.to_le_bytes(); 64 * 64].concat();
    let pool = create_pool(&mut client, &shm, pool_bytes(&pixels), 64 * 64 * 4);
    let buffer = create_buffer(&mut client, &pool, 0, 64, 64, 256, 0x3432_5258);
    let top = client
        .conn
        .create_object(&comp, "create_surface", vec![])
        .expect("top surface");
    set_opaque(&mut client, &top, Rect::new(0, 0, 64, 64));
    frame(&mut client, &top, 7);
    attach(&mut client, &top, &buffer);
    damage(&mut client, &top, &[Rect::new(0, 0, 64, 64)]);
    commit(&mut client, &top, 0x5302);
    client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "presented" && r.args[0] == Value::Uint64(7))
    });
    tb.world(|world| {
        let mock = world.device.as_mock().expect("the mock driver");
        assert_ne!(
            mock.plane_state(PlaneId::new(51).unwrap()).unwrap().fb,
            0,
            "the overlay carries the window"
        );
    });

    // The window goes away: the plan shrinks to nothing offloaded —
    // the retire commit turns the overlay OFF (stale scanout above
    // the canvas is the bug this forbids).
    destroy(&mut client, &top);
    destroy(&mut client, &buffer);
    client.wait_until(|c| c.records.iter().any(|r| r.event == "destroyed"));
    client.sync();
    tb.world(|world| {
        let mock = world.device.as_mock().expect("the mock driver");
        assert_eq!(
            mock.plane_state(PlaneId::new(51).unwrap()).unwrap().fb,
            0,
            "the overlay retired with the window"
        );
        // The base composites again (the split's canvas), the visible
        // truth back to the base alone.
        let scanout = world.scanout_words().expect("lit");
        assert_eq!(scanout[0], 0x0066_33FFu32 | 0xFF00_0000);
    });
}

#[test]
fn superseded_buffers_release_under_direct_scanout() {
    let tb = start("release");
    let mut client = TestClient::connect(&tb.addr);
    let shm = client.bind("ldp.core.shm");
    let comp = client.bind("ldp.core.compositor");

    let surface = client
        .conn
        .create_object(&comp, "create_surface", vec![])
        .expect("surface");
    set_opaque(&mut client, &surface, Rect::new(0, 0, W, H));

    // Buffer one scans out directly.
    let pool1 = fullscreen_pool(&mut client, &shm, 0x0000_FF00u32);
    let buffer1 = create_buffer(
        &mut client,
        &pool1,
        0,
        W as i32,
        H as i32,
        (W * 4) as i32,
        0x3432_5258,
    );
    frame(&mut client, &surface, 1);
    attach(&mut client, &surface, &buffer1);
    damage(&mut client, &surface, &[Rect::new(0, 0, W, H)]);
    commit(&mut client, &surface, 0x5401);
    client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "presented" && r.args[0] == Value::Uint64(1))
    });

    // Buffer two replaces it — the superseded buffer must release
    // after the replacing flip lands (the release gate under direct
    // scanout: the plane read it until the next flip passed).
    let pool2 = fullscreen_pool(&mut client, &shm, 0x00FF_0000u32);
    let buffer2 = create_buffer(
        &mut client,
        &pool2,
        0,
        W as i32,
        H as i32,
        (W * 4) as i32,
        0x3432_5258,
    );
    frame(&mut client, &surface, 2);
    attach(&mut client, &surface, &buffer2);
    damage(&mut client, &surface, &[Rect::new(0, 0, W, H)]);
    commit(&mut client, &surface, 0x5402);
    client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "presented" && r.args[0] == Value::Uint64(2))
    });
    client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "release" && r.target == buffer1.id().as_u32())
    });
    tb.world(|world| {
        // Both frames were zero-composite: the release flowed without
        // a single render pass.
        assert!(world.zero_pass_frames >= 2);
        let scanout = world.scanout_words().expect("lit");
        assert!(scanout.iter().all(|&w| w == 0x00FF_0000u32 | 0xFF00_0000));
    });
}

/// The world's own report line sanity (the operator's one glance).
#[test]
fn the_planes_report_names_the_inventory() {
    let tb = start("report");
    let mut client = TestClient::connect(&tb.addr);
    let shm = client.bind("ldp.core.shm");
    let comp = client.bind("ldp.core.compositor");
    present_fullscreen(&tb, &mut client, &comp, &shm, 0x00FF_00FFu32, 0x5501);
    tb.world(|world: &World| {
        let report = world.planes_report();
        assert!(report.contains("2 overlays"), "the report: {report}");
        assert!(report.contains("cursor reserved"), "the report: {report}");
    });
}
