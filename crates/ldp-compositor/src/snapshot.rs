//! Immutable per-frame snapshots: the render path's view of the tree.
//!
//! [`SurfaceTree::snapshot`] captures the committed state at a frame
//! boundary into a [`FrameSnapshot`] — an owned tree of
//! `Arc`-shared [`SnapshotNode`]s. Structural sharing
//! keeps captures cheap (unchanged subtrees are shared pointers), and
//! the immutability contract is structural: later mutations of the
//! authoring tree can never reach a captured snapshot, so the render
//! thread reads it without synchronization.
//!
//! Each snapshot carries a monotonically increasing [`FrameId`] and the
//! capture's flat back-to-front order (the render list). Invariants
//! asserted at capture time and re-checkable via [`FrameSnapshot::check`]:
//! acyclicity (implied by construction from the tree), parent-before-
//! child consistency in the node tree, and a flat order that is a
//! permutation of exactly the tree's surfaces.

use std::collections::HashMap;
use std::sync::Arc;

use ldp_core::color::{ColorDescription, HdrMetadata};
use ldp_core::geometry::{Rect, Region, Transform};
use ldp_core::scale::ScaleFactor;
use ldp_core::time::PresentationMode;

use crate::surface::SurfaceId;
use crate::tree::SurfaceTree;

/// Monotonic frame counter of a snapshot lineage.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub struct FrameId(pub u64);

/// One node of a snapshot: the committed state, frozen.
#[derive(Clone, Debug)]
pub struct SnapshotNode {
    /// The surface's identity.
    pub id: SurfaceId,
    /// Output-space bounds of the current mapping.
    pub bounds: Rect,
    /// Accumulated offset in output coordinates.
    pub offset: (i32, i32),
    /// The buffer-to-surface transform.
    pub transform: Transform,
    /// The buffer scale.
    pub scale: ScaleFactor,
    /// The opaque region (surface coordinates).
    pub opaque_region: Region,
    /// The input region (surface coordinates).
    pub input_region: Region,
    /// The color description.
    pub color: ColorDescription,
    /// The HDR metadata, when present.
    pub hdr: Option<HdrMetadata>,
    /// The presentation mode.
    pub presentation: PresentationMode,
    /// Whether a buffer is attached (mapped).
    pub mapped: bool,
    /// The children (back-to-front order), structurally shared.
    pub children: Vec<Arc<SnapshotNode>>,
}

impl SnapshotNode {
    /// Every node in the subtree, parent before child, in
    /// back-to-front child order.
    pub fn walk(&self, out: &mut Vec<SurfaceId>) {
        out.push(self.id);
        for c in &self.children {
            c.walk(out);
        }
    }
}

/// An immutable capture of the tree at a frame boundary.
#[derive(Clone, Debug)]
pub struct FrameSnapshot {
    /// The capture's frame identity (monotonic per tree).
    pub frame: FrameId,
    /// The root nodes, back-to-front order.
    pub roots: Vec<Arc<SnapshotNode>>,
    /// Every node by id (flat index).
    pub by_id: HashMap<SurfaceId, Arc<SnapshotNode>>,
    /// The flat back-to-front render list (draw order): parents before
    /// their children (subsurfaces render above parents), siblings in
    /// stacking order, roots in stacking order.
    pub render_order: Vec<SurfaceId>,
}

impl SurfaceTree {
    /// Capture the committed state as an immutable snapshot.
    #[must_use]
    pub fn snapshot(&mut self) -> FrameSnapshot {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let frame = FrameId(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed));

        let roots: Vec<Arc<SnapshotNode>> = self
            .children_of(None)
            .iter()
            .map(|r| capture(self, *r))
            .collect();

        // Flat render order: back-to-front draw order. A parent draws
        // before its children (subsurfaces render above it), and
        // siblings draw in their stacking order — exactly the reverse
        // of the damage engine's front-to-back flatten.
        let mut render_order = Vec::new();
        for r in &roots {
            push_back_to_front(r, &mut render_order);
        }

        let mut by_id = HashMap::with_capacity(render_order.len());
        for r in &roots {
            index(r, &mut by_id);
        }

        FrameSnapshot {
            frame,
            roots,
            by_id,
            render_order,
        }
    }
}

/// Recursively capture one subtree as shared snapshot nodes.
fn capture(tree: &SurfaceTree, id: SurfaceId) -> Arc<SnapshotNode> {
    let surface = tree
        .get(id)
        .expect("stacking lists only contain live surfaces");
    let state = surface.state();
    let children: Vec<Arc<SnapshotNode>> = tree
        .children_of(Some(id))
        .iter()
        .map(|c| capture(tree, *c))
        .collect();
    Arc::new(SnapshotNode {
        id,
        bounds: state
            .bounds()
            .translate(surface.position().0, surface.position().1),
        offset: surface.position(),
        transform: state.transform,
        scale: state.scale,
        opaque_region: state.opaque_region.clone(),
        input_region: state.input_region.clone(),
        color: state.color,
        hdr: state.hdr,
        presentation: state.presentation,
        mapped: state.is_mapped(),
        children,
    })
}

/// Append a subtree's ids in back-to-front draw order (parents first).
fn push_back_to_front(node: &SnapshotNode, out: &mut Vec<SurfaceId>) {
    out.push(node.id);
    for c in &node.children {
        push_back_to_front(c, out);
    }
}

/// Index a subtree by surface id (structural sharing: one `Arc` clone
/// per node).
fn index(node: &Arc<SnapshotNode>, by_id: &mut HashMap<SurfaceId, Arc<SnapshotNode>>) {
    by_id.insert(node.id, Arc::clone(node));
    for c in &node.children {
        index(c, by_id);
    }
}

/// Check that every child draws strictly after its parent.
fn check_order(
    node: &SnapshotNode,
    position: &HashMap<SurfaceId, usize>,
) -> Result<(), SnapshotError> {
    let here = position[&node.id];
    for c in &node.children {
        if position[&c.id] <= here {
            return Err(SnapshotError::ChildBeforeParent(c.id));
        }
        check_order(c, position)?;
    }
    Ok(())
}

impl FrameSnapshot {
    /// Re-check the structural invariants (test/audit hook):
    ///
    /// * every node appears exactly once in `render_order` and `by_id`,
    /// * `render_order` is a permutation of the node set,
    /// * children precede parents in `render_order`.
    ///
    /// # Errors
    ///
    /// [`SnapshotError`] naming the first violated invariant.
    pub fn check(&self) -> Result<(), SnapshotError> {
        let mut seen = std::collections::HashSet::new();
        for id in &self.render_order {
            if !seen.insert(*id) {
                return Err(SnapshotError::Duplicate(*id));
            }
        }
        if seen.len() != self.by_id.len() {
            return Err(SnapshotError::IndexMismatch);
        }
        // Parents before children: a parent draws before its children
        // (subsurfaces render above parents), so every child must come
        // strictly after its parent in the render list.
        let position: HashMap<SurfaceId, usize> = self
            .render_order
            .iter()
            .enumerate()
            .map(|(i, id)| (*id, i))
            .collect();
        for r in &self.roots {
            check_order(r, &position)?;
        }
        Ok(())
    }

    /// The back-to-front draw list (parents before children).
    #[must_use]
    pub fn render_order(&self) -> &[SurfaceId] {
        &self.render_order
    }

    /// One node by surface id.
    #[must_use]
    pub fn node(&self, id: SurfaceId) -> Option<&Arc<SnapshotNode>> {
        self.by_id.get(&id)
    }
}

/// A violated snapshot invariant.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SnapshotError {
    /// A surface appears twice in the render order.
    Duplicate(SurfaceId),
    /// The by-id index does not match the render order.
    IndexMismatch,
    /// A child renders before its parent in the draw list.
    ChildBeforeParent(SurfaceId),
}

impl std::fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SnapshotError::Duplicate(id) => write!(f, "{id} appears twice in the render order"),
            SnapshotError::IndexMismatch => {
                f.write_str("by-id index disagrees with the render order")
            }
            SnapshotError::ChildBeforeParent(id) => {
                write!(f, "child {id} renders before its parent")
            }
        }
    }
}

impl std::error::Error for SnapshotError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::BufferAttachment;

    fn id(n: u64) -> SurfaceId {
        SurfaceId::from_raw(n)
    }

    #[test]
    fn snapshots_are_stable_under_later_mutation() {
        let mut t = SurfaceTree::new();
        t.create_root(id(1), 0, 0).unwrap();
        t.create_subsurface(id(2), id(1)).unwrap();
        t.pending_mut(id(1)).unwrap().attach(Some(BufferAttachment {
            width: 10,
            height: 10,
            generation: 1,
        }));
        t.commit(id(1)).unwrap();
        let s1 = t.snapshot();
        assert_eq!(s1.by_id.len(), 2);
        assert_eq!(s1.node(id(1)).unwrap().bounds, Rect::new(0, 0, 10, 10));
        // Mutate the tree heavily.
        t.pending_mut(id(1)).unwrap().attach(Some(BufferAttachment {
            width: 99,
            height: 99,
            generation: 2,
        }));
        t.commit(id(1)).unwrap();
        t.destroy(id(2)).unwrap();
        t.create_root(id(3), 50, 50).unwrap();
        // The old snapshot is untouched (structural immutability).
        assert_eq!(s1.by_id.len(), 2);
        assert_eq!(s1.node(id(1)).unwrap().bounds, Rect::new(0, 0, 10, 10));
        assert!(s1.node(id(2)).is_some());
        assert!(s1.node(id(3)).is_none());
        assert!(s1.check().is_ok());
        // The new snapshot reflects the mutations.
        let s2 = t.snapshot();
        assert_eq!(s2.by_id.len(), 2);
        assert_eq!(s2.node(id(1)).unwrap().bounds, Rect::new(0, 0, 99, 99));
        assert!(s2.node(id(2)).is_none());
        assert!(s2.node(id(3)).is_some());
        assert!(s2.check().is_ok());
        assert!(s2.frame.0 > s1.frame.0);
    }

    #[test]
    fn render_order_puts_parents_before_children() {
        let mut t = SurfaceTree::new();
        t.create_root(id(1), 0, 0).unwrap();
        t.create_root(id(2), 0, 0).unwrap();
        t.create_subsurface(id(3), id(1)).unwrap();
        t.create_subsurface(id(4), id(3)).unwrap();
        let s = t.snapshot();
        // Back-to-front: root 1 draws first, then its subsurface 3 and
        // 3's child 4 (above 3), then the front-most root 2.
        assert_eq!(
            s.render_order.iter().map(|s| s.raw()).collect::<Vec<_>>(),
            vec![1, 3, 4, 2]
        );
        assert!(s.check().is_ok());
    }

    #[test]
    fn sharing_keeps_old_snapshots_alive_across_edits() {
        let mut t = SurfaceTree::new();
        t.create_root(id(1), 0, 0).unwrap();
        t.create_subsurface(id(2), id(1)).unwrap();
        let s1 = t.snapshot();
        let child_arc = Arc::clone(s1.node(id(2)).unwrap());
        // Destroy the subtree in the authoring tree.
        t.destroy(id(1)).unwrap();
        assert!(t.is_empty());
        // The snapshot's Arc keeps the node alive and unchanged.
        assert_eq!(child_arc.id, id(2));
        assert!(child_arc.children.is_empty());
        assert_eq!(s1.by_id.len(), 2);
    }

    #[test]
    fn unmapped_surfaces_snapshot_as_unmapped() {
        let mut t = SurfaceTree::new();
        t.create_root(id(1), 0, 0).unwrap();
        let s = t.snapshot();
        assert!(!s.node(id(1)).unwrap().mapped);
        assert_eq!(s.node(id(1)).unwrap().bounds, Rect::EMPTY);
    }
}
