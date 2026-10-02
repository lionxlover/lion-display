//! The damage engine: exact repaint/presentation/scanout damage from
//! the tree plus the frame's change records.
//!
//! # Derivation (normative model)
//!
//! Per output pixel, the composited value is a bottom-to-top fold over
//! the covering surfaces; a translucent surface contributes, an opaque
//! surface (per its guarantee) wipes everything below it. A pixel must
//! be repainted iff its fold value changed between the last damage pass
//! and now. Four rule families realize that exactly:
//!
//! * **Content** (R1): a surface's accumulated surface-local damage,
//!   translated to output space and clipped to its current bounds,
//!   minus the opaque regions above it *now* (an occluded change is
//!   invisible). The same region is the per-surface *presentation*
//!   damage — the visible change the client is told about.
//! * **Coverage** (R2): cells the surface stopped covering (old mapped
//!   bounds minus new) are repainted unless they were occluded *then*
//!   (the old occlusion, recorded at the last pass); cells it newly
//!   covers are repainted unless occluded *now*. This one rule covers
//!   position changes, geometry changes, first maps, and detaches.
//! * **Opaque flips** (R3): cells where the surface's opaque guarantee
//!   flipped (old footprint ∆ new footprint, each mapped with its own
//!   geometry) are repainted unless occluded now — a flip changes both
//!   the surface's own contribution and the visibility of everything
//!   below it.
//! * **Restack pairs** (R4): when a surface crosses siblings, cells
//!   covered by *both* the mover and a crossed sibling change their
//!   fold (relative order flipped), unless an opaque surface above the
//!   whole crossed range wipes them — evaluated when the front-to-back
//!   walk reaches the front-most surviving participant.
//!
//! Removed subtrees: the old mapped bounds minus the old occlusion (a
//! removal is the coverage rule with an empty new side).
//!
//! Everything is finally clipped to the output bounds. The randomized
//! corpus (`tests/corpus.rs`) checks equality against an independent
//! per-pixel implementation of the fold model.

use std::collections::HashMap;

use ldp_core::geometry::{Rect, Region, Transform};

use crate::occlusion::flatten;
use crate::surface::{Role, SurfaceId};
use crate::tree::{FrameChanges, SurfaceTree};

/// The three damage classes of one frame (`docs/architecture.md` §10.2).
#[derive(Debug, Default)]
pub struct FrameDamage {
    /// What the renderer must redraw this frame (output coordinates).
    pub repaint: Region,
    /// The repaint subset that lies inside direct-scanout candidates
    /// (fully-opaque, untransformed, unoccluded, fully-on-screen root
    /// surfaces) — the overlay/scanout plane update set.
    pub scanout: Region,
    /// Per-surface *visible* content change (output coordinates) — the
    /// presentation feedback class: what the `presented` accounting
    /// attributes to each client.
    pub presentation: HashMap<SurfaceId, Region>,
    /// Per-surface visible region (mapped bounds minus occlusion) —
    /// the reference data for presentation and input semantics.
    pub visible: HashMap<SurfaceId, Region>,
}

impl FrameDamage {
    /// Whether the frame needs any work at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.repaint.is_empty() && self.presentation.values().all(Region::is_empty)
    }
}

/// The damage engine: a pure function of the tree, the frame's change
/// records, and the output bounds.
#[derive(Debug, Default)]
pub struct DamageEngine;

impl DamageEngine {
    /// Compute one frame's damage from the tree's change records (see
    /// [`SurfaceTree::take_changes`]), updating each surface's pass
    /// records (mapped bounds, occlusion, opaque footprint — the next
    /// frame's "old" side) and clearing the accumulated per-surface
    /// damage.
    ///
    /// # Panics
    ///
    /// Panics if the tree is internally inconsistent (a stacking list
    /// referencing a surface the tree does not store) — a caller-side
    /// invariant violation, never reachable through the public API.
    pub fn compute(tree: &mut SurfaceTree, changes: &FrameChanges, output: Rect) -> FrameDamage {
        let flat = flatten(tree);
        let flushes = Self::flush_points(&flat, changes);
        let mut out = Self::walk(tree, &flat, &flushes);

        // ---- removals: coverage with an empty new side ---------------
        for r in &changes.removals {
            out.repaint
                .add_region(&Region::from_rect(r.last_bounds).subtract(&r.last_occlusion));
        }

        // ---- clip to the output --------------------------------------
        out.repaint = std::mem::take(&mut out.repaint).clipped_to(output);
        for region in out.presentation.values_mut() {
            *region = std::mem::take(region).clipped_to(output);
        }
        for region in out.visible.values_mut() {
            *region = std::mem::take(region).clipped_to(output);
        }

        // ---- scanout damage -------------------------------------------
        let candidates = Self::scanout_candidates(tree, &flat, output, &out.fully_visible);
        let scanout = out
            .repaint
            .subtract(&Region::from_rect(output).subtract(&candidates));

        // ---- commit the pass records ----------------------------------
        for (id, bounds, occlusion, opaque) in out.records {
            if let Some(surface) = tree.get_mut(id) {
                surface.record_pass(bounds, occlusion, opaque);
            }
        }

        FrameDamage {
            repaint: out.repaint,
            scanout,
            presentation: out.presentation,
            visible: out.visible,
        }
    }

    /// R4 precompute: restack pair regions keyed by the flat index of
    /// the first node of the front-most surviving participant's subtree
    /// (the flush point; the mask there is the opaque set strictly above
    /// the whole crossed range). Destroyed participants are pruned —
    /// their removal records subsume the pair cells.
    fn flush_points(
        flat: &crate::occlusion::FlatTree,
        changes: &FrameChanges,
    ) -> HashMap<usize, Region> {
        let subtree_union = |id: SurfaceId| -> Option<Region> {
            flat.subtree.get(&id).map(|&(start, end)| {
                let mut u = Region::new();
                for n in &flat.nodes[start..=end] {
                    u.add(n.mapped_bounds);
                }
                u
            })
        };
        let mut flushes: HashMap<usize, Region> = HashMap::new();
        for record in &changes.restacks {
            let Some(moved_union) = subtree_union(record.moved) else {
                continue; // destroyed mover: its removal subsumes the pairs
            };
            let mut crossed_union = Region::new();
            for x in &record.crossed {
                if let Some(u) = subtree_union(*x) {
                    crossed_union.add_region(&u);
                }
            }
            let pair = intersect_regions(&crossed_union, &moved_union);
            if pair.is_empty() {
                continue;
            }
            let key = std::iter::once(record.moved)
                .chain(record.crossed.iter().copied())
                .filter_map(|p| flat.subtree.get(&p).map(|&(s, _)| s))
                .min();
            if let Some(start) = key {
                flushes.entry(start).or_default().add_region(&pair);
            }
        }
        flushes
    }

    /// Suffix unions *at the nodes that need them*: R3 consumes the
    /// union of everything below a node only where that node's opaque
    /// footprint flipped this frame. The v0.11 pass materialized the
    /// full suffix table — one deep region clone per node per frame,
    /// O(n²) rect copies on a quiet desktop whose flips number zero.
    /// This pass snapshots the same unions (same order, same content)
    /// at exactly the flip indices: one backward accumulation pass,
    /// O(n) rect adds, one clone per flip.
    fn below_unions(
        tree: &SurfaceTree,
        flat: &crate::occlusion::FlatTree,
    ) -> HashMap<usize, Region> {
        let mut flips: Vec<bool> = vec![false; flat.nodes.len()];
        let mut any = false;
        for (i, node) in flat.nodes.iter().enumerate() {
            if let Some(surface) = tree.get(node.id) {
                // The R3 fast path's own test: a footprint that did not
                // move contributes no flip damage and needs no below.
                if surface.last_opaque_mapped != node.opaque_mapped {
                    flips[i] = true;
                    any = true;
                }
            }
        }
        let mut below: HashMap<usize, Region> = HashMap::new();
        if !any {
            return below;
        }
        let mut acc = Region::new();
        for i in (0..flat.nodes.len()).rev() {
            // Loop invariant: `acc` holds the union of nodes[i+1..] —
            // exactly the old `suffix[i + 1]` (the backward-built order
            // the table produced).
            if flips[i] {
                below.insert(i + 1, acc.clone());
            }
            acc.add(flat.nodes[i].mapped_bounds);
        }
        below
    }

    /// The front-to-back walk applying R1–R4 per node.
    fn walk(
        tree: &SurfaceTree,
        flat: &crate::occlusion::FlatTree,
        flushes: &HashMap<usize, Region>,
    ) -> WalkOutput {
        let below_at = Self::below_unions(tree, flat);
        let mut out = WalkOutput::default();
        let mut mask = Region::new();
        for (node_index, node) in flat.nodes.iter().enumerate() {
            let surface = tree.get(node.id).expect("flatten only emits live surfaces");

            // R4 flush: pair damage through the current mask.
            if let Some(pair) = flushes.get(&node_index) {
                out.repaint.add_region(&pair.subtract(&mask));
            }

            // R1 content: accumulated damage, translated, clipped to the
            // current bounds, minus the current occlusion.
            let content = surface
                .frame_damage()
                .translate(node.offset.0, node.offset.1)
                .clipped_to(node.mapped_bounds);
            let visible_content = content.subtract(&mask);
            if !visible_content.is_empty() {
                out.presentation.insert(node.id, visible_content.clone());
                out.repaint.add_region(&visible_content);
            }

            // R2 coverage. The old-only cells repaint through the old
            // occlusion; the new-only cells through the current one;
            // and when the *offset* changed, the interior (covered in
            // both frames) repaints too — every interior cell now shows
            // a *different* content cell (shifted) — minus the cells
            // occluded in both frames (the value is the occluder's
            // either way). With the offset unchanged, interior cells
            // keep showing the same content cell and stay clean.
            //
            // The quiet-frame fast path: a surface whose bounds did
            // not move since the last pass contributes nothing to R2 by
            // construction (old-only, new-only, and the offset test all
            // collapse to the empty region), so the algebra below runs
            // only for surfaces that actually moved or resized — every
            // static window on a quiet desktop skips it whole, which
            // is the difference between four region allocations per
            // window per frame and none.
            if surface.last_bounds != node.mapped_bounds {
                let offset_changed = surface.last_bounds.x != node.mapped_bounds.x
                    || surface.last_bounds.y != node.mapped_bounds.y;
                let old_only =
                    Region::from_rect(surface.last_bounds).subtract_rect(node.mapped_bounds);
                let new_only =
                    Region::from_rect(node.mapped_bounds).subtract_rect(surface.last_bounds);
                out.repaint
                    .add_region(&old_only.subtract(surface.last_occlusion()));
                out.repaint.add_region(&new_only.subtract(&mask));
                if offset_changed {
                    let interior =
                        Region::from_rect(surface.last_bounds).clipped_to(node.mapped_bounds);
                    let both_occluded = intersect_regions(surface.last_occlusion(), &mask);
                    out.repaint.add_region(&interior.subtract(&both_occluded));
                }
            }

            // R3 opaque flips: old footprint ∆ new footprint, each with
            // its own geometry, minus the current occlusion — and only
            // where a surface below can be revealed or hidden (the flip
            // alone does not change a fold of one).
            //
            // The same fast path: equal footprints have no symmetric
            // difference, and the region equality is a cheap
            // same-length scan. The `below` union is only materialized
            // at the (rare) flip nodes — see [`Self::below_unions`].
            if surface.last_opaque_mapped != node.opaque_mapped {
                let shrink = surface.last_opaque_mapped.subtract(&node.opaque_mapped);
                let grow = node.opaque_mapped.subtract(&surface.last_opaque_mapped);
                let below = below_at
                    .get(&(node_index + 1))
                    .expect("flip nodes have a below union");
                out.repaint
                    .add_region(&intersect_regions(&shrink.subtract(&mask), below));
                out.repaint
                    .add_region(&intersect_regions(&grow.subtract(&mask), below));
            }

            // Visible region + scanout candidacy.
            let node_visible = Region::from_rect(node.mapped_bounds).subtract(&mask);
            // Fully visible iff the visible region *is* the bounds: the
            // subtract either passed the single bounds rect through
            // untouched (nothing above intersects it) or carved it — a
            // one-rect comparison the old round-trip (bounds minus
            // visible, then an emptiness check) needed three region
            // allocations to ask.
            let fully_visible = !node.mapped_bounds.is_empty()
                && node_visible.len() == 1
                && node_visible.iter().next() == Some(&node.mapped_bounds);
            if fully_visible {
                out.fully_visible.push(node.id);
            }
            out.visible.insert(node.id, node_visible);

            // Pass records: the next frame's "old" side. A record that
            // would not change a field *and* belongs to a quiescent
            // surface (no accumulated damage for `record_pass` to
            // consume) is a no-op — skipping its clone keeps a quiet
            // desktop allocation-free per window instead of cloning
            // the occlusion mask once per window per frame.
            if surface.last_bounds != node.mapped_bounds
                || surface.last_opaque_mapped != node.opaque_mapped
                || surface.last_occlusion != mask
                || !surface.frame_damage().is_empty()
            {
                out.records.push((
                    node.id,
                    node.mapped_bounds,
                    mask.clone(),
                    node.opaque_mapped.clone(),
                ));
            }
            mask.add_region(&node.opaque_mapped);
        }
        out
    }

    /// Direct-scanout candidate footprints: fully-visible, untransformed,
    /// unscaled, fully-opaque root surfaces entirely on the output.
    fn scanout_candidates(
        tree: &SurfaceTree,
        flat: &crate::occlusion::FlatTree,
        output: Rect,
        fully_visible: &[SurfaceId],
    ) -> Region {
        let mut candidates = Region::new();
        for node in &flat.nodes {
            if !fully_visible.contains(&node.id) {
                continue;
            }
            let Some(surface) = tree.get(node.id) else {
                continue;
            };
            let state = surface.state();
            if !state.is_mapped()
                || state.transform != Transform::Normal
                || !state.scale.is_identity()
                || !matches!(surface.role(), Role::Root { .. })
            {
                continue;
            }
            // Fully opaque and fully on the output.
            let opaque_full = Region::from_rect(node.local_bounds)
                .subtract(&state.opaque_region)
                .is_empty();
            if opaque_full && output.intersect(node.mapped_bounds) == Some(node.mapped_bounds) {
                candidates.add(node.mapped_bounds);
            }
        }
        candidates
    }
}

/// The walk's outputs (internal accumulator).
#[derive(Default)]
struct WalkOutput {
    repaint: Region,
    presentation: HashMap<SurfaceId, Region>,
    visible: HashMap<SurfaceId, Region>,
    fully_visible: Vec<SurfaceId>,
    records: Vec<(SurfaceId, Rect, Region, Region)>,
}

/// Region intersection (not provided by `ldp-core`; rect-pairwise).
fn intersect_regions(a: &Region, b: &Region) -> Region {
    let mut out = Region::new();
    for ra in a {
        for rb in b {
            if let Some(i) = ra.intersect(*rb) {
                out.add(i);
            }
        }
    }
    out
}
