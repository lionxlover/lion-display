//! The split-point solver.

use crate::facts::{LayerFacts, OutputFacts};
use crate::inventory::PlaneInventory;
use crate::plan::{Demotion, PlaneAssignment, PlaneRole, ScanoutPlan};
use ldp_display::ids::PlaneId;

/// The plane assignment engine.
///
/// Stateless — one `Assigner` serves every output (or construct one
/// per output; the decision is pure either way). See the crate docs
/// for the split-point doctrine this type implements.
#[derive(Clone, Copy, Debug, Default)]
pub struct Assigner;

/// One layer's independent plane eligibility.
#[derive(Clone, Copy, Debug)]
enum Eligibility {
    /// May ride an overlay plane as-is.
    Overlay,
    /// May also take the primary role: covers the output opaquely.
    Bottom,
    /// Cannot ride any plane; the demotion says why.
    Rejected(Demotion),
}

impl Eligibility {
    const fn capable(self) -> bool {
        matches!(self, Self::Overlay | Self::Bottom)
    }
}

impl Assigner {
    /// Solve one frame: map `stack` (bottom-to-front order) onto
    /// `inventory`'s plane set for `output`.
    ///
    /// The result is the maximally-offloading plan the doctrine
    /// admits. The composite set is always a *prefix* of the stack
    /// and the offloaded set a *suffix* — a composited layer renders
    /// into the canvas on the primary plane, so anything beneath it
    /// must render there too (nothing can render between hardware
    /// planes). The solver therefore:
    ///
    /// 1. grades every layer's independent eligibility (the policy
    ///    table — fb, 1:1, untransformed, color, in-bounds, opacity,
    ///    capability);
    /// 2. finds the deepest split `k` whose suffix `[k..]` is fully
    ///    capable;
    /// 3. fits the suffix to the overlay budget (`n - k <= slots`,
    ///    or `n - 1 <= slots` when the bottom layer itself would ride
    ///    the primary — the zero-composite shape), pushing the split
    ///    up when overlays run short;
    /// 4. assigns planes within the suffix by depth-first search
    ///    over per-layer candidate sets (ordered `(zpos, id)`
    ///    ascending), backtracking when an early layer grabs the only
    ///    plane a later one can use — and shrinking the suffix (never
    ///    enlarging the demotion set beyond the doctrine) if the
    ///    search still fails;
    /// 5. emits one [`Demotion`] per composite layer: the named,
    ///    honest reason it renders in software.
    ///
    /// # Panics
    ///
    /// Never in practice: the two `expect` sites (the primary in the
    /// zero-on-primary shape, an eligible layer's fb) are guarded by
    /// the same conditions that admit the shape.
    #[must_use]
    pub fn solve(
        &self,
        stack: &[LayerFacts],
        inventory: &PlaneInventory,
        output: &OutputFacts,
    ) -> ScanoutPlan {
        let n = stack.len();
        if n == 0 {
            return ScanoutPlan::default();
        }
        let eligibility: Vec<Eligibility> = stack
            .iter()
            .map(|facts| Self::eligibility(facts, inventory, output))
            .collect();

        // (2) The deepest fully-capable suffix.
        let mut k = n;
        while k > 0 && eligibility[k - 1].capable() {
            k -= 1;
        }

        // (3) Zero-composite: the whole stack rides planes, no canvas
        // pass at all. Two shapes reach it, both requiring k == 0 and
        // a Bottom-grade bottom layer (it covers the output opaquely,
        // so nothing needs to render beneath it):
        //
        // * **on the primary** — the classic direct-scanout frame; the
        //   primary must actually take the bottom layer's (format,
        //   modifier), with overlay room for the n-1 layers above;
        // * **on the lowest capable overlay** — the fullscreen-video
        //   shape on drivers whose primary refuses the format (older
        //   primaries without 4:2:0): the opaque bottom layer covers
        //   the stale canvas entirely, all n layers ride overlays.
        let primary_takes_bottom = inventory
            .primary
            .as_ref()
            .is_some_and(|p| p.supports(stack[0].format, stack[0].modifier));
        let bottom_overlay_slot = if primary_takes_bottom {
            None
        } else {
            inventory
                .overlays
                .iter()
                .position(|p| p.supports(stack[0].format, stack[0].modifier))
        };
        let slots = inventory.overlay_slots();
        let zero_on_primary = k == 0
            && matches!(eligibility[0], Eligibility::Bottom)
            && primary_takes_bottom
            && n - 1 <= slots;
        let zero_on_overlay = k == 0
            && matches!(eligibility[0], Eligibility::Bottom)
            && bottom_overlay_slot.is_some()
            && n <= slots;
        let mut zero_composite = zero_on_primary || zero_on_overlay;

        // Fit the overlay budget for the non-zero shapes: the suffix
        // [k..n] must fit the overlays. Push the split up as far as
        // needed; capability is preserved because [k..n] was fully
        // capable and shrinking the suffix only drops capable layers
        // into the composite prefix.
        let budget_k = |k: usize| k.max(n.saturating_sub(slots)).max(1);

        // (4) Plane assignment inside the suffix, with DFS repair.
        // zero: layer 0 → primary (or the lowest capable overlay),
        // DFS over the layers above; else: the canvas → primary, DFS
        // over [k..n]. A failed search shrinks the suffix one layer at
        // a time (the demotion set grows only as the hardware forces
        // it).
        let mut assignments: Vec<PlaneAssignment> = Vec::new();
        if zero_composite {
            let placed = Self::place_zero_shape(
                stack,
                inventory,
                zero_on_primary,
                bottom_overlay_slot,
                &mut assignments,
            );
            if !placed {
                assignments.clear();
                zero_composite = false;
                k = budget_k(1);
            }
        } else {
            k = budget_k(k);
        }
        if !zero_composite {
            while !Self::place_suffix(stack, k, inventory, None, 0, &mut assignments) {
                assignments.clear();
                k += 1;
                if k > n {
                    k = n;
                    break;
                }
            }
        }

        // (5) The demotion ledger: one named reason per composite
        // layer, in stack order.
        let demotions = Self::demotion_ledger(&eligibility, &assignments, k, zero_composite);

        ScanoutPlan {
            composite_count: n - assignments.len(),
            assignments,
            demotions,
            zero_composite,
            canvas_on_primary: !zero_composite,
        }
    }

    /// Place the zero-composite shapes: the bottom layer on the
    /// primary (the classic direct-scanout frame) or, when the
    /// primary refuses its format, on the lowest capable overlay (the
    /// fullscreen-video shape — the opaque layer covers the stale
    /// canvas entirely). The layers above ride the remaining overlays.
    ///
    /// Returns whether the whole stack placed; on failure `out` is
    /// empty and the caller falls back to the split shape.
    ///
    /// # Panics
    ///
    /// Never in practice: both `expect` sites are guarded by the
    /// conditions that admitted the zero shape.
    fn place_zero_shape(
        stack: &[LayerFacts],
        inventory: &PlaneInventory,
        on_primary: bool,
        bottom_overlay_slot: Option<usize>,
        out: &mut Vec<PlaneAssignment>,
    ) -> bool {
        let bottom = &stack[0];
        if on_primary {
            let plane = inventory
                .primary
                .as_ref()
                .expect("zero-on-primary requires the primary");
            out.push(PlaneAssignment {
                layer: 0,
                plane: plane.info.id,
                role: PlaneRole::Primary,
                fb: bottom.fb.expect("eligible layers carry an fb"),
                src: (0, 0, bottom.width, bottom.height),
                dst: (bottom.dest.x, bottom.dest.y, bottom.dest.w, bottom.dest.h),
                zpos: plane.zpos,
            });
            Self::place_suffix(stack, 1, inventory, None, 0, out)
        } else {
            let slot = bottom_overlay_slot.expect("zero-on-overlay requires a slot");
            let plane = &inventory.overlays[slot];
            out.push(PlaneAssignment {
                layer: 0,
                plane: plane.info.id,
                role: PlaneRole::Overlay,
                fb: bottom.fb.expect("eligible layers carry an fb"),
                src: (0, 0, bottom.width, bottom.height),
                dst: (bottom.dest.x, bottom.dest.y, bottom.dest.w, bottom.dest.h),
                zpos: plane.zpos,
            });
            Self::place_suffix(stack, 1, inventory, Some(slot), plane.zpos + 1, out)
        }
    }

    /// One [`Demotion`] per composite layer: the rejected reason when
    /// the layer itself is incapable, `BottomNotCovering` for a solo
    /// bottom that failed the covering bar, `BelowSplit` for the
    /// layers the canvas renders because something above composites.
    fn demotion_ledger(
        eligibility: &[Eligibility],
        assignments: &[PlaneAssignment],
        k: usize,
        zero_composite: bool,
    ) -> Vec<(usize, Demotion)> {
        let mut demotions = Vec::new();
        for (i, e) in eligibility.iter().enumerate() {
            if assignments.iter().any(|a| a.layer == i) {
                continue;
            }
            let reason = match e {
                Eligibility::Rejected(d) => *d,
                Eligibility::Overlay => {
                    if i == 0 && k == 1 && !zero_composite {
                        Demotion::BottomNotCovering
                    } else {
                        Demotion::BelowSplit
                    }
                }
                Eligibility::Bottom => Demotion::BelowSplit,
            };
            demotions.push((i, reason));
        }
        demotions
    }

    /// Independent eligibility of one layer — the policy table.
    fn eligibility(
        facts: &LayerFacts,
        inventory: &PlaneInventory,
        output: &OutputFacts,
    ) -> Eligibility {
        if facts.needs_backdrop {
            return Eligibility::Rejected(Demotion::NeedsBackdrop);
        }
        if facts.styled {
            return Eligibility::Rejected(Demotion::Styled);
        }
        if facts.fb.is_none() {
            return Eligibility::Rejected(Demotion::NoFb);
        }
        if facts.transform != ldp_core::geometry::Transform::Normal {
            return Eligibility::Rejected(Demotion::Transformed);
        }
        if !facts.is_one_to_one() {
            return Eligibility::Rejected(Demotion::Scaled);
        }
        if facts.color != output.color {
            return Eligibility::Rejected(Demotion::ColorMismatch);
        }
        // Fully inside the output (v1: no partially-offscreen planes).
        if facts.dest.x < 0
            || facts.dest.y < 0
            || i64::from(facts.dest.x) + i64::from(facts.dest.w) > i64::from(output.width)
            || i64::from(facts.dest.y) + i64::from(facts.dest.h) > i64::from(output.height)
        {
            return Eligibility::Rejected(Demotion::OutOfBounds);
        }
        if facts.opacity < 1.0 {
            return Eligibility::Rejected(Demotion::Translucent);
        }
        // Capability: some available plane takes the (format, modifier).
        let primary_takes = inventory
            .primary
            .as_ref()
            .is_some_and(|p| p.supports(facts.format, facts.modifier));
        let overlay_takes = inventory
            .overlays
            .iter()
            .any(|p| p.supports(facts.format, facts.modifier));
        if !primary_takes && !overlay_takes {
            return Eligibility::Rejected(Demotion::FormatUnsupported);
        }
        // Bottom-worthiness: covering the output opaquely (only the
        // bottom layer of a zero-composite stack ever needs it — the
        // canvas covers for everyone else).
        if facts.covers_output(output) && facts.opaque_covers_dest() {
            Eligibility::Bottom
        } else {
            Eligibility::Overlay
        }
    }

    /// Assign overlay planes to `stack[from..]` by depth-first search
    /// over per-layer candidate sets. Candidates are the overlays
    /// whose capabilities take the layer's (format, modifier),
    /// ordered `(zpos, id)` ascending; the search never reuses a slot,
    /// requires each successive layer's plane to stack strictly above
    /// the previous one's (the zpos monotonicity the display's blend
    /// order demands), and backtracks on exhaustion — exactly the
    /// repair for the greedy failure mode (an early layer
    /// monopolizing the only plane a later layer can use, or the only
    /// zpos slot it can stack under).
    ///
    /// Returns whether the whole suffix placed; on failure `out` is
    /// left empty (the caller shrinks the suffix and retries).
    fn place_suffix(
        stack: &[LayerFacts],
        from: usize,
        inventory: &PlaneInventory,
        skip: Option<usize>,
        floor: u64,
        out: &mut Vec<PlaneAssignment>,
    ) -> bool {
        let layers: Vec<usize> = (from..stack.len()).collect();
        // Per-layer candidates: (overlay slot, plane id, zpos), the
        // consumed slot excluded.
        let candidates: Vec<Vec<(usize, PlaneId, u64)>> = layers
            .iter()
            .map(|&i| {
                inventory
                    .overlays
                    .iter()
                    .enumerate()
                    .filter(|(slot, p)| {
                        Some(*slot) != skip && p.supports(stack[i].format, stack[i].modifier)
                    })
                    .map(|(slot, p)| (slot, p.info.id, p.zpos))
                    .collect()
            })
            .collect();
        let mut chosen: Vec<Option<usize>> = vec![None; layers.len()];
        if !Self::dfs(&candidates, 0, floor, &mut chosen) {
            out.clear();
            return false;
        }
        for (step, slot) in chosen.iter().enumerate() {
            let Some(slot) = *slot else {
                continue;
            };
            let layer = layers[step];
            let plane = &inventory.overlays[slot];
            out.push(PlaneAssignment {
                layer,
                plane: plane.info.id,
                role: PlaneRole::Overlay,
                fb: stack[layer].fb.expect("eligible layers carry an fb"),
                src: (0, 0, stack[layer].width, stack[layer].height),
                dst: (
                    stack[layer].dest.x,
                    stack[layer].dest.y,
                    stack[layer].dest.w,
                    stack[layer].dest.h,
                ),
                zpos: plane.zpos,
            });
        }
        true
    }

    /// The DFS: give step `i` an unused candidate slot stacking
    /// strictly above `floor_zpos` (the previous layer's plane) and
    /// recurse. The suffix length is at most the overlay count (the
    /// budget fit), so the search space is bounded by `slots!` —
    /// single digits on every real device.
    fn dfs(
        candidates: &[Vec<(usize, PlaneId, u64)>],
        i: usize,
        floor_zpos: u64,
        chosen: &mut [Option<usize>],
    ) -> bool {
        if i == candidates.len() {
            return true;
        }
        for &(slot, _plane, zpos) in &candidates[i] {
            if zpos < floor_zpos {
                continue;
            }
            if chosen[..i].contains(&Some(slot)) {
                continue;
            }
            chosen[i] = Some(slot);
            if Self::dfs(candidates, i + 1, zpos + 1, chosen) {
                return true;
            }
            chosen[i] = None;
        }
        false
    }
}
