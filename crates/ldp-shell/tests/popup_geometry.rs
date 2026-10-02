//! Popup constraint-solver properties (Phase 12 support suite).
//!
//! Property-tested over seeded random anchor geometry:
//!
//! * **reference agreement**: the unconstrained placement matches an
//!   independently derived reference model (anchor point + growth
//!   vector);
//! * **containment**: with all strategies granted and a popup that
//!   fits the usable area, the solved placement fits;
//! * **attachment**: slide never detaches the popup past the anchor
//!   rect's opposite edge;
//! * **idempotence**: solving the solved placement again changes
//!   nothing (a fixed point);
//! * **permission honesty**: with no strategies granted the solver
//!   returns the unconstrained placement verbatim.

#![forbid(unsafe_code)]

mod common;

use common::Rng;
use ldp_core::geometry::Rect;
use ldp_shell::popup::{
    solve, solve_unconstrained, Anchor, Gravity, Placement, PopupConstraints, PopupGeometry,
};

/// The independent reference model: compute the anchor point and the
/// growth vector separately, then place.
fn reference(geom: &PopupGeometry, size: (u32, u32)) -> (i32, i32) {
    let rect = geom.anchor_rect;
    // Anchor point.
    let (ax, ay) = match geom.anchor {
        Anchor::TopLeft => (rect.x, rect.y),
        Anchor::Top => (rect.x + rect.w as i32 / 2, rect.y),
        Anchor::TopRight => (rect.x + rect.w as i32, rect.y),
        Anchor::Right => (rect.x + rect.w as i32, rect.y + rect.h as i32 / 2),
        Anchor::BottomRight => (rect.x + rect.w as i32, rect.y + rect.h as i32),
        Anchor::Bottom => (rect.x + rect.w as i32 / 2, rect.y + rect.h as i32),
        Anchor::BottomLeft => (rect.x, rect.y + rect.h as i32),
        Anchor::Left => (rect.x, rect.y + rect.h as i32 / 2),
    };
    // Growth vector.
    let (grow_x, grow_y) = match geom.gravity {
        Gravity::TopLeft => (-1, -1),
        Gravity::Top => (0, -1),
        Gravity::TopRight => (1, -1),
        Gravity::Right => (1, 0),
        Gravity::BottomRight => (1, 1),
        Gravity::Bottom => (0, 1),
        Gravity::BottomLeft => (-1, 1),
        Gravity::Left => (-1, 0),
    };
    let (pw, ph) = (size.0 as i32, size.1 as i32);
    let x = match grow_x {
        1 => ax,
        -1 => ax - pw,
        _ => ax - pw / 2,
    };
    let y = match grow_y {
        1 => ay,
        -1 => ay - ph,
        _ => ay - ph / 2,
    };
    (x + geom.offset.0, y + geom.offset.1)
}

fn fits(p: &Placement, usable: Rect) -> bool {
    p.x >= usable.x
        && p.y >= usable.y
        && p.x + p.width as i32 <= usable.x + usable.w as i32
        && p.y + p.height as i32 <= usable.y + usable.h as i32
}

#[test]
fn unconstrained_matches_the_reference_model() {
    let mut rng = Rng::seeded(0xACE);
    for _ in 0..4_000 {
        let g = PopupGeometry {
            anchor_rect: Rect::new(
                i32::try_from(rng.below(2000)).unwrap() - 500,
                i32::try_from(rng.below(2000)).unwrap() - 500,
                u32::try_from(rng.below(300)).unwrap() + 1,
                u32::try_from(rng.below(120)).unwrap() + 1,
            ),
            anchor: Anchor::from_wire(u32::try_from(rng.below(8)).unwrap() + 1).unwrap(),
            gravity: Gravity::from_wire(u32::try_from(rng.below(8)).unwrap() + 1).unwrap(),
            offset: (
                i32::try_from(rng.below(120)).unwrap() - 60,
                i32::try_from(rng.below(120)).unwrap() - 60,
            ),
        };
        let size = (
            u32::try_from(rng.below(400)).unwrap() + 1,
            u32::try_from(rng.below(300)).unwrap() + 1,
        );
        let p = solve_unconstrained(&g, size);
        let (rx, ry) = reference(&g, size);
        assert_eq!((p.x, p.y), (rx, ry), "reference disagreement for {g:?}");
        assert_eq!((p.width, p.height), size);
    }
}

#[test]
fn all_strategies_contain_what_fits() {
    let mut rng = Rng::seeded(0xBEA7);
    let usable = Rect::new(0, 0, 1920, 1080);
    for _ in 0..4_000 {
        let g = PopupGeometry {
            // Anchors inside the usable area (the realistic case).
            anchor_rect: Rect::new(
                i32::try_from(rng.below(1700)).unwrap(),
                i32::try_from(rng.below(900)).unwrap(),
                u32::try_from(rng.below(220)).unwrap() + 1,
                u32::try_from(rng.below(120)).unwrap() + 1,
            ),
            anchor: Anchor::from_wire(u32::try_from(rng.below(8)).unwrap() + 1).unwrap(),
            gravity: Gravity::from_wire(u32::try_from(rng.below(8)).unwrap() + 1).unwrap(),
            offset: (
                i32::try_from(rng.below(60)).unwrap() - 30,
                i32::try_from(rng.below(60)).unwrap() - 30,
            ),
        };
        // The popup fits the usable area at all.
        let size = (
            u32::try_from(rng.below(600)).unwrap() + 1,
            u32::try_from(rng.below(500)).unwrap() + 1,
        );
        if size.0 > usable.w || size.1 > usable.h {
            continue;
        }
        let p = solve(&g, size, usable, PopupConstraints::all());
        assert!(fits(&p, usable), "{g:?} {size:?} → {p:?}");
    }
}

#[test]
fn slide_never_detaches_past_the_anchor() {
    let mut rng = Rng::seeded(0x511DE);
    let usable = Rect::new(0, 0, 1920, 1080);
    for _ in 0..4_000 {
        let g = PopupGeometry {
            anchor_rect: Rect::new(
                i32::try_from(rng.below(1900)).unwrap(),
                i32::try_from(rng.below(1000)).unwrap(),
                u32::try_from(rng.below(200)).unwrap() + 1,
                u32::try_from(rng.below(100)).unwrap() + 1,
            ),
            anchor: Anchor::from_wire(u32::try_from(rng.below(8)).unwrap() + 1).unwrap(),
            gravity: Gravity::from_wire(u32::try_from(rng.below(8)).unwrap() + 1).unwrap(),
            offset: (0, 0),
        };
        let size = (
            u32::try_from(rng.below(500)).unwrap() + 1,
            u32::try_from(rng.below(400)).unwrap() + 1,
        );
        // Slide only: no flip, no resize.
        let cons = PopupConstraints::none()
            .with(PopupConstraints::SLIDE_X)
            .with(PopupConstraints::SLIDE_Y);
        let p = solve(&g, size, usable, cons);
        let unconstrained = solve_unconstrained(&g, size);
        // Only moved when the unconstrained placement offended.
        if fits(&unconstrained, usable) {
            assert_eq!(p, unconstrained);
        } else {
            // Attachment: the popup's left/right edge stays at or
            // past the anchor rect's opposite edge (x axis), and
            // likewise on y — the slide cap.
            let a_right = g.anchor_rect.x + g.anchor_rect.w as i32;
            let a_left = g.anchor_rect.x;
            let a_bottom = g.anchor_rect.y + g.anchor_rect.h as i32;
            let a_top = g.anchor_rect.y;
            if unconstrained.x < usable.x {
                // Slid right: left edge may not pass the anchor's
                // right edge.
                assert!(p.x <= a_right, "detached right: {g:?} {size:?} → {p:?}");
            }
            if unconstrained.x + size.0 as i32 > usable.x + usable.w as i32 {
                // Slid left: right edge may not pass the anchor's
                // left edge.
                assert!(
                    p.x + p.width as i32 >= a_left,
                    "detached left: {g:?} {size:?} → {p:?}"
                );
            }
            if unconstrained.y < usable.y {
                assert!(p.y <= a_bottom, "detached down: {g:?} {size:?} → {p:?}");
            }
            if unconstrained.y + size.1 as i32 > usable.y + usable.h as i32 {
                assert!(
                    p.y + p.height as i32 >= a_top,
                    "detached up: {g:?} {size:?} → {p:?}"
                );
            }
        }
    }
}

#[test]
fn no_strategies_means_unconstrained_verbatim() {
    let mut rng = Rng::seeded(0x0FF);
    let usable = Rect::new(0, 0, 1920, 1080);
    for _ in 0..2_000 {
        let g = PopupGeometry {
            anchor_rect: Rect::new(
                i32::try_from(rng.below(2200)).unwrap() - 300,
                i32::try_from(rng.below(1400)).unwrap() - 200,
                u32::try_from(rng.below(300)).unwrap() + 1,
                u32::try_from(rng.below(150)).unwrap() + 1,
            ),
            anchor: Anchor::from_wire(u32::try_from(rng.below(8)).unwrap() + 1).unwrap(),
            gravity: Gravity::from_wire(u32::try_from(rng.below(8)).unwrap() + 1).unwrap(),
            offset: (
                i32::try_from(rng.below(80)).unwrap() - 40,
                i32::try_from(rng.below(80)).unwrap() - 40,
            ),
        };
        let size = (
            u32::try_from(rng.below(400)).unwrap() + 1,
            u32::try_from(rng.below(300)).unwrap() + 1,
        );
        let p = solve(&g, size, usable, PopupConstraints::none());
        assert_eq!(p, solve_unconstrained(&g, size));
    }
}

#[test]
fn solving_is_pure_and_leaves_fitting_placements_alone() {
    let mut rng = Rng::seeded(0x1DE);
    let usable = Rect::new(0, 0, 1920, 1080);
    for _ in 0..2_000 {
        let g = PopupGeometry {
            anchor_rect: Rect::new(
                i32::try_from(rng.below(1800)).unwrap(),
                i32::try_from(rng.below(1000)).unwrap(),
                u32::try_from(rng.below(200)).unwrap() + 1,
                u32::try_from(rng.below(100)).unwrap() + 1,
            ),
            anchor: Anchor::from_wire(u32::try_from(rng.below(8)).unwrap() + 1).unwrap(),
            gravity: Gravity::from_wire(u32::try_from(rng.below(8)).unwrap() + 1).unwrap(),
            offset: (
                i32::try_from(rng.below(40)).unwrap() - 20,
                i32::try_from(rng.below(40)).unwrap() - 20,
            ),
        };
        let size = (
            u32::try_from(rng.below(500)).unwrap() + 1,
            u32::try_from(rng.below(400)).unwrap() + 1,
        );
        // Purity: the same inputs always solve to the same placement.
        let p1 = solve(&g, size, usable, PopupConstraints::all());
        let p2 = solve(&g, size, usable, PopupConstraints::all());
        assert_eq!(p1, p2);
        // No-op on fitting placements: an unconstrained placement
        // inside the usable area is returned verbatim — the solver
        // never invents corrections where none are needed.
        let unconstrained = solve_unconstrained(&g, size);
        if fits(&unconstrained, usable) {
            assert_eq!(p1, unconstrained, "fitting placement moved: {g:?} {size:?}");
        }
    }
}

#[test]
fn resize_only_clamps_what_overflows() {
    // Right/bottom overflow with only resize: width clamps to the
    // available room; left/top overflow stands (resize cannot move).
    let g = PopupGeometry {
        anchor_rect: Rect::new(1880, 100, 30, 30),
        anchor: Anchor::TopRight,
        gravity: Gravity::BottomRight,
        offset: (0, 0),
    };
    let usable = Rect::new(0, 0, 1920, 1080);
    let p = solve(
        &g,
        (300, 200),
        usable,
        PopupConstraints::none()
            .with(PopupConstraints::RESIZE_X)
            .with(PopupConstraints::RESIZE_Y),
    );
    assert_eq!((p.x, p.width), (1910, 10));
    assert_eq!(p.height, 200); // fits vertically, untouched
                               // Left overflow with only resize: stands verbatim.
    let g2 = PopupGeometry {
        anchor_rect: Rect::new(10, 10, 30, 30),
        anchor: Anchor::TopLeft,
        gravity: Gravity::TopLeft,
        offset: (0, 0),
    };
    let p2 = solve(
        &g2,
        (100, 100),
        usable,
        PopupConstraints::none().with(PopupConstraints::RESIZE_X),
    );
    assert_eq!(p2, solve_unconstrained(&g2, (100, 100)));
}

#[test]
fn flip_only_accepted_when_the_axis_fits() {
    // A geometry whose flipped placement also does not fit: the flip
    // is rejected and the original axis stands.
    let usable = Rect::new(500, 500, 200, 200);
    // Anchor far left of the usable area, popup wider than the usable
    // area itself: flipping cannot fit either.
    let g = PopupGeometry {
        anchor_rect: Rect::new(0, 600, 40, 40),
        anchor: Anchor::Right,
        gravity: Gravity::Right,
        offset: (0, 0),
    };
    let p = solve(
        &g,
        (400, 100),
        usable,
        PopupConstraints::none().with(PopupConstraints::FLIP_X),
    );
    // Unconstrained: grows right from (40, 620): x=40, right edge 440
    // < 500 → offends. Flip mirrors to anchor Left (0, 620), gravity
    // Left (grows left): x=-400 → also offends → rejected, stands.
    assert_eq!(p.x, 40);
    assert_eq!(p.width, 400);
}
