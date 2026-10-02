//! Property catalogs for the mock device.
//!
//! DRM properties are *global* objects: `CRTC_ID` is one property id
//! shared by every connector and plane. The mock mirrors that — this
//! module allocates one definition per unique name and attaches it
//! to every object that carries it, with per-object value slots in the
//! device's value table. Immutable blobs (EDID, `IN_FORMATS`) are minted
//! at build time.

#![forbid(unsafe_code)]

use crate::backend::Blob;
use crate::ids::{AnyId, BlobId, CrtcId, PropId};
use crate::plane::{FormatModifier, InFormats};
use crate::props::{prop, EnumToken, ObjectProperties, PropType, PropertyInfo, PropertyName};

use super::device::MockDevice;

impl MockDevice {
    /// Register one property definition (reusing the global id for a
    /// known name — DRM properties are shared objects).
    pub(super) fn prop(&mut self, name: &str, kind: PropType, immutable: bool) -> PropId {
        // DRM properties are global: reuse an existing definition.
        if let Some(existing) = self.prop_defs.iter().find(|p| p.name.as_str() == name) {
            return existing.id;
        }
        let id = PropId::new(self.next_prop).unwrap();
        self.next_prop += 1;
        self.prop_defs.push(PropertyInfo {
            id,
            name: PropertyName::new(name),
            kind,
            immutable,
        });
        id
    }

    pub(super) fn build_catalogs(&mut self) {
        self.build_crtc_catalogs();
        self.build_connector_catalogs();
        self.build_plane_catalogs();
    }

    /// CRTC property catalogs (ACTIVE, MODE_ID, VRR, fences, gamma).
    fn build_crtc_catalogs(&mut self) {
        // Snapshots first: `prop()` needs &mut self, so the per-object
        // walk must not borrow the device vectors in place.
        let crtcs: Vec<(CrtcId, bool, u64, bool)> = self
            .crtcs
            .iter()
            .map(|c| (c.id, c.active, c.mode_blob, c.vrr_enabled))
            .collect();
        for (id, active, mode_blob, vrr_enabled) in crtcs {
            let obj = AnyId::Crtc(id);
            let active_prop = self.prop(prop::ACTIVE, PropType::Boolean, false);
            let mode = self.prop(prop::MODE_ID, PropType::Blob, false);
            let vrr = self.prop(prop::VRR_ENABLED, PropType::Boolean, false);
            let out_fence = self.prop(
                prop::OUT_FENCE_PTR,
                PropType::SignedRange { min: 0, max: 1 },
                false,
            );
            let gamma = self.prop(prop::GAMMA_LUT, PropType::Blob, false);
            let gamma_size = self.prop(
                prop::GAMMA_LUT_SIZE,
                PropType::Range { min: 256, max: 256 },
                true,
            );
            self.catalogs.insert(
                obj,
                vec![active_prop, mode, vrr, out_fence, gamma, gamma_size],
            );
            self.values.insert((obj, active_prop), u64::from(active));
            self.values.insert((obj, mode), mode_blob);
            self.values.insert((obj, vrr), u64::from(vrr_enabled));
            self.values.insert((obj, out_fence), 0);
            self.values.insert((obj, gamma), 0);
            self.values.insert((obj, gamma_size), 256);
        }
    }

    /// Connector property catalogs (binding, DPMS, EDID, signalling).
    fn build_connector_catalogs(&mut self) {
        let conns: Vec<(AnyId, Option<Vec<u8>>)> = self
            .connectors
            .iter()
            .map(|c| (AnyId::Connector(c.info.id), c.info.edid.clone()))
            .collect();
        for (obj, edid_bytes) in conns {
            let crtc_id = self.prop(
                prop::CRTC_ID,
                PropType::Object {
                    kind: crate::ids::ObjectType::Crtc,
                },
                false,
            );
            let dpms = self.prop(prop::DPMS, dpms_type(), false);
            let edid = self.prop(prop::EDID, PropType::Blob, true);
            let broadcast = self.prop(prop::BROADCAST_RGB, broadcast_type(), false);
            let colorspace = self.prop(prop::COLORSPACE, colorspace_type(), false);
            let hdr = self.prop(prop::HDR_OUTPUT_METADATA, PropType::Blob, false);
            let max_bpc = self.prop(prop::MAX_BPC, PropType::Range { min: 6, max: 16 }, false);
            let psr = self.prop(prop::PANEL_SELF_REFRESH, PropType::Boolean, false);
            self.catalogs.insert(
                obj,
                vec![
                    crtc_id, dpms, edid, broadcast, colorspace, hdr, max_bpc, psr,
                ],
            );
            self.values.insert((obj, crtc_id), 0);
            self.values.insert((obj, dpms), 0);
            // EDID blob: minted now, immutable, present when connected.
            let edid_value = match edid_bytes {
                Some(bytes) => u64::from(self.create_blob_internal(bytes).raw()),
                None => 0,
            };
            self.values.insert((obj, edid), edid_value);
            self.values.insert((obj, broadcast), 0);
            self.values.insert((obj, colorspace), 0);
            self.values.insert((obj, hdr), 0);
            self.values.insert((obj, max_bpc), 8);
            self.values.insert((obj, psr), 0);
        }
    }

    /// Plane property catalogs (type, scanout geometry, fences, caps).
    fn build_plane_catalogs(&mut self) {
        let planes: Vec<(AnyId, u64, super::device::PlaneState, Vec<FormatModifier>)> = self
            .planes
            .iter()
            .map(|p| {
                (
                    AnyId::Plane(p.info.id),
                    u64::from(p.info.kind.kernel()),
                    p.state,
                    p.info
                        .in_formats
                        .as_ref()
                        .map(|f| f.pairs.clone())
                        .unwrap_or_default(),
                )
            })
            .collect();
        for (obj, type_value, state, caps) in planes {
            self.register_plane_catalog(obj, type_value, state, &caps);
        }
    }

    /// One plane's catalog: the immutable `type` and `IN_FORMATS`
    /// definitions plus every mutable scanout property, seeded from
    /// the plane's current state.
    fn register_plane_catalog(
        &mut self,
        obj: AnyId,
        type_value: u64,
        state: super::device::PlaneState,
        caps: &[FormatModifier],
    ) {
        let type_prop = self.prop(prop::TYPE, plane_type_enum(), true);
        let crtc_id = self.prop(
            prop::CRTC_ID,
            PropType::Object {
                kind: crate::ids::ObjectType::Crtc,
            },
            false,
        );
        let fb_id = self.prop(
            prop::FB_ID,
            PropType::Object {
                kind: crate::ids::ObjectType::Framebuffer,
            },
            false,
        );
        let src_x = self.prop(prop::SRC_X, u32_range(), false);
        let src_y = self.prop(prop::SRC_Y, u32_range(), false);
        let src_w = self.prop(prop::SRC_W, u32_range(), false);
        let src_h = self.prop(prop::SRC_H, u32_range(), false);
        let crtc_x = self.prop(prop::CRTC_X, i32_signed_range(), false);
        let crtc_y = self.prop(prop::CRTC_Y, i32_signed_range(), false);
        let crtc_w = self.prop(prop::CRTC_W, u32_range(), false);
        let crtc_h = self.prop(prop::CRTC_H, u32_range(), false);
        let in_fence = self.prop(prop::IN_FENCE_FD, fence_range(), false);
        let zpos = self.prop(prop::ZPOS, PropType::Range { min: 0, max: 255 }, false);
        let blend = self.prop(prop::PIXEL_BLEND_MODE, blend_enum(), false);
        let in_formats = self.prop(prop::IN_FORMATS, PropType::Blob, true);
        self.catalogs.insert(
            obj,
            vec![
                type_prop, crtc_id, fb_id, src_x, src_y, src_w, src_h, crtc_x, crtc_y, crtc_w,
                crtc_h, in_fence, zpos, blend, in_formats,
            ],
        );
        self.values.insert((obj, type_prop), type_value);
        self.values.insert((obj, crtc_id), state.crtc);
        self.values.insert((obj, fb_id), state.fb);
        self.values.insert((obj, src_x), state.src_x);
        self.values.insert((obj, src_y), state.src_y);
        self.values.insert((obj, src_w), state.src_w);
        self.values.insert((obj, src_h), state.src_h);
        self.values.insert((obj, crtc_x), wire_i64(state.crtc_x));
        self.values.insert((obj, crtc_y), wire_i64(state.crtc_y));
        self.values.insert((obj, crtc_w), state.crtc_w);
        self.values.insert((obj, crtc_h), state.crtc_h);
        self.values.insert((obj, in_fence), wire_i64(-1));
        self.values.insert((obj, zpos), state.zpos);
        self.values.insert((obj, blend), 1);
        let caps_blob = InFormats::encode(caps);
        let caps_id = self.create_blob_internal(caps_blob);
        self.values
            .insert((obj, in_formats), u64::from(caps_id.raw()));
    }

    /// Mint a blob into the store (id allocation + insert).
    pub(super) fn create_blob_internal(&mut self, data: Vec<u8>) -> BlobId {
        let id = BlobId::new(self.next_blob).unwrap();
        self.next_blob += 1;
        self.blobs.push(Blob { id, data });
        id
    }
}

/// The property ids attached to one object.
pub(super) fn catalog_of(dev: &MockDevice, obj: AnyId) -> Vec<PropId> {
    dev.catalogs.get(&obj).cloned().unwrap_or_default()
}

/// Read one object's property snapshot in catalog order.
pub(super) fn object_properties(dev: &MockDevice, obj: AnyId) -> ObjectProperties {
    let mut props = ObjectProperties::default();
    for pid in catalog_of(dev, obj) {
        let value = dev.values.get(&(obj, pid)).copied().unwrap_or(0);
        props = props.with(pid, value);
    }
    props
}

/// The DPMS enum tokens.
fn dpms_type() -> PropType {
    PropType::Enum {
        tokens: vec![
            EnumToken {
                value: 0,
                name: "On".into(),
            },
            EnumToken {
                value: 1,
                name: "Standby".into(),
            },
            EnumToken {
                value: 2,
                name: "Suspend".into(),
            },
            EnumToken {
                value: 3,
                name: "Off".into(),
            },
        ],
    }
}

/// The Broadcast RGB enum tokens.
fn broadcast_type() -> PropType {
    PropType::Enum {
        tokens: vec![
            EnumToken {
                value: 0,
                name: "Automatic".into(),
            },
            EnumToken {
                value: 1,
                name: "Full".into(),
            },
            EnumToken {
                value: 2,
                name: "Limited 16:235".into(),
            },
        ],
    }
}

/// The HDMI/DP colorspace enum tokens.
fn colorspace_type() -> PropType {
    PropType::Enum {
        tokens: vec![
            EnumToken {
                value: 0,
                name: "Default".into(),
            },
            EnumToken {
                value: 1,
                name: "BT709".into(),
            },
            EnumToken {
                value: 3,
                name: "RGB".into(),
            },
            EnumToken {
                value: 4,
                name: "RGB_Limited".into(),
            },
        ],
    }
}

/// The immutable plane class enum.
fn plane_type_enum() -> PropType {
    PropType::Enum {
        tokens: vec![
            EnumToken {
                value: 0,
                name: "Overlay".into(),
            },
            EnumToken {
                value: 1,
                name: "Primary".into(),
            },
            EnumToken {
                value: 2,
                name: "Cursor".into(),
            },
        ],
    }
}

/// The pixel blend mode enum.
fn blend_enum() -> PropType {
    PropType::Enum {
        tokens: vec![
            EnumToken {
                value: 0,
                name: "None".into(),
            },
            EnumToken {
                value: 1,
                name: "Pre-multiplied".into(),
            },
            EnumToken {
                value: 2,
                name: "Coverage".into(),
            },
        ],
    }
}

/// A `[0, u32::MAX]` range (SRC_*/CRTC_W/H geometry).
fn u32_range() -> PropType {
    PropType::Range {
        min: 0,
        max: u64::from(u32::MAX),
    }
}

/// An `[i32::MIN, i32::MAX]` signed range (CRTC_X/Y).
fn i32_signed_range() -> PropType {
    PropType::SignedRange {
        min: i64::from(i32::MIN),
        max: i64::from(i32::MAX),
    }
}

/// The `[-1, i32::MAX]` acquire-fence range.
fn fence_range() -> PropType {
    PropType::SignedRange {
        min: -1,
        max: i64::from(i32::MAX),
    }
}

/// The wire encoding of a signed property value (sign-extended into
/// the u64 the property store keeps).
fn wire_i64(v: i64) -> u64 {
    u64::from_ne_bytes(v.to_ne_bytes())
}
