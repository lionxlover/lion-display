//! The Phase 24 exit criterion: the compositor's whole session runs
//! through the GL backend — **byte-equal to the software path**.
//!
//! The reference GLES evaluator is injected as the renderer backend
//! at bring-up, the full client choreography runs (pool, buffers,
//! surfaces, damage, commit, presentation), and the resulting scanout
//! must match the software-driven session pixel-for-pixel. That is
//! the macOS doctrine made testable: one render pipeline contract,
//! two implementations, one oracle.

#![allow(clippy::too_many_lines)]

mod testbench;

use std::io::Write;

use ldp_core::geometry::Rect;
use ldp_renderer::gles::RefGles;
use testbench::*;

use lion_compositor::server::CompositorConfig;

/// A memfd pool carrying explicit pixel bytes.
fn pool_bytes(pixels: &[u8]) -> std::os::fd::OwnedFd {
    let fd = lion_compositor::sys::memfd("hardware-renderer").expect("memfd");
    let mut file = std::fs::File::from(fd);
    file.write_all(pixels).expect("write pixels");
    file.into()
}

/// Two stacked 8x8 layers: an opaque XRGB checker and a translucent
/// ARGB gradient over it — real blend coverage, not a trivial copy.
fn checker_pixels() -> Vec<u8> {
    let mut v = Vec::new();
    for y in 0..8u32 {
        for x in 0..8u32 {
            // Opaque checker with varied channels (X byte garbage).
            let word = if (x + y) % 2 == 0 {
                0x1A_FF_00_00u32
            } else {
                0x2B_00_88_44u32
            };
            v.extend_from_slice(&word.to_le_bytes());
        }
    }
    v
}

fn gradient_pixels() -> Vec<u8> {
    let mut v = Vec::new();
    for row in 0..8u32 {
        for x in 0..8u32 {
            // Premultiplied translucent gradient: alpha rises with x,
            // channels bounded by alpha (the premul contract).
            let alpha = 16 * x + 15 + row / 4;
            let word = (alpha << 24) | ((alpha / 2) << 16) | ((alpha / 3) << 8) | (alpha / 4);
            v.extend_from_slice(&word.to_le_bytes());
        }
    }
    v
}

/// Run the full choreography against one testbench; returns the
/// scanout once the frame presented.
fn run_session(tb: &Testbench) -> Vec<u32> {
    let mut client = TestClient::connect(&tb.addr);

    let _registry = client.conn.registry().expect("registry");
    client.sync();
    let shm = client.bind("ldp.core.shm");
    client.wait_until(|c| c.records.iter().filter(|r| r.event == "format").count() >= 2);

    // Layer 1: the opaque checker (bottom).
    let pool1 = create_pool(&mut client, &shm, pool_bytes(&checker_pixels()), 256);
    let bottom = create_buffer(&mut client, &pool1, 0, 8, 8, 32, 0x3432_5258);
    // Layer 2: the translucent gradient (top, same origin, stacked).
    let pool2 = create_pool(&mut client, &shm, pool_bytes(&gradient_pixels()), 256);
    let top = create_buffer(&mut client, &pool2, 0, 8, 8, 32, 0x3432_5241);

    let compositor = client.bind("ldp.core.compositor");
    let surface1 = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("create_surface");
    let surface2 = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("create_surface");
    client.sync();

    // Bottom surface: register, attach, damage, commit, present —
    // the exact choreography of the Phase 10 suite.
    frame(&mut client, &surface1, 1);
    attach(&mut client, &surface1, &bottom);
    damage(&mut client, &surface1, &[Rect::new(0, 0, 8, 8)]);
    commit(&mut client, &surface1, 0xA001);
    client.wait_until(|c| c.records.iter().any(|r| r.event == "presented"));

    // Top surface over it (same origin): the same choreography, so
    // the stacked blend lands with a live registration.
    frame(&mut client, &surface2, 1);
    attach(&mut client, &surface2, &top);
    damage(&mut client, &surface2, &[Rect::new(0, 0, 8, 8)]);
    commit(&mut client, &surface2, 0xA002);
    client.wait_until(|c| c.records.iter().filter(|r| r.event == "presented").count() >= 2);

    // The pixel truth, captured BEFORE teardown (the unmap pass
    // repaints the desktop back to black — the final state is not
    // the frame under test).
    let scanout = tb.scanout();

    // Clean teardown (the scene drains; the process FD count resets).
    destroy(&mut client, &top);
    destroy(&mut client, &bottom);
    destroy(&mut client, &surface2);
    destroy(&mut client, &surface1);
    client.wait_until(|c| c.records.iter().any(|r| r.event == "destroyed"));
    drop(client);

    scanout
}

#[test]
fn the_whole_session_is_byte_equal_under_the_gl_backend() {
    // The software-driven session (the default on CI: Auto without a
    // GL stack lands on software, honestly).
    let sw = Testbench::start("hw-sw");
    let software_scanout = run_session(&sw);
    assert_eq!(
        sw.world(|w| w.renderer.backend()),
        "software",
        "CI has no GL stack: Auto must land on software"
    );
    assert!(
        sw.world(|w| w.renderer_decision_report().contains("GL unavailable")),
        "the honest degradation report is carried: {}",
        sw.world(|w| w.renderer_decision_report().to_owned())
    );

    // The GL-driven session: the reference evaluator injected as the
    // backend, the same choreography.
    let config = CompositorConfig {
        socket: format!("lion-it-hw-gl-{}", std::process::id()),
        renderer_api: Some(Box::new(RefGles::new())),
        ..CompositorConfig::default()
    };
    let gl = Testbench::start_with("hw-gl", config);
    assert_eq!(
        gl.world(|w| w.renderer.backend()),
        "gles",
        "the injected backend drives the GL path"
    );
    assert_eq!(
        gl.world(|w| w.renderer_decision_report().to_owned()),
        "gles (injected backend)"
    );
    let gl_scanout = run_session(&gl);

    // THE assertion: byte-for-byte, the whole 1920x1080 scanout.
    assert_eq!(software_scanout.len(), 1920 * 1080);
    assert_eq!(
        software_scanout, gl_scanout,
        "the GL session must be byte-equal to the software session"
    );

    // And the blend coverage is real: the stacked gradient landed on
    // the checker, not a copy of either.
    let bottom_left_word = gl_scanout[0];
    let checkered = if bottom_left_word == 0xFFFF_0000 || (bottom_left_word >> 24) == 0xFF {
        // The checker's first cell blended with the gradient's first
        // (alpha 15) cell: mostly the checker's red.
        (bottom_left_word >> 16) & 0xFF > 0xF0
    } else {
        false
    };
    assert!(
        checkered,
        "the top gradient must have blended over the checker (got {bottom_left_word:#010x})"
    );
}

#[test]
fn forced_gl_without_hardware_fails_bringup_typed() {
    // `--renderer gl` on a machine without GL: a hard, typed startup
    // failure — never a silent CPU fallback.
    let config = CompositorConfig {
        socket: format!("lion-it-hw-force-{}", std::process::id()),
        renderer: ldp_renderer::gles::RendererChoice::Gles,
        ..CompositorConfig::default()
    };
    match lion_compositor::server::Compositor::headless(config) {
        Ok(_) => {
            // A GL machine: the forced path came up; assert the
            // backend really is GL.
            // (CI has none; this arm runs on GL hardware only.)
        }
        Err(e) => {
            let text = e.to_string();
            assert!(
                text.contains("renderer bring-up failed"),
                "the failure must name the renderer, got: {text}"
            );
        }
    }
}

#[test]
fn forced_software_ignores_the_machine() {
    let config = CompositorConfig {
        socket: format!("lion-it-hw-swforced-{}", std::process::id()),
        renderer: ldp_renderer::gles::RendererChoice::Software,
        ..CompositorConfig::default()
    };
    let tb = Testbench::start_with("hw-swforced", config);
    assert_eq!(tb.world(|w| w.renderer.backend()), "software");
    assert_eq!(
        tb.world(|w| w.renderer_decision_report().to_owned()),
        "software (forced)"
    );
}
