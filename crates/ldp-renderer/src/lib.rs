//! LDP rendering backends.
//!
//! This crate holds the [`Renderer`] contract — the backend-agnostic frame
//! protocol of `docs/architecture.md` §9.1 — and the reference software
//! implementation, a damage-clipped scanline compositor:
//!
//! * **Format-complete sampling** of every v1 fourcc (7 RGB + 4 YUV, full
//!   and studio ranges, P010 10-bit),
//! * **All eight buffer transforms** with exact integer 1:1 mappings and a
//!   continuous nearest-sample scaled mapping,
//! * **Color pipeline hooks** ([`ColorPipeline`]): range → transfer →
//!   primaries → reference-white anchor, shared math every backend will
//!   reuse; ldp-color (Phase 14) layers tone-mapping policy on top,
//! * **Golden-image conformance**: the software renderer *is* the pixel
//!   oracle — `tests/golden_*.rs` pin reference frames byte-exactly.
//!
//! Determinism doctrine (same as the scheduler's): the renderer never reads
//! a clock, never iterates a hashmap on the pixel path, and every rounding
//! rule is named — integer paths are bit-stable everywhere, float paths use
//! exactly-specified IEEE ops (only transfer curves touch libm `powf`, and
//! those are tolerance-anchored in the golden suite).
//!
//! Blending policy, locked this phase:
//!
//! * same-description layers composite in the **target's encoded space**
//!   (integer premultiplied `over`, pixman/GL-fast-path equivalence), and
//! * cross-description layers composite in **scene-linear** (decode,
//!   convert, blend, re-encode).
//!
//! # Example
//!
//! ```
//! use ldp_core::buffer::{BufferGeometry, FourCC, Modifier, PlaneLayout};
//! use ldp_core::color::ColorDescription;
//! use ldp_core::geometry::{Rect, Region, Transform};
//! use ldp_renderer::{BufferView, OutputDesc, Renderer, SoftwareRenderer, SurfaceLayer};
//!
//! // 1x1 opaque red XRGB buffer.
//! let data = 0x00_FF_00_00u32.to_le_bytes(); // fields X,R,G,B
//! let layout = [PlaneLayout { offset: 0, stride: 4 }];
//! let geometry = BufferGeometry::new(1, 1, FourCC::XRGB8888, Modifier::LINEAR, &layout, 4).unwrap();
//! let view = BufferView::new(1, &data, geometry).unwrap();
//! let output = OutputDesc::new(1, 1, FourCC::XRGB8888, ColorDescription::srgb_sdr()).unwrap();
//!
//! let mut renderer = SoftwareRenderer::new();
//! renderer.begin_frame(&output, &Region::from_rect(Rect::new(0, 0, 1, 1))).unwrap();
//! let layer = SurfaceLayer::new(
//!     view, Rect::new(0, 0, 1, 1), Transform::Normal,
//!     ColorDescription::srgb_sdr(), 1.0, Region::new(),
//! );
//! let stats = renderer.submit(&[layer]).unwrap();
//! assert_eq!(stats.pixels_opaque, 1);
//! renderer.end_frame().unwrap();
//! assert_eq!(renderer.readout(), &[0xFF_FF_00_00]); // premul ARGB word
//! ```

#![forbid(unsafe_code)]

mod composite;
mod effects;
mod errors;
pub mod gles;
mod layer;
mod mapping;
mod pipeline;
mod sample;
mod software;
mod spans;
mod style;
mod transfer;
pub mod view;
mod yuv;

pub mod testkit;

pub use composite::RenderStats;
pub use effects::{
    blur_words, edge_light_material, frost_material, over_premul, pack_canonical, rounded_coverage,
    scale_premul, shadow_material, unpack_canonical, BlurEdge, EdgeMemo, FrostMemo, MaterialCache,
    EDGE_MEMO_ENTRIES, FROST_MEMO_BUDGET_WORDS, FROST_MEMO_ENTRIES, MATERIAL_CACHE_BUDGET_WORDS,
    MATERIAL_CACHE_MAX_ENTRIES,
};
pub use errors::RendererError;
pub use gles::{
    GlesApi, GlesCmd, GlesError, GlesProgram, GlesRenderer, GlesTarget, GlesTexture, RecordingGles,
    RefGles, RendererBackend, RendererChoice, RendererDecision, FRAGMENT_SRC, VERTEX_SRC,
};
pub use layer::{
    scanout_candidate, OutputDesc, ScanoutDecision, ScanoutRejection, SurfaceLayer, TonePolicy,
    OUTPUT_FORMATS,
};
pub use pipeline::{primaries_matrix, ColorPipeline, Matrix3};
pub use sample::{format_has_alpha, sample_premul_u8};
pub use software::{CompletedFrame, SoftwareRenderer};
pub use style::{
    BackdropParams, EdgeLightParams, EffectChoice, EffectTier, LayerStyle, Material, ShadowParams,
};
pub use transfer::{decode_transfer, encode_transfer};
pub use view::BufferView;
pub use yuv::{ycbcr_coefficients, YuvCoefficients};

/// The rendering contract every backend implements (`architecture.md` §9.1).
///
/// One frame is: [`begin_frame`](Renderer::begin_frame) (target output +
/// damage) → [`submit`](Renderer::submit) (back-to-front layer list; each
/// layer carries its buffer, placement, transform and color description) →
/// [`end_frame`](Renderer::end_frame) (completion with statistics). The GL
/// backend (Phase 9) adds fences and plane handoff through the same shape.
///
/// Layers must arrive **back-to-front** — the painter's order of the
/// compositor's render list. Writes land exactly on damaged pixels;
/// undamaged framebuffer content must stay valid because only the damaged
/// subset of the layer stack is re-submitted.
pub trait Renderer {
    /// Backend identity for logs and conformance labels.
    fn backend(&self) -> &'static str;

    /// Begin a frame for `output`, clipping all subsequent writes to
    /// `damage`.
    ///
    /// # Errors
    ///
    /// Invalid output descriptions (see [`OutputDesc::new`]).
    fn begin_frame(
        &mut self,
        output: &OutputDesc,
        damage: &ldp_core::geometry::Region,
    ) -> Result<(), RendererError>;

    /// Composite `layers` (back-to-front) into the open frame and report
    /// the frame's accumulated statistics.
    ///
    /// # Errors
    ///
    /// [`RendererError::NoFrameInProgress`] without an open frame.
    fn submit(&mut self, layers: &[SurfaceLayer<'_>]) -> Result<RenderStats, RendererError>;

    /// Clear exactly the open frame's damage region to one color — the
    /// background pass. Damage a submitted layer stack does not cover
    /// falls back to this; callers drawing an opaque background first
    /// keep undamaged pixels untouched (the damage-clip contract).
    ///
    /// # Errors
    ///
    /// [`RendererError::NoFrameInProgress`] without an open frame.
    fn clear_damage(&mut self, r: u8, g: u8, b: u8, a: u8) -> Result<(), RendererError>;

    /// Finish the frame.
    ///
    /// # Errors
    ///
    /// [`RendererError::NoFrameInProgress`] without an open frame.
    fn end_frame(&mut self) -> Result<CompletedFrame, RendererError>;
}

/// A minimal SplitMix64 generator for in-crate randomized tests (the
/// renderer takes no dependencies; the constants are the canonical
/// reference set). Test-only.
#[cfg(test)]
#[derive(Clone, Copy)]
pub(crate) struct TestRng {
    state: u64,
}

#[cfg(test)]
impl TestRng {
    /// Seed the generator (any seed works — the mixer escapes zero).
    pub(crate) fn new(seed: u64) -> TestRng {
        TestRng { state: seed }
    }

    /// Next raw 64-bit value.
    pub(crate) fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// The crate's shared test generator entry point (seeded, deterministic).
#[cfg(test)]
pub(crate) fn test_rng() -> TestRng {
    // "LDP22SEED" as hex-friendly bytes; any fixed constant is fine.
    TestRng::new(0x1D0_0225_EED0_0001)
}
