//! The libdrm runtime layer — dlopen, symbols, safe wrappers.
//!
//! This is the crate's **audit boundary**: every `unsafe` in the display
//! crate lives here, each call site carrying a `SAFETY` comment
//! (CONTRIBUTING rule 3; the `ldp-transport` sys precedent). Everything
//! the rest of the crate sees is safe, owned-data Rust.
//!
//! Loading is lazy and failure-tolerant by design: `LibDrm::open()`
//! resolves *all* required symbols up front or reports the exact
//! missing one, so a machine with a partial libdrm fails loudly at
//! startup instead of UB at first modeset. Devices are opened by path;
//! on headless machines (no `/dev/dri`) the open fails with the raw
//! errno wrapped in [`DisplayError::DeviceOpen`] — a legitimate,
//! tested outcome.
//!
//! ABI notes: the `repr(C)` structs declare the *stable prefixes* of
//! libdrm's heap objects (newer libdrm appends fields at the end); sizes
//! are pinned with `const` assertions. libdrm allocates and frees these
//! objects itself through `drmModeFree*`, so prefix reads cannot
//! misread.

use std::ffi::{c_char, c_int, c_uint, c_void, CStr};
use std::os::fd::{FromRawFd, OwnedFd, RawFd};
use std::ptr::NonNull;

use crate::error::{DisplayError, Result};

use super::raw::{
    RawConnectorOwned, RawDrmDevice, RawMode, RawObjectPropsOwned, RawPlaneOwned, RawPropertyOwned,
    RawVersionOwned,
};

/// SONAME.
const LIBDRM: &str = "libdrm.so.2";

/// The `drmModeGetResources` result, unpacked: CRTC, connector, and
/// encoder ids in resource-list order, framebuffer ids, and the
/// framebuffer size limits.
pub type ResourceLists = (Vec<u32>, Vec<u32>, Vec<u32>, Vec<u32>, (u32, u32));

/// One decoded page-flip event (raw fields, pre-translation).
#[derive(Clone, Copy, Debug)]
pub struct RawFlipEvent {
    /// The CRTC that flipped.
    pub crtc_id: u32,
    /// Per-CRTC sequence.
    pub sequence: c_uint,
    /// Timestamp seconds.
    pub tv_sec: c_uint,
    /// Timestamp microseconds.
    pub tv_usec: c_uint,
}

// ---------------------------------------------------------------------------
// repr(C) ABI prefixes. Sizes pinned.
// ---------------------------------------------------------------------------

#[repr(C)]
struct RawVersion {
    major: c_int,
    minor: c_int,
    patch: c_int,
    name_len: usize,
    name: *mut c_char,
    date_len: usize,
    date: *mut c_char,
    desc_len: usize,
    desc: *mut c_char,
}
const _: () = assert!(size_of::<RawVersion>() == 64);

#[repr(C)]
struct RawModeRes {
    count_fbs: c_int,
    _pad0: u32,
    fbs: *mut c_uint,
    count_crtcs: c_int,
    _pad1: u32,
    crtcs: *mut c_uint,
    count_connectors: c_int,
    _pad2: u32,
    connectors: *mut c_uint,
    count_encoders: c_int,
    _pad3: u32,
    encoders: *mut c_uint,
    min_width: c_uint,
    max_width: c_uint,
    min_height: c_uint,
    max_height: c_uint,
}
const _: () = assert!(size_of::<RawModeRes>() == 80);

#[repr(C)]
struct RawConnector {
    connector_id: c_uint,
    encoder_id: c_uint,
    connector_type: c_uint,
    connector_type_id: c_uint,
    connection: c_uint,
    mm_width: c_uint,
    mm_height: c_uint,
    subpixel: c_uint,
    count_modes: c_int,
    _pad0: u32,
    modes: *mut RawMode,
    count_props: c_int,
    _pad1: u32,
    props: *mut c_uint,
    prop_values: *mut u64,
    count_encoders: c_int,
    _pad2: u32,
    encoders: *mut c_uint,
}
const _: () = assert!(size_of::<RawConnector>() == 88);

#[repr(C)]
struct RawCrtc {
    crtc_id: c_uint,
    buffer_id: c_uint,
    x: c_uint,
    y: c_uint,
    width: c_uint,
    height: c_uint,
    mode_valid: c_int,
    mode: RawMode,
    gamma_size: c_int,
    // Newer libdrm appends props_count/props beyond this prefix.
}
const _: () = assert!(size_of::<RawCrtc>() == 100);

#[repr(C)]
struct RawPlane {
    count_formats: c_int,
    _pad0: u32,
    formats: *mut c_uint,
    plane_id: c_uint,
    crtc_id: c_uint,
    fb_id: c_uint,
    crtc_x: c_uint,
    crtc_y: c_uint,
    x: c_uint,
    y: c_uint,
    possible_crtcs: c_uint,
    gamma_size: c_uint,
    // Newer libdrm may append modifier fields beyond this prefix.
}
const _: () = assert!(size_of::<RawPlane>() == 56);

#[repr(C)]
struct RawPropEnum {
    value: u64,
    name: [u8; 32],
}
const _: () = assert!(size_of::<RawPropEnum>() == 40);

#[repr(C)]
struct RawProperty {
    prop_id: c_uint,
    flags: c_uint,
    name: [u8; 32],
    count_values: c_int,
    _pad0: u32,
    values: *mut u64,
    count_enum_blobs: c_int,
    _pad1: u32,
    enum_blobs: *mut RawPropEnum,
}
const _: () = assert!(size_of::<RawProperty>() == 72);

#[repr(C)]
struct RawObjectProps {
    count_props: c_uint,
    _pad0: u32,
    props: *mut c_uint,
    prop_values: *mut u64,
}
const _: () = assert!(size_of::<RawObjectProps>() == 24);

#[repr(C)]
struct RawPropertyBlob {
    id: c_uint,
    length: c_uint,
    data: *mut c_void,
}
const _: () = assert!(size_of::<RawPropertyBlob>() == 16);

#[repr(C)]
struct RawDrmModePlaneRes {
    count_planes: c_int,
    _pad0: u32,
    planes: *mut c_uint,
}
const _: () = assert!(size_of::<RawDrmModePlaneRes>() == 16);

#[repr(C)]
struct RawDrmDeviceAbi {
    nodes: *mut *mut c_char,
    available_nodes: c_int,
    bustype: c_int,
    businfo: *mut c_void,
    deviceinfo: *mut c_void,
}
const _: () = assert!(size_of::<RawDrmDeviceAbi>() == 32);

#[repr(C)]
struct RawEventContext {
    version: c_int,
    _pad0: u32,
    vblank_handler: *const c_void,
    page_flip_handler: *const c_void,
    page_flip_handler2:
        Option<unsafe extern "C" fn(c_int, c_uint, c_uint, c_uint, c_uint, *mut c_void)>,
}
const _: () = assert!(size_of::<RawEventContext>() == 32);

/// The page-flip handler (v2: carries `crtc_id`).
///
/// The `user_data` in each event is whatever pointer was supplied to
/// `drmModeAtomicCommit` for the flip that matured — for this backend,
/// always the address of the backend-owned event mailbox (a heap-stable
/// `Box<Vec<RawFlipEvent>>`); see [`LibDrm::handle_events`].
///
/// Kernel field names are kept verbatim (`tv_sec`/`tv_usec`); the
/// similarity is the ABI's, not a style choice.
#[allow(clippy::similar_names)]
unsafe extern "C" fn flip_handler2(
    _fd: c_int,
    sequence: c_uint,
    tv_sec: c_uint,
    tv_usec: c_uint,
    crtc_id: c_uint,
    user_data: *mut c_void,
) {
    // SAFETY: the contract above guarantees user_data points at a live
    // `Vec<RawFlipEvent>` for as long as committed flips are pending —
    // the backend's mailbox outlives every flip it commits. The kernel
    // calls handlers only inside drmHandleEvent, single-threaded.
    let sink = unsafe { &mut *user_data.cast::<Vec<RawFlipEvent>>() };
    sink.push(RawFlipEvent {
        crtc_id,
        sequence,
        tv_sec,
        tv_usec,
    });
}

// ---------------------------------------------------------------------------
// Symbol table.
// ---------------------------------------------------------------------------

/// `DRM_IOCTL_MODE_CREATE_DUMB` payload (`drm_mode_create_dumb`).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RawCreateDumb {
    /// Requested height.
    pub height: u32,
    /// Requested width.
    pub width: u32,
    /// Bits per pixel.
    pub bpp: u32,
    /// Flags (0 for the classic dumb buffer).
    pub flags: u32,
    /// The kernel's GEM handle (out).
    pub handle: u32,
    /// The kernel's row pitch (out).
    pub pitch: u32,
    /// The buffer's byte size (out).
    pub size: u64,
}

/// `DRM_IOCTL_MODE_DESTROY_DUMB` payload.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RawDestroyDumb {
    /// The GEM handle to destroy.
    pub handle: u32,
}

/// `DRM_IOCTL_MODE_MAP_DUMB` payload (`drm_mode_map_dumb`): the
/// ioctl that trades a GEM handle for the mmap offset of its backing
/// storage.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RawMapDumb {
    /// The GEM handle to map.
    pub handle: u32,
    /// Struct padding (explicit, as the kernel header declares it).
    pub pad: u32,
    /// The kernel's fake mmap offset (out).
    pub offset: u64,
}
const _: () = assert!(size_of::<RawMapDumb>() == 16);

/// `_IOWR('d', nr, size)` — the ioctl request encoder, computed from
/// first principles (pinned by a unit test against the published
/// dumb-buffer numbers).
#[must_use]
pub const fn iowr(nr: u8, size: u32) -> libc::c_ulong {
    // Linux _IOC layout: dir<<30 | size<<16 | type<<8 | nr.
    (3u64 << 30)
        | ((size as u64) << 16)
        | (0x64 << 8) // 'd'
        | nr as u64
}

/// `DRM_IOCTL_MODE_CREATE_DUMB` (`_IOWR('d', 0xb2, 32)`).
pub const IOCTL_CREATE_DUMB: libc::c_ulong = iowr(0xb2, 32);
/// `DRM_IOCTL_MODE_DESTROY_DUMB` (`_IOWR('d', 0xb4, 4)`).
pub const IOCTL_DESTROY_DUMB: libc::c_ulong = iowr(0xb4, 4);
/// `DRM_IOCTL_MODE_MAP_DUMB` (`_IOWR('d', 0xb9, 16)`).
pub const IOCTL_MAP_DUMB: libc::c_ulong = iowr(0xb9, 16);

macro_rules! drm_fns {
    ($( $field:ident : $sig:ty = $sym:literal ),+ $(,)?) => {
        /// The resolved function table.
        #[derive(Clone, Copy)]
        struct DrmFns {
            $( pub $field: $sig ),+
        }
        impl DrmFns {
            /// Resolve every symbol from `handle` or fail naming it.
            ///
            /// # Safety
            /// `handle` must be a live `dlopen` handle. Each `$sig` must
            /// exactly match the C prototype of `$sym` — they are the
            /// libdrm public ABI, stable by SONAME contract.
            unsafe fn resolve(handle: *mut c_void) -> Result<Self> {
                let missing =
                    |symbol: &'static str| DisplayError::MissingSymbol { library: LIBDRM, symbol };
                unsafe {
                    Ok(Self {
                        $( $field: {
                            let ptr = libc::dlsym(handle, concat!($sym, "\0").as_ptr().cast::<c_char>());
                            if ptr.is_null() {
                                return Err(missing($sym));
                            }
                            std::mem::transmute::<*mut c_void, $sig>(ptr)
                        } ),+
                    })
                }
            }
        }
        /// The symbol names, in table order (diagnostics and tests).
        const SYMBOLS: &[&str] = &[ $( $sym ),+ ];
    };
}

drm_fns! {
    get_version: unsafe extern "C" fn(c_int) -> *mut RawVersion = "drmGetVersion",
    free_version: unsafe extern "C" fn(*mut RawVersion) = "drmFreeVersion",
    get_resources: unsafe extern "C" fn(c_int) -> *mut RawModeRes = "drmModeGetResources",
    free_resources: unsafe extern "C" fn(*mut RawModeRes) = "drmModeFreeResources",
    get_connector: unsafe extern "C" fn(c_int, c_uint) -> *mut RawConnector = "drmModeGetConnector",
    free_connector: unsafe extern "C" fn(*mut RawConnector) = "drmModeFreeConnector",
    get_crtc: unsafe extern "C" fn(c_int, c_uint) -> *mut RawCrtc = "drmModeGetCrtc",
    free_crtc: unsafe extern "C" fn(*mut RawCrtc) = "drmModeFreeCrtc",
    get_plane_res: unsafe extern "C" fn(c_int) -> *mut RawDrmModePlaneRes = "drmModeGetPlaneResources",
    free_plane_res: unsafe extern "C" fn(*mut RawDrmModePlaneRes) = "drmModeFreePlaneResources",
    get_plane: unsafe extern "C" fn(c_int, c_uint) -> *mut RawPlane = "drmModeGetPlane",
    free_plane: unsafe extern "C" fn(*mut RawPlane) = "drmModeFreePlane",
    get_property: unsafe extern "C" fn(c_int, c_uint) -> *mut RawProperty = "drmModeGetProperty",
    free_property: unsafe extern "C" fn(*mut RawProperty) = "drmModeFreeProperty",
    get_obj_props: unsafe extern "C" fn(c_int, c_uint, c_uint) -> *mut RawObjectProps = "drmModeObjectGetProperties",
    free_obj_props: unsafe extern "C" fn(*mut RawObjectProps) = "drmModeFreeObjectProperties",
    get_blob: unsafe extern "C" fn(c_int, c_uint) -> *mut RawPropertyBlob = "drmModeGetPropertyBlob",
    free_blob: unsafe extern "C" fn(*mut RawPropertyBlob) = "drmModeFreePropertyBlob",
    create_blob: unsafe extern "C" fn(c_int, *const c_void, usize, *mut c_uint) -> c_int = "drmModeCreatePropertyBlob",
    destroy_blob: unsafe extern "C" fn(c_int, c_uint) -> c_int = "drmModeDestroyPropertyBlob",
    atomic_alloc: unsafe extern "C" fn() -> *mut c_void = "drmModeAtomicAlloc",
    atomic_free: unsafe extern "C" fn(*mut c_void) = "drmModeAtomicFree",
    atomic_add: unsafe extern "C" fn(*mut c_void, c_uint, c_uint, u64) -> c_int = "drmModeAtomicAddProperty",
    atomic_commit: unsafe extern "C" fn(c_int, *mut c_void, c_uint, *mut c_void) -> c_int = "drmModeAtomicCommit",
    add_fb2_mod: unsafe extern "C" fn(
        c_int, c_uint, c_uint, c_uint, *const c_uint, *const c_uint, *const c_uint,
        *const u64, c_uint, *mut c_uint, c_uint,
    ) -> c_int = "drmModeAddFB2WithModifiers",
    rm_fb: unsafe extern "C" fn(c_int, c_uint) -> c_int = "drmModeRmFB",
    handle_event: unsafe extern "C" fn(c_int, *mut RawEventContext) -> c_int = "drmHandleEvent",
    is_master: unsafe extern "C" fn(c_int) -> c_int = "drmIsMaster",
    set_master: unsafe extern "C" fn(c_int) -> c_int = "drmSetMaster",
    drop_master: unsafe extern "C" fn(c_int) -> c_int = "drmDropMaster",
    prime_fd_to_handle: unsafe extern "C" fn(c_int, c_int, *mut u32) -> c_int =
        "drmPrimeFDToHandle",
    drm_ioctl: unsafe extern "C" fn(c_int, libc::c_ulong, *mut c_void) -> c_int =
        "drmIoctl",
    get_devices2: unsafe extern "C" fn(c_uint, *mut *mut RawDrmDeviceAbi, c_int) -> c_int = "drmGetDevices2",
    free_devices: unsafe extern "C" fn(*mut *mut RawDrmDeviceAbi, c_int) = "drmFreeDevices",
}

/// The loaded library.
pub struct LibDrm {
    handle: *mut c_void,
    fns: DrmFns,
}

// The handle and table are plain pointers; libdl handles are usable
// from any thread (refcounted internally by the loader).
unsafe impl Send for LibDrm {}
unsafe impl Sync for LibDrm {}

impl core::fmt::Debug for LibDrm {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // The handle and table are opaque pointers; the name is the
        // identity that matters in a failure report.
        f.debug_struct("LibDrm")
            .field("library", &LIBDRM)
            .finish_non_exhaustive()
    }
}

impl LibDrm {
    /// `dlopen("libdrm.so.2")` and resolve the ABI.
    ///
    /// # Errors
    /// [`DisplayError::LibraryLoad`] when the library is absent;
    /// [`DisplayError::MissingSymbol`] naming the first gap.
    pub fn open() -> Result<Self> {
        Self::open_path(c"libdrm.so.2")
    }

    /// Open a specific path (tests).
    ///
    /// # Errors
    /// [`DisplayError::LibraryLoad`] when the path does not resolve to
    /// a library; [`DisplayError::MissingSymbol`] naming the first
    /// unresolved symbol.
    pub fn open_path(name: &CStr) -> Result<Self> {
        // SAFETY: `name` is NUL-terminated; dlopen only retains it for
        // the duration of the call.
        let handle = unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
        if handle.is_null() {
            return Err(DisplayError::LibraryLoad { library: LIBDRM });
        }
        // SAFETY: handle is live; the transmuted prototypes match the
        // published libdrm ABI (see the macro's Safety contract).
        let fns = unsafe { DrmFns::resolve(handle)? };
        Ok(Self { handle, fns })
    }

    /// Number of symbols in the table (test introspection).
    #[must_use]
    pub fn symbol_count() -> usize {
        SYMBOLS.len()
    }

    /// Open a DRM device node by path.
    ///
    /// # Errors
    /// [`DisplayError::DeviceOpen`] with the raw errno (ENOENT on
    /// headless machines, EACCES/EPERM without DRM-Master rights).
    pub fn open_device(&self, path: &std::path::Path) -> Result<OwnedFd> {
        use std::os::unix::ffi::OsStrExt;
        // SAFETY: path bytes are NUL-appended locally; open() copies
        // what it needs before returning.
        let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| {
            DisplayError::DeviceOpen {
                path: path.to_owned(),
                errno: 22,
            }
        })?;
        let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
        if fd < 0 {
            return Err(DisplayError::DeviceOpen {
                path: path.to_owned(),
                errno: std::io::Error::last_os_error().raw_os_error().unwrap_or(-1),
            });
        }
        // SAFETY: fd is a freshly opened, owned descriptor.
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    /// Driver identity for a device.
    ///
    /// # Errors
    /// [`DisplayError::System`] when `drmGetVersion` fails.
    pub fn version(&self, fd: RawFd) -> Result<RawVersionOwned> {
        // SAFETY: fd is a live device descriptor; the returned pointer
        // is freed below with its matching drmFreeVersion.
        let raw = unsafe { (self.fns.get_version)(fd) };
        if raw.is_null() {
            return Err(sys_error("drmGetVersion"));
        }
        let owned = unsafe {
            let take = |len: usize, ptr: *mut c_char| -> String {
                if ptr.is_null() || len == 0 {
                    return String::new();
                }
                CStr::from_ptr(ptr).to_string_lossy().into_owned()
            };
            let out = RawVersionOwned {
                name: take((*raw).name_len, (*raw).name),
                date: take((*raw).date_len, (*raw).date),
                desc: take((*raw).desc_len, (*raw).desc),
                major: (*raw).major,
                minor: (*raw).minor,
                patch: (*raw).patch,
            };
            (self.fns.free_version)(raw);
            out
        };
        Ok(owned)
    }

    /// Resource ids: (crtcs, connectors, encoders, fbs, max dims).
    ///
    /// # Errors
    /// [`DisplayError::System`] when `drmModeGetResources` fails.
    pub fn resources(&self, fd: RawFd) -> Result<ResourceLists> {
        // SAFETY: fd live; result freed with drmModeFreeResources; the
        // pointer arrays are copied before the free.
        unsafe {
            let res = (self.fns.get_resources)(fd);
            if res.is_null() {
                return Err(sys_error("drmModeGetResources"));
            }
            let take_u32 = |count: c_int, ptr: *mut c_uint| -> Vec<u32> {
                if count <= 0 || ptr.is_null() {
                    Vec::new()
                } else {
                    std::slice::from_raw_parts(ptr, count as usize).to_vec()
                }
            };
            let out = (
                take_u32((*res).count_crtcs, (*res).crtcs),
                take_u32((*res).count_connectors, (*res).connectors),
                take_u32((*res).count_encoders, (*res).encoders),
                take_u32((*res).count_fbs, (*res).fbs),
                ((*res).max_width, (*res).max_height),
            );
            (self.fns.free_resources)(res);
            Ok(out)
        }
    }

    /// One connector snapshot.
    ///
    /// # Errors
    /// [`DisplayError::System`] when `drmModeGetConnector` fails.
    pub fn connector(&self, fd: RawFd, connector_id: u32) -> Result<RawConnectorOwned> {
        // SAFETY: fd live and connector_id from the resource walk; the
        // result is freed with drmModeFreeConnector after copying.
        unsafe {
            let raw = (self.fns.get_connector)(fd, connector_id);
            if raw.is_null() {
                return Err(sys_error("drmModeGetConnector"));
            }
            let take_u32 = |count: c_int, ptr: *mut c_uint| -> Vec<u32> {
                if count <= 0 || ptr.is_null() {
                    Vec::new()
                } else {
                    std::slice::from_raw_parts(ptr, count as usize).to_vec()
                }
            };
            let modes = if (*raw).count_modes <= 0 || (*raw).modes.is_null() {
                Vec::new()
            } else {
                std::slice::from_raw_parts((*raw).modes, (*raw).count_modes as usize).to_vec()
            };
            let prop_values = if (*raw).count_props <= 0 || (*raw).prop_values.is_null() {
                Vec::new()
            } else {
                std::slice::from_raw_parts((*raw).prop_values, (*raw).count_props as usize).to_vec()
            };
            let out = RawConnectorOwned {
                connector_id: (*raw).connector_id,
                encoder_id: (*raw).encoder_id,
                connector_type: (*raw).connector_type,
                connector_type_id: (*raw).connector_type_id,
                connection: (*raw).connection,
                mm_width: (*raw).mm_width,
                mm_height: (*raw).mm_height,
                subpixel: (*raw).subpixel,
                modes,
                prop_ids: take_u32((*raw).count_props, (*raw).props),
                prop_values,
                encoders: take_u32((*raw).count_encoders, (*raw).encoders),
            };
            (self.fns.free_connector)(raw);
            Ok(out)
        }
    }

    /// One CRTC snapshot: (buffer_id, mode).
    ///
    /// # Errors
    /// [`DisplayError::System`] when `drmModeGetCrtc` fails.
    pub fn crtc(&self, fd: RawFd, crtc_id: u32) -> Result<(u32, Option<RawMode>)> {
        // SAFETY: fd live; result freed with drmModeFreeCrtc after copy.
        unsafe {
            let raw = (self.fns.get_crtc)(fd, crtc_id);
            if raw.is_null() {
                return Err(sys_error("drmModeGetCrtc"));
            }
            let mode = if (*raw).mode_valid != 0 {
                Some((*raw).mode)
            } else {
                None
            };
            let out = ((*raw).buffer_id, mode);
            (self.fns.free_crtc)(raw);
            Ok(out)
        }
    }

    /// All plane ids.
    ///
    /// # Errors
    /// [`DisplayError::System`] when `drmModeGetPlaneResources` fails.
    pub fn plane_ids(&self, fd: RawFd) -> Result<Vec<u32>> {
        // SAFETY: fd live; result freed with drmModeFreePlaneResources.
        unsafe {
            let res = (self.fns.get_plane_res)(fd);
            if res.is_null() {
                return Err(sys_error("drmModeGetPlaneResources"));
            }
            let out = if (*res).count_planes <= 0 || (*res).planes.is_null() {
                Vec::new()
            } else {
                std::slice::from_raw_parts((*res).planes, (*res).count_planes as usize).to_vec()
            };
            (self.fns.free_plane_res)(res);
            Ok(out)
        }
    }

    /// One plane snapshot.
    ///
    /// # Errors
    /// [`DisplayError::System`] when `drmModeGetPlane` fails.
    pub fn plane(&self, fd: RawFd, plane_id: u32) -> Result<RawPlaneOwned> {
        // SAFETY: fd live; result freed with drmModeFreePlane after copy.
        unsafe {
            let raw = (self.fns.get_plane)(fd, plane_id);
            if raw.is_null() {
                return Err(sys_error("drmModeGetPlane"));
            }
            let formats = if (*raw).count_formats <= 0 || (*raw).formats.is_null() {
                Vec::new()
            } else {
                std::slice::from_raw_parts((*raw).formats, (*raw).count_formats as usize).to_vec()
            };
            let out = RawPlaneOwned {
                plane_id: (*raw).plane_id,
                crtc_id: (*raw).crtc_id,
                fb_id: (*raw).fb_id,
                possible_crtcs: (*raw).possible_crtcs,
                formats,
            };
            (self.fns.free_plane)(raw);
            Ok(out)
        }
    }

    /// One property definition.
    ///
    /// # Errors
    /// [`DisplayError::System`] when `drmModeGetProperty` fails.
    pub fn property(&self, fd: RawFd, prop_id: u32) -> Result<RawPropertyOwned> {
        // SAFETY: fd live; result freed with drmModeFreeProperty.
        unsafe {
            let raw = (self.fns.get_property)(fd, prop_id);
            if raw.is_null() {
                return Err(sys_error("drmModeGetProperty"));
            }
            // One explicit reference to the libdrm object, formed inside
            // this unsafe block; every read below goes through it.
            let prop: &RawProperty = &*raw;
            let name_len = prop.name.iter().position(|&c| c == 0).unwrap_or(32);
            let values = if prop.count_values <= 0 || prop.values.is_null() {
                Vec::new()
            } else {
                std::slice::from_raw_parts(prop.values, prop.count_values as usize).to_vec()
            };
            let enum_blobs = if prop.count_enum_blobs <= 0 || prop.enum_blobs.is_null() {
                Vec::new()
            } else {
                std::slice::from_raw_parts(prop.enum_blobs, prop.count_enum_blobs as usize)
                    .iter()
                    .map(|e| {
                        let n = e.name.iter().position(|&c| c == 0).unwrap_or(32);
                        (e.value, String::from_utf8_lossy(&e.name[..n]).into_owned())
                    })
                    .collect()
            };
            let out = RawPropertyOwned {
                prop_id: prop.prop_id,
                flags: prop.flags,
                name: String::from_utf8_lossy(&prop.name[..name_len]).into_owned(),
                values,
                enum_blobs,
            };
            (self.fns.free_property)(raw);
            Ok(out)
        }
    }

    /// One object's (property id, value) pairs.
    ///
    /// # Errors
    /// [`DisplayError::System`] when `drmModeObjectGetProperties` fails.
    pub fn object_properties(
        &self,
        fd: RawFd,
        object_id: u32,
        object_type: u32,
    ) -> Result<RawObjectPropsOwned> {
        // SAFETY: fd live; result freed with drmModeFreeObjectProperties.
        unsafe {
            let raw = (self.fns.get_obj_props)(fd, object_id, object_type);
            if raw.is_null() {
                return Err(sys_error("drmModeObjectGetProperties"));
            }
            let out = if (*raw).count_props == 0 {
                RawObjectPropsOwned::default()
            } else {
                RawObjectPropsOwned {
                    prop_ids: std::slice::from_raw_parts((*raw).props, (*raw).count_props as usize)
                        .to_vec(),
                    prop_values: std::slice::from_raw_parts(
                        (*raw).prop_values,
                        (*raw).count_props as usize,
                    )
                    .to_vec(),
                }
            };
            (self.fns.free_obj_props)(raw);
            Ok(out)
        }
    }

    /// One blob's payload.
    ///
    /// # Errors
    /// [`DisplayError::NotFound`] when the blob id does not resolve.
    pub fn blob(&self, fd: RawFd, blob_id: u32) -> Result<Vec<u8>> {
        // SAFETY: fd live; result freed with drmModeFreePropertyBlob.
        unsafe {
            let raw = (self.fns.get_blob)(fd, blob_id);
            if raw.is_null() {
                return Err(DisplayError::NotFound {
                    what: "blob",
                    id: u64::from(blob_id),
                });
            }
            let data = if (*raw).length == 0 || (*raw).data.is_null() {
                Vec::new()
            } else {
                std::slice::from_raw_parts((*raw).data.cast::<u8>(), (*raw).length as usize)
                    .to_vec()
            };
            (self.fns.free_blob)(raw);
            Ok(data)
        }
    }

    /// Create a property blob, returning its id.
    ///
    /// # Errors
    /// [`DisplayError::System`] (negative errno) when the allocation
    /// ioctl fails.
    pub fn create_blob(&self, fd: RawFd, data: &[u8]) -> Result<u32> {
        let mut id: c_uint = 0;
        // SAFETY: fd live; data pointer + length describe a valid borrow
        // for the call; id is a valid out-pointer.
        let rc = unsafe {
            (self.fns.create_blob)(fd, data.as_ptr().cast::<c_void>(), data.len(), &mut id)
        };
        check_rc(rc, "drmModeCreatePropertyBlob")?;
        Ok(id)
    }

    /// Destroy a property blob.
    ///
    /// # Errors
    /// [`DisplayError::System`] (negative errno) when the destroy
    /// ioctl fails.
    pub fn destroy_blob(&self, fd: RawFd, blob_id: u32) -> Result<()> {
        // SAFETY: fd live; blob_id was minted by this device.
        let rc = unsafe { (self.fns.destroy_blob)(fd, blob_id) };
        check_rc(rc, "drmModeDestroyPropertyBlob")
    }

    /// Register a framebuffer with modifiers.
    ///
    /// The eleven parameters are the `drmModeAddFB2WithModifiers`
    /// prototype verbatim — the ABI is the API here.
    ///
    /// # Errors
    /// [`DisplayError::System`] (negative errno) when registration
    /// fails (format rejection, bad geometry, driver refusal).
    #[allow(clippy::too_many_arguments)]
    pub fn add_fb2(
        &self,
        fd: RawFd,
        width: u32,
        height: u32,
        format: u32,
        handles: &[u32; 4],
        pitches: &[u32; 4],
        offsets: &[u32; 4],
        modifier: u64,
    ) -> Result<u32> {
        let modifiers = [modifier; 4];
        let mut fb_id: c_uint = 0;
        // SAFETY: fd live; the four fixed-size arrays are valid borrows;
        // fb_id is a valid out-pointer.
        let rc = unsafe {
            (self.fns.add_fb2_mod)(
                fd,
                width,
                height,
                format,
                handles.as_ptr(),
                pitches.as_ptr(),
                offsets.as_ptr(),
                modifiers.as_ptr(),
                0,
                &mut fb_id,
                0,
            )
        };
        check_rc(rc, "drmModeAddFB2WithModifiers")?;
        Ok(fb_id)
    }

    /// Unregister a framebuffer.
    ///
    /// # Errors
    /// [`DisplayError::System`] (negative errno) when `drmModeRmFB`
    /// fails.
    pub fn rm_fb(&self, fd: RawFd, fb_id: u32) -> Result<()> {
        // SAFETY: fd live; fb_id was minted by this device.
        let rc = unsafe { (self.fns.rm_fb)(fd, fb_id) };
        check_rc(rc, "drmModeRmFB")
    }

    /// Whether this fd holds DRM-Master.
    ///
    /// # Errors
    /// Never fails; the `Result` keeps the call uniform with the
    /// ioctls around it.
    pub fn is_master(&self, fd: RawFd) -> Result<bool> {
        // SAFETY: fd live.
        Ok(unsafe { (self.fns.is_master)(fd) } != 0)
    }

    /// Acquire DRM-Master rights (`drmSetMaster`) — the privilege the
    /// modeset path (and its rehearsal) needs to change display state.
    ///
    /// # Errors
    /// [`DisplayError::System`] when the takeover fails (another
    /// master owns the device — a running display server).
    pub fn set_master(&self, fd: RawFd) -> Result<()> {
        // SAFETY: fd live; no pointers cross.
        if unsafe { (self.fns.set_master)(fd) } != 0 {
            return Err(sys_error("drmSetMaster"));
        }
        Ok(())
    }

    /// Release DRM-Master rights (`drmDropMaster`).
    ///
    /// # Errors
    /// [`DisplayError::System`] on failure.
    pub fn drop_master(&self, fd: RawFd) -> Result<()> {
        // SAFETY: fd live; no pointers cross.
        if unsafe { (self.fns.drop_master)(fd) } != 0 {
            return Err(sys_error("drmDropMaster"));
        }
        Ok(())
    }

    /// Import a PRIME/dma-buf file descriptor as a GEM handle
    /// (`drmPrimeFDToHandle`) — the scanout registration walk's first
    /// step. Re-importing the same underlying dma-buf on the same fd
    /// yields the same handle (the kernel's object-identity
    /// semantics).
    ///
    /// Only PRIME/dma-buf descriptors import: a memfd or regular file
    /// fails cleanly with `EBADF`/`EINVAL`-shaped errors, which the
    /// caller reports as "CPU composition" (the honest degradation —
    /// never a silent fallback).
    ///
    /// # Errors
    /// [`DisplayError::System`] when the kernel rejects the import.
    pub fn prime_fd_to_handle(&self, fd: RawFd, prime_fd: RawFd) -> Result<u32> {
        let mut handle: u32 = 0;
        // SAFETY: both fds live; `handle` is a valid, properly-aligned
        // out-pointer of the exact type the ABI writes.
        let rc = unsafe { (self.fns.prime_fd_to_handle)(fd, prime_fd, &mut handle) };
        if rc != 0 {
            return Err(sys_error("drmPrimeFDToHandle"));
        }
        Ok(handle)
    }

    /// Allocate a dumb buffer (`DRM_IOCTL_MODE_CREATE_DUMB`): the
    /// classic scanout-capable CPU buffer. Returns `(handle, pitch)`.
    ///
    /// # Errors
    /// [`DisplayError::System`] when the kernel rejects the allocation.
    pub fn create_dumb(&self, fd: RawFd, width: u32, height: u32, bpp: u32) -> Result<(u32, u32)> {
        let mut req = RawCreateDumb {
            height,
            width,
            bpp,
            flags: 0,
            handle: 0,
            pitch: 0,
            size: 0,
        };
        // SAFETY: fd live; `req` is a valid, properly-aligned borrow
        // of the exact struct the ioctl expects; the kernel writes
        // only the out-fields.
        let rc = unsafe {
            (self.fns.drm_ioctl)(
                fd,
                IOCTL_CREATE_DUMB,
                std::ptr::addr_of_mut!(req).cast::<c_void>(),
            )
        };
        if rc != 0 {
            return Err(sys_error("DRM_IOCTL_MODE_CREATE_DUMB"));
        }
        Ok((req.handle, req.pitch))
    }

    /// Destroy a dumb buffer (`DRM_IOCTL_MODE_DESTROY_DUMB`).
    ///
    /// # Errors
    /// [`DisplayError::System`] when the kernel rejects the free.
    pub fn destroy_dumb(&self, fd: RawFd, handle: u32) -> Result<()> {
        let req = RawDestroyDumb { handle };
        // SAFETY: fd live; `req` is a valid borrow of the exact struct.
        let rc = unsafe {
            (self.fns.drm_ioctl)(
                fd,
                IOCTL_DESTROY_DUMB,
                std::ptr::addr_of!(req) as *mut c_void,
            )
        };
        if rc != 0 {
            return Err(sys_error("DRM_IOCTL_MODE_DESTROY_DUMB"));
        }
        Ok(())
    }

    /// Resolve a dumb buffer's mmap offset
    /// (`DRM_IOCTL_MODE_MAP_DUMB`) — the fake offset `mmap(2)` accepts
    /// for this GEM object on this fd.
    ///
    /// # Errors
    /// [`DisplayError::System`] when the kernel rejects the map request.
    pub fn map_dumb_offset(&self, fd: RawFd, handle: u32) -> Result<u64> {
        let mut req = RawMapDumb {
            handle,
            pad: 0,
            offset: 0,
        };
        // SAFETY: fd live; `req` is a valid, properly-aligned borrow of
        // the exact struct the ioctl expects; the kernel writes only
        // the out-field.
        let rc = unsafe {
            (self.fns.drm_ioctl)(
                fd,
                IOCTL_MAP_DUMB,
                std::ptr::addr_of_mut!(req).cast::<c_void>(),
            )
        };
        if rc != 0 {
            return Err(sys_error("DRM_IOCTL_MODE_MAP_DUMB"));
        }
        Ok(req.offset)
    }

    /// Map a dumb buffer for CPU access: resolve the mmap offset, then
    /// `mmap(PROT_READ | PROT_WRITE, MAP_SHARED)` the kernel's backing
    /// storage. The returned [`DumbMapping`] owns the mapping and
    /// unmaps on drop.
    ///
    /// # Errors
    /// [`DisplayError::System`] when either the offset ioctl or the
    /// `mmap` itself fails.
    pub fn map_dumb(&self, fd: RawFd, handle: u32, size: u64) -> Result<DumbMapping> {
        let offset = self.map_dumb_offset(fd, handle)?;
        let len = usize::try_from(size).map_err(|_| DisplayError::System {
            errno: 0,
            while_doing: "dumb mapping size overflows the address space",
        })?;
        if len == 0 {
            return Err(DisplayError::System {
                errno: 0,
                while_doing: "zero-length dumb mapping",
            });
        }
        // SAFETY: fd live; offset is the kernel-minted fake offset for
        // exactly this (fd, handle) pair; len is the CREATE_DUMB size;
        // PROT_READ|PROT_WRITE over MAP_SHARED is the dumb-buffer CPU
        // access contract. mmap returns MAP_FAILED (all-ones) on error —
        // checked immediately, so the pointer handed to DumbMapping is
        // always a live mapping.
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                offset as libc::off_t,
            )
        };
        if ptr == libc::MAP_FAILED {
            return Err(sys_error("mmap of a dumb buffer"));
        }
        // SAFETY: ptr is a live, page-aligned mapping of exactly len
        // bytes (mmap contract, checked above).
        let base = unsafe { NonNull::new_unchecked(ptr.cast::<u8>()) };
        Ok(DumbMapping { base, len })
    }

    /// Enumerate DRM devices and their node paths.
    ///
    /// # Errors
    /// [`DisplayError::System`] when the second `drmGetDevices2` call
    /// fails after the count call succeeded; an empty list (no DRM on
    /// the machine) is `Ok(vec![])`.
    pub fn devices(&self) -> Result<Vec<RawDrmDevice>> {
        // SAFETY: first call with max=0 returns the needed count without
        // touching the (null) out array.
        let count = unsafe { (self.fns.get_devices2)(0, std::ptr::null_mut(), 0) };
        if count <= 0 {
            return Ok(Vec::new());
        }
        let mut devices: Vec<*mut RawDrmDeviceAbi> = vec![std::ptr::null_mut(); count as usize];
        // SAFETY: the out array has `count` slots as requested.
        let got = unsafe { (self.fns.get_devices2)(0, devices.as_mut_ptr(), count) };
        if got <= 0 {
            return Err(sys_error("drmGetDevices2"));
        }
        let mut out = Vec::with_capacity(got as usize);
        for &dev in &devices[..got as usize] {
            // SAFETY: dev points at a libdrm-owned drmDevice for the
            // duration of this block; node strings are NUL-terminated
            // libdrm allocations read via CStr (copied out).
            unsafe {
                if dev.is_null() {
                    continue;
                }
                let nodes_ptr = (*dev).nodes;
                let read = |i: usize| -> Option<String> {
                    if nodes_ptr.is_null() {
                        return None;
                    }
                    let p = *nodes_ptr.add(i);
                    if p.is_null() {
                        return None;
                    }
                    Some(CStr::from_ptr(p).to_string_lossy().into_owned())
                };
                out.push(RawDrmDevice {
                    nodes: [read(0), read(1), read(2)],
                    has_primary: (*dev).available_nodes & 1 != 0,
                    has_render: (*dev).available_nodes & 4 != 0,
                });
            }
        }
        // SAFETY: the array was filled by drmGetDevices2; free it with
        // the matching drmFreeDevices.
        unsafe { (self.fns.free_devices)(devices.as_mut_ptr(), got) };
        Ok(out)
    }

    /// Atomic request builder guard. Freeing happens on drop.
    ///
    /// # Errors
    /// [`DisplayError::System`] when `drmModeAtomicAlloc` fails
    /// (allocation failure).
    pub fn atomic(&self) -> Result<AtomicReq<'_>> {
        // SAFETY: allocation returns a libdrm-owned opaque request.
        let req = unsafe { (self.fns.atomic_alloc)() };
        if req.is_null() {
            return Err(sys_error("drmModeAtomicAlloc"));
        }
        Ok(AtomicReq { lib: self, req })
    }

    /// Commit an atomic request. The `mailbox` is the caller-owned
    /// event sink whose address libdrm hands to the page-flip handler
    /// when the flips in this commit mature; the backend keeps one
    /// `Box`-pinned mailbox for its whole lifetime, so the pointer stays
    /// valid across every pending flip.
    pub(crate) fn atomic_commit(
        &self,
        fd: RawFd,
        req: *mut c_void,
        flags: u32,
        mailbox: &mut Vec<RawFlipEvent>,
    ) -> Result<()> {
        // SAFETY: req is a live drmModeAtomicReq from atomic_alloc with
        // properties added; mailbox outlives every flip in this commit
        // (the backend owns the Box for its lifetime); libdrm stores the
        // pointer and only dereferences it inside drmHandleEvent, on the
        // same thread.
        let rc = unsafe {
            (self.fns.atomic_commit)(
                fd,
                req,
                flags,
                (mailbox as *mut Vec<RawFlipEvent>).cast::<c_void>(),
            )
        };
        check_rc(rc, "drmModeAtomicCommit")
    }

    /// Add one property to an atomic request.
    pub(crate) fn atomic_add(
        &self,
        req: *mut c_void,
        object_id: u32,
        prop_id: u32,
        value: u64,
    ) -> Result<()> {
        // SAFETY: req is live; ids came from this device's walks.
        let rc = unsafe { (self.fns.atomic_add)(req, object_id, prop_id, value) };
        check_rc(rc, "drmModeAtomicAddProperty")
    }

    /// Drain pending page-flip events into the mailbox whose address
    /// was committed with the flips. The kernel queues events in the
    /// fd's read buffer; this call reads and dispatches what is already
    /// there (nonblocking when the fd is O_NONBLOCK).
    ///
    /// SAFETY contract (caller): every pending event's stored user_data
    /// must point at a live `Vec<RawFlipEvent>` — guaranteed by the
    /// backend owning one `Box`-pinned mailbox per device for its whole
    /// lifetime and passing that same allocation at commit time.
    ///
    /// # Errors
    /// [`DisplayError::System`] when `drmHandleEvent` fails on the
    /// read.
    pub fn handle_events(&self, fd: RawFd) -> Result<()> {
        let ctx = RawEventContext {
            version: 2,
            _pad0: 0,
            vblank_handler: std::ptr::null(),
            page_flip_handler: std::ptr::null(),
            page_flip_handler2: Some(flip_handler2),
        };
        // SAFETY: fd live; ctx valid for the call's duration and not
        // retained by libdrm; handlers touch only the user_data the
        // backend committed, per the contract above.
        let rc = unsafe { (self.fns.handle_event)(fd, std::ptr::addr_of!(ctx).cast_mut()) };
        check_rc(rc, "drmHandleEvent")
    }

    /// Adopt a file descriptor the kernel minted for this process (an
    /// out-fence sync file). Ownership transfers here and nowhere else.
    pub(crate) fn owned_fd_from_kernel(fd: i32) -> OwnedFd {
        debug_assert!(fd >= 0);
        // SAFETY: the kernel allocated this descriptor for the caller
        // during the ioctl; taking it via from_raw_fd claims exactly the
        // one reference that exists, so the OwnedFd's drop closes it once.
        unsafe { OwnedFd::from_raw_fd(fd) }
    }
}

/// A live atomic request; freed on drop.
pub struct AtomicReq<'a> {
    lib: &'a LibDrm,
    req: *mut c_void,
}

impl AtomicReq<'_> {
    /// Add one (object, property, value) triple.
    ///
    /// # Errors
    /// [`DisplayError::System`] (negative errno) when the add fails
    /// (request capacity exhausted).
    pub fn add(&mut self, object_id: u32, prop_id: u32, value: u64) -> Result<()> {
        self.lib.atomic_add(self.req, object_id, prop_id, value)
    }

    /// Commit with flags; flips mature into `mailbox` (the backend's
    /// Box-pinned event sink, whose address outlives the commit).
    ///
    /// # Errors
    /// [`DisplayError::System`] (negative errno) when the commit fails
    /// (`EBUSY` for a pending flip, `EINVAL` for shape, ...); callers
    /// translate to the typed taxonomy.
    pub fn commit(&mut self, fd: RawFd, flags: u32, mailbox: &mut Vec<RawFlipEvent>) -> Result<()> {
        self.lib.atomic_commit(fd, self.req, flags, mailbox)
    }
}

impl Drop for AtomicReq<'_> {
    fn drop(&mut self) {
        // SAFETY: req was allocated by atomic_alloc and not yet freed.
        unsafe { (self.lib.fns.atomic_free)(self.req) };
    }
}

impl Drop for LibDrm {
    fn drop(&mut self) {
        // SAFETY: handle came from dlopen and is dropped exactly once.
        unsafe { libc::dlclose(self.handle) };
    }
}

/// An owned CPU mapping of a dumb buffer (or any shared mapping the
/// sys layer mints).
///
/// Created through [`LibDrm::map_dumb`] (kernel GEM storage) or
/// [`anon_mapping`] (anonymous, the test vehicle). Drop unmaps. The
/// mapping is `Send`: a mapped region is not thread-bound, and every
/// in-crate owner serializes access behind the compositor's world
/// lock (the same discipline the mailbox follows).
#[derive(Debug)]
pub struct DumbMapping {
    base: NonNull<u8>,
    len: usize,
}

// SAFETY: the mapped region itself carries no thread affinity; the
// compositor accesses it under one mutex (the single-pipeline
// doctrine), so transferring the handle across threads is sound.
unsafe impl Send for DumbMapping {}
// SAFETY: same reasoning for shared references — access stays
// serialized by the owning layer.
unsafe impl Sync for DumbMapping {}

impl DumbMapping {
    /// The mapping's byte length.
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the mapping is empty (never — constructors reject it).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The mapping as shared bytes.
    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        // SAFETY: base..base+len is the live mapping we own; no &mut
        // aliasing exists while this borrow lives (single-pipeline
        // lock discipline).
        unsafe { std::slice::from_raw_parts(self.base.as_ptr(), self.len) }
    }

    /// The mapping as mutable bytes.
    #[must_use]
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        // SAFETY: base..base+len is the live mapping we own; &mut self
        // makes this the only writer for the borrow's duration.
        unsafe { std::slice::from_raw_parts_mut(self.base.as_ptr(), self.len) }
    }
}

impl Drop for DumbMapping {
    fn drop(&mut self) {
        // SAFETY: base..base+len is exactly the mapping Drop owns and
        // was never unmapped elsewhere (the type forbids duplication).
        unsafe { libc::munmap(self.base.as_ptr().cast(), self.len) };
    }
}

/// An anonymous shared mapping of `len` bytes — the same lifetime
/// discipline as a dumb-buffer mapping, minus the kernel object. The
/// equivalence suites use it to exercise the mapped-delivery path
/// (pitch-honoring writes, front/back tracking) on CI machines
/// without DRM nodes.
///
/// # Errors
/// [`DisplayError::System`] when `mmap` fails.
pub fn anon_mapping(len: usize) -> Result<DumbMapping> {
    if len == 0 {
        return Err(DisplayError::System {
            errno: 0,
            while_doing: "zero-length anonymous mapping",
        });
    }
    // SAFETY: fd -1 with MAP_ANONYMOUS is the anonymous-mapping
    // contract; MAP_FAILED is checked before the pointer escapes.
    let ptr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED | libc::MAP_ANONYMOUS,
            -1,
            0,
        )
    };
    if ptr == libc::MAP_FAILED {
        return Err(sys_error("mmap of an anonymous mapping"));
    }
    // SAFETY: ptr is a live, page-aligned mapping of exactly len bytes.
    let base = unsafe { NonNull::new_unchecked(ptr.cast::<u8>()) };
    Ok(DumbMapping { base, len })
}

/// Poll one descriptor for readability, waiting at most `timeout_ms`
/// (negative = indefinitely). `false` on timeout, spurious wake, or
/// `EINTR` — the caller re-checks its own shutdown conditions and
/// retries; a real poll failure is a typed error.
///
/// # Errors
/// [`DisplayError::System`] when `poll` fails for a reason other than
/// interruption.
pub fn poll_readable(fd: RawFd, timeout_ms: i32) -> Result<bool> {
    let mut fds = [libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    }];
    // SAFETY: fds is a valid one-element pollfd array for the call's
    // duration; the kernel writes only revents.
    let rc = unsafe { libc::poll(fds.as_mut_ptr(), 1, timeout_ms) };
    if rc < 0 {
        let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(-1);
        if errno == libc::EINTR {
            return Ok(false);
        }
        return Err(DisplayError::System {
            errno,
            while_doing: "poll for device readability",
        });
    }
    Ok(rc > 0 && (fds[0].revents & libc::POLLIN) != 0)
}

/// The machine's monotonic clock — the *real* device's time domain
/// (kernel flip timestamps use CLOCK_MONOTONIC; the mock device's
/// injected clock mirrors it for CI determinism).
///
/// # Errors
/// [`DisplayError::System`] when `clock_gettime` fails (a kernel
/// contract violation; effectively unreachable).
pub fn monotonic_now() -> Result<ldp_core::time::Mono> {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: ts is a valid timespec borrow; the kernel writes only it.
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    if rc != 0 {
        return Err(sys_error("clock_gettime(CLOCK_MONOTONIC)"));
    }
    Ok(ldp_core::time::Mono::from_ns(
        u64::try_from(ts.tv_sec)
            .unwrap_or(0)
            .saturating_mul(1_000_000_000)
            + u64::try_from(ts.tv_nsec).unwrap_or(0),
    ))
}

fn sys_error(what: &'static str) -> DisplayError {
    DisplayError::System {
        errno: std::io::Error::last_os_error().raw_os_error().unwrap_or(-1),
        while_doing: what,
    }
}

fn check_rc(rc: c_int, what: &'static str) -> Result<()> {
    if rc == 0 {
        Ok(())
    } else {
        // libdrm returns -errno.
        let errno = -rc;
        Err(DisplayError::System {
            errno,
            while_doing: what,
        })
    }
}
