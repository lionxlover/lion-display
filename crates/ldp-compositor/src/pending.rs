//! The pending state accumulator and the commit effect record.
//!
//! [`PendingState`] accumulates request-shaped setters mirroring the
//! `ldp.core.surface` requests (`attach`, `damage`, `damage_buffer`,
//! `set_transform`, `set_buffer_scale`, `set_input_region`,
//! `set_opaque_region`, `set_color`, `set_hdr_metadata`,
//! `set_presentation_mode`) and applies them atomically onto the
//! committed [`SurfaceState`] — the surface-local half of the atomic
//! commit contract. Sync-mode subsurfaces *stash* their pending state
//! (merged over an earlier stash) until an ancestor applies it; see
//! `surface.rs` and `tree.rs` for the cascade.
//!
//! Commit damage rules (surface-local, feeding the engine's content
//! rule): explicit damage accumulates in the pending region; a
//! *different* buffer attached with no explicit damage damages the
//! whole surface (content is unknowable); damage is clipped to the new
//! bounds. A detach contributes no content damage — the unmap is a
//! coverage change handled exactly by the damage engine's coverage
//! rule.

use ldp_core::color::{ColorDescription, HdrMetadata};
use ldp_core::geometry::{Rect, Region, Transform};
use ldp_core::limits::Limits;
use ldp_core::scale::ScaleFactor;
use ldp_core::time::PresentationMode;

use crate::state::{BufferAttachment, SurfaceState};

/// The accumulated-damage rectangle ceiling before the pending region
/// collapses to its bounding box.
///
/// Each `surface.damage` request is validated against
/// [`Limits::region_rects`] (4,096 rects), but nothing bounded the
/// *accumulated* region between commits: a client that never commits
/// could park one 4,096-rect request after another, millions of rects
/// deep, all of which the commit path would clone, clip, and walk
/// node-by-node through the damage engine's O(n·m) region algebra —
/// an unbounded-amplification stall authored from protocol-legal
/// requests. Damage is a *repaint hint*: a superset is always
/// semantically correct (repaint more, never less), so past the cap
/// the region folds to one rect — its own bounding box — and the
/// amplification ceiling becomes a single rectangle's repaint. The
/// cap is twice the per-request limit: every legitimate frame's
/// damage (a real frame sits under a hundred rects) stays exact;
/// only flood-grade accumulation pays the conservative fold.
const ACCUMULATED_DAMAGE_RECT_CAP: usize = 2 * Limits::DEFAULT.region_rects as usize;

/// The pending state accumulator: request-shaped setters mirroring the
/// `ldp.core.surface` requests, applied atomically on commit.
#[derive(Clone, Debug)]
pub struct PendingState {
    buffer: Option<BufferAttachment>,
    buffer_set: bool,
    transform: Option<Transform>,
    scale: Option<ScaleFactor>,
    input_region: Option<Region>,
    opaque_region: Option<Region>,
    color: Option<ColorDescription>,
    hdr: Option<HdrMetadata>,
    /// Whether a `set_hdr_metadata` request ran (so `hdr` itself can
    /// carry `None` = clear).
    hdr_set: bool,
    presentation: Option<PresentationMode>,
    /// Accumulated surface-local damage (already buffer-converted where
    /// the request was `damage_buffer`).
    damage: Region,
}

impl Default for PendingState {
    fn default() -> Self {
        PendingState {
            buffer: None,
            buffer_set: false,
            transform: None,
            scale: None,
            input_region: None,
            opaque_region: None,
            color: None,
            hdr: None,
            hdr_set: false,
            presentation: None,
            damage: Region::new(),
        }
    }
}

impl PendingState {
    /// Set the pending buffer (`None` detaches).
    pub fn attach(&mut self, buffer: Option<BufferAttachment>) {
        self.buffer = buffer;
        self.buffer_set = true;
    }

    /// Fold the accumulated damage to its bounding box once the rect
    /// count crosses [`ACCUMULATED_DAMAGE_RECT_CAP`] — the conservative
    /// superset that keeps flood-grade accumulation bounded (see the
    /// constant's docs for the doctrine).
    fn compact_if_flooded(&mut self) {
        if self.damage.len() > ACCUMULATED_DAMAGE_RECT_CAP {
            let bounds = self.damage.bounds();
            self.damage = Region::from_rect(bounds);
        }
    }

    /// Add surface-coordinate damage for the pending commit.
    pub fn damage(&mut self, rects: &[Rect]) {
        for r in rects {
            self.damage.add(*r);
        }
        self.compact_if_flooded();
    }

    /// Add buffer-coordinate damage (pre-transform), converted against
    /// the **pending** geometry — the buffer this commit will attach,
    /// the transform it will carry — falling back to the committed
    /// state for unset fields (damage describes the pending buffer).
    pub fn damage_buffer(&mut self, rects: &[Rect], current: &SurfaceState) {
        let effective = SurfaceState {
            buffer: self.buffer.or(current.buffer),
            transform: self.transform.unwrap_or(current.transform),
            scale: self.scale.unwrap_or(current.scale),
            input_region: Region::new(),
            opaque_region: Region::new(),
            color: ColorDescription::srgb_sdr(),
            hdr: None,
            presentation: PresentationMode::Vsync,
        };
        for r in rects {
            let s = effective.buffer_to_surface(*r);
            if !s.is_empty() {
                self.damage.add(s);
            }
        }
        self.compact_if_flooded();
    }

    /// Set the pending buffer-to-surface transform.
    pub fn set_transform(&mut self, t: Transform) {
        self.transform = Some(t);
    }

    /// Set the pending buffer scale (Q8.8).
    pub fn set_buffer_scale(&mut self, scale: ScaleFactor) {
        self.scale = Some(scale);
    }

    /// Set the pending input region (empty disables input).
    pub fn set_input_region(&mut self, region: Region) {
        self.input_region = Some(region);
    }

    /// Set the pending opaque region (empty = translucent).
    pub fn set_opaque_region(&mut self, region: Region) {
        self.opaque_region = Some(region);
    }

    /// Set the pending color description.
    pub fn set_color(&mut self, color: ColorDescription) {
        self.color = Some(color);
    }

    /// Set/clear the pending HDR metadata.
    pub fn set_hdr_metadata(&mut self, hdr: Option<HdrMetadata>) {
        self.hdr = hdr;
        self.hdr_set = true;
    }

    /// Set the pending presentation mode.
    pub fn set_presentation_mode(&mut self, mode: PresentationMode) {
        self.presentation = Some(mode);
    }

    /// Whether any setter has run (a commit with no pending changes is a
    /// null commit — no damage, no event of record).
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.buffer_set
            || self.transform.is_some()
            || self.scale.is_some()
            || self.input_region.is_some()
            || self.opaque_region.is_some()
            || self.color.is_some()
            || self.hdr_set
            || self.presentation.is_some()
            || !self.damage.is_empty()
    }

    /// Accumulate this pending state into the next one (sync-mode
    /// subsurfaces stash their pending state on commit; later requests
    /// merge on top).
    pub fn merge_into(&mut self, later: PendingState) {
        if later.buffer_set {
            self.buffer = later.buffer;
            self.buffer_set = true;
        }
        if later.transform.is_some() {
            self.transform = later.transform;
        }
        if later.scale.is_some() {
            self.scale = later.scale;
        }
        if later.input_region.is_some() {
            self.input_region = later.input_region;
        }
        if later.opaque_region.is_some() {
            self.opaque_region = later.opaque_region;
        }
        if later.color.is_some() {
            self.color = later.color;
        }
        if later.hdr_set {
            self.hdr = later.hdr;
            self.hdr_set = true;
        }
        if later.presentation.is_some() {
            self.presentation = later.presentation;
        }
        self.damage.add_region(&later.damage);
    }

    /// Apply atomically onto `current`, producing the committed state
    /// and the surface-local effect record for the damage engine.
    #[must_use]
    pub fn commit(&self, current: &SurfaceState) -> (SurfaceState, CommitEffect) {
        let mut next = current.clone();
        if let Some(t) = self.transform {
            next.transform = t;
        }
        if let Some(scale) = self.scale {
            next.scale = scale;
        }
        if let Some(region) = &self.input_region {
            next.input_region = region.clone();
        }
        if let Some(region) = &self.opaque_region {
            next.opaque_region = region.clone();
        }
        if let Some(color) = self.color {
            next.color = color;
        }
        if self.hdr_set {
            next.hdr = self.hdr;
        }
        if let Some(mode) = self.presentation {
            next.presentation = mode;
        }

        let old_bounds = current.bounds();
        let old_opaque = current.opaque_region.clone();
        let buffer_swapped = self.buffer_set && self.buffer != current.buffer;
        if self.buffer_set {
            next.buffer = self.buffer;
        }
        let new_bounds = next.bounds();

        // Content damage: explicit damage, clipped to the *new* bounds;
        // a swapped buffer with no explicit damage damages everything.
        // (A detach contributes NO content damage: the unmap is a
        // coverage change — old bounds through the old occlusion — and
        // the damage engine's coverage rule handles it exactly.)
        let mut content = self.damage.clone().clipped_to(new_bounds);
        if buffer_swapped && self.damage.is_empty() {
            content = Region::from_rect(new_bounds);
        }
        let opaque_changed = next.opaque_region != old_opaque;
        let opaque_new = next.opaque_region.clone();
        let unmapped = current.buffer.is_some() && next.buffer.is_none();
        let first_map = current.buffer.is_none() && next.buffer.is_some();
        (
            next,
            CommitEffect {
                content,
                old_bounds,
                new_bounds,
                opaque_old: old_opaque,
                opaque_new,
                buffer_swapped,
                unmapped,
                first_map,
                geometry_changed: old_bounds != new_bounds,
                opaque_changed,
            },
        )
    }
}

/// What one commit changed, in surface-local terms — the damage
/// engine's per-surface input.
///
/// The boolean fields are independent semantic facts about the commit
/// (each selects a distinct downstream behavior); a packed bitset would
/// obscure them for no memory benefit on this cold path.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug)]
pub struct CommitEffect {
    /// Content-damage region (surface coordinates): cells whose *content
    /// version* changed (explicit damage + full-surface fallbacks).
    pub content: Region,
    /// Bounds before the commit (empty when previously unmapped).
    pub old_bounds: Rect,
    /// Bounds after the commit (empty when now unmapped).
    pub new_bounds: Rect,
    /// Opaque region before the commit.
    pub opaque_old: Region,
    /// Opaque region after the commit.
    pub opaque_new: Region,
    /// Whether a different buffer was attached.
    pub buffer_swapped: bool,
    /// Whether the surface unmapped (detach).
    pub unmapped: bool,
    /// Whether the surface mapped for the first time.
    pub first_map: bool,
    /// Whether the surface-local bounds changed.
    pub geometry_changed: bool,
    /// Whether the opaque region changed (grow or shrink).
    pub opaque_changed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(w: u32, h: u32, gen: u64) -> BufferAttachment {
        BufferAttachment {
            width: w,
            height: h,
            generation: gen,
        }
    }

    #[test]
    fn attach_without_damage_damages_everything() {
        let mut p = PendingState::default();
        p.attach(Some(buffer(10, 20, 1)));
        let (_, e) = p.commit(&SurfaceState::default());
        assert_eq!(e.content.bounds(), Rect::new(0, 0, 10, 20));
        assert!(e.first_map);
    }

    #[test]
    fn same_buffer_reattach_is_null() {
        let current = SurfaceState {
            buffer: Some(buffer(10, 20, 7)),
            ..SurfaceState::default()
        };
        let mut p = PendingState::default();
        p.attach(Some(buffer(10, 20, 7)));
        let (_, e) = p.commit(&current);
        assert!(e.content.is_empty());
        assert!(!e.buffer_swapped);
    }

    #[test]
    fn detach_contributes_no_content_damage() {
        // The unmap is a coverage change; the damage engine's coverage
        // rule repaints the old footprint through the old occlusion.
        let current = SurfaceState {
            buffer: Some(buffer(30, 40, 2)),
            ..SurfaceState::default()
        };
        let mut p = PendingState::default();
        p.attach(None);
        let (_, e) = p.commit(&current);
        assert!(e.content.is_empty());
        assert!(e.unmapped);
        assert_eq!(e.old_bounds, Rect::new(0, 0, 30, 40));
        assert_eq!(e.new_bounds, Rect::EMPTY);
    }

    #[test]
    fn buffer_damage_uses_the_pending_geometry() {
        let current = SurfaceState {
            buffer: Some(buffer(100, 100, 1)),
            ..SurfaceState::default()
        };
        let mut p = PendingState::default();
        // Attach a 200x100 buffer AND rotate: the damage_buffer rect must
        // convert against the pending buffer/transform, not the current.
        p.attach(Some(buffer(200, 100, 2)));
        p.set_transform(Transform::Rot90);
        p.damage_buffer(&[Rect::new(0, 0, 200, 100)], &current);
        let (_, e) = p.commit(&current);
        assert_eq!(e.content.bounds(), Rect::new(0, 0, 100, 200));
    }

    #[test]
    fn buffer_damage_converts_through_transform_and_scale() {
        let current = SurfaceState {
            buffer: Some(buffer(200, 100, 1)), // rot90 → surface 100x200
            transform: Transform::Rot90,
            ..SurfaceState::default()
        };
        let mut p = PendingState::default();
        p.damage_buffer(&[Rect::new(0, 0, 200, 100)], &current);
        let (_, e) = p.commit(&current);
        assert_eq!(e.content.bounds(), Rect::new(0, 0, 100, 200));

        let scaled = SurfaceState {
            buffer: Some(buffer(200, 200, 1)),
            scale: ScaleFactor::from_q8(512).unwrap(), // 2x
            ..SurfaceState::default()
        };
        let mut p2 = PendingState::default();
        p2.damage_buffer(&[Rect::new(100, 100, 50, 50)], &scaled);
        let (_, e2) = p2.commit(&scaled);
        // 100..150 device → 50..75 logical (outward).
        assert_eq!(e2.content.bounds(), Rect::new(50, 50, 25, 25));
    }

    #[test]
    fn explicit_damage_beats_full_fallback() {
        let current = SurfaceState {
            buffer: Some(buffer(100, 100, 1)),
            ..SurfaceState::default()
        };
        let mut p = PendingState::default();
        p.attach(Some(buffer(100, 100, 2)));
        p.damage(&[Rect::new(5, 5, 10, 10)]);
        let (_, e) = p.commit(&current);
        assert_eq!(e.content.len(), 1);
        assert_eq!(e.content.bounds(), Rect::new(5, 5, 10, 10));
    }

    #[test]
    fn damage_clips_to_new_bounds() {
        let current = SurfaceState {
            buffer: Some(buffer(100, 100, 1)),
            ..SurfaceState::default()
        };
        let mut p = PendingState::default();
        p.damage(&[Rect::new(50, 50, 100, 100)]);
        let (_, e) = p.commit(&current);
        // 50..150 clipped to 0..100 → 50..100 (width 50).
        assert_eq!(e.content.bounds(), Rect::new(50, 50, 50, 50));
    }

    #[test]
    fn geometry_change_reports_old_and_new() {
        let current = SurfaceState {
            buffer: Some(buffer(10, 10, 1)),
            ..SurfaceState::default()
        };
        let mut p = PendingState::default();
        p.attach(Some(buffer(20, 30, 2)));
        let (_, e) = p.commit(&current);
        assert!(e.geometry_changed);
        assert_eq!(e.old_bounds, Rect::new(0, 0, 10, 10));
        assert_eq!(e.new_bounds, Rect::new(0, 0, 20, 30));
        // New buffer + no damage ⇒ full new-bounds damage (content),
        // old bounds flow through the coverage rule instead.
        assert_eq!(e.content.bounds(), e.new_bounds);
    }

    #[test]
    fn pending_merges_for_deferred_commits() {
        let mut first = PendingState::default();
        first.damage(&[Rect::new(0, 0, 5, 5)]);
        let mut second = PendingState::default();
        second.damage(&[Rect::new(10, 10, 5, 5)]);
        second.attach(Some(buffer(8, 8, 1)));
        first.merge_into(second);
        let (_, e) = first.commit(&SurfaceState::default());
        // The in-bounds rect survives; the out-of-bounds one clips away.
        assert_eq!(e.content.bounds(), Rect::new(0, 0, 5, 5));
        assert_eq!(e.new_bounds, Rect::new(0, 0, 8, 8));
    }

    #[test]
    fn opaque_change_is_reported() {
        let current = SurfaceState {
            buffer: Some(buffer(10, 10, 1)),
            ..SurfaceState::default()
        };
        let mut p = PendingState::default();
        p.set_opaque_region(Region::from_rect(Rect::new(0, 0, 10, 10)));
        let (_, e) = p.commit(&current);
        assert!(e.opaque_changed);
        assert_eq!(e.opaque_new.bounds(), Rect::new(0, 0, 10, 10));
        assert!(e.opaque_old.is_empty());
    }

    #[test]
    fn null_commit_is_recognized() {
        let mut p = PendingState::default();
        assert!(!p.is_dirty());
        p.damage(&[Rect::new(1, 1, 1, 1)]);
        assert!(p.is_dirty());
    }
}
