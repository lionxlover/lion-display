//! The mock KMS device module.
//!
//! * `device` — the device, its state, hotplug injection, and the
//!   injected-clock advance entry points.
//! * `preset` — the reference laptop-dual topology builder.
//! * `catalog` — the global property registry and per-object catalogs.
//! * `queries` — the [`crate::backend::KmsBackend`] implementation.
//! * `timeline` — the deterministic scanout clock (nominal grid, VRR
//!   windows, fenced flips).
//! * `validate` — the atomic commit rules and application.

mod catalog;
mod device;
mod preset;
mod queries;
mod timeline;
mod validate;

pub use device::{MockDevice, PlaneState};
