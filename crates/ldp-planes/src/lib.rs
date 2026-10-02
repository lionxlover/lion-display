//! The plane engine: hardware plane assignment for a composited stack.
//!
//! Phase 33 — the GPU-compositing field's missing organ. Everything
//! before this crate answered one narrow question ("could this single
//! fullscreen layer ride a primary plane?" — the renderer's
//! `scanout_candidate` pre-filter). The giants' compositors answer a much richer one: *which subset of the
//! layer stack can the display hardware blend on its own, so the GPU
//! never touches those pixels at all?* Windows calls the shape MPO
//! (multi-plane overlay); macOS's WindowServer composes the same
//! doctrine over its CALayer render server. This crate is LDP's
//! answer, as a pure decision engine:
//!
//! * [`PlaneInventory`] — the per-CRTC plane capability snapshot
//!   (kinds, `IN_FORMATS` (format, modifier) pairs, zpos slots),
//!   collected from any [`ldp_display::backend::KmsBackend`].
//! * [`LayerFacts`] — what the compositor knows about one layer of
//!   the stack: placement, buffer geometry, color, opacity, opaque
//!   coverage, and — the zero-copy truth — whether the buffer has a
//!   registered kernel framebuffer behind it.
//! * [`Assigner::solve`] — the doctrine below, producing a
//!   [`ScanoutPlan`].
//! * [`Demotion`] — the honest ledger: every layer that composites
//!   carries the *named reason* it did not ride a plane (the
//!   project's no-silent-degrade rule, applied to hardware offload).
//!
//! # The split-point doctrine
//!
//! KMS stacks planes by zpos above a CRTC; the compositor's rendered
//! canvas lives on the **primary** plane. A layer that composites
//! must blend *into something below it*, and that something is the
//! canvas on the primary plane — so a composited layer forces every
//! layer **under** it to composite too (there is no GL surface
//! sandwiched between hardware planes). Layers **above** the
//! composited group are free: they ride overlay planes whose zpos
//! sits above the primary. This yields the classic underlay shape:
//!
//! ```text
//!  zpos 2: [ overlay plane 52 ]   ← top client layer (1:1, fb-backed)
//!  zpos 1: [ overlay plane 51 ]   ← dock (plain tier, fb-backed)
//!  zpos 0: [ primary plane 50 ]   ← canvas: composited bottom group
//!                                    (or, at zero-composite, the
//!                                    fullscreen client buffer itself)
//! ```
//!
//! `solve` therefore finds the **split point** `k`: layers `[0..k]`
//! composite into the canvas, layers `[k..]` ride planes in stack
//! order. `k == 0` with an opaque, output-covering bottom layer is
//! the **zero-composite frame** — the renderer emits no pass at all,
//! the atomic commit carries every plane, and the client's own buffer
//! scans out untouched. That frame is the GPU-compositing ceiling's
//! proof: zero GPU passes, zero uploads, zero copies.
//!
//! # Eligibility (the honest v1 contract)
//!
//! A layer may ride an overlay plane when it has a kernel FB
//! (GEM-backed buffer — the DMA-BUF import walk's product), is placed
//! 1:1 (planes do not scale in v1; a scaled layer composites), is
//! untransformed (rotation caps are a named future line), blends in
//! the output's own color description (no per-plane color management
//! in v1), sits fully inside the output, carries surface opacity 1.0
//! (there is no standard plane-alpha property; per-pixel alpha in an
//! ARGB-family buffer is fine — the hardware blends it), and its
//! (format, modifier) is in the plane's `IN_FORMATS`. The first plane
//! assignment additionally requires the layer to cover the output
//! opaquely — unless the canvas composites below it, in which case
//! the canvas is the coverage. Cursor planes are reserved for the
//! pointer sprite and never assigned. Overlay planes below the
//! primary's zpos are ignored (below-canvas stacking is a named
//! future line).
//!
//! # Determinism
//!
//! `solve` is a pure function of `(facts, inventory, output)` — no
//! clocks, no device state, no allocation-order dependence. Plane
//! preference is `(zpos, id)`-ascending; the search is a depth-first
//! walk with backtracking over per-layer candidate sets, so a
//! capability-starved middle layer cannot strand an upper one (the
//! greedy failure mode the backtracking exists to fix). Identical
//! inputs always produce the identical plan — the CI oracle property
//! the rest of the project's renderers already uphold.

#![forbid(unsafe_code)]

mod decision;
mod facts;
mod inventory;
mod plan;
mod solver;

pub use decision::{CompositionPath, DecisionRecord};
pub use facts::{LayerFacts, OutputFacts};
pub use inventory::{PlaneCaps, PlaneInventory};
pub use plan::{Demotion, PlaneAssignment, PlaneRole, ScanoutPlan};
pub use solver::Assigner;
