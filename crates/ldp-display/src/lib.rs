//! LDP display backend: DRM/KMS atomic modesetting behind an
//! object-safe device trait.
//!
//! Two interchangeable implementations:
//!
//! * [`MockDevice`] — a deterministic, in-memory DRM (connectors with
//!   EDID identity, CRTCs with VRR-capable timelines, planes with
//!   `IN_FORMATS` capability blobs, atomic commits with the kernel's
//!   validation rules, hotplug injection). This is the headless-CI
//!   vehicle: the Phase 9 exit criteria (atomic-commit goldens, VRR
//!   proofs) run against it, byte-reproducible from its injected clock.
//! * `drm::DrmBackend` (feature-gated at runtime by library presence) —
//!   the real thing via `dlopen("libdrm.so.2")`, with every `unsafe`
//!   call audited in `drm::sys`. On machines without DRM nodes it loads
//!   the library, resolves every symbol, and fails cleanly at device
//!   open — also fully exercised in CI.
//!
//! The crate also models the periphery: EDID parsing ([`edid`]), the
//! mode blob codec ([`mode`]), the property system ([`props`]), atomic
//! requests ([`atomic`]), device events ([`events`]), backlight sysfs
//! policy ([`backlight`]), udev hotplug monitoring ([`hotplug`]), and
//! the named-quirk table with operator escapes ([`quirks`]).
//!
//! Timing doctrine: no code in this crate reads a clock. Timestamps are
//! [`ldp_core::time::Mono`] values supplied by the caller or the device
//! (the mock's injected clock; the kernel's flip timestamps), so every
//! recorded event sequence is deterministic.
//!
//! Safety doctrine: every module is `#![forbid(unsafe_code)]` except the
//! audited `sys` layers (`drm::sys`, `udev_sys`), which hold all FFI and
//! carry `SAFETY` comments per call — the `ldp-transport` precedent. The
//! crate root therefore does *not* forbid unsafe (a forbid cannot be
//! lifted per module); instead each module states its own boundary.

pub mod atomic;
pub mod backend;
pub mod backlight;
pub mod commit;
pub mod connector;
pub mod driver;
pub mod drm;
pub mod edid;
pub mod error;
pub mod events;
pub mod fb;
pub mod hotplug;
pub mod ids;
pub mod mock;
pub mod mode;
pub mod plane;
pub mod props;
pub mod quirks;
pub mod serve;
pub mod timing;
pub mod udev_sys;

pub use atomic::{AtomicOp, AtomicRequest};
pub use backend::{Blob, CommitOutcome, CrtcInfo, DeviceVersion, KmsBackend, Topology};
pub use commit::{CommitFlags, DpmsState, DstRect, SrcRect};
pub use connector::{ConnectorInfo, ConnectorStatus, ConnectorType, Subpixel};
pub use driver::{DisplayDriver, DrmDriver};
pub use edid::{audit, DetailedTiming, EdidFinding, EdidIdentity, RangeLimits};
pub use error::{DisplayError, RejectReason, Result};
pub use events::{DeviceEvent, HotplugEvent, OutFence, PageFlipEvent, VblankEvent};
pub use fb::FbSpec;
pub use ids::{AnyId, BlobId, ConnectorId, CrtcId, CrtcMask, EncoderId, FbId, PlaneId, PropId};
pub use mock::MockDevice;
pub use mode::{Mode, ModeFlags, ModeType};
pub use plane::{FormatModifier, InFormats, PlaneInfo, PlaneType};
pub use props::{EnumToken, ObjectProperties, PropType, PropValue, PropertyInfo, PropertyName};
pub use quirks::{Quirk, QuirkClass};
pub use serve::{
    disable, disable_request, enable, enable_request, select_pipeline, select_pipeline_synth,
    select_pipelines, select_pipelines_synth, ModeSize, Pipeline, SelectError, SynthError,
};
pub use timing::{cvt_rb, cvt_rb2, cvt_standard, gtf, pour, SynthRequest, TimingFamily};
