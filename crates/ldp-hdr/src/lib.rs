//! LDP HDR policy — the layer above `ldp-color`'s math.
//!
//! Phase 14's second crate (architecture §12): everything that is
//! *policy* rather than arithmetic —
//!
//! * [`metadata`]: HDR10 static-metadata validation and normalization
//!   (the CTA "0 = unknown" sentinels, FALL ≤ CLL, the PQ ceiling),
//!   producing the InfoFrame-ready coding,
//! * [`infoframe`]: the CTA-861.3 HDMI HDR Static Metadata InfoFrame —
//!   a deterministic 30-byte codec (3-byte header, 26-byte payload,
//!   checksum) with parse-back round-trips,
//! * [`adaptation`]: luminance adaptation — the SDR-in-HDR canvas
//!   level (where SDR reference white lands on an HDR output) and the
//!   HLG OOTF system gamma (the BT.2100 peak-dependent formula),
//! * [`policy`]: the output-mode decision (SDR / PQ / HLG) from
//!   output capabilities and the surface stack, with dwell-based
//!   hysteresis so the mode never flaps frame to frame,
//! * [`negotiation`] (Phase 38): the panel negotiation — the
//!   effective peak (`--hdr-peak` clamps the advertisement), the
//!   negotiated canvas ceiling (content clamped to the panel), and
//!   the per-surface mastering refinement (the static metadata that
//!   reaches the ink).
//!
//! ST 2094-40 dynamic metadata is a capability-gated extension path
//! (per architecture §12): the capability bit exists in the spec's
//! `color_caps`; parsing lands with the dynamic-HDR phase, documented
//! in the roadmap — not stubbed here.
//!
//! Determinism doctrine as everywhere: no clocks, no randomness, every
//! rounding rule named. Pure policy over `ldp-core` types and
//! `ldp-color` math; zero external dependencies.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod adaptation;
pub mod infoframe;
pub mod metadata;
pub mod negotiation;
pub mod policy;

pub use adaptation::{hlg_system_gamma, LuminanceAdapter};
pub use infoframe::{Eotf, HdrInfoFrame};
pub use metadata::{HdrStaticMetadata, MetadataError};
pub use negotiation::{layer_mastering, negotiated_ceiling, NegotiationError, PanelPeak};
pub use policy::{ModeController, OutputCaps, OutputMode, StackSummary};
