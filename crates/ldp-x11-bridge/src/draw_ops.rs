//! Drawing-side request handlers (opcodes 53–93 plus MIT-SHM).
//!
//! Everything that touches pixels lives here: pixmaps, graphics
//! contexts, the core drawing requests, `PutImage`/`GetImage`,
//! colormaps and cursors, and the MIT-SHM minor opcodes that route
//! image data through the host seam. Window drawing flows through the
//! server's draw-on-window path (GC clip + subwindow clip +
//! recomposite); pixmap drawing through its pixmap twin.

#![forbid(unsafe_code)]
// Handler signatures are uniform (`Result<(), OpErr>`) even where an
// individual handler cannot fail, and field names restate the wire's
// single-character geometry vocabulary.
#![allow(clippy::unnecessary_wraps, clippy::many_single_char_names)]

use ldp_core::geometry::Rect;

use crate::dispatch::{ClientId, DrawableKind, Server};
use crate::events::{err, XEvent};
use crate::gc::GcStore;
use crate::ops::{OpErr, Rd};

pub(crate) fn handle(
    s: &mut Server,
    client: ClientId,
    opcode: u8,
    data: u8,
    rd: &mut Rd<'_>,
) -> Result<(), OpErr> {
    match opcode {
        53 => create_pixmap(s, client, data, rd),
        54 => free_pixmap(s, client, rd),
        55 => create_gc(s, client, rd),
        56 => change_gc(s, client, rd),
        57 => copy_gc(s, client, rd),
        58 => set_dashes(s, client, rd),
        59 => set_clip_rectangles(s, client, data, rd),
        60 => free_gc(s, client, rd),
        61 => clear_area(s, client, data, rd),
        62 => copy_area(s, client, rd),
        63 => copy_plane(s, client, rd),
        64 => poly_point(s, client, data, rd),
        65 => poly_line(s, client, data, rd),
        66 => poly_segment(s, client, rd),
        67 => poly_rectangle(s, client, rd),
        68 => poly_arc(s, client, rd),
        69 => fill_poly(s, client, data, rd),
        70 => poly_fill_rectangle(s, client, rd),
        71 => poly_fill_arc(s, client, rd),
        72 => put_image(s, client, data, rd),
        73 => get_image(s, client, data, rd),
        74 => create_colormap(s, client, data, rd),
        75 => free_colormap(s, client, rd),
        76 => copy_colormap_and_free(s, client, rd),
        77 | 78 => install_colormap(s, client, rd),
        79 => list_installed_colormaps(s, client, rd),
        80 => alloc_color(s, client, rd),
        81 => alloc_named_color(s, client, rd),
        82 | 83 => alloc_color_cells(s, client, rd),
        84 => free_colors(s, client, rd),
        85 | 86 => store_colors(s, client, rd),
        87 => query_colors(s, client, rd),
        88 => lookup_color(s, client, rd),
        89 => create_cursor(s, client, rd),
        90 => create_glyph_cursor(s, client, rd),
        91 => free_cursor(s, client, rd),
        92 => recolor_cursor(s, client, rd),
        93 => query_best_size(s, client, data, rd),
        _ => Err(OpErr::X(err::IMPLEMENTATION, u32::from(opcode))),
    }
}

/// Resolve a drawable (the error event is emitted once by
/// `dispatch_request` from the returned `OpErr`).
fn drawable_or_err(
    s: &Server,
    _client: ClientId,
    id: u32,
    _major: u8,
) -> Result<DrawableKind, OpErr> {
    match s.drawable_kind(id) {
        Some(k) => Ok(k),
        None => Err(OpErr::X(err::DRAWABLE, id)),
    }
}

/// Fetch (a clone of) a GC (single error emission as above).
fn gc_or_err(s: &Server, _client: ClientId, id: u32, _major: u8) -> Result<crate::gc::Gc, OpErr> {
    s.gcs().get(id).cloned().ok_or(OpErr::X(err::GC, id))
}

/// Apply a CreateGC/ChangeGC value list, mapping decoder errors.
fn apply_gc_list(gc: &mut crate::gc::Gc, mask: u32, payload: &[u8]) -> Result<(), OpErr> {
    match crate::gc::apply_value_list(gc, mask, payload) {
        Ok(()) => Ok(()),
        Err(crate::gc::GcError::Short) => Err(OpErr::Short),
        Err(crate::gc::GcError::BadValue(_, v)) => Err(OpErr::X(err::VALUE, v)),
        Err(crate::gc::GcError::UnknownBits(m)) => Err(OpErr::X(err::VALUE, m)),
    }
}

/// Check the GC's unsupported state (BadImplementation at draw time).
fn check_supported(gc: &crate::gc::Gc, drawable: u32) -> Result<(), OpErr> {
    if gc.unsupported() {
        return Err(OpErr::X(err::IMPLEMENTATION, drawable));
    }
    Ok(())
}

/// Points in absolute coordinates from a coordinate-mode list.
fn absolute_points(mode: u8, pts: &[(i32, i32)]) -> Vec<(i32, i32)> {
    if mode == 0 {
        return pts.to_vec();
    }
    // Previous mode: each point relative to the one before it.
    let mut out = Vec::with_capacity(pts.len());
    let mut cur = (0i32, 0i32);
    for &p in pts {
        cur = (cur.0 + p.0, cur.1 + p.1);
        out.push(cur);
    }
    out
}

// ---- pixmaps ----

fn create_pixmap(
    s: &mut Server,
    client: ClientId,
    depth: u8,
    rd: &mut Rd<'_>,
) -> Result<(), OpErr> {
    let pid = rd.u32()?;
    let drawable = rd.u32()?;
    let width = u32::from(rd.u16()?);
    let height = u32::from(rd.u16()?);
    if depth != 24 {
        // The subset's single FORMAT is depth 24 / 32 bpp.
        return Err(OpErr::X(err::VALUE, u32::from(depth)));
    }
    if width == 0 || height == 0 || width > 0xffff || height > 0xffff {
        return Err(OpErr::X(err::VALUE, width.max(height)));
    }
    if s.pixmaps().contains_key(&pid) || s.tree().windows.contains_key(&pid) {
        return Err(OpErr::X(err::ID_CHOICE, pid));
    }
    drawable_or_err(s, client, drawable, 53)?;
    s.pixmaps_mut()
        .insert(pid, crate::render::Store::filled(width, height, 0));
    Ok(())
}

fn free_pixmap(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let pid = rd.u32()?;
    let _ = client;
    if s.pixmaps_mut().remove(&pid).is_none() {
        return Err(OpErr::X(err::PIXMAP, pid));
    }
    Ok(())
}

// ---- graphics contexts ----

fn create_gc(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let cid = rd.u32()?;
    let drawable = rd.u32()?;
    let mask = rd.u32()?;
    drawable_or_err(s, client, drawable, 55)?;
    if s.gcs().get(cid).is_some() {
        return Err(OpErr::X(err::ID_CHOICE, cid));
    }
    let mut gc = crate::gc::Gc::new(cid);
    apply_gc_list(&mut gc, mask, rd.rest())?;
    match s.gcs_mut().create(cid) {
        Ok(g) => *g = gc,
        Err(_) => return Err(OpErr::X(err::ID_CHOICE, cid)),
    }
    Ok(())
}

fn change_gc(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let cid = rd.u32()?;
    let mask = rd.u32()?;
    let Some(mut gc) = s.gcs().get(cid).cloned() else {
        return Err(OpErr::X(err::GC, cid));
    };
    apply_gc_list(&mut gc, mask, rd.rest())?;
    if let Some(g) = s.gcs_mut().get_mut(cid) {
        *g = gc;
    }
    let _ = client;
    Ok(())
}

fn copy_gc(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let src_gc = rd.u32()?;
    let dst_gc = rd.u32()?;
    let mask = rd.u32()?;
    let _ = client;
    let Some(source) = s.gcs().get(src_gc).cloned() else {
        return Err(OpErr::X(err::GC, src_gc));
    };
    let Some(dest) = s.gcs_mut().get_mut(dst_gc) else {
        return Err(OpErr::X(err::GC, dst_gc));
    };
    if mask >> 23 != 0 {
        return Err(OpErr::X(err::VALUE, mask));
    }
    GcStore::copy_fields(dest, &source, mask);
    Ok(())
}

fn set_dashes(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let cid = rd.u32()?;
    let dash_offset = rd.u16()?;
    let n = rd.u16()? as usize;
    let dashes = rd.rest().to_vec();
    if dashes.len() < n {
        return Err(OpErr::Short);
    }
    let dashes = dashes[..n].to_vec();
    let _ = client;
    if dashes.is_empty() {
        return Err(OpErr::X(err::VALUE, 0));
    }
    if dashes.contains(&0) {
        // A zero dash is a zero-length dash — illegal per the spec.
        return Err(OpErr::X(err::VALUE, 0));
    }
    let Some(gc) = s.gcs_mut().get_mut(cid) else {
        return Err(OpErr::X(err::GC, cid));
    };
    gc.dash_offset = u32::from(dash_offset);
    gc.dashes = dashes;
    Ok(())
}

fn set_clip_rectangles(
    s: &mut Server,
    client: ClientId,
    ordering: u8,
    rd: &mut Rd<'_>,
) -> Result<(), OpErr> {
    let cid = rd.u32()?;
    let clip_x = rd.i32()?;
    let clip_y = rd.i32()?;
    if ordering > 3 {
        return Err(OpErr::X(err::VALUE, u32::from(ordering)));
    }
    let rest = rd.rest();
    if rest.len() % 8 != 0 {
        return Err(OpErr::Short);
    }
    let mut rects = Vec::with_capacity(rest.len() / 8);
    for chunk in rest.chunks_exact(8) {
        let x = i32::from(u16::from_le_bytes([chunk[0], chunk[1]]) as i16);
        let y = i32::from(u16::from_le_bytes([chunk[2], chunk[3]]) as i16);
        let w = u32::from(u16::from_le_bytes([chunk[4], chunk[5]]));
        let h = u32::from(u16::from_le_bytes([chunk[6], chunk[7]]));
        rects.push(Rect::new(x, y, w, h));
    }
    let _ = client;
    let Some(gc) = s.gcs_mut().get_mut(cid) else {
        return Err(OpErr::X(err::GC, cid));
    };
    gc.clip_origin = (clip_x, clip_y);
    gc.clip_rects = rects;
    gc.clip_mask = 0; // An explicit list clears any bitmap mask.
    Ok(())
}

fn free_gc(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let cid = rd.u32()?;
    let _ = client;
    if !s.gcs_mut().free(cid) {
        return Err(OpErr::X(err::GC, cid));
    }
    Ok(())
}

// ---- drawing ----

fn clear_area(
    s: &mut Server,
    client: ClientId,
    exposures: u8,
    rd: &mut Rd<'_>,
) -> Result<(), OpErr> {
    let window = rd.u32()?;
    let x = i32::from(rd.i16()?);
    let y = i32::from(rd.i16()?);
    let w = u32::from(rd.u16()?);
    let h = u32::from(rd.u16()?);
    let _ = client;
    if !s.tree().windows.contains_key(&window) {
        return Err(OpErr::X(err::WINDOW, window));
    }
    s.clear_area(window, x, y, w, h, exposures != 0);
    Ok(())
}

/// Read a w×h region of a drawable into BGRA bytes (zeros outside
/// bounds) — the CopyArea source staging.
fn read_region(s: &Server, drawable: u32, x: i32, y: i32, w: u32, h: u32) -> Vec<u8> {
    let mut out = vec![0u8; (w as usize) * (h as usize) * 4];
    let src: Option<&crate::render::Store> = if let Some(win) = s.tree().get(drawable) {
        win.store.as_ref()
    } else {
        s.pixmaps().get(&drawable)
    };
    let Some(store) = src else {
        return out;
    };
    for yy in 0..h as i32 {
        for xx in 0..w as i32 {
            if let Some(p) = store.pixel(x + xx, y + yy) {
                let at = (yy as usize * w as usize + xx as usize) * 4;
                out[at] = p as u8;
                out[at + 1] = (p >> 8) as u8;
                out[at + 2] = (p >> 16) as u8;
            }
        }
    }
    out
}

fn copy_area(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let src = rd.u32()?;
    let dst = rd.u32()?;
    let gc_id = rd.u32()?;
    let src_x = i32::from(rd.i16()?);
    let src_y = i32::from(rd.i16()?);
    let dst_x = i32::from(rd.i16()?);
    let dst_y = i32::from(rd.i16()?);
    let w = u32::from(rd.u16()?);
    let h = u32::from(rd.u16()?);
    let src_kind = drawable_or_err(s, client, src, 62)?;
    let dst_kind = drawable_or_err(s, client, dst, 62)?;
    if src_kind == DrawableKind::InputOnlyWindow || dst_kind == DrawableKind::InputOnlyWindow {
        return Err(OpErr::X(err::MATCH, dst));
    }
    let gc = gc_or_err(s, client, gc_id, 62)?;
    check_supported(&gc, dst)?;
    // Source bounds (for GraphicsExpose bookkeeping).
    let (sw, sh) = match src_kind {
        DrawableKind::Window => s.tree().get(src).map_or((0, 0), |w| (w.width, w.height)),
        _ => s
            .pixmaps()
            .get(&src)
            .map_or((0, 0), |p| (p.width, p.height)),
    };
    let src_rect = Rect::new(src_x, src_y, w, h);
    let in_bounds = Rect::new(0, 0, sw, sh).intersect(src_rect);
    let stage = read_region(s, src, src_x, src_y, w, h);
    match dst_kind {
        DrawableKind::Window => {
            let (dw, dh) = (w, h);
            s.draw_on_window(dst, &gc, |t| {
                for yy in 0..dh as i32 {
                    for xx in 0..dw as i32 {
                        let at = (yy as usize * dw as usize + xx as usize) * 4;
                        let px = u32::from(stage[at])
                            | (u32::from(stage[at + 1]) << 8)
                            | (u32::from(stage[at + 2]) << 16);
                        t.put(dst_x + xx, dst_y + yy, px);
                    }
                }
            });
        }
        DrawableKind::Pixmap => {
            let (dw, dh) = (w, h);
            s.draw_on_pixmap(dst, &gc, |t| {
                for yy in 0..dh as i32 {
                    for xx in 0..dw as i32 {
                        let at = (yy as usize * dw as usize + xx as usize) * 4;
                        let px = u32::from(stage[at])
                            | (u32::from(stage[at + 1]) << 8)
                            | (u32::from(stage[at + 2]) << 16);
                        t.put(dst_x + xx, dst_y + yy, px);
                    }
                }
            });
        }
        DrawableKind::InputOnlyWindow => {}
    }
    // NoExpose when the source was fully in bounds; GraphicsExpose for
    // the clipped-away destination area otherwise.
    if gc.graphics_exposures {
        if in_bounds.is_some() {
            let ev = XEvent::NoExpose {
                drawable: dst,
                minor: 0,
                major: 62,
            };
            s.deliver_to_window_owner(dst, &ev);
        } else {
            // The uncovered destination box (the part of the request
            // outside the source drawable, mapped into dst space).
            let uncovered = Rect::new(dst_x, dst_y, w, h);
            let ev = XEvent::GraphicsExpose {
                drawable: dst,
                x: uncovered.x.unsigned_abs() as u16,
                y: uncovered.y.unsigned_abs() as u16,
                width: uncovered.w as u16,
                height: uncovered.h as u16,
                minor: 0,
                count: 0,
                major: 62,
            };
            s.deliver_to_window_owner(dst, &ev);
        }
    }
    Ok(())
}

fn copy_plane(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let src = rd.u32()?;
    let dst = rd.u32()?;
    let gc_id = rd.u32()?;
    let src_x = i32::from(rd.i16()?);
    let src_y = i32::from(rd.i16()?);
    let dst_x = i32::from(rd.i16()?);
    let dst_y = i32::from(rd.i16()?);
    let w = u32::from(rd.u16()?);
    let h = u32::from(rd.u16()?);
    let plane = rd.u32()?;
    if plane.count_ones() != 1 || plane > 0x00ff_ffff {
        return Err(OpErr::X(err::VALUE, plane));
    }
    let src_kind = drawable_or_err(s, client, src, 63)?;
    let dst_kind = drawable_or_err(s, client, dst, 63)?;
    if src_kind == DrawableKind::InputOnlyWindow || dst_kind == DrawableKind::InputOnlyWindow {
        return Err(OpErr::X(err::MATCH, dst));
    }
    let gc = gc_or_err(s, client, gc_id, 63)?;
    check_supported(&gc, dst)?;
    let stage = read_region(s, src, src_x, src_y, w, h);
    let fg = gc.foreground;
    let bg = gc.background;
    match dst_kind {
        DrawableKind::Window => {
            let (dw, dh) = (w, h);
            s.draw_on_window(dst, &gc, |t| {
                for yy in 0..dh as i32 {
                    for xx in 0..dw as i32 {
                        let at = (yy as usize * dw as usize + xx as usize) * 4;
                        let px = u32::from(stage[at])
                            | (u32::from(stage[at + 1]) << 8)
                            | (u32::from(stage[at + 2]) << 16);
                        let expanded = if px & plane != 0 { fg } else { bg };
                        t.put(dst_x + xx, dst_y + yy, expanded);
                    }
                }
            });
        }
        DrawableKind::Pixmap => {
            let (dw, dh) = (w, h);
            s.draw_on_pixmap(dst, &gc, |t| {
                for yy in 0..dh as i32 {
                    for xx in 0..dw as i32 {
                        let at = (yy as usize * dw as usize + xx as usize) * 4;
                        let px = u32::from(stage[at])
                            | (u32::from(stage[at + 1]) << 8)
                            | (u32::from(stage[at + 2]) << 16);
                        let expanded = if px & plane != 0 { fg } else { bg };
                        t.put(dst_x + xx, dst_y + yy, expanded);
                    }
                }
            });
        }
        DrawableKind::InputOnlyWindow => {}
    }
    Ok(())
}

fn poly_point(s: &mut Server, client: ClientId, mode: u8, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    if mode > 1 {
        return Err(OpErr::X(err::VALUE, u32::from(mode)));
    }
    let drawable = rd.u32()?;
    let gc_id = rd.u32()?;
    let pts = rd.points_rest()?;
    let kind = drawable_or_err(s, client, drawable, 64)?;
    if kind == DrawableKind::InputOnlyWindow {
        return Err(OpErr::X(err::MATCH, drawable));
    }
    let gc = gc_or_err(s, client, gc_id, 64)?;
    check_supported(&gc, drawable)?;
    let pts = absolute_points(mode, &pts);
    let fg = gc.foreground;
    match kind {
        DrawableKind::Window => s.draw_on_window(drawable, &gc, |t| t.points(&pts, fg)),
        DrawableKind::Pixmap => s.draw_on_pixmap(drawable, &gc, |t| t.points(&pts, fg)),
        DrawableKind::InputOnlyWindow => {}
    }
    Ok(())
}

fn poly_line(s: &mut Server, client: ClientId, mode: u8, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    if mode > 1 {
        return Err(OpErr::X(err::VALUE, u32::from(mode)));
    }
    let drawable = rd.u32()?;
    let gc_id = rd.u32()?;
    let pts = rd.points_rest()?;
    let kind = drawable_or_err(s, client, drawable, 65)?;
    if kind == DrawableKind::InputOnlyWindow {
        return Err(OpErr::X(err::MATCH, drawable));
    }
    let gc = gc_or_err(s, client, gc_id, 65)?;
    check_supported(&gc, drawable)?;
    let pts = absolute_points(mode, &pts);
    let fg = gc.foreground;
    let not_last = gc.cap_style == 0; // CapNotLast
    match kind {
        DrawableKind::Window => s.draw_on_window(drawable, &gc, |t| {
            stroke(t, &gc, &pts, fg, not_last);
        }),
        DrawableKind::Pixmap => s.draw_on_pixmap(drawable, &gc, |t| {
            stroke(t, &gc, &pts, fg, not_last);
        }),
        DrawableKind::InputOnlyWindow => {}
    }
    Ok(())
}

/// One stroked path through the GC's line state.
fn stroke(
    t: &mut crate::render::Target<'_>,
    gc: &crate::gc::Gc,
    pts: &[(i32, i32)],
    fg: u32,
    not_last: bool,
) {
    if gc.line_width == 0 {
        let dashes = gc.dash_state().map(|(off, pat)| (off, pat.to_vec()));
        let dash_bg = if gc.line_style == crate::gc::LineStyle::DoubleDash {
            Some(gc.background)
        } else {
            None
        };
        match (dashes, dash_bg) {
            (Some((off, pat)), bg) => {
                t.polyline(pts, fg, not_last, Some((off, &pat)), bg);
            }
            (None, _) => {
                t.polyline(pts, fg, not_last, None, None);
            }
        }
    } else {
        t.wide_polyline(pts, gc.line_width, fg);
    }
}

fn poly_segment(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let drawable = rd.u32()?;
    let gc_id = rd.u32()?;
    let rest = rd.rest();
    if rest.len() % 8 != 0 {
        return Err(OpErr::Short);
    }
    let mut segs = Vec::with_capacity(rest.len() / 8);
    for c in rest.chunks_exact(8) {
        segs.push((
            i32::from(u16::from_le_bytes([c[0], c[1]]) as i16),
            i32::from(u16::from_le_bytes([c[2], c[3]]) as i16),
            i32::from(u16::from_le_bytes([c[4], c[5]]) as i16),
            i32::from(u16::from_le_bytes([c[6], c[7]]) as i16),
        ));
    }
    let kind = drawable_or_err(s, client, drawable, 66)?;
    if kind == DrawableKind::InputOnlyWindow {
        return Err(OpErr::X(err::MATCH, drawable));
    }
    let gc = gc_or_err(s, client, gc_id, 66)?;
    check_supported(&gc, drawable)?;
    let fg = gc.foreground;
    match kind {
        DrawableKind::Window => s.draw_on_window(drawable, &gc, |t| {
            t.segments(&segs, fg, gc.cap_style == 0);
        }),
        DrawableKind::Pixmap => s.draw_on_pixmap(drawable, &gc, |t| {
            t.segments(&segs, fg, gc.cap_style == 0);
        }),
        DrawableKind::InputOnlyWindow => {}
    }
    Ok(())
}

fn poly_rectangle(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let drawable = rd.u32()?;
    let gc_id = rd.u32()?;
    let rects = rd.rects_rest()?;
    let kind = drawable_or_err(s, client, drawable, 67)?;
    if kind == DrawableKind::InputOnlyWindow {
        return Err(OpErr::X(err::MATCH, drawable));
    }
    let gc = gc_or_err(s, client, gc_id, 67)?;
    check_supported(&gc, drawable)?;
    let fg = gc.foreground;
    match kind {
        DrawableKind::Window => s.draw_on_window(drawable, &gc, |t| {
            for r in &rects {
                t.rect_outline(r.x, r.y, r.w, r.h, fg);
            }
        }),
        DrawableKind::Pixmap => s.draw_on_pixmap(drawable, &gc, |t| {
            for r in &rects {
                t.rect_outline(r.x, r.y, r.w, r.h, fg);
            }
        }),
        DrawableKind::InputOnlyWindow => {}
    }
    Ok(())
}

fn poly_arc(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let drawable = rd.u32()?;
    let gc_id = rd.u32()?;
    let arcs = rd.arcs_rest()?;
    let kind = drawable_or_err(s, client, drawable, 68)?;
    if kind == DrawableKind::InputOnlyWindow {
        return Err(OpErr::X(err::MATCH, drawable));
    }
    let gc = gc_or_err(s, client, gc_id, 68)?;
    check_supported(&gc, drawable)?;
    let fg = gc.foreground;
    match kind {
        DrawableKind::Window => s.draw_on_window(drawable, &gc, |t| {
            for a in &arcs {
                t.arc_outline(*a, fg);
            }
        }),
        DrawableKind::Pixmap => s.draw_on_pixmap(drawable, &gc, |t| {
            for a in &arcs {
                t.arc_outline(*a, fg);
            }
        }),
        DrawableKind::InputOnlyWindow => {}
    }
    Ok(())
}

fn fill_poly(s: &mut Server, client: ClientId, _shape: u8, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let drawable = rd.u32()?;
    let gc_id = rd.u32()?;
    let mode = rd.i16()? as u8;
    rd.skip(2)?;
    if mode > 1 {
        return Err(OpErr::X(err::VALUE, u32::from(mode)));
    }
    let rest = rd.rest();
    if rest.len() % 4 != 0 {
        return Err(OpErr::Short);
    }
    let mut pts = Vec::with_capacity(rest.len() / 4);
    for c in rest.chunks_exact(4) {
        pts.push((
            i32::from(u16::from_le_bytes([c[0], c[1]]) as i16),
            i32::from(u16::from_le_bytes([c[2], c[3]]) as i16),
        ));
    }
    let kind = drawable_or_err(s, client, drawable, 69)?;
    if kind == DrawableKind::InputOnlyWindow {
        return Err(OpErr::X(err::MATCH, drawable));
    }
    let gc = gc_or_err(s, client, gc_id, 69)?;
    check_supported(&gc, drawable)?;
    let pts = absolute_points(mode, &pts);
    let fg = gc.foreground;
    let winding = gc.fill_rule == 1;
    match kind {
        DrawableKind::Window => s.draw_on_window(drawable, &gc, |t| {
            t.fill_polygon(&pts, fg, winding);
        }),
        DrawableKind::Pixmap => s.draw_on_pixmap(drawable, &gc, |t| {
            t.fill_polygon(&pts, fg, winding);
        }),
        DrawableKind::InputOnlyWindow => {}
    }
    Ok(())
}

fn poly_fill_rectangle(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let drawable = rd.u32()?;
    let gc_id = rd.u32()?;
    let rects = rd.rects_rest()?;
    let kind = drawable_or_err(s, client, drawable, 70)?;
    if kind == DrawableKind::InputOnlyWindow {
        return Err(OpErr::X(err::MATCH, drawable));
    }
    let gc = gc_or_err(s, client, gc_id, 70)?;
    check_supported(&gc, drawable)?;
    let fg = gc.foreground;
    match kind {
        DrawableKind::Window => s.draw_on_window(drawable, &gc, |t| {
            for r in &rects {
                t.fill_rect(r.x, r.y, r.w, r.h, fg);
            }
        }),
        DrawableKind::Pixmap => s.draw_on_pixmap(drawable, &gc, |t| {
            for r in &rects {
                t.fill_rect(r.x, r.y, r.w, r.h, fg);
            }
        }),
        DrawableKind::InputOnlyWindow => {}
    }
    Ok(())
}

fn poly_fill_arc(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let drawable = rd.u32()?;
    let gc_id = rd.u32()?;
    let arcs = rd.arcs_rest()?;
    let kind = drawable_or_err(s, client, drawable, 71)?;
    if kind == DrawableKind::InputOnlyWindow {
        return Err(OpErr::X(err::MATCH, drawable));
    }
    let gc = gc_or_err(s, client, gc_id, 71)?;
    check_supported(&gc, drawable)?;
    let fg = gc.foreground;
    let pie = gc.arc_mode == 1;
    match kind {
        DrawableKind::Window => s.draw_on_window(drawable, &gc, |t| {
            for a in &arcs {
                t.fill_arc(*a, fg, pie);
            }
        }),
        DrawableKind::Pixmap => s.draw_on_pixmap(drawable, &gc, |t| {
            for a in &arcs {
                t.fill_arc(*a, fg, pie);
            }
        }),
        DrawableKind::InputOnlyWindow => {}
    }
    Ok(())
}

// ---- images ----

#[allow(clippy::too_many_lines)] // one decoder per image format, flat
fn put_image(s: &mut Server, client: ClientId, format: u8, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    // The real xPutImageReq field order: width, height, dst-x, dst-y,
    // drawable, gc, left-pad, depth — then the data. The format rides
    // the header's flag byte (xproto.h).
    let w = u32::from(rd.u16()?);
    let h = u32::from(rd.u16()?);
    let dst_x = i32::from(rd.i16()?);
    let dst_y = i32::from(rd.i16()?);
    let drawable = rd.u32()?;
    let gc_id = rd.u32()?;
    let left_pad = rd.u8()?;
    let depth = rd.u8()?;
    let kind = drawable_or_err(s, client, drawable, 72)?;
    if kind == DrawableKind::InputOnlyWindow {
        return Err(OpErr::X(err::MATCH, drawable));
    }
    let gc = gc_or_err(s, client, gc_id, 72)?;
    check_supported(&gc, drawable)?;
    let dst_depth = if kind == DrawableKind::InputOnlyWindow {
        0
    } else {
        24
    };
    match format {
        2 => {
            // ZPixmap, depth 24, 32 bpp, rows = w * 4.
            if depth != dst_depth {
                return Err(OpErr::X(err::MATCH, u32::from(depth)));
            }
            if left_pad != 0 {
                return Err(OpErr::X(err::MATCH, u32::from(left_pad)));
            }
            let need = (w as usize) * (h as usize) * 4;
            let rest = rd.rest();
            if rest.len() < need {
                return Err(OpErr::Short);
            }
            let data = rest[..need].to_vec();
            let (dw, dh) = (w, h);
            match kind {
                DrawableKind::Window => s.draw_on_window(drawable, &gc, |t| {
                    for yy in 0..dh as i32 {
                        for xx in 0..dw as i32 {
                            let at = (yy as usize * dw as usize + xx as usize) * 4;
                            let px = u32::from(data[at])
                                | (u32::from(data[at + 1]) << 8)
                                | (u32::from(data[at + 2]) << 16);
                            t.put(dst_x + xx, dst_y + yy, px);
                        }
                    }
                }),
                DrawableKind::Pixmap => s.draw_on_pixmap(drawable, &gc, |t| {
                    for yy in 0..dh as i32 {
                        for xx in 0..dw as i32 {
                            let at = (yy as usize * dw as usize + xx as usize) * 4;
                            let px = u32::from(data[at])
                                | (u32::from(data[at + 1]) << 8)
                                | (u32::from(data[at + 2]) << 16);
                            t.put(dst_x + xx, dst_y + yy, px);
                        }
                    }
                }),
                DrawableKind::InputOnlyWindow => {}
            }
            Ok(())
        }
        0 => {
            // XYBitmap: depth 1, 32-bit scanline units; bits expand
            // through the GC's foreground/background.
            if depth != 1 || dst_depth != 24 {
                return Err(OpErr::X(err::MATCH, u32::from(depth)));
            }
            let stride = (w as usize).div_ceil(32) * 4;
            let need = stride * h as usize;
            let rest = rd.rest();
            if rest.len() < need {
                return Err(OpErr::Short);
            }
            let data = rest[..need].to_vec();
            let (fg, bg) = (gc.foreground, gc.background);
            let (dw, dh) = (w, h);
            match kind {
                DrawableKind::Window => s.draw_on_window(drawable, &gc, |t| {
                    for yy in 0..dh as i32 {
                        for xx in 0..dw as i32 {
                            let byte = data[yy as usize * stride + (xx as usize / 8)];
                            let bit = (byte >> (xx % 8)) & 1;
                            let px = if bit == 1 { fg } else { bg };
                            t.put(dst_x + xx, dst_y + yy, px);
                        }
                    }
                }),
                DrawableKind::Pixmap => s.draw_on_pixmap(drawable, &gc, |t| {
                    for yy in 0..dh as i32 {
                        for xx in 0..dw as i32 {
                            let byte = data[yy as usize * stride + (xx as usize / 8)];
                            let bit = (byte >> (xx % 8)) & 1;
                            let px = if bit == 1 { fg } else { bg };
                            t.put(dst_x + xx, dst_y + yy, px);
                        }
                    }
                }),
                DrawableKind::InputOnlyWindow => {}
            }
            Ok(())
        }
        _ => Err(OpErr::X(err::VALUE, u32::from(format))), // XYPixmap is out of the subset
    }
}

fn get_image(s: &mut Server, client: ClientId, format: u8, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let drawable = rd.u32()?;
    let x = i32::from(rd.i16()?);
    let y = i32::from(rd.i16()?);
    let w = u32::from(rd.u16()?);
    let h = u32::from(rd.u16()?);
    let _plane_mask = rd.u32()?;
    if format != 2 {
        return Err(OpErr::X(err::VALUE, u32::from(format)));
    }
    let kind = drawable_or_err(s, client, drawable, 73)?;
    if kind == DrawableKind::InputOnlyWindow {
        return Err(OpErr::X(err::MATCH, drawable));
    }
    // Ensure the window store exists so reads see the background.
    if kind == DrawableKind::Window {
        s.ensure_window_store(drawable);
    }
    let data = read_region(s, drawable, x, y, w, h);
    let e = s.endian_of(client);
    let mut out = s.reply_prefix(client);
    out.push(24); // depth
    out.push(0);
    out.extend_from_slice(&e.put_u32(s.visual()));
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&data);
    s.finish_reply(client, out);
    Ok(())
}

// ---- colormaps and cursors ----

fn create_colormap(
    s: &mut Server,
    _client: ClientId,
    alloc: u8,
    rd: &mut Rd<'_>,
) -> Result<(), OpErr> {
    let mid = rd.u32()?;
    let window = rd.u32()?;
    let visual = rd.u32()?;
    if alloc > 2 {
        return Err(OpErr::X(err::VALUE, u32::from(alloc)));
    }
    if !s.tree().windows.contains_key(&window) {
        return Err(OpErr::X(err::WINDOW, window));
    }
    if visual != s.visual() {
        return Err(OpErr::X(err::MATCH, visual));
    }
    if s.colormaps().contains(&mid) {
        return Err(OpErr::X(err::ID_CHOICE, mid));
    }
    s.colormaps_mut().insert(mid);
    Ok(())
}

fn free_colormap(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let mid = rd.u32()?;
    let _ = client;
    if !s.colormaps_mut().remove(&mid) {
        return Err(OpErr::X(err::COLOR, mid));
    }
    Ok(())
}

fn copy_colormap_and_free(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let mid = rd.u32()?;
    let src = rd.u32()?;
    let _ = client;
    if !s.colormaps().contains(&src) {
        return Err(OpErr::X(err::COLOR, src));
    }
    if s.colormaps().contains(&mid) {
        return Err(OpErr::X(err::ID_CHOICE, mid));
    }
    s.colormaps_mut().insert(mid);
    Ok(())
}

fn install_colormap(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let mid = rd.u32()?;
    let _ = client;
    if !s.colormaps().contains(&mid) {
        return Err(OpErr::X(err::ALLOC, mid));
    }
    Ok(())
}

fn list_installed_colormaps(
    s: &mut Server,
    client: ClientId,
    rd: &mut Rd<'_>,
) -> Result<(), OpErr> {
    let window = rd.u32()?;
    if !s.tree().windows.contains_key(&window) {
        return Err(OpErr::X(err::WINDOW, window));
    }
    let e = s.endian_of(client);
    let mut out = s.reply_prefix(client);
    out.push(1); // one installed colormap
    out.extend_from_slice(&e.put_u16(0));
    out.extend_from_slice(&e.put_u32(s.default_colormap()));
    s.finish_reply(client, out);
    Ok(())
}

/// TrueColor pixel decode (masks 0xFF0000/0xFF00/0xFF).
fn decode_pixel(px: u32) -> (u16, u16, u16) {
    (
        ((px >> 16) & 0xff) as u16 * 257,
        ((px >> 8) & 0xff) as u16 * 257,
        (px & 0xff) as u16 * 257,
    )
}

fn alloc_color(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let cmap = rd.u32()?;
    let red = rd.u16()?;
    let green = rd.u16()?;
    let blue = rd.u16()?;
    if !s.colormaps().contains(&cmap) {
        return Err(OpErr::X(err::COLOR, cmap));
    }
    // Direct pixel: 8-bit channels re-scaled to 16.
    let px = (u32::from(red >> 8) << 16) | (u32::from(green >> 8) << 8) | u32::from(blue >> 8);
    let e = s.endian_of(client);
    let mut out = s.reply_prefix(client);
    out.extend_from_slice(&e.put_u16(red));
    out.extend_from_slice(&e.put_u16(green));
    out.extend_from_slice(&e.put_u16(blue));
    out.extend_from_slice(&e.put_u16(0));
    out.extend_from_slice(&e.put_u32(px));
    s.finish_reply(client, out);
    Ok(())
}

/// The tiny named-color table (black/white and the primaries).
fn named_color(name: &[u8]) -> Option<(u16, u16, u16)> {
    let (r, g, b) = match name {
        b"black" => (0u16, 0u16, 0u16),
        b"white" => (0xffff, 0xffff, 0xffff),
        b"red" => (0xffff, 0, 0),
        b"green" => (0, 0xffff, 0),
        b"blue" => (0, 0, 0xffff),
        b"gray" | b"grey" => (0xbebe, 0xbebe, 0xbebe),
        _ => return None,
    };
    Some((r, g, b))
}

fn alloc_named_color(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let cmap = rd.u32()?;
    let name_len = rd.u16()? as usize;
    rd.skip(2)?;
    let rest = rd.rest();
    if rest.len() < name_len {
        return Err(OpErr::Short);
    }
    let name = rest[..name_len].to_vec();
    if !s.colormaps().contains(&cmap) {
        return Err(OpErr::X(err::COLOR, cmap));
    }
    let Some((r, g, b)) = named_color(&name) else {
        return Err(OpErr::X(err::NAME, 0));
    };
    let px = (u32::from(r >> 8) << 16) | (u32::from(g >> 8) << 8) | u32::from(b >> 8);
    let e = s.endian_of(client);
    let mut out = s.reply_prefix(client);
    out.extend_from_slice(&e.put_u32(px));
    out.extend_from_slice(&e.put_u16(r));
    out.extend_from_slice(&e.put_u16(g));
    out.extend_from_slice(&e.put_u16(b));
    out.extend_from_slice(&e.put_u16(r));
    out.extend_from_slice(&e.put_u16(g));
    out.extend_from_slice(&e.put_u16(b));
    out.extend_from_slice(&e.put_u16(0));
    s.finish_reply(client, out);
    Ok(())
}

fn alloc_color_cells(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    // A TrueColor visual has no writable cells: the subset reads the
    // colormap id for validation, then answers BadAlloc.
    let cmap = rd.u32()?;
    let _ = client;
    if !s.colormaps().contains(&cmap) {
        return Err(OpErr::X(err::COLOR, cmap));
    }
    Err(OpErr::X(err::ALLOC, cmap))
}

fn free_colors(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let cmap = rd.u32()?;
    let _plane_mask = rd.u32()?;
    let _ = client;
    if !s.colormaps().contains(&cmap) {
        return Err(OpErr::X(err::COLOR, cmap));
    }
    Ok(())
}

fn store_colors(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let cmap = rd.u32()?;
    let _ = client;
    if !s.colormaps().contains(&cmap) {
        return Err(OpErr::X(err::COLOR, cmap));
    }
    // Read-only colormap: writing cells is BadAccess.
    Err(OpErr::X(err::ACCESS, cmap))
}

fn query_colors(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let cmap = rd.u32()?;
    let rest = rd.rest();
    if rest.len() % 4 != 0 {
        return Err(OpErr::Short);
    }
    let mut pixels = Vec::with_capacity(rest.len() / 4);
    for c in rest.chunks_exact(4) {
        pixels.push(
            u32::from(c[0])
                | (u32::from(c[1]) << 8)
                | (u32::from(c[2]) << 16)
                | (u32::from(c[3]) << 24),
        );
    }
    if !s.colormaps().contains(&cmap) {
        return Err(OpErr::X(err::COLOR, cmap));
    }
    let e = s.endian_of(client);
    let mut out = s.reply_prefix(client);
    for px in pixels {
        let (r, g, b) = decode_pixel(px);
        out.extend_from_slice(&e.put_u16(r));
        out.extend_from_slice(&e.put_u16(g));
        out.extend_from_slice(&e.put_u16(b));
    }
    s.finish_reply(client, out);
    Ok(())
}

fn lookup_color(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let cmap = rd.u32()?;
    let name_len = rd.u16()? as usize;
    rd.skip(2)?;
    let rest = rd.rest();
    if rest.len() < name_len {
        return Err(OpErr::Short);
    }
    let name = rest[..name_len].to_vec();
    if !s.colormaps().contains(&cmap) {
        return Err(OpErr::X(err::COLOR, cmap));
    }
    let Some((r, g, b)) = named_color(&name) else {
        return Err(OpErr::X(err::NAME, 0));
    };
    let e = s.endian_of(client);
    let mut out = s.reply_prefix(client);
    out.extend_from_slice(&e.put_u16(r));
    out.extend_from_slice(&e.put_u16(g));
    out.extend_from_slice(&e.put_u16(b));
    out.extend_from_slice(&e.put_u16(r));
    out.extend_from_slice(&e.put_u16(g));
    out.extend_from_slice(&e.put_u16(b));
    s.finish_reply(client, out);
    Ok(())
}

fn create_cursor(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let cid = rd.u32()?;
    let source = rd.u32()?;
    let mask = rd.u32()?;
    let _fg = (rd.u16()?, rd.u16()?, rd.u16()?);
    let _bg = (rd.u16()?, rd.u16()?, rd.u16()?);
    let _x = rd.u16()?;
    let _y = rd.u16()?;
    if !s.pixmaps().contains_key(&source) {
        return Err(OpErr::X(err::PIXMAP, source));
    }
    if mask != 0 && !s.pixmaps().contains_key(&mask) {
        return Err(OpErr::X(err::PIXMAP, mask));
    }
    if s.cursors().contains(&cid) {
        return Err(OpErr::X(err::ID_CHOICE, cid));
    }
    s.cursors_mut().insert(cid);
    let _ = client;
    Ok(())
}

fn create_glyph_cursor(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let cid = rd.u32()?;
    let source_font = rd.u32()?;
    let mask_font = rd.u32()?;
    let _source_char = rd.u32()?;
    let _mask_char = rd.u32()?;
    let _fg = (rd.u16()?, rd.u16()?, rd.u16()?);
    let _bg = (rd.u16()?, rd.u16()?, rd.u16()?);
    if !s.fonts().contains(&source_font) {
        return Err(OpErr::X(err::FONT, source_font));
    }
    if mask_font != 0 && !s.fonts().contains(&mask_font) {
        return Err(OpErr::X(err::FONT, mask_font));
    }
    if s.cursors().contains(&cid) {
        return Err(OpErr::X(err::ID_CHOICE, cid));
    }
    s.cursors_mut().insert(cid);
    let _ = client;
    Ok(())
}

fn free_cursor(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let cid = rd.u32()?;
    let _ = client;
    if !s.cursors_mut().remove(&cid) {
        return Err(OpErr::X(err::CURSOR, cid));
    }
    Ok(())
}

fn recolor_cursor(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let cid = rd.u32()?;
    let _fg = (rd.u16()?, rd.u16()?, rd.u16()?);
    let _bg = (rd.u16()?, rd.u16()?, rd.u16()?);
    let _ = client;
    if !s.cursors().contains(&cid) {
        return Err(OpErr::X(err::CURSOR, cid));
    }
    Ok(())
}

fn query_best_size(
    s: &mut Server,
    client: ClientId,
    _class: u8,
    rd: &mut Rd<'_>,
) -> Result<(), OpErr> {
    let drawable = rd.u32()?;
    let width = rd.u16()?;
    let height = rd.u16()?;
    drawable_or_err(s, client, drawable, 93)?;
    let e = s.endian_of(client);
    let mut out = s.reply_prefix(client);
    out.extend_from_slice(&e.put_u16(width));
    out.extend_from_slice(&e.put_u16(height));
    s.finish_reply(client, out);
    Ok(())
}

// ---- MIT-SHM ----

pub(crate) fn shm(
    s: &mut Server,
    client: ClientId,
    minor: u8,
    rd: &mut Rd<'_>,
) -> Result<(), OpErr> {
    match minor {
        0 => shm_query_version(s, client),
        1 => shm_attach(s, client, rd),
        2 => shm_detach(s, client, rd),
        3 => shm_put_image(s, client, rd),
        4 => shm_get_image(s, client, rd),
        5 => shm_create_pixmap(s, client, rd),
        _ => Err(OpErr::X(err::REQUEST, u32::from(minor))),
    }
}

fn shm_query_version(s: &mut Server, client: ClientId) -> Result<(), OpErr> {
    let e = s.endian_of(client);
    let mut out = s.reply_prefix(client);
    out.push(1); // shared pixmaps supported (snapshot semantics)
    out.extend_from_slice(&e.put_u16(1)); // major version
    out.extend_from_slice(&e.put_u16(1)); // minor version
    out.push(0); // number of shared pixmap formats
    s.finish_reply(client, out);
    Ok(())
}

fn shm_attach(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    // The real MIT-SHM Attach wire order: shmseg (the client's XID),
    // shmid (the SysV key), read-only, three pad bytes — the order
    // real X11 clients emit (X11 protocol §MIT-SHM).
    let seg = rd.u32()?;
    let shmid = rd.u32()?;
    let read_only = rd.u8()?;
    rd.skip(3)?;
    let _ = client;
    s.shm_host_mut()
        .attach_segment(seg, u64::from(shmid), read_only != 0);
    match s.shm_state_mut().attach(seg, read_only != 0) {
        Ok(()) => Ok(()),
        Err(_) => Err(OpErr::X(err::ACCESS, seg)),
    }
}

fn shm_detach(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let seg = rd.u32()?;
    let _ = client;
    match s.shm_state_mut().detach(seg) {
        Ok(()) => Ok(()),
        Err(_) => Err(OpErr::X(err::VALUE, seg)),
    }
}

fn shm_put_image(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let drawable = rd.u32()?;
    let gc_id = rd.u32()?;
    let total_width = rd.u16()?;
    let _total_height = rd.u16()?;
    let src_x = rd.u16()?;
    let src_y = rd.u16()?;
    let src_w = rd.u16()?;
    let src_h = rd.u16()?;
    let dst_x = rd.i16()?;
    let dst_y = rd.i16()?;
    let depth = rd.u8()?;
    let format = rd.u8()?;
    let send_event = rd.u8()?;
    rd.skip(1)?;
    let seg = rd.u32()?;
    let offset = rd.u32()?;
    if format != 2 {
        return Err(OpErr::X(err::VALUE, u32::from(format)));
    }
    if total_width == 0 {
        return Err(OpErr::X(err::VALUE, u32::from(total_width)));
    }
    let kind = drawable_or_err(s, client, drawable, crate::dispatch::ext::SHM)?;
    if kind == DrawableKind::InputOnlyWindow {
        return Err(OpErr::X(err::MATCH, drawable));
    }
    if depth != 24 {
        return Err(OpErr::X(err::MATCH, u32::from(depth)));
    }
    let gc = gc_or_err(s, client, gc_id, crate::dispatch::ext::SHM)?;
    check_supported(&gc, drawable)?;
    // The segment image is total_width wide; the source rect is carved
    // out of it at (src_x, src_y).
    let stride = u32::from(total_width);
    let need = (src_w as usize) * (src_h as usize) * 4;
    let Some(data) = s.shm_host_mut().read_segment(seg, offset as usize, need) else {
        return Err(OpErr::X(err::ACCESS, seg));
    };
    if data.len() < need {
        return Err(OpErr::X(err::LENGTH, offset));
    }
    let (dw, dh) = (u32::from(src_w), u32::from(src_h));
    let (sx, sy) = (u32::from(src_x), u32::from(src_y));
    match kind {
        DrawableKind::Window => s.draw_on_window(drawable, &gc, |t| {
            for yy in 0..dh as i32 {
                for xx in 0..dw as i32 {
                    let at = ((sy as i32 + yy) as usize * stride as usize
                        + (sx as i32 + xx) as usize)
                        * 4;
                    let px = u32::from(data[at])
                        | (u32::from(data[at + 1]) << 8)
                        | (u32::from(data[at + 2]) << 16);
                    t.put(i32::from(dst_x) + xx, i32::from(dst_y) + yy, px);
                }
            }
        }),
        DrawableKind::Pixmap => s.draw_on_pixmap(drawable, &gc, |t| {
            for yy in 0..dh as i32 {
                for xx in 0..dw as i32 {
                    let at = ((sy as i32 + yy) as usize * stride as usize
                        + (sx as i32 + xx) as usize)
                        * 4;
                    let px = u32::from(data[at])
                        | (u32::from(data[at + 1]) << 8)
                        | (u32::from(data[at + 2]) << 16);
                    t.put(i32::from(dst_x) + xx, i32::from(dst_y) + yy, px);
                }
            }
        }),
        DrawableKind::InputOnlyWindow => {}
    }
    if send_event != 0 {
        // ShmCompletion (the subset's only SHM event, code 66 by the
        // conventional extension-event base; simplified to 66).
        let ev = XEvent::ClientMessage {
            format: 32,
            window: drawable,
            type_atom: 1,
            data: [0u8; 20],
        };
        let _ = ev;
        // (Sent as the raw completion notification below.)
        let e = s.endian_of(client);
        let mut env = Vec::with_capacity(32);
        crate::wire::event_prefix(&mut env, e, 66, 1, 0);
        crate::wire::event_tail(&mut env);
        s.out_bytes(client, &env);
    }
    Ok(())
}

fn shm_get_image(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let drawable = rd.u32()?;
    let x = rd.i16()?;
    let y = rd.i16()?;
    let w = rd.u32()?;
    let h = rd.u32()?;
    let _plane_mask = rd.u32()?;
    let format = rd.u8()?;
    rd.skip(3)?;
    let seg = rd.u32()?;
    let offset = rd.u32()?;
    if format != 2 {
        return Err(OpErr::X(err::VALUE, u32::from(format)));
    }
    let kind = drawable_or_err(s, client, drawable, crate::dispatch::ext::SHM)?;
    if kind == DrawableKind::InputOnlyWindow {
        return Err(OpErr::X(err::MATCH, drawable));
    }
    if kind == DrawableKind::Window {
        s.ensure_window_store(drawable);
    }
    let data = read_region(s, drawable, i32::from(x), i32::from(y), w, h);
    if s.shm_host_mut()
        .write_segment(seg, offset as usize, &data)
        .is_none()
    {
        return Err(OpErr::X(err::ACCESS, seg));
    }
    if !s.shm_state().writable(seg) {
        return Err(OpErr::X(err::ACCESS, seg));
    }
    let e = s.endian_of(client);
    let mut out = s.reply_prefix(client);
    out.push(24);
    out.push(0);
    out.extend_from_slice(&e.put_u32(s.visual()));
    out.extend_from_slice(&e.put_u32(w * h * 4));
    s.finish_reply(client, out);
    Ok(())
}

fn shm_create_pixmap(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let pid = rd.u32()?;
    let drawable = rd.u32()?;
    let width = rd.u32()?;
    let height = rd.u32()?;
    let depth = rd.u8()?;
    rd.skip(1)?;
    let seg = rd.u32()?;
    let offset = rd.u32()?;
    let _ = client;
    if depth != 24 {
        return Err(OpErr::X(err::VALUE, u32::from(depth)));
    }
    if width == 0 || height == 0 || width > 0xffff || height > 0xffff {
        return Err(OpErr::X(err::VALUE, width.max(height)));
    }
    drawable_or_err(s, client, drawable, crate::dispatch::ext::SHM)?;
    if s.pixmaps().contains_key(&pid) || s.tree().windows.contains_key(&pid) {
        return Err(OpErr::X(err::ID_CHOICE, pid));
    }
    let need = (width as usize) * (height as usize) * 4;
    let Some(data) = s.shm_host_mut().read_segment(seg, offset as usize, need) else {
        return Err(OpErr::X(err::ACCESS, seg));
    };
    if data.len() < need {
        return Err(OpErr::X(err::LENGTH, offset));
    }
    let store = crate::render::Store {
        width,
        height,
        data,
    };
    s.pixmaps_mut().insert(pid, store);
    Ok(())
}
