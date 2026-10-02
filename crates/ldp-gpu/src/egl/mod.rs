//! The EGL module.
//!
//! * [`sys`] — the audited `dlopen("libEGL.so.1")` layer: symbols,
//!   constants, safe wrappers. Every `unsafe` in this tree lives
//!   there.
//! * [`api`] — the object-safe [`EglApi`] contract, the bootstrap
//!   state machine's typed failures, and the mock context whose
//!   import path decodes the real attribute encoding and whose draw
//!   path uses the reference blend math.

pub mod api;
pub mod sys;

pub use api::{
    Capabilities, EglApi, EglVersion, ImageHandle, MockEgl, RealEgl, EXT_DMABUF_IMPORT,
    EXT_DMABUF_MODIFIERS, EXT_NATIVE_FENCE,
};
pub use sys::{EglError, LibEgl};
