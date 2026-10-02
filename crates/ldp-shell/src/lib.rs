//! LDP shell layer: window roles, the configure/ack two-phase commit,
//! server-side decoration geometry, spaces, stacking and focus policy.
//!
//! The layering (architecture §8): this crate implements the
//! `ldp.shell` module as *pure window-management policy*:
//!
//! * [`ssd`] — server-side decoration metrics in logical units and
//!   their per-output scaling (the mixed-DPI contract: insets are
//!   ceil-scaled so physical chrome never under-covers the logical
//!   reservation), plus the client-side decoration metrics the shell
//!   still applies to CSD windows so they land on the system grid.
//! * [`serial`] — the per-object configure serial clock: monotonic,
//!   wrapping, and collision-free against the one live serial.
//! * [`toplevel`] — [`toplevel::Toplevel`]: the state-flag machine and
//!   the two-phase commit (server proposes `configure` with a serial;
//!   the client `ack_configure`s; the next commit realizes it).
//! * [`popup`] — [`popup::Popup`]: anchor/gravity placement and the
//!   slide/flip/resize constraint solver, reposition, dismissal, and
//!   the explicit-grab bookkeeping.
//! * [`dialog`] — [`dialog::Dialog`]: transient windows attached to a
//!   toplevel, modality, and server-side centering.
//! * [`spaces`] — [`spaces::Spaces`]: macOS-style spaces; assignment,
//!   stickiness, per-seat active space, and visibility queries.
//! * [`stack`] — [`stack::WindowStack`] and the focus policy with
//!   modal gating and activation attribution (the *cause* of every
//!   focus change is recorded for a11y and audit).
//! * [`event`] — the typed shell-event vocabulary and its encoding
//!   against the compiled protocol schema.
//!
//! Timing doctrine: no code in this crate reads a clock; state changes
//! are driven by the requests fed to them. Safety doctrine: every
//! module is `#![forbid(unsafe_code)]` — the shell has no FFI surface
//! at all.
//!
//! Integration doctrine: the shell addresses windows by the opaque
//! [`WindowKey`]; the server binary maps keys to compositor scene
//! nodes. The shell decides *policy* (order, focus, proposals); the
//! compositor executes *scene order* — the two never share types.

#![forbid(unsafe_code)]

pub mod dialog;
pub mod event;
pub mod layout;
pub mod popup;
pub mod serial;
pub mod spaces;
pub mod ssd;
pub mod stack;
pub mod toplevel;

pub use dialog::{Dialog, DialogModality, DialogState};
pub use layout::{
    clamp_into, dock_rect, place, usable_area, DeviceClass, DockConfig, Edge, Layout,
    PlacementPolicy, CASCADE_STEP,
};
pub use popup::{Anchor, Gravity, Placement, Popup, PopupConstraints, PopupError, PopupGeometry};
pub use serial::SerialClock;
pub use spaces::{Spaces, SpacesError};
pub use ssd::{DecorationMode, Insets, SsdMetrics};
pub use stack::{Activation, ActivationCause, WindowStack};
pub use toplevel::{ResizeEdge, Toplevel, ToplevelError, ToplevelStates};

/// Opaque window identity for shell policy (the integrator maps these
/// to compositor scene keys; nothing in this crate dereferences them).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct WindowKey(u64);

impl WindowKey {
    /// Construct from an integrator-chosen value.
    #[must_use]
    pub const fn new(id: u64) -> WindowKey {
        WindowKey(id)
    }

    /// The raw key.
    #[must_use]
    pub const fn as_u64(self) -> u64 {
        self.0
    }
}
