//! Occlusion: the front-to-back walk over the flattened tree.
//!
//! [`flatten`] produces the front-to-back node list (children render
//! above their parents; later siblings above earlier ones), each node
//! carrying its output-space geometry: the accumulated position offset,
//! the mapped bounds, and the mapped opaque footprint (the opaque
//! region translated to output space and clipped to the node's own
//! bounds — an opaque guarantee outside the buffer covers nothing).
//!
//! [`visible_regions`] runs the walk: processing nodes front-to-back
//! with a running opaque mask, each surface's *visible* region is its
//! mapped bounds minus the opaque regions of everything stacked above
//! it — exactly the region where it contributes to the composited
//! result. This is the shared primitive behind damage subtraction,
//! presentation feedback, and (later) input picking.

use std::collections::HashMap;

use ldp_core::geometry::{Rect, Region};

use crate::surface::SurfaceId;
use crate::tree::SurfaceTree;

/// One flattened tree node with its output-space geometry.
#[derive(Clone, Debug)]
pub struct FlatNode {
    /// The surface.
    pub id: SurfaceId,
    /// Accumulated position offset in output coordinates.
    pub offset: (i32, i32),
    /// Surface-local bounds of the current mapping.
    pub local_bounds: Rect,
    /// [`Self::local_bounds`] translated to output space.
    pub mapped_bounds: Rect,
    /// The opaque region translated to output space and clipped to the
    /// mapped bounds.
    pub opaque_mapped: Region,
}

/// The flattened tree: the front-to-back node list plus each node's
/// subtree extent in that list.
///
/// Subtree extents matter for restack damage: a restack moves whole
/// subtrees, so "the pixels that crossed" are subtree footprints, not
/// single-surface bounds — and the flush point is the first node the
/// walk visits inside the front-most participant's subtree.
#[derive(Clone, Debug)]
pub struct FlatTree {
    /// Front-to-back nodes.
    pub nodes: Vec<FlatNode>,
    /// Per surface: `(first_index_of_subtree, own_index)` in `nodes`.
    pub subtree: HashMap<SurfaceId, (usize, usize)>,
}

/// Flatten the tree front-to-back.
///
/// Order: for each root (last root first), the subtree's children
/// (front-most sibling first) recursively *before* the parent —
/// subsurfaces render above their parents by construction.
#[must_use]
pub fn flatten(tree: &SurfaceTree) -> FlatTree {
    let mut out = FlatTree {
        nodes: Vec::new(),
        subtree: HashMap::new(),
    };
    let roots: Vec<SurfaceId> = tree.children_of(None).to_vec();
    for root in roots.iter().rev() {
        visit(tree, *root, None, &mut out);
    }
    out
}

fn visit(tree: &SurfaceTree, id: SurfaceId, parent_offset: Option<(i32, i32)>, out: &mut FlatTree) {
    let Some(surface) = tree.get(id) else {
        return;
    };
    // Roots carry absolute output positions; subsurface positions are
    // parent-surface coordinates that compose linearly down the chain
    // (v1: parent transforms map the parent's own content, not the
    // child coordinate space).
    let own = surface.position();
    let offset = match parent_offset {
        None => own,
        // Saturating, not wrapping: subsurface positions are raw
        // client ints that accumulate down a chain, and a client that
        // parks `i32::MAX` twice must not panic the walk (debug) or
        // silently wrap to a negative offset (release). The very next
        // line maps bounds through `Rect::translate`, which saturates
        // by construction — the accumulation owes the same discipline.
        Some(p) => (p.0.saturating_add(own.0), p.1.saturating_add(own.1)),
    };
    let local_bounds = surface.state().bounds();
    let mapped_bounds = local_bounds.translate(offset.0, offset.1);
    let opaque_mapped = surface
        .state()
        .opaque_region
        .translate(offset.0, offset.1)
        .clipped_to(mapped_bounds);
    let children: Vec<SurfaceId> = tree.children_of(Some(id)).to_vec();
    let start = out.nodes.len();
    for child in children.iter().rev() {
        visit(tree, *child, Some(offset), out);
    }
    let own_index = out.nodes.len();
    out.nodes.push(FlatNode {
        id,
        offset,
        local_bounds,
        mapped_bounds,
        opaque_mapped,
    });
    out.subtree.insert(id, (start, own_index));
}

/// The per-surface occlusion result of one walk position.
#[derive(Clone, Debug)]
pub struct Occluded {
    /// The surface.
    pub id: SurfaceId,
    /// Mapped bounds in output space.
    pub mapped_bounds: Rect,
    /// Opaque regions of everything above (output space).
    pub occlusion: Region,
    /// `mapped_bounds` minus the occlusion — where the surface
    /// contributes to the composited result.
    pub visible: Region,
}

/// Run the front-to-back walk, producing each surface's occlusion and
/// visible region (see the module docs).
///
/// The walk state is exposed as a slice so damage passes can replay it
/// incrementally (the engine flushes restack records between nodes).
#[must_use]
pub fn visible_regions(nodes: &[FlatNode]) -> Vec<Occluded> {
    let mut mask = Region::new();
    let mut out = Vec::with_capacity(nodes.len());
    for node in nodes {
        let visible = Region::from_rect(node.mapped_bounds).subtract(&mask);
        out.push(Occluded {
            id: node.id,
            mapped_bounds: node.mapped_bounds,
            occlusion: mask.clone(),
            visible: visible.clone(),
        });
        mask.add_region(&node.opaque_mapped);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::BufferAttachment;

    fn tree_with(ops: fn(&mut SurfaceTree)) -> SurfaceTree {
        let mut t = SurfaceTree::new();
        ops(&mut t);
        t
    }

    #[test]
    fn flatten_orders_front_to_back() {
        let t = tree_with(|t| {
            t.create_root(SurfaceId::from_raw(1), 0, 0).unwrap();
            t.create_root(SurfaceId::from_raw(2), 0, 0).unwrap();
            t.create_subsurface(SurfaceId::from_raw(3), SurfaceId::from_raw(1))
                .unwrap();
        });
        let flat = flatten(&t).nodes;
        let ids: Vec<u64> = flat.iter().map(|n| n.id.raw()).collect();
        // Root 2 is front-most among roots; within root 1's family the
        // subsurface (3) renders above its parent.
        assert_eq!(ids, vec![2, 3, 1]);
    }

    #[test]
    fn offsets_accumulate_down_the_chain() {
        let t = tree_with(|t| {
            t.create_root(SurfaceId::from_raw(1), 100, 200).unwrap();
            t.create_subsurface(SurfaceId::from_raw(2), SurfaceId::from_raw(1))
                .unwrap();
            t.set_position(SurfaceId::from_raw(2), 10, 20).unwrap();
            t.commit(SurfaceId::from_raw(1)).unwrap(); // applies position
        });
        let flat = flatten(&t).nodes;
        let child = flat.iter().find(|n| n.id.raw() == 2).unwrap();
        assert_eq!(child.offset, (110, 220));
    }

    #[test]
    fn unattached_surfaces_have_empty_geometry() {
        let t = tree_with(|t| {
            t.create_root(SurfaceId::from_raw(1), 5, 5).unwrap();
        });
        let flat = flatten(&t).nodes;
        assert!(flat[0].mapped_bounds.is_empty());
        assert!(flat[0].opaque_mapped.is_empty());
    }

    #[test]
    fn opaque_footprints_clip_to_bounds() {
        let t = tree_with(|t| {
            t.create_root(SurfaceId::from_raw(1), 0, 0).unwrap();
            t.pending_mut(SurfaceId::from_raw(1))
                .unwrap()
                .attach(Some(BufferAttachment {
                    width: 10,
                    height: 10,
                    generation: 1,
                }));
            let mut opaque = Region::new();
            opaque.add(Rect::new(0, 0, 40, 40)); // far beyond the buffer
            t.pending_mut(SurfaceId::from_raw(1))
                .unwrap()
                .set_opaque_region(opaque);
            t.commit(SurfaceId::from_raw(1)).unwrap();
        });
        let flat = flatten(&t).nodes;
        assert_eq!(flat[0].opaque_mapped.bounds(), Rect::new(0, 0, 10, 10));
    }

    #[test]
    fn occlusion_subtracts_opaque_above() {
        let t = tree_with(|t| {
            // Front: 20x20 opaque at (0,0). Back: 20x20 at (0,0).
            t.create_root(SurfaceId::from_raw(1), 0, 0).unwrap();
            t.create_root(SurfaceId::from_raw(2), 0, 0).unwrap();
            for i in [1u64, 2] {
                t.pending_mut(SurfaceId::from_raw(i))
                    .unwrap()
                    .attach(Some(BufferAttachment {
                        width: 20,
                        height: 20,
                        generation: 1,
                    }));
                t.pending_mut(SurfaceId::from_raw(i))
                    .unwrap()
                    .set_opaque_region(Region::from_rect(Rect::new(0, 0, 20, 20)));
            }
            // Only the front surface (2, committed last) stays opaque; make
            // the back one translucent by clearing via a fresh commit.
            t.commit(SurfaceId::from_raw(1)).unwrap();
            t.pending_mut(SurfaceId::from_raw(2))
                .unwrap()
                .set_opaque_region(Region::from_rect(Rect::new(0, 0, 20, 20)));
            t.commit(SurfaceId::from_raw(2)).unwrap();
        });
        let flat = flatten(&t).nodes;
        let occ = visible_regions(&flat);
        // Front surface fully visible.
        let front = occ.iter().find(|o| o.id.raw() == 2).unwrap();
        assert_eq!(front.visible.bounds(), Rect::new(0, 0, 20, 20));
        assert!(front.occlusion.is_empty());
        // Back surface fully occluded.
        let back = occ.iter().find(|o| o.id.raw() == 1).unwrap();
        assert!(back.visible.is_empty());
    }

    #[test]
    fn partial_occlusion_leaves_visible_slice() {
        let t = tree_with(|t| {
            t.create_root(SurfaceId::from_raw(1), 0, 0).unwrap(); // back, 20x30
            t.create_root(SurfaceId::from_raw(2), 0, 0).unwrap(); // front, 20x10
            t.pending_mut(SurfaceId::from_raw(1))
                .unwrap()
                .attach(Some(BufferAttachment {
                    width: 20,
                    height: 30,
                    generation: 1,
                }));
            t.commit(SurfaceId::from_raw(1)).unwrap();
            t.pending_mut(SurfaceId::from_raw(2))
                .unwrap()
                .attach(Some(BufferAttachment {
                    width: 20,
                    height: 10,
                    generation: 1,
                }));
            t.pending_mut(SurfaceId::from_raw(2))
                .unwrap()
                .set_opaque_region(Region::from_rect(Rect::new(0, 0, 20, 10)));
            t.commit(SurfaceId::from_raw(2)).unwrap();
        });
        let flat = flatten(&t).nodes;
        let occ = visible_regions(&flat);
        let back = occ.iter().find(|o| o.id.raw() == 1).unwrap();
        assert_eq!(back.visible.bounds(), Rect::new(0, 10, 20, 20));
    }

    #[test]
    fn translucent_front_does_not_occlude() {
        let t = tree_with(|t| {
            t.create_root(SurfaceId::from_raw(1), 0, 0).unwrap();
            t.create_root(SurfaceId::from_raw(2), 0, 0).unwrap();
            for i in [1u64, 2] {
                t.pending_mut(SurfaceId::from_raw(i))
                    .unwrap()
                    .attach(Some(BufferAttachment {
                        width: 10,
                        height: 10,
                        generation: 1,
                    }));
                t.commit(SurfaceId::from_raw(i)).unwrap(); // no opaque region
            }
        });
        let flat = flatten(&t).nodes;
        let occ = visible_regions(&flat);
        assert!(occ
            .iter()
            .all(|o| o.visible.bounds() == Rect::new(0, 0, 10, 10)));
    }
}
