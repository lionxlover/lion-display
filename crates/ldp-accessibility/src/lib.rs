//! LDP accessibility — settings broadcast, the screen-reader event bus,
//! magnifier control.
//!
//! This crate is the a11y layer of `docs/architecture.md` §17:
//!
//! * [`settings`]: the feature-toggle snapshot model and its broadcast
//!   (every bound client restyles from the same bytes).
//! * [`bus`]: the provider/subscriber event bus — ordered delivery
//!   with push-time focus attribution, `a11y_control`-gated
//!   subscriptions, bounded per-provider queues (the Phase 16 event
//!   ordering exit criterion).
//! * [`magnifier`]: lens state and geometry — follow modes, Q8 scale
//!   with the 0-keeps sentinel, lens/content clamping.
//!
//! Keyboard access *processing* (sticky/slow/bounce/mouse keys) is
//! ldp-input's; the settings model here carries only the enabled state
//! the input layer reads. The magnifier *rendering* is the
//! compositor's output pass; this crate owns the control state and
//! geometry math.
//!
//! Purity doctrine: no clock, no devices, no I/O — pure state machines
//! over ldp-core types. `#![forbid(unsafe_code)]` crate-wide.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod bus;
pub mod magnifier;
pub mod settings;

pub use bus::{
    A11yBus, AnnouncePriority, Delivery, FocusInfo, Provider, ProviderEvent, SubscribeError,
};
pub use magnifier::{Magnifier, MagnifierError, MagnifierFollow, TrackSource};
pub use settings::{A11yFeature, A11ySettings, SettingsBus, SettingsError};
