//! Atomic commit requests — full pipeline state in one ioctl.
//!
//! An [`AtomicRequest`] names `(object, property, value)` triples plus
//! commit [`CommitFlags`]. The builder wraps the well-known choreography:
//! enabling a pipeline (connector + CRTC + mode + primary plane), page
//! flips (plane FB update on an active pipeline), VRR toggles, DPMS. The
//! request is *declarative*: the backend resolves property names to ids,
//! mints mode blobs, validates, and either applies everything or nothing.
//!
//! Source rectangles are 16.16 fixed point on the wire; the builder takes
//! pixel-space coordinates and shifts, and refuses fractional/zero
//! geometry at build time (fail-fast beats a kernel round trip).
//!
//! Kernel flag values (checked against `drm_mode.h`): page-flip event
//! `0x01`, async `0x02`, TEST_ONLY `0x0100`, NONBLOCK `0x0200`,
//! ALLOW_MODESET `0x0400`.

#![forbid(unsafe_code)]

use crate::commit::{CommitFlags, DpmsState, DstRect, SrcRect};
use crate::error::{DisplayError, RejectReason, Result};
use crate::ids::{AnyId, ConnectorId, CrtcId, FbId, PlaneId};
use crate::mode::Mode;
use crate::props::{prop, PropValue, PropertyName};

/// One raw operation inside a request.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AtomicOp {
    /// Target object.
    pub obj: AnyId,
    /// Property name (resolved by the backend).
    pub name: PropertyName,
    /// Value (blobs minted by the backend).
    pub value: PropValue,
}

/// A declarative atomic commit.
#[derive(Clone, Default, Debug)]
pub struct AtomicRequest {
    /// Commit flags.
    pub flags: CommitFlags,
    /// The operations, in insertion order.
    pub ops: Vec<AtomicOp>,
}

impl AtomicRequest {
    /// An empty request.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            flags: CommitFlags(0),
            ops: Vec::new(),
        }
    }

    /// Set the full flag word (kernel encoding).
    #[must_use]
    pub const fn with_flags(mut self, flags: CommitFlags) -> Self {
        self.flags = flags;
        self
    }

    /// OR in one flag.
    #[must_use]
    pub const fn flag(mut self, flag: CommitFlags) -> Self {
        self.flags = CommitFlags(self.flags.0 | flag.0);
        self
    }

    /// Set one property on one object (raw escape hatch).
    #[must_use]
    pub fn set(mut self, obj: AnyId, name: &str, value: PropValue) -> Self {
        self.ops.push(AtomicOp {
            obj,
            name: PropertyName::new(name),
            value,
        });
        self
    }

    /// Enable/disable a CRTC.
    #[must_use]
    pub fn crtc_active(self, crtc: CrtcId, active: bool) -> Self {
        self.set(AnyId::Crtc(crtc), prop::ACTIVE, PropValue::Bool(active))
    }

    /// Set the CRTC mode (payload becomes a mode blob).
    #[must_use]
    pub fn crtc_mode(self, crtc: CrtcId, mode: &Mode) -> Self {
        self.set(
            AnyId::Crtc(crtc),
            prop::MODE_ID,
            PropValue::Blob(mode.to_blob().to_vec()),
        )
    }

    /// Reference an existing mode blob.
    #[must_use]
    pub fn crtc_mode_blob(self, crtc: CrtcId, blob: crate::ids::BlobId) -> Self {
        self.set(
            AnyId::Crtc(crtc),
            prop::MODE_ID,
            PropValue::U64(u64::from(blob.raw())),
        )
    }

    /// Toggle variable refresh rate.
    #[must_use]
    pub fn crtc_vrr(self, crtc: CrtcId, enabled: bool) -> Self {
        self.set(
            AnyId::Crtc(crtc),
            prop::VRR_ENABLED,
            PropValue::Bool(enabled),
        )
    }

    /// Ask for an out-fence on the CRTC's next flip.
    #[must_use]
    pub fn crtc_out_fence(self, crtc: CrtcId) -> Self {
        self.set(AnyId::Crtc(crtc), prop::OUT_FENCE_PTR, PropValue::U64(1))
    }

    /// Bind a connector to a CRTC.
    #[must_use]
    pub fn connector_bind(self, connector: ConnectorId, crtc: CrtcId) -> Self {
        self.set(
            AnyId::Connector(connector),
            prop::CRTC_ID,
            PropValue::U64(u64::from(crtc.raw())),
        )
    }

    /// Unbind a connector.
    #[must_use]
    pub fn connector_unbind(self, connector: ConnectorId) -> Self {
        self.set(
            AnyId::Connector(connector),
            prop::CRTC_ID,
            PropValue::U64(0),
        )
    }

    /// Set connector DPMS.
    #[must_use]
    pub fn connector_dpms(self, connector: ConnectorId, state: DpmsState) -> Self {
        self.set(
            AnyId::Connector(connector),
            prop::DPMS,
            PropValue::U64(state.wire()),
        )
    }

    /// Engage or release connector panel self-refresh (the Phase 35
    /// sleeping-panel doctrine). Writing the property on engages: the
    /// panel holds its own GRAM copy and the display engine may stop
    /// scanning. Writing it off — or simply submitting a page flip to
    /// the CRTC the connector drives, the kernel's implicit rescan —
    /// releases it. A connector that does not expose the property
    /// rejects the write; the caller treats that as the honest
    /// "unsupported, never retry" report, never a fatal error (a
    /// power hint degrades, a session does not die for one).
    #[must_use]
    pub fn connector_psr(self, connector: ConnectorId, on: bool) -> Self {
        self.set(
            AnyId::Connector(connector),
            prop::PANEL_SELF_REFRESH,
            PropValue::U64(u64::from(on)),
        )
    }

    /// Enable a plane: CRTC, FB, crop, destination.
    #[must_use]
    pub fn plane_on(
        self,
        plane: PlaneId,
        crtc: CrtcId,
        fb: FbId,
        src: SrcRect,
        dst: DstRect,
    ) -> Self {
        self.set(
            AnyId::Plane(plane),
            prop::CRTC_ID,
            PropValue::U64(u64::from(crtc.raw())),
        )
        .set(
            AnyId::Plane(plane),
            prop::FB_ID,
            PropValue::U64(u64::from(fb.raw())),
        )
        .set(
            AnyId::Plane(plane),
            prop::SRC_X,
            PropValue::U64(src.wire_x()),
        )
        .set(
            AnyId::Plane(plane),
            prop::SRC_Y,
            PropValue::U64(src.wire_y()),
        )
        .set(
            AnyId::Plane(plane),
            prop::SRC_W,
            PropValue::U64(src.wire_w()),
        )
        .set(
            AnyId::Plane(plane),
            prop::SRC_H,
            PropValue::U64(src.wire_h()),
        )
        .set(
            AnyId::Plane(plane),
            prop::CRTC_X,
            PropValue::I64(i64::from(dst.x)),
        )
        .set(
            AnyId::Plane(plane),
            prop::CRTC_Y,
            PropValue::I64(i64::from(dst.y)),
        )
        .set(
            AnyId::Plane(plane),
            prop::CRTC_W,
            PropValue::U64(u64::from(dst.w)),
        )
        .set(
            AnyId::Plane(plane),
            prop::CRTC_H,
            PropValue::U64(u64::from(dst.h)),
        )
    }

    /// Disable a plane (FB 0 releases it).
    #[must_use]
    pub fn plane_off(self, plane: PlaneId) -> Self {
        self.set(AnyId::Plane(plane), prop::FB_ID, PropValue::U64(0))
            .set(AnyId::Plane(plane), prop::CRTC_ID, PropValue::U64(0))
    }

    /// Attach an acquire fence to a plane (fd, or −1 to clear).
    #[must_use]
    pub fn plane_in_fence(self, plane: PlaneId, fd: i64) -> Self {
        self.set(AnyId::Plane(plane), prop::IN_FENCE_FD, PropValue::I64(fd))
    }

    /// Set plane stacking order.
    #[must_use]
    pub fn plane_zpos(self, plane: PlaneId, zpos: u64) -> Self {
        self.set(AnyId::Plane(plane), prop::ZPOS, PropValue::U64(zpos))
    }

    /// Pre-flight: no duplicate (object, property) pairs, at least one op
    /// when not empty-flagged.
    ///
    /// # Errors
    /// [`RejectReason::Malformed`] on duplicates or empty requests.
    pub fn validate_shape(&self) -> Result<()> {
        if self.ops.is_empty() {
            return Err(DisplayError::AtomicReject {
                reason: RejectReason::Malformed,
                detail: "empty request".into(),
            });
        }
        for (i, op) in self.ops.iter().enumerate() {
            if self.ops[..i]
                .iter()
                .any(|other| other.obj == op.obj && other.name == op.name)
            {
                return Err(DisplayError::AtomicReject {
                    reason: RejectReason::Malformed,
                    detail: format!("duplicate set of {} on {}", op.name, op.obj),
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids() -> (CrtcId, PlaneId, ConnectorId, FbId) {
        (
            CrtcId::new(42).unwrap(),
            PlaneId::new(50).unwrap(),
            ConnectorId::new(60).unwrap(),
            FbId::new(70).unwrap(),
        )
    }

    #[test]
    fn full_pipeline_request_shape() {
        let (crtc, plane, conn, fb) = ids();
        let mode = Mode::panel_1080p60();
        let req = AtomicRequest::new()
            .flag(CommitFlags::ALLOW_MODESET)
            .flag(CommitFlags::PAGE_FLIP_EVENT)
            .connector_bind(conn, crtc)
            .crtc_active(crtc, true)
            .crtc_mode(crtc, &mode)
            .plane_on(
                plane,
                crtc,
                fb,
                SrcRect::new(0, 0, 1920, 1080).unwrap(),
                DstRect::new(0, 0, 1920, 1080).unwrap(),
            );
        assert!(req.flags.allows_modeset());
        assert!(req.flags.wants_flip_event());
        assert!(!req.flags.is_test_only());
        assert_eq!(req.ops.len(), 3 + 10);
        req.validate_shape().unwrap();
        // Wire values: 16.16 source.
        let src_w = req
            .ops
            .iter()
            .find(|o| o.name.as_str() == prop::SRC_W)
            .unwrap();
        assert_eq!(src_w.value, PropValue::U64(u64::from(1920u32) << 16));
        let crtc_x = req
            .ops
            .iter()
            .find(|o| o.name.as_str() == prop::CRTC_X)
            .unwrap();
        assert_eq!(crtc_x.value, PropValue::I64(0));
    }

    #[test]
    fn duplicate_and_empty_rejected() {
        let (crtc, _, _, _) = ids();
        let dup = AtomicRequest::new()
            .crtc_active(crtc, true)
            .crtc_active(crtc, false);
        let err = dup.validate_shape().unwrap_err();
        assert!(matches!(
            err,
            DisplayError::AtomicReject {
                reason: RejectReason::Malformed,
                ..
            }
        ));
        assert!(AtomicRequest::new().validate_shape().is_err());
    }

    #[test]
    fn zero_size_rejected_at_build() {
        assert!(SrcRect::new(0, 0, 0, 10).is_err());
        assert!(DstRect::new(0, 0, 10, 0).is_err());
        let (crtc, plane, _, fb) = ids();
        let req = AtomicRequest::new().plane_on(
            plane,
            crtc,
            fb,
            SrcRect::new(0, 0, 8, 8).unwrap(),
            DstRect::new(-4, -4, 8, 8).unwrap(),
        );
        let crtc_x = req
            .ops
            .iter()
            .find(|o| o.name.as_str() == prop::CRTC_X)
            .unwrap();
        assert_eq!(crtc_x.value, PropValue::I64(-4));
    }
}
