//! Snapshot/immutability invariants under mutation (roadmap Phase 6):
//! a randomized mutation stream against the authoring tree, checked
//! after every frame that (a) all live snapshots still satisfy their
//! structural invariants, (b) their contents are unchanged by later
//! mutations, and (c) a fresh snapshot reflects the tree exactly.

use ldp_compositor::state::BufferAttachment;
use ldp_compositor::surface::SubsurfaceMode;
use ldp_compositor::{DamageEngine, Surface, SurfaceId, SurfaceTree};
use ldp_core::geometry::Rect;

const OUT: Rect = Rect::new(0, 0, 128, 128);

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed.max(1))
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn range(&mut self, lo: i32, hi: i32) -> i32 {
        lo + (self.below((hi - lo + 1).max(1) as u64) as i32)
    }
}

fn id(n: u64) -> SurfaceId {
    SurfaceId::from_raw(n)
}

/// One to three random mutations against the authoring tree.
fn mutate(tree: &mut SurfaceTree, rng: &mut Rng, next_id: &mut u64) {
    // --- one to three mutations ----------------------------------
    for _ in 0..=rng.below(2) {
        let alive: Vec<SurfaceId> = {
            let mut v: Vec<SurfaceId> = tree.iter().map(Surface::id).collect();
            v.sort_by_key(|s| s.raw());
            v
        };
        match rng.below(6) {
            0 | 1 if alive.len() < 10 => {
                let new = id(*next_id);
                *next_id += 1;
                let x = rng.range(0, 60);
                let y = rng.range(0, 60);
                if alive.is_empty() || rng.below(2) == 0 {
                    tree.create_root(new, x, y).unwrap();
                } else {
                    let parent = alive[rng.below(alive.len() as u64) as usize];
                    tree.create_subsurface(new, parent).unwrap();
                }
                tree.pending_mut(new)
                    .unwrap()
                    .attach(Some(BufferAttachment {
                        width: 16,
                        height: 12,
                        generation: *next_id,
                    }));
                tree.commit(new).unwrap();
            }
            2 if !alive.is_empty() => {
                let target = alive[rng.below(alive.len() as u64) as usize];
                tree.set_position(target, rng.range(0, 60), rng.range(0, 60))
                    .unwrap();
                tree.commit(target).unwrap();
            }
            3 if !alive.is_empty() => {
                let target = alive[rng.below(alive.len() as u64) as usize];
                tree.pending_mut(target).unwrap().damage(&[Rect::new(
                    rng.range(0, 8),
                    rng.range(0, 8),
                    rng.range(1, 8) as u32,
                    rng.range(1, 8) as u32,
                )]);
                tree.commit(target).unwrap();
            }
            4 if alive.len() > 3 => {
                let kids = tree.children_of(None).to_vec();
                if kids.len() >= 2 {
                    let mover = kids[rng.below(kids.len() as u64) as usize];
                    let sibling = kids[rng.below(kids.len() as u64) as usize];
                    if mover != sibling {
                        if rng.below(2) == 0 {
                            tree.place_above(mover, sibling).unwrap();
                        } else {
                            tree.place_below(mover, sibling).unwrap();
                        }
                    }
                } else if tree.parent_of(alive[1]).is_some() {
                    let target = alive[rng.below(alive.len() as u64) as usize];
                    tree.set_mode(
                        target,
                        if rng.below(2) == 0 {
                            SubsurfaceMode::Desync
                        } else {
                            SubsurfaceMode::Sync
                        },
                    )
                    .unwrap();
                }
            }
            _ => {
                if alive.len() > 4 {
                    let victim = alive[rng.below(alive.len() as u64) as usize];
                    tree.destroy(victim).unwrap();
                }
            }
        }
    }
}

#[test]
fn snapshots_survive_a_random_mutation_storm() {
    let mut rng = Rng::new(0xABCD_1357);
    let mut tree = SurfaceTree::new();
    let mut next_id: u64 = 1;
    let mut kept: Vec<ldp_compositor::FrameSnapshot> = Vec::new();
    let mut last_digest: Vec<(u64, Rect, bool, usize)> = Vec::new();

    for round in 0..120 {
        mutate(&mut tree, &mut rng, &mut next_id);

        // A damage pass keeps the tree's bookkeeping realistic.
        let changes = tree.take_changes();
        DamageEngine::compute(&mut tree, &changes, OUT);

        // --- take a snapshot ------------------------------------------
        let snap = tree.snapshot();
        assert!(snap.check().is_ok(), "round {round}: invariants");

        // --- previously kept snapshots are unchanged -------------------
        // Digest of the freshest snapshot (sorted for determinism).
        let mut digest: Vec<(u64, Rect, bool, usize)> = snap
            .by_id
            .values()
            .map(|n| (n.id.raw(), n.bounds, n.mapped, n.children.len()))
            .collect();
        digest.sort_by_key(|d| d.0);
        if !last_digest.is_empty() {
            check_no_drift(&kept[kept.len() - 1], &last_digest, round);
        }
        last_digest = digest;

        // Keep at most the last three snapshots (Arc sharing across
        // them exercises structural sharing under mutation).
        kept.push(snap);
        if kept.len() > 3 {
            kept.remove(0);
        }

        // --- the fresh snapshot reflects the tree exactly --------------
        check_reflects_tree(&tree, &kept[kept.len() - 1], round);
    }
}

/// The previous snapshot still holds its exact digest and invariants.
fn check_no_drift(
    previous: &ldp_compositor::FrameSnapshot,
    last_digest: &[(u64, Rect, bool, usize)],
    round: usize,
) {
    let mut prev_digest: Vec<(u64, Rect, bool, usize)> = previous
        .by_id
        .values()
        .map(|n| (n.id.raw(), n.bounds, n.mapped, n.children.len()))
        .collect();
    prev_digest.sort_by_key(|d| d.0);
    assert_eq!(&prev_digest, last_digest, "round {round}: drift");
    assert!(previous.check().is_ok(), "round {round}: old invariants");
}

/// The fresh snapshot reflects the tree exactly (ids, bounds, mapping).
fn check_reflects_tree(tree: &SurfaceTree, snap: &ldp_compositor::FrameSnapshot, round: usize) {
    let mut tree_ids: Vec<u64> = tree.iter().map(|s| s.id().raw()).collect();
    tree_ids.sort_unstable();
    let mut snap_ids: Vec<u64> = snap.by_id.keys().map(|k| k.raw()).collect();
    snap_ids.sort_unstable();
    assert_eq!(tree_ids, snap_ids, "round {round}: id mismatch");
    for s in tree.iter() {
        let node = snap.node(s.id()).unwrap();
        let live = s.state().bounds().translate(s.position().0, s.position().1);
        assert_eq!(
            node.bounds,
            live,
            "round {round}: bounds of #{}",
            s.id().raw()
        );
        assert_eq!(node.mapped, s.state().is_mapped());
    }
}

#[test]
fn snapshot_render_order_matches_the_damage_walk() {
    let mut tree = SurfaceTree::new();
    tree.create_root(id(1), 0, 0).unwrap();
    tree.create_root(id(2), 0, 0).unwrap();
    tree.create_subsurface(id(3), id(1)).unwrap();
    tree.create_subsurface(id(4), id(3)).unwrap();
    tree.create_subsurface(id(5), id(1)).unwrap();
    let snap = tree.snapshot();
    // Back-to-front: root 1's family with children (front-most first
    // in rendering, so in the back-to-front list the deepest-front
    // child appears latest... children precede parents, and within
    // siblings the front-most precedes): 4, 3, 5, 1, 2.
    assert_eq!(
        snap.render_order()
            .iter()
            .map(|s| s.raw())
            .collect::<Vec<_>>(),
        vec![1, 3, 4, 5, 2]
    );
    assert!(snap.check().is_ok());
    // The damage engine's front-to-back flatten reversed is exactly
    // the snapshot's back-to-front draw list.
    let flat = ldp_compositor::occlusion::flatten(&tree).nodes;
    let mut engine_back_to_front: Vec<u64> = flat.iter().map(|n| n.id.raw()).collect();
    engine_back_to_front.reverse();
    let snap_order: Vec<u64> = snap.render_order().iter().map(|s| s.raw()).collect();
    assert_eq!(engine_back_to_front, snap_order);
}
