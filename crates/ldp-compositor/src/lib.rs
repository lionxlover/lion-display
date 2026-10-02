//! # ldp-compositor — scene graph, damage, and the frame scheduler
//!
//! The compositor core (`docs/architecture.md` §7 and §10, roadmap
//! Phases 6–7): the surface tree with subsurface roles, atomic state
//! commits with sync/desync semantics, damage propagation with
//! occlusion subtraction built on the `ldp-core` region algebra,
//! stacking and focus data structures, immutable per-frame snapshots —
//! and, on top of that scene graph, the **deadline frame scheduler**
//! with its vblank PLL, presentation feedback, event coalescing, and
//! determinism replay harness.
//!
//! * [`tree::SurfaceTree`] — the mutable authoring structure: create and
//!   destroy (sub)surfaces, set positions, restack, and run the atomic
//!   [`tree::SurfaceTree::commit`] cascade,
//! * [`damage::DamageEngine`] — computes a [`damage::FrameDamage`] from
//!   the tree: **repaint** damage (what the renderer must redraw),
//!   **presentation** damage per surface (the visible change clients are
//!   told about), and **scanout** damage (overlay/direct-scanout
//!   candidates), all in output coordinates and clipped to output bounds,
//! * [`occlusion`] — the front-to-back occlusion walk: which parts of
//!   each surface are actually visible, given the opaque regions of the
//!   surfaces stacked above it,
//! * [`stacking`] / [`focus`] — per-parent stacking order with restack
//!   bookkeeping, and the MRU/keyboard focus stack,
//! * [`snapshot::FrameSnapshot`] — the immutable per-frame capture the
//!   render path consumes; structurally shared, stable under later
//!   mutation of the tree,
//! * [`scheduler::FrameScheduler`] — the per-output deadline scheduler
//!   (§10.3): frame registrations and deadline contracts, escalation,
//!   presentation feedback events, all as a pure function of the
//!   timestamped input stream,
//! * [`coalesce::CoalescingQueue`] — the §10.4 class-aware emission
//!   path (input never dropped, presentation/configuration coalesce to
//!   latest, position state replaces the still-pending sample in its
//!   slot — the discrete barrier seals it, data never dropped),
//! * [`replay`] — the recording + replay harness proving the
//!   scheduler's decisions are reproducible ([`replay_codec`] owns the
//!   byte format; [`sched_types`] the shared vocabulary),
//!
//! # The damage model (normative)
//!
//! Damage is defined per output pixel by a *fold* model that mirrors how
//! blending composites a stack of surfaces bottom-to-top:
//!
//! * every covering (translucent) surface contributes to a pixel's value,
//! * an **opaque** surface (per its `set_opaque_region` guarantee) wipes
//!   everything below it,
//! * a pixel must be repainted iff its fold value changed.
//!
//! From that model the engine derives four exact rule families (see
//! `damage.rs` for the derivations): content changes visible through the
//! current occlusion, coverage changes (old and new bounds through their
//! respective occlusions), opaque-region flips, and restack pairs. The
//! randomized corpus test (`tests/corpus.rs`) proves equality of the
//! region-algebra engine against an independent per-pixel reference
//! implementation of the fold model.
//!
//! # Coordinates
//!
//! *Surface* coordinates are logical units (buffer pixels divided by the
//! buffer scale); *buffer* coordinates are device pixels before the
//! buffer-to-surface [`Transform`](ldp_core::geometry::Transform).
//! Subsurface positions are expressed in the parent's surface
//! coordinates and accumulate down the chain. All damage returned by the
//! engine is in **output** coordinates. Subsurfaces are not clipped to
//! their parents (v1; clips arrive with the shell in Phase 12), but
//! everything is clipped to the output bounds.
//!
//! # Threading
//!
//! The tree is authored from one thread; the render path takes a
//! [`snapshot::FrameSnapshot`] (owned, structurally shared) at frame
//! boundaries and never touches the tree. Damage computation is a pure
//! function of the tree plus the recorded frame changes. The scheduler
//! is single-threaded per output by construction: it is a state machine
//! over timestamped inputs, so the embedder's I/O thread feeds it and
//! the render thread consumes its events.
//!
//! # Safety and dependencies
//!
//! `#![forbid(unsafe_code)]`. The only dependency is `ldp-core` — this
//! crate contains no I/O, no wire code, and no system interaction; it is
//! the same library whether driven by the protocol dispatcher, by the
//! display backend, or by tests.

#![forbid(unsafe_code)]

pub mod coalesce;
pub mod damage;
pub mod focus;
pub mod occlusion;
pub mod pending;
pub mod predictor;
pub mod replay;
pub mod replay_codec;
pub mod sched_types;
pub mod scheduler;
pub mod semantics;
pub mod snapshot;
pub mod spring;
pub mod stacking;
pub mod state;
pub mod surface;
pub mod transitions;
pub mod tree;

pub use coalesce::{CoalesceKey, Coalescible, CoalescingQueue, EventClass, QueueFull};
pub use damage::{DamageEngine, FrameDamage};
pub use focus::FocusStack;
pub use pending::{CommitEffect, PendingState};
pub use predictor::{FlipObservation, FrameClock};
pub use replay::{Recording, ReplayError, SchedInput};
pub use replay_codec::fnv1a;
pub use scheduler::{ConfigError, FrameScheduler, SchedEvent, SchedulerConfig};
pub use semantics::{CaptureRule, SceneProfile, SecurityClass, SemanticRole, Semantics};
pub use snapshot::{FrameSnapshot, SnapshotNode};
pub use spring::Spring;
pub use stacking::StackingList;
pub use state::{BufferAttachment, SurfaceState};
pub use surface::{Surface, SurfaceId};
pub use transitions::{Transition, TransitionCatalog, TransitionKind, TransitionSpec};
pub use tree::{FrameChanges, RemovalRecord, SurfaceTree, TreeError, MAX_SUBSURFACE_DEPTH};

#[cfg(test)]
mod tests {
    #[test]
    fn crate_surface_is_reachable() {
        // Compile-level smoke: the prelude names resolve.
        let _ = crate::SurfaceId::from_raw(1);
        let _ = crate::SurfaceState::default();
        assert!(!crate::SurfaceState::default().is_mapped());
    }
}
