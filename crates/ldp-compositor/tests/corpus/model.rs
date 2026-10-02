//! The reference model: an independent per-pixel implementation of the
//! fold semantics (see `../corpus.rs` for the contract).

use std::collections::HashMap;

use ldp_compositor::SurfaceId;
use ldp_core::geometry::{Point, Rect};

pub(crate) const GRID: usize = 64;

pub(crate) type Grid = Vec<Vec<bool>>;

/// One surface's flat-walk data: id, output offset, logical size,
/// opaque rects, per-cell version grid.
type FlatEntry = (SurfaceId, (i32, i32), (i32, i32), Vec<Rect>, Vec<Vec<u32>>);

/// Per-cell fold value: `(surface, local_x, local_y, version)` list,
/// top-to-bottom, truncated at the topmost opaque surface.
pub(crate) type CellValues = Vec<Vec<Vec<(u64, u32, u32, u32)>>>;

/// Front-to-back flat walk (children above parents, later siblings
/// above earlier ones), offset-accumulating.
fn visit(model: &Model, id: SurfaceId, parent_offset: (i32, i32), flat: &mut Vec<FlatEntry>) {
    let Some(surface) = model.surfaces.get(&id) else {
        return;
    };
    let offset = match surface.parent {
        None => surface.position,
        Some(_) => (
            parent_offset.0 + surface.position.0,
            parent_offset.1 + surface.position.1,
        ),
    };
    let kids = model.children.get(&Some(id)).cloned().unwrap_or_default();
    for kid in kids.iter().rev() {
        visit(model, *kid, offset, flat);
    }
    if surface.logical != (0, 0) {
        flat.push((
            id,
            offset,
            (surface.logical.0 as i32, surface.logical.1 as i32),
            surface.opaque.clone(),
            surface.version.clone(),
        ));
    }
}

/// (device width, device height, buffer generation, scale q8).
pub(crate) type BufSpec = (u32, u32, u64, u32);

#[derive(Clone, Default)]
pub(crate) struct MPending {
    /// The requested buffer spec (`None` = detach) when `attach_set`.
    pub(crate) attach: Option<BufSpec>,
    /// Whether an attach request ran.
    pub(crate) attach_set: bool,
    pub(crate) opaque: Option<Vec<Rect>>,
    pub(crate) damage: Vec<Rect>,
}

#[derive(Clone)]
pub(crate) struct MSurface {
    pub(crate) parent: Option<SurfaceId>,
    pub(crate) position: (i32, i32),
    pub(crate) pos_pending: Option<(i32, i32)>,
    pub(crate) desync: bool,
    pub(crate) buffer: Option<BufSpec>,
    pub(crate) logical: (u32, u32),
    pub(crate) opaque: Vec<Rect>,
    pub(crate) version: Vec<Vec<u32>>,
    pub(crate) deferred: Option<MPending>,
}

impl MSurface {
    fn is_sync_sub(&self) -> bool {
        self.parent.is_some() && !self.desync
    }

    /// Logical size of a device buffer (outward rounding, mirroring
    /// `BufferAttachment::surface_size`).
    fn logical_of(dev: (u32, u32), q8: u32) -> (u32, u32) {
        let lw = (u64::from(dev.0) * 256)
            .div_ceil(u64::from(q8))
            .min(u64::from(u32::MAX)) as u32;
        let lh = (u64::from(dev.1) * 256)
            .div_ceil(u64::from(q8))
            .min(u64::from(u32::MAX)) as u32;
        (lw, lh)
    }

    /// Buffer identity for swap detection: (w, h, generation) — the
    /// scale is surface state, not buffer identity.
    fn key(b: Option<BufSpec>) -> Option<(u32, u32, u64)> {
        b.map(|s| (s.0, s.1, s.2))
    }
}

#[derive(Default)]
pub(crate) struct Model {
    pub(crate) surfaces: HashMap<SurfaceId, MSurface>,
    /// Back-to-front child order per parent (`None` = roots).
    pub(crate) children: HashMap<Option<SurfaceId>, Vec<SurfaceId>>,
    /// Uncommitted request accumulation (the engine's `PendingState`).
    pub(crate) scratch: HashMap<SurfaceId, MPending>,
    /// Cells whose version was bumped this frame (per surface).
    pub(crate) bumped: HashMap<SurfaceId, Grid>,
}

impl Model {
    pub(crate) fn create_root(&mut self, id: SurfaceId, x: i32, y: i32) {
        self.surfaces.insert(
            id,
            MSurface {
                parent: None,
                position: (x, y),
                pos_pending: None,
                desync: false,
                buffer: None,
                logical: (0, 0),
                opaque: Vec::new(),
                version: Vec::new(),
                deferred: None,
            },
        );
        self.children.entry(None).or_default().push(id);
    }

    pub(crate) fn create_sub(&mut self, id: SurfaceId, parent: SurfaceId) {
        self.surfaces.insert(
            id,
            MSurface {
                parent: Some(parent),
                position: (0, 0),
                pos_pending: None,
                desync: false,
                buffer: None,
                logical: (0, 0),
                opaque: Vec::new(),
                version: Vec::new(),
                deferred: None,
            },
        );
        self.children.entry(Some(parent)).or_default().push(id);
    }

    pub(crate) fn set_position(&mut self, id: SurfaceId, x: i32, y: i32) {
        self.surfaces.get_mut(&id).unwrap().pos_pending = Some((x, y));
    }

    pub(crate) fn set_mode(&mut self, id: SurfaceId, desync: bool) {
        self.surfaces.get_mut(&id).unwrap().desync = desync;
    }

    /// Restack through list rebuild (an independent implementation of
    /// the same "immediately above/below the sibling" contract).
    pub(crate) fn restack(
        &mut self,
        parent: Option<SurfaceId>,
        mover: SurfaceId,
        sibling: SurfaceId,
        above: bool,
    ) {
        let Some(list) = self.children.get_mut(&parent) else {
            return;
        };
        if mover == sibling || !list.contains(&mover) || !list.contains(&sibling) {
            return;
        }
        let rest: Vec<SurfaceId> = list.iter().copied().filter(|s| *s != mover).collect();
        let j = rest.iter().position(|s| *s == sibling).unwrap();
        let at = if above { j + 1 } else { j };
        let mut rest = rest;
        rest.insert(at, mover);
        *list = rest;
    }

    pub(crate) fn destroy(&mut self, id: SurfaceId) {
        let parent = self.surfaces[&id].parent;
        if let Some(list) = self.children.get_mut(&parent) {
            list.retain(|s| *s != id);
        }
        let kids = self.children.remove(&Some(id)).unwrap_or_default();
        self.surfaces.remove(&id);
        self.bumped.remove(&id);
        for kid in kids {
            self.destroy(kid);
        }
    }

    pub(crate) fn commit(&mut self, id: SurfaceId) {
        let pending = self.scratch.remove(&id).unwrap_or_default();
        if self.surfaces[&id].is_sync_sub() {
            let s = self.surfaces.get_mut(&id).unwrap();
            match &mut s.deferred {
                Some(d) => {
                    if pending.attach_set {
                        d.attach = pending.attach;
                        d.attach_set = true;
                    }
                    if pending.opaque.is_some() {
                        d.opaque = pending.opaque;
                    }
                    d.damage.extend(pending.damage);
                }
                None => s.deferred = Some(pending),
            }
        } else {
            self.apply(id, pending);
            self.flush_sync_children(id);
        }
    }

    fn flush_sync_children(&mut self, parent: SurfaceId) {
        let kids = self
            .children
            .get(&Some(parent))
            .cloned()
            .unwrap_or_default();
        for kid in kids {
            let (is_sync, has_deferred) = {
                let s = &self.surfaces[&kid];
                (
                    s.is_sync_sub(),
                    s.deferred.is_some() || s.pos_pending.is_some(),
                )
            };
            if is_sync && has_deferred {
                let deferred = self.surfaces.get_mut(&kid).unwrap().deferred.take();
                self.apply(kid, deferred.unwrap_or_default());
                self.flush_sync_children(kid);
            }
        }
    }

    /// Apply a (merged) pending batch: state swap, version bumps,
    /// opaque change, position release.
    fn apply(&mut self, id: SurfaceId, mut merged: MPending) {
        let s = self.surfaces.get_mut(&id).unwrap();
        if let Some(mut d) = s.deferred.take() {
            // Later requests win; damage unions (mirrors merge_into).
            let later = std::mem::take(&mut merged);
            if later.attach_set {
                d.attach = later.attach;
                d.attach_set = true;
            }
            if later.opaque.is_some() {
                d.opaque = later.opaque;
            }
            d.damage.extend(later.damage);
            merged = d;
        }
        let old_buffer = s.buffer;
        let new_buffer = if merged.attach_set {
            merged.attach
        } else {
            s.buffer
        };
        let new_logical = new_buffer.map_or((0, 0), |b| MSurface::logical_of((b.0, b.1), b.3));
        // Resize the version grid, preserving overlapping cells.
        let mut version = vec![vec![0u32; new_logical.0 as usize]; new_logical.1 as usize];
        for (y, row) in s.version.iter().enumerate() {
            if y >= new_logical.1 as usize {
                break;
            }
            for (x, v) in row.iter().enumerate() {
                if x < new_logical.0 as usize {
                    version[y][x] = *v;
                }
            }
        }
        s.version = version;
        s.logical = new_logical;
        s.buffer = new_buffer;
        if let Some(op) = merged.opaque {
            s.opaque = op;
        }
        if let Some(p) = s.pos_pending.take() {
            s.position = p;
        }

        // Version bumps: explicit damage clipped to the new bounds; a
        // swapped buffer with no explicit damage bumps everything.
        let swapped = MSurface::key(new_buffer) != MSurface::key(old_buffer);
        let mut rects = merged.damage;
        if rects.is_empty() && swapped && new_logical != (0, 0) {
            rects = vec![Rect::new(0, 0, new_logical.0, new_logical.1)];
        }
        let s = self.surfaces.get_mut(&id).unwrap();
        let mut bump = self
            .bumped
            .remove(&id)
            .unwrap_or_else(|| vec![vec![false; new_logical.0 as usize]; new_logical.1 as usize]);
        if bump.len() != new_logical.1 as usize
            || bump.first().map_or(0, Vec::len) != new_logical.0 as usize
        {
            bump = vec![vec![false; new_logical.0 as usize]; new_logical.1 as usize];
        }
        for r in &rects {
            let x0 = r.x.max(0).min(new_logical.0 as i32) as usize;
            let y0 = r.y.max(0).min(new_logical.1 as i32) as usize;
            let x1 = r.right().clamp(0, new_logical.0 as i32) as usize;
            let y1 = r.bottom().clamp(0, new_logical.1 as i32) as usize;
            for (row_v, row_b) in s
                .version
                .iter_mut()
                .zip(bump.iter_mut())
                .skip(y0)
                .take(y1 - y0)
            {
                for (cell_v, cell_b) in row_v
                    .iter_mut()
                    .zip(row_b.iter_mut())
                    .skip(x0)
                    .take(x1 - x0)
                {
                    *cell_v = cell_v.saturating_add(1);
                    *cell_b = true;
                }
            }
        }
        self.bumped.insert(id, bump);
    }

    /// Per-cell fold values: the ordered (top-to-bottom) contribution
    /// list of `(surface, local_x, local_y, version)` — the local cell
    /// is part of the identity: a moved surface shows *shifted* content,
    /// so every interior cell changes. The list is truncated at the
    /// topmost opaque surface.
    pub(crate) fn values(&self) -> CellValues {
        let mut flat: Vec<FlatEntry> = Vec::new();
        let roots = self.children.get(&None).cloned().unwrap_or_default();
        for root in roots.iter().rev() {
            visit(self, *root, (0, 0), &mut flat);
        }

        let mut values: CellValues = vec![vec![Vec::new(); GRID]; GRID];
        for (y, row) in values.iter_mut().enumerate() {
            for (x, cell) in row.iter_mut().enumerate() {
                let mut acc = Vec::new();
                for (id, off, (w, h), opaque, version) in &flat {
                    let lx = x as i32 - off.0;
                    let ly = y as i32 - off.1;
                    if lx >= 0 && ly >= 0 && lx < *w && ly < *h {
                        acc.push((
                            id.raw(),
                            lx as u32,
                            ly as u32,
                            version[ly as usize][lx as usize],
                        ));
                        let opaque_here =
                            opaque.iter().any(|r| r.contains_point(Point::new(lx, ly)));
                        if opaque_here {
                            break;
                        }
                    }
                }
                *cell = acc;
            }
        }
        values
    }
}
