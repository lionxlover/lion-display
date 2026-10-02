//! # ldp-wayland-bridge — the Wayland compatibility proxy for LDP
//!
//! A pure-Rust Wayland *compositor subset* (core + `wl_shm` + seats +
//! `xdg-shell`) that maps foreign windows onto LDP surfaces with
//! implicit-to-explicit sync translation. The crate is protocol
//! machinery only: sockets, the client pool fds and the keymap fd are
//! seams the process binary supplies; the crate reads no clock and
//! holds no `unsafe`.
//!
//! Module map:
//!
//! * [`wire`] — the object-id/opcode header, argument framing, and
//!   the checked decoder;
//! * [`protocol`] — the pinned interface/message tables (the subset's
//!   single dialect, byte-pinned by the golden tests);
//! * [`state`] — surface double-buffering, the xdg role machines,
//!   positioner vocabulary, buffers;
//! * [`dispatch`] — the per-client connection state machine: request
//!   routing, registry replay, serials, input/shell injection seams;
//! * [`driver`] — the LDP side: bridge-scope token check, the
//!   foreign-surface export through the shm read seam, configure
//!   routing, close, and the implicit-to-explicit sync statement.

#![forbid(unsafe_code)]

pub mod dispatch;
pub mod driver;
pub mod protocol;
pub mod state;
pub mod wire;
