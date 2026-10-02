//! LDP session — the D-Bus minimal client, the logind session
//! machine, VT switching, lock-screen orchestration, and inhibitor
//! cookies.
//!
//! This crate is the session layer of `docs/architecture.md` §18:
//!
//! * [`dbus`]: the D-Bus wire codec (marshal/parse, alignment-exact,
//!   bounds-checked) — implemented from the specification like the
//!   evdev codec, with no external binding.
//! * [`conn`]: the connection state machine over the
//!   [`DbusTransport`] seam — serial allocation, reply matching, the
//!   EXTERNAL auth preamble.
//! * [`logind`]: `TakeControl`/`TakeDevice`, the `PauseDevice`/
//!   `ResumeDevice`/`Lock`/`Unlock`/`PrepareForSleep` signals as
//!   typed events.
//! * [`vt`]: the VT-switch choreography (release master, ack pause,
//!   reacquire device) as an event→action machine.
//! * [`lock`]: lock-screen orchestration with the focus gate — in
//!   `Locked`, only lock surfaces are interactive.
//! * [`inhibit`]: the cookie registry behind `ldp.session.inhibit`.
//!
//! Purity: no sockets, no unsafe, no clock — the transport is a trait,
//! every timestamp is injected. `#![forbid(unsafe_code)]` crate-wide;
//! the only dependency is ldp-core.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod conn;
pub mod dbus;
pub mod inhibit;
pub mod lock;
pub mod logind;
pub mod vt;

pub use conn::{auth_external_bytes, DbusConnection, DbusEvent, DbusTransport, OutgoingCall};
pub use dbus::{parse_message, DbusError, DbusMessage, DbusValue, MessageType};
pub use inhibit::{InhibitBits, InhibitRegistry};
pub use lock::{LockAction, LockScreen, LockState};
pub use logind::{LogindSession, PauseReason, SessionEvent};
pub use vt::{VtAction, VtState, VtSwitcher};
