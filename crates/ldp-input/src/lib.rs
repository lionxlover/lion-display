//! LDP input backend: raw evdev devices to normalized input events.
//!
//! This crate owns everything between the kernel's `/dev/input/event*`
//! byte streams and the event vocabulary the seat layer routes to
//! clients:
//!
//! * [`codes`] — the evdev ABI constants (event types, key/button/rel/
//!   abs/LED codes) this crate understands, each pinned to its numeric
//!   value with the kernel header it comes from.
//! * [`evdev`] — the 24-byte `input_event` wire codec (LP64 layout) and
//!   the `SYN_REPORT` framing that turns event runs into
//!   [`evdev::DeviceFrame`]s.
//! * [`device`] — the device model: capability bits, ABS axis ranges
//!   ([`device::AbsInfo`]), class classification, and LED state.
//! * [`mt`] — the multitouch slot machine (kernel protocol B):
//!   per-slot state, tracking identities, drop resynchronization.
//! * [`normalizer`] — axis-value normalization (device units to 0..1 /
//!   radians / logical pixels) with fuzz dead-zones, producing the
//!   typed [`normalizer::InputEvent`] vocabulary.
//! * [`accel`] — the pointer acceleration filter: a two-tier smooth
//!   curve, latency-first (one event of history, no smoothing lag).
//! * [`gesture`] — touchpad gesture state machines (swipe / pinch /
//!   hold) plus two-finger scroll emulation.
//! * [`repeat`] — the server-side key repeat schedule model.
//! * [`xkb`] — keymap compilation and modifier state through
//!   `dlopen("libxkbcommon.so.0")`, with the v1 keymap serialization
//!   clients receive verbatim on a read-only descriptor.
//! * [`backend`] — `/dev/input` enumeration and the ioctl probe; on
//!   headless machines both report typed, honest results.
//!
//! Timing doctrine: no code in this crate reads a clock. Device events
//! carry their own timestamps (converted to [`ldp_core::time::Mono`]),
//! and every state machine here is driven by the timestamps of the
//! events fed to it, which is what makes the golden-trace corpus
//! byte-reproducible.
//!
//! Safety doctrine: every module is `#![forbid(unsafe_code)]` except the
//! audited `sys` layers (`xkb::sys` holds the libxkbcommon FFI, and the
//! probe ioctls in `backend` route through the same treatment), which
//! carry `SAFETY` comments per call — the `ldp-display` precedent.

pub mod accel;
pub mod backend;
pub mod codes;
pub mod device;
pub mod evdev;
pub mod gesture;
pub mod mt;
pub mod normalizer;
pub mod repeat;
pub mod xkb;
