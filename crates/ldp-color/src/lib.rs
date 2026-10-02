//! LDP color math — the canonical color pipeline library.
//!
//! Phase 8's renderer hook ([`ldp_renderer::pipeline`]) established the
//! per-pixel contract: range → transfer decode → primaries conversion →
//! reference-white anchor. This crate is the **canonical** home of that
//! math and everything layered on top of it
//! (`docs/architecture.md` §12):
//!
//! * [`matrix`] — row-major 3×3 algebra and the chromaticity-derived
//!   RGB↔XYZ matrices, anchored against the published IEC/ITU values
//!   (sRGB→XYZ, BT.2020→XYZ, DCI-P3→XYZ, BT.709↔BT.2020),
//! * [`transfer`] — the six transfer curves as f32 encode/decode pairs
//!   (bit-identical to the renderer's hook; the cross-consistency suite
//!   pins them together) plus the **f64 PQ reference** used by ramps,
//!   tone mapping, and the KAVT suite,
//! * [`ramp`] — the exact integer PQ ramps (10/12-bit, generic 8..16):
//!   `decode(encode(l))` round-trips through integer codes with **zero**
//!   float rounding drift,
//! * [`gamut`] — luma-preserving desaturation gamut mapping with a hard
//!   clip mode and a soft-knee perceptual mode,
//! * [`tone`] — the BT.2390-structured EETF: PQ-domain normalization,
//!   75% knee, and a monotonicity-guaranteed Hermite/smoothstep rolloff,
//! * [`icc`], [`icc_trc`], [`icc_write`] — ICC v2/v4 matrix-TRC profile
//!   import (parse → model → resolve to a parametric description) and a
//!   deterministic canonical writer used by the round-trip fixtures,
//! * [`pipeline`] — [`ColorSpace`] (from primaries or ICC) and
//!   [`OutputTransform`], the per-output tail
//!   (tone-map scale → gamut map → encode).
//!
//! # Determinism doctrine
//!
//! Same as the renderer's: no clocks, no iteration order dependence, every
//! rounding rule named. Integer paths are bit-stable; float paths use
//! exactly-specified IEEE operations except the transfer curves'
//! `powf`/`exp`/`ln` (tolerance-anchored in the KAVT suite). Table
//! generation (PQ ramps, ICC canonical profiles) runs in f64 and casts
//! once, at the end, so table contents never depend on host float
//! whimsy.
//!
//! # Layering
//!
//! `ldp-color` depends only on `ldp-core`. HDR *policy* (luminance
//! adaptation, InfoFrames, output mode selection) lives one layer up in
//! `ldp-hdr`, which depends on this crate — never the reverse.
//!
//! [`ldp_renderer::pipeline`]: https://docs.rs/ldp-renderer/latest/ldp_renderer/pipeline/

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod gamut;
pub mod icc;
pub mod icc_parse;
pub mod icc_trc;
pub mod icc_write;
pub mod matrix;
pub mod pipeline;
pub mod ramp;
pub mod tone;
pub mod transfer;

pub use gamut::{GamutMapper, GamutMode};
pub use matrix::{luma_coefficients, primaries_matrix, rgb_to_xyz, xyz_to_rgb, Matrix3};
pub use pipeline::{ColorSpace, Curve, OutputTransform};
pub use ramp::PqRamp;
pub use tone::{HdrToHdr, ToneMapper};
pub use transfer::{decode_transfer, encode_transfer};
