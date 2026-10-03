//! The reference software backend: a scanline compositor over a `u32`
//! framebuffer.
//!
//! [`SoftwareRenderer`] implements the [`Renderer`] contract with the
//! [`Mode`] dispatch of `composite.rs`; it owns no clock, no threads and no
//! allocation outside frame resize — `submit` is a pure function of
//! (framebuffer state, damage, layers). The framebuffer stores
//! **premultiplied** RGBA (alpha forced to 255 for X-family outputs), the
//! convention every blend kernel and the Phase 16/17 bridges assume.

use ldp_core::buffer::FourCC;
use ldp_core::geometry::{Rect, Region};

use crate::composite::{
    apply_material, composite_layer, opaque_path_ok, save_region_into, LayerPlan, Mode, RenderStats,
};
use crate::effects::{apply_corner_fold, EdgeMemo, FrostMemo, MaterialCache};
use crate::errors::RendererError;
use crate::layer::{OutputDesc, SurfaceLayer};
use crate::mapping::{FrameTarget, Mapping};
use crate::pipeline::ColorPipeline;
use crate::spans::{RowIndex, RowSpans};
use crate::yuv::ycbcr_coefficients;
use crate::Renderer;

/// A finished frame: statistics plus the output it was rendered into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompletedFrame {
    /// The compositing work the frame performed.
    pub stats: RenderStats,
    /// Output width.
    pub width: u32,
    /// Output height.
    pub height: u32,
    /// Output format.
    pub format: FourCC,
}

/// The software reference backend.
///
/// ```no_run
/// use ldp_core::geometry::{Rect, Region};
/// use ldp_renderer::{OutputDesc, Renderer, SoftwareRenderer};
/// use ldp_core::color::ColorDescription;
/// use ldp_core::buffer::FourCC;
///
/// let mut renderer = SoftwareRenderer::new();
/// let output = OutputDesc::new(320, 200, FourCC::XRGB8888, ColorDescription::srgb_sdr()).unwrap();
/// renderer.begin_frame(&output, &Region::new()).unwrap();
/// let stats = renderer.submit(&[]).unwrap();
/// assert_eq!(stats.layers, 0);
/// let done = renderer.end_frame().unwrap();
/// assert_eq!(done.width, 320);
/// ```
pub struct SoftwareRenderer {
    fb: Vec<u32>,
    out: Option<OutputDesc>,
    damage: Region,
    in_frame: bool,
    stats: RenderStats,
    spans: RowSpans,
    /// The per-frame damage row index (Phase 22): built once per frame,
    /// shared by every layer, all buffers retained across frames.
    index: RowIndex,
    /// The styled path's retained state (Phase 29).
    style: StyleState,
    /// The other outputs' parked framebuffers (Phase 31): a shared
    /// renderer alternates between the outputs of a multi-output
    /// desktop, and each output's undamaged regions must survive the
    /// alternation — the damage-clip contract is per *output*, not per
    /// renderer. Parked by the output's full geometry identity —
    /// origin, size, format (Phase 37: the origin alone is not unique;
    /// a mirrored desktop places every output at the same origin, and
    /// two same-size mirrors *share* one canvas — they show identical
    /// content — while a different-size mirror keeps its own) — a
    /// small LRU (the practical desktops are 1-4 monitors); a renderer
    /// that only ever serves one output never parks anything, keeping
    /// the single-output doctrine byte-identical.
    parked: Vec<(CanvasKey, Vec<u32>)>,
}

/// The framebuffer's identity: where it sits on the desktop and what
/// shape it is (Phase 37 — the origin alone stops being unique the
/// moment two displays overlap, which is exactly what a mirror is).
type CanvasKey = ((i32, i32), u32, u32, FourCC);

/// The key a parked canvas files under.
const fn canvas_key(o: &OutputDesc) -> CanvasKey {
    (o.origin, o.width, o.height, o.format)
}

/// The styled path's retained state (Phase 29): the shadow and frost
/// memos plus the frost working buffers — every steady-state frame
/// allocates nothing, and the first frame of any (params, geometry)
/// pays once, ever.
///
/// Pure memoization throughout (the crate's doctrine holds): the
/// cached bytes are exactly what the Phase 27 per-frame computation
/// produced, so `submit` remains a pure function of (framebuffer
/// state, damage, layers) — the styled golden suites re-render frames
/// and pin the bytes.
#[derive(Debug, Default)]
pub struct StyleState {
    /// The shadow material memo (one build per unique shadow).
    shadows: MaterialCache,
    /// The frost material memo (one build per unchanged backdrop).
    frost: FrostMemo,
    /// The edge-light material memo (Phase 40: one build per unique
    /// hairline — the ring is imagery like the shadow).
    edges: EdgeMemo,
    /// The frost snapshot's retained destination buffer.
    frost_saved: Vec<u32>,
    /// The frost material's unfolded working copy (the corner fold
    /// applies here, per frame, band-limited).
    frost_words: Vec<u32>,
    /// The edge-light material's working copy (served from the memo
    /// into the shared `apply_material` write path).
    edge_words: Vec<u32>,
}

impl StyleState {
    /// Empty state (nothing memoized, nothing retained).
    #[must_use]
    pub fn new() -> StyleState {
        StyleState::default()
    }
}

impl Default for SoftwareRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl SoftwareRenderer {
    /// A renderer with no output yet (the first `begin_frame` allocates).
    #[must_use]
    pub fn new() -> SoftwareRenderer {
        SoftwareRenderer {
            fb: Vec::new(),
            out: None,
            damage: Region::new(),
            in_frame: false,
            stats: RenderStats::default(),
            spans: RowSpans::default(),
            index: RowIndex::default(),
            style: StyleState::new(),
            parked: Vec::new(),
        }
    }

    /// The framebuffer as packed output-format words (row-major). Empty
    /// before the first frame; keeps the last frame's contents after
    /// `end_frame` so callers can read it out.
    #[must_use]
    pub fn readout(&self) -> &[u32] {
        &self.fb
    }

    /// Current output dimensions, if a frame has begun.
    #[must_use]
    pub fn dimensions(&self) -> Option<(u32, u32)> {
        self.out.map(|o| (o.width, o.height))
    }

    /// How many shadow materials were built since construction (the
    /// thrash oracle — Phase 30): rendering the same scene twice must
    /// not grow it, whatever the output size. A 4K desktop's shadow
    /// working set once exceeded the fixed 6 Mi-word budget and
    /// evicted-and-rebuilt every material every frame (194 ms steady
    /// state); the output-scaled budget keeps the working set resident
    /// (28 ms) and this counter proves it stays flat.
    #[must_use]
    pub fn shadow_rebuilds(&self) -> u64 {
        self.style.shadows.rebuilds()
    }

    /// How many frost materials were built since construction (the
    /// Phase 55 thrash oracle, the shadow counter's own doctrine): a
    /// steady re-render of the same scene must not grow it. A desktop
    /// of Liquid bands is a desktop of frost panes — this counter is
    /// what proves the memo's budget holds the whole working set
    /// resident instead of evict-and-rebuild every frame.
    #[must_use]
    pub fn frost_rebuilds(&self) -> u64 {
        self.style.frost.rebuilds()
    }

    /// Clear the framebuffer to one premultiplied color during a frame.
    ///
    /// # Errors
    ///
    /// [`RendererError::NoFrameInProgress`] without an open frame.
    pub fn clear(&mut self, r: u8, g: u8, b: u8, a: u8) -> Result<(), RendererError> {
        let Some(out) = self.out else {
            return Err(RendererError::NoFrameInProgress);
        };
        let px = crate::composite::pack_out(out.format, r, g, b, a);
        self.fb.fill(px);
        Ok(())
    }

    /// Resolve a layer into its compositing plan (path + mapping).
    ///
    /// `corner_radius` arrives sanitized (the caller clamped the style
    /// against the destination's dimensions) — a nonzero value routes
    /// the layer through the Liquid per-pixel path in `composite.rs`.
    fn plan_layer<'a>(
        layer: &'a SurfaceLayer<'a>,
        out: &OutputDesc,
        corner_radius: u32,
    ) -> LayerPlan<'a> {
        let g = layer.buffer.geometry();
        let format = g.format();
        let q = layer.opacity_q();
        // Phase 38: a tail-bearing layer takes the scene-linear path
        // whatever its description says — the encoded-space fast
        // paths must not bypass the luminance rolloff.
        let identity = layer.color == out.color && layer.tone.is_pass();
        let one_to_one = layer.is_one_to_one();
        let t = layer.transform;

        // Same-format-family 1:1 untransformed fully-opaque layers never
        // need to read the destination: X-family words copy with forced
        // alpha, A-family words copy per opaque chunk (Phase 22; Phase 29
        // broadens "same format" to the same *word family* — an XRGB
        // window over an ARGB output copies too, the X byte forced —
        // byte-equal to the sampling path by the field orders).
        let src_is_argb_order = matches!(format, FourCC::XRGB8888 | FourCC::ARGB8888);
        let out_is_argb_order = matches!(out.format, FourCC::XRGB8888 | FourCC::ARGB8888);
        let src_is_bgr_order = matches!(format, FourCC::XBGR8888 | FourCC::ABGR8888);
        let out_is_bgr_order = matches!(out.format, FourCC::XBGR8888 | FourCC::ABGR8888);
        let same_family_copy = identity
            && q == 255
            && t == ldp_core::geometry::Transform::Normal
            && one_to_one
            && ((src_is_argb_order && out_is_argb_order) || (src_is_bgr_order && out_is_bgr_order));
        let mode = if same_family_copy && matches!(format, FourCC::XRGB8888 | FourCC::XBGR8888) {
            Mode::Copy
        } else if same_family_copy && matches!(format, FourCC::ARGB8888 | FourCC::ABGR8888) {
            Mode::CopyPremul
        } else if identity && opaque_path_ok(format, q) {
            Mode::Opaque
        } else if identity {
            Mode::Alpha
        } else {
            Mode::Pipeline
        };

        let mapping = if one_to_one {
            if t == ldp_core::geometry::Transform::Normal {
                Mapping::Direct {
                    ox: layer.dest.x,
                    oy: layer.dest.y,
                }
            } else {
                Mapping::Integer {
                    t,
                    w: g.width() as i32,
                    h: g.height() as i32,
                    ox: layer.dest.x,
                    oy: layer.dest.y,
                }
            }
        } else {
            let (sw, sh) = if t.swaps_axes() {
                (g.height(), g.width())
            } else {
                (g.width(), g.height())
            };
            Mapping::Scaled {
                t,
                w: g.width() as i32,
                h: g.height() as i32,
                ox: layer.dest.x as f32,
                oy: layer.dest.y as f32,
                ax: (sw as f32) / (layer.dest.w as f32),
                ay: (sh as f32) / (layer.dest.h as f32),
            }
        };

        LayerPlan {
            view: &layer.buffer,
            dest: layer.dest,
            mapping,
            opacity_q: q,
            coeffs: ycbcr_coefficients(layer.color.primaries),
            range: layer.color.range,
            pipeline: ColorPipeline::with_tone(&layer.color, &out.color, &layer.tone),
            mode,
            corner_radius,
        }
    }

    /// The Liquid passes of one styled layer, in the macOS order:
    /// snapshot the backdrop (before anything of *this* layer touches
    /// it), replace it with the frosted material — rounded like the
    /// glass pane itself, so the corner cutouts stay honest — then
    /// draw the shadow (holed under the ink; its soft fringe lands in
    /// the cutouts over the untouched backdrop), then let
    /// `composite_layer` draw the pixels with the corner coverage
    /// folded in. Every pass is damage-clipped through the frame's
    /// row index.
    ///
    /// Phase 29: the materials come from the memo — the shadow built
    /// once per (params, geometry), the frost rebuilt only when the
    /// backdrop's words actually changed (a full word comparison, no
    /// hashing) — and the corner fold is band-limited. The steady
    /// state: one snapshot copy, one word comparison, zero blur.
    fn apply_style_passes(
        target: &mut FrameTarget<'_>,
        index: &RowIndex,
        spans: &mut RowSpans,
        stats: &mut RenderStats,
        dest: Rect,
        style: &crate::style::LayerStyle,
        style_state: &mut StyleState,
    ) {
        // The frost snapshot comes first: the material samples the
        // backdrop as it sits beneath the layer, before the shadow
        // fringes it.
        if let Some(backdrop) = &style.backdrop {
            save_region_into(target, dest, &mut style_state.frost_saved);
            let material = style_state
                .frost
                .get_or_build(dest, backdrop, &style_state.frost_saved);
            // The glass pane is rounded: the working copy carries the
            // same corner coverage the ink will (the GL upload fold
            // applies the identical scaling, keeping the backends
            // byte-equal).
            style_state.frost_words.clear();
            style_state.frost_words.extend_from_slice(material);
            apply_corner_fold(
                &mut style_state.frost_words,
                dest.w,
                dest.h,
                style.corner_radius,
            );
            apply_material(target, index, spans, stats, dest, &style_state.frost_words);
        }
        if let Some(shadow) = &style.shadow {
            let (material, rect) =
                style_state
                    .shadows
                    .get_or_build(shadow, dest, style.corner_radius);
            apply_material(target, index, spans, stats, rect, material);
        }
    }

    /// The Liquid's last stroke (Phase 40): the luminous hairline,
    /// drawn *over the ink* at the rounded silhouette's inner 1-pixel
    /// ring — the light catch that separates glass from its backdrop.
    /// Called after [`composite_layer`] so the stroke reads over the
    /// layer's own pixels (the macOS material edge).
    ///
    /// The ring is memoized imagery (the shadow's doctrine): served
    /// from [`EdgeMemo`] per `(dest, radius, params)`, staged into the
    /// retained working copy, and written through the same
    /// damage-clipped `apply_material` path the shadow and frost use —
    /// the GL stream's textured quad of the identical words is the
    /// byte-equality anchor.
    fn apply_edge_light(
        target: &mut FrameTarget<'_>,
        index: &RowIndex,
        spans: &mut RowSpans,
        stats: &mut RenderStats,
        dest: Rect,
        style: &crate::style::LayerStyle,
        style_state: &mut StyleState,
    ) {
        let Some(light) = style.edge_light else {
            return;
        };
        let material = style_state
            .edges
            .get_or_build(dest, style.corner_radius, light);
        style_state.edge_words.clear();
        style_state.edge_words.extend_from_slice(material);
        apply_material(target, index, spans, stats, dest, &style_state.edge_words);
    }
}

impl Renderer for SoftwareRenderer {
    fn backend(&self) -> &'static str {
        "software-scalar"
    }

    fn begin_frame(&mut self, output: &OutputDesc, damage: &Region) -> Result<(), RendererError> {
        let pixels = output.width as usize * output.height as usize;
        // The output switch (Phase 31): a different origin (or a
        // different geometry) means a different output's frame — park
        // the current framebuffer under its own key and restore (or
        // allocate) the new one. The alternation keeps every output's
        // undamaged content; a same-output re-begin (the single-output
        // doctrine and steady-state multi frames) touches nothing.
        let switched = self.out.is_some_and(|o| {
            o.origin != output.origin
                || o.width != output.width
                || o.height != output.height
                || o.format != output.format
        });
        if switched {
            let key = canvas_key(self.out.as_ref().expect("checked above"));
            let current = core::mem::take(&mut self.fb);
            // Park the outgoing frame (LRU head; bounded).
            if let Some(pos) = self.parked.iter().position(|(k, _)| *k == key) {
                self.parked[pos].1 = current;
            } else {
                self.parked.insert(0, (key, current));
                if self.parked.len() > 4 {
                    self.parked.pop();
                }
            }
            // Restore the incoming output's parked frame, if any —
            // by the full geometry identity: a same-origin canvas of
            // a *different size* (the mirror's mixed modes) is never
            // mistaken for this output's (Phase 37 — the crop's own
            // per-pixel proof caught the origin-keyed restore
            // truncating the wrong canvas into the wrong shape).
            match self
                .parked
                .iter()
                .position(|(k, _)| *k == canvas_key(output))
            {
                Some(pos) => {
                    self.fb = self.parked.remove(pos).1;
                    if self.fb.len() != pixels {
                        self.fb.resize(pixels, 0xFF00_0000);
                    }
                }
                None => {
                    self.fb = vec![0xFF00_0000; pixels];
                }
            }
        } else if self.fb.len() != pixels {
            self.fb.resize(pixels, 0xFF00_0000);
        }
        self.out = Some(*output);
        // Phase 30: the shadow cache's budget scales with the output —
        // twice its pixels, floored at the phone-era 6 Mi words. A 4K
        // desktop's working set (five shadows ≈ 6.5 Mi words) thrashed
        // the fixed cap, rebuilding every material every frame; a
        // phone-sized output keeps the floor and its byte-exact
        // Phase 29 behavior (the budget only ever grows).
        self.style
            .shadows
            .set_budget_words(pixels.saturating_mul(2));
        // Phase 55: the frost memo rides the same output-scaled
        // doctrine — one pane retains four buffers over its rect (the
        // saved backdrop, the material, the blur ping-pong pair), so
        // the budget counts the true footprint (the shadow's x2 is
        // per-material words; the frost's x4 is per-entry).
        self.style.frost.set_budget_words(pixels.saturating_mul(4));
        // Damage is clipped to the output once; the row index serves each
        // row's merged intervals to every layer (Phase 22 — the per-layer
        // rescan of every damage rect is gone).
        self.damage = damage.clipped_to(output.rect());
        self.index.build(&self.damage, &output.rect());
        self.in_frame = true;
        self.stats = RenderStats::default();
        Ok(())
    }

    fn submit(&mut self, layers: &[SurfaceLayer<'_>]) -> Result<RenderStats, RendererError> {
        if !self.in_frame {
            return Err(RendererError::NoFrameInProgress);
        }
        let Some(out) = self.out else {
            return Err(RendererError::NoFrameInProgress);
        };
        let mut stats = self.stats;
        stats.layers += layers.len() as u32;
        for layer in layers {
            // Sanitize the style against the destination's own extents
            // (the pipeline never fails — out-of-domain styles clamp).
            let style = layer.style.sanitized(layer.dest.w, layer.dest.h);
            let mut target = FrameTarget {
                fb: &mut self.fb,
                width: out.width,
                height: out.height,
                format: out.format,
            };
            if !style.is_plain() {
                Self::apply_style_passes(
                    &mut target,
                    &self.index,
                    &mut self.spans,
                    &mut stats,
                    layer.dest,
                    &style,
                    &mut self.style,
                );
            }
            let plan = Self::plan_layer(layer, &out, style.corner_radius);
            composite_layer(&mut target, &plan, &self.index, &mut self.spans, &mut stats);
            if style.edge_light.is_some() {
                Self::apply_edge_light(
                    &mut target,
                    &self.index,
                    &mut self.spans,
                    &mut stats,
                    layer.dest,
                    &style,
                    &mut self.style,
                );
            }
        }
        self.stats = stats;
        Ok(stats)
    }

    fn clear_damage(&mut self, r: u8, g: u8, b: u8, a: u8) -> Result<(), RendererError> {
        if !self.in_frame {
            return Err(RendererError::NoFrameInProgress);
        }
        let Some(out) = self.out else {
            return Err(RendererError::NoFrameInProgress);
        };
        let px = crate::composite::pack_out(out.format, r, g, b, a);
        let bounds = out.rect();
        for y in 0..out.height as i32 {
            // The row index already holds damage ∩ row (clipped to the
            // output); clipping to the full-bounds dest is the identity.
            let row = self.index.row(y);
            if row.is_empty() {
                continue;
            }
            RowSpans::clip(row, &bounds, &mut self.spans);
            for &(x0, x1) in self.spans.intervals() {
                let row = (y as u32 * out.width) as usize;
                self.fb[row + x0 as usize..row + x1 as usize].fill(px);
            }
        }
        Ok(())
    }

    fn end_frame(&mut self) -> Result<CompletedFrame, RendererError> {
        if !self.in_frame {
            return Err(RendererError::NoFrameInProgress);
        }
        self.in_frame = false;
        let Some(out) = self.out else {
            return Err(RendererError::NoFrameInProgress);
        };
        Ok(CompletedFrame {
            stats: self.stats,
            width: out.width,
            height: out.height,
            format: out.format,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ldp_core::buffer::{BufferGeometry, Modifier, PlaneLayout};
    use ldp_core::color::ColorDescription;
    use ldp_core::geometry::Rect;

    fn xrgb_output(w: u32, h: u32) -> OutputDesc {
        OutputDesc::new(w, h, FourCC::XRGB8888, ColorDescription::srgb_sdr()).unwrap()
    }

    fn one_pixel_layer(word: u32) -> SurfaceLayer<'static> {
        // A 1x1 XRGB buffer; leak-free via a leaked box (test-only).
        let data: &'static [u8] = Box::leak(word.to_le_bytes().to_vec().into_boxed_slice());
        let layout = PlaneLayout {
            offset: 0,
            stride: 4,
        };
        let g =
            BufferGeometry::new(1, 1, FourCC::XRGB8888, Modifier::LINEAR, &[layout], 4).unwrap();
        let view = crate::view::BufferView::new(1, data, g).unwrap();
        SurfaceLayer::new(
            view,
            Rect::new(0, 0, 1, 1),
            ldp_core::geometry::Transform::Normal,
            ColorDescription::srgb_sdr(),
            1.0,
            Region::from_rect(Rect::new(0, 0, 1, 1)),
        )
    }

    #[test]
    fn protocol_errors_are_precise() {
        let mut r = SoftwareRenderer::new();
        assert!(matches!(
            r.submit(&[]),
            Err(RendererError::NoFrameInProgress)
        ));
        assert!(matches!(
            r.end_frame(),
            Err(RendererError::NoFrameInProgress)
        ));
        assert!(r.readout().is_empty());
        let out = xrgb_output(4, 4);
        r.begin_frame(&out, &Region::new()).unwrap();
        // Damage outside the output composites nothing.
        let stats = r.submit(&[one_pixel_layer(0x00_FF_00_00)]).unwrap();
        assert_eq!(stats.layers, 1);
        assert_eq!(stats.layers_rendered, 0);
        r.end_frame().unwrap();
        // Double end is an error.
        assert!(matches!(
            r.end_frame(),
            Err(RendererError::NoFrameInProgress)
        ));
    }

    #[test]
    fn resize_reallocates_and_clears_opaque_black() {
        let mut r = SoftwareRenderer::new();
        r.begin_frame(&xrgb_output(2, 2), &Region::new()).unwrap();
        r.end_frame().unwrap();
        r.begin_frame(&xrgb_output(4, 2), &Region::new()).unwrap();
        let done = r.end_frame().unwrap();
        assert_eq!((done.width, done.height), (4, 2));
        assert_eq!(r.readout().len(), 8);
    }

    /// Phase 37's canvas identity: the parking key is the output's
    /// *geometry* (origin + size + format), not the origin alone — a
    /// mirrored desktop places every output at the same origin, and
    /// the origin-keyed restore would hand the 720p display the
    /// 1080p canvas it just parked, truncated into the wrong shape
    /// (the mirror session's per-pixel crop proof caught exactly
    /// that). The alternation must keep each geometry its own canvas
    /// — and a *same*-geometry mirror re-begin must not switch at
    /// all (the shared canvas is the mirror's own sharing).
    #[test]
    fn same_origin_different_size_keeps_its_own_canvas() {
        let mut r = SoftwareRenderer::new();
        // The 1080p frame paints red.
        let big = xrgb_output(8, 4);
        r.begin_frame(&big, &Region::from_rect(Rect::new(0, 0, 8, 4)))
            .unwrap();
        r.clear(0xFF, 0, 0, 0xFF).unwrap();
        r.end_frame().unwrap();
        let big_frame = r.readout().to_vec();
        assert!(big_frame.iter().all(|w| *w == 0xFFFF_0000));
        // The 720p (smaller, same origin) frame paints blue.
        let small = xrgb_output(4, 2);
        r.begin_frame(&small, &Region::from_rect(Rect::new(0, 0, 4, 2)))
            .unwrap();
        r.clear(0, 0, 0xFF, 0xFF).unwrap();
        r.end_frame().unwrap();
        assert!(r.readout().iter().all(|w| *w == 0xFF00_00FF));
        // Back to the big frame, zero damage: its red survived the
        // alternation *whole* — not the small canvas resized, not a
        // truncate of either.
        r.begin_frame(&big, &Region::new()).unwrap();
        r.end_frame().unwrap();
        assert_eq!(r.readout(), &big_frame, "the big canvas round-trips");
        // And the same-geometry re-begin never parks (the shared
        // canvas doctrine — same origin, same size: one canvas).
        assert_eq!(r.parked.len(), 1, "one parked canvas: the small one");
    }

    #[test]
    fn clear_packs_through_output_format() {
        let mut r = SoftwareRenderer::new();
        let out = xrgb_output(2, 1);
        r.begin_frame(&out, &Region::new()).unwrap();
        r.clear(0x11, 0x22, 0x33, 0x44).unwrap();
        // XRGB output forces alpha.
        assert_eq!(r.readout(), &[0xFF11_2233, 0xFF11_2233]);
    }

    /// Phase 29's exit oracle: the memoized styled path is
    /// **transparent** — a persistent renderer (warm shadow and frost
    /// memos, retained buffers) produces byte-identical frames and
    /// statistics to a fresh renderer's first render of the same
    /// scene, and repeated frames stay identical to each other.
    #[test]
    fn styled_steady_state_is_byte_equal_to_a_cold_render() {
        use crate::style::{BackdropParams, EdgeLightParams, LayerStyle, ShadowParams};
        let (w, h) = (48u32, 32u32);
        let output = xrgb_output(w, h);
        let (bg_data, bg_geom) = crate::testkit::pattern_buffer(w, h, FourCC::XRGB8888);
        let (card_data, card_geom) = crate::testkit::pattern_buffer(20, 14, FourCC::ARGB8888);
        let style = LayerStyle {
            corner_radius: 5,
            shadow: Some(ShadowParams {
                radius: 5,
                blur: 3,
                passes: 2,
                color: [10, 10, 30],
                alpha: 120,
                offset: (2, 3),
            }),
            backdrop: Some(BackdropParams {
                blur: 4,
                passes: 2,
                saturation: 200,
                tint: [0xF0, 0xF0, 0xF5],
                tint_alpha: 90,
            }),
            // Phase 40: the hairline rides the same oracle — the warm
            // edge memo must stay transparent against the cold render.
            edge_light: Some(EdgeLightParams {
                color: [0xFF, 0xFF, 0xFF],
                alpha: 90,
            }),
        };
        let damage = crate::testkit::full_damage(&output);
        let render_once = || -> (Vec<u32>, RenderStats) {
            let bg = crate::view::BufferView::new(1, &bg_data, bg_geom.clone()).unwrap();
            let card = crate::view::BufferView::new(2, &card_data, card_geom.clone()).unwrap();
            let mut under = SurfaceLayer::new(
                bg,
                Rect::new(0, 0, w, h),
                ldp_core::geometry::Transform::Normal,
                ColorDescription::srgb_sdr(),
                1.0,
                Region::from_rect(Rect::new(0, 0, w, h)),
            );
            under.style = LayerStyle::default();
            let mut card_layer = SurfaceLayer::new(
                card,
                Rect::new(9, 7, 20, 14),
                ldp_core::geometry::Transform::Normal,
                ColorDescription::srgb_sdr(),
                1.0,
                Region::from_rect(Rect::new(0, 0, 20, 14)),
            );
            card_layer.style = style;
            let mut r = SoftwareRenderer::new();
            r.begin_frame(&output, &damage).unwrap();
            let stats = r.submit(&[under, card_layer]).unwrap();
            r.end_frame().unwrap();
            (r.readout().to_vec(), stats)
        };
        // The warm renderer: three frames through one instance.
        let mut warm = SoftwareRenderer::new();
        let mut warm_stats = Vec::new();
        let mut warm_frames = Vec::new();
        for _ in 0..3 {
            let bg = crate::view::BufferView::new(1, &bg_data, bg_geom.clone()).unwrap();
            let card = crate::view::BufferView::new(2, &card_data, card_geom.clone()).unwrap();
            let mut under = SurfaceLayer::new(
                bg,
                Rect::new(0, 0, w, h),
                ldp_core::geometry::Transform::Normal,
                ColorDescription::srgb_sdr(),
                1.0,
                Region::from_rect(Rect::new(0, 0, w, h)),
            );
            under.style = LayerStyle::default();
            let mut card_layer = SurfaceLayer::new(
                card,
                Rect::new(9, 7, 20, 14),
                ldp_core::geometry::Transform::Normal,
                ColorDescription::srgb_sdr(),
                1.0,
                Region::from_rect(Rect::new(0, 0, 20, 14)),
            );
            card_layer.style = style;
            warm.begin_frame(&output, &damage).unwrap();
            let stats = warm.submit(&[under, card_layer]).unwrap();
            warm.end_frame().unwrap();
            warm_stats.push(stats);
            warm_frames.push(warm.readout().to_vec());
        }
        // Repeated warm frames agree with each other...
        assert_eq!(warm_frames[0], warm_frames[1], "frame 2 == frame 1");
        assert_eq!(warm_frames[1], warm_frames[2], "frame 3 == frame 2");
        assert_eq!(warm_stats[0], warm_stats[1]);
        assert_eq!(warm_stats[1], warm_stats[2]);
        // ...and the warm path equals the cold render, bytes and stats.
        let (cold_readout, cold_stats) = render_once();
        assert_eq!(warm_frames[2], cold_readout, "warm == cold bytes");
        assert_eq!(warm_stats[2], cold_stats, "warm == cold stats");
    }

    /// Phase 30's thrash oracle: the 4K desktop's shadow working set
    /// (five materials, ~6.5 Mi words) once exceeded the fixed 6 Mi
    /// word budget and evicted-and-rebuilt every material every frame
    /// (the 194 ms steady state the desktop matrix caught). The
    /// output-scaled budget (`begin_frame` pays twice the output's
    /// pixels) keeps the working set resident — proven by the flat
    /// [`SoftwareRenderer::shadow_rebuilds`] counter across steady
    /// frames, with the bytes and stats pinned equal too.
    #[test]
    fn four_k_desktop_steady_state_does_not_thrash_the_shadow_memo() {
        use crate::style::EffectTier;
        let (w, h) = (3840u32, 2160u32);
        let output = OutputDesc::new(w, h, FourCC::ARGB8888, ColorDescription::srgb_sdr()).unwrap();
        let (paper_data, paper_geom) = crate::testkit::pattern_buffer(w, h, FourCC::XRGB8888);
        let card_w = w * 2 / 5;
        let card_h = h * 2 / 5;
        let (card_data, card_geom) =
            crate::testkit::pattern_buffer(card_w, card_h, FourCC::ARGB8888);
        let dock_w = w * 3 / 4;
        let dock_h = h / 15 + 1;
        let (dock_data, dock_geom) =
            crate::testkit::pattern_buffer(dock_w, dock_h, FourCC::ARGB8888);
        let damage = crate::testkit::full_damage(&output);
        let card_style = EffectTier::High.opaque_style();
        let dock_style = EffectTier::High.translucent_style();

        let build_layers = || {
            let mut layers: Vec<SurfaceLayer<'_>> = Vec::with_capacity(6);
            let paper = crate::view::BufferView::new(1, &paper_data, paper_geom.clone()).unwrap();
            let mut under = SurfaceLayer::new(
                paper,
                Rect::new(0, 0, w, h),
                ldp_core::geometry::Transform::Normal,
                ColorDescription::srgb_sdr(),
                1.0,
                Region::from_rect(Rect::new(0, 0, w, h)),
            );
            under.style = crate::style::LayerStyle::default();
            layers.push(under);
            let inset_x = (w * 3 / 50) as i32;
            let inset_y = (h * 3 / 25) as i32;
            for (cx, cy) in [
                (inset_x, inset_y),
                (w as i32 - inset_x - card_w as i32, inset_y),
                (inset_x, h as i32 - inset_y - card_h as i32),
                (
                    w as i32 - inset_x - card_w as i32,
                    h as i32 - inset_y - card_h as i32,
                ),
            ] {
                let view = crate::view::BufferView::new(2, &card_data, card_geom.clone()).unwrap();
                let mut layer = SurfaceLayer::new(
                    view,
                    Rect::new(cx, cy, card_w, card_h),
                    ldp_core::geometry::Transform::Normal,
                    ColorDescription::srgb_sdr(),
                    1.0,
                    Region::from_rect(Rect::new(0, 0, card_w, card_h)),
                );
                layer.style = card_style;
                layers.push(layer);
            }
            let dock = crate::view::BufferView::new(3, &dock_data, dock_geom.clone()).unwrap();
            let mut bar = SurfaceLayer::new(
                dock,
                Rect::new(
                    (w - dock_w) as i32 / 2,
                    h as i32 - dock_h as i32 - (h as i32 / 45),
                    dock_w,
                    dock_h,
                ),
                ldp_core::geometry::Transform::Normal,
                ColorDescription::srgb_sdr(),
                1.0,
                Region::from_rect(Rect::new(0, 0, dock_w, dock_h)),
            );
            bar.style = dock_style;
            layers.push(bar);
            layers
        };

        // The warm renderer: three frames through one instance.
        let mut warm = SoftwareRenderer::new();
        let mut frames = Vec::new();
        let mut stats = Vec::new();
        let mut rebuilds = Vec::new();
        for _ in 0..3 {
            let layers = build_layers();
            warm.begin_frame(&output, &damage).unwrap();
            let s = warm.submit(&layers).unwrap();
            warm.end_frame().unwrap();
            frames.push(warm.readout().to_vec());
            stats.push(s);
            rebuilds.push(warm.shadow_rebuilds());
        }
        // Five distinct materials, built once — not three or fifteen.
        assert_eq!(rebuilds[0], 5, "the first frame builds the five materials");
        assert_eq!(
            rebuilds[1], 5,
            "the second frame builds nothing (no thrash)"
        );
        assert_eq!(rebuilds[2], 5, "the steady state stays flat");
        // The steady state is byte- and stat-stable.
        assert_eq!(frames[1], frames[2], "steady bytes");
        assert_eq!(stats[1], stats[2], "steady stats");
        assert_eq!(stats[2].layers, 6);
        assert_eq!(stats[2].layers_rendered, 6);
    }

    /// Phase 55's thrash oracle: a desktop of Liquid bands is a desktop
    /// of frost panes — five dressed bands over one backdrop — and the
    /// grown frost memo ([`SoftwareRenderer::frost_rebuilds`]) holds
    /// the whole working set resident: the first frame builds each
    /// pane once, every steady frame after serves the memo (the Phase
    /// 29 two-entry memo would have evicted-and-rebuilt four of the
    /// five every frame — the fixed-cap regression the shadow's Phase
    /// 30 budget closed, reborn).
    #[test]
    fn a_desktop_of_liquid_bands_does_not_thrash_the_frost_memo() {
        use crate::style::EffectTier;
        let (w, h) = (640u32, 480u32);
        let output = xrgb_output(w, h);
        let (paper_data, paper_geom) = crate::testkit::pattern_buffer(w, h, FourCC::XRGB8888);
        let (band_data, band_geom) = crate::testkit::pattern_buffer(240, 44, FourCC::ARGB8888);
        let damage = crate::testkit::full_damage(&output);
        // The Sheet material at Medium (the chrome material's own
        // backdrop tier): a real blur, a real veil — a real pane.
        let style = EffectTier::Medium.translucent_style();
        let build_layers = || {
            let mut layers: Vec<SurfaceLayer<'_>> = Vec::with_capacity(6);
            let paper = crate::view::BufferView::new(1, &paper_data, paper_geom.clone()).unwrap();
            let mut under = SurfaceLayer::new(
                paper,
                Rect::new(0, 0, w, h),
                ldp_core::geometry::Transform::Normal,
                ColorDescription::srgb_sdr(),
                1.0,
                Region::from_rect(Rect::new(0, 0, w, h)),
            );
            under.style = crate::style::LayerStyle::default();
            layers.push(under);
            // Five band-shaped panes at distinct placements (distinct
            // dests, distinct memo entries — the desktop of windows).
            for i in 0..5i32 {
                let view = crate::view::BufferView::new(2, &band_data, band_geom.clone()).unwrap();
                let mut layer = SurfaceLayer::new(
                    view,
                    Rect::new(24 + i * 60, 32 + i * 88, 240, 44),
                    ldp_core::geometry::Transform::Normal,
                    ColorDescription::srgb_sdr(),
                    1.0,
                    Region::from_rect(Rect::new(0, 0, 240, 44)),
                );
                layer.style = style;
                layers.push(layer);
            }
            layers
        };
        // Three frames through one renderer; the counter must rise by
        // exactly five on the first and never again.
        let mut r = SoftwareRenderer::new();
        let mut frames = Vec::new();
        let mut rebuilds = Vec::new();
        for _ in 0..3 {
            let layers = build_layers();
            r.begin_frame(&output, &damage).unwrap();
            r.submit(&layers).unwrap();
            r.end_frame().unwrap();
            frames.push(r.readout().to_vec());
            rebuilds.push(r.frost_rebuilds());
        }
        assert_eq!(rebuilds[0], 5, "the first frame builds the five panes");
        assert_eq!(
            rebuilds[1], 5,
            "the second frame builds nothing (no thrash)"
        );
        assert_eq!(rebuilds[2], 5, "the steady state stays flat");
        assert_eq!(frames[1], frames[2], "steady bytes");
    }
}
