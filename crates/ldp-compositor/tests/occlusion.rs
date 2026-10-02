//! Occlusion conformance (roadmap Phase 6): the visible-region walk
//! over nested trees, stacked opaque chains, and the damage
//! consequences of occlusion changes.

use ldp_compositor::occlusion::{flatten, visible_regions};
use ldp_compositor::state::BufferAttachment;
use ldp_compositor::{DamageEngine, SurfaceId, SurfaceTree};
use ldp_core::geometry::{Rect, Region};

const OUT: Rect = Rect::new(0, 0, 256, 256);

fn id(n: u64) -> SurfaceId {
    SurfaceId::from_raw(n)
}

fn attach(tree: &mut SurfaceTree, id: SurfaceId, w: u32, h: u32) {
    tree.pending_mut(id).unwrap().attach(Some(BufferAttachment {
        width: w,
        height: h,
        generation: 1,
    }));
}

fn opaque(tree: &mut SurfaceTree, id: SurfaceId, rects: &[Rect]) {
    let mut region = Region::new();
    for r in rects {
        region.add(*r);
    }
    tree.pending_mut(id).unwrap().set_opaque_region(region);
}

fn commit(tree: &mut SurfaceTree, id: SurfaceId) {
    tree.commit(id).unwrap();
}

fn frame(tree: &mut SurfaceTree) -> ldp_compositor::FrameDamage {
    let changes = tree.take_changes();
    DamageEngine::compute(tree, &changes, OUT)
}

#[test]
fn nested_subsurfaces_occlude_through_the_tree() {
    let mut t = SurfaceTree::new();
    // Root R (40x40) with subsurface A (20x20 at 10,10, opaque) and
    // A's own subsurface B (10x10 at 5,5) — B renders above A.
    t.create_root(id(1), 0, 0).unwrap();
    t.create_subsurface(id(2), id(1)).unwrap();
    t.create_subsurface(id(3), id(2)).unwrap();
    attach(&mut t, id(1), 40, 40);
    attach(&mut t, id(2), 20, 20);
    attach(&mut t, id(3), 10, 10);
    t.set_position(id(2), 10, 10).unwrap();
    t.set_position(id(3), 5, 5).unwrap();
    opaque(&mut t, id(2), &[Rect::new(0, 0, 20, 20)]);
    commit(&mut t, id(3)); // sync stash
    commit(&mut t, id(2)); // sync stash
    commit(&mut t, id(1)); // cascade applies all

    let flat = flatten(&t).nodes;
    let occ = visible_regions(&flat);
    let visible = |n: u64| {
        occ.iter()
            .find(|o| o.id.raw() == n)
            .map(|o| o.visible.bounds())
            .unwrap()
    };
    // B sits fully inside A's opaque area but is ABOVE it: fully visible.
    assert_eq!(visible(3), Rect::new(15, 15, 10, 10));
    // A's own visibility: unoccluded (nothing above it).
    assert_eq!(visible(2), Rect::new(10, 10, 20, 20));
    // The root shows the 40x40 minus A's opaque footprint: four
    // strips (top, bottom, left, right) around the 20x20 hole.
    let root_visible = occ.iter().find(|o| o.id.raw() == 1).unwrap().visible.len();
    assert_eq!(root_visible, 4);
}

#[test]
fn stacked_opaque_chain_hides_the_bottom() {
    let mut t = SurfaceTree::new();
    t.create_root(id(1), 0, 0).unwrap();
    t.create_root(id(2), 0, 0).unwrap();
    t.create_root(id(3), 0, 0).unwrap();
    for i in [1u64, 2, 3] {
        attach(&mut t, id(i), 30, 30);
        opaque(&mut t, id(i), &[Rect::new(0, 0, 30, 30)]);
        commit(&mut t, id(i));
    }
    frame(&mut t); // establish occlusion records
                   // Damage the bottom surface: fully occluded by the opaque stack.
    t.pending_mut(id(1))
        .unwrap()
        .damage(&[Rect::new(0, 0, 30, 30)]);
    commit(&mut t, id(1));
    let d = frame(&mut t);
    assert!(d.repaint.is_empty());
    assert!(!d.presentation.contains_key(&id(1)));
    // Grow the top surface's damage: visible.
    t.pending_mut(id(3))
        .unwrap()
        .damage(&[Rect::new(5, 5, 3, 3)]);
    commit(&mut t, id(3));
    let d = frame(&mut t);
    assert_eq!(d.repaint.bounds(), Rect::new(5, 5, 3, 3));
    assert_eq!(d.presentation[&id(3)].bounds(), Rect::new(5, 5, 3, 3));
}

#[test]
fn partially_occluded_damage_survives_in_the_visible_slice() {
    let mut t = SurfaceTree::new();
    t.create_root(id(1), 0, 0).unwrap(); // back, 40x20 at (0,10)
    t.create_root(id(2), 0, 0).unwrap(); // front, opaque 20x20
    t.set_position(id(1), 0, 10).unwrap();
    attach(&mut t, id(1), 40, 20);
    commit(&mut t, id(1));
    attach(&mut t, id(2), 20, 20);
    opaque(&mut t, id(2), &[Rect::new(0, 0, 20, 20)]);
    commit(&mut t, id(2));
    frame(&mut t);
    // Damage the back surface fully: the occluder covers only its
    // upper-left 20x20 window, so the right half AND the bottom strip
    // survive occlusion subtraction.
    t.pending_mut(id(1))
        .unwrap()
        .damage(&[Rect::new(0, 0, 40, 20)]);
    commit(&mut t, id(1));
    let d = frame(&mut t);
    assert_eq!(d.repaint.bounds(), Rect::new(0, 10, 40, 20));
    assert_eq!(d.repaint.len(), 2); // right half + bottom strip
    assert_eq!(d.presentation[&id(1)].bounds(), Rect::new(0, 10, 40, 20));
}

#[test]
fn moving_the_occluder_reveals_the_hidden_surface() {
    let mut t = SurfaceTree::new();
    t.create_root(id(1), 0, 0).unwrap(); // back, 40x20 at (0,0)
    t.create_root(id(2), 0, 0).unwrap(); // front, opaque 20x20
    attach(&mut t, id(1), 40, 20);
    commit(&mut t, id(1));
    attach(&mut t, id(2), 20, 20);
    opaque(&mut t, id(2), &[Rect::new(0, 0, 20, 20)]);
    commit(&mut t, id(2));
    frame(&mut t);
    // Move the occluder off to the right: the revealed strip must
    // repaint, plus the occluder's own new footprint.
    t.set_position(id(2), 30, 0).unwrap();
    commit(&mut t, id(2));
    let d = frame(&mut t);
    // The occluder's full old∪new footprint (move semantics) — the
    // back surface's old occluded area is inside that union.
    assert_eq!(d.repaint.bounds(), Rect::new(0, 0, 50, 20));
    // A quiet frame afterwards is fully clean.
    let d = frame(&mut t);
    assert!(d.repaint.is_empty());
}

#[test]
fn opaque_growth_with_nothing_below_is_clean() {
    let mut t = SurfaceTree::new();
    t.create_root(id(1), 0, 0).unwrap(); // back
    t.create_root(id(2), 0, 0).unwrap(); // front, translucent
    attach(&mut t, id(1), 20, 20);
    attach(&mut t, id(2), 20, 20);
    commit(&mut t, id(1));
    commit(&mut t, id(2));
    frame(&mut t);
    // The back surface turns fully opaque: nothing sits below it, so
    // the fold is unchanged (the flip only matters when something
    // below would be wiped or revealed).
    opaque(&mut t, id(1), &[Rect::new(0, 0, 20, 20)]);
    commit(&mut t, id(1));
    let d = frame(&mut t);
    assert!(d.repaint.is_empty(), "nothing below: no fold change");
}

#[test]
fn opaque_growth_wipes_a_surface_below() {
    let mut t = SurfaceTree::new();
    t.create_root(id(1), 0, 0).unwrap(); // bottom
    t.create_root(id(2), 0, 0).unwrap(); // middle, turns opaque
    t.create_root(id(3), 0, 0).unwrap(); // top, translucent
    attach(&mut t, id(1), 20, 20);
    attach(&mut t, id(2), 20, 20);
    attach(&mut t, id(3), 20, 20);
    commit(&mut t, id(1));
    commit(&mut t, id(2));
    commit(&mut t, id(3));
    frame(&mut t);
    // The middle surface turns opaque: the bottom surface is wiped
    // from the fold — those cells changed.
    opaque(&mut t, id(2), &[Rect::new(0, 0, 20, 20)]);
    commit(&mut t, id(2));
    let d = frame(&mut t);
    assert_eq!(d.repaint.bounds(), Rect::new(0, 0, 20, 20));
}

#[test]
fn opaque_flip_over_empty_backdrop_is_clean() {
    let mut t = SurfaceTree::new();
    t.create_root(id(1), 0, 0).unwrap(); // the only surface
    attach(&mut t, id(1), 20, 20);
    commit(&mut t, id(1));
    frame(&mut t);
    // Flipping opacity with nothing below changes no composited pixel.
    opaque(&mut t, id(1), &[Rect::new(0, 0, 20, 20)]);
    commit(&mut t, id(1));
    let d = frame(&mut t);
    assert!(d.repaint.is_empty(), "nothing below: no fold change");
}
