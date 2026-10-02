//! The Phase 24 exit criterion: **byte-equality** between the GL path
//! (`GlesRenderer` over the reference GLES evaluator) and the software
//! reference renderer, over randomized corpora — the same
//! cross-implementation oracle discipline the Phase 9 mock EGL
//! established (`ldp-gpu`'s `bit_comparable`).
//!
//! Both frames of every case run the exact compositor choreography
//! (`begin_frame` → `clear_damage` → `submit` → `end_frame`), with
//! partially-offscreen placements, random damage rects, both
//! damage-empty and multi-rect frames, and a *second* frame on the
//! same renderers exercising the damage-clip persistence contract
//! (undamaged pixels must survive identically in both paths).

#![allow(clippy::missing_panics_doc)]

use ldp_core::buffer::{BufferGeometry, FourCC, Modifier, PlaneLayout};
use ldp_core::color::ColorDescription;
use ldp_core::geometry::{Rect, Region, Transform};
use ldp_renderer::gles::{GlesRenderer, RefGles};
use ldp_renderer::view::BufferView;
use ldp_renderer::{OutputDesc, Renderer, SoftwareRenderer, SurfaceLayer};

/// The in-file deterministic rng (SplitMix64, the workspace canon).
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n
    }
}

const W: u32 = 97;
const H: u32 = 61;
const FORMATS: [FourCC; 4] = [
    FourCC::XRGB8888,
    FourCC::ARGB8888,
    FourCC::XBGR8888,
    FourCC::ABGR8888,
];

/// One randomly generated layer: format, extent, placement, pixels.
struct Layer {
    format: FourCC,
    w: u32,
    h: u32,
    x: i32,
    y: i32,
    /// Premultiplied ARGB words (per the buffer contract).
    words: Vec<u32>,
    opacity: f32,
}

fn make_layer(rng: &mut Rng) -> Layer {
    let format = FORMATS[usize::try_from(rng.below(4)).unwrap()];
    let w = u32::try_from(rng.below(24)).unwrap() + 1;
    let h = u32::try_from(rng.below(16)).unwrap() + 1;
    // Placements deliberately hang off the output on some layers.
    let x = i32::try_from(rng.below(u64::from(W + 24))).unwrap() - 12;
    let y = i32::try_from(rng.below(u64::from(H + 24))).unwrap() - 12;
    let mut words = Vec::with_capacity((w * h) as usize);
    for _ in 0..w * h {
        // Premultiplied content: channels bounded by alpha; X-family
        // X bytes are garbage (the sampler's trap).
        let alpha = match rng.below(8) {
            0 => 0u32,   // fully transparent
            1 => 255u32, // fully opaque
            _ => u32::try_from(rng.below(256)).unwrap(),
        };
        let chan = |rng: &mut Rng, bound: u32| match bound {
            0 => 0u32,
            _ => u32::try_from(rng.below(u64::from(bound) + 1)).unwrap(),
        };
        let (red, green, blue) = (chan(rng, alpha), chan(rng, alpha), chan(rng, alpha));
        let alpha_slot = if matches!(format, FourCC::XRGB8888 | FourCC::XBGR8888) {
            u32::try_from(rng.below(256)).unwrap() // garbage by definition
        } else {
            alpha
        };
        let word = if matches!(format, FourCC::XRGB8888 | FourCC::ARGB8888) {
            alpha_slot << 24 | red << 16 | green << 8 | blue
        } else {
            alpha_slot << 24 | blue << 16 | green << 8 | red
        };
        words.push(word);
    }
    let opacity = match rng.below(4) {
        0 => 1.0,
        1 => 0.5,
        _ => f32::from(u32::try_from(rng.below(256)).unwrap() as u16) / 255.0,
    };
    Layer {
        format,
        w,
        h,
        x,
        y,
        words,
        opacity,
    }
}

fn geometry_of(format: FourCC, w: u32, h: u32) -> BufferGeometry {
    let layout = [PlaneLayout {
        offset: 0,
        stride: w * 4,
    }];
    BufferGeometry::new(
        w,
        h,
        format,
        Modifier::LINEAR,
        &layout,
        u64::from(w * h * 4),
    )
    .unwrap()
}

/// Build the `SurfaceLayer` list over a shared backing store.
fn build_layers<'a>(layers: &[Layer], backing: &'a [Vec<u8>]) -> Vec<SurfaceLayer<'a>> {
    layers
        .iter()
        .zip(backing)
        .map(|(l, data)| {
            let view = BufferView::new(1, data, geometry_of(l.format, l.w, l.h)).unwrap();
            SurfaceLayer::new(
                view,
                Rect::new(l.x, l.y, l.w, l.h),
                Transform::Normal,
                ColorDescription::srgb_sdr(),
                l.opacity,
                Region::new(),
            )
        })
        .collect()
}

/// Run one full frame through any renderer (the compositor's exact
/// choreography).
fn run_frame<R: Renderer>(
    r: &mut R,
    output: &OutputDesc,
    damage: &Region,
    layers: &[SurfaceLayer<'_>],
) {
    r.begin_frame(output, damage).unwrap();
    r.clear_damage(0, 0, 0, 0xFF).unwrap();
    r.submit(layers).unwrap();
    r.end_frame().unwrap();
}

#[test]
fn gl_path_is_byte_equal_to_software_over_a_randomized_corpus() {
    for seed in 0..12u64 {
        let mut rng = Rng::new(0x1D0_0240_CAFE_0000 + seed);
        let output = OutputDesc::new(W, H, FourCC::XRGB8888, ColorDescription::srgb_sdr()).unwrap();

        // Layer set + backing stores for this seed.
        let count = usize::try_from(rng.below(5)).unwrap() + 1;
        let layers: Vec<Layer> = (0..count).map(|_| make_layer(&mut rng)).collect();
        let backing: Vec<Vec<u8>> = layers
            .iter()
            .map(|l| l.words.iter().flat_map(|w| w.to_le_bytes()).collect())
            .collect();

        let mut sw = SoftwareRenderer::new();
        let mut gl = GlesRenderer::new(Box::new(RefGles::new()));

        // Frame 1: full or partial damage.
        let damage = match rng.below(3) {
            0 => Region::from_rect(Rect::new(0, 0, W, H)),
            1 => Region::new(),
            _ => {
                let mut region = Region::new();
                for _ in 0..=rng.below(3) {
                    let x = i32::try_from(rng.below(u64::from(W))).unwrap();
                    let y = i32::try_from(rng.below(u64::from(H))).unwrap();
                    let w = u32::try_from(rng.below(u64::from(W - x as u32) + 1)).unwrap();
                    let h = u32::try_from(rng.below(u64::from(H - y as u32) + 1)).unwrap();
                    region.add(Rect::new(x, y, w.max(1), h.max(1)));
                }
                region
            }
        };
        {
            let built = build_layers(&layers, &backing);
            run_frame(&mut sw, &output, &damage, &built);
            run_frame(&mut gl, &output, &damage, &built);
        }
        assert_eq!(
            gl.readout(),
            sw.readout(),
            "seed {seed}: frame 1 (damage {} rects) diverged",
            damage.len()
        );

        // Frame 2 on the SAME renderers: a fresh random damage subset —
        // the persistence contract (undamaged pixels survive both
        // paths identically).
        let x = i32::try_from(rng.below(u64::from(W / 2))).unwrap();
        let y = i32::try_from(rng.below(u64::from(H / 2))).unwrap();
        let damage2 = Region::from_rect(Rect::new(x, y, W / 3, H / 3));
        {
            let built = build_layers(&layers, &backing);
            run_frame(&mut sw, &output, &damage2, &built);
            run_frame(&mut gl, &output, &damage2, &built);
        }
        assert_eq!(
            gl.readout(),
            sw.readout(),
            "seed {seed}: frame 2 (partial damage, persistence) diverged"
        );
    }
}

#[test]
fn target_resizes_recreate_content_honestly() {
    // A size change reallocates both paths' storage; both must agree
    // again after the next full-frame render.
    let output1 = OutputDesc::new(8, 8, FourCC::XRGB8888, ColorDescription::srgb_sdr()).unwrap();
    let output2 = OutputDesc::new(12, 5, FourCC::XRGB8888, ColorDescription::srgb_sdr()).unwrap();
    let mut sw = SoftwareRenderer::new();
    let mut gl = GlesRenderer::new(Box::new(RefGles::new()));
    for output in [&output1, &output2] {
        let full = Region::from_rect(Rect::new(0, 0, output.width, output.height));
        run_frame(&mut sw, output, &full, &[]);
        run_frame(&mut gl, output, &full, &[]);
        assert_eq!(gl.readout(), sw.readout());
    }
}

#[test]
fn rotated_layers_still_fail_typed() {
    // A rotated layer is outside the GL v1 transform subset (the one
    // boundary Phase 33 keeps: transforms ride the CPU path).
    let geom = geometry_of(FourCC::XRGB8888, 4, 4);
    let data = vec![0x40u8; 64];
    let view = BufferView::new(1, &data, geom).unwrap();
    let layer = SurfaceLayer::new(
        view,
        Rect::new(0, 0, 4, 4),
        Transform::Rot90,
        ColorDescription::srgb_sdr(),
        1.0,
        Region::new(),
    );
    let mut gl = GlesRenderer::new(Box::new(RefGles::new()));
    let output = OutputDesc::new(8, 8, FourCC::XRGB8888, ColorDescription::srgb_sdr()).unwrap();
    gl.begin_frame(&output, &Region::from_rect(Rect::new(0, 0, 8, 8)))
        .unwrap();
    let err = gl.submit(&[layer]).unwrap_err();
    assert!(matches!(
        err,
        ldp_renderer::RendererError::UnsupportedLayer { .. }
    ));
}

#[test]
fn yuv_and_scaled_layers_composite_byte_equal_to_software() {
    // Phase 33: the GL path composites what it used to refuse. An
    // NV12 layer (the general decode arm — the software path's own
    // sampler feeding the upload) and a half-resolution XRGB layer
    // scaled 2x across the output (the continuous-nearest rule) must
    // both land byte-equal to the software renderer.
    let mut sw = SoftwareRenderer::new();
    let mut gl = GlesRenderer::new(Box::new(RefGles::new()));
    let output = OutputDesc::new(16, 16, FourCC::XRGB8888, ColorDescription::srgb_sdr()).unwrap();
    let full = Region::from_rect(Rect::new(0, 0, 16, 16));

    // The NV12 layer: a filled luma plane and a neutral chroma plane
    // (gray) — the decode must produce the same gray both paths.
    let layout = [
        PlaneLayout {
            offset: 0,
            stride: 16,
        },
        PlaneLayout {
            offset: 256,
            stride: 16,
        },
    ];
    let geom = BufferGeometry::new(16, 16, FourCC::NV12, Modifier::LINEAR, &layout, 512).unwrap();
    let mut data = vec![0u8; geom.spanned_bytes() as usize];
    for row in 0..16 {
        for col in 0..16 {
            data[row * 16 + col] = 128 + ((row * 7 + col * 13) % 100) as u8;
        }
    }
    for byte in &mut data[256..] {
        *byte = 128; // neutral chroma
    }
    let nv12_view = BufferView::new(1, &data, geom).unwrap();

    // The scaled layer: 8x8 source stretched to the 16x16 output.
    let small_geom = geometry_of(FourCC::XRGB8888, 8, 8);
    let mut small = vec![0u8; 8 * 8 * 4];
    for (i, px) in small.chunks_exact_mut(4).enumerate() {
        let v = (i * 11 % 256) as u8;
        px.copy_from_slice(&[v, v, v, 0]);
    }
    let small_view = BufferView::new(2, &small, small_geom).unwrap();

    let layers = vec![
        SurfaceLayer::new(
            nv12_view,
            Rect::new(0, 0, 16, 16),
            Transform::Normal,
            ColorDescription::srgb_sdr(),
            1.0,
            Region::new(),
        ),
        SurfaceLayer::new(
            small_view,
            Rect::new(0, 0, 16, 16),
            Transform::Normal,
            ColorDescription::srgb_sdr(),
            0.75,
            Region::new(),
        ),
    ];
    run_frame(&mut sw, &output, &full, &layers);
    run_frame(&mut gl, &output, &full, &layers);
    assert_eq!(gl.readout(), sw.readout());
}
