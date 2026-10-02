//! The GLES 2.0 rendering backend — the real hardware side of
//! [`ldp_renderer::gles`].
//!
//! * [`sys`] — the audited symbol layer: an EGL context
//!   (RGBA8888 pbuffer-capable config, GLES 2.0 context, made
//!   current on the calling thread) plus the ~40 `gl*` entry points
//!   resolved through `eglGetProcAddress`. Every `unsafe` in this
//!   tree lives there, each call site carrying its `SAFETY` note.
//! * [`api`] — [`RealGles`], the [`ldp_renderer::gles::GlesApi`]
//!   implementation over that context: programs, textures, targets
//!   (FBOs), scissored passes, premultiplied-over draws, readback.
//!
//! The honest-headless doctrine: on a machine without a GL stack the
//! constructor fails *typed* (`GlesError::Unavailable` carrying the
//! EGL layer's reason) — exactly the outcome the renderer-selection
//! policy turns into the software fallback. CI pins that branch; a
//! GPU machine exercises the rest.

pub mod api;
pub mod sys;

pub use api::RealGles;
