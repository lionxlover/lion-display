//! The device backend seam.
//!
//! [`KmsBackend`] is the object-safe contract every display driver
//! implements: the libdrm translation layer (real hardware) and the mock
//! KMS device (headless CI, the Phase 9 exit-criteria vehicle). The
//! compositor core (Phase 10) talks *only* to this trait — never to
//! libdrm, never to the mock directly — so both paths execute the same
//! request validation, the same property addressing, and the same event
//! model.
//!
//! The trait is deliberately pull-based: the caller owns the event loop
//! and the clock. `try_events` drains what is already due; the mock's
//! time is advanced through its own handle, the real backend's events
//! arrive on the DRM fd the caller polls.

#![forbid(unsafe_code)]

use crate::atomic::AtomicRequest;
use crate::connector::ConnectorInfo;
use crate::error::Result;
use crate::events::DeviceEvent;
use crate::fb::FbSpec;
use crate::ids::{AnyId, BlobId, ConnectorId, CrtcId, FbId, PlaneId};
use crate::mode::Mode;
use crate::plane::PlaneInfo;
use crate::props::{ObjectProperties, PropertyInfo};

/// Driver identity, from `drmGetVersion`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DeviceVersion {
    /// Driver name (`"i915"`, `"amdgpu"`, `"vkms"`).
    pub name: String,
    /// Free-form description.
    pub description: String,
    /// Build date string (kernel convention: `"20240101"`-ish).
    pub date: String,
    /// Driver version triple.
    pub version: (i32, i32, i32),
}

/// The connector/CRTC/plane topology of one device.
#[derive(Clone, Debug)]
pub struct Topology {
    /// Every connector, in resource-list order (indexing matches
    /// nothing else; connectors are addressed by id).
    pub connectors: Vec<ConnectorId>,
    /// Every CRTC, in resource-list order — plane `possible_crtcs`
    /// bit *i* refers to `crtcs[i]`.
    pub crtcs: Vec<CrtcId>,
    /// Every plane.
    pub planes: Vec<PlaneId>,
    /// Maximum framebuffer width (0 = no driver limit).
    pub max_width: u32,
    /// Maximum framebuffer height.
    pub max_height: u32,
}

impl Topology {
    /// The index of a CRTC in the resource list (what `possible_crtcs`
    /// bits index); `None` when unknown.
    #[must_use]
    pub fn crtc_index(&self, crtc: CrtcId) -> Option<u32> {
        self.crtcs.iter().position(|c| *c == crtc).map(|i| i as u32)
    }
}

/// One CRTC snapshot (the fields a compositor needs beyond properties).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CrtcInfo {
    /// Object id.
    pub id: CrtcId,
    /// Current framebuffer, if scanout is live.
    pub current_fb: Option<FbId>,
    /// The active mode, if any.
    pub current_mode: Option<Mode>,
}

/// What a successful atomic commit produced.
#[derive(Clone, Debug)]
pub struct CommitOutcome {
    /// False when the request ran with `TEST_ONLY` (validated, not
    /// applied).
    pub applied: bool,
    /// Out-fences minted by this commit, in CRTC order.
    pub out_fences: Vec<(CrtcId, crate::events::OutFence)>,
}

/// A minted property blob.
#[derive(Clone, Debug)]
pub struct Blob {
    /// The blob id (referencable from atomic requests).
    pub id: BlobId,
    /// The payload.
    pub data: Vec<u8>,
}

/// The display device contract.
pub trait KmsBackend {
    /// Driver identity and version.
    ///
    /// # Errors
    /// [`crate::error::DisplayError::System`] when the version ioctl fails.
    fn version(&self) -> Result<DeviceVersion>;

    /// The connector/CRTC/plane topology (refresh after hotplug).
    ///
    /// # Errors
    /// [`crate::error::DisplayError::System`] when the resource walk fails.
    fn topology(&self) -> Result<Topology>;

    /// Full connector snapshot.
    ///
    /// # Errors
    /// [`crate::error::DisplayError::NotFound`] for an unknown connector;
    /// [`crate::error::DisplayError::System`] on query failure.
    fn connector_info(&self, connector: ConnectorId) -> Result<ConnectorInfo>;

    /// CRTC snapshot.
    ///
    /// # Errors
    /// [`crate::error::DisplayError::NotFound`] for an unknown CRTC;
    /// [`crate::error::DisplayError::System`] on query failure.
    fn crtc_info(&self, crtc: CrtcId) -> Result<CrtcInfo>;

    /// Plane snapshot.
    ///
    /// # Errors
    /// [`crate::error::DisplayError::NotFound`] for an unknown plane;
    /// [`crate::error::DisplayError::System`] on query failure.
    fn plane_info(&self, plane: PlaneId) -> Result<PlaneInfo>;

    /// A property definition by id.
    ///
    /// # Errors
    /// [`crate::error::DisplayError::NotFound`] for an unknown property id.
    fn property(&self, prop: crate::ids::PropId) -> Result<PropertyInfo>;

    /// The property catalog of one object (id + current value pairs).
    ///
    /// # Errors
    /// [`crate::error::DisplayError::NotFound`] for an unknown object;
    /// [`crate::error::DisplayError::System`] on walk failure.
    fn object_properties(&self, obj: AnyId) -> Result<ObjectProperties>;

    /// A blob's payload.
    ///
    /// # Errors
    /// [`crate::error::DisplayError::NotFound`] when the blob id does not resolve.
    fn blob(&self, blob: BlobId) -> Result<Vec<u8>>;

    /// Create a property blob (mode payloads, gamma LUTs, HDR metadata).
    ///
    /// # Errors
    /// [`crate::error::DisplayError::System`] when allocation fails.
    fn create_blob(&mut self, data: &[u8]) -> Result<Blob>;

    /// Destroy a property blob.
    ///
    /// # Errors
    /// [`crate::error::DisplayError::NotFound`] when the blob id does not resolve;
    /// [`crate::error::DisplayError::System`] on ioctl failure.
    fn destroy_blob(&mut self, blob: BlobId) -> Result<()>;

    /// Register a framebuffer (`drmModeAddFB2`). Modifier tracking is on
    /// (AddFB2WithModifiers semantics) via the spec's format.
    ///
    /// # Errors
    /// [`crate::error::DisplayError::AtomicReject`] with [`crate::error::RejectReason::Malformed`]
    /// when the spec fails structural validation;
    /// [`crate::error::DisplayError::System`] on ioctl failure.
    fn add_fb(
        &mut self,
        spec: &FbSpec,
        modifier: Option<ldp_core::buffer::Modifier>,
    ) -> Result<FbId>;

    /// Import a PRIME/dma-buf file descriptor as a GEM handle
    /// (`drmPrimeFDToHandle`): the scanout registration walk's first
    /// step — the imported handle is what an [`FbSpec`] names in its
    /// per-plane `handles`, and only GEM-backed memory can scan out.
    ///
    /// On real hardware only PRIME/dma-buf descriptors import: a memfd
    /// or regular file fails cleanly, and the caller takes the CPU
    /// composition path (the honest degradation, never silent). The
    /// mock mirrors the kernel's object-identity semantics by fd
    /// number — valid while the importing session keeps every buffer's
    /// fd open, which the compositor's pool ownership guarantees.
    ///
    /// # Errors
    /// [`crate::error::DisplayError::System`] when the import fails
    /// (wrong descriptor class, a dead fd, driver refusal).
    fn import_gem(&mut self, fd: i32) -> Result<u32>;

    /// Unregister a framebuffer (`drmModeRmFB`).
    ///
    /// # Errors
    /// [`crate::error::DisplayError::NotFound`] for an unknown framebuffer;
    /// [`crate::error::DisplayError::System`] on ioctl failure.
    fn rm_fb(&mut self, fb: FbId) -> Result<()>;

    /// Submit an atomic commit. On success every operation applied (or
    /// validated, under `TEST_ONLY`); on failure nothing did.
    ///
    /// Page-flip events requested via [`crate::commit::CommitFlags::PAGE_FLIP_EVENT`]
    /// arrive through [`KmsBackend::try_events`] when the flip lands.
    ///
    /// # Errors
    /// [`crate::error::DisplayError::AtomicReject`] with the typed
    /// [`crate::error::RejectReason`] for every validation failure (shape,
    /// unknown object/property, type fit, semantics, modeset,
    /// busy); [`crate::error::DisplayError::System`] for raw ioctl errors the
    /// taxonomy does not cover.
    fn commit(&mut self, request: &AtomicRequest) -> Result<CommitOutcome>;

    /// Drain events already due. Never blocks, never reads a clock.
    ///
    /// # Errors
    /// [`crate::error::DisplayError::System`] when the event read fails.
    fn try_events(&mut self) -> Result<Vec<DeviceEvent>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topology_indexes_crtcs() {
        let t = Topology {
            connectors: vec![ConnectorId::new(91).unwrap()],
            crtcs: vec![CrtcId::new(42).unwrap(), CrtcId::new(43).unwrap()],
            planes: vec![],
            max_width: 8192,
            max_height: 8192,
        };
        assert_eq!(t.crtc_index(CrtcId::new(43).unwrap()), Some(1));
        assert_eq!(t.crtc_index(CrtcId::new(99).unwrap()), None);
    }
}
