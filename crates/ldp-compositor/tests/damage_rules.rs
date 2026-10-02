//! Damage-rule conformance (roadmap Phase 6): the engine's four rule
//! families — content, coverage (geometry vs. move semantics), opaque
//! flips, and restack pairs — against hand-built scenes. The
//! randomized corpus (`corpus.rs`) proves equality with the reference
//! model; these tests pin the individual rules on minimal cases.

use ldp_compositor::state::BufferAttachment;
use ldp_compositor::{DamageEngine, SurfaceId, SurfaceTree};
use ldp_core::geometry::{Rect, Region};

fn id(n: u64) -> SurfaceId {
    SurfaceId::from_raw(n)
}

fn attach(tree: &mut SurfaceTree, id: SurfaceId, w: u32, h: u32, gen: u64) {
    tree.pending_mut(id).unwrap().attach(Some(BufferAttachment {
        width: w,
        height: h,
        generation: gen,
    }));
}

fn opaque_rects(tree: &mut SurfaceTree, id: SurfaceId, rects: &[Rect]) {
    let mut region = Region::new();
    for r in rects {
        region.add(*r);
    }
    tree.pending_mut(id).unwrap().set_opaque_region(region);
}

const OUT: Rect = Rect::new(0, 0, 200, 200);

fn frame(tree: &mut SurfaceTree) -> ldp_compositor::FrameDamage {
    let changes = tree.take_changes();
    DamageEngine::compute(tree, &changes, OUT)
}

#[test]
fn first_map_damages_the_new_bounds() {
    let mut t = SurfaceTree::new();
    t.create_root(id(1), 10, 20).unwrap();
    attach(&mut t, id(1), 30, 40, 1);
    t.commit(id(1)).unwrap();
    let d = frame(&mut t);
    assert_eq!(d.repaint.bounds(), Rect::new(10, 20, 30, 40));
    assert_eq!(d.presentation[&id(1)].bounds(), Rect::new(10, 20, 30, 40));
}

#[test]
fn occluded_content_damage_is_subtracted() {
    let mut t = SurfaceTree::new();
    t.create_root(id(1), 0, 0).unwrap(); // back
    t.create_root(id(2), 0, 0).unwrap(); // front, opaque 20x20
    attach(&mut t, id(1), 20, 20, 1);
    t.commit(id(1)).unwrap();
    attach(&mut t, id(2), 20, 20, 1);
    opaque_rects(&mut t, id(2), &[Rect::new(0, 0, 20, 20)]);
    t.commit(id(2)).unwrap();
    frame(&mut t); // establish occlusion
                   // Damage the back surface fully: everything is occluded.
    t.pending_mut(id(1))
        .unwrap()
        .damage(&[Rect::new(0, 0, 20, 20)]);
    t.commit(id(1)).unwrap();
    let d = frame(&mut t);
    assert!(d.repaint.is_empty(), "occluded damage must vanish");
    assert!(!d.presentation.contains_key(&id(1)));
    // Shrink the occluder: its own new content (top half), its
    // coverage loss (bottom half), and its opaque flip (bottom
    // half) repaint the full old footprint.
    attach(&mut t, id(2), 20, 10, 2);
    t.commit(id(2)).unwrap();
    let d = frame(&mut t);
    assert_eq!(d.repaint.bounds(), Rect::new(0, 0, 20, 20));
}

#[test]
fn moving_a_surface_damages_old_and_new_cells() {
    let mut t = SurfaceTree::new();
    t.create_root(id(1), 0, 0).unwrap();
    attach(&mut t, id(1), 10, 10, 1);
    t.commit(id(1)).unwrap();
    frame(&mut t);
    t.set_position(id(1), 30, 0).unwrap();
    t.commit(id(1)).unwrap(); // applies the position
    let d = frame(&mut t);
    // Old cells (0..10) and new cells (30..40).
    assert_eq!(d.repaint.len(), 2);
    let b = d.repaint.bounds();
    assert_eq!((b.x, b.w), (0, 40));
}

#[test]
fn opaque_growth_damages_the_flip_area() {
    let mut t = SurfaceTree::new();
    t.create_root(id(1), 0, 0).unwrap();
    t.create_root(id(2), 0, 0).unwrap(); // front
    attach(&mut t, id(1), 20, 20, 1);
    attach(&mut t, id(2), 20, 20, 1);
    t.commit(id(1)).unwrap();
    t.commit(id(2)).unwrap();
    frame(&mut t);
    // Grow the front surface's opaque region: the flip cells must
    // repaint (the fold below changes even without content damage).
    opaque_rects(&mut t, id(2), &[Rect::new(0, 0, 20, 10)]);
    t.commit(id(2)).unwrap();
    let d = frame(&mut t);
    assert_eq!(d.repaint.bounds(), Rect::new(0, 0, 20, 10));
}

#[test]
fn restack_crossing_damages_the_overlap() {
    let mut t = SurfaceTree::new();
    // Back A (0,0,20,20); front B (0,0,20,20) overlapping fully.
    t.create_root(id(1), 0, 0).unwrap();
    t.create_root(id(2), 0, 0).unwrap();
    attach(&mut t, id(1), 20, 20, 1);
    attach(&mut t, id(2), 20, 20, 1);
    t.commit(id(1)).unwrap();
    t.commit(id(2)).unwrap();
    frame(&mut t);
    // Raise A above B: they crossed; the overlap (all of it) must
    // repaint even though neither has content damage.
    t.place_above(id(1), id(2)).unwrap();
    let d = frame(&mut t);
    assert_eq!(d.repaint.bounds(), Rect::new(0, 0, 20, 20));
    // A second identical restack-free frame is clean.
    let d = frame(&mut t);
    assert!(d.repaint.is_empty());
}

#[test]
fn restack_beside_each_other_damages_nothing() {
    let mut t = SurfaceTree::new();
    t.create_root(id(1), 0, 0).unwrap();
    t.create_root(id(2), 50, 0).unwrap(); // disjoint
    attach(&mut t, id(1), 20, 20, 1);
    attach(&mut t, id(2), 20, 20, 1);
    t.commit(id(1)).unwrap();
    t.commit(id(2)).unwrap();
    frame(&mut t);
    t.place_above(id(1), id(2)).unwrap();
    let d = frame(&mut t);
    assert!(d.repaint.is_empty(), "no overlap: no fold change");
}

#[test]
fn removal_damages_the_old_footprint() {
    let mut t = SurfaceTree::new();
    t.create_root(id(1), 5, 5).unwrap();
    attach(&mut t, id(1), 10, 10, 1);
    t.commit(id(1)).unwrap();
    frame(&mut t);
    t.destroy(id(1)).unwrap();
    let d = frame(&mut t);
    assert_eq!(d.repaint.bounds(), Rect::new(5, 5, 10, 10));
}

#[test]
fn damage_persists_until_a_pass_consumes_it() {
    let mut t = SurfaceTree::new();
    t.create_root(id(1), 0, 0).unwrap();
    attach(&mut t, id(1), 50, 50, 1);
    t.commit(id(1)).unwrap();
    let first = frame(&mut t);
    assert_eq!(first.repaint.bounds(), Rect::new(0, 0, 50, 50));
    // No pass for the next two frames: damage accumulates.
    t.pending_mut(id(1))
        .unwrap()
        .damage(&[Rect::new(1, 1, 2, 2)]);
    t.commit(id(1)).unwrap();
    t.pending_mut(id(1))
        .unwrap()
        .damage(&[Rect::new(10, 10, 2, 2)]);
    t.commit(id(1)).unwrap();
    let d = frame(&mut t);
    assert_eq!(d.repaint.len(), 2);
    assert_eq!(d.presentation[&id(1)].len(), 2);
}

#[test]
fn everything_clips_to_the_output() {
    let mut t = SurfaceTree::new();
    t.create_root(id(1), 190, 0).unwrap(); // straddles the edge
    attach(&mut t, id(1), 20, 20, 1);
    t.commit(id(1)).unwrap();
    let d = frame(&mut t);
    assert_eq!(d.repaint.bounds(), Rect::new(190, 0, 10, 20));
    assert_eq!(d.visible[&id(1)].bounds(), Rect::new(190, 0, 10, 20));
}

#[test]
fn scanout_tracks_opaque_unoccluded_roots() {
    let mut t = SurfaceTree::new();
    t.create_root(id(1), 0, 0).unwrap(); // candidate
    t.create_root(id(2), 0, 0).unwrap(); // front, translucent
    attach(&mut t, id(1), 20, 20, 1);
    opaque_rects(&mut t, id(1), &[Rect::new(0, 0, 20, 20)]);
    t.commit(id(1)).unwrap();
    attach(&mut t, id(2), 20, 20, 1); // no opaque region
    t.commit(id(2)).unwrap();
    let d = frame(&mut t);
    // The repaint (first map of both) intersects the candidate.
    assert_eq!(d.scanout.bounds(), Rect::new(0, 0, 20, 20));
    // A frame with damage only under the translucent overlay still
    // lands in scanout (the overlay does not occlude).
    t.pending_mut(id(1))
        .unwrap()
        .damage(&[Rect::new(2, 2, 4, 4)]);
    t.commit(id(1)).unwrap();
    let d = frame(&mut t);
    assert_eq!(d.scanout.bounds(), Rect::new(2, 2, 4, 4));
}

#[test]
fn sync_subsurface_damage_waits_for_the_parent_commit() {
    let mut t = SurfaceTree::new();
    t.create_root(id(1), 0, 0).unwrap();
    t.create_subsurface(id(2), id(1)).unwrap();
    attach(&mut t, id(2), 8, 8, 1);
    t.set_position(id(2), 4, 4).unwrap();
    // Sync commit stashes; the parent commit releases it.
    t.commit(id(2)).unwrap();
    let d = frame(&mut t);
    assert!(d.repaint.is_empty(), "nothing applied yet");
    t.commit(id(1)).unwrap();
    let d = frame(&mut t);
    assert_eq!(d.repaint.bounds(), Rect::new(4, 4, 8, 8));
}

#[test]
fn set_position_now_moves_without_a_commit() {
    // Phase 28's re-layout arm: the shell re-places a mapped root
    // (a migration) with no commit in flight. The R2 coverage rule
    // must produce the same old-and-new damage as a committed move.
    let mut t = SurfaceTree::new();
    t.create_root(id(1), 0, 0).unwrap();
    attach(&mut t, id(1), 10, 10, 1);
    t.commit(id(1)).unwrap();
    frame(&mut t);
    t.set_position_now(id(1), 30, 0).unwrap();
    let d = frame(&mut t);
    // Old cells (0..10) and new cells (30..40) — identical to
    // `moving_a_surface_damages_old_and_new_cells`.
    assert_eq!(d.repaint.len(), 2);
    let b = d.repaint.bounds();
    assert_eq!((b.x, b.w), (0, 40));
    // A second quiet frame is clean (the pass records latched).
    let d = frame(&mut t);
    assert!(d.repaint.is_empty());
    // The immediate form supersedes a queued position: the queued
    // one must NOT apply at the next commit (it is gone).
    t.set_position(id(1), 5, 5).unwrap();
    t.set_position_now(id(1), 60, 0).unwrap();
    let d = frame(&mut t);
    let b = d.repaint.bounds();
    assert_eq!((b.x, b.w), (30, 40), "moved from 30..40 to 60..70");
    // Unknown ids are refused.
    assert!(t.set_position_now(id(9), 0, 0).is_err());
}

#[test]
fn quiet_frames_stay_empty_and_consume_damage_exactly_once() {
    // The v0.12 quiet-frame fast paths skip pass records whose fields
    // would not change — the guard must keep the one side effect that
    // matters: `record_pass` consumes accumulated damage. A quiescent
    // surface with pending damage still records (and clears) it; a
    // fully quiescent surface stays clean forever.
    let mut t = SurfaceTree::new();
    t.create_root(id(1), 0, 0).unwrap();
    t.create_root(id(2), 0, 0).unwrap();
    attach(&mut t, id(1), 30, 30, 1);
    attach(&mut t, id(2), 30, 30, 1);
    t.commit(id(1)).unwrap();
    t.commit(id(2)).unwrap();
    frame(&mut t); // both mapped, records latched

    // Damage on surface 1 only: this pass must consume it (the record
    // is pushed despite unchanged bounds/occlusion/footprint) ...
    t.pending_mut(id(1))
        .unwrap()
        .damage(&[Rect::new(5, 5, 2, 2)]);
    t.commit(id(1)).unwrap();
    let d = frame(&mut t);
    assert_eq!(d.repaint.bounds(), Rect::new(5, 5, 2, 2));
    // ... and the next quiet frame must be clean — consumed exactly
    // once, never re-delivered.
    let d = frame(&mut t);
    assert!(d.repaint.is_empty());
    assert!(d.is_empty());
}

#[test]
fn occlusion_change_alone_relatches_the_pass_records() {
    // A's own bounds and opaque footprint never move, but the opaque
    // window above it goes away: A's occlusion record must relatch in
    // exactly that pass (the skip guard's third term). If it did not,
    // A's next damage — under the vanished window's old footprint —
    // would still subtract the stale occlusion and vanish.
    let mut t = SurfaceTree::new();
    t.create_root(id(1), 0, 0).unwrap(); // back
    t.create_root(id(2), 0, 0).unwrap(); // front, opaque
    attach(&mut t, id(1), 20, 20, 1);
    t.commit(id(1)).unwrap();
    attach(&mut t, id(2), 20, 20, 1);
    opaque_rects(&mut t, id(2), &[Rect::new(0, 0, 20, 20)]);
    t.commit(id(2)).unwrap();
    let d = frame(&mut t);
    assert!(d.visible[&id(1)].is_empty(), "fully occluded at first");

    // The front window goes away: the removal damages its footprint.
    t.destroy(id(2)).unwrap();
    let d = frame(&mut t);
    assert_eq!(d.repaint.bounds(), Rect::new(0, 0, 20, 20));
    // A is fully visible now (its occlusion record relatched).
    assert_eq!(d.visible[&id(1)].bounds(), Rect::new(0, 0, 20, 20));

    // Damage under the vanished footprint must survive the R1
    // occlusion subtraction — the stale record would have eaten it.
    t.pending_mut(id(1))
        .unwrap()
        .damage(&[Rect::new(4, 4, 4, 4)]);
    t.commit(id(1)).unwrap();
    let d = frame(&mut t);
    assert_eq!(d.repaint.bounds(), Rect::new(4, 4, 4, 4));
    assert_eq!(d.presentation[&id(1)].bounds(), Rect::new(4, 4, 4, 4));
}

#[test]
fn partially_occluded_roots_are_not_scanout_candidates() {
    // The fully-visible fast path (visible region *is* the bounds) must
    // agree with the round-trip it replaced: a root partially covered
    // by an opaque sibling is not a candidate, while the covering
    // sibling itself is.
    let mut t = SurfaceTree::new();
    t.create_root(id(1), 0, 0).unwrap(); // back, 20x20 opaque
    t.create_root(id(2), 0, 0).unwrap(); // front, 20x10 opaque
    attach(&mut t, id(1), 20, 20, 1);
    opaque_rects(&mut t, id(1), &[Rect::new(0, 0, 20, 20)]);
    t.commit(id(1)).unwrap();
    attach(&mut t, id(2), 20, 10, 1);
    opaque_rects(&mut t, id(2), &[Rect::new(0, 0, 20, 10)]);
    t.commit(id(2)).unwrap();
    let d = frame(&mut t);
    // Only the front root's own footprint: the back root is partially
    // occluded and must not join the candidacy. (The scanout region is
    // an exact set difference, so its rect count is representation —
    // the bounds and the containment below are the set truth: if the
    // back root had joined, the bounds would be its full 20x20.)
    let front = Rect::new(0, 0, 20, 10);
    assert_eq!(d.scanout.bounds(), front);
    for r in &d.scanout {
        assert_eq!(
            r.intersect(front),
            Some(*r),
            "scanout rect {r:?} escapes the front footprint"
        );
    }
    // The back root's visible region is the unoccluded slice.
    assert_eq!(d.visible[&id(1)].bounds(), Rect::new(0, 10, 20, 10));
}
