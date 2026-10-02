//! The render path's inputs: output description, surface layers, and the
//! direct-scanout eligibility check.
//!
//! A [`SurfaceLayer`] is one composited buffer: where it lands on the output
//! (`dest`, output-space, scale already folded in), its orientation
//! (`transform`), its color description, and a surface-level opacity
//! animation factor. The renderer never re-derives scene state — the
//! compositor's snapshot already resolved positions, transforms and scale.

use ldp_core::buffer::FourCC;
use ldp_core::color::ColorDescription;
use ldp_core::geometry::{Rect, Region, Size, Transform};

use crate::errors::RendererError;
use crate::style::LayerStyle;
use crate::view::BufferView;

/// Formats the software framebuffer can hold (the 32-bit RGB family).
pub const OUTPUT_FORMATS: [FourCC; 4] = [
    FourCC::XRGB8888,
    FourCC::ARGB8888,
    FourCC::XBGR8888,
    FourCC::ABGR8888,
];

/// The target of a frame: size, pixel format, color description, and
/// the output's layout origin.
///
/// The origin identifies *which* output of a multi-output desktop a
/// frame belongs to (Phase 31): renderers key their persistent
/// framebuffers by it, so each output's undamaged regions survive the
/// alternation of a shared renderer. The default constructor roots at
/// `(0, 0)` — the single-output doctrine, byte-identical.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutputDesc {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Framebuffer format; one of [`OUTPUT_FORMATS`].
    pub format: FourCC,
    /// The output's color description (blending and encoding target).
    pub color: ColorDescription,
    /// The output's layout origin in desktop coordinates — the
    /// renderer's framebuffer identity (several outputs share one
    /// renderer; each keeps its own canonical frame).
    pub origin: (i32, i32),
}

impl OutputDesc {
    /// Validate and build.
    ///
    /// # Errors
    ///
    /// [`RendererError::ZeroOutputSize`] or
    /// [`RendererError::UnsupportedOutputFormat`].
    pub fn new(
        width: u32,
        height: u32,
        format: FourCC,
        color: ColorDescription,
    ) -> Result<OutputDesc, RendererError> {
        if width == 0 || height == 0 {
            return Err(RendererError::ZeroOutputSize);
        }
        if !OUTPUT_FORMATS.contains(&format) {
            return Err(RendererError::UnsupportedOutputFormat(format));
        }
        Ok(OutputDesc {
            width,
            height,
            format,
            color,
            origin: (0, 0),
        })
    }

    /// Validate and build at a layout origin (the multi-output
    /// doctrine's output identity).
    ///
    /// # Errors
    ///
    /// The same validation as [`OutputDesc::new`].
    pub fn with_origin(
        width: u32,
        height: u32,
        format: FourCC,
        color: ColorDescription,
        origin: (i32, i32),
    ) -> Result<OutputDesc, RendererError> {
        let mut desc = Self::new(width, height, format, color)?;
        desc.origin = origin;
        Ok(desc)
    }

    /// The output's full bounds as a [`Rect`] rooted at the origin.
    #[must_use]
    pub fn rect(&self) -> Rect {
        Rect::new(0, 0, self.width, self.height)
    }
}

/// The per-layer luminance tail (Phase 38): what the negotiated
/// panel peak and the content's own mastering metadata conclude
/// for one layer's ink.
///
/// The v0.10.0 HDR pipeline decoded HDR layers onto the PQ canvas
/// and let whatever code values arrived pass through — the panel's
/// own hard clip was the only ceiling, and the static metadata
/// (`set_hdr_metadata`) refined nothing. The tail closes that: the
/// compositor computes the layer's policy from the *negotiated*
/// ceiling (the stack's brightest content clamped to the panel's
/// effective peak) and the layer's declared mastering bounds, and
/// the pipeline applies it in the linear domain — the system's
/// rolloff replaces the panel's hard clip (the system owns the
/// materials).
///
/// SDR layers carry [`TonePolicy::Pass`] (they ride the BT.2408
/// anchor unchanged), and so does every layer on an SDR canvas (the
/// anchor-pull doctrine, pinned by the existing goldens). The
/// GL v1 path composites only `Pass` policies — a tail-bearing
/// layer fails typed rather than silently ignoring the refinement
/// (the honest-boundary doctrine the whole backend follows).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum TonePolicy {
    /// No luminance work: SDR layers (the BT.2408 anchor is the whole
    /// story), HDR layers on an SDR canvas (the anchor-pull
    /// doctrine), and identity pipelines (the fast paths stay
    /// byte-identical).
    Pass,
    /// Clip at `ceiling_nits`: HDR content with no declared
    /// mastering metadata — the honest default. Never exceed the
    /// panel; nothing else is known.
    Clip {
        /// The negotiated ceiling (nits).
        ceiling_nits: f32,
    },
    /// The BT.2390-structured knee from the content's declared
    /// mastering range onto the ceiling — the per-surface luminance
    /// metadata refinement. Content dimmer than the ceiling rides
    /// at its own level (the EETF is the identity when the display
    /// covers the master range); content brighter rolls off through
    /// the knee instead of the panel's hard clip.
    Eetf {
        /// The mastering range's floor (nits).
        master_min_nits: f32,
        /// The mastering range's peak (nits).
        master_max_nits: f32,
        /// The negotiated ceiling (nits).
        ceiling_nits: f32,
    },
}

impl TonePolicy {
    /// The policy's ceiling (nits) — `None` when the policy passes.
    #[must_use]
    pub const fn ceiling_nits(&self) -> Option<f32> {
        match *self {
            TonePolicy::Pass => None,
            TonePolicy::Clip { ceiling_nits } | TonePolicy::Eetf { ceiling_nits, .. } => {
                Some(ceiling_nits)
            }
        }
    }

    /// Whether the policy does luminance work at all (the fast-path
    /// guard: `Pass` layers keep every byte-exact integer path).
    #[must_use]
    pub const fn is_pass(&self) -> bool {
        matches!(self, TonePolicy::Pass)
    }
}

/// One layer to composite: a buffer plus its output-space placement —
/// or an imported EGLImage (the DMA-BUF zero-copy path: the pixels
/// never cross the CPU).
#[derive(Clone, Debug)]
pub struct SurfaceLayer<'a> {
    /// The buffer (data + validated geometry + stable identity).
    pub buffer: BufferView<'a>,
    /// Output-space placement; the scale is folded in by the caller
    /// (`dest` size versus the transform-swapped buffer size is the
    /// effective scaling factor).
    pub dest: Rect,
    /// Buffer-to-surface orientation.
    pub transform: Transform,
    /// The buffer's color description.
    pub color: ColorDescription,
    /// The per-layer luminance tail (Phase 38): the negotiated
    /// ceiling and the content's declared mastering bounds, as the
    /// compositor concluded them. The default (`Pass`) keeps every
    /// pre-Phase-38 pixel oracle byte-identical.
    pub tone: TonePolicy,
    /// Surface-level opacity in `0..=1` (animation fades); clamped here.
    pub opacity: f32,
    /// The surface's opaque region in *surface* coordinates — informational
    /// for blending (per-pixel alpha drives the fast write path) and the
    /// input to direct-scanout eligibility.
    pub opaque: Region,
    /// The Liquid style the system adds around the pixels (Phase 27):
    /// rounded corners, shadow, frosted backdrop. The default is plain
    /// — Phase 26 behavior, byte-identical.
    pub style: LayerStyle,
    /// The imported EGLImage handle when this layer is a DMA-BUF
    /// zero-copy composite (Phase 31): the pixels never cross the CPU
    /// — the GL backend binds the image as a texture directly. `None`
    /// (always, on the CPU-buffer path) composites the `buffer`'s
    /// bytes; the software renderer refuses the zero-copy arm (an
    /// imported image has no words to blend — the honest boundary).
    pub egl_image: Option<u64>,
}

impl<'a> SurfaceLayer<'a> {
    /// Build a layer, clamping `opacity` into `0..=1`.
    #[must_use]
    pub fn new(
        buffer: BufferView<'a>,
        dest: Rect,
        transform: Transform,
        color: ColorDescription,
        opacity: f32,
        opaque: Region,
    ) -> SurfaceLayer<'a> {
        let opacity = if opacity.is_finite() {
            opacity.clamp(0.0, 1.0)
        } else {
            1.0
        };
        SurfaceLayer {
            buffer,
            dest,
            transform,
            color,
            tone: TonePolicy::Pass,
            opacity,
            opaque,
            style: LayerStyle::default(),
            egl_image: None,
        }
    }

    /// Opacity quantized to the blending grid (`0..=255`).
    ///
    /// All paths (integer fast path and float pipeline) use the *same*
    /// quantized factor so results agree across path selection.
    #[must_use]
    pub fn opacity_q(&self) -> u8 {
        (self.opacity.clamp(0.0, 1.0) * 255.0).round() as u8
    }

    /// Whether this layer samples 1:1 (no scaling): the destination size
    /// equals the transform-swapped buffer size.
    #[must_use]
    pub fn is_one_to_one(&self) -> bool {
        let g = self.buffer.geometry();
        let swapped = self
            .transform
            .transform_size(Size::new(g.width(), g.height()));
        self.dest.w == swapped.w && self.dest.h == swapped.h
    }

    /// Whether the layer's destination exactly covers the output.
    #[must_use]
    pub fn covers_output(&self, output: &OutputDesc) -> bool {
        self.dest == output.rect()
    }
}

/// Why a layer is not direct-scanout eligible.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum ScanoutRejection {
    /// The destination does not exactly cover the output.
    NotFullScreen,
    /// The buffer is rotated or mirrored (no transform hardware assumption).
    Transformed,
    /// The buffer is not output-sized — KMS planes do not scale on typical
    /// hardware, so a scaled layer must composite.
    Scaled,
    /// Opacity below 1 (would need to be blended under something).
    Translucent,
    /// The layer blends in a different color description than the output.
    ColorMismatch,
    /// The buffer format differs from the output format.
    FormatMismatch,
    /// The opaque region does not cover the whole destination.
    NotOpaque,
}

/// The outcome of a direct-scanout eligibility check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScanoutDecision {
    /// Whether the layer could go on a primary plane unchanged.
    pub eligible: bool,
    /// The first failing condition when not eligible.
    pub rejection: Option<ScanoutRejection>,
}

/// Direct-scanout eligibility: can this layer be handed to a KMS primary
/// plane byte-unchanged (`docs/architecture.md` §9.4)?
///
/// The conditions are the classic set: fullscreen placement, no transform,
/// fully opaque (both opacity and opaque region), identical format, and an
/// identical color description so no pipeline stage intervenes. The
/// scheduler's plane-assignment oracle (Phase 15) re-checks with
/// `DRM_MODE_ATOMIC_TEST_ONLY`; this predicate is the cheap pre-filter.
#[must_use]
pub fn scanout_candidate(layer: &SurfaceLayer<'_>, output: &OutputDesc) -> ScanoutDecision {
    let reject = |r: ScanoutRejection| ScanoutDecision {
        eligible: false,
        rejection: Some(r),
    };
    if !layer.covers_output(output) {
        return reject(ScanoutRejection::NotFullScreen);
    }
    if layer.transform != Transform::Normal {
        return reject(ScanoutRejection::Transformed);
    }
    let g = layer.buffer.geometry();
    if g.width() != output.width || g.height() != output.height {
        return reject(ScanoutRejection::Scaled);
    }
    if layer.opacity < 1.0 {
        return reject(ScanoutRejection::Translucent);
    }
    if layer.color != output.color {
        return reject(ScanoutRejection::ColorMismatch);
    }
    if g.format() != output.format {
        return reject(ScanoutRejection::FormatMismatch);
    }
    // Untransformed fullscreen: surface coordinates == output coordinates.
    // The holes of the opaque region inside the destination are what
    // disqualify scanout: `dest \ opaque` must be empty.
    let opaque_here = layer.opaque.clipped_to(layer.dest);
    let holes = Region::from_rect(layer.dest).subtract(&opaque_here);
    if !holes.is_empty() {
        return reject(ScanoutRejection::NotOpaque);
    }
    ScanoutDecision {
        eligible: true,
        rejection: None,
    }
}
