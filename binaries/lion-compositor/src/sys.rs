//! The audited syscall layer: shared-memory mapping, memfd creation,
//! and eventfd release fences.
//!
//! Every `unsafe` in this crate lives here, each call carrying a
//! `SAFETY` comment — the `ldp-transport`/`ldp-display` precedent. The
//! rest of the crate is `#![forbid(unsafe_code)]` per module; the crate
//! root therefore does not forbid (a forbid cannot be lifted per
//! module). Nothing here reads a clock or allocates; failures are
//! typed [`SysError`]s so callers stay honest about resource limits.
//!
//! * [`map_read_only`] — `mmap(PROT_READ, MAP_SHARED)` of a received
//!   pool descriptor; [`Mapping`] owns the region and unmaps on drop.
//! * [`memfd`] — `memfd_create(MFD_CLOEXEC)`, the anonymous pool
//!   backing clients and tests share.
//! * [`eventfd_signalled`] — an already-signalled eventfd: the
//!   headless stand-in for a kernel sync-file as a `buffer.release`
//!   fence (readability means "the compositor is done reading"; see
//!   the crate docs for the DRM-mode story).

use std::ffi::CString;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::ptr::NonNull;

use ldp_core::error::{LdpError, Result};

/// A raw-syscall failure, classified.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SysError {
    /// `mmap` returned `MAP_FAILED` (errno captured).
    MapFailed(i32),
    /// `munmap` failed (errno captured).
    UnmapFailed(i32),
    /// `memfd_create` failed (errno captured).
    MemfdFailed(i32),
    /// `eventfd` failed (errno captured).
    EventfdFailed(i32),
    /// Writing the eventfd would overflow its counter (compositor bug).
    EventfdOverflow,
    /// A zero-byte or otherwise unusable size was requested.
    BadSize,
    /// The platform's page size is unusably large (cannot happen on
    /// Linux; kept so every failure path is a named variant).
    BadPageSize,
}

impl SysError {
    /// The captured errno.
    #[must_use]
    pub fn errno(self) -> i32 {
        match self {
            Self::MapFailed(e)
            | Self::UnmapFailed(e)
            | Self::MemfdFailed(e)
            | Self::EventfdFailed(e) => e,
            Self::EventfdOverflow | Self::BadSize | Self::BadPageSize => 0,
        }
    }
}

impl std::fmt::Display for SysError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MapFailed(e) => write!(f, "mmap failed (errno {e})"),
            Self::UnmapFailed(e) => write!(f, "munmap failed (errno {e})"),
            Self::MemfdFailed(e) => write!(f, "memfd_create failed (errno {e})"),
            Self::EventfdFailed(e) => write!(f, "eventfd failed (errno {e})"),
            Self::EventfdOverflow => f.write_str("eventfd counter would overflow"),
            Self::BadSize => f.write_str("zero-size mapping requested"),
            Self::BadPageSize => f.write_str("unusable page size"),
        }
    }
}

impl std::error::Error for SysError {}

impl From<SysError> for LdpError {
    fn from(e: SysError) -> LdpError {
        LdpError::Io(std::sync::Arc::new(std::io::Error::other(format!(
            "lion-compositor sys: {e}"
        ))))
    }
}

/// Wrap a syscall's `-1`-errno result.
fn check(ret: i64, f: impl FnOnce(i32) -> SysError) -> std::result::Result<i64, SysError> {
    if ret == -1 {
        Err(f(std::io::Error::last_os_error()
            .raw_os_error()
            .unwrap_or(0)))
    } else {
        Ok(ret)
    }
}

/// The system page size (mmap length granularity; the kernel rounds up).
///
/// # Errors
/// [`SysError::BadPageSize`] if `sysconf` misbehaves.
pub fn page_size() -> std::result::Result<usize, SysError> {
    // SAFETY: sysconf with a static argument reads a global constant; it
    // touches no caller memory and never blocks.
    let ps = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if ps <= 0 {
        return Err(SysError::BadPageSize);
    }
    Ok(ps as usize)
}

/// An owned read-only `MAP_SHARED` mapping of a file descriptor.
///
/// The mapping is `Send`/`Sync`: it is a process-wide kernel mapping
/// with no interior mutability visible to Rust.
pub struct Mapping {
    ptr: NonNull<u8>,
    len: usize,
}

// SAFETY: the region is kernel-backed, read-only from this process's
// perspective, and has no thread-affinity.
unsafe impl Send for Mapping {}
// SAFETY: concurrent reads of a read-only shared mapping are safe; the
// peer may write concurrently, which is the shm contract (the protocol
// fence discipline serializes writers).
unsafe impl Sync for Mapping {}

impl Mapping {
    /// The mapped bytes.
    ///
    /// # Safety of the borrow
    /// The slice is valid until `self` is dropped or re-mapped; callers
    /// (the render path) never store it past the frame.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        // SAFETY: ptr/len came from a successful mmap of `len` bytes and
        // stay valid for the mapping's lifetime; read-only reads are
        // within the PROT_READ contract.
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }

    /// The mapping's length in bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the mapping is empty (never: construction rejects zero).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: ptr/len are exactly the values mmap returned and have
        // not been unmapped elsewhere (single ownership).
        unsafe {
            check(
                libc::munmap(self.ptr.as_ptr().cast(), self.len) as i64,
                SysError::UnmapFailed,
            )
            .ok();
        }
    }
}

impl std::fmt::Debug for Mapping {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mapping")
            .field("len", &self.len)
            .finish_non_exhaustive()
    }
}

/// `mmap` `size` bytes of `fd` read-only and shared.
///
/// The kernel rounds the length up to the page size; the returned
/// [`Mapping`] reports the requested `len` (the extra tail is zeros).
///
/// # Errors
/// [`SysError::BadSize`] for zero sizes; [`SysError::MapFailed`] with
/// the errno otherwise.
pub fn map_read_only(fd: &OwnedFd, size: u64) -> std::result::Result<Mapping, SysError> {
    if size == 0 {
        return Err(SysError::BadSize);
    }
    let len = usize::try_from(size).map_err(|_| SysError::BadSize)?;
    // SAFETY: mmap with a nonzero length, a valid fd we own for the call
    // duration, and PROT_READ|MAP_SHARED is the documented contract; the
    // return is checked against MAP_FAILED before use.
    let ret = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_READ,
            libc::MAP_SHARED,
            fd.as_raw_fd(),
            0,
        )
    };
    if ret == libc::MAP_FAILED {
        return Err(SysError::MapFailed(
            std::io::Error::last_os_error().raw_os_error().unwrap_or(0),
        ));
    }
    // SAFETY: ret is a non-null kernel mapping address.
    let ptr = unsafe { NonNull::new_unchecked(ret.cast::<u8>()) };
    Ok(Mapping { ptr, len })
}

/// Create an anonymous sealed-able memory file (`memfd_create`), owned.
///
/// The name is cosmetic (appears in `/proc` listings); the descriptor
/// is close-on-exec.
///
/// # Errors
/// [`SysError::MemfdFailed`] with the errno.
pub fn memfd(name: &str) -> std::result::Result<OwnedFd, SysError> {
    let cname = CString::new(name).map_err(|_| SysError::MemfdFailed(22))?;
    // SAFETY: memfd_create takes a valid NUL-terminated name pointer and
    // flag bits; the return is a raw fd checked for -1 before adoption.
    let ret = unsafe { libc::memfd_create(cname.as_ptr(), libc::MFD_CLOEXEC) };
    let fd = check(i64::from(ret), SysError::MemfdFailed)? as i32;
    // SAFETY: fd is a freshly created, unowned descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// A `buffer.release` fence for the headless path: an eventfd whose
/// counter starts at 1 — *already signalled* (readable ⇒ done), the
/// documented stand-in for a kernel sync-file.
///
/// # Errors
/// [`SysError::EventfdFailed`] with the errno.
pub fn eventfd_signalled() -> std::result::Result<OwnedFd, SysError> {
    // SAFETY: eventfd with EFD_CLOEXEC returns a raw fd checked for -1
    // before adoption.
    let ret = unsafe { libc::eventfd(1, libc::EFD_CLOEXEC) };
    let fd = check(i64::from(ret), SysError::EventfdFailed)? as i32;
    // SAFETY: fd is a freshly created, unowned descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Whether an eventfd fence is signalled (counter non-zero).
///
/// # Errors
/// [`LdpError::Io`] on a failed read.
pub fn fence_signalled(fd: &OwnedFd) -> Result<bool> {
    fence_signalled_raw(fd.as_raw_fd())
}

/// Whether an eventfd fence (referenced by raw descriptor) is
/// signalled — the borrow-only form for events whose descriptor the
/// caller does not own.
///
/// # Safety of the borrow
/// The descriptor must be valid for the duration of the call (the
/// caller's `FdList` outlives it).
///
/// # Errors
/// [`LdpError::Io`] on a failed read.
pub fn fence_signalled_raw(fd: i32) -> Result<bool> {
    let mut counter = [0u8; 8];
    // SAFETY: a plain read(2) of a valid fd into an 8-byte buffer sized
    // exactly to the eventfd contract; the return count is checked.
    unsafe {
        let got = libc::read(fd, counter.as_mut_ptr().cast(), counter.len());
        if got == -1 {
            let e = std::io::Error::last_os_error();
            if e.raw_os_error() == Some(libc::EAGAIN) {
                return Ok(false);
            }
            return Err(LdpError::Io(std::sync::Arc::new(e)));
        }
        Ok(u64::from_le_bytes(counter) > 0)
    }
}

// ---------------------------------------------------------------------------
// Shutdown signals — the serve loop's exit path.
// ---------------------------------------------------------------------------

/// The shutdown flag the signal handler sets (async-signal-safe: one
/// relaxed atomic store, nothing else).
static SHUTDOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

extern "C" fn on_shutdown_signal(_sig: libc::c_int) {
    // The only async-signal-safe thing to do: set the flag. The serve
    // loop's poll timeout re-checks it and unwinds through teardown.
    SHUTDOWN.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Install the shutdown handlers (SIGINT, SIGTERM) — call once before
/// entering a serve loop. Re-registration is idempotent by nature (the
/// same handler, the same flag); returns the previous state.
///
/// # Errors
/// [`LdpError::Io`] when `sigaction` is rejected by the kernel (an
/// effectively unreachable contract violation, surfaced honestly).
pub fn install_shutdown_handlers() -> Result<()> {
    // SAFETY: an all-zero sigset_t is the empty signal mask — the
    // documented valid representation (no bit patterns are invalid).
    let empty_mask = unsafe { std::mem::zeroed::<libc::sigset_t>() };
    let action = libc::sigaction {
        // The libc crate models the handler slot as an address
        // (sighandler_t); the extern "C" fn casts to it exactly.
        sa_sigaction: on_shutdown_signal as extern "C" fn(libc::c_int) as usize,
        sa_mask: empty_mask,
        sa_flags: 0,
        sa_restorer: None,
    };
    for sig in [libc::SIGINT, libc::SIGTERM] {
        // SAFETY: `action` is a valid sigaction struct for the call's
        // duration; the handler is async-signal-safe (one atomic
        // store); passing null for the old-action out-param discards
        // it by contract.
        let rc = unsafe {
            libc::sigaction(sig, std::ptr::addr_of!(action).cast(), std::ptr::null_mut())
        };
        if rc != 0 {
            return Err(LdpError::Io(std::sync::Arc::new(
                std::io::Error::last_os_error(),
            )));
        }
    }
    Ok(())
}

/// Whether a shutdown signal arrived — the serve loop's exit
/// condition, checked every poll turn.
#[must_use]
pub fn shutdown_requested() -> bool {
    SHUTDOWN.load(std::sync::atomic::Ordering::Relaxed)
}

/// Forward a stop signal to one owned child process (Phase 45: the
/// supervisor's graceful-stop arm). SIGTERM, not SIGKILL — the child
/// owns an honest teardown (DRM master dropped, buffers destroyed,
/// display dark) and deserves the chance to run it.
///
/// # Errors
///
/// [`LdpError::Io`] when `kill(2)` fails for a reason other than the
/// child already being gone (ESRCH — the caller's "nothing to stop").
pub fn terminate_child(pid: u32) -> Result<()> {
    // SAFETY: `kill(2)` with SIGTERM on a pid the caller owns as its
    // child; the signal number is SIGTERM by construction; the only
    // plausible failures are ESRCH (gone — mapped to Ok below) and
    // EPERM (not possible for one's own child before it is reaped).
    let rc = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
    if rc == 0 {
        return Ok(());
    }
    let err = std::io::Error::last_os_error();
    if err.kind() == std::io::ErrorKind::NotFound {
        // ESRCH: already gone — nothing to stop, not a failure.
        return Ok(());
    }
    Err(LdpError::Io(std::sync::Arc::new(err)))
}

#[cfg(test)]
mod shutdown_tests {
    use super::*;

    /// Reset the flag between assertions (tests share one process).
    fn reset() {
        SHUTDOWN.store(false, std::sync::atomic::Ordering::Relaxed);
    }

    #[test]
    fn handlers_install_and_flag_reads() {
        install_shutdown_handlers().expect("sigaction accepts the handler");
        // Nobody signalled us: the flag is down.
        assert!(!shutdown_requested());
        // The handler itself is async-signal-safe by construction; the
        // flag flips when called directly (as the kernel would), and
        // only then.
        on_shutdown_signal(libc::SIGINT);
        assert!(shutdown_requested());
        reset();
        assert!(!shutdown_requested());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn mapping_round_trips_pool_bytes() {
        let fd = memfd("sys-test").unwrap();
        let payload: [u8; 16] = [7; 16];
        let mut file = std::fs::File::from(OwnedFd::try_clone(&fd).unwrap());
        file.write_all(&payload).unwrap();
        drop(file);
        let map = map_read_only(&fd, 16).unwrap();
        assert_eq!(map.bytes(), &payload);
        assert_eq!(map.len(), 16);
    }

    #[test]
    fn zero_size_mapping_is_rejected() {
        let fd = memfd("sys-zero").unwrap();
        assert!(matches!(map_read_only(&fd, 0), Err(SysError::BadSize)));
    }

    #[test]
    fn page_size_is_sane() {
        let ps = page_size().unwrap();
        assert!(ps.is_power_of_two());
        assert!(ps >= 4096);
    }

    #[test]
    fn signalled_fence_reads_signalled() {
        let fd = eventfd_signalled().unwrap();
        assert!(fence_signalled(&fd).unwrap());
    }
}
