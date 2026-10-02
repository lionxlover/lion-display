//! The KMS property model.
//!
//! Atomic modesetting is "set property values on objects". Every property
//! has a [`PropType`] that dictates which values are legal: ranges bound
//! a `u64` (or `i64` when signed), enums name their tokens, blobs carry
//! opaque kernel-managed payloads, object props point at another KMS
//! object of a fixed kind, bitmasks combine named bits, and booleans are
//! enums in kernel clothing with two implicit tokens.
//!
//! Requests address properties *by name* (`"ACTIVE"`, `"SRC_X"`) because
//! ids are driver-specific and unstable across hotplug re-probes; the
//! backend resolves names through its per-object catalogs. The
//! [`PropertyName`] newtype keeps request construction typo-safe against
//! the well-known registry in the [`prop`] sibling module.

#![forbid(unsafe_code)]

use crate::error::{DisplayError, RejectReason, Result};
use crate::ids::{ObjectType, PropId};

/// A property name as addressed by atomic requests.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct PropertyName(String);

impl PropertyName {
    /// Wrap a kernel property name (`"VRR_ENABLED"`).
    #[must_use]
    pub fn new(name: &str) -> Self {
        Self(name.to_owned())
    }

    /// The name string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl core::fmt::Display for PropertyName {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One named enum/bitmask token.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct EnumToken {
    /// The token's wire value.
    pub value: u64,
    /// Its name (`"On"`, `"Full"`).
    pub name: String,
}

/// The value shape a property accepts.
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum PropType {
    /// Inclusive `[min, max]` on `u64`.
    Range {
        /// Inclusive lower bound.
        min: u64,
        /// Inclusive upper bound.
        max: u64,
    },
    /// Inclusive `[min, max]` on `i64` (wire still `u64`).
    SignedRange {
        /// Inclusive lower bound (sign-extended on the wire).
        min: i64,
        /// Inclusive upper bound.
        max: i64,
    },
    /// One of the named tokens.
    Enum {
        /// The legal (value, name) pairs.
        tokens: Vec<EnumToken>,
    },
    /// A kernel blob object.
    Blob,
    /// An id of the given object kind.
    Object {
        /// Which object namespace the value addresses.
        kind: ObjectType,
    },
    /// OR of named bits.
    Bitmask {
        /// The legal bits with their names.
        tokens: Vec<EnumToken>,
    },
    /// The kernel's BOOLEAN encoding: enum {0,1}; helper type.
    Boolean,
}

impl PropType {
    /// The kernel flag bits (`DRM_MODE_PROP_*`).
    #[must_use]
    pub fn kernel_flags(&self) -> u32 {
        match self {
            Self::Range { .. } => 1 << 1,
            Self::SignedRange { .. } => 1 << 1 | 1 << 7,
            Self::Enum { .. } | Self::Boolean => 1 << 3,
            Self::Blob => 1 << 4,
            Self::Object { .. } => 1 << 6,
            Self::Bitmask { .. } => 1 << 5,
        }
    }

    /// Whether writes are allowed at all.
    #[must_use]
    pub const fn is_mutable(&self) -> bool {
        !matches!(self, Self::Blob) // blob ids are written; payloads are immutable
    }
}

/// One property definition (the `drmModePropertyRes` mirror).
#[derive(Clone, Debug)]
pub struct PropertyInfo {
    /// Property object id.
    pub id: PropId,
    /// Name.
    pub name: PropertyName,
    /// Value shape.
    pub kind: PropType,
    /// Immutable properties reject writes (`DRM_MODE_PROP_IMMUTABLE`).
    pub immutable: bool,
}

/// A value bound for one object in an atomic request.
///
/// Construction validates the value against the property's type
/// client-side (cheap pre-flight); the backend re-validates — the client
/// check is a fast reject, not a guarantee.
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum PropValue {
    /// Boolean properties.
    Bool(bool),
    /// Unsigned (range, enum, bitmask, blob-id, object-id).
    U64(u64),
    /// Signed ranges (`IN_FENCE_FD`, `OUT_FENCE_PTR`, `CRTC_X/Y`).
    I64(i64),
    /// A blob to create inside the commit; the backend mints the blob and
    /// substitutes the id.
    Blob(Vec<u8>),
}

impl PropValue {
    /// Pre-flight: does this value fit the property's type?
    ///
    /// `Blob` payloads are always accepted here (payloads are opaque to
    /// the client; only kernel semantic checks like mode validity reject
    /// later).
    #[must_use]
    pub fn fits(&self, prop: &PropertyInfo) -> bool {
        match (&prop.kind, self) {
            // Boolean accepts its own shape; object/blob props accept
            // both id references and inline payloads.
            (PropType::Boolean, PropValue::Bool(_))
            | (PropType::Object { .. } | PropType::Blob, PropValue::U64(_) | PropValue::Blob(_)) => {
                true
            }
            (PropType::Boolean, PropValue::U64(v)) => *v <= 1,
            (PropType::Range { min, max }, PropValue::U64(v)) => v >= min && v <= max,
            (PropType::SignedRange { min, max }, PropValue::I64(v)) => v >= min && v <= max,
            (PropType::SignedRange { min, max }, PropValue::U64(v)) => {
                if let Ok(u) = i64::try_from(*v) {
                    u >= *min && u <= *max
                } else {
                    false
                }
            }
            (PropType::Enum { tokens }, PropValue::U64(v)) => tokens.iter().any(|t| t.value == *v),
            (PropType::Bitmask { tokens }, PropValue::U64(v)) => {
                let mask = tokens.iter().fold(0u64, |a, t| a | t.value);
                v & !mask == 0
            }
            _ => false,
        }
    }

    /// The wire `u64` once blobs are resolved (blob payloads are replaced
    /// by their ids by the backend; the resolver is `resolve_wire`).
    #[must_use]
    pub fn wire_u64(&self) -> u64 {
        match self {
            Self::Bool(b) => u64::from(*b),
            Self::U64(v) => *v,
            Self::I64(v) => u64::from_ne_bytes(v.to_ne_bytes()),
            Self::Blob(_) => 0,
        }
    }
}

/// The property snapshot of one object: `(property id, value)` pairs as
/// `drmModeObjectGetProperties` returns them, resolved to names through
/// the backend's catalog.
#[derive(Clone, Debug, Default)]
pub struct ObjectProperties {
    /// All live (id, value) pairs.
    pub entries: Vec<(PropId, u64)>,
}

impl ObjectProperties {
    /// Append a pair (builder).
    #[must_use]
    pub fn with(mut self, id: PropId, value: u64) -> Self {
        self.entries.push((id, value));
        self
    }

    /// Value of one property by id.
    #[must_use]
    pub fn get(&self, id: PropId) -> Option<u64> {
        self.entries.iter().find(|(i, _)| *i == id).map(|(_, v)| *v)
    }
}

/// Well-known property names, in one place.
///
/// These are the registry names every driver uses (they are UAPI, stable
/// across drivers); the constants exist so request construction is
/// compile-checked against typos. Each is `&'static str`; wrap with
/// [`PropertyName::new`] when addressing a request.
pub mod prop {
    /// CRTC: scanout active.
    pub const ACTIVE: &str = "ACTIVE";
    /// CRTC: mode blob id.
    pub const MODE_ID: &str = "MODE_ID";
    /// CRTC: variable refresh rate enable.
    pub const VRR_ENABLED: &str = "VRR_ENABLED";
    /// CRTC: out-fence for the commit's flip.
    pub const OUT_FENCE_PTR: &str = "OUT_FENCE_PTR";
    /// CRTC: post-CSC gamma LUT blob.
    pub const GAMMA_LUT: &str = "GAMMA_LUT";
    /// CRTC: gamma LUT length (immutable).
    pub const GAMMA_LUT_SIZE: &str = "GAMMA_LUT_SIZE";
    /// CRTC: pre-CTM degamma LUT blob.
    pub const DEGAMMA_LUT: &str = "DEGAMMA_LUT";
    /// CRTC: color transformation matrix blob.
    pub const CTM: &str = "CTM";
    /// Connector/plane: the driving CRTC.
    pub const CRTC_ID: &str = "CRTC_ID";
    /// Connector: legacy power control.
    pub const DPMS: &str = "DPMS";
    /// Connector: sink EDID blob (immutable).
    pub const EDID: &str = "EDID";
    /// Connector: RGB quantization range.
    pub const BROADCAST_RGB: &str = "Broadcast RGB";
    /// Connector: HDMI/DP colorspace signalling.
    pub const COLORSPACE: &str = "Colorspace";
    /// Connector: HDR metadata blob.
    pub const HDR_OUTPUT_METADATA: &str = "HDR_OUTPUT_METADATA";
    /// Connector: maximum bits per channel.
    pub const MAX_BPC: &str = "max bpc";
    /// Connector: panel self refresh enable.
    pub const PANEL_SELF_REFRESH: &str = "panel self refresh";
    /// Plane: framebuffer id.
    pub const FB_ID: &str = "FB_ID";
    /// Plane: acquire fence fd (-1 = none).
    pub const IN_FENCE_FD: &str = "IN_FENCE_FD";
    /// Plane: crop x, 16.16 fixed point.
    pub const SRC_X: &str = "SRC_X";
    /// Plane: crop y, 16.16 fixed point.
    pub const SRC_Y: &str = "SRC_Y";
    /// Plane: crop width, 16.16 fixed point.
    pub const SRC_W: &str = "SRC_W";
    /// Plane: crop height, 16.16 fixed point.
    pub const SRC_H: &str = "SRC_H";
    /// Plane: destination x on the CRTC.
    pub const CRTC_X: &str = "CRTC_X";
    /// Plane: destination y on the CRTC.
    pub const CRTC_Y: &str = "CRTC_Y";
    /// Plane: destination width.
    pub const CRTC_W: &str = "CRTC_W";
    /// Plane: destination height.
    pub const CRTC_H: &str = "CRTC_H";
    /// Plane: stacking position.
    pub const ZPOS: &str = "ZPOS";
    /// Plane: rotation bitmask.
    pub const ROTATION: &str = "rotation";
    /// Plane: blend mode enum.
    pub const PIXEL_BLEND_MODE: &str = "pixel blend mode";
    /// Plane: class (immutable).
    pub const TYPE: &str = "type";
    /// Plane: format/modifier capabilities blob (immutable).
    pub const IN_FORMATS: &str = "IN_FORMATS";
    /// Cursor plane: hotspot x.
    pub const HOTSPOT_X: &str = "hotspot_x";
    /// Cursor plane: hotspot y.
    pub const HOTSPOT_Y: &str = "hotspot_y";
}

/// Resolve a property name against an object's catalog, returning the
/// rejection reason if it does not fit.
pub(crate) fn check_value(prop: &PropertyInfo, value: &PropValue) -> Result<()> {
    if prop.immutable {
        return Err(DisplayError::AtomicReject {
            reason: RejectReason::Immutable,
            detail: format!("property {}", prop.name),
        });
    }
    if !value.fits(prop) {
        return Err(DisplayError::AtomicReject {
            reason: RejectReason::PropertyType,
            detail: format!("value does not fit {}", prop.name),
        });
    }
    Ok(())
}

/// Look up a property definition by name in an object catalog.
pub(crate) fn find<'a>(
    catalog: &'a [PropertyInfo],
    name: &PropertyName,
) -> Result<&'a PropertyInfo> {
    catalog
        .iter()
        .find(|p| p.name == *name)
        .ok_or_else(|| DisplayError::AtomicReject {
            reason: RejectReason::UnknownProperty,
            detail: format!("{name}"),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range_prop() -> PropertyInfo {
        PropertyInfo {
            id: PropId::new(1).unwrap(),
            name: PropertyName::new(prop::MAX_BPC),
            kind: PropType::Range { min: 6, max: 16 },
            immutable: false,
        }
    }

    fn enum_prop() -> PropertyInfo {
        PropertyInfo {
            id: PropId::new(2).unwrap(),
            name: PropertyName::new(prop::DPMS),
            kind: PropType::Enum {
                tokens: vec![
                    EnumToken {
                        value: 0,
                        name: "On".into(),
                    },
                    EnumToken {
                        value: 3,
                        name: "Off".into(),
                    },
                ],
            },
            immutable: false,
        }
    }

    fn bool_prop() -> PropertyInfo {
        PropertyInfo {
            id: PropId::new(3).unwrap(),
            name: PropertyName::new(prop::ACTIVE),
            kind: PropType::Boolean,
            immutable: false,
        }
    }

    #[test]
    fn value_fits_matrix() {
        let bpc = range_prop();
        assert!(PropValue::U64(6).fits(&bpc));
        assert!(PropValue::U64(16).fits(&bpc));
        assert!(!PropValue::U64(5).fits(&bpc));
        assert!(!PropValue::U64(17).fits(&bpc));
        assert!(!PropValue::Bool(true).fits(&bpc));

        let dpms = enum_prop();
        assert!(PropValue::U64(0).fits(&dpms));
        assert!(PropValue::U64(3).fits(&dpms));
        assert!(!PropValue::U64(1).fits(&dpms));

        let active = bool_prop();
        assert!(PropValue::Bool(false).fits(&active));
        assert!(PropValue::U64(1).fits(&active));
        assert!(!PropValue::U64(2).fits(&active));
    }

    #[test]
    fn immutable_rejected_by_check() {
        let mut p = range_prop();
        p.immutable = true;
        let err = check_value(&p, &PropValue::U64(8)).unwrap_err();
        assert!(matches!(
            err,
            DisplayError::AtomicReject {
                reason: RejectReason::Immutable,
                ..
            }
        ));
    }

    #[test]
    fn signed_range_fits_both_shapes() {
        let prop = PropertyInfo {
            id: PropId::new(4).unwrap(),
            name: PropertyName::new(prop::IN_FENCE_FD),
            kind: PropType::SignedRange {
                min: -1,
                max: i32::MAX as i64,
            },
            immutable: false,
        };
        assert!(PropValue::I64(-1).fits(&prop));
        assert!(PropValue::U64(5).fits(&prop)); // positive u64 accepted
        assert!(!PropValue::I64(-2).fits(&prop));
        assert_eq!(
            PropValue::I64(-1).wire_u64(),
            u64::from_ne_bytes((-1i64).to_ne_bytes())
        );
    }

    #[test]
    fn object_properties_lookup() {
        let props = ObjectProperties::default()
            .with(PropId::new(1).unwrap(), 0)
            .with(PropId::new(2).unwrap(), 1);
        assert_eq!(props.get(PropId::new(2).unwrap()), Some(1));
        assert_eq!(props.get(PropId::new(3).unwrap()), None);
    }

    #[test]
    fn well_known_names_are_kernel_exact() {
        assert_eq!(prop::BROADCAST_RGB, "Broadcast RGB");
        assert_eq!(prop::MAX_BPC, "max bpc");
        assert_eq!(prop::PIXEL_BLEND_MODE, "pixel blend mode");
        assert_eq!(prop::IN_FORMATS, "IN_FORMATS");
        assert_eq!(PropertyName::new(prop::ACTIVE).as_str(), "ACTIVE");
    }

    #[test]
    fn kernel_flag_bits() {
        assert_eq!(PropType::Boolean.kernel_flags(), 1 << 3);
        assert_eq!(PropType::Blob.kernel_flags(), 1 << 4);
        assert_eq!(PropType::Range { min: 0, max: 1 }.kernel_flags(), 1 << 1);
        assert_eq!(
            PropType::SignedRange { min: 0, max: 1 }.kernel_flags(),
            1 << 1 | 1 << 7
        );
    }
}
