//! Phase 9 exit criteria — the bit-comparable harness.
//!
//! The same layer list composites through two paths:
//!
//! * **Reference** — `ldp-renderer`'s `SoftwareRenderer`, the
//!   reference compositor from Phase 8.
//! * **EGL path** — buffers wrapped as DMA-BUF descriptors, imported
//!   through the mock EGL context (whose import *decodes the real
//!   attribute encoding*, so the codec round trip runs on every
//!   import), a fenced draw gated on a mock sync driver, and a
//!   readback.
//!
//! The two paths agree **byte-for-byte** on the representable subset:
//! 1:1 placements, identity transforms, ARGB8888/XRGB8888 layers over
//! an ARGB8888/XRGB8888 target, uniform per-layer opacity — because
//! the mock's blend is the reference blend (premultiplied source-over
//! with the shared `mul255` rounding), reached through a genuinely
//! different plumbing path. That makes this a cross-implementation
//! oracle: any channel swap, stride loss, offset drift, or format
//! confusion anywhere in the import path breaks the equality.
//!
//! The fenced draw also proves the sync interlock: the draw refuses
//! until the driver's clock passes the acquire fence's ready time.

#![allow(clippy::too_many_lines)]

use std::rc::Rc;

use ldp_core::buffer::{FourCC, Modifier};
use ldp_core::color::ColorDescription;
use ldp_core::geometry::{Rect, Region};
use ldp_core::time::Mono;
use ldp_gpu::dmabuf::{DmaBufDescriptor, DmaBufPlane};
use ldp_gpu::egl::{EglApi, ImageHandle, MockEgl};
use ldp_gpu::sync::{Fence, MockSyncDriver, SyncDriver};
use ldp_renderer::testkit::{geometry_for, pattern_buffer, pattern_rgba};
use ldp_renderer::{OutputDesc, Renderer, SoftwareRenderer, SurfaceLayer};

/// One composite case: layers with placement and opacity, plus the
/// output format.
struct Case {
    width: u32,
    height: u32,
    output: FourCC,
    layers: Vec<LayerSpec>,
}

struct LayerSpec {
    width: u32,
    height: u32,
    format: FourCC,
    dx: i32,
    dy: i32,
    opacity: f32,
}

impl Case {
    fn reference(&self) -> Vec<u8> {
        let mut renderer = SoftwareRenderer::new();
        let output = OutputDesc::new(
            self.width,
            self.height,
            self.output,
            ColorDescription::default(),
        )
        .expect("output");
        let damage = Region::from_rect(Rect::new(0, 0, self.width, self.height));
        renderer.begin_frame(&output, &damage).expect("begin");
        // Keep the buffers alive alongside the layers that borrow them.
        let buffers: Vec<(Vec<u8>, ldp_core::buffer::BufferGeometry)> = self
            .layers
            .iter()
            .map(|spec| pattern_buffer(spec.width, spec.height, spec.format))
            .collect();
        let layers: Vec<SurfaceLayer<'_>> = buffers
            .iter()
            .map(|(data, geometry)| {
                let view = ldp_renderer::BufferView::new(0, data, geometry.clone()).expect("view");
                // Find this layer's placement back by index.
                let index = buffers
                    .iter()
                    .position(|(d, g)| d.as_ptr() == data.as_ptr() && g.width() == geometry.width())
                    .expect("layer index");
                let spec = &self.layers[index];
                SurfaceLayer::new(
                    view,
                    Rect::new(spec.dx, spec.dy, spec.width, spec.height),
                    ldp_core::geometry::Transform::Normal,
                    ColorDescription::default(),
                    spec.opacity,
                    Region::default(),
                )
            })
            .collect();
        renderer.submit(&layers).expect("submit");
        renderer.end_frame().expect("end");
        // The renderer stores premultiplied output words; pack to the
        // output format's byte layout for comparison.
        renderer
            .readout()
            .iter()
            .flat_map(|w| match self.output {
                FourCC::ARGB8888 => w.to_ne_bytes().to_vec(),
                // XRGB reads back with the reserved bits set.
                _ => (w | 0xFF00_0000).to_ne_bytes().to_vec(),
            })
            .collect()
    }

    fn egl_path(&self) -> Vec<u8> {
        let mut egl = MockEgl::capable();
        egl.bootstrap().expect("bootstrap");
        let mut driver = MockSyncDriver::new(Mono::ZERO);
        // The acquire fence matures at 1 ms; the draw must refuse
        // before that and succeed after.
        driver.schedule_token(1, Mono::from_ns(1_000_000));
        let acquire = Fence::Mock(1);

        let mut handles: Vec<ImageHandle> = Vec::new();
        for (fd, spec) in self.layers.iter().enumerate() {
            let (data, geometry) = pattern_buffer(spec.width, spec.height, spec.format);
            let stride = geometry.planes()[0].stride;
            let descriptor = DmaBufDescriptor::new(
                spec.width,
                spec.height,
                spec.format,
                Modifier::LINEAR,
                &[DmaBufPlane {
                    fd: fd as i32,
                    offset: 0,
                    stride,
                }],
            )
            .expect("descriptor");
            egl.register(fd as i32, geometry, Rc::new(data));
            handles.push(egl.import_image(&descriptor).expect("import"));
        }

        let layers: Vec<(ImageHandle, i32, i32, u8)> = self
            .layers
            .iter()
            .zip(&handles)
            .map(|(spec, handle)| (*handle, spec.dx, spec.dy, opacity_u8(spec.opacity)))
            .collect();

        // The fenced draw refuses while the acquire fence pends.
        let refused = egl.draw_and_readback(
            self.width,
            self.height,
            self.output,
            &layers,
            &acquire,
            &driver,
        );
        assert!(refused.is_err(), "the draw must refuse on a pending fence");

        driver.advance_ns(1_000_000);
        assert!(driver.state(&acquire).unwrap().is_signaled());
        egl.draw_and_readback(
            self.width,
            self.height,
            self.output,
            &layers,
            &acquire,
            &driver,
        )
        .expect("fenced draw")
    }
}

/// Quantize opacity exactly as the renderer's `opacity_q` does
/// (round-half-up to the u8 domain).
fn opacity_u8(opacity: f32) -> u8 {
    let q = (opacity.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
    u8::try_from(q.min(255)).unwrap_or(255)
}

#[test]
fn single_opaque_layer_is_bit_comparable() {
    let case = Case {
        width: 37,
        height: 23,
        output: FourCC::ARGB8888,
        layers: vec![LayerSpec {
            width: 37,
            height: 23,
            format: FourCC::ARGB8888,
            dx: 0,
            dy: 0,
            opacity: 1.0,
        }],
    };
    assert_eq!(
        case.reference(),
        case.egl_path(),
        "byte-equality on the copy path"
    );
}

#[test]
fn single_xrgb_layer_over_argb_target_is_bit_comparable() {
    // Opaque XRGB source: the renderer's copy path over an ARGB
    // target — channel order and stride fidelity included.
    let case = Case {
        width: 24,
        height: 16,
        output: FourCC::ARGB8888,
        layers: vec![LayerSpec {
            width: 24,
            height: 16,
            format: FourCC::XRGB8888,
            dx: 0,
            dy: 0,
            opacity: 1.0,
        }],
    };
    assert_eq!(case.reference(), case.egl_path());
}

// r/g/b/a/x/y are the pixel domain's own names throughout.
#[allow(clippy::many_single_char_names)]
#[test]
fn partial_placement_is_bit_comparable() {
    // A layer smaller than the target: the uncovered region stays
    // transparent, the covered region carries the pattern.
    let case = Case {
        width: 40,
        height: 30,
        output: FourCC::ARGB8888,
        layers: vec![LayerSpec {
            width: 17,
            height: 11,
            format: FourCC::ARGB8888,
            dx: 5,
            dy: 7,
            opacity: 1.0,
        }],
    };
    let reference = case.reference();
    let egl = case.egl_path();
    assert_eq!(reference, egl);
    // And the bytes really are the pattern where the layer sits:
    // premultiplied source over the opaque-black fresh framebuffer.
    let (x, y) = (6, 8);
    let [r, g, b, a] = pattern_rgba(x - 5, y - 7);
    let mul255 = |v: u8, f: u8| (u32::from(v) * u32::from(f) + 127) / 255;
    let i = (y * 40 + x) as usize * 4;
    let word = u32::from_ne_bytes([egl[i], egl[i + 1], egl[i + 2], egl[i + 3]]);
    let expect = 0xFF00_0000 | mul255(r, a) << 16 | mul255(g, a) << 8 | mul255(b, a);
    assert_eq!(word, expect);
}

#[test]
fn two_layer_stack_with_opacity_is_bit_comparable() {
    // The blended case: a half-opaque ARGB layer over an opaque XRGB
    // base — premultiplied source-over with the shared rounding, on
    // both paths, byte for byte.
    let case = Case {
        width: 32,
        height: 20,
        output: FourCC::ARGB8888,
        layers: vec![
            LayerSpec {
                width: 32,
                height: 20,
                format: FourCC::XRGB8888,
                dx: 0,
                dy: 0,
                opacity: 1.0,
            },
            LayerSpec {
                width: 13,
                height: 9,
                format: FourCC::ARGB8888,
                dx: 3,
                dy: 4,
                opacity: 0.5,
            },
        ],
    };
    assert_eq!(case.reference(), case.egl_path());
}

#[test]
fn three_layer_partial_opacity_stack_is_bit_comparable() {
    // A deeper stack with three opacities, none 0 or 1.
    let case = Case {
        width: 26,
        height: 18,
        output: FourCC::ARGB8888,
        layers: vec![
            LayerSpec {
                width: 26,
                height: 18,
                format: FourCC::XRGB8888,
                dx: 0,
                dy: 0,
                opacity: 1.0,
            },
            LayerSpec {
                width: 20,
                height: 14,
                format: FourCC::ARGB8888,
                dx: 2,
                dy: 2,
                opacity: 0.75,
            },
            LayerSpec {
                width: 9,
                height: 7,
                format: FourCC::ARGB8888,
                dx: 8,
                dy: 5,
                opacity: 0.33,
            },
        ],
    };
    assert_eq!(case.reference(), case.egl_path());
}

#[test]
fn xrgb_output_target_is_bit_comparable() {
    let case = Case {
        width: 19,
        height: 13,
        output: FourCC::XRGB8888,
        layers: vec![LayerSpec {
            width: 19,
            height: 13,
            format: FourCC::ARGB8888,
            dx: 0,
            dy: 0,
            opacity: 0.6,
        }],
    };
    assert_eq!(case.reference(), case.egl_path());
}

// r/g/b/a/x/y are the pixel domain's own names throughout.
#[allow(clippy::many_single_char_names)]
#[test]
fn descriptor_geometry_is_pinned_by_the_import() {
    // The import path must use the descriptor's OWN stride and
    // offset, not the buffer's default geometry: a buffer with
    // padding rows still reads correctly.
    let mut egl = MockEgl::capable();
    egl.bootstrap().unwrap();
    let (data, geometry) = pattern_buffer(8, 8, FourCC::ARGB8888);
    // Widen the stride by one row of padding.
    let padded_stride = geometry.planes()[0].stride * 2;
    let mut padded = vec![0u8; padded_stride as usize * 8];
    for row in 0..8 {
        let src = geometry.planes()[0].offset as usize + geometry.planes()[0].stride as usize * row;
        let dst = padded_stride as usize * row;
        padded[dst..dst + geometry.planes()[0].stride as usize]
            .copy_from_slice(&data[src..src + geometry.planes()[0].stride as usize]);
    }
    let descriptor = DmaBufDescriptor::new(
        8,
        8,
        FourCC::ARGB8888,
        Modifier::LINEAR,
        &[DmaBufPlane {
            fd: 0,
            offset: 0,
            stride: padded_stride,
        }],
    )
    .expect("descriptor");
    // Register with a default geometry — the import's stride walk
    // addresses the buffer through the descriptor's plane offset and
    // stride, so the padding rows are skipped by construction.
    let padded_geometry = geometry_for(8, 8, FourCC::ARGB8888);
    egl.register(0, padded_geometry, Rc::new(padded));
    let image = egl.import_image(&descriptor).expect("import");

    let driver = MockSyncDriver::new(Mono::ZERO);
    let layers = [(image, 0, 0, 255)];
    let out = egl
        .draw_and_readback(
            8,
            8,
            FourCC::ARGB8888,
            &layers,
            &Fence::merge(Vec::new()),
            &driver,
        )
        .expect("draw");
    // Every pixel is the pattern, read through the padded stride:
    // premultiplied over the opaque-black fresh framebuffer.
    let mul255 = |v: u8, f: u8| (u32::from(v) * u32::from(f) + 127) / 255;
    for y in 0..8u32 {
        for x in 0..8u32 {
            let [r, g, b, a] = pattern_rgba(x, y);
            let i = (y * 8 + x) as usize * 4;
            let word = u32::from_ne_bytes([out[i], out[i + 1], out[i + 2], out[i + 3]]);
            let expect = 0xFF00_0000 | mul255(r, a) << 16 | mul255(g, a) << 8 | mul255(b, a);
            assert_eq!(word, expect, "pixel ({x},{y}) through the padded stride");
        }
    }
}
