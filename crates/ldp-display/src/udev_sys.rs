//! The libudev runtime layer — hotplug monitoring over the kernel's
//! uevent netlink.
//!
//! This is the second audited `sys` boundary of the crate (the
//! `ldp-transport` precedent: every `unsafe` call carries a `SAFETY`
//! comment, the module declares no `forbid`, and everything the rest
//! of the crate sees is safe, owned Rust). [`UdevMonitor`] implements
//! [`HotplugSource`] directly — the trait is safe, the FFI under it is
//! not, so the implementation lives here and nowhere else.
//!
//! Loading mirrors [`crate::drm::sys`]: `LibUdev::open()` resolves the
//! full symbol table up front or names the missing symbol. The monitor
//! subscribes to the `udev` netlink source filtered to the DRM
//! subsystem, flips its socket nonblocking (drain must never park the
//! caller's event loop), and hands the raw fd out for polling.
//!
//! What reaches [`HotplugSignal`]: the action string (`change`, `add`,
//! `remove`, verbatim — future kernel actions survive) and the device
//! node path when the event carries one. Unknown-shape events are not
//! dropped: a `NULL` devnode is a legitimate `HotplugSignal` with
//! `devnode: None`.

use std::ffi::{c_char, c_int, c_void, CStr, CString};

use crate::error::{DisplayError, Result};
use crate::hotplug::{HotplugSignal, HotplugSource};

/// SONAME.
const LIBUDEV: &str = "libudev.so.1";

/// The DRM subsystem name (kernel constant).
const DRM_SUBSYSTEM: &str = "drm";

// ---------------------------------------------------------------------------
// Symbol table (prototypes from libudev.h — the LIBUDEV_183 baseline).
// ---------------------------------------------------------------------------

macro_rules! udev_fns {
    ($( $field:ident : $sig:ty = $sym:literal ),+ $(,)?) => {
        /// The resolved function table.
        #[derive(Clone, Copy)]
        struct UdevFns {
            $( pub $field: $sig ),+
        }
        impl UdevFns {
            /// Resolve every symbol from `handle` or fail naming it.
            ///
            /// # Safety
            /// `handle` must be a live `dlopen` handle. Each `$sig` must
            /// exactly match the C prototype of `$sym` — the public
            /// libudev ABI, stable across the LIBUDEV_183 baseline.
            unsafe fn resolve(handle: *mut c_void) -> Result<Self> {
                let missing =
                    |symbol: &'static str| DisplayError::MissingSymbol { library: LIBUDEV, symbol };
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
    };
}

udev_fns! {
    new: unsafe extern "C" fn() -> *mut c_void = "udev_new",
    unref: unsafe extern "C" fn(*mut c_void) -> *mut c_void = "udev_unref",
    monitor_new: unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void =
        "udev_monitor_new_from_netlink",
    monitor_unref: unsafe extern "C" fn(*mut c_void) -> *mut c_void = "udev_monitor_unref",
    filter_subsystem: unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) -> c_int =
        "udev_monitor_filter_add_match_subsystem_devtype",
    enable_receiving: unsafe extern "C" fn(*mut c_void) -> c_int =
        "udev_monitor_enable_receiving",
    monitor_fd: unsafe extern "C" fn(*mut c_void) -> c_int = "udev_monitor_get_fd",
    receive_device: unsafe extern "C" fn(*mut c_void) -> *mut c_void =
        "udev_monitor_receive_device",
    device_unref: unsafe extern "C" fn(*mut c_void) -> *mut c_void = "udev_device_unref",
    device_action: unsafe extern "C" fn(*mut c_void) -> *const c_char = "udev_device_get_action",
    device_devnode: unsafe extern "C" fn(*mut c_void) -> *const c_char = "udev_device_get_devnode",
    device_subsystem: unsafe extern "C" fn(*mut c_void) -> *const c_char =
        "udev_device_get_subsystem",
}

/// The loaded library.
struct LibUdev {
    handle: *mut c_void,
    fns: UdevFns,
}

// dlopen handles are thread-safe (loader-refcounted); the table is a
// plain copy of function pointers.
unsafe impl Send for LibUdev {}
unsafe impl Sync for LibUdev {}

impl LibUdev {
    /// `dlopen("libudev.so.1")` and resolve the ABI.
    ///
    /// # Errors
    /// [`DisplayError::LibraryLoad`] without the library;
    /// [`DisplayError::MissingSymbol`] naming the first gap.
    fn open() -> Result<Self> {
        Self::open_path(c"libudev.so.1")
    }

    /// Open a specific path (tests).
    fn open_path(name: &CStr) -> Result<Self> {
        // SAFETY: `name` is NUL-terminated; dlopen only reads it for the
        // duration of the call.
        let handle = unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
        if handle.is_null() {
            return Err(DisplayError::LibraryLoad { library: LIBUDEV });
        }
        // SAFETY: handle is live; prototypes match the libudev ABI (see
        // the macro's Safety contract).
        let fns = unsafe { UdevFns::resolve(handle)? };
        Ok(Self { handle, fns })
    }
}

impl core::fmt::Debug for LibUdev {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // The handle and table are opaque; the name is the identity.
        f.debug_struct("LibUdev")
            .field("library", &LIBUDEV)
            .finish_non_exhaustive()
    }
}

impl Drop for LibUdev {
    fn drop(&mut self) {
        // SAFETY: handle came from dlopen and is dropped exactly once.
        unsafe { libc::dlclose(self.handle) };
    }
}

/// A DRM-subsystem hotplug monitor.
///
/// Owns its libudev context and monitor; the monitor is unreffed before
/// the context on drop (libudev's documented teardown order).
pub struct UdevMonitor {
    lib: LibUdev,
    udev: *mut c_void,
    mon: *mut c_void,
}

// The monitor is used from the compositor's poll thread only; the
// libudev objects themselves are not documented thread-safe, so we
// keep the type !Send by default and let callers wrap it.
impl UdevMonitor {
    /// Subscribe to DRM hotplug on the `udev` netlink source.
    ///
    /// # Errors
    /// [`DisplayError::LibraryLoad`]/[`DisplayError::MissingSymbol`]
    /// without a usable libudev; [`DisplayError::System`] when the
    /// netlink monitor, the subsystem filter, or the receive-enable
    /// step fails (permissions, no uevent socket in the sandbox).
    ///
    /// # Panics
    /// Never in practice: the one `expect` guards a static, NUL-free
    /// subsystem name.
    pub fn new() -> Result<Self> {
        let lib = LibUdev::open()?;
        // SAFETY: pure constructor.
        let udev = unsafe { (lib.fns.new)() };
        if udev.is_null() {
            return Err(DisplayError::System {
                errno: 12,
                while_doing: "udev_new",
            });
        }
        // SAFETY: udev context is live for the call; the name literal is
        // NUL-terminated static storage.
        let mon = unsafe { (lib.fns.monitor_new)(udev, c"udev".as_ptr()) };
        if mon.is_null() {
            // SAFETY: teardown of the just-created context on the error
            // path (monitor holds no reference yet).
            unsafe { (lib.fns.unref)(udev) };
            return Err(DisplayError::System {
                errno: 12,
                while_doing: "udev_monitor_new_from_netlink",
            });
        }
        // SAFETY: mon is live; the DRM subsystem name is NUL-terminated
        // CString storage valid for the call; a NULL devtype matches
        // every devtype — the conservative choice, since a re-probe is
        // always the safe response to any DRM-subsystem event.
        let drm = CString::new(DRM_SUBSYSTEM).expect("subsystem name has no NUL");
        let filtered = unsafe { (lib.fns.filter_subsystem)(mon, drm.as_ptr(), std::ptr::null()) };
        if filtered < 0 {
            // SAFETY: mon is live and not yet enabled.
            unsafe { (lib.fns.monitor_unref)(mon) };
            return Err(DisplayError::System {
                errno: -filtered,
                while_doing: "udev_monitor_filter_add_match_subsystem_devtype",
            });
        }
        // SAFETY: mon is live with the filter installed.
        let enabled = unsafe { (lib.fns.enable_receiving)(mon) };
        if enabled < 0 {
            // SAFETY: mon is live.
            unsafe { (lib.fns.monitor_unref)(mon) };
            return Err(DisplayError::System {
                errno: -enabled,
                while_doing: "udev_monitor_enable_receiving",
            });
        }
        // Nonblocking drain contract: the socket must never park the
        // caller. SAFETY: fd belongs to mon, which stays live; fcntl on
        // our own descriptor is a plain flag set.
        let fd = unsafe { (lib.fns.monitor_fd)(mon) };
        if fd < 0 {
            // SAFETY: mon is live.
            unsafe { (lib.fns.monitor_unref)(mon) };
            return Err(DisplayError::System {
                errno: 9,
                while_doing: "udev_monitor_get_fd",
            });
        }
        // SAFETY: valid descriptor; F_SETFL with F_GETFL-merged flags.
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            if flags >= 0 {
                libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
            }
        }
        Ok(Self { lib, udev, mon })
    }

    /// The pollable descriptor (the netlink socket).
    #[must_use]
    pub fn fd(&self) -> i32 {
        // SAFETY: mon is live for the backend's lifetime.
        unsafe { (self.lib.fns.monitor_fd)(self.mon) }
    }

    /// Pull every already-queued device event. Never blocks (the socket
    /// runs O_NONBLOCK); a would-block read simply ends the loop.
    fn drain_devices(&mut self) -> Vec<HotplugSignal> {
        let mut out = Vec::new();
        loop {
            // SAFETY: mon is live; each returned device is unreffed below
            // before the next receive, and its borrowed strings are
            // copied out before that unref.
            let dev = unsafe { (self.lib.fns.receive_device)(self.mon) };
            if dev.is_null() {
                // No event queued (EAGAIN) or a transient malformed
                // event: both mean "nothing more to drain".
                break;
            }
            // SAFETY: dev is live for this block; the returned strings
            // are borrows of dev, valid until its unref.
            let (action, devnode, subsystem) = unsafe {
                (
                    (self.lib.fns.device_action)(dev),
                    (self.lib.fns.device_devnode)(dev),
                    (self.lib.fns.device_subsystem)(dev),
                )
            };
            let read = |ptr: *const c_char| -> Option<String> {
                if ptr.is_null() {
                    None
                } else {
                    // SAFETY: libudev returns NUL-terminated strings that
                    // stay valid until the device is unreffed (same block).
                    Some(
                        unsafe { CStr::from_ptr(ptr) }
                            .to_string_lossy()
                            .into_owned(),
                    )
                }
            };
            // The kernel filter already scopes us to DRM; a monitor
            // created before the filter edge (or a udev race) can still
            // surface foreign subsystems — skip them without dropping
            // the loop.
            if subsystem.is_null() || (unsafe { CStr::from_ptr(subsystem) }.to_bytes() == b"drm") {
                out.push(HotplugSignal::new(
                    read(action).as_deref().unwrap_or("unknown"),
                    read(devnode).as_deref(),
                ));
            }
            // SAFETY: dev was returned by receive_device and not yet
            // unreffed; this releases it exactly once.
            unsafe { (self.lib.fns.device_unref)(dev) };
        }
        out
    }
}

impl HotplugSource for UdevMonitor {
    fn drain(&mut self) -> Vec<HotplugSignal> {
        self.drain_devices()
    }

    fn fd(&self) -> Option<i32> {
        Some(self.fd())
    }
}

impl Drop for UdevMonitor {
    fn drop(&mut self) {
        // SAFETY: both handles came from their constructors and are
        // released exactly once, monitor first (libudev teardown order).
        unsafe {
            (self.lib.fns.monitor_unref)(self.mon);
            (self.lib.fns.unref)(self.udev);
        }
    }
}

/// Whether the runtime libudev is loadable and complete on this machine.
///
/// A diagnostic helper for callers that want to decide between the real
/// monitor and a fallback before constructing anything.
#[must_use]
pub fn libudev_available() -> bool {
    LibUdev::open().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_loads_and_resolves() {
        // This sandbox ships libudev.so.1 with the full monitor ABI.
        assert!(LibUdev::open().is_ok(), "libudev should load here");
        assert!(libudev_available());
    }

    #[test]
    fn bogus_soname_reports_library_load() {
        let err = LibUdev::open_path(c"libudev_does_not_exist.so.99").unwrap_err();
        assert!(matches!(
            err,
            DisplayError::LibraryLoad {
                library: "libudev.so.1"
            }
        ));
    }

    #[test]
    fn monitor_bootstraps_or_fails_typed() {
        // In containers without a uevent socket the bootstrap can fail;
        // what must NEVER happen is a panic or a hang — either a live
        // monitor or a typed DisplayError is acceptable.
        match UdevMonitor::new() {
            Ok(mut mon) => {
                let fd = mon.fd();
                assert!(fd >= 0);
                assert_eq!(HotplugSource::fd(&mon), Some(fd));
                // Nothing queued on a fresh monitor.
                assert!(mon.drain().is_empty());
            }
            Err(
                e @ (DisplayError::System { .. }
                | DisplayError::LibraryLoad { .. }
                | DisplayError::MissingSymbol { .. }),
            ) => {
                // Typed failure path exercised honestly.
                assert!(!e.to_string().is_empty());
            }
            Err(other) => panic!("unexpected error kind: {other:?}"),
        }
    }
}
