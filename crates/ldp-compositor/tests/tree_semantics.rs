//! Tree semantics conformance (roadmap Phase 6): the atomic commit
//! cascade (sync stash, root flush, nested sync chains, desync),
//! restack validation and change records, destroy capture, and the
//! error surface.

use ldp_compositor::state::BufferAttachment;
use ldp_compositor::surface::SubsurfaceMode;
use ldp_compositor::{SurfaceId, SurfaceTree, TreeError};

fn id(n: u64) -> SurfaceId {
    SurfaceId::from_raw(n)
}

#[test]
fn roots_and_subsurfaces_are_tracked() {
    let mut t = SurfaceTree::new();
    t.create_root(id(1), 10, 20).unwrap();
    t.create_subsurface(id(2), id(1)).unwrap();
    assert_eq!(t.len(), 2);
    assert_eq!(t.parent_of(id(2)), Some(id(1)));
    assert_eq!(t.parent_of(id(1)), None);
    assert_eq!(t.children_of(Some(id(1))), &[id(2)]);
    assert!(t.children_of(None).contains(&id(1)));
}

#[test]
fn duplicates_and_missing_parents_are_rejected() {
    let mut t = SurfaceTree::new();
    t.create_root(id(1), 0, 0).unwrap();
    assert_eq!(
        t.create_root(id(1), 0, 0).unwrap_err(),
        TreeError::Duplicate
    );
    assert_eq!(
        t.create_subsurface(id(2), id(9)).unwrap_err(),
        TreeError::MissingParent
    );
    assert_eq!(t.pending_mut(id(9)).unwrap_err(), TreeError::Missing);
}

#[test]
fn sync_commits_stash_until_parent_applies() {
    let mut t = SurfaceTree::new();
    t.create_root(id(1), 0, 0).unwrap();
    t.create_subsurface(id(2), id(1)).unwrap();
    t.pending_mut(id(2)).unwrap().attach(Some(BufferAttachment {
        width: 4,
        height: 4,
        generation: 1,
    }));
    // Sync commit: stashed, nothing applied.
    assert!(t.commit(id(2)).unwrap().is_none());
    assert!(!t.get(id(2)).unwrap().state().is_mapped());
    // Root commit flushes the child.
    t.commit(id(1)).unwrap();
    assert!(t.get(id(2)).unwrap().state().is_mapped());
}

#[test]
fn desync_commits_apply_immediately() {
    let mut t = SurfaceTree::new();
    t.create_root(id(1), 0, 0).unwrap();
    t.create_subsurface(id(2), id(1)).unwrap();
    t.set_mode(id(2), SubsurfaceMode::Desync).unwrap();
    t.pending_mut(id(2)).unwrap().attach(Some(BufferAttachment {
        width: 4,
        height: 4,
        generation: 1,
    }));
    assert!(t.commit(id(2)).unwrap().is_some());
    assert!(t.get(id(2)).unwrap().state().is_mapped());
}

#[test]
fn sync_chain_flushes_at_the_root() {
    let mut t = SurfaceTree::new();
    t.create_root(id(1), 0, 0).unwrap();
    t.create_subsurface(id(2), id(1)).unwrap();
    t.create_subsurface(id(3), id(2)).unwrap();
    for i in [2u64, 3] {
        t.pending_mut(id(i)).unwrap().attach(Some(BufferAttachment {
            width: 2,
            height: 2,
            generation: 1,
        }));
        assert!(t.commit(id(i)).unwrap().is_none());
    }
    assert!(!t.get(id(2)).unwrap().state().is_mapped());
    assert!(!t.get(id(3)).unwrap().state().is_mapped());
    t.commit(id(1)).unwrap();
    assert!(t.get(id(2)).unwrap().state().is_mapped());
    assert!(t.get(id(3)).unwrap().state().is_mapped());
}

#[test]
fn restacks_validate_siblings_and_record() {
    let mut t = SurfaceTree::new();
    t.create_root(id(1), 0, 0).unwrap();
    t.create_root(id(2), 0, 0).unwrap();
    t.create_subsurface(id(3), id(1)).unwrap();
    t.create_subsurface(id(4), id(1)).unwrap();
    // Cross-parent restack rejected.
    assert_eq!(
        t.place_above(id(3), id(2)).unwrap_err(),
        TreeError::NotASibling
    );
    // Same parent accepted and recorded.
    t.place_above(id(3), id(4)).unwrap();
    let changes = t.take_changes();
    assert_eq!(changes.restacks.len(), 1);
    assert_eq!(changes.restacks[0].moved, id(3));
}

#[test]
fn destroy_captures_removal_records() {
    let mut t = SurfaceTree::new();
    t.create_root(id(1), 0, 0).unwrap();
    t.create_subsurface(id(2), id(1)).unwrap();
    t.destroy(id(1)).unwrap();
    assert!(t.is_empty());
    let changes = t.take_changes();
    assert_eq!(changes.removals.len(), 2);
    assert!(changes.removals.iter().any(|r| r.id == id(1)));
    assert!(changes.removals.iter().any(|r| r.id == id(2)));
}

#[test]
fn null_commits_are_recognized() {
    let mut t = SurfaceTree::new();
    t.create_root(id(1), 0, 0).unwrap();
    assert!(t.commit(id(1)).unwrap().is_none());
    let changes = t.take_changes();
    assert!(changes.is_empty());
}

#[test]
fn subsurface_chains_are_capped_at_the_depth_limit() {
    // The crash vector the cap closes: a client-authored chain deep
    // enough that the recursive walks (occlusion's `visit`, snapshot
    // capture, the sync flush) overflow the stack on the next damage
    // pass. The cap refuses the link *at creation* — the same typed
    // error path an unknown parent takes, never a later crash.
    let mut t = SurfaceTree::new();
    t.create_root(id(1), 0, 0).unwrap();
    // A chain one under the cap is legal ...
    for n in 2..=ldp_compositor::MAX_SUBSURFACE_DEPTH as u64 + 1 {
        // n-1 is the parent: ids 2..=65 form a 64-deep chain.
        t.create_subsurface(id(n), id(n - 1)).unwrap();
        assert_eq!(
            t.len() as u64,
            n,
            "chain of depth {} must be accepted",
            n - 1
        );
    }
    // ... and the next link is refused, typed:
    let next = ldp_compositor::MAX_SUBSURFACE_DEPTH as u64 + 2;
    assert_eq!(
        t.create_subsurface(id(next), id(next - 1)).unwrap_err(),
        TreeError::TooDeep
    );
    // The refusal is persistent (not a one-shot guard) and the tree
    // stays usable: a shallow sibling under the root still lands.
    t.create_subsurface(id(10_000), id(1)).unwrap();
    assert_eq!(t.parent_of(id(10_000)), Some(id(1)));
}

#[test]
fn the_depth_cap_walk_is_bounded_by_the_cap_itself() {
    // Every existing chain passed the creation check, so the upward
    // measurement walk is O(cap): building the deepest legal chain
    // and *then* asking for one more link costs 64 map steps, not a
    // walk proportional to the whole tree.
    let mut t = SurfaceTree::new();
    t.create_root(id(1), 0, 0).unwrap();
    let depth = ldp_compositor::MAX_SUBSURFACE_DEPTH as u64;
    for n in 2..=depth + 1 {
        t.create_subsurface(id(n), id(n - 1)).unwrap();
    }
    assert_eq!(
        t.create_subsurface(id(depth + 2), id(depth + 1))
            .unwrap_err(),
        TreeError::TooDeep
    );
    assert_eq!(
        t.create_subsurface(id(depth + 3), id(depth + 1))
            .unwrap_err(),
        TreeError::TooDeep
    );
}
