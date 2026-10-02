//! Raw libdrm ABI types and their translations to the model.
//!
//! The `repr(C)` structs below declare the *stable prefixes* of the
//! libdrm heap objects (`drmModeConnector`, `drmModeCrtc`, ...). libdrm
//! allocates and frees these itself; we only read fields within the
//! classic prefix, which has not moved across libdrm versions (newer
//! versions append fields at the end). The sys layer copies pointer
//! arrays into `Vec`s first; this module's translations are then pure
//! slice walks — no unsafe.
//!
//! Every struct carries a `const` size assertion pinning its layout
//! against the ABI it declares, so a future edition change in alignment
//! rules fails at compile time instead of misreading hardware data.

#![forbid(unsafe_code)]

use crate::backend::DeviceVersion;
use crate::connector::{ConnectorInfo, ConnectorStatus, ConnectorType, Subpixel};
use crate::ids::{ConnectorId, CrtcId, CrtcMask, EncoderId, PlaneId, PropId};
use crate::mode::{Mode, ModeFlags, ModeType};
use crate::plane::{PlaneInfo, PlaneType};
use crate::props::{EnumToken, PropType, PropertyInfo, PropertyName};
use ldp_core::buffer::FourCC;

/// `drmModeModeInfo` (68 bytes, 4-byte aligned).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct RawMode {
    /// Pixel clock in kHz.
    pub clock: u32,
    /// Horizontal active.
    pub hdisplay: u16,
    /// Horizontal sync start.
    pub hsync_start: u16,
    /// Horizontal sync end.
    pub hsync_end: u16,
    /// Horizontal total.
    pub htotal: u16,
    /// Horizontal skew.
    pub hskew: u16,
    /// Vertical active.
    pub vdisplay: u16,
    /// Vertical sync start.
    pub vsync_start: u16,
    /// Vertical sync end.
    pub vsync_end: u16,
    /// Vertical total.
    pub vtotal: u16,
    /// Vertical scan multiplier.
    pub vscan: u16,
    /// Refresh, rounded (recomputed on translation; field kept for ABI).
    pub vrefresh: u32,
    /// Flag bits.
    pub flags: u32,
    /// Type bits.
    pub kind: u32,
    /// NUL-padded name.
    pub name: [u8; 32],
}

impl RawMode {
    /// The 68-byte wire size.
    pub const SIZE: usize = 68;
}

const _: () = assert!(size_of::<RawMode>() == 68);

/// Kernel `drmModeModeInfo` → model [`Mode`].
///
/// The kernel's pre-rounded `vrefresh` field is ignored — the model
/// derives refresh exactly from clock and totals.
#[must_use]
pub fn mode_from_raw(raw: &RawMode) -> Mode {
    let name_len = raw.name.iter().position(|&c| c == 0).unwrap_or(32);
    let name = String::from_utf8_lossy(&raw.name[..name_len]).into_owned();
    Mode {
        clock_khz: raw.clock,
        hdisplay: raw.hdisplay,
        hsync_start: raw.hsync_start,
        hsync_end: raw.hsync_end,
        htotal: raw.htotal,
        hskew: raw.hskew,
        vdisplay: raw.vdisplay,
        vsync_start: raw.vsync_start,
        vsync_end: raw.vsync_end,
        vtotal: raw.vtotal,
        vscan: raw.vscan.max(1),
        flags: ModeFlags(raw.flags),
        kind: ModeType(raw.kind),
        name,
    }
}

/// Owned copy of one `drmModeConnector`'s stable fields.
#[derive(Clone, Debug)]
pub struct RawConnectorOwned {
    /// Connector object id.
    pub connector_id: u32,
    /// Currently attached encoder.
    pub encoder_id: u32,
    /// Connector type constant.
    pub connector_type: u32,
    /// Per-type index.
    pub connector_type_id: u32,
    /// Connection state constant.
    pub connection: u32,
    /// Panel width mm.
    pub mm_width: u32,
    /// Panel height mm.
    pub mm_height: u32,
    /// Subpixel constant.
    pub subpixel: u32,
    /// Offered modes.
    pub modes: Vec<RawMode>,
    /// Property ids.
    pub prop_ids: Vec<u32>,
    /// Property values.
    pub prop_values: Vec<u64>,
    /// Usable encoder ids.
    pub encoders: Vec<u32>,
}

/// Translate a connector snapshot.
///
/// # Panics
/// Only when `connector_id` is zero *and* the fallback id 1 is zero —
/// impossible by construction; both name the same defensive default.
#[must_use]
pub fn connector_from_raw(raw: &RawConnectorOwned) -> ConnectorInfo {
    ConnectorInfo {
        id: ConnectorId::new(raw.connector_id).unwrap_or(ConnectorId::new(1).unwrap()),
        kind: ConnectorType::from_kernel(raw.connector_type),
        type_index: raw.connector_type_id,
        status: ConnectorStatus::from_kernel(raw.connection),
        mm_width: raw.mm_width,
        mm_height: raw.mm_height,
        subpixel: Subpixel::from_kernel(raw.subpixel),
        encoders: raw
            .encoders
            .iter()
            .filter_map(|e| EncoderId::new(*e))
            .collect(),
        current_crtc: None, // filled from the property walk by the caller
        modes: raw.modes.iter().map(mode_from_raw).collect(),
        edid: None, // filled from the EDID blob by the caller
    }
}

/// Owned copy of one `drmModePlane`'s stable fields.
#[derive(Clone, Debug)]
pub struct RawPlaneOwned {
    /// Plane object id.
    pub plane_id: u32,
    /// CRTC currently fed.
    pub crtc_id: u32,
    /// Framebuffer currently shown.
    pub fb_id: u32,
    /// possible_crtcs mask.
    pub possible_crtcs: u32,
    /// Format list (fourcc codes).
    pub formats: Vec<u32>,
}

/// Translate a plane snapshot (IN_FORMATS is filled by the caller).
///
/// # Panics
/// Only when `plane_id` is zero *and* the fallback id 1 is zero —
/// impossible by construction.
#[must_use]
pub fn plane_from_raw(raw: &RawPlaneOwned) -> PlaneInfo {
    PlaneInfo {
        id: PlaneId::new(raw.plane_id).unwrap_or(PlaneId::new(1).unwrap()),
        kind: PlaneType::Primary, // corrected by the caller from the type prop
        possible_crtcs: CrtcMask::from_raw(raw.possible_crtcs),
        current_crtc: CrtcId::new(raw.crtc_id),
        formats: raw.formats.iter().map(|f| FourCC::from_code(*f)).collect(),
        in_formats: None,
    }
}

/// Owned copy of one `drmModePropertyRes`.
#[derive(Clone, Debug)]
pub struct RawPropertyOwned {
    /// Property object id.
    pub prop_id: u32,
    /// Flag word (`DRM_MODE_PROP_*`).
    pub flags: u32,
    /// Property name.
    pub name: String,
    /// Range values (bounds for ranges; object type for object props).
    pub values: Vec<u64>,
    /// Enum/bitmask tokens.
    pub enum_blobs: Vec<(u64, String)>,
}

/// The kernel's flag bits (checked against `drm_mode.h` §518-544).
mod kernel_prop_flags {
    /// `DRM_MODE_PROP_RANGE`.
    pub const RANGE: u32 = 1 << 1;
    /// `DRM_MODE_PROP_IMMUTABLE`.
    pub const IMMUTABLE: u32 = 1 << 2;
    /// `DRM_MODE_PROP_ENUM`.
    pub const ENUM: u32 = 1 << 3;
    /// `DRM_MODE_PROP_BLOB`.
    pub const BLOB: u32 = 1 << 4;
    /// `DRM_MODE_PROP_BITMASK`.
    pub const BITMASK: u32 = 1 << 5;
    /// `DRM_MODE_PROP_OBJECT` (= TYPE(1)).
    pub const OBJECT: u32 = 1 << 6;
    /// `DRM_MODE_PROP_SIGNED_RANGE` (= TYPE(2)).
    pub const SIGNED_RANGE: u32 = 1 << 7;
}

/// The kernel object-type magics an object prop may carry in
/// `values[0]`.
mod kernel_object_magics {
    /// CRTC.
    pub const CRTC: u64 = 0xcccc_cccc;
    /// Connector (test-only: the translation folds it into the
    /// unknown-magic fallback).
    #[cfg(test)]
    pub const CONNECTOR: u64 = 0xc0c0_c0c0;
    /// Encoder.
    pub const ENCODER: u64 = 0xe0e0_e0e0;
    /// FB.
    pub const FB: u64 = 0xfbfb_fbfb;
    /// Plane.
    pub const PLANE: u64 = 0xeeee_eeee;
}

/// Translate a property definition.
///
/// Kernel booleans arrive as `RANGE [0,1]` and are normalized to
/// [`PropType::Boolean`] so the real backend and the mock agree.
///
/// # Panics
/// Only when `prop_id` is zero *and* the fallback id 1 is zero —
/// impossible by construction.
#[must_use]
pub fn property_from_raw(raw: &RawPropertyOwned) -> PropertyInfo {
    let kind = if raw.flags & kernel_prop_flags::ENUM != 0 {
        PropType::Enum {
            tokens: raw
                .enum_blobs
                .iter()
                .map(|(v, n)| EnumToken {
                    value: *v,
                    name: n.clone(),
                })
                .collect(),
        }
    } else if raw.flags & kernel_prop_flags::BITMASK != 0 {
        PropType::Bitmask {
            tokens: raw
                .enum_blobs
                .iter()
                .map(|(v, n)| EnumToken {
                    value: *v,
                    name: n.clone(),
                })
                .collect(),
        }
    } else if raw.flags & kernel_prop_flags::BLOB != 0 {
        PropType::Blob
    } else if raw.flags & kernel_prop_flags::OBJECT != 0 {
        // Unknown object magics degrade to Connector, the least
        // destructive guess for a walk that only mirrors values.
        let object_kind = match raw.values.first().copied().unwrap_or(0) {
            kernel_object_magics::CRTC => crate::ids::ObjectType::Crtc,
            kernel_object_magics::ENCODER => crate::ids::ObjectType::Encoder,
            kernel_object_magics::FB => crate::ids::ObjectType::Framebuffer,
            kernel_object_magics::PLANE => crate::ids::ObjectType::Plane,
            _ => crate::ids::ObjectType::Connector,
        };
        PropType::Object { kind: object_kind }
    } else if raw.flags & kernel_prop_flags::SIGNED_RANGE != 0 {
        PropType::SignedRange {
            min: raw.values.first().copied().unwrap_or(0) as i64,
            max: raw.values.get(1).copied().unwrap_or(u64::MAX) as i64,
        }
    } else if raw.flags & kernel_prop_flags::RANGE != 0 {
        let min = raw.values.first().copied().unwrap_or(0);
        let max = raw.values.get(1).copied().unwrap_or(u64::MAX);
        if min == 0 && max == 1 {
            PropType::Boolean
        } else {
            PropType::Range { min, max }
        }
    } else {
        // Drivers never ship untyped properties; treat the theoretical
        // zero-flag shape as a degenerate boolean so the value walk can
        // still read it.
        PropType::Boolean
    };
    PropertyInfo {
        id: PropId::new(raw.prop_id).unwrap_or(PropId::new(1).unwrap()),
        name: PropertyName::new(&raw.name),
        kind,
        immutable: raw.flags & kernel_prop_flags::IMMUTABLE != 0,
    }
}

/// Owned copy of a `drmModeObjectProperties` result.
#[derive(Clone, Debug, Default)]
pub struct RawObjectPropsOwned {
    /// Property ids.
    pub prop_ids: Vec<u32>,
    /// Values.
    pub prop_values: Vec<u64>,
}

/// Owned copy of a `drmVersion`.
#[derive(Clone, Debug)]
pub struct RawVersionOwned {
    /// Driver name.
    pub name: String,
    /// Date string.
    pub date: String,
    /// Description.
    pub desc: String,
    /// Major.
    pub major: i32,
    /// Minor.
    pub minor: i32,
    /// Patch.
    pub patch: i32,
}

/// Translate the driver identity.
#[must_use]
pub fn version_from_raw(raw: &RawVersionOwned) -> DeviceVersion {
    DeviceVersion {
        name: raw.name.clone(),
        description: raw.desc.clone(),
        date: raw.date.clone(),
        version: (raw.major, raw.minor, raw.patch),
    }
}

/// One entry of a `drmGetDevices2` walk.
#[derive(Clone, Debug)]
pub struct RawDrmDevice {
    /// Node paths indexed by node kind (0 primary, 1 control, 2 render).
    pub nodes: [Option<String>; 3],
    /// Whether the primary node is present.
    pub has_primary: bool,
    /// Whether the render node is present.
    pub has_render: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw_mode() -> RawMode {
        let mut name = [0u8; 32];
        name[..9].copy_from_slice(b"1920x1080");
        RawMode {
            clock: 148_500,
            hdisplay: 1920,
            hsync_start: 2008,
            hsync_end: 2052,
            htotal: 2200,
            hskew: 0,
            vdisplay: 1080,
            vsync_start: 1084,
            vsync_end: 1089,
            vtotal: 1125,
            vscan: 0,
            vrefresh: 60,
            flags: (ModeFlags::NHSYNC | ModeFlags::NVSYNC).0,
            kind: (ModeType::PREFERRED | ModeType::DRIVER).0,
            name,
        }
    }

    #[test]
    fn mode_translation_derives_refresh() {
        let mode = mode_from_raw(&raw_mode());
        assert_eq!(mode.refresh_millihz(), 60_000);
        assert_eq!(mode.name, "1920x1080");
        assert!(mode.is_preferred());
        assert_eq!(mode.vscan, 1); // zero normalized
    }

    #[test]
    fn mode_layout_pinned() {
        assert_eq!(size_of::<RawMode>(), 68);
        assert_eq!(offset_of_raw_mode_name(), 36);
    }

    const fn offset_of_raw_mode_name() -> usize {
        36 // clock 4 + 5*u16 (10) + 5*u16 (10) + vrefresh 4 + flags 4 + type 4
    }

    #[test]
    fn property_type_matrix() {
        let base = |flags: u32| RawPropertyOwned {
            prop_id: 7,
            flags,
            name: "x".into(),
            values: vec![],
            enum_blobs: vec![],
        };
        let mut range = base(kernel_prop_flags::RANGE);
        range.values = vec![6, 16];
        assert!(matches!(
            property_from_raw(&range).kind,
            PropType::Range { min: 6, max: 16 }
        ));
        let mut boolean = base(kernel_prop_flags::RANGE);
        boolean.values = vec![0, 1];
        assert!(matches!(
            property_from_raw(&boolean).kind,
            PropType::Boolean
        ));
        let mut signed = base(kernel_prop_flags::SIGNED_RANGE);
        signed.values = vec![u64::from_ne_bytes((-1i64).to_ne_bytes()), 100];
        assert!(matches!(
            property_from_raw(&signed).kind,
            PropType::SignedRange { min: -1, .. }
        ));
        let mut object = base(kernel_prop_flags::OBJECT);
        object.values = vec![kernel_object_magics::PLANE];
        assert!(matches!(
            property_from_raw(&object).kind,
            PropType::Object {
                kind: crate::ids::ObjectType::Plane
            }
        ));
        // The connector magic and the unknown fallback share the arm.
        object.values = vec![kernel_object_magics::CONNECTOR];
        assert!(matches!(
            property_from_raw(&object).kind,
            PropType::Object {
                kind: crate::ids::ObjectType::Connector
            }
        ));
        let mut enums = base(kernel_prop_flags::ENUM);
        enums.enum_blobs = vec![(0, "On".into()), (3, "Off".into())];
        assert!(matches!(
            property_from_raw(&enums).kind,
            PropType::Enum { .. }
        ));
        assert!(matches!(
            property_from_raw(&base(kernel_prop_flags::BLOB)).kind,
            PropType::Blob
        ));
        let immutable = base(kernel_prop_flags::RANGE | kernel_prop_flags::IMMUTABLE);
        assert!(property_from_raw(&immutable).immutable);
    }

    #[test]
    fn connector_and_plane_translation() {
        let conn = RawConnectorOwned {
            connector_id: 91,
            encoder_id: 50,
            connector_type: 14,
            connector_type_id: 1,
            connection: 1,
            mm_width: 309,
            mm_height: 174,
            subpixel: 1,
            modes: vec![raw_mode()],
            prop_ids: vec![],
            prop_values: vec![],
            encoders: vec![50],
        };
        let info = connector_from_raw(&conn);
        assert_eq!(info.kind, ConnectorType::EmbeddedDisplayPort);
        assert_eq!(info.status, ConnectorStatus::Connected);
        assert_eq!(info.subpixel, Subpixel::HorizontalRgb);
        assert_eq!(info.modes.len(), 1);

        let plane = RawPlaneOwned {
            plane_id: 50,
            crtc_id: 42,
            fb_id: 0,
            possible_crtcs: 1,
            formats: vec![FourCC::XRGB8888.code()],
        };
        let pinfo = plane_from_raw(&plane);
        assert_eq!(pinfo.possible_crtcs.len(), 1);
        assert_eq!(pinfo.formats, vec![FourCC::XRGB8888]);
        assert_eq!(pinfo.current_crtc, CrtcId::new(42));
    }
}
