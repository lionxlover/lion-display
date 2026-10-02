//! Syscall layer — this binary's *only* module containing `unsafe`.
//!
//! The bridge process owns exactly three syscall families the pure
//! crates left as seams: multiplexed readiness ([`poll`]), the
//! MIT-SHM SysV segment mapping ([`ShmMap`]), and nothing else —
//! socket I/O with ancillary data already lives in
//! `ldp-transport`'s audited `sys` module, which this crate calls
//! through its public API. Every `unsafe` block below carries a
//! `SAFETY` comment stating the invariant the call relies on, the
//! `ldp-transport` precedent; the rest of this crate is
//! `#![forbid(unsafe_code)]`.
//!
//! `poll(2)` readiness flags are exposed as plain `bool`s so the
//! state machines stay safe-code.

#![allow(clippy::similar_names)]

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};

/// Readiness interest in one descriptor.
#[derive(Clone, Copy, Debug)]
pub struct Interest {
    /// The descriptor.
    pub fd: RawFd,
    /// Watch for readable data.
    pub read: bool,
    /// Watch for writable space (only while a backlog is pending).
    pub write: bool,
}

/// The observed readiness of one descriptor (one `poll` turn).
#[derive(Clone, Copy, Debug, Default)]
pub struct Ready {
    /// The descriptor this row describes.
    pub fd: RawFd,
    /// Data (or EOF) is readable.
    pub readable: bool,
    /// The kernel accepts more bytes.
    pub writable: bool,
    /// Hangup or error: the session owning this fd must close.
    pub gone: bool,
}

/// One `poll(2)` turn over `set` (a zero timeout is a pure drain).
///
/// # Errors
///
/// [`std::io::Error`] when the `poll` syscall itself fails (EINTR is
/// retried internally — signal deliveries do not end the loop).
pub fn poll(set: &[Interest], timeout_ms: i32) -> io::Result<Vec<Ready>> {
    let mut fds: Vec<libc::pollfd> = set
        .iter()
        .map(|i| libc::pollfd {
            fd: i.fd,
            events: libc::POLLIN | (i16::from(i.write) * libc::POLLOUT),
            revents: 0,
        })
        .collect();
    loop {
        // SAFETY: `fds` is a valid, exclusive slice of `pollfd` for the
        // duration of the call; `poll` does not retain the pointer.
        // Timeout is bounded by the caller (<= 250 ms), so an EINTR
        // retry cannot extend a hang beyond one signal.
        let rc = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout_ms) };
        if rc < 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        return Ok(fds
            .iter()
            .map(|p| Ready {
                fd: p.fd,
                readable: (p.revents & (libc::POLLIN | libc::POLLRDNORM)) != 0,
                writable: (p.revents & (libc::POLLOUT | libc::POLLWRNORM)) != 0,
                gone: (p.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL)) != 0,
            })
            .collect());
    }
}

/// Mint an anonymous in-memory descriptor of `size` bytes (memfd).
///
/// The descriptor serves both sharing roles: the keymap blob (one per
/// keyboard event) and the LDP pool export (one per foreign window).
///
/// # Errors
///
/// [`std::io::Error`] when `memfd_create` or the sizing `ftruncate`
/// fails.
pub fn memfd(size: u64) -> io::Result<OwnedFd> {
    let name = b"lion-bridge\0";
    // SAFETY: `name` is a NUL-terminated static byte string (a valid
    // C string for the call's duration); `MFD_CLOEXEC` is a defined
    // flag bit; the returned fd (or -1) is handled immediately.
    let fd = unsafe {
        libc::syscall(
            libc::SYS_memfd_create,
            name.as_ptr().cast::<libc::c_char>(),
            libc::MFD_CLOEXEC,
        )
    };
    let fd: i32 = i32::try_from(fd)
        .map_err(|_| io::Error::other("memfd_create returned an out-of-range descriptor"))?;
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` is a positive, freshly-minted descriptor that
    // nobody else owns — taking it into `OwnedFd` moves that
    // ownership with no double-close.
    let owned = unsafe { OwnedFd::from_raw_fd(fd) };
    ftruncate(owned.as_raw_fd(), size)?;
    Ok(owned)
}

/// Size a descriptor (`ftruncate`).
///
/// # Errors
///
/// [`std::io::Error`] when the syscall fails (a closed or read-only
/// descriptor).
pub fn ftruncate(fd: RawFd, size: u64) -> io::Result<()> {
    let len = libc::off_t::try_from(size)
        .map_err(|_| io::Error::other("the requested size overflows off_t"))?;
    // SAFETY: `fd` is a live descriptor owned by the caller for the
    // duration of the call; `len` is a plain value argument.
    if unsafe { libc::ftruncate(fd, len) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Bind and listen on an abstract Unix socket (the Wayland
/// `@name` form), returning a standard listener.
///
/// # Errors
///
/// [`std::io::Error`] when the socket, bind, or listen syscall fails.
pub fn bind_abstract(name: &[u8]) -> io::Result<std::os::unix::net::UnixListener> {
    let addr = ldp_transport::UnixAddr::abstract_name(name)
        .map_err(|e| io::Error::other(e.to_string()))?;
    let fd = ldp_transport::sys::raw_socket(true).map_err(io_of)?;
    ldp_transport::sys::raw_bind(fd.as_raw_fd(), &addr).map_err(io_of)?;
    ldp_transport::sys::raw_listen(fd.as_raw_fd(), 16).map_err(io_of)?;
    // SAFETY: `fd` is a fresh, bound, listening descriptor owned
    // exclusively by this function; wrapping it moves that ownership
    // into the listener (its drop closes the socket). The descriptor
    // is nonblocking, matching the listener's expectation.
    Ok(unsafe { std::os::unix::net::UnixListener::from_raw_fd(IntoRawFd::into_raw_fd(fd)) })
}

/// Convert the transport's error into the plain I/O kind (the sys
/// helpers speak `io::Result`).
fn io_of(e: ldp_core::error::LdpError) -> io::Error {
    io::Error::other(e.to_string())
}

/// One mapped SysV shared-memory segment (MIT-SHM backing).
///
/// The X client creates the segment (`shmget`) and attaches it into
/// the bridge (`shmat`) once; the pure crate's `ShmHost` seam then
/// reads and writes through this mapping. The mapping detaches on
/// drop — the segment itself stays owned by the client (or the
/// operator's namespace).
pub struct ShmMap {
    ptr: *mut libc::c_void,
    len: usize,
}

impl ShmMap {
    /// Attach segment `shmid` and measure it (`IPC_STAT`).
    ///
    /// # Errors
    ///
    /// [`std::io::Error`] when the segment does not exist, is
    /// permissioned away, or `IPC_STAT` refuses (the mapping is
    /// rolled back before returning — no leak).
    pub fn attach(shmid: libc::key_t) -> io::Result<ShmMap> {
        // SAFETY: `shmat` with a null address and flags 0 asks the
        // kernel for a fresh read-write mapping of a valid shmid; a
        // failure returns MAP_FAILED without side effects. The
        // returned pointer is exclusively ours (no other holder in
        // this process) and lives until `shmdt` in `Drop`.
        let ptr = unsafe { libc::shmat(shmid, std::ptr::null::<libc::c_void>(), 0) };
        if ptr == libc::MAP_FAILED {
            return Err(std::io::Error::last_os_error());
        }
        let info: libc::shmid_ds = {
            // SAFETY: a zeroed `shmid_ds` is valid storage for
            // `IPC_STAT` output; the kernel fills it wholesale.
            let mut info = std::mem::MaybeUninit::<libc::shmid_ds>::uninit();
            // SAFETY: `shmid` was just attached successfully and `info`
            // is valid, exclusive storage for the call's output.
            let rc = unsafe { libc::shmctl(shmid, libc::IPC_STAT, info.as_mut_ptr()) };
            if rc < 0 {
                // SAFETY: `ptr` came from a successful `shmat` above
                // and has not been detached yet.
                unsafe { libc::shmdt(ptr) };
                return Err(std::io::Error::last_os_error());
            }
            // SAFETY: `IPC_STAT` succeeded, so the whole struct is
            // initialized kernel output.
            unsafe { info.assume_init() }
        };
        let len = info.shm_segsz as usize;
        if len == 0 {
            // SAFETY: as above — the successful `shmat` pointer.
            unsafe { libc::shmdt(ptr) };
            return Err(io::Error::other("shm segment reports zero size"));
        }
        Ok(ShmMap { ptr, len })
    }

    /// The segment's size in bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// The segment's size in bytes (`true` — a mapping always exists).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Read `count` bytes at `offset` (bounds-checked; `None` outside).
    #[must_use]
    pub fn read(&self, offset: usize, count: usize) -> Option<Vec<u8>> {
        let end = offset.checked_add(count)?;
        if end > self.len {
            return None;
        }
        let mut out = vec![0u8; count];
        // SAFETY: `offset + count <= len` was just proven, so the
        // source range is inside the mapped segment; `out` is a valid
        // exclusive destination of exactly `count` bytes; concurrent
        // writers are the X client's own contract (MIT-SHM semantics).
        let src = unsafe { self.ptr.cast::<u8>().add(offset) };
        unsafe { std::ptr::copy_nonoverlapping(src, out.as_mut_ptr(), count) };
        Some(out)
    }

    /// Write `data` at `offset` (bounds-checked; `None` outside).
    pub fn write(&mut self, offset: usize, data: &[u8]) -> Option<()> {
        let end = offset.checked_add(data.len())?;
        if end > self.len {
            return None;
        }
        // SAFETY: as `read` — the destination range is proven inside
        // the mapping and `data` is a valid source of its own length.
        let dst = unsafe { self.ptr.cast::<u8>().add(offset) };
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), dst, data.len()) };
        Some(())
    }
}

// SAFETY: `ShmMap` owns one kernel mapping exclusively (detached
// exactly once, in `Drop`); the pointer is valid for that lifetime and
// every access is bounds-checked against the kernel-reported size.
unsafe impl Send for ShmMap {}
// SAFETY: the mapping is immutable in size; data races on the segment
// are the X clients' own protocol semantics (the same races the real
// X server's SHM host has always had).
unsafe impl Sync for ShmMap {}

impl Drop for ShmMap {
    fn drop(&mut self) {
        // SAFETY: `ptr` came from a successful `shmat` in `attach`
        // and has not been detached since (the only `shmdt` sites are
        // the error paths inside `attach` and this drop).
        unsafe { libc::shmdt(self.ptr) };
    }
}

/// Create a private SysV segment (test-only; the production path
/// maps shmid's the X clients supply — the caller must
/// [`mark_shm_destroyed`] once a mapping holds it, or the segment
/// leaks).
#[cfg(test)]
pub fn create_shm_segment(size: usize) -> io::Result<i32> {
    // SAFETY: IPC_PRIVATE asks for a fresh key; the size is
    // page-sane; 0o600 is owner-only.
    let shmid = unsafe { libc::shmget(libc::IPC_PRIVATE, size, 0o600) };
    if shmid < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(shmid)
}

/// Mark a SysV segment for destruction (it dies when its last
/// mapping detaches — tests mark once a mapping holds the segment,
/// so they never leak one even on failing asserts).
#[cfg(test)]
pub fn mark_shm_destroyed(shmid: i32) -> io::Result<()> {
    // SAFETY: `shmid` is a live id the caller owns; a null ds pointer
    // is the documented IPC_RMID form.
    if unsafe {
        libc::shmctl(
            shmid,
            libc::IPC_RMID,
            std::ptr::null_mut::<libc::shmid_ds>(),
        )
    } < 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poll_reports_zero_timeout_drain() {
        let r = poll(&[], 0).expect("an empty poll returns immediately");
        assert!(r.is_empty());
    }

    #[test]
    fn poll_sees_a_readable_pipe() {
        let (mut a, b) = std::os::unix::net::UnixStream::pair().expect("pair");
        use std::io::Write as _;
        a.write_all(b"a").expect("write");
        use std::os::fd::AsRawFd;
        let set = [Interest {
            fd: b.as_raw_fd(),
            read: true,
            write: false,
        }];
        let ready = poll(&set, 50).expect("poll");
        assert!(ready[0].readable, "the written byte is readable");
    }

    #[test]
    fn shm_map_round_trips() {
        // A private SysV segment: create, attach, THEN mark for
        // destruction (the mapping keeps it alive until dropped —
        // the test never leaks one, and a fresh attach of a marked
        // segment is EINVAL on Linux, so the order is the doctrine).
        let shmid = create_shm_segment(4096).expect("the private segment");
        let mut map = ShmMap::attach(shmid).expect("attach");
        mark_shm_destroyed(shmid).expect("marked");
        assert_eq!(map.len(), 4096);
        assert!(map.write(4, b"bridge").is_some());
        assert_eq!(map.read(4, 6).as_deref(), Some(&b"bridge"[..]));
        assert!(map.read(4090, 8).is_none(), "past the end refuses");
        drop(map);
    }
}
