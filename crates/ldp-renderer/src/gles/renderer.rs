//! The GL backend of the [`Renderer`] contract.
//!
//! [`GlesRenderer`] owns one composite program and one persistent
//! render target per output size (target content survives frames —
//! the damage-clip contract), and drives whatever
//! [`GlesApi`] implementation sits below it:
//! the reference evaluator in CI, the real EGL+GLESv2 backend on
//! hardware. Every decision the renderer makes — which damage rects
//! get a pass, which layers a pass skips, how pixels swizzle on
//! upload and readback — is its own pure logic, pinned by the golden
//! stream and the equivalence corpus.

use ldp_core::buffer::FourCC;
use ldp_core::geometry::{Rect, Region, Transform};

use crate::composite::RenderStats;
use crate::effects::{
    corner_band_xs, rounded_coverage, scale_premul, EdgeMemo, FrostMemo, MaterialCache,
};
use crate::errors::RendererError;
use crate::gles::api::{GlesApi, GlesProgram, GlesTarget};
use crate::gles::{FRAGMENT_SRC, VERTEX_SRC};
use crate::layer::{OutputDesc, SurfaceLayer, OUTPUT_FORMATS};
use crate::software::CompletedFrame;
use crate::style::{BackdropParams, EdgeLightParams, ShadowParams};
use crate::Renderer;

/// The open frame's state.
struct OpenFrame {
    desc: OutputDesc,
    /// Damage rects clipped to the output bounds, in region order.
    rects: Vec<Rect>,
    stats: RenderStats,
}

/// One layer fully prepared for the GL stream (Phase 27): the
/// swizzled (and corner-folded) texture payload, plus the Liquid
/// materials that ride as their own draws. Phase 29: the shadow rides
/// as its *parameters* — the material's texture bytes come from the
/// renderer's memo, converted once per unique shadow.
struct Upload {
    /// Premultiplied RGBA pixels — coverage-folded at the corners when
    /// styled (the software path folds the identical coverage inside
    /// its blend, the byte-equality anchor). EMPTY for the zero-copy
    /// arm (`egl_image` carries the layer). Buffer-resolved (Phase 33):
    /// the payload is the *source* pixels at `tex_w` × `tex_h`; a
    /// scaled placement scales in the draw (the GPU's sampler, the
    /// evaluator's continuous-nearest rule — the same math the
    /// software path's `Mapping::Scaled` runs).
    rgba: Vec<u8>,
    /// The texture's width in pixels (the buffer's, not the dest's).
    tex_w: u32,
    /// The texture's height in pixels.
    tex_h: u32,
    /// The imported EGLImage handle (the DMA-BUF zero-copy arm,
    /// Phase 31): the pixels never cross the CPU — the stream binds
    /// the image as a texture directly.
    egl_image: Option<u64>,
    /// Output-space placement.
    dest: Rect,
    /// Quantized opacity.
    opacity_q: u8,
    /// Whether the source format carries no alpha (opaque writes).
    opaque_format: bool,
    /// The sanitized corner radius (`0` = plain).
    corner_radius: u32,
    /// The shadow parameters (the material is served from the memo at
    /// stream time).
    shadow: Option<ShadowParams>,
    /// The frosted-backdrop parameters (the material is computed from
    /// the live target at draw order).
    frost: Option<BackdropParams>,
    /// The luminous hairline's parameters (Phase 40): the ring is
    /// memoized imagery — served from the edge memo at stream time
    /// and drawn *after* the ink quad (the stroke reads over the
    /// layer's own pixels).
    edge: Option<EdgeLightParams>,
}

impl Upload {
    /// Whether this layer carries any Liquid styling.
    fn styled(&self) -> bool {
        self.corner_radius > 0 || self.shadow.is_some() || self.frost.is_some()
    }
}

/// Canonical premultiplied words → tightly packed RGBA bytes (the
/// texture-upload layout the seam mandates), into a retained buffer.
fn words_to_rgba_into(words: &[u32], out: &mut Vec<u8>) {
    out.clear();
    out.reserve(words.len() * 4);
    for &w in words {
        out.extend_from_slice(&crate::effects::unpack_canonical(w));
    }
}

/// Extract a destination-sized region of a full-target RGBA readback
/// as canonical premultiplied words, into a retained buffer; pixels
/// outside the target read as transparent (the software snapshot's
/// honest boundary).
fn extract_region_into(rgba: &[u8], target_w: u32, target_h: u32, dest: Rect, out: &mut Vec<u32>) {
    out.clear();
    out.resize((dest.w as usize) * (dest.h as usize), 0);
    let rw = dest.w as usize;
    for (y, out_row) in (0..dest.h as i32).zip(out.chunks_mut(rw)) {
        let sy = dest.y + y;
        if sy < 0 || sy >= target_h as i32 {
            continue;
        }
        for (x, slot) in (0..dest.w as i32).zip(out_row.iter_mut()) {
            let sx = dest.x + x;
            if sx < 0 || sx >= target_w as i32 {
                continue;
            }
            let i = (sy as usize * target_w as usize + sx as usize) * 4;
            *slot = crate::effects::pack_canonical(rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]);
        }
    }
}

/// Merge overlapping rects into a disjoint set of bounding boxes:
/// one survivor per connected component of the overlap relation,
/// placed at the component's earliest input position (a survivor
/// never moves ahead of a rect that preceded it). Empty inputs drop.
///
/// The merge is union-find over the pairwise overlap relation — every
/// pair is examined exactly once, so the worst case is one pass of
/// O(n²) cheap comparisons with no restarts. The historical
/// implementation restarted its entire double scan after every merge
/// (O(n³) intersect tests): a legal 4096-rect mutually-overlapping
/// `surface.damage` payload — one request — parked the GL path's
/// `begin_frame` for minutes of intersect tests, a per-frame stall a
/// client could author by hand. The union-find form computes the same
/// fixpoint: bounding boxes are associative and commutative, so every
/// overlap component collapses to its own bounding box whatever the
/// merge order — the result is canonical, and the cost is one a
/// client cannot amplify by overlap topology.
fn disjoint_rects(rects: Vec<Rect>) -> Vec<Rect> {
    // Union-find with the component's *earliest* index as the root:
    // the survivor keeps the position of the first rect of its group.
    // Path halving inside `find` keeps the trees flat without a second
    // pass.
    fn find(parent: &mut [u32], mut x: u32) -> u32 {
        while parent[x as usize] != x {
            let grandparent = parent[parent[x as usize] as usize];
            parent[x as usize] = grandparent;
            x = grandparent;
        }
        x
    }
    let n = rects.len();
    if n <= 1 {
        return rects;
    }
    let mut parent: Vec<u32> = (0..n as u32).collect();
    for i in 0..n {
        for j in (i + 1)..n {
            if rects[i].intersect(rects[j]).is_some() {
                let (a, b) = (find(&mut parent, i as u32), find(&mut parent, j as u32));
                if a != b {
                    // The smaller root wins, so the survivor's position
                    // is the component's minimum index.
                    let (lo, hi) = if a < b { (a, b) } else { (b, a) };
                    parent[hi as usize] = lo;
                }
            }
        }
    }
    // One bounding box per component, folded in input order.
    let mut boxes: Vec<Option<Rect>> = vec![None; n];
    for (i, r) in rects.iter().enumerate() {
        let root = find(&mut parent, i as u32) as usize;
        boxes[root] = Some(match boxes[root] {
            Some(acc) => acc.union(*r),
            None => *r,
        });
    }
    boxes.into_iter().flatten().collect()
}

/// The GL renderer: one program, one target per size, damage-driven
/// passes.
///
/// Construction is fallible through the first use: the program links
/// lazily at the first `begin_frame` (so a renderer built on a dead
/// backend fails on the first frame, typed, instead of at
/// construction — callers that want eager validation render a 1x1
/// frame).
pub struct GlesRenderer {
    api: Box<dyn GlesApi>,
    program: Option<GlesProgram>,
    /// The persistent target: `(handle, width, height)`.
    target: Option<(GlesTarget, u32, u32)>,
    /// The active target's output origin (its identity).
    target_origin: (i32, i32),
    /// The other outputs' parked targets (Phase 31): a shared renderer
    /// alternates between the outputs of a multi-output desktop, and
    /// each output's target content (its undamaged regions) must
    /// survive the alternation — parked by geometry (origin + size,
    /// Phase 37: origins stop being unique the moment two displays
    /// overlap, which is what a mirror is), a small LRU. A renderer
    /// that only ever serves one output parks nothing.
    parked_targets: Vec<((i32, i32), GlesTarget, u32, u32)>,
    open: Option<OpenFrame>,
    /// The last completed frame's readout, top-down words.
    readout: Vec<u32>,
    /// The retained readback buffer (Phase 29): one allocation per
    /// output size, not per frosted layer per frame.
    readback_bytes: Vec<u8>,
    /// The styled path's retained state (Phase 29): the shadow memo
    /// (texture bytes converted once per unique shadow) and the frost
    /// memo (rebuilt only when the readback's words actually changed).
    shadows: MaterialCache,
    frost: FrostMemo,
    /// The edge-light material memo (Phase 40): one build per unique
    /// hairline, the texture bytes converted once.
    edges: EdgeMemo,
    /// The frost pipeline's retained buffers (snapshot words, the
    /// unfolded material working copy, the texture payload).
    frost_saved: Vec<u32>,
    frost_words: Vec<u32>,
    frost_rgba: Vec<u8>,
}

impl core::fmt::Debug for GlesRenderer {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("GlesRenderer")
            .field("program_linked", &self.program.is_some())
            .field("target", &self.target.map(|(_, w, h)| (w, h)))
            .field("frame_open", &self.open.is_some())
            .field("readout_words", &self.readout.len())
            .finish_non_exhaustive()
    }
}

impl GlesRenderer {
    /// Build over any backend (the reference evaluator in tests, the
    /// dlopen'd EGL+GLESv2 backend on hardware).
    #[must_use]
    pub fn new(api: Box<dyn GlesApi>) -> Self {
        Self {
            api,
            program: None,
            target: None,
            target_origin: (0, 0),
            parked_targets: Vec::new(),
            open: None,
            readout: Vec::new(),
            readback_bytes: Vec::new(),
            shadows: MaterialCache::default(),
            frost: FrostMemo::default(),
            edges: EdgeMemo::default(),
            frost_saved: Vec::new(),
            frost_words: Vec::new(),
            frost_rgba: Vec::new(),
        }
    }

    /// The last completed frame's pixels: premultiplied ARGB words,
    /// top-down, one per output pixel (empty before the first frame).
    ///
    /// Mirrors `SoftwareRenderer::readout` — the compositor's scanout
    /// copy treats both backends identically.
    #[must_use]
    pub fn readout(&self) -> &[u32] {
        &self.readout
    }

    /// Link the composite program (once).
    fn ensure_program(&mut self) -> Result<GlesProgram, RendererError> {
        if let Some(p) = self.program {
            return Ok(p);
        }
        let p = self
            .api
            .create_program(VERTEX_SRC, FRAGMENT_SRC)
            .map_err(RendererError::Gles)?;
        self.program = Some(p);
        Ok(p)
    }

    /// Ensure the persistent target matches `(origin, width, height)`
    /// — the output's identity: recreating it on a size change, and
    /// parking/restoring on an output switch (Phase 31: each output of
    /// a multi-output desktop keeps its own target, so its undamaged
    /// content survives the renderer's alternation). The parking key
    /// is the *geometry* — origin and size together (Phase 37: the
    /// origin alone is not unique; a mirrored desktop places every
    /// output at the same origin, so same-size mirrors share one
    /// target — they show identical content — while a different-size
    /// mirror keeps its own, instead of destroying and recreating a
    /// target on every alternation and losing every undamaged pixel).
    fn ensure_target(
        &mut self,
        origin: (i32, i32),
        width: u32,
        height: u32,
    ) -> Result<GlesTarget, RendererError> {
        if let Some((handle, w, h)) = self.target {
            if w == width && h == height && self.target_origin == origin {
                return Ok(handle);
            }
            // Park the outgoing target under its own geometry.
            let stale = self.target.take().expect("checked above");
            let stale_origin = self.target_origin;
            if let Some(pos) = self
                .parked_targets
                .iter()
                .position(|(o, _, pw, ph)| *o == stale_origin && *pw == w && *ph == h)
            {
                let (_, old, _, _) = self.parked_targets.remove(pos);
                self.api.destroy_target(old).map_err(RendererError::Gles)?;
            }
            self.parked_targets.insert(0, (stale_origin, stale.0, w, h));
            if self.parked_targets.len() > 3 {
                if let Some((_, old, _, _)) = self.parked_targets.pop() {
                    self.api.destroy_target(old).map_err(RendererError::Gles)?;
                }
            }
        }
        // Restore the incoming output's parked target, if any — by
        // geometry: a same-origin target of a different size is
        // another display's, never this one's.
        if let Some(pos) = self
            .parked_targets
            .iter()
            .position(|(o, _, pw, ph)| *o == origin && *pw == width && *ph == height)
        {
            let (_, handle, w, h) = self.parked_targets.remove(pos);
            debug_assert_eq!((w, h), (width, height), "the geometry key matched");
            self.target = Some((handle, width, height));
            self.target_origin = origin;
            return Ok(handle);
        }
        let handle = self
            .api
            .create_target(width, height)
            .map_err(RendererError::Gles)?;
        self.target = Some((handle, width, height));
        self.target_origin = origin;
        Ok(handle)
    }

    /// Clip `rects` to the output bounds, drop empties, and **merge
    /// overlapping rects into disjoint bounding boxes** (order stable).
    ///
    /// The disjointness is the GL stream's correctness contract: every
    /// pass draws a layer once per damage rect, so two rects sharing a
    /// pixel would double-composite it (a translucent layer darkening
    /// itself twice — the software path is immune, its row index
    /// merges the spans). The merge's bounding boxes may repaint a few
    /// never-damaged pixels — cost, never wrongness: the whole stack
    /// re-renders there over a fresh clear.
    fn clip_damage(desc: &OutputDesc, damage: &Region) -> Vec<Rect> {
        let bounds = Rect::new(0, 0, desc.width, desc.height);
        let clipped: Vec<Rect> = damage.iter().filter_map(|r| r.intersect(bounds)).collect();
        disjoint_rects(clipped)
    }

    /// The Liquid stream (Phase 27): layer-major — every layer visits
    /// every damage rect in its own passes, so the frost readback sees
    /// exactly the backdrop state the software path snapshots (all layers
    /// below, drawn over all damage). Per layer: the frost material first
    /// (the snapshot order the software path uses), then per rect the
    /// shadow, the frost, the ink, and — Phase 40 — the luminous
    /// hairline over the ink (the material edge).
    ///
    /// Phase 29: the readback buffer is retained (one allocation per
    /// output size, not one per frosted layer per frame), the frost
    /// material comes from the memo (rebuilt only when the readback's
    /// words actually changed — the steady state skips the blur
    /// entirely), and the shadow's texture bytes are the memo's
    /// once-per-unique-shadow conversion.
    // The four draws are one narrative — the stream's order IS the
    // pixel semantics — splitting them would hide it.
    #[allow(clippy::too_many_lines)]
    fn stream_styled(
        &mut self,
        target: GlesTarget,
        out_w: u32,
        out_h: u32,
        rects: &[Rect],
        uploads: &[Upload],
    ) -> Result<(), RendererError> {
        for u in uploads {
            // The frost material (the memo's unfolded words, corner-folded
            // into the retained working copy, converted into the retained
            // texture payload).
            let mut frost_tex: Option<(&[u8], Rect)> = None;
            if let Some(params) = &u.frost {
                let need = (out_w as usize) * (out_h as usize) * 4;
                if self.readback_bytes.len() != need {
                    self.readback_bytes.resize(need, 0);
                }
                self.api
                    .readback(target, &mut self.readback_bytes)
                    .map_err(RendererError::Gles)?;
                extract_region_into(
                    &self.readback_bytes,
                    out_w,
                    out_h,
                    u.dest,
                    &mut self.frost_saved,
                );
                let material = self.frost.get_or_build(u.dest, params, &self.frost_saved);
                self.frost_words.clear();
                self.frost_words.extend_from_slice(material);
                crate::effects::apply_corner_fold(
                    &mut self.frost_words,
                    u.dest.w,
                    u.dest.h,
                    u.corner_radius,
                );
                words_to_rgba_into(&self.frost_words, &mut self.frost_rgba);
                frost_tex = Some((&self.frost_rgba, u.dest));
            }
            // The shadow material from the memo (texture bytes converted
            // once per unique shadow).
            let shadow_tex: Option<(&[u8], Rect)> = match &u.shadow {
                Some(sh) => Some(self.shadows.get_or_build_rgba(sh, u.dest, u.corner_radius)),
                None => None,
            };
            // The hairline's ring imagery from the memo (Phase 40):
            // texture bytes converted once per unique ring, drawn after
            // the ink — the material edge over the layer's own pixels.
            // The ring covers the destination exactly (its own rect —
            // no padded extent like the shadow's silhouette box).
            let edge_tex: Option<(&[u8], Rect)> = match &u.edge {
                Some(light) => {
                    let rgba = self
                        .edges
                        .get_or_build_rgba(u.dest, u.corner_radius, *light);
                    Some((rgba, u.dest))
                }
                None => None,
            };
            for rect in rects {
                self.api
                    .begin_pass(target, Some(*rect))
                    .map_err(RendererError::Gles)?;
                // The software pass order, replayed: the frosted material
                // (the backdrop's replacement), then the shadow (its
                // fringe lands in the corner cutouts over the backdrop),
                // then the ink itself.
                if let Some((frost_rgba, frect)) = &frost_tex {
                    if frect.intersect(*rect).is_some() {
                        let tex = self
                            .api
                            .create_texture(frect.w, frect.h, frost_rgba)
                            .map_err(RendererError::Gles)?;
                        self.api
                            .draw_layer(tex, *frect, 255)
                            .map_err(RendererError::Gles)?;
                        self.api.destroy_texture(tex).map_err(RendererError::Gles)?;
                    }
                }
                if let Some((shadow_rgba, srect)) = &shadow_tex {
                    if srect.intersect(*rect).is_some() {
                        let tex = self
                            .api
                            .create_texture(srect.w, srect.h, shadow_rgba)
                            .map_err(RendererError::Gles)?;
                        self.api
                            .draw_layer(tex, *srect, 255)
                            .map_err(RendererError::Gles)?;
                        self.api.destroy_texture(tex).map_err(RendererError::Gles)?;
                    }
                }
                if u.dest.intersect(*rect).is_some() {
                    let tex = match u.egl_image {
                        // The zero-copy arm: bind the imported image —
                        // no CPU payload ever flows.
                        Some(image) => self
                            .api
                            .create_texture_from_egl_image(image)
                            .map_err(RendererError::Gles)?,
                        None => self
                            .api
                            .create_texture(u.tex_w, u.tex_h, &u.rgba)
                            .map_err(RendererError::Gles)?,
                    };
                    self.api
                        .draw_layer(tex, u.dest, u.opacity_q)
                        .map_err(RendererError::Gles)?;
                    self.api.destroy_texture(tex).map_err(RendererError::Gles)?;
                }
                // The luminous hairline (Phase 40): the ring's own
                // textured quad, straight-over at full opacity — the
                // same write path the shadow and frost materials take,
                // so the software path's `apply_material` of the
                // identical words lands on identical bytes.
                if let Some((edge_rgba, erect)) = &edge_tex {
                    if erect.intersect(*rect).is_some() {
                        let tex = self
                            .api
                            .create_texture(erect.w, erect.h, edge_rgba)
                            .map_err(RendererError::Gles)?;
                        self.api
                            .draw_layer(tex, *erect, 255)
                            .map_err(RendererError::Gles)?;
                        self.api.destroy_texture(tex).map_err(RendererError::Gles)?;
                    }
                }
                self.api.end_pass().map_err(RendererError::Gles)?;
            }
        }
        Ok(())
    }
}

/// Validate, decode and style one layer into its upload record
/// (pure — no GL calls, so a malformed layer fails the submit before
/// any pass opens).
///
/// Phase 29: the swizzle is a per-pixel word operation (the R/B field
/// swap and the forced alpha are one mask-and-shift each) and the
/// corner fold is band-limited (`corner_band_xs`) — byte-equal to the
/// Phase 27 per-pixel loops (the equivalence suites pin it).
///
/// Phase 33: the GL path composites **every format the software path
/// composites** — the 32-bit RGB family keeps the fast word-swizzle
/// arm, and YUV-family / RGB888 / RGB565 layers decode through the
/// software path's own sampler (`sample_premul_u8`, the identical
/// fetch + matrix + quantize), so the decoded upload is byte-equal
/// to what the CPU renderer would blend by construction. Scaled
/// placements (dest ≠ buffer extent) upload the *source* pixels and
/// let the draw scale them — the GPU's NEAREST sampler and the
/// evaluator's continuous-nearest rule are the same math the software
/// path's `Mapping::Scaled` runs.
fn build_upload(layer: &SurfaceLayer<'_>) -> Result<Upload, RendererError> {
    let geometry = layer.buffer.geometry();
    let (bw, bh) = (geometry.width(), geometry.height());
    // The zero-copy arm (Phase 31): an imported EGLImage layer never
    // touches the CPU payload — the validation is placement-only (the
    // image's own geometry is the EGL side's contract), and the
    // stream binds the image directly.
    if let Some(image) = layer.egl_image {
        return Ok(Upload {
            rgba: Vec::new(),
            tex_w: bw,
            tex_h: bh,
            egl_image: Some(image),
            dest: layer.dest,
            opacity_q: layer.opacity_q(),
            opaque_format: false,
            corner_radius: 0,
            shadow: None,
            frost: None,
            edge: None,
        });
    }
    if layer.transform != Transform::Normal {
        return Err(RendererError::UnsupportedLayer {
            detail: format!(
                "the GL v1 path composites Transform::Normal, not {:?}",
                layer.transform
            ),
        });
    }
    // Phase 38: the GL v1 path composites `Pass` tone policies only —
    // its encoded-space stream cannot carry the luminance rolloff, and
    // silently ignoring the negotiated ceiling would render the wrong
    // ink. The typed refusal routes the frame to the software arm
    // (the compositor's mixed-mode fallback, the documented boundary).
    if !layer.tone.is_pass() {
        return Err(RendererError::UnsupportedLayer {
            detail: "the GL v1 path composites Pass tone policies — the \
                     luminance tail rides the software compositor"
                .to_owned(),
        });
    }
    let format = geometry.format();
    let style = layer.style.sanitized(bw, bh);
    let mut rgba = Vec::with_capacity((bw * bh * 4) as usize);
    if OUTPUT_FORMATS.contains(&format) {
        // The fast arm: per-word swizzle (the Phase 24 bytes).
        //
        // ARGB/XRGB words are B,G,R,A bytes (swap to R,G,B,A);
        // ABGR/XBGR words are R,G,B,A already. X-family alpha is
        // garbage by definition — forced opaque.
        let bytes_bgra = matches!(format, FourCC::XRGB8888 | FourCC::ARGB8888);
        let force_opaque = matches!(format, FourCC::XRGB8888 | FourCC::XBGR8888);
        let layout = geometry.planes()[0];
        let data = layer.buffer.data();
        for row in 0..bh {
            let line = layout.offset as usize + layout.stride as usize * row as usize;
            for col in 0..bw {
                let s = line + col as usize * 4;
                let word = u32::from_le_bytes([data[s], data[s + 1], data[s + 2], data[s + 3]]);
                // The R/B field swap for the BGRA-order sources; the
                // alpha byte forced for the X family.
                let mut out_word = if bytes_bgra {
                    (word & 0xFF00_FF00)
                        | ((word & 0x00FF_0000) >> 16)
                        | ((word & 0x0000_00FF) << 16)
                } else {
                    word
                };
                if force_opaque {
                    out_word |= 0xFF00_0000;
                }
                rgba.extend_from_slice(&out_word.to_le_bytes());
            }
        }
    } else {
        // The general arm (Phase 33): decode through the software
        // path's own sampler — YCbCr coefficients derived from the
        // layer's primaries, the encoded-range expansion, the same
        // quantize — so a YUV upload is what the CPU renderer would
        // have blended, pixel for pixel.
        let coeffs = crate::yuv::ycbcr_coefficients(layer.color.primaries);
        let range = layer.color.range;
        for row in 0..bh {
            for col in 0..bw {
                let px = crate::sample::sample_premul_u8(&layer.buffer, col, row, coeffs, range);
                rgba.extend_from_slice(&px);
            }
        }
    }
    // The Liquid corner fold: source premultiplied channels scale by
    // the coverage before the blend — the identical `scale_premul` the
    // software path applies inside its blend (the byte-equality
    // anchor), band-limited to the corner arcs (merged — the bands
    // overlap on narrow rects, and every column must fold exactly once).
    fold_rgba_corners(&mut rgba, bw, bh, style.corner_radius);
    Ok(Upload {
        rgba,
        tex_w: bw,
        tex_h: bh,
        dest: layer.dest,
        egl_image: None,
        opacity_q: layer.opacity_q(),
        opaque_format: !crate::sample::format_has_alpha(format),
        corner_radius: style.corner_radius,
        shadow: style.shadow,
        frost: style.backdrop,
        edge: style.edge_light,
    })
}

/// Fold the rounded-corner coverage into an RGBA-byte payload
/// (`w` × `h`): every pixel outside the corner bands keeps its value
/// (coverage 255 — the identity), band pixels scale by their
/// antialiased coverage (coverage 0 zeroes the pixel).
fn fold_rgba_corners(rgba: &mut [u8], w: u32, h: u32, radius: u32) {
    if radius == 0 || w == 0 || h == 0 {
        return;
    }
    let rect = Rect::new(0, 0, w, h);
    for y in 0..h as i32 {
        let band = corner_band_xs(rect, radius, y);
        if band.is_empty() {
            continue;
        }
        for (x0, x1) in band.merged() {
            for x in x0.max(0)..x1.min(w as i32) {
                let cov = rounded_coverage(x, y, rect, radius);
                if cov == 255 {
                    continue;
                }
                let i = (y as usize * w as usize + x as usize) * 4;
                let px = [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]];
                let scaled = if cov == 0 {
                    [0u8; 4]
                } else {
                    let kept = scale_premul(px, cov);
                    [kept[0], kept[1], kept[2], kept[3]]
                };
                rgba[i..i + 4].copy_from_slice(&scaled);
            }
        }
    }
}

/// The plain stream (Phase 24, the goldens pin it): per damage rect,
/// layers back-to-front in one pass each.
fn stream_plain(
    api: &mut dyn GlesApi,
    target: GlesTarget,
    rects: &[Rect],
    uploads: &[Upload],
) -> Result<(), RendererError> {
    for rect in rects {
        api.begin_pass(target, Some(*rect))
            .map_err(RendererError::Gles)?;
        for u in uploads {
            // Layers that miss this damage rect are skipped — the
            // decision the stream golden pins.
            if u.dest.intersect(*rect).is_none() {
                continue;
            }
            // The zero-copy arm (Phase 31): an imported EGLImage binds
            // directly — no CPU payload ever flows (the empty `rgba`
            // is the layer's whole CPU-side truth; the stream's
            // `CreateTextureFromImage` is the proof).
            let tex = match u.egl_image {
                Some(image) => api
                    .create_texture_from_egl_image(image)
                    .map_err(RendererError::Gles)?,
                None => api
                    .create_texture(u.tex_w, u.tex_h, &u.rgba)
                    .map_err(RendererError::Gles)?,
            };
            api.draw_layer(tex, u.dest, u.opacity_q)
                .map_err(RendererError::Gles)?;
            api.destroy_texture(tex).map_err(RendererError::Gles)?;
        }
        api.end_pass().map_err(RendererError::Gles)?;
    }
    Ok(())
}

impl Renderer for GlesRenderer {
    fn backend(&self) -> &'static str {
        "gles"
    }

    fn begin_frame(&mut self, output: &OutputDesc, damage: &Region) -> Result<(), RendererError> {
        if self.open.is_some() {
            return Err(RendererError::NoFrameInProgress);
        }
        // Validate exactly like the software path (shared constructors),
        // keeping the output's origin (the multi-output identity).
        let desc = OutputDesc::with_origin(
            output.width,
            output.height,
            output.format,
            output.color,
            output.origin,
        )?;
        // Phase 30: the shadow cache's budget scales with the output
        // (twice its pixels, floored at the phone-era 6 Mi words) —
        // the same thrash fix the software path carries; a large
        // output's shadow working set fits without eviction.
        let pixels = desc.width as usize * desc.height as usize;
        self.shadows.set_budget_words(pixels.saturating_mul(2));
        self.ensure_program()?;
        self.ensure_target(desc.origin, desc.width, desc.height)?;
        let rects = Self::clip_damage(&desc, damage);
        self.open = Some(OpenFrame {
            desc,
            rects,
            stats: RenderStats::default(),
        });
        Ok(())
    }

    fn submit(&mut self, layers: &[SurfaceLayer<'_>]) -> Result<RenderStats, RendererError> {
        if self.open.is_none() {
            return Err(RendererError::NoFrameInProgress);
        }
        // Pass 1 (pure): validate and prepare every layer up front, so
        // a malformed layer fails the submit before any pass opens.
        let mut uploads = Vec::with_capacity(layers.len());
        for layer in layers {
            uploads.push(build_upload(layer)?);
        }
        let target = self.target.expect("begin_frame ensured the target").0;
        let (frame_rects, out_w, out_h) = {
            let frame = self.open.as_ref().expect("checked above");
            (frame.rects.clone(), frame.desc.width, frame.desc.height)
        };
        // Pass 2: the draw stream. Plain frames keep the Phase 24
        // shape (the goldens pin it); styled frames take the
        // layer-major Liquid stream whose frost readbacks snapshot the
        // same backdrop states the software path sees.
        if uploads.iter().any(Upload::styled) {
            self.stream_styled(target, out_w, out_h, &frame_rects, &uploads)?;
        } else {
            stream_plain(self.api.as_mut(), target, &frame_rects, &uploads)?;
        }
        // Statistics from geometry: coverage of each layer against the
        // frame's damage, classified opaque/blended. X-family layers
        // at full opacity write opaquely; everything else blends —
        // the GL path does not read back per-pixel stats, the class is
        // computed from coverage (documented). Styled layers count
        // their effect coverage through the same geometric lens.
        let frame = self.open.as_mut().expect("checked above");
        for u in &uploads {
            let covered = frame
                .rects
                .iter()
                .filter_map(|r| u.dest.intersect(*r))
                .map(|r| u64::from(r.w) * u64::from(r.h))
                .sum::<u64>();
            if covered == 0 {
                continue;
            }
            frame.stats.layers += 1;
            if u.opaque_format && u.opacity_q == 255 {
                frame.stats.pixels_opaque += covered;
            } else {
                frame.stats.pixels_blended += covered;
            }
            if let Some(sh) = &u.shadow {
                // The shadow's box (the material's own extent — the same
                // arithmetic `shadow_material` returns).
                let pad = sh.blur.saturating_mul(sh.passes);
                let srect = Rect::new(
                    u.dest.x + sh.offset.0 - pad as i32,
                    u.dest.y + sh.offset.1 - pad as i32,
                    u.dest.w + 2 * pad,
                    u.dest.h + 2 * pad,
                );
                frame.stats.pixels_effect += frame
                    .rects
                    .iter()
                    .filter_map(|r| srect.intersect(*r))
                    .map(|r| u64::from(r.w) * u64::from(r.h))
                    .sum::<u64>();
            }
            if u.frost.is_some() || u.corner_radius > 0 {
                frame.stats.pixels_effect += covered;
            }
        }
        Ok(frame.stats)
    }

    #[allow(clippy::many_single_char_names)] // the Renderer contract's own parameter names
    fn clear_damage(&mut self, r: u8, g: u8, b: u8, a: u8) -> Result<(), RendererError> {
        let Some(frame) = self.open.as_ref() else {
            return Err(RendererError::NoFrameInProgress);
        };
        let target = self.target.expect("begin_frame ensured the target").0;
        // The clear color is straight (the software contract); the GL
        // clear takes premultiplied — fold here.
        let q = u32::from(a);
        let mul255 = |v: u32| -> u32 { (v * q + 127) / 255 };
        let (pr, pg, pb) = (
            mul255(u32::from(r)) as u8,
            mul255(u32::from(g)) as u8,
            mul255(u32::from(b)) as u8,
        );
        for rect in frame.rects.clone() {
            self.api
                .begin_pass(target, Some(rect))
                .map_err(RendererError::Gles)?;
            self.api.clear(pr, pg, pb, a).map_err(RendererError::Gles)?;
            self.api.end_pass().map_err(RendererError::Gles)?;
        }
        if let Some(frame) = self.open.as_mut() {
            frame.stats.pixels_damaged += frame
                .rects
                .iter()
                .map(|r| u64::from(r.w) * u64::from(r.h))
                .sum::<u64>();
        }
        Ok(())
    }

    fn end_frame(&mut self) -> Result<CompletedFrame, RendererError> {
        let Some(frame) = self.open.take() else {
            return Err(RendererError::NoFrameInProgress);
        };
        let (target, width, height) = self.target.expect("begin_frame ensured the target");
        // Read back as RGBA and swizzle into the output's word layout
        // (the retained readback buffer — Phase 29: one allocation per
        // output size, not per frame).
        let need = (width as usize) * (height as usize) * 4;
        if self.readback_bytes.len() != need {
            self.readback_bytes.resize(need, 0);
        }
        let rgba = &mut self.readback_bytes;
        self.api
            .readback(target, rgba)
            .map_err(RendererError::Gles)?;
        let words_bgra = matches!(frame.desc.format, FourCC::XRGB8888 | FourCC::ARGB8888);
        let force_opaque = matches!(frame.desc.format, FourCC::XRGB8888 | FourCC::XBGR8888);
        let mut readout = Vec::with_capacity(rgba.len() / 4);
        for px in rgba.chunks_exact(4) {
            let (r, g, b, a) = (px[0], px[1], px[2], px[3]);
            let mut word = if words_bgra {
                u32::from(a) << 24 | u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b)
            } else {
                u32::from(a) << 24 | u32::from(b) << 16 | u32::from(g) << 8 | u32::from(r)
            };
            if force_opaque {
                word |= 0xFF00_0000;
            }
            readout.push(word);
        }
        self.readout = readout;
        Ok(CompletedFrame {
            stats: frame.stats,
            width: frame.desc.width,
            height: frame.desc.height,
            format: frame.desc.format,
        })
    }
}

impl Drop for GlesRenderer {
    fn drop(&mut self) {
        // Best-effort teardown: the backend's own Drop is the real
        // owner (context destruction); these calls only release the
        // renderer's objects early. Failures are ignored by policy.
        if let Some(program) = self.program.take() {
            let _ = self.api.destroy_program(program);
        }
        if let Some((target, _, _)) = self.target.take() {
            let _ = self.api.destroy_target(target);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::gles::mock::RefGles;

    use ldp_core::buffer::{BufferGeometry, Modifier, PlaneLayout};
    use ldp_core::color::ColorDescription;
    use ldp_core::geometry::Region;

    fn geometry(w: u32, h: u32, format: FourCC) -> BufferGeometry {
        let layout = [PlaneLayout {
            offset: 0,
            stride: w * 4,
        }];
        BufferGeometry::new(w, h, format, Modifier::LINEAR, &layout, (w * h * 4) as u64).unwrap()
    }

    /// One opaque XRGB layer, full-frame damage: byte-equal to the
    /// software renderer (the oracle's smallest case).
    #[test]
    fn one_opaque_layer_matches_software() {
        use crate::software::SoftwareRenderer;
        // 2x2 XRGB words with garbage X bytes (the sampler's trap).
        let words: Vec<u32> = vec![0x0044_3322, 0x0088_7766, 0x00CC_BBAA, 0x0055_4433];
        let data: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let geom = geometry(2, 2, FourCC::XRGB8888);
        let view = crate::view::BufferView::new(1, &data, geom).unwrap();
        let layer = SurfaceLayer::new(
            view,
            Rect::new(0, 0, 2, 2),
            Transform::Normal,
            ColorDescription::srgb_sdr(),
            1.0,
            Region::new(),
        );
        let output = OutputDesc::new(2, 2, FourCC::XRGB8888, ColorDescription::srgb_sdr()).unwrap();
        let damage = Region::from_rect(Rect::new(0, 0, 2, 2));

        let mut sw = SoftwareRenderer::new();
        sw.begin_frame(&output, &damage).unwrap();
        sw.clear_damage(0, 0, 0, 0xFF).unwrap();
        sw.submit(std::slice::from_ref(&layer)).unwrap();
        sw.end_frame().unwrap();

        let mut gl = GlesRenderer::new(Box::new(RefGles::new()));
        gl.begin_frame(&output, &damage).unwrap();
        gl.clear_damage(0, 0, 0, 0xFF).unwrap();
        gl.submit(&[layer]).unwrap();
        gl.end_frame().unwrap();

        assert_eq!(gl.readout(), sw.readout(), "the GL path matches software");
        // X forced opaque in the readout.
        assert_eq!(gl.readout()[0] >> 24, 0xFF);
    }
}
