//! The real device backend — [`KmsBackend`] over libdrm.
//!
//! [`DrmBackend`] owns one DRM device node, the runtime-loaded
//! [`LibDrm`], a `Box`-pinned page-flip mailbox (the address handed to
//! the kernel as the commit's `user_data` — see [`sys`] for the safety
//! contract), and one heap-stable out-fence slot per CRTC (the kernel
//! writes the sync-file descriptor through the `OUT_FENCE_PTR` property
//! value, so the value we set is the slot's address, never a plain
//! integer).
//!
//! Translation policy:
//!
//! * Requests are *declarative*: property names resolve through the
//!   object's property walk, values pre-flight through
//!   [`crate::props`] (the same typed checks the mock applies), and
//!   blob payloads mint real kernel blobs.
//! * `OUT_FENCE_PTR` with value 1 means "give me a fence"; the backend
//!   substitutes the slot address, commits, reads the fd back, and
//!   returns it as [`OutFence::Fd`].
//! * `TEST_ONLY` mints blobs and destroys them after the commit, so
//!   probes leave no residue — the mock's contract, kept.
//! * Commit errors map errno to the typed taxonomy (`EBUSY` →
//!   [`RejectReason::Busy`], `ENOENT`/`EINVAL` → object/shape reasons
//!   with the pre-flight detail attached).
//!
//! Safety doctrine: the `drm` module tree root does **not** forbid
//! unsafe (a forbid cannot be lifted per child); the translation layer
//! (`mod.rs`, `raw.rs`) stays unsafe-free by construction, while every
//! `unsafe` in the tree lives in the audited [`sys`] module with
//! per-call SAFETY comments. Pointer *values* (the mailbox and
//! fence-slot addresses) cross the seam as plain integers — forming and
//! passing them is not an unsafe operation.

pub mod raw;
pub mod sys;

use std::collections::BTreeMap;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::path::Path;

use crate::atomic::{AtomicOp, AtomicRequest};
use crate::backend::{Blob, CommitOutcome, CrtcInfo, DeviceVersion, KmsBackend, Topology};
use crate::connector::ConnectorInfo;
use crate::error::{DisplayError, RejectReason, Result};
use crate::events::{DeviceEvent, OutFence, PageFlipEvent, PageFlipFlags};
use crate::fb::FbSpec;
use crate::ids::{AnyId, BlobId, ConnectorId, CrtcId, FbId, PlaneId, PropId};
use crate::plane::{InFormats, PlaneInfo, PlaneType};
use crate::props::{self, prop, PropValue, PropertyInfo, PropertyName};

use ldp_core::buffer::Modifier;
use ldp_core::time::Mono;

use raw::RawObjectPropsOwned;
use sys::{LibDrm, RawFlipEvent};

/// The libdrm-backed device.
pub struct DrmBackend {
    lib: LibDrm,
    fd: OwnedFd,
    /// Heap-stable flip-event sink (its address is the commit
    /// user_data). The `Box` is load-bearing: it pins the `Vec`
    /// *header* at a heap address that survives moves of the backend
    /// itself — a bare `Vec` field would move with the struct and
    /// dangle every pointer the kernel still holds.
    #[allow(clippy::box_collection)]
    mailbox: Box<Vec<RawFlipEvent>>,
    /// Per-CRTC out-fence slots; the kernel writes fds through these
    /// addresses. `Box` pins the payload; the map never removes entries.
    fence_slots: BTreeMap<u32, Box<u32>>,
}

impl core::fmt::Debug for DrmBackend {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // The lib handle, mailbox, and fence slots are opaque; the fd
        // number is the identity that matters.
        f.debug_struct("DrmBackend")
            .field("fd", &self.fd.as_raw_fd())
            .finish_non_exhaustive()
    }
}

impl DrmBackend {
    /// Load libdrm and open a device node (`/dev/dri/card0`).
    ///
    /// # Errors
    /// [`DisplayError::LibraryLoad`] without libdrm;
    /// [`DisplayError::MissingSymbol`] with a partial libdrm;
    /// [`DisplayError::DeviceOpen`] when the node is absent or
    /// inaccessible (the headless-machine outcome, errno preserved).
    pub fn open(path: &Path) -> Result<Self> {
        let lib = LibDrm::open()?;
        let fd = lib.open_device(path)?;
        Ok(Self {
            lib,
            fd,
            mailbox: Box::default(),
            fence_slots: BTreeMap::new(),
        })
    }

    /// Acquire DRM-Master rights — the privilege the modeset path
    /// (and its rehearsal) needs to change display state.
    ///
    /// # Errors
    /// [`DisplayError::System`] when the takeover fails (another
    /// master owns the device).
    pub fn set_master(&self) -> Result<()> {
        self.lib.set_master(self.fd.as_raw_fd())
    }

    /// Release DRM-Master rights.
    ///
    /// # Errors
    /// [`DisplayError::System`] on failure.
    pub fn drop_master(&self) -> Result<()> {
        self.lib.drop_master(self.fd.as_raw_fd())
    }

    /// Whether this device currently holds DRM-Master rights.
    ///
    /// # Errors
    /// Never in practice (the call cannot fail on a live fd).
    pub fn is_master(&self) -> Result<bool> {
        self.lib.is_master(self.fd.as_raw_fd())
    }

    /// Allocate a dumb buffer and return `(gem_handle, pitch)` — the
    /// classic scanout-capable CPU buffer the modeset rehearsal (and
    /// the dumb-buffer scanout service) builds framebuffers from.
    ///
    /// # Errors
    /// [`DisplayError::System`] when the kernel rejects the allocation.
    pub fn create_dumb(&self, width: u32, height: u32, bpp: u32) -> Result<(u32, u32)> {
        self.lib
            .create_dumb(self.fd.as_raw_fd(), width, height, bpp)
    }

    /// Destroy a dumb buffer by GEM handle.
    ///
    /// # Errors
    /// [`DisplayError::System`] when the kernel rejects the free.
    pub fn destroy_dumb(&self, handle: u32) -> Result<()> {
        self.lib.destroy_dumb(self.fd.as_raw_fd(), handle)
    }

    /// Map a dumb buffer for CPU access (`DRM_IOCTL_MODE_MAP_DUMB` +
    /// `mmap`): the scanout surface the serve loop composites into.
    /// The returned [`sys::DumbMapping`] owns the mapping and unmaps on
    /// drop — before the caller may destroy the buffer.
    ///
    /// # Errors
    /// [`DisplayError::System`] when the offset ioctl or the `mmap`
    /// fails.
    pub fn map_dumb(&self, handle: u32, size: u64) -> Result<sys::DumbMapping> {
        self.lib.map_dumb(self.fd.as_raw_fd(), handle, size)
    }

    /// The device descriptor (polling integrations).
    #[must_use]
    pub fn fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }

    /// One object's property walk, resolved to `(definition, value)` in
    /// walk order.
    fn walk(&self, obj: AnyId) -> Result<Vec<(PropertyInfo, u64)>> {
        let raw: RawObjectPropsOwned = self.lib.object_properties(
            self.fd.as_raw_fd(),
            obj.raw(),
            obj.object_type().kernel(),
        )?;
        if raw.prop_ids.len() != raw.prop_values.len() {
            return Err(DisplayError::System {
                errno: 0,
                while_doing: "object property walk",
            });
        }
        let mut out = Vec::with_capacity(raw.prop_ids.len());
        for (id, value) in raw.prop_ids.into_iter().zip(raw.prop_values) {
            let def = self.lib.property(self.fd.as_raw_fd(), id)?;
            out.push((raw::property_from_raw(&def), value));
        }
        Ok(out)
    }

    /// Find one property on one object by name (the shared resolver,
    /// so mock and libdrm agree on what "not found" means).
    fn find_on(&self, obj: AnyId, name: &PropertyName) -> Result<(PropId, PropertyInfo)> {
        let walk = self.walk(obj)?;
        let defs: Vec<PropertyInfo> = walk.iter().map(|(d, _)| d.clone()).collect();
        let found = props::find(&defs, name)?;
        Ok((found.id, found.clone()))
    }

    /// The value of a named property on an object.
    fn value_of(&self, obj: AnyId, name: &str) -> Result<Option<u64>> {
        let walk = self.walk(obj)?;
        Ok(walk
            .iter()
            .find(|(d, _)| d.name.as_str() == name)
            .map(|(_, v)| *v))
    }

    /// Read a named blob property's payload (`EDID`, `IN_FORMATS`).
    fn blob_of(&self, obj: AnyId, name: &str) -> Result<Option<Vec<u8>>> {
        match self.value_of(obj, name)? {
            None | Some(0) => Ok(None),
            Some(id) => Ok(Some(
                self.lib
                    .blob(self.fd.as_raw_fd(), u32::try_from(id).unwrap_or(0))?,
            )),
        }
    }

    /// The out-fence slot address for a CRTC, creating it on first use.
    fn fence_slot_addr(&mut self, crtc: u32) -> u64 {
        let slot = self.fence_slots.entry(crtc).or_insert_with(|| Box::new(0));
        // Box pins the u32; the map keeps it for the backend's lifetime.
        let ptr: *mut u32 = &mut **slot;
        ptr as u64
    }

    /// Translate and submit one commit.
    fn submit(&mut self, request: &AtomicRequest) -> Result<CommitOutcome> {
        request.validate_shape()?;
        let fd = self.fd.as_raw_fd();
        let test_only = request.flags.is_test_only();

        // Resolve every op against the live catalogs, pre-flighting the
        // same typed checks the mock applies.
        let mut resolved: Vec<(u32, u32, u64)> = Vec::with_capacity(request.ops.len());
        let mut minted: Vec<(AnyId, PropId, u32)> = Vec::new();
        let mut out_fence_crtcs: Vec<CrtcId> = Vec::new();
        for AtomicOp { obj, name, value } in &request.ops {
            let (prop_id, def) = self.find_on(*obj, name)?;
            props::check_value(&def, value)?;
            // OUT_FENCE_PTR is addressed by value 1 ("request a fence");
            // the kernel contract needs the slot address instead.
            if obj.object_type() == crate::ids::ObjectType::Crtc
                && name.as_str() == prop::OUT_FENCE_PTR
            {
                if let AnyId::Crtc(crtc) = obj {
                    let addr = self.fence_slot_addr(crtc.raw());
                    resolved.push((obj.raw(), prop_id.raw(), addr));
                    out_fence_crtcs.push(*crtc);
                }
                continue;
            }
            let wire = match value {
                PropValue::Blob(data) => {
                    // Mint now; destroyed after the commit either way.
                    let id = self.lib.create_blob(fd, data)?;
                    minted.push((*obj, prop_id, id));
                    u64::from(id)
                }
                other => other.wire_u64(),
            };
            resolved.push((obj.raw(), prop_id.raw(), wire));
        }

        // Build and submit the kernel request.
        let mut req = self.lib.atomic()?;
        for (object, property, value) in &resolved {
            req.add(*object, *property, *value)?;
        }
        let commit_result = req.commit(fd, request.flags.0, &mut self.mailbox);

        // The kernel either took blob refs (success) or did not (any
        // failure, including TEST_ONLY validation passes); dropping our
        // own minted reference is correct in every case.
        for (_, _, blob) in &minted {
            let _ = self.lib.destroy_blob(fd, *blob);
        }
        commit_result.map_err(map_commit_error)?;

        // Harvest out-fences the kernel wrote into our slots.
        let mut out_fences = Vec::new();
        for crtc in out_fence_crtcs {
            let fd_value = self.fence_slots.get(&crtc.raw()).map_or(0, |slot| **slot);
            if let Some(slot) = self.fence_slots.get_mut(&crtc.raw()) {
                **slot = 0;
            }
            if fd_value != 0 {
                let fence = LibDrm::owned_fd_from_kernel(i32::try_from(fd_value).unwrap_or(-1));
                out_fences.push((crtc, OutFence::Fd(fence)));
            }
        }
        Ok(CommitOutcome {
            applied: !test_only,
            out_fences,
        })
    }
}

impl KmsBackend for DrmBackend {
    fn version(&self) -> Result<DeviceVersion> {
        Ok(raw::version_from_raw(
            &self.lib.version(self.fd.as_raw_fd())?,
        ))
    }

    fn topology(&self) -> Result<Topology> {
        let fd = self.fd.as_raw_fd();
        let (crtcs, connectors, _encoders, _fbs, (max_width, max_height)) =
            self.lib.resources(fd)?;
        Ok(Topology {
            connectors: connectors
                .iter()
                .filter_map(|c| ConnectorId::new(*c))
                .collect(),
            crtcs: crtcs.iter().filter_map(|c| CrtcId::new(*c)).collect(),
            planes: self
                .lib
                .plane_ids(fd)?
                .iter()
                .filter_map(|p| PlaneId::new(*p))
                .collect(),
            max_width,
            max_height,
        })
    }

    fn connector_info(&self, connector: ConnectorId) -> Result<ConnectorInfo> {
        let fd = self.fd.as_raw_fd();
        let raw = self.lib.connector(fd, connector.raw())?;
        let mut info = raw::connector_from_raw(&raw);
        if let Some(binding) = self.value_of(AnyId::Connector(connector), prop::CRTC_ID)? {
            info.current_crtc = CrtcId::new(u32::try_from(binding).unwrap_or(0));
        }
        info.edid = self.blob_of(AnyId::Connector(connector), prop::EDID)?;
        Ok(info)
    }

    fn crtc_info(&self, crtc: CrtcId) -> Result<CrtcInfo> {
        let (buffer_id, mode) = self.lib.crtc(self.fd.as_raw_fd(), crtc.raw())?;
        Ok(CrtcInfo {
            id: crtc,
            current_fb: FbId::new(buffer_id),
            current_mode: mode.as_ref().map(raw::mode_from_raw),
        })
    }

    fn plane_info(&self, plane: PlaneId) -> Result<PlaneInfo> {
        let fd = self.fd.as_raw_fd();
        let raw = self.lib.plane(fd, plane.raw())?;
        let mut info = raw::plane_from_raw(&raw);
        // The class lives in the immutable `type` property.
        if let Some(kind) = self.value_of(AnyId::Plane(plane), prop::TYPE)? {
            info.kind = PlaneType::from_kernel(u32::try_from(kind).unwrap_or(0));
        }
        if let Some(blob) = self.blob_of(AnyId::Plane(plane), prop::IN_FORMATS)? {
            info.in_formats = Some(InFormats::parse(&blob)?);
        }
        Ok(info)
    }

    fn property(&self, prop: PropId) -> Result<PropertyInfo> {
        Ok(raw::property_from_raw(
            &self.lib.property(self.fd.as_raw_fd(), prop.raw())?,
        ))
    }

    fn object_properties(&self, obj: AnyId) -> Result<props::ObjectProperties> {
        let walk = self.walk(obj)?;
        let mut out = props::ObjectProperties::default();
        for (def, value) in walk {
            out = out.with(def.id, value);
        }
        Ok(out)
    }

    fn blob(&self, blob: BlobId) -> Result<Vec<u8>> {
        self.lib.blob(self.fd.as_raw_fd(), blob.raw())
    }

    fn create_blob(&mut self, data: &[u8]) -> Result<Blob> {
        let fd = self.fd.as_raw_fd();
        let id = self.lib.create_blob(fd, data)?;
        Ok(Blob {
            id: BlobId::new(id).ok_or(DisplayError::System {
                errno: 0,
                while_doing: "blob id zero",
            })?,
            data: data.to_vec(),
        })
    }

    fn destroy_blob(&mut self, blob: BlobId) -> Result<()> {
        self.lib.destroy_blob(self.fd.as_raw_fd(), blob.raw())
    }

    fn add_fb(&mut self, spec: &FbSpec, modifier: Option<Modifier>) -> Result<FbId> {
        spec.validate()?;
        let fd = self.fd.as_raw_fd();
        // Absent modifier means the AddFB2 contract: linear layout.
        let modifier = modifier.unwrap_or(Modifier::LINEAR);
        let id = self.lib.add_fb2(
            fd,
            spec.width,
            spec.height,
            spec.format.code(),
            &spec.handles,
            &spec.pitches,
            &spec.offsets,
            modifier.code(),
        )?;
        FbId::new(id).ok_or(DisplayError::System {
            errno: 0,
            while_doing: "fb id zero",
        })
    }

    fn import_gem(&mut self, fd: i32) -> Result<u32> {
        self.lib.prime_fd_to_handle(self.fd.as_raw_fd(), fd)
    }

    fn rm_fb(&mut self, fb: FbId) -> Result<()> {
        self.lib.rm_fb(self.fd.as_raw_fd(), fb.raw())
    }

    fn commit(&mut self, request: &AtomicRequest) -> Result<CommitOutcome> {
        self.submit(request)
    }

    fn try_events(&mut self) -> Result<Vec<DeviceEvent>> {
        let fd = self.fd.as_raw_fd();
        self.lib.handle_events(fd)?;
        let flips = std::mem::take(&mut *self.mailbox);
        let mut out = Vec::with_capacity(flips.len());
        for flip in flips {
            let timestamp = Mono::from_ns(
                u64::from(flip.tv_sec) * 1_000_000_000 + u64::from(flip.tv_usec) * 1_000,
            );
            out.push(DeviceEvent::PageFlip(PageFlipEvent {
                crtc: CrtcId::new(flip.crtc_id).ok_or(DisplayError::System {
                    errno: 0,
                    while_doing: "flip event with crtc id zero",
                })?,
                sequence: u64::from(flip.sequence),
                timestamp,
                flags: PageFlipFlags::NONE,
                // Real out-fences are delivered in the CommitOutcome at
                // commit time; the event itself needs no fence handle.
                out_fence: None,
            }));
        }
        Ok(out)
    }
}

/// Map a commit failure to the typed taxonomy where the errno carries
/// more information than the pre-flight already did.
fn map_commit_error(err: DisplayError) -> DisplayError {
    let DisplayError::System { errno, while_doing } = &err else {
        return err;
    };
    match *errno {
        16 => DisplayError::AtomicReject {
            reason: RejectReason::Busy,
            detail: "kernel reports a flip still pending".into(),
        },
        2 => DisplayError::AtomicReject {
            reason: RejectReason::UnknownObject,
            detail: format!("commit addressed a missing object ({while_doing})"),
        },
        22 => DisplayError::AtomicReject {
            reason: RejectReason::Malformed,
            detail: format!("kernel rejected the request shape ({while_doing})"),
        },
        _ => err,
    }
}

#[cfg(test)]
mod master_tests {
    /// The published numbers: `_IOWR('d', 0xb2, 32)`, `_IOWR('d', 0xb4, 4)`,
    /// and `_IOWR('d', 0xb9, 16)` — the kernel's drm_mode_create_dumb /
    /// drm_mode_destroy_dumb / drm_mode_map_dumb requests.
    #[test]
    fn dumb_ioctl_numbers_match_the_published_abi() {
        use super::sys::{iowr, IOCTL_CREATE_DUMB, IOCTL_DESTROY_DUMB, IOCTL_MAP_DUMB};
        assert_eq!(IOCTL_CREATE_DUMB, 0xC020_64B2);
        assert_eq!(IOCTL_DESTROY_DUMB, 0xC004_64B4);
        assert_eq!(IOCTL_MAP_DUMB, 0xC010_64B9);
        // And the encoder from first principles agrees.
        assert_eq!(iowr(0xb2, 32), 0xC020_64B2);
        assert_eq!(iowr(0xb4, 4), 0xC004_64B4);
        assert_eq!(iowr(0xb9, 16), 0xC010_64B9);
    }
}
