//! The command-stream goldens: exactly what the GL renderer emits for
//! a known frame, and the pinned shader sources.
//!
//! The stream is the renderer's *own* logic — which damage rects get
//! a pass, which layers a pass skips (a layer that misses the damage
//! rect must not draw), the per-pass scissor, and the readback. A
//! regression here is a rendering regression even when pixels still
//! happen to match, because the real GPU executes this exact stream.

#![allow(clippy::missing_panics_doc)]

use ldp_core::buffer::{BufferGeometry, FourCC, Modifier, PlaneLayout};
use ldp_core::color::ColorDescription;
use ldp_core::geometry::{Rect, Region, Transform};
use ldp_renderer::gles::{GlesCmd, GlesRenderer, RecordingGles, FRAGMENT_SRC, VERTEX_SRC};
use ldp_renderer::view::BufferView;
use ldp_renderer::{OutputDesc, Renderer, SurfaceLayer};

fn geometry_of(w: u32, h: u32) -> BufferGeometry {
    let layout = [PlaneLayout {
        offset: 0,
        stride: w * 4,
    }];
    BufferGeometry::new(
        w,
        h,
        FourCC::XRGB8888,
        Modifier::LINEAR,
        &layout,
        u64::from(w * h * 4),
    )
    .unwrap()
}

fn layer(data: &[u8], x: i32, y: i32, w: u32, h: u32) -> SurfaceLayer<'_> {
    let view = BufferView::new(1, data, geometry_of(w, h)).unwrap();
    SurfaceLayer::new(
        view,
        Rect::new(x, y, w, h),
        Transform::Normal,
        ColorDescription::srgb_sdr(),
        1.0,
        Region::new(),
    )
}

/// The recorded stream of one frame: two layers, two damage rects,
/// each layer intersecting exactly one rect — so each pass draws one
/// layer and skips the other.
#[test]
fn two_layers_two_damage_rects_emit_the_exact_stream() {
    // 8x8 output; layer A at (0,0) 4x4; layer B at (4,4) 4x4.
    // Damage: (0,0,4,8) and (4,4,4,4) — A hits rect 1 only, B rect 2.
    let a: Vec<u8> = (0..64u32).map(|i| i as u8).collect();
    let b: Vec<u8> = (64..128u32).map(|i| i as u8).collect();
    let output = OutputDesc::new(8, 8, FourCC::XRGB8888, ColorDescription::srgb_sdr()).unwrap();
    let mut damage = Region::new();
    damage.add(Rect::new(0, 0, 4, 8));
    damage.add(Rect::new(4, 4, 4, 4));

    let (rec, log) = RecordingGles::with_shared_log();
    let mut gl = GlesRenderer::new(Box::new(rec));
    gl.begin_frame(&output, &damage).unwrap();
    gl.clear_damage(0, 0, 0, 0xFF).unwrap();
    gl.submit(&[layer(&a, 0, 0, 4, 4), layer(&b, 4, 4, 4, 4)])
        .unwrap();
    gl.end_frame().unwrap();
    drop(gl);

    let stream: Vec<GlesCmd> = log.lock().map(|l| l.clone()).unwrap_or_default();
    // Handle numbers are dense per kind: program 1, target 1,
    // textures 1..2 (created and destroyed inside their pass).
    assert_eq!(
        stream,
        vec![
            // Program link (lazy, at begin_frame).
            GlesCmd::CreateProgram,
            // Target creation (the persistent output-sized FBO).
            GlesCmd::CreateTarget(1, 8, 8),
            // clear_damage: one pass per damage rect.
            GlesCmd::BeginPass(1, Some(Rect::new(0, 0, 4, 8))),
            GlesCmd::Clear(0, 0, 0, 255),
            GlesCmd::EndPass,
            GlesCmd::BeginPass(1, Some(Rect::new(4, 4, 4, 4))),
            GlesCmd::Clear(0, 0, 0, 255),
            GlesCmd::EndPass,
            // submit: per damage rect, back-to-front layers that hit
            // the rect (each layer misses the other rect).
            GlesCmd::BeginPass(1, Some(Rect::new(0, 0, 4, 8))),
            GlesCmd::CreateTexture(1, 4, 4),
            GlesCmd::DrawLayer(1, Rect::new(0, 0, 4, 4), 255),
            GlesCmd::DestroyTexture(1),
            GlesCmd::EndPass,
            GlesCmd::BeginPass(1, Some(Rect::new(4, 4, 4, 4))),
            GlesCmd::CreateTexture(2, 4, 4),
            GlesCmd::DrawLayer(2, Rect::new(4, 4, 4, 4), 255),
            GlesCmd::DestroyTexture(2),
            GlesCmd::EndPass,
            // end_frame: readback.
            GlesCmd::Readback(1),
            // Renderer drop: program and target teardown.
            GlesCmd::DestroyProgram(1),
            GlesCmd::DestroyTarget(1),
        ]
    );
}

/// A layer disjoint from the damage emits no pass content at all —
/// the skip decision, pinned.
#[test]
fn layers_outside_damage_never_draw() {
    let data: Vec<u8> = vec![0x40; 64];
    let output = OutputDesc::new(8, 8, FourCC::XRGB8888, ColorDescription::srgb_sdr()).unwrap();
    let damage = Region::from_rect(Rect::new(6, 6, 2, 2));

    let (rec, log) = RecordingGles::with_shared_log();
    let mut gl = GlesRenderer::new(Box::new(rec));
    gl.begin_frame(&output, &damage).unwrap();
    gl.clear_damage(0, 0, 0, 0xFF).unwrap();
    gl.submit(&[layer(&data, 0, 0, 4, 4)]).unwrap();
    gl.end_frame().unwrap();
    drop(gl);

    let stream: Vec<GlesCmd> = log.lock().map(|l| l.clone()).unwrap_or_default();
    // One clear pass, one empty draw pass (the layer missed), readback.
    assert_eq!(
        stream,
        vec![
            GlesCmd::CreateProgram,
            GlesCmd::CreateTarget(1, 8, 8),
            GlesCmd::BeginPass(1, Some(Rect::new(6, 6, 2, 2))),
            GlesCmd::Clear(0, 0, 0, 255),
            GlesCmd::EndPass,
            GlesCmd::BeginPass(1, Some(Rect::new(6, 6, 2, 2))),
            GlesCmd::EndPass,
            GlesCmd::Readback(1),
            GlesCmd::DestroyProgram(1),
            GlesCmd::DestroyTarget(1),
        ]
    );
}

/// Damage clipped to the output bounds: off-output damage rects are
/// dropped, not passed to the backend.
#[test]
fn damage_is_clipped_to_the_output_before_passes() {
    let _data: Vec<u8> = vec![0x40; 16];
    let output = OutputDesc::new(4, 4, FourCC::XRGB8888, ColorDescription::srgb_sdr()).unwrap();
    let mut damage = Region::new();
    damage.add(Rect::new(-4, -4, 6, 6)); // clipped to (0,0,2,2)
    damage.add(Rect::new(8, 8, 4, 4)); // fully outside: dropped

    let (rec, log) = RecordingGles::with_shared_log();
    let mut gl = GlesRenderer::new(Box::new(rec));
    gl.begin_frame(&output, &damage).unwrap();
    gl.clear_damage(0, 0, 0, 0xFF).unwrap();
    gl.submit(&[]).unwrap();
    gl.end_frame().unwrap();
    drop(gl);

    let stream: Vec<GlesCmd> = log.lock().map(|l| l.clone()).unwrap_or_default();
    assert_eq!(
        stream,
        vec![
            GlesCmd::CreateProgram,
            GlesCmd::CreateTarget(1, 4, 4),
            GlesCmd::BeginPass(1, Some(Rect::new(0, 0, 2, 2))),
            GlesCmd::Clear(0, 0, 0, 255),
            GlesCmd::EndPass,
            GlesCmd::BeginPass(1, Some(Rect::new(0, 0, 2, 2))),
            GlesCmd::EndPass,
            GlesCmd::Readback(1),
            GlesCmd::DestroyProgram(1),
            GlesCmd::DestroyTarget(1),
        ]
    );
}

#[test]
fn shader_sources_are_pinned() {
    // The vertex shader: unit-quad input, pixel-space rect, top-down
    // contract with the GL Y-flip inside.
    assert!(VERTEX_SRC.contains("uniform vec4 u_rect;"));
    assert!(VERTEX_SRC.contains("gl_Position = vec4(ndc.x, -ndc.y, 0.0, 1.0);"));
    // The fragment shader: premultiplied sample scaled by opacity.
    assert!(FRAGMENT_SRC.contains("gl_FragColor = texture2D(u_tex, v_uv) * u_opacity;"));
    // The composite pass only: one sampler, no extras.
    assert_eq!(VERTEX_SRC.matches("attribute").count(), 2);
    assert!(!FRAGMENT_SRC.contains("texture3D"));
}

/// The DMA-BUF zero-copy stream (Phase 31): a layer carrying an
/// imported EGLImage draws through the image-texture binding — the
/// command stream proves the zero copy: `CreateTextureFromImage`
/// (handle + image), the draw, the destroy — and **no**
/// `CreateTexture` with a CPU payload for that layer (the pixels
/// never cross the CPU; the accounting the comparison's GPU row
/// carries).
#[test]
fn a_dmabuf_layer_binds_the_image_with_zero_cpu_payload() {
    let a: Vec<u8> = (0..64u32).map(|i| i as u8).collect();
    let output = OutputDesc::new(8, 8, FourCC::XRGB8888, ColorDescription::srgb_sdr()).unwrap();
    let mut damage = Region::new();
    damage.add(Rect::new(0, 0, 4, 4));
    let (rec, log) = RecordingGles::with_shared_log();
    let mut renderer = GlesRenderer::new(Box::new(rec));
    renderer.begin_frame(&output, &damage).unwrap();
    // The zero-copy layer: the EGLImage handle 0xABCD (the EGL side's
    // minted pointer) with the same placement geometry.
    let mut dmabuf = layer(&a, 0, 0, 4, 4);
    dmabuf.egl_image = Some(0x00AB_CDEF_1234_5678);
    let stats = renderer.submit(&[dmabuf]).unwrap();
    renderer.end_frame().unwrap();
    assert_eq!(stats.layers, 1);
    let cmds: Vec<GlesCmd> = log.lock().map(|l| l.clone()).unwrap_or_default();
    // The zero-copy binding happened exactly once.
    let image_binds = cmds
        .iter()
        .filter(|c| matches!(c, GlesCmd::CreateTextureFromImage(_, image) if *image == 0x00AB_CDEF_1234_5678))
        .count();
    assert_eq!(image_binds, 1, "the image bound once: {cmds:?}");
    // And NO CPU payload upload for that layer — the zero in
    // zero-copy: the only CreateTexture calls (if any) belong to
    // other layers (none here).
    let cpu_uploads = cmds
        .iter()
        .filter(|c| matches!(c, GlesCmd::CreateTexture(..)))
        .count();
    assert_eq!(cpu_uploads, 0, "no CPU payload ever flows: {cmds:?}");
    // The draw and the destroy rode the bound texture.
    assert!(cmds.iter().any(|c| matches!(c, GlesCmd::DrawLayer(..))));
    assert!(cmds
        .iter()
        .any(|c| matches!(c, GlesCmd::DestroyTexture(..))));
}

/// The mixed stack: a CPU layer and a zero-copy layer in one frame —
/// each rides its own path (the payload upload for the CPU layer, the
/// image binding for the imported one), the stream pins both.
#[test]
fn a_mixed_stack_uploads_and_binds_in_one_frame() {
    let a: Vec<u8> = (0..64u32).map(|i| i as u8).collect();
    let output = OutputDesc::new(8, 8, FourCC::XRGB8888, ColorDescription::srgb_sdr()).unwrap();
    let mut damage = Region::new();
    damage.add(Rect::new(0, 0, 8, 8));
    let (rec, log) = RecordingGles::with_shared_log();
    let mut renderer = GlesRenderer::new(Box::new(rec));
    renderer.begin_frame(&output, &damage).unwrap();
    let cpu_layer = layer(&a, 0, 0, 4, 4);
    let mut dmabuf = layer(&a, 4, 4, 4, 4);
    dmabuf.egl_image = Some(0x1111);
    let stats = renderer.submit(&[cpu_layer, dmabuf]).unwrap();
    renderer.end_frame().unwrap();
    assert_eq!(stats.layers, 2);
    let cmds: Vec<GlesCmd> = log.lock().map(|l| l.clone()).unwrap_or_default();
    assert_eq!(
        cmds.iter()
            .filter(|c| matches!(c, GlesCmd::CreateTexture(..)))
            .count(),
        1,
        "exactly the CPU layer uploaded: {cmds:?}"
    );
    assert_eq!(
        cmds.iter()
            .filter(|c| matches!(c, GlesCmd::CreateTextureFromImage(..)))
            .count(),
        1,
        "exactly the imported layer bound: {cmds:?}"
    );
}

/// The reference evaluator refuses the zero-copy arm honestly (an
/// imported image has no CPU-side words to blend) — the byte-equality
/// oracle's boundary, documented and pinned.
#[test]
fn the_reference_evaluator_refuses_imported_images() {
    let a: Vec<u8> = (0..64u32).map(|i| i as u8).collect();
    let output = OutputDesc::new(8, 8, FourCC::XRGB8888, ColorDescription::srgb_sdr()).unwrap();
    let mut damage = Region::new();
    damage.add(Rect::new(0, 0, 4, 4));
    let mut renderer = GlesRenderer::new(Box::new(ldp_renderer::gles::mock::RefGles::new()));
    renderer.begin_frame(&output, &damage).unwrap();
    let mut dmabuf = layer(&a, 0, 0, 4, 4);
    dmabuf.egl_image = Some(0x2222);
    // The reference evaluator refuses the layer: the byte-equality
    // oracle's honest boundary.
    let result = renderer.submit(&[dmabuf]);
    assert!(result.is_err(), "the reference boundary holds");
}
