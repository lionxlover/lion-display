//! The [`KmsBackend`] implementation for the mock device — pure queries
//! over the device's state (no mutation, no clock).

#![forbid(unsafe_code)]

use crate::atomic::AtomicRequest;
use crate::backend::{Blob, CommitOutcome, CrtcInfo, DeviceVersion, KmsBackend, Topology};
use crate::connector::ConnectorInfo;
use crate::error::{DisplayError, Result};
use crate::events::DeviceEvent;
use crate::fb::FbSpec;
use crate::ids::{AnyId, BlobId, ConnectorId, CrtcId, FbId, PlaneId, PropId};
use crate::plane::{PlaneInfo, PlaneType};
use crate::props::{ObjectProperties, PropertyInfo};
use ldp_core::buffer::Modifier;

use super::catalog;
use super::device::MockDevice;

impl KmsBackend for MockDevice {
    fn version(&self) -> Result<DeviceVersion> {
        Ok(DeviceVersion {
            name: "mockdrm".into(),
            description: "LDP deterministic mock KMS".into(),
            date: "20260924".into(),
            version: (1, 0, 0),
        })
    }

    fn topology(&self) -> Result<Topology> {
        Ok(Topology {
            connectors: self.connectors.iter().map(|c| c.info.id).collect(),
            crtcs: self.crtcs.iter().map(|c| c.id).collect(),
            planes: self.planes.iter().map(|p| p.info.id).collect(),
            max_width: 8192,
            max_height: 8192,
        })
    }

    fn connector_info(&self, connector: ConnectorId) -> Result<ConnectorInfo> {
        self.connectors
            .iter()
            .find(|c| c.info.id == connector)
            .map(|c| {
                let mut info = c.info.clone();
                info.current_crtc = self.connector_binding(connector);
                info
            })
            .ok_or(DisplayError::NotFound {
                what: "connector",
                id: u64::from(connector.raw()),
            })
    }

    fn crtc_info(&self, crtc: CrtcId) -> Result<CrtcInfo> {
        let c = self
            .crtcs
            .iter()
            .find(|c| c.id == crtc)
            .ok_or(DisplayError::NotFound {
                what: "crtc",
                id: u64::from(crtc.raw()),
            })?;
        Ok(CrtcInfo {
            id: c.id,
            current_fb: None,
            current_mode: self.crtc_mode_now(crtc),
        })
    }

    fn plane_info(&self, plane: PlaneId) -> Result<PlaneInfo> {
        self.planes
            .iter()
            .find(|p| p.info.id == plane)
            .map(|p| {
                let mut info = p.info.clone();
                info.current_crtc = CrtcId::new(u32::try_from(p.state.crtc).unwrap_or(0));
                info
            })
            .ok_or(DisplayError::NotFound {
                what: "plane",
                id: u64::from(plane.raw()),
            })
    }

    fn property(&self, prop: PropId) -> Result<PropertyInfo> {
        self.prop_defs
            .iter()
            .find(|p| p.id == prop)
            .cloned()
            .ok_or(DisplayError::NotFound {
                what: "property",
                id: u64::from(prop.raw()),
            })
    }

    fn object_properties(&self, obj: AnyId) -> Result<ObjectProperties> {
        if !self.catalogs.contains_key(&obj) {
            return Err(DisplayError::NotFound {
                what: "object",
                id: u64::from(obj.raw()),
            });
        }
        Ok(catalog::object_properties(self, obj))
    }

    fn blob(&self, blob: BlobId) -> Result<Vec<u8>> {
        self.blobs
            .iter()
            .find(|b| b.id == blob)
            .map(|b| b.data.clone())
            .ok_or(DisplayError::NotFound {
                what: "blob",
                id: u64::from(blob.raw()),
            })
    }

    fn create_blob(&mut self, data: &[u8]) -> Result<Blob> {
        let id = self.create_blob_internal(data.to_vec());
        Ok(Blob {
            id,
            data: data.to_vec(),
        })
    }

    fn destroy_blob(&mut self, blob: BlobId) -> Result<()> {
        let before = self.blobs.len();
        self.blobs.retain(|b| b.id != blob);
        if self.blobs.len() == before {
            return Err(DisplayError::NotFound {
                what: "blob",
                id: u64::from(blob.raw()),
            });
        }
        Ok(())
    }

    fn add_fb(&mut self, spec: &FbSpec, modifier: Option<Modifier>) -> Result<FbId> {
        spec.validate()?;
        let id = FbId::new(self.next_fb).unwrap();
        self.next_fb += 1;
        self.fbs.push((id, spec.clone(), modifier));
        Ok(id)
    }

    fn import_gem(&mut self, fd: i32) -> Result<u32> {
        // The kernel's object-identity mirror: the same fd re-imports
        // to the same handle. A negative fd is the one rejection the
        // mock shares with the real ABI.
        if fd < 0 {
            return Err(DisplayError::System {
                errno: libc::EBADF,
                while_doing: "drmPrimeFDToHandle",
            });
        }
        if let Some(&handle) = self.gems.get(&fd) {
            return Ok(handle);
        }
        let handle = self.next_gem;
        self.next_gem += 1;
        self.gems.insert(fd, handle);
        Ok(handle)
    }

    fn rm_fb(&mut self, fb: FbId) -> Result<()> {
        let before = self.fbs.len();
        self.fbs.retain(|(id, _, _)| *id != fb);
        if self.fbs.len() == before {
            return Err(DisplayError::NotFound {
                what: "fb",
                id: u64::from(fb.raw()),
            });
        }
        Ok(())
    }

    fn commit(&mut self, request: &AtomicRequest) -> Result<CommitOutcome> {
        self.commit_request(request)
    }

    fn try_events(&mut self) -> Result<Vec<DeviceEvent>> {
        // Drain without advancing time: events already due at `now`.
        let mut out = std::mem::take(&mut self.queued);
        for crtc in &mut self.crtcs {
            if let Some(timeline) = &mut crtc.timeline {
                out.extend(timeline.advance_to(self.now, crtc.id));
            }
        }
        Ok(out)
    }
}

impl MockDevice {
    /// The plane class as carried by the immutable `type` property; this
    /// helper decodes it for callers that walk catalogs directly
    /// (round-trip proof for the property walk).
    #[must_use]
    pub fn plane_kind_of(&self, plane: PlaneId) -> Option<PlaneType> {
        let obj = AnyId::Plane(plane);
        let props = catalog::object_properties(self, obj);
        let type_prop = self
            .prop_defs
            .iter()
            .find(|p| p.name.as_str() == crate::props::prop::TYPE)?;
        let value = props.get(type_prop.id)?;
        Some(PlaneType::from_kernel(u32::try_from(value).unwrap_or(0)))
    }
}
