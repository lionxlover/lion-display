//! Convenience re-exports.
//!
//! `use ldp_core::prelude::*;` brings in the vocabulary types used by
//! every LDP crate. Star imports are otherwise discouraged in this
//! codebase (see CONTRIBUTING.md) — the prelude exists because these
//! names are the protocol's shared nouns.

pub use crate::bitset::Bitset128;
pub use crate::buffer::{BufferGeometry, FourCC, Modifier, PlaneLayout};
pub use crate::caps::{SandboxFlavor, Scope, ScopeSet};
pub use crate::color::{
    ColorDescription, ColorRange, HdrMetadata, Luminance, Primaries, TransferFunction,
};
pub use crate::error::{ErrorCode, LdpError, LimitKind, Result};
pub use crate::geometry::{Point, PointF, Rect, Region, Size, Transform};
pub use crate::ids::{ClientId, Generation, ObjectId, SlotId};
pub use crate::limits::Limits;
pub use crate::scale::ScaleFactor;
pub use crate::time::{
    FrameDeadline, Mono, PresentationFlags, PresentationMode, PresentationTiming, RefreshInterval,
    VblankPredictor,
};
pub use crate::token::AccessToken;
pub use crate::version::{Version, VersionRange};
pub use crate::wire::{ArgType, Primitive, Value};
