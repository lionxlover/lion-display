//! Exit criterion (roadmap Phase 6): damage equals a reference model on
//! a randomized scene-graph corpus.
//!
//! The reference model is an **independent per-pixel implementation of
//! the fold semantics**: every surface carries a per-cell version grid;
//! each output cell's composited value is the ordered list of
//! `(surface, version)` contributions from top to bottom, truncated at
//! the topmost opaque surface (everything below an opaque surface is
//! wiped). A cell must be repainted iff its value changed since the
//! last frame.
//!
//! The corpus drives the real [`SurfaceTree`] + [`DamageEngine`] and a
//! parallel cell-grid model through the same random operation streams
//! (create/commit/damage/move/restack/mode-flip/destroy), then asserts
//! exact cell-set equality of:
//!
//! * the engine's repaint damage vs. the model's changed-value cells,
//! * the engine's per-surface presentation damage vs. the model's
//!   "surface contributes to the fold and its version changed" cells.
//!
//! Corpus restrictions (each covered by unit tests elsewhere): identity
//! transforms and surface-coordinate damage only (the damage-buffer
//! conversion is unit-tested), one damage pass per frame (cross-frame
//! damage accumulation is unit-tested).

#[path = "corpus/model.rs"]
mod model;

use ldp_compositor::state::BufferAttachment;
use ldp_compositor::surface::SubsurfaceMode;
use ldp_compositor::{DamageEngine, Surface, SurfaceId, SurfaceTree};
use ldp_core::geometry::{Rect, Region};
use ldp_core::scale::ScaleFactor;
use model::{CellValues, Grid, Model, GRID};

const OUT: Rect = Rect::new(0, 0, 64, 64);
// ---------------------------------------------------------------------------
// Deterministic RNG (xorshift64)
// ---------------------------------------------------------------------------

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

    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
}

// ---------------------------------------------------------------------------
// Grid helpers
// ---------------------------------------------------------------------------

fn empty_grid() -> Grid {
    vec![vec![false; GRID]; GRID]
}

fn region_to_grid(region: &Region) -> Grid {
    let mut grid = empty_grid();
    for r in region {
        for y in r.y.max(0)..r.bottom().min(GRID as i32) {
            for x in r.x.max(0)..r.right().min(GRID as i32) {
                grid[y as usize][x as usize] = true;
            }
        }
    }
    grid
}

fn first_mismatch(got: &Grid, want: &Grid) -> Option<(usize, usize, bool, bool)> {
    for y in 0..GRID {
        for x in 0..GRID {
            if got[y][x] != want[y][x] {
                return Some((x, y, got[y][x], want[y][x]));
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// The corpus driver
// ---------------------------------------------------------------------------

struct Driver {
    tree: SurfaceTree,
    model: Model,
    next_id: u64,
    gen_counter: u64,
}

impl Driver {
    fn new() -> Driver {
        Driver {
            tree: SurfaceTree::new(),
            model: Model::default(),
            next_id: 1,
            gen_counter: 1,
        }
    }

    fn fresh_id(&mut self) -> SurfaceId {
        let id = SurfaceId::from_raw(self.next_id);
        self.next_id += 1;
        id
    }

    fn alive(&self) -> Vec<SurfaceId> {
        // Sorted: the op stream must be reproducible, and HashMap
        // iteration order is randomized per process.
        let mut ids: Vec<SurfaceId> = self.tree.iter().map(Surface::id).collect();
        ids.sort_by_key(|s| s.raw());
        ids
    }

    /// A request burst + commit on one surface (mirrored).
    fn request_burst(&mut self, rng: &mut Rng, id: SurfaceId) {
        let scale_q8 = if rng.chance(20) { 512 } else { 256 };
        let attach = rng.chance(60);
        let new_gen = rng.chance(30);
        let state = self.tree.get(id).unwrap().state();
        let old = state.buffer;
        let (dev_w, dev_h) = if rng.chance(25) {
            (rng.range(4, 20) as u32, rng.range(4, 20) as u32)
        } else if let Some(b) = old {
            // Keep the size; only the generation may change.
            (b.width, b.height)
        } else {
            (rng.range(4, 20) as u32, rng.range(4, 20) as u32)
        };
        let gen = if new_gen {
            self.gen_counter += 1;
            self.gen_counter
        } else if let Some(b) = old {
            b.generation
        } else {
            self.gen_counter += 1;
            self.gen_counter
        };
        if attach {
            let spec = (dev_w, dev_h, gen, scale_q8);
            self.tree
                .pending_mut(id)
                .unwrap()
                .attach(Some(BufferAttachment {
                    width: dev_w,
                    height: dev_h,
                    generation: gen,
                }));
            // The buffer scale is persistent surface state in the
            // engine (set_buffer_scale is its own request); the model
            // attaches it with the buffer. Always issuing the request
            // keeps both sides on the same scale after every attach.
            self.tree
                .pending_mut(id)
                .unwrap()
                .set_buffer_scale(ScaleFactor::from_q8(scale_q8).unwrap());
            self.model.scratch.entry(id).or_default().attach = Some(spec);
            self.model.scratch.entry(id).or_default().attach_set = true;
        }
        // Damage in surface (logical) coordinates.
        let mut damage_rects: Vec<Rect> = Vec::new();
        if rng.chance(60) {
            let logical = self.model.surfaces[&id].logical;
            let (surf_w, surf_h) = if logical == (0, 0) { (8, 8) } else { logical };
            for _ in 0..=rng.below(1) {
                let px = rng.range(0, (surf_w as i32 - 2).max(1));
                let py = rng.range(0, (surf_h as i32 - 2).max(1));
                let pw = rng.range(1, (surf_w as i32 - px).max(1));
                let ph = rng.range(1, (surf_h as i32 - py).max(1));
                damage_rects.push(Rect::new(px, py, pw as u32, ph as u32));
            }
            self.tree.pending_mut(id).unwrap().damage(&damage_rects);
            self.model
                .scratch
                .entry(id)
                .or_default()
                .damage
                .clone_from(&damage_rects);
        }
        // Opaque region change.
        let mut opaque_rects: Vec<Rect> = Vec::new();
        if rng.chance(30) {
            let logical = self.model.surfaces[&id].logical;
            let (surf_w, surf_h) = if logical == (0, 0) { (8, 8) } else { logical };
            for _ in 0..rng.below(3) {
                let px = rng.range(0, (surf_w as i32 - 2).max(1));
                let py = rng.range(0, (surf_h as i32 - 2).max(1));
                let pw = rng.range(1, (surf_w as i32 - px).max(1));
                let ph = rng.range(1, (surf_h as i32 - py).max(1));
                opaque_rects.push(Rect::new(px, py, pw as u32, ph as u32));
            }
            let mut region = Region::new();
            for r in &opaque_rects {
                region.add(*r);
            }
            self.tree.pending_mut(id).unwrap().set_opaque_region(region);
            self.model.scratch.entry(id).or_default().opaque = Some(opaque_rects.clone());
        }
        if std::env::var("LDP_CORPUS_TRACE").is_ok() {
            eprintln!(
                "    [op] burst on #{}: attach={:?} old={:?} damage={:?} opaque={:?}",
                id.raw(),
                attach.then_some((dev_w, dev_h, gen, scale_q8)),
                old,
                damage_rects,
                opaque_rects
            );
        }
        // Commit (sync subs stash; roots/desync apply + cascade).
        self.tree.commit(id).unwrap();
        self.model.commit(id);
    }

    fn random_op(&mut self, rng: &mut Rng) {
        let alive = self.alive();
        if alive.is_empty() {
            // The first op must seed the tree.
            let id = self.fresh_id();
            let x = rng.range(0, 40);
            let y = rng.range(0, 40);
            self.tree.create_root(id, x, y).unwrap();
            self.model.create_root(id, x, y);
            self.request_burst(rng, id);
            return;
        }
        let choice = rng.below(10);
        match choice {
            0 | 1 if alive.len() < 12 => {
                // Create a root with an initial commit.
                let id = self.fresh_id();
                let x = rng.range(0, 40);
                let y = rng.range(0, 40);
                self.tree.create_root(id, x, y).unwrap();
                self.model.create_root(id, x, y);
                self.request_burst(rng, id);
            }
            2 | 3 if alive.len() < 14 => {
                // Create a subsurface under a random parent.
                let parent = alive[rng.below(alive.len() as u64) as usize];
                let id = self.fresh_id();
                self.tree.create_subsurface(id, parent).unwrap();
                self.model.create_sub(id, parent);
                if rng.chance(30) {
                    self.tree.set_mode(id, SubsurfaceMode::Desync).unwrap();
                    self.model.set_mode(id, true);
                }
                self.request_burst(rng, id);
                // Maybe place it somewhere in the parent.
                if rng.chance(50) {
                    let x = rng.range(0, 24);
                    let y = rng.range(0, 24);
                    self.tree.set_position(id, x, y).unwrap();
                    self.model.set_position(id, x, y);
                    self.tree.commit(id).unwrap();
                    self.model.commit(id);
                }
            }
            4..=6 => {
                // Commit burst on a random alive surface.
                let id = alive[rng.below(alive.len() as u64) as usize];
                self.request_burst(rng, id);
            }
            7 => {
                // Move a random surface.
                let id = alive[rng.below(alive.len() as u64) as usize];
                let x = rng.range(0, 40);
                let y = rng.range(0, 40);
                self.tree.set_position(id, x, y).unwrap();
                self.model.set_position(id, x, y);
                self.tree.commit(id).unwrap();
                self.model.commit(id);
            }
            8 => {
                // Restack within a random parent (roots included).
                let mut parents: Vec<Option<SurfaceId>> = vec![None];
                for s in alive {
                    let kids = self.tree.children_of(Some(s));
                    if kids.len() >= 2 {
                        parents.push(Some(s));
                    }
                }
                let parent = parents[rng.below(parents.len() as u64) as usize];
                let kids = self.tree.children_of(parent).to_vec();
                if kids.len() >= 2 {
                    let mover = kids[rng.below(kids.len() as u64) as usize];
                    let sibling = kids[rng.below(kids.len() as u64) as usize];
                    if mover != sibling {
                        let above = rng.chance(50);
                        if above {
                            self.tree.place_above(mover, sibling).unwrap();
                        } else {
                            self.tree.place_below(mover, sibling).unwrap();
                        }
                        self.model.restack(parent, mover, sibling, above);
                    }
                }
            }
            _ => {
                // Mode flip or destroy.
                if rng.chance(50) && alive.len() > 2 {
                    let id = alive[rng.below(alive.len() as u64) as usize];
                    if self.tree.parent_of(id).is_some() {
                        let desync = rng.chance(50);
                        let mode = if desync {
                            SubsurfaceMode::Desync
                        } else {
                            SubsurfaceMode::Sync
                        };
                        self.tree.set_mode(id, mode).unwrap();
                        self.model.set_mode(id, desync);
                    }
                } else if alive.len() > 3 {
                    let id = alive[rng.below(alive.len() as u64) as usize];
                    self.tree.destroy(id).unwrap();
                    self.model.destroy(id);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The test
// ---------------------------------------------------------------------------

#[test]
fn randomized_corpus_matches_the_reference_model() {
    for seed in [
        0x5EED_1234,
        0xC0FF_EE01,
        0xBADC_0DE2,
        0x00C0_FFEE,
        0xD15E_A5E5,
    ] {
        run_corpus(seed, 80);
    }
}

/// Compare the engine's damage against the model's expectation.
#[allow(clippy::too_many_lines)]
fn compare_frame(
    damage: &ldp_compositor::FrameDamage,
    driver: &Driver,
    new_values: &CellValues,
    old_values: &CellValues,
    seed: u64,
    frame: usize,
) {
    // Repaint comparison.
    let mut expected = empty_grid();
    for y in 0..GRID {
        for x in 0..GRID {
            if new_values[y][x] != old_values[y][x] {
                expected[y][x] = true;
            }
        }
    }
    let got = region_to_grid(&damage.repaint);
    if let Some((x, y, g, w)) = first_mismatch(&got, &expected) {
        if std::env::var("LDP_CORPUS_DEBUG").is_ok() {
            eprintln!(
                "=== seed {seed:#x} frame {frame} mismatch at ({x},{y}) engine={g} model={w}"
            );
            eprintln!("--- model surfaces:");
            let mut ids: Vec<u64> = driver.model.surfaces.keys().map(|k| k.raw()).collect();
            ids.sort_unstable();
            for raw in ids {
                let id = SurfaceId::from_raw(raw);
                let s = &driver.model.surfaces[&id];
                let parent = s.parent.map(SurfaceId::raw);
                eprintln!(
            "  #{raw}: parent={parent:?} pos={:?} logical={:?} buffer={:?} opaque={:?} bumped_cells={}",
            s.position,
            s.logical,
            s.buffer,
            s.opaque,
            driver
                .model
                .bumped
                .get(&id)
                .map_or(0, |g| g.iter().flatten().filter(|b| **b).count())
            );
            }
            eprintln!("--- old value at ({x},{y}): {:?}", old_values[y][x]);
            eprintln!("--- new value at ({x},{y}): {:?}", new_values[y][x]);
            eprintln!(
                "--- engine repaint rects: {:?}",
                damage.repaint.iter().collect::<Vec<_>>()
            );
            eprintln!("--- engine surfaces (engine side):");
            for s in driver.tree.iter() {
                eprintln!(
                    "   #{}: parent={:?} pos={:?} cur_bounds={:?} last_bounds={:?} last_occ={:?}",
                    s.id().raw(),
                    driver.tree.parent_of(s.id()).map(SurfaceId::raw),
                    s.position(),
                    s.state().bounds(),
                    s.last_bounds(),
                    s.last_occlusion().bounds(),
                );
            }
            eprintln!(
                "--- engine presentation: {:?}",
                damage
                    .presentation
                    .iter()
                    .map(|(k, v)| (k.raw(), v.bounds()))
                    .collect::<Vec<_>>()
            );
        }
        panic!(
            "seed {seed:#x} frame {frame}: repaint mismatch at ({x},{y}): \
         engine={g} model={w}"
        );
    }

    // Presentation comparison per surface.
    let alive = driver.alive();
    for id in alive {
        let mut want = empty_grid();
        let bumped = driver.model.bumped.get(&id);
        let off = model_offset(driver, id);
        for y in 0..GRID {
            for x in 0..GRID {
                let contributes = new_values[y][x].iter().any(|(raw, ..)| *raw == id.raw());
                if contributes {
                    let lx = x as i32 - off.0;
                    let ly = y as i32 - off.1;
                    let bumped_here = bumped.is_some_and(|b| {
                        lx >= 0
                            && ly >= 0
                            && (ly as usize) < b.len()
                            && (lx as usize) < b[ly as usize].len()
                            && b[ly as usize][lx as usize]
                    });
                    want[y][x] = bumped_here;
                }
            }
        }
        let got = damage
            .presentation
            .get(&id)
            .map_or_else(empty_grid, region_to_grid);
        if let Some((x, y, g, w)) = first_mismatch(&got, &want) {
            panic!(
                "seed {seed:#x} frame {frame}: presentation mismatch for \
             surface {} at ({x},{y}): engine={g} model={w}",
                id.raw()
            );
        }
    }
}

fn run_corpus(seed: u64, frames: usize) {
    let mut rng = Rng::new(seed);
    let mut driver = Driver::new();
    let mut old_values: CellValues = vec![vec![Vec::new(); GRID]; GRID];

    for frame in 0..frames {
        // Random ops.
        let n_ops = rng.below(4);
        for _ in 0..n_ops {
            driver.random_op(&mut rng);
        }
        // Root sweep: null commits flush sync chains.
        let roots: Vec<SurfaceId> = driver.tree.children_of(None).to_vec();
        for r in roots {
            if rng.chance(40) {
                driver.tree.commit(r).unwrap();
                driver.model.commit(r);
            }
        }

        // The engine's damage pass.
        let changes = driver.tree.take_changes();
        if std::env::var("LDP_CORPUS_DEBUG").is_ok() {
            eprintln!(
                "   [changes] restacks={:?} removals={:?}",
                changes.restacks,
                changes
                    .removals
                    .iter()
                    .map(|r| r.id.raw())
                    .collect::<Vec<_>>()
            );
        }
        let damage = DamageEngine::compute(&mut driver.tree, &changes, OUT);

        // The model's fold values.
        let new_values = driver.model.values();

        if std::env::var("LDP_CORPUS_TRACE").is_ok() {
            eprintln!("== frame {frame}");
            for s in driver.tree.iter() {
                let id = s.id().raw();
                let m = &driver.model.surfaces[&s.id()];
                let bounds = s.state().bounds();
                eprintln!(
                    "   #{id}: eng_cur=({},{}) eng_last=({},{}) mdl_cur=({},{})",
                    bounds.w,
                    bounds.h,
                    s.last_bounds().w,
                    s.last_bounds().h,
                    m.logical.0,
                    m.logical.1
                );
            }
        }

        compare_frame(&damage, &driver, &new_values, &old_values, seed, frame);

        old_values = new_values;
        driver.model.bumped.clear();
    }
}

fn model_offset(driver: &Driver, id: SurfaceId) -> (i32, i32) {
    // Accumulate positions up the chain (model side).
    let mut offset = driver.model.surfaces[&id].position;
    let mut current = driver.model.surfaces[&id].parent;
    while let Some(p) = current {
        let ps = &driver.model.surfaces[&p];
        offset = (offset.0 + ps.position.0, offset.1 + ps.position.1);
        current = ps.parent;
    }
    offset
}
