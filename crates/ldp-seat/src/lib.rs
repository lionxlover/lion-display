//! LDP seat layer: seats, device assignment, per-seat focus stacks,
//! grabs, and the routing of normalized input events to protocol
//! events.
//!
//! The layering (architecture §14): `ldp-input` turns devices into
//! normalized events; this crate turns normalized events into the
//! `ldp.input` wire vocabulary:
//!
//! * [`seat`] — [`seat::SeatManager`]: devices assigned
//!   by their udev seat tags, capability masks recomputed on every
//!   change, one seat one focus (structurally: nothing is shared).
//! * [`focus`] — subpixel input-region hit testing, the scene view,
//!   and the pointer/touch focus domains.
//! * [`grab`] — the grab state machine: implicit grabs while buttons
//!   hold, explicit grabs for popups, the capability-gated keyboard
//!   grab.
//! * [`route`] — [`route::Router`], the per-seat state machine
//!   that owns acceleration, gestures, keymap state, and repeat, and
//!   emits [`RoutedEvent`][route::RoutedEvent] batches.
//! * [`event`] — the typed event vocabulary and its encoding against
//!   the compiled protocol schema.
//!
//! Timing doctrine: the router is driven by the timestamps of the
//! frames it is fed; no code in this crate reads a clock. Safety
//! doctrine: every module is `#![forbid(unsafe_code)]` — all FFI
//! lives in `ldp-input`'s audited `sys` layers.

#![forbid(unsafe_code)]

pub mod event;
pub mod focus;
pub mod grab;
pub mod route;
pub mod seat;

pub use event::SeatEvent;
pub use focus::{ClientBinding, FocusRouter, InputRegion, RectF, Scene, SurfaceKey, SurfaceRef};
pub use grab::{DismissReason, GrabKind, GrabModel, PointerGrab};
pub use route::{Router, RouterConfig, FINGER_WHEEL_RADIUS_MM};
pub use seat::{CapsChange, Seat, SeatError, SeatManager};
