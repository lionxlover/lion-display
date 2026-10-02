//! The plane session (Phase 34): the frame loop's hardware arm.
//!
//! Three pieces, all pure policy over the world's own state:
//!
//! * `import_walk` — the buffer-to-framebuffer registration: the
//!   pool's descriptor imports as a GEM handle, the buffer's
//!   validated geometry names the planes, `add_fb` mints the
//!   [`FbId`] the atomic commit references. On real hardware only
//!   PRIME/dma-buf descriptors import — an shm memfd refuses cleanly
//!   and the layer composites on the CPU (the cached refusal is the
//!   honest per-buffer truth, never a silent fallback). The mock
//!   imports any live descriptor, so the full orchestration — solve,
//!   commit, flip, release — is CI-proven end to end with real
//!   client buffers over the real wire.
//! * `layer_facts` — the solver's input for one composited layer:
//!   placement, buffer geometry, color, opaque coverage, the import
//!   walk's framebuffer, and the Liquid frost's backdrop dependency
//!   (a frosted material's truth is the composite — it can never
//!   ride a plane).
//! * `plane_commit` — the multi-plane atomic request: every
//!   assignment on with its zpos, the overlays that left the plan
//!   off (stale scanout would otherwise hover above the canvas),
//!   the canvas on the primary for split plans, and the flip flags
//!   the release gates and presentation feedback ride.
//!
//! The dock's own ink stays on the canvas in v1 (its pixels are
//! CPU-generated; delivering them to a plane needs the dumb-buffer
//! staging path — the named roadmap line). Client buffers carry the
//! doctrine: a fullscreen opaque client scans out untouched, and
//! overlay-eligible windows above it ride hardware the same way.

use std::collections::HashMap;
use std::os::fd::AsRawFd as _;

use ldp_core::buffer::Modifier;
use ldp_core::color::ColorDescription;
use ldp_core::geometry::{Rect, Region};
use ldp_display::atomic::AtomicRequest;
use ldp_display::backend::KmsBackend;
use ldp_display::commit::{CommitFlags, DstRect, SrcRect};
use ldp_display::fb::FbSpec;
use ldp_display::ids::{CrtcId, FbId, PlaneId};
use ldp_planes::{LayerFacts, OutputFacts, PlaneAssignment, ScanoutPlan};

use crate::scene::{ObjectKey, OutputSlot, World};
use crate::shm::{ShmBuffer, ShmPool};

/// One buffer's kernel framebuffer — the import walk.
///
/// The walk runs once per buffer object (the ledger caches both
/// successes and refusals): the pool's descriptor imports as a GEM
/// handle, the buffer's geometry becomes the `AddFB2` spec, and the
/// minted [`FbId`] is what a plane assignment references. A refused
/// import (a memfd on real hardware, a dead descriptor anywhere)
/// returns `None` — the layer composites, and the demotion ledger
/// names the reason.
///
/// # Panics
///
/// Never: the device errors are the typed `DisplayError`s the caller
/// reports through the frame path.
#[allow(clippy::missing_errors_doc)]
pub(crate) fn import_walk(
    device: &mut dyn KmsBackend,
    imports: &mut HashMap<ObjectKey, Option<FbId>>,
    key: ObjectKey,
    buffer: &ShmBuffer,
    pool: &ShmPool,
) -> Option<FbId> {
    if let Some(cached) = imports.get(&key) {
        return *cached;
    }
    let walked = (|| {
        let handle = device.import_gem(pool.as_fd().as_raw_fd()).ok()?;
        let geometry = buffer.geometry(pool.size());
        let spec = FbSpec::from_layout(
            buffer.width(),
            buffer.height(),
            buffer.format(),
            handle,
            geometry.planes(),
        );
        // Shm pools are linear by construction — the modifier the
        // AddFB2WithModifiers contract carries.
        device.add_fb(&spec, Some(Modifier::LINEAR)).ok()
    })();
    imports.insert(key, walked);
    walked
}

/// The solver's facts for one *client* layer of the frame's stack
/// (the dock's facts differ: CPU-generated ink, no framebuffer).
///
/// The shm doctrine rides inside: linear layouts only (the import
/// walk's contract above). `opacity` is the Phase-47 transitions
/// truth (1.0 when no motion serves): the layer's alpha — a fading
/// surface composites (the planes doctrine has no v1 alpha), exactly
/// what the renderer's own layer carries (the two read one host).
#[must_use]
pub(crate) fn client_layer_facts(
    dest: Rect,
    geometry: &ldp_core::buffer::BufferGeometry,
    node: &ldp_compositor::snapshot::SnapshotNode,
    opaque: Region,
    style: &ldp_renderer::LayerStyle,
    fb: Option<FbId>,
    opacity: f32,
) -> LayerFacts {
    LayerFacts {
        dest,
        width: geometry.width(),
        height: geometry.height(),
        format: geometry.format(),
        modifier: Modifier::LINEAR,
        transform: node.transform,
        color: node.color,
        opacity,
        opaque,
        fb,
        needs_backdrop: style.backdrop.is_some(),
        styled: style.corner_radius > 0 || style.shadow.is_some(),
        system: false,
    }
}

/// The output facts for one render target.
#[must_use]
pub(crate) fn output_facts(
    width: u32,
    height: u32,
    format: ldp_core::buffer::FourCC,
    color: ColorDescription,
) -> OutputFacts {
    OutputFacts {
        width,
        height,
        format,
        color,
    }
}

/// The multi-plane atomic commit for one output's frame.
///
/// * Every plan assignment goes **on** with its stacking zpos.
/// * Overlays the previous commit left on but the plan no longer
///   carries go **off** — stale scanout above the canvas is the bug
///   this forbids.
/// * The canvas rides the primary for split plans (the composite
///   group's framebuffer); for zero-composite plans the bottom
///   layer's own buffer is the primary's framebuffer and the canvas
///   is not touched at all.
///
/// # Panics
///
/// Never in-crate: the src/dst rect constructors reject zero extents,
/// which the solver never emits (an empty layer never reaches the
/// plan).
#[must_use]
pub(crate) fn plane_commit(
    slot: &OutputSlot,
    plan: &ScanoutPlan,
    canvas_fb: Option<FbId>,
    mode: (u32, u32),
) -> AtomicRequest {
    let mut request = AtomicRequest::new()
        .flag(CommitFlags::PAGE_FLIP_EVENT)
        .flag(CommitFlags::NONBLOCK);
    // The primary: the canvas (split plans) or the bottom layer's
    // buffer (zero-composite) — always on, always mode-sized.
    let (w, h) = mode;
    let primary_fb = plan
        .assignments
        .iter()
        .find(|a| a.role == ldp_planes::PlaneRole::Primary)
        .map(|a| a.fb)
        .or(canvas_fb);
    if let Some(fb) = primary_fb {
        request = request.plane_on(
            slot.plane,
            slot.crtc,
            fb,
            SrcRect::new(0, 0, w, h).expect("mode-sized source"),
            DstRect::new(0, 0, w, h).expect("mode-sized destination"),
        );
    }
    // The overlays this plan carries.
    let mut on: Vec<PlaneId> = Vec::with_capacity(plan.assignments.len());
    for assignment in &plan.assignments {
        if assignment.role == ldp_planes::PlaneRole::Primary {
            continue;
        }
        on.push(assignment.plane);
        request = apply_assignment(request, assignment, slot.crtc);
    }
    // The overlays that fell out of the plan: off.
    for plane in &slot.active_planes {
        if !on.contains(plane) && *plane != slot.plane {
            request = request.plane_off(*plane);
        }
    }
    request
}

/// One overlay assignment onto the request (the on + zpos pair the
/// stacking order demands).
fn apply_assignment(request: AtomicRequest, a: &PlaneAssignment, crtc: CrtcId) -> AtomicRequest {
    let src = SrcRect::new(a.src.0, a.src.1, a.src.2, a.src.3)
        .expect("the solver emits full-extent sources");
    let dst = DstRect::new(a.dst.0, a.dst.1, a.dst.2, a.dst.3)
        .expect("the solver emits positive placements");
    request
        .plane_on(a.plane, crtc, a.fb, src, dst)
        .plane_zpos(a.plane, a.zpos)
}

/// The session's plane-arm summary (the shutdown report line).
#[must_use]
pub(crate) fn session_report(world: &World) -> String {
    format!(
        "{} zero-composite frames, {} buffers imported",
        world.zero_pass_frames,
        world.imports.values().flatten().count()
    )
}

/// Blend the plane-assigned layers over a display model that already
/// holds the composited base, in zpos order — *what the panel's plane
/// blender shows*, the pixel oracle the testbench and the capture path
/// read (Phase 34's compose step, split from the base copy).
///
/// The blend is the KMS `PIXEL_BLEND_MODE = Pre-multiplied` semantics
/// through the renderer's own canonical sampler
/// ([`ldp_renderer::sample_premul_u8`]) and blend kernel
/// ([`ldp_renderer::over_premul`]) — the same math the composite path
/// would have produced, which is exactly why the oracle stays honest:
/// a plane-offloaded frame's display model equals the composite
/// frame's canvas (the equivalence the session tests pin).
///
/// The caller owns the base copy: the frame loop reuses the output
/// slot's own `Vec` (capacity persists across frames — one base
/// memcpy, no per-frame heap allocation), and the zero-composite pass
/// blends straight into its own owned base. The historical
/// `compose_display(base.to_vec())` allocated and copied a fresh full
/// canvas every frame — 8 MiB at 4K — to produce bytes the caller
/// already held.
///
/// The base may be stale (a zero-composite frame never touched the
/// canvas) — harmless by construction: the primary-role assignment
/// covers the output opaquely, so its blend replaces every word.
pub(crate) fn blend_planes(
    display: &mut [u32],
    width: u32,
    height: u32,
    stack: &[Option<ldp_renderer::SurfaceLayer<'_>>],
    plan: &ScanoutPlan,
) {
    // The assignments in stacking order (zpos ascending = the
    // hardware's blend order).
    let mut ordered: Vec<&PlaneAssignment> = plan.assignments.iter().collect();
    ordered.sort_by_key(|a| a.zpos);
    for assignment in ordered {
        let Some(Some(layer)) = stack.get(assignment.layer) else {
            continue;
        };
        blend_layer_over(
            display,
            width,
            height,
            layer,
            assignment.dst.0,
            assignment.dst.1,
        );
    }
}

/// Blend one plane-assigned layer over the display model at its
/// destination (1:1 — the solver never offloads scaled placements).
fn blend_layer_over(
    display: &mut [u32],
    width: u32,
    height: u32,
    layer: &ldp_renderer::SurfaceLayer<'_>,
    dst_x: i32,
    dst_y: i32,
) {
    let geometry = layer.buffer.geometry();
    let (bw, bh) = (geometry.width(), geometry.height());
    // Clip once, not per pixel: the row and column ranges that land
    // inside the destination are fixed for the whole blend, so the
    // inner loops run branch-free over exactly the visible span
    // (the historical per-pixel `x < 0 || x >= width` test cost a
    // predictable branch on every pixel of every offloaded layer).
    let row_lo = (-dst_y).max(0).min(bh as i32);
    let row_hi = (height as i32).saturating_sub(dst_y).clamp(0, bh as i32);
    let col_lo = (-dst_x).max(0).min(bw as i32);
    let col_hi = (width as i32).saturating_sub(dst_x).clamp(0, bw as i32);
    // Clamped non-negative by construction — u32 loop vars, cast once,
    // not per pixel.
    let (row_lo, row_hi) = (row_lo as u32, row_hi.max(row_lo) as u32);
    let (col_lo, col_hi) = (col_lo as u32, col_hi.max(col_lo) as u32);
    debug_assert!(display.len() >= width as usize * height as usize);
    // The word-copy fast path: an alpha-free 32-bit RGB layer at full
    // opacity replaces its destination outright — a per-row field
    // swizzle (the same transform the render path's upload arm runs),
    // so a fullscreen layer blends in milliseconds even in debug
    // builds where the per-pixel sampler would cost seconds.
    let format = geometry.format();
    let opaque_word_copy = layer.opacity >= 1.0
        && matches!(
            format,
            ldp_core::buffer::FourCC::XRGB8888 | ldp_core::buffer::FourCC::XBGR8888
        );
    if opaque_word_copy {
        let swap_rb = matches!(format, ldp_core::buffer::FourCC::XBGR8888);
        let layout = geometry.planes()[0];
        let data = layer.buffer.data();
        for row in row_lo..row_hi {
            let y = (dst_y + row as i32) as usize;
            let line = layout.offset as usize + layout.stride as usize * row as usize;
            for col in col_lo..col_hi {
                let x = (dst_x + col as i32) as usize;
                let s = line + col as usize * 4;
                let word = u32::from_le_bytes([data[s], data[s + 1], data[s + 2], data[s + 3]]);
                // The alpha-free families carry the canonical field
                // layout already (A-slot is the ignored byte); XBGR
                // swaps the R/B fields on the way in.
                let out = if swap_rb {
                    (word & 0xFF00_FF00)
                        | ((word & 0x00FF_0000) >> 16)
                        | ((word & 0x0000_00FF) << 16)
                } else {
                    word
                } | 0xFF00_0000;
                let di = y * width as usize + x;
                display[di] = out;
            }
        }
        return;
    }
    let coeffs = ldp_renderer::ycbcr_coefficients(layer.color.primaries);
    let range = layer.color.range;
    for row in row_lo..row_hi {
        let y = (dst_y + row as i32) as usize;
        for col in col_lo..col_hi {
            let x = (dst_x + col as i32) as usize;
            let di = y * width as usize + x;
            let dst_word = display[di];
            let src = ldp_renderer::sample_premul_u8(&layer.buffer, col, row, coeffs, range);
            let out = ldp_renderer::over_premul(src, ldp_renderer::unpack_canonical(dst_word), 255);
            display[di] = ldp_renderer::pack_canonical(out[0], out[1], out[2], out[3]);
        }
    }
}
