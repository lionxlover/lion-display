//! The surface tree: creation and destruction, stacking under parents,
//! the atomic commit cascade, and the frame's change records.
//!
//! Storage is flat (one map of surfaces, one stacking list per parent)
//! with roles linking subsurfaces to parents; the *commit cascade* is
//! the interesting part. A root (or desync subsurface) commit applies
//! immediately and then flushes every sync-mode descendant that carries
//! deferred state — recursively, so a whole sync chain releases at the
//! outermost application point. A sync subsurface's commit only
//! stashes: its pending state merges into the deferred slot and waits.
//!
//! The tree also owns [`FrameChanges`]: the restack records and removal
//! records the damage engine needs (per-surface inputs — accumulated
//! damage, bounds, opaque regions — are read directly off the surfaces,
//! so effects are carried for inspection and future dispatcher seams).

use std::collections::HashMap;

use ldp_core::geometry::{Rect, Region};

use crate::stacking::{StackChange, StackingList};
use crate::surface::{Role, SubsurfaceMode, Surface, SurfaceId};

/// The maximum subsurface nesting depth (levels of parent-of-parent
/// below a root).
///
/// This is a *server-side policy cap against a crash vector*, not a
/// protocol surface change: the tree's recursive walks (occlusion's
/// `visit`, `snapshot`'s capture, the sync-mode flush) consume one
/// stack frame per nesting level, and a client that authors a chain
/// tens of thousands of links deep turns the next damage pass into a
/// stack overflow — a deterministic server kill from protocol-legal
/// requests. Real scenes sit under a dozen levels (the deepest legit
/// stack the test corpus models is four); 64 is generous headroom
/// while staying four orders of magnitude under the ~10⁵ frames a
/// 8 MiB stack actually dies at.
pub const MAX_SUBSURFACE_DEPTH: usize = 64;

/// A removed surface's captured "old side" for the damage engine.
#[derive(Clone, Debug)]
pub struct RemovalRecord {
    /// The removed surface.
    pub id: SurfaceId,
    /// Output-space bounds at the last damage pass.
    pub last_bounds: Rect,
    /// Opaque-above mask at the last damage pass.
    pub last_occlusion: Region,
    /// Opaque footprint at the last damage pass.
    pub last_opaque_mapped: Region,
}

/// Everything the damage engine needs from this frame's mutations.
#[derive(Debug, Default)]
pub struct FrameChanges {
    /// Restacks that happened since the last damage pass.
    pub restacks: Vec<StackChange>,
    /// Subtrees destroyed since the last damage pass.
    pub removals: Vec<RemovalRecord>,
    /// Commit effects, in application order (inspection; the engine
    /// reads damage off the surfaces directly).
    pub effects: Vec<(SurfaceId, crate::pending::CommitEffect)>,
}

impl FrameChanges {
    /// Whether anything at all happened.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.restacks.is_empty() && self.removals.is_empty() && self.effects.is_empty()
    }
}

/// Tree misuse.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TreeError {
    /// A surface with this id exists already.
    Duplicate,
    /// No surface with this id exists.
    Missing,
    /// The surface already carries the subsurface role (a subsurface
    /// may not take another role).
    AlreadySubsurface,
    /// The restack target is not a sibling (different parents).
    NotASibling,
    /// The parent does not exist (subsurface creation).
    MissingParent,
    /// The subsurface chain under `parent` already sits at the
    /// [`MAX_SUBSURFACE_DEPTH`] cap (a client-authored chain deep
    /// enough to overflow the recursive walks — occlusion, snapshot,
    /// the sync flush — is a crash attempt, not a scene).
    TooDeep,
}

impl std::fmt::Display for TreeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TreeError::Duplicate => f.write_str("surface id already exists"),
            TreeError::Missing => f.write_str("no such surface"),
            TreeError::AlreadySubsurface => {
                f.write_str("surface already carries the subsurface role")
            }
            TreeError::NotASibling => f.write_str("mover and sibling do not share a parent"),
            TreeError::MissingParent => f.write_str("subsurface parent does not exist"),
            TreeError::TooDeep => f.write_str("subsurface nesting depth cap exceeded"),
        }
    }
}

impl std::error::Error for TreeError {}

/// The authoring surface tree.
#[derive(Debug, Default)]
pub struct SurfaceTree {
    surfaces: HashMap<SurfaceId, Surface>,
    children: HashMap<SurfaceId, StackingList>,
    roots: StackingList,
    changes: FrameChanges,
}

impl SurfaceTree {
    /// An empty tree.
    #[must_use]
    pub fn new() -> SurfaceTree {
        SurfaceTree::default()
    }

    /// Create a root surface at an output-space position.
    ///
    /// # Errors
    ///
    /// [`TreeError::Duplicate`] when the id is taken.
    pub fn create_root(&mut self, id: SurfaceId, x: i32, y: i32) -> Result<(), TreeError> {
        if self.surfaces.contains_key(&id) {
            return Err(TreeError::Duplicate);
        }
        self.surfaces
            .insert(id, Surface::new(id, Role::Root { x, y }, (x, y)));
        self.roots.push_front(id);
        Ok(())
    }

    /// Give `id` the subsurface role under `parent` (position 0,0; sync
    /// mode by default). The parent may itself be a subsurface (nested
    /// trees); `id` must be role-free.
    ///
    /// # Errors
    ///
    /// [`TreeError::Duplicate`], [`TreeError::MissingParent`],
    /// [`TreeError::AlreadySubsurface`] when `id` is a subsurface, or
    /// [`TreeError::TooDeep`] when the chain under `parent` already sits
    /// at the depth cap.
    pub fn create_subsurface(&mut self, id: SurfaceId, parent: SurfaceId) -> Result<(), TreeError> {
        if self.surfaces.contains_key(&id) {
            return Err(TreeError::Duplicate);
        }
        let Some(parent_surface) = self.surfaces.get(&parent) else {
            return Err(TreeError::MissingParent);
        };
        // The parent may itself be a subsurface (nested trees are legal);
        // only the child must be role-free, which the duplicate check
        // above guarantees for fresh ids. But the chain must stay inside
        // the depth cap: every tree walk (occlusion's `visit`, the
        // snapshot's capture, the sync-flush recursion) descends the
        // parent chain with one stack frame per level, and a
        // client-authored chain of a few tens of thousands of links
        // turns the next damage pass into a stack overflow — a crash
        // attempt, not a scene. Wayland caps the same abuse with its
        // subsurface depth limits; the cap is a *server policy*, not a
        // protocol change: the refusal rides the same typed error path
        // an unknown parent takes.
        if let Role::Subsurface { parent: up } = parent_surface.role() {
            // Measure the parent's own depth: 1 for being a subsurface,
            // plus one per subsurface ancestor above it (the walk is
            // bounded by the cap itself — every existing chain passed
            // this check — so the measurement is O(depth) <= O(cap)).
            let mut depth = 1usize;
            let mut cursor = up;
            while let Some(s) = self.surfaces.get(&cursor) {
                match s.role() {
                    Role::Subsurface { parent: next } => {
                        depth += 1;
                        cursor = next;
                    }
                    Role::Root { .. } => break,
                }
            }
            // The new link would sit at `depth + 1`; refuse at the cap.
            if depth >= MAX_SUBSURFACE_DEPTH {
                return Err(TreeError::TooDeep);
            }
        }
        self.surfaces
            .insert(id, Surface::new(id, Role::Subsurface { parent }, (0, 0)));
        self.children.entry(parent).or_default().push_front(id);
        Ok(())
    }

    /// The committed-state accessor.
    #[must_use]
    pub fn get(&self, id: SurfaceId) -> Option<&Surface> {
        self.surfaces.get(&id)
    }

    /// The mutable accessor (damage-pass record updates; crate-internal
    /// discipline keeps commits going through [`Self::commit`]).
    #[must_use]
    pub(crate) fn get_mut(&mut self, id: SurfaceId) -> Option<&mut Surface> {
        self.surfaces.get_mut(&id)
    }

    /// The parent of a subsurface (`None` for roots).
    #[must_use]
    pub fn parent_of(&self, id: SurfaceId) -> Option<SurfaceId> {
        match self.surfaces.get(&id)?.role {
            Role::Subsurface { parent } => Some(parent),
            Role::Root { .. } => None,
        }
    }

    /// The back-to-front stacking order of one parent's children
    /// (roots when `parent` is `None`).
    #[must_use]
    pub fn children_of(&self, parent: Option<SurfaceId>) -> &[SurfaceId] {
        match parent {
            None => self.roots.order(),
            Some(p) => self.children.get(&p).map_or(&[][..], StackingList::order),
        }
    }

    /// Every surface, unordered.
    pub fn iter(&self) -> impl Iterator<Item = &Surface> {
        self.surfaces.values()
    }

    /// Number of live surfaces.
    #[must_use]
    pub fn len(&self) -> usize {
        self.surfaces.len()
    }

    /// Whether the tree has no surfaces.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.surfaces.is_empty()
    }

    /// Pending-state access for request-shaped accumulation
    /// (`attach`, `damage`, `set_opaque_region`, …).
    ///
    /// # Errors
    ///
    /// [`TreeError::Missing`] when the id is unknown.
    pub fn pending_mut(
        &mut self,
        id: SurfaceId,
    ) -> Result<&mut crate::pending::PendingState, TreeError> {
        self.surfaces
            .get_mut(&id)
            .map_or(Err(TreeError::Missing), |s| Ok(&mut s.pending))
    }

    /// Queue a position change (applied by the commit cascade).
    ///
    /// # Errors
    ///
    /// [`TreeError::Missing`] when the id is unknown.
    pub fn set_position(&mut self, id: SurfaceId, x: i32, y: i32) -> Result<(), TreeError> {
        let Some(s) = self.surfaces.get_mut(&id) else {
            return Err(TreeError::Missing);
        };
        s.set_position(x, y);
        Ok(())
    }

    /// Apply a position change *now* (Phase 28: the positioning
    /// shell's re-layout arm — migrations and output swaps re-place
    /// mapped roots without waiting for each client's next commit).
    /// The damage engine's R2 coverage rule picks the move up at the
    /// next damage pass (live position versus `last_bounds`), so the
    /// movement repaints correctly; no change record is needed.
    ///
    /// # Errors
    ///
    /// [`TreeError::Missing`] when the id is unknown.
    pub fn set_position_now(&mut self, id: SurfaceId, x: i32, y: i32) -> Result<(), TreeError> {
        let Some(s) = self.surfaces.get_mut(&id) else {
            return Err(TreeError::Missing);
        };
        s.set_position_now(x, y);
        Ok(())
    }

    /// Set the subsurface mode (sync/desync).
    ///
    /// # Errors
    ///
    /// [`TreeError::Missing`] when the id is unknown.
    pub fn set_mode(&mut self, id: SurfaceId, mode: SubsurfaceMode) -> Result<(), TreeError> {
        let Some(s) = self.surfaces.get_mut(&id) else {
            return Err(TreeError::Missing);
        };
        s.mode = mode;
        Ok(())
    }

    /// Restack `mover` immediately above `sibling` (both must share a
    /// parent; the change is recorded for the damage engine).
    ///
    /// # Errors
    ///
    /// [`TreeError::Missing`], or [`TreeError::NotASibling`] when the
    /// parents differ, or the stacking errors from
    /// [`StackingList::place_above`].
    pub fn place_above(&mut self, mover: SurfaceId, sibling: SurfaceId) -> Result<(), TreeError> {
        self.restack(mover, sibling, true)
    }

    /// Restack `mover` immediately below `sibling`.
    ///
    /// # Errors
    ///
    /// As [`Self::place_above`].
    pub fn place_below(&mut self, mover: SurfaceId, sibling: SurfaceId) -> Result<(), TreeError> {
        self.restack(mover, sibling, false)
    }

    fn restack(
        &mut self,
        mover: SurfaceId,
        sibling: SurfaceId,
        above: bool,
    ) -> Result<(), TreeError> {
        if !self.surfaces.contains_key(&mover) || !self.surfaces.contains_key(&sibling) {
            return Err(TreeError::Missing);
        }
        let parent_mover = self.parent_of(mover);
        if parent_mover != self.parent_of(sibling) {
            return Err(TreeError::NotASibling);
        }
        let list = match parent_mover {
            None => &mut self.roots,
            Some(p) => self.children.entry(p).or_default(),
        };
        let change = if above {
            list.place_above(mover, sibling)
        } else {
            list.place_below(mover, sibling)
        };
        match change {
            Ok(Some(record)) => {
                self.changes.restacks.push(record);
                Ok(())
            }
            Ok(None) => Ok(()),                    // null restack
            Err(_) => Err(TreeError::NotASibling), // self-sibling: unreachable here
        }
    }

    /// Commit `id`: apply immediately (roots, desync subsurfaces — then
    /// flush sync descendants) or stash (sync subsurfaces). Returns the
    /// applied effect, or `None` for a stash/null commit.
    ///
    /// # Errors
    ///
    /// [`TreeError::Missing`] when the id is unknown.
    ///
    /// # Panics
    ///
    /// Never through the public API: the internal `get_mut`/`unwrap`
    /// paths are guarded by the `Missing` check above.
    pub fn commit(
        &mut self,
        id: SurfaceId,
    ) -> Result<Option<crate::pending::CommitEffect>, TreeError> {
        let Some(surface) = self.surfaces.get(&id) else {
            return Err(TreeError::Missing);
        };
        if !surface.commits_apply_immediately() {
            self.surfaces.get_mut(&id).unwrap().stash_deferred();
            return Ok(None);
        }
        let dirty = {
            let s = self.surfaces.get(&id).unwrap();
            s.deferred.is_some() || s.pending.is_dirty() || s.position_pending.is_some()
        };
        let effect = dirty.then(|| self.surfaces.get_mut(&id).unwrap().apply_now());
        // Flush sync descendants that carry deferred (committed) state,
        // recursively. Uncommitted pending requests stay with their
        // surface — only the stash and pending positions release here.
        self.flush_sync_children(id);
        Ok(effect)
    }

    /// Apply every sync-mode child of `parent` that carries deferred
    /// state or a pending position, recursing through their own sync
    /// children.
    fn flush_sync_children(&mut self, parent: SurfaceId) {
        let kids: Vec<SurfaceId> = self
            .children
            .get(&parent)
            .map_or(Vec::new(), |l| l.order().to_vec());
        for kid in kids {
            let applies = {
                let Some(s) = self.surfaces.get(&kid) else {
                    continue;
                };
                matches!(s.role, Role::Subsurface { .. })
                    && s.mode == SubsurfaceMode::Sync
                    && s.has_deferred()
            };
            if applies {
                self.surfaces.get_mut(&kid).unwrap().apply_deferred();
                self.flush_sync_children(kid);
            }
        }
    }

    /// Destroy `id` and its whole subtree, capturing removal records
    /// for the damage engine. Returns the destroyed ids in
    /// parent-before-child order.
    ///
    /// # Errors
    ///
    /// [`TreeError::Missing`] when the id is unknown.
    pub fn destroy(&mut self, id: SurfaceId) -> Result<Vec<SurfaceId>, TreeError> {
        if !self.surfaces.contains_key(&id) {
            return Err(TreeError::Missing);
        }
        let mut destroyed = Vec::new();
        let parent = self.parent_of(id);
        match parent {
            None => {
                self.roots.remove(id);
            }
            Some(p) => {
                if let Some(list) = self.children.get_mut(&p) {
                    list.remove(id);
                }
            }
        }
        self.destroy_subtree(id, &mut destroyed);
        Ok(destroyed)
    }

    fn destroy_subtree(&mut self, id: SurfaceId, destroyed: &mut Vec<SurfaceId>) {
        let kids: Vec<SurfaceId> = self
            .children
            .get(&id)
            .map_or(Vec::new(), |l| l.order().to_vec());
        if let Some(surface) = self.surfaces.remove(&id) {
            self.changes.removals.push(RemovalRecord {
                id,
                last_bounds: surface.last_bounds,
                last_occlusion: surface.last_occlusion,
                last_opaque_mapped: surface.last_opaque_mapped,
            });
            destroyed.push(id);
        }
        self.children.remove(&id);
        for kid in kids {
            self.destroy_subtree(kid, destroyed);
        }
    }

    /// Take the frame's change records (restacks, removals, effects)
    /// for the damage pass.
    #[must_use]
    pub fn take_changes(&mut self) -> FrameChanges {
        std::mem::take(&mut self.changes)
    }
}
