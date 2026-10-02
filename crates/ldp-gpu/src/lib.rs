//! LDP GPU backend — the hardware side of the render pipeline.
//!
//! Four concerns, four modules:
//!
//! * [`node`] — DRM device discovery (`drmGetDevices2` behind the
//!   minimal [`drm_sys`] dlopen layer) and the render-node selection
//!   policy: prefer render nodes (no DRM-Master rights needed), fall
//!   back to the primary node only when a device has no render node.
//! * [`dmabuf`] — the DMA-BUF import model: plane descriptors with
//!   kernel-grade validation, and the exact
//!   `EGL_EXT_image_dma_buf_import(_modifiers)` attribute encoding
//!   (round-trip codec included, because the mock context imports by
//!   *decoding* the same attribute list the real one encodes).
//! * [`sync`] — the fence model: sync-file descriptors, syncobj
//!   timeline points, AND-merges (a merged fence is ready when its
//!   *latest* member is ready), and a virtual-clock mock driver that
//!   also accepts `ldp-display`'s mock out-fence tokens — the two
//!   crates' fence provenances meet here.
//! * [`feedback`] — the DMA-BUF allocation advice: the tranche
//!   model that intersects the GL device's importable formats with
//!   the scanout planes' `IN_FORMATS` pairs, ordered so a client's
//!   next allocation is zero-copy end to end by default (the policy
//!   the future dma-buf negotiation surface serves).
//! * [`egl`] — the EGL bootstrap state machine behind the
//!   object-safe [`egl::EglApi`] trait: load, get the display,
//!   initialize, parse the extension string, and gate the import path
//!   on `EGL_EXT_image_dma_buf_import(_modifiers)`. The real backend
//!   is `dlopen("libEGL.so.1")`-backed (typed
//!   [`egl::EglError`]s when the library is absent — the honest
//!   headless outcome); the mock context composites CPU buffers with
//!   the reference blend math so the Phase 9 exit criteria can demand
//!   **byte-equality** against `ldp-renderer`'s software output.
//!
//! Timing doctrine: no clock reads. The mock sync driver owns an
//! injected [`ldp_core::time::Mono`] clock the caller advances, so
//! fence traces are reproducible.
//!
//! Safety doctrine: the crate root does not forbid unsafe (it cannot
//! be lifted per module); every module except the audited `sys`
//! layers (`drm_sys`, `egl::sys`, `gles::{sys, api}`) is
//! `#![forbid(unsafe_code)]`, and each FFI call site carries a
//! `SAFETY` comment — the `ldp-transport`/`ldp-display` precedent.

pub mod classify;
pub mod dmabuf;
pub mod drm_sys;
pub mod egl;
pub mod feedback;
pub mod gles;
pub mod node;
pub mod sync;

pub use classify::{classify, GlIdentity, GpuClass};
pub use dmabuf::{DmaBufDescriptor, DmaBufPlane, EglAttrib};
pub use node::{GpuDevice, GpuNode, NodeCatalog, NodeKind};
pub use sync::{Fence, FenceState, MockSyncDriver, SyncDriver, TimelinePoint};
