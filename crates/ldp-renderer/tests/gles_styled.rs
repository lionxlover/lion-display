//! The Phase 27 exit criterion: **byte-equality** between the GL path
//! (`GlesRenderer` over the reference GLES evaluator) and the software
//! reference renderer over randomized **styled** corpora — rounded
//! corners, shadows, and frosted backdrops together, with partially
//! offscreen placements, random damage (full, empty, single-rect and
//! multi-rect), and second frames exercising the damage persistence
//! contract.
//!
//! The equivalence is genuine, not tautological: the two paths share
//! only the *effect kernels* (`effects.rs`) — the coverage folds, the
//! material preparation, and the blend all run through different code
//! (the software span walkers versus the GL command stream over
//! `RefGles`'s own evaluator), so agreement pins the choreography: the
//! upload-time coverage fold against the blend-time fold, the
//! layer-major stream against the sequential passes, the readback
//! frost against the saved-region frost.
//!
//! Phase 40 grew the corpus with the material depth: the saturation
//! dial now spans the **vibrant domain** (`0..=510` — boosts included,
//! the desaturating half kept) and the **edge light** joins the style
//! vocabulary, so the hairline's post-ink stroke is proven on both
//! paths (the ring drawn by `apply_material` against the ring drawn
//! as the stream's after-ink textured quad).

#![allow(clippy::missing_panics_doc)]

use ldp_core::buffer::{BufferGeometry, FourCC, Modifier, PlaneLayout};
use ldp_core::color::ColorDescription;
use ldp_core::geometry::{Rect, Region, Transform};
use ldp_renderer::gles::{GlesRenderer, RefGles};
use ldp_renderer::view::BufferView;
use ldp_renderer::{
    BackdropParams, EdgeLightParams, LayerStyle, OutputDesc, Renderer, ShadowParams,
    SoftwareRenderer, SurfaceLayer,
};

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

const W: u32 = 89;
const H: u32 = 57;

/// One randomly generated styled layer.
struct Layer {
    format: FourCC,
    w: u32,
    h: u32,
    x: i32,
    y: i32,
    words: Vec<u32>,
    opacity: f32,
    style: LayerStyle,
}

/// One randomly generated styled layer — every dimension of the style
/// vocabulary drawn in one place (the corpus's whole grammar; the
/// body is the fixture, not logic).
#[allow(clippy::too_many_lines)]
fn make_layer(rng: &mut Rng) -> Layer {
    let format = match rng.below(2) {
        0 => FourCC::XRGB8888,
        _ => FourCC::ARGB8888,
    };
    let w = u32::try_from(rng.below(28)).unwrap() + 4;
    let h = u32::try_from(rng.below(20)).unwrap() + 4;
    // Placements deliberately hang off the output on some layers —
    // the shadow box and the frost snapshot both face the boundary.
    let x = i32::try_from(rng.below(u64::from(W + 24))).unwrap() - 12;
    let y = i32::try_from(rng.below(u64::from(H + 24))).unwrap() - 12;
    let mut words = Vec::with_capacity((w * h) as usize);
    for py in 0..h {
        for px in 0..w {
            // Structured content: translucent gradients the frost can
            // meaningfully blur, opaque bodies the shadow can frame.
            let alpha = if rng.below(3) == 0 {
                u32::try_from(40 + rng.below(160)).unwrap()
            } else {
                255
            };
            let red = u32::try_from(rng.below(u64::from(alpha) + 1)).unwrap();
            let green = u32::try_from(rng.below(u64::from(alpha) + 1)).unwrap();
            let blue = u32::try_from(rng.below(u64::from(alpha) + 1)).unwrap();
            let alpha_slot = if format == FourCC::XRGB8888 {
                u32::try_from(rng.below(256)).unwrap() // garbage by definition
            } else {
                alpha
            };
            let _ = (px, py);
            words.push(alpha_slot << 24 | red << 16 | green << 8 | blue);
        }
    }
    let opacity = match rng.below(4) {
        0 => 1.0,
        1 => 0.6,
        _ => f32::from(u32::try_from(rng.below(256)).unwrap() as u16) / 255.0,
    };
    // The style: corners usually on, shadow usually on, frost on the
    // translucent-content layers (the macOS doctrine).
    let corner_radius = match rng.below(4) {
        0 => 0,
        1 => 2,
        2 => 6,
        _ => 12,
    };
    let shadow = (rng.below(4) != 0).then(|| ShadowParams {
        radius: match rng.below(3) {
            0 => 0,
            1 => 4,
            _ => 10,
        },
        blur: match rng.below(4) {
            0 => 0,
            1 => 2,
            2 => 4,
            _ => 7,
        },
        passes: u32::try_from(rng.below(4)).unwrap(),
        color: [
            u32::try_from(rng.below(64)).unwrap() as u8,
            u32::try_from(rng.below(64)).unwrap() as u8,
            u32::try_from(rng.below(64)).unwrap() as u8,
        ],
        alpha: u32::try_from(40 + rng.below(180)).unwrap() as u8,
        offset: (
            i32::try_from(rng.below(9)).unwrap() - 4,
            i32::try_from(rng.below(9)).unwrap() - 4,
        ),
    });
    let backdrop = (rng.below(2) == 0).then(|| BackdropParams {
        blur: match rng.below(4) {
            0 => 0,
            1 => 2,
            2 => 5,
            _ => 8,
        },
        passes: u32::try_from(rng.below(4)).unwrap(),
        // Phase 40: the full vibrant domain — the desaturating half
        // (0..=255) and the boosts above it (256..=510) both appear,
        // so the corpus pins whichever side of identity a frame lands
        // on (the boost arm and the desat arm are different code).
        saturation: u16::try_from(rng.below(511)).unwrap(),
        tint: [
            u32::try_from(rng.below(256)).unwrap() as u8,
            u32::try_from(rng.below(256)).unwrap() as u8,
            u32::try_from(rng.below(256)).unwrap() as u8,
        ],
        tint_alpha: u32::try_from(rng.below(200)).unwrap() as u8,
    });
    // Phase 40: the hairline joins the vocabulary — half the styled
    // layers trace one (the stroke color and alpha free).
    let edge_light = (rng.below(2) == 0).then(|| EdgeLightParams {
        color: [
            u32::try_from(rng.below(256)).unwrap() as u8,
            u32::try_from(rng.below(256)).unwrap() as u8,
            u32::try_from(rng.below(256)).unwrap() as u8,
        ],
        alpha: u32::try_from(rng.below(256)).unwrap() as u8,
    });
    Layer {
        format,
        w,
        h,
        x,
        y,
        words,
        opacity,
        style: LayerStyle {
            corner_radius,
            shadow,
            backdrop,
            edge_light,
        },
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

/// Build the styled `SurfaceLayer` list over a shared backing store.
fn build_layers<'a>(layers: &[Layer], backing: &'a [Vec<u8>]) -> Vec<SurfaceLayer<'a>> {
    layers
        .iter()
        .zip(backing)
        .map(|(l, data)| {
            let view = BufferView::new(1, data, geometry_of(l.format, l.w, l.h)).unwrap();
            let mut surface = SurfaceLayer::new(
                view,
                Rect::new(l.x, l.y, l.w, l.h),
                Transform::Normal,
                ColorDescription::srgb_sdr(),
                l.opacity,
                Region::new(),
            );
            surface.style = l.style;
            surface
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
fn styled_gl_path_is_byte_equal_to_software_over_a_randomized_corpus() {
    for seed in 0..16u64 {
        let mut rng = Rng::new(0x1D0_0271_E000_0000 + seed);
        let output = OutputDesc::new(W, H, FourCC::XRGB8888, ColorDescription::srgb_sdr()).unwrap();

        let count = usize::try_from(rng.below(4)).unwrap() + 2;
        let layers: Vec<Layer> = (0..count).map(|_| make_layer(&mut rng)).collect();
        let backing: Vec<Vec<u8>> = layers
            .iter()
            .map(|l| l.words.iter().flat_map(|w| w.to_le_bytes()).collect())
            .collect();

        let mut sw = SoftwareRenderer::new();
        let mut gl = GlesRenderer::new(Box::new(RefGles::new()));

        // Frame 1: full, single-rect, or multi-rect damage (the
        // layer-major stream makes every shape exact).
        let damage = match rng.below(4) {
            0 => Region::from_rect(Rect::new(0, 0, W, H)),
            1 => Region::new(),
            2 => {
                let x = i32::try_from(rng.below(u64::from(W / 2))).unwrap();
                let y = i32::try_from(rng.below(u64::from(H / 2))).unwrap();
                Region::from_rect(Rect::new(x, y, W / 2, H / 2))
            }
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
            "seed {seed}: styled frame 1 (damage {} rects) diverged",
            damage.len()
        );

        // Frame 2 on the SAME renderers: partial damage — the
        // persistence contract under styles (the frost of frame two
        // samples frame one's result beneath the layer, on both paths).
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
            "seed {seed}: styled frame 2 (partial damage, persistence) diverged"
        );
    }
}

/// The plain stream survives untouched: unstyled layers through the
/// styled-aware renderer still match byte-for-byte (the Phase 24
/// corpus's smallest case, restated as a regression gate).
#[test]
fn plain_layers_still_match_after_the_phase_27_growth() {
    let output = OutputDesc::new(8, 6, FourCC::XRGB8888, ColorDescription::srgb_sdr()).unwrap();
    let words: Vec<u32> = vec![0x0044_3322, 0x0088_7766, 0x00CC_BBAA, 0x0055_4433];
    let data: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let geom = geometry_of(FourCC::XRGB8888, 2, 2);
    let view = BufferView::new(1, &data, geom).unwrap();
    let layer = SurfaceLayer::new(
        view,
        Rect::new(1, 1, 2, 2),
        Transform::Normal,
        ColorDescription::srgb_sdr(),
        1.0,
        Region::new(),
    );
    let damage = Region::from_rect(Rect::new(0, 0, 8, 6));
    let mut sw = SoftwareRenderer::new();
    let mut gl = GlesRenderer::new(Box::new(RefGles::new()));
    run_frame(&mut sw, &output, &damage, std::slice::from_ref(&layer));
    run_frame(&mut gl, &output, &damage, &[layer]);
    assert_eq!(gl.readout(), sw.readout());
}
