//! The libdrm runtime layer for node discovery — dlopen, two symbols.
//!
//! This is the crate's first **audit boundary**: every `unsafe` lives
//! here, each call site carrying a `SAFETY` comment. `ldp-display`
//! carries the full KMS symbol table; this crate needs exactly one
//! libdrm capability — enumerating devices and their node paths — so
//! the table here is deliberately minimal (loading all of libdrm to
//! call two functions would widen the audit for nothing).
//!
//! Failure doctrine matches the display crate: on machines without
//! DRM the count call returns non-positive and the walk yields an
//! empty catalog — a legitimate, tested outcome, not an error.

use std::ffi::{c_char, c_int, c_uint, c_void, CStr};

use crate::node::GpuDevice;

/// SONAME.
const LIBDRM: &str = "libdrm.so.2";

/// `DRM_NODE_*` indices into `drmDevice::nodes` (drm.h §1026).
const NODE_PRIMARY: usize = 0;
const NODE_CONTROL: usize = 1;
const NODE_RENDER: usize = 2;

/// The `available_nodes` bitmask bits (`1 << NODE_*`).
const BIT_PRIMARY: i32 = 1;
const BIT_RENDER: i32 = 4;

/// The resolved function table.
#[derive(Clone, Copy)]
struct NodeFns {
    get_devices2: unsafe extern "C" fn(c_uint, *mut *mut RawDrmDevice, c_int) -> c_int,
    free_devices: unsafe extern "C" fn(*mut *mut RawDrmDevice, c_int),
}

/// `drmDevice`'s stable prefix: `nodes[3]`, `available_nodes`, then
/// opaque bus info pointers we never touch.
#[repr(C)]
struct RawDrmDevice {
    nodes: *mut *mut c_char,
    available_nodes: c_int,
    _bustype: c_int,
    _businfo: *mut c_void,
    _deviceinfo: *mut c_void,
}
use core::mem::size_of;

const _: () = assert!(size_of::<RawDrmDevice>() == 32);

/// The loaded library.
pub struct LibDrmNodes {
    handle: *mut c_void,
    fns: NodeFns,
}

// dlopen handles are loader-refcounted and thread-safe; the table is a
// plain copy of function pointers.
unsafe impl Send for LibDrmNodes {}
unsafe impl Sync for LibDrmNodes {}

impl core::fmt::Debug for LibDrmNodes {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // The handle and table are opaque pointers; the name is the
        // identity that matters in a failure report.
        f.debug_struct("LibDrmNodes")
            .field("library", &LIBDRM)
            .finish_non_exhaustive()
    }
}

impl LibDrmNodes {
    /// `dlopen("libdrm.so.2")` and resolve the two symbols.
    ///
    /// # Errors
    /// [`NodeError::LibraryLoad`] without the library;
    /// [`NodeError::MissingSymbol`] naming the gap.
    pub fn open() -> Result<Self, NodeError> {
        Self::open_path(c"libdrm.so.2")
    }

    /// Open a specific path (tests).
    ///
    /// # Errors
    /// [`NodeError::LibraryLoad`] when the path does not resolve to a
    /// library; [`NodeError::MissingSymbol`] naming the gap.
    pub fn open_path(name: &CStr) -> Result<Self, NodeError> {
        // SAFETY: `name` is NUL-terminated; dlopen only reads it for
        // the duration of the call.
        let handle = unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
        if handle.is_null() {
            return Err(NodeError::LibraryLoad { library: LIBDRM });
        }
        // SAFETY: handle is live; each prototype matches the published
        // libdrm ABI exactly (drmGetDevices2/drmFreeDevices, LIBUDEV-era
        // stable since libdrm 2.4.78).
        let fns = unsafe {
            let get = libc::dlsym(handle, c"drmGetDevices2".as_ptr());
            if get.is_null() {
                libc::dlclose(handle);
                return Err(NodeError::MissingSymbol {
                    library: LIBDRM,
                    symbol: "drmGetDevices2",
                });
            }
            let free = libc::dlsym(handle, c"drmFreeDevices".as_ptr());
            if free.is_null() {
                libc::dlclose(handle);
                return Err(NodeError::MissingSymbol {
                    library: LIBDRM,
                    symbol: "drmFreeDevices",
                });
            }
            NodeFns {
                get_devices2: core::mem::transmute::<
                    *mut c_void,
                    unsafe extern "C" fn(c_uint, *mut *mut RawDrmDevice, c_int) -> c_int,
                >(get),
                free_devices: core::mem::transmute::<
                    *mut c_void,
                    unsafe extern "C" fn(*mut *mut RawDrmDevice, c_int),
                >(free),
            }
        };
        Ok(Self { handle, fns })
    }

    /// Enumerate DRM devices with their node paths.
    ///
    /// # Errors
    /// [`NodeError::System`] when the second `drmGetDevices2` call
    /// fails after the count call succeeded. No DRM devices on the
    /// machine is `Ok(vec![])`.
    pub fn devices(&self) -> Result<Vec<GpuDevice>, NodeError> {
        // SAFETY: first call with max=0 returns the needed count
        // without touching the (null) out array.
        let count = unsafe { (self.fns.get_devices2)(0, core::ptr::null_mut(), 0) };
        if count <= 0 {
            return Ok(Vec::new());
        }
        let mut raw: Vec<*mut RawDrmDevice> = vec![core::ptr::null_mut(); count as usize];
        // SAFETY: the out array has `count` slots as requested.
        let got = unsafe { (self.fns.get_devices2)(0, raw.as_mut_ptr(), count) };
        if got <= 0 {
            return Err(NodeError::System {
                errno: 0,
                while_doing: "drmGetDevices2",
            });
        }
        let mut out = Vec::with_capacity(got as usize);
        for &dev in &raw[..got as usize] {
            // SAFETY: dev points at a libdrm-owned drmDevice for this
            // block; node strings are NUL-terminated libdrm allocations
            // read via CStr and copied out before the array is freed.
            unsafe {
                if dev.is_null() {
                    continue;
                }
                let nodes = (*dev).nodes;
                let read = |i: usize| -> Option<String> {
                    if nodes.is_null() {
                        return None;
                    }
                    // SAFETY: `nodes` has three slots; slot i is either
                    // NULL or a NUL-terminated path valid for the block.
                    let p = *nodes.add(i);
                    if p.is_null() {
                        None
                    } else {
                        Some(CStr::from_ptr(p).to_string_lossy().into_owned())
                    }
                };
                out.push(GpuDevice {
                    primary: read(NODE_PRIMARY),
                    control: read(NODE_CONTROL),
                    render: read(NODE_RENDER),
                    has_primary: (*dev).available_nodes & BIT_PRIMARY != 0,
                    has_render: (*dev).available_nodes & BIT_RENDER != 0,
                });
            }
        }
        // SAFETY: the array was filled by drmGetDevices2; freeing it
        // with the matching drmFreeDevices releases it exactly once.
        unsafe { (self.fns.free_devices)(raw.as_mut_ptr(), got) };
        Ok(out)
    }
}

impl Drop for LibDrmNodes {
    fn drop(&mut self) {
        // SAFETY: handle came from dlopen and is dropped exactly once.
        unsafe { libc::dlclose(self.handle) };
    }
}

/// Node discovery failures.
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum NodeError {
    /// The shared library could not be loaded.
    LibraryLoad {
        /// SONAME that failed.
        library: &'static str,
    },
    /// The library loaded but a required symbol is missing.
    MissingSymbol {
        /// SONAME being searched.
        library: &'static str,
        /// The missing symbol.
        symbol: &'static str,
    },
    /// An ioctl returned an error the above do not cover.
    System {
        /// Raw errno (0 when the call returned a negative count without
        /// setting errno).
        errno: i32,
        /// What was being attempted.
        while_doing: &'static str,
    },
}

impl core::fmt::Display for NodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::LibraryLoad { library } => write!(f, "cannot load {library}"),
            Self::MissingSymbol { library, symbol } => {
                write!(f, "{library} is missing symbol {symbol}")
            }
            Self::System { errno, while_doing } => {
                write!(f, "drm error while {while_doing}: errno {errno}")
            }
        }
    }
}

impl std::error::Error for NodeError {}
