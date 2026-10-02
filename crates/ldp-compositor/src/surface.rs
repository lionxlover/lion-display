//! The surface node: identity, role, state slots, position, sync mode,
//! and the frame-to-frame bookkeeping the damage engine consumes
//! (previous mapped bounds, previous occlusion, previous opaque
//! footprint — the "old" side of every exact rule).

use ldp_core::geometry::{Rect, Region};

use crate::pending::{CommitEffect, PendingState};
use crate::state::SurfaceState;

/// Stable identity of one surface in the tree. Raw values are assigned
/// by the embedder (the protocol layer uses its object IDs); zero is
/// reserved as "no surface" so `Option<SurfaceId>` never needs to
/// appear in change records.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct SurfaceId(u64);

impl SurfaceId {
    /// Build from an embedder-assigned raw value (non-zero).
    #[must_use]
    pub const fn from_raw(raw: u64) -> SurfaceId {
        SurfaceId(raw)
    }

    /// The raw value.
    #[must_use]
    pub const fn raw(self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for SurfaceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "surface:{}", self.0)
    }
}

/// How a subsurface's commits synchronize with its parent.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SubsurfaceMode {
    /// Commit-atomic with the parent: the subsurface's commits stash
    /// into a deferred slot that applies when an ancestor applies.
    Sync,
    /// Independent: commits apply immediately.
    Desync,
}

/// The role a surface plays in the tree.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    /// A root surface (toplevel/dialog/popup distinctions arrive with
    /// the shell in Phase 7; the scene graph treats them uniformly).
    Root {
        /// Position in output coordinates.
        x: i32,
        /// Position in output coordinates.
        y: i32,
    },
    /// A subsurface of a parent surface.
    Subsurface {
        /// The parent surface.
        parent: SurfaceId,
    },
}

/// One surface: the authoring state plus the damage engine's
/// frame-to-frame records.
#[derive(Debug)]
pub struct Surface {
    /// Stable identity.
    pub(crate) id: SurfaceId,
    /// Current role.
    pub(crate) role: Role,
    /// Committed state.
    pub(crate) current: SurfaceState,
    /// Accumulated uncommitted requests.
    pub(crate) pending: PendingState,
    /// Deferred state for sync-mode subsurfaces (applied when an
    /// ancestor applies).
    pub(crate) deferred: Option<PendingState>,
    /// Sync mode (subsurfaces only).
    pub(crate) mode: SubsurfaceMode,
    /// Current position: output coordinates for roots, parent-surface
    /// coordinates for subsurfaces.
    pub(crate) position: (i32, i32),
    /// Pending position (applied with the same cascade as state).
    pub(crate) position_pending: Option<(i32, i32)>,
    /// Damage accumulated since the last damage pass (surface-local).
    /// Survives across frames until a repaint consumes it.
    pub(crate) frame_damage: Region,
    // ---- records from the previous damage pass (the "old" side) ----
    /// Mapped bounds in output coordinates at the last pass.
    pub(crate) last_bounds: Rect,
    /// Opaque-above mask (output coordinates) at the last pass.
    pub(crate) last_occlusion: Region,
    /// Opaque footprint (output coordinates) at the last pass.
    pub(crate) last_opaque_mapped: Region,
}

impl Surface {
    /// A fresh unmapped surface at the given role and position.
    pub(crate) fn new(id: SurfaceId, role: Role, position: (i32, i32)) -> Surface {
        Surface {
            id,
            role,
            current: SurfaceState::default(),
            pending: PendingState::default(),
            deferred: None,
            mode: SubsurfaceMode::Sync,
            position,
            position_pending: None,
            frame_damage: Region::new(),
            last_bounds: Rect::EMPTY,
            last_occlusion: Region::new(),
            last_opaque_mapped: Region::new(),
        }
    }

    /// The surface's identity.
    #[must_use]
    pub fn id(&self) -> SurfaceId {
        self.id
    }

    /// The current role.
    #[must_use]
    pub fn role(&self) -> Role {
        self.role
    }

    /// The committed state.
    #[must_use]
    pub fn state(&self) -> &SurfaceState {
        &self.current
    }

    /// The sync mode.
    #[must_use]
    pub fn mode(&self) -> SubsurfaceMode {
        self.mode
    }

    /// The current position (output coords for roots, parent coords for
    /// subsurfaces).
    #[must_use]
    pub fn position(&self) -> (i32, i32) {
        self.position
    }

    /// Damage accumulated since the last damage pass (surface-local).
    #[must_use]
    pub fn frame_damage(&self) -> &Region {
        &self.frame_damage
    }

    /// The mapped bounds at the last damage pass (output coordinates).
    #[must_use]
    pub fn last_bounds(&self) -> Rect {
        self.last_bounds
    }

    /// The occlusion-above mask at the last damage pass (output
    /// coordinates) — the "old" occlusion for coverage rules.
    #[must_use]
    pub fn last_occlusion(&self) -> &Region {
        &self.last_occlusion
    }

    /// Queue a position change (applied by the commit cascade: roots
    /// and desync subsurfaces on their own commit, sync subsurfaces
    /// when an ancestor applies).
    pub(crate) fn set_position(&mut self, x: i32, y: i32) {
        self.position_pending = Some((x, y));
    }

    /// Apply a position change *now* (Phase 28: the shell's
    /// re-layout arm), superseding any queued one. The damage
    /// engine's R2 coverage rule reads the live position at the next
    /// damage pass — `last_bounds` versus the new bounds — so a
    /// shell-moved surface repaints correctly without a commit (the
    /// migration choreography's full-scene re-render is exactly that
    /// pass).
    pub(crate) fn set_position_now(&mut self, x: i32, y: i32) {
        self.position_pending = None;
        self.position = (x, y);
    }

    /// Apply this surface's pending (or deferred) state *now*, folding
    /// the effect into the frame records. Returns the effect; `None`
    /// when there was nothing to apply (null commit).
    pub(crate) fn apply_now(&mut self) -> CommitEffect {
        let pending = match self.deferred.take() {
            Some(mut deferred) => {
                // Newer pending requests merge on top of the deferred
                // state (they are the more recent accumulator).
                deferred.merge_into(std::mem::take(&mut self.pending));
                deferred
            }
            None => std::mem::take(&mut self.pending),
        };
        let (next, effect) = pending.commit(&self.current);
        self.current = next;
        // Position applies with the same atomicity.
        if let Some(p) = self.position_pending.take() {
            self.position = p;
        }
        // Fold the effect into the accumulated frame damage: content
        // damage persists until the next damage pass consumes it.
        self.frame_damage.add_region(&effect.content);
        effect
    }

    /// Apply only the deferred (committed-but-waiting) state plus a
    /// pending position — the ancestor-cascade path. Uncommitted
    /// request state (`pending`) stays put: it belongs to this
    /// surface's own next commit, not to the ancestor's.
    pub(crate) fn apply_deferred(&mut self) {
        if let Some(deferred) = self.deferred.take() {
            let (next, effect) = deferred.commit(&self.current);
            self.current = next;
            self.frame_damage.add_region(&effect.content);
        }
        if let Some(p) = self.position_pending.take() {
            self.position = p;
        }
    }

    /// A sync-mode subsurface commit: stash the pending state (merged
    /// over any earlier stash) for the ancestor cascade.
    pub(crate) fn stash_deferred(&mut self) {
        let pending = std::mem::take(&mut self.pending);
        match &mut self.deferred {
            Some(deferred) => deferred.merge_into(pending),
            None => self.deferred = Some(pending),
        }
    }

    /// Whether a commit on this surface applies immediately (roots and
    /// desync subsurfaces) or stashes (sync subsurfaces).
    pub(crate) fn commits_apply_immediately(&self) -> bool {
        !matches!(
            (self.role, self.mode),
            (Role::Subsurface { .. }, SubsurfaceMode::Sync)
        )
    }

    /// Record the damage pass's outputs for the next frame's "old"
    /// side, and consume the accumulated frame damage.
    pub(crate) fn record_pass(&mut self, bounds: Rect, occlusion: Region, opaque_mapped: Region) {
        self.last_bounds = bounds;
        self.last_occlusion = occlusion;
        self.last_opaque_mapped = opaque_mapped;
        self.frame_damage.clear();
    }

    /// Whether this surface carries unapplied deferred state (sync-mode
    /// stash waiting for an ancestor commit).
    pub(crate) fn has_deferred(&self) -> bool {
        self.deferred.is_some() || self.position_pending.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_and_display() {
        let id = SurfaceId::from_raw(42);
        assert_eq!(id.raw(), 42);
        assert_eq!(id.to_string(), "surface:42");
        assert_ne!(id, SurfaceId::from_raw(43));
    }

    #[test]
    fn sync_subsurface_stashes_then_applies() {
        let mut s = Surface::new(
            SurfaceId::from_raw(2),
            Role::Subsurface {
                parent: SurfaceId::from_raw(1),
            },
            (0, 0),
        );
        s.pending.attach(Some(crate::state::BufferAttachment {
            width: 8,
            height: 8,
            generation: 1,
        }));
        s.pending.damage(&[Rect::new(0, 0, 4, 4)]);
        assert!(!s.commits_apply_immediately());
        s.stash_deferred();
        assert!(s.has_deferred());
        let effect = s.apply_now();
        assert_eq!(effect.content.bounds(), Rect::new(0, 0, 4, 4));
        assert!(!s.has_deferred());
        // Frame damage accumulated.
        assert_eq!(s.frame_damage().bounds(), Rect::new(0, 0, 4, 4));
    }

    #[test]
    fn position_applies_with_state() {
        let mut s = Surface::new(SurfaceId::from_raw(1), Role::Root { x: 0, y: 0 }, (0, 0));
        s.set_position(10, 20);
        assert_eq!(s.position(), (0, 0));
        s.apply_now();
        assert_eq!(s.position(), (10, 20));
    }

    #[test]
    fn pass_records_clear_damage() {
        let mut s = Surface::new(SurfaceId::from_raw(1), Role::Root { x: 0, y: 0 }, (0, 0));
        s.pending.attach(Some(crate::state::BufferAttachment {
            width: 16,
            height: 16,
            generation: 1,
        }));
        s.pending.damage(&[Rect::new(0, 0, 8, 8)]);
        s.apply_now();
        assert!(!s.frame_damage().is_empty());
        s.record_pass(Rect::new(0, 0, 8, 8), Region::new(), Region::new());
        assert!(s.frame_damage().is_empty());
        assert_eq!(s.last_bounds(), Rect::new(0, 0, 8, 8));
        assert!(s.last_occlusion().is_empty());
    }

    #[test]
    fn desync_subsurface_applies_immediately() {
        let mut s = Surface::new(
            SurfaceId::from_raw(3),
            Role::Subsurface {
                parent: SurfaceId::from_raw(1),
            },
            (0, 0),
        );
        s.mode = SubsurfaceMode::Desync;
        assert!(s.commits_apply_immediately());
        s.pending.attach(Some(crate::state::BufferAttachment {
            width: 8,
            height: 8,
            generation: 1,
        }));
        s.pending.damage(&[Rect::new(1, 1, 2, 2)]);
        let e = s.apply_now();
        assert_eq!(e.content.bounds(), Rect::new(1, 1, 2, 2));
    }
}
