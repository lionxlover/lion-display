//! The audited syscall layer for the remote relay.
//!
//! Every `unsafe` in this crate lives here, each call carrying a
//! `SAFETY` comment — the `ldp-transport` / `ldp-tools` precedent. All
//! other modules are `#![forbid(unsafe_code)]`, so the crate root does
//! not forbid (a forbid cannot be lifted per module).
//!
//! The relay needs exactly five operations the standard library does
//! not expose (or not with the required semantics):
//!
//! * [`memfd`] — reconstructing relayed pool/snapshot content as
//!   shared-memory files,
//! * [`eventfd`] — reconstructing relayed eventfd fences,
//! * [`pipe`] — substituting streaming descriptors (clipboard-class
//!   pipes) on the far side of the network,
//! * [`poll_readable`] — sampling an eventfd's signal state without
//!   consuming it (a plain read would drain the counter),
//! * [`shutdown_socket`] — tearing a wedged socket out of a blocking
//!   syscall so a dead peer's session thread can join.
//!
//! Everything else (reads, writes, seeks, `ftruncate` via
//! [`std::fs::File::set_len`]) uses safe `std` wrappers over the same
//! descriptors.

use std::ffi::CString;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

use ldp_core::error::{LdpError, Result};

/// A raw-syscall failure, classified.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SysError {
    /// `memfd_create` failed (errno captured).
    MemfdFailed(i32),
    /// `eventfd` failed (errno captured).
    EventfdFailed(i32),
    /// `pipe2` failed (errno captured).
    PipeFailed(i32),
    /// `poll` failed (errno captured).
    PollFailed(i32),
    /// `shutdown` failed (errno captured).
    ShutdownFailed(i32),
    /// A name contained an interior NUL.
    BadName,
}

impl SysError {
    /// The captured errno (0 for the name error).
    #[must_use]
    pub fn errno(self) -> i32 {
        match self {
            Self::MemfdFailed(e)
            | Self::EventfdFailed(e)
            | Self::PipeFailed(e)
            | Self::PollFailed(e)
            | Self::ShutdownFailed(e) => e,
            Self::BadName => 0,
        }
    }
}

impl std::fmt::Display for SysError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MemfdFailed(e) => write!(f, "memfd_create failed (errno {e})"),
            Self::EventfdFailed(e) => write!(f, "eventfd failed (errno {e})"),
            Self::PipeFailed(e) => write!(f, "pipe2 failed (errno {e})"),
            Self::PollFailed(e) => write!(f, "poll failed (errno {e})"),
            Self::ShutdownFailed(e) => write!(f, "shutdown failed (errno {e})"),
            Self::BadName => f.write_str("name contains an interior NUL"),
        }
    }
}

impl std::error::Error for SysError {}

impl From<SysError> for LdpError {
    fn from(e: SysError) -> LdpError {
        LdpError::Io(std::sync::Arc::new(std::io::Error::other(format!(
            "ldp-remote sys: {e}"
        ))))
    }
}

/// Create an anonymous shared-memory file (`memfd_create`), owned and
/// close-on-exec. The name is cosmetic (`/proc/<pid>/fd` listings).
///
/// # Errors
/// [`SysError::BadName`] for interior NULs; [`SysError::MemfdFailed`]
/// with the errno otherwise.
pub fn memfd(name: &str) -> std::result::Result<OwnedFd, SysError> {
    let cname = CString::new(name).map_err(|_| SysError::BadName)?;
    // SAFETY: memfd_create takes a valid NUL-terminated name pointer and
    // flag bits; the return is a raw fd checked for -1 before adoption.
    let ret = unsafe { libc::memfd_create(cname.as_ptr(), libc::MFD_CLOEXEC) };
    if ret == -1 {
        return Err(SysError::MemfdFailed(os_errno()));
    }
    // SAFETY: `ret` is a freshly created, unowned descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(ret) })
}

/// Create an eventfd (counter semantics, blocking mode), owned.
///
/// # Errors
/// [`SysError::EventfdFailed`] with the errno.
pub fn eventfd() -> std::result::Result<OwnedFd, SysError> {
    // SAFETY: eventfd takes an initial counter and flag bits; the
    // return is a raw fd checked for -1 before adoption.
    let ret = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC) };
    if ret == -1 {
        return Err(SysError::EventfdFailed(os_errno()));
    }
    // SAFETY: `ret` is a freshly created, unowned descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(ret) })
}

/// Create a pipe with close-on-exec ends: `(read, write)`.
///
/// # Errors
/// [`SysError::PipeFailed`] with the errno.
pub fn pipe() -> std::result::Result<(OwnedFd, OwnedFd), SysError> {
    let mut fds = [0i32; 2];
    // SAFETY: pipe2 takes a two-int out array and flag bits; on success
    // both slots hold fresh descriptors adopted below.
    let ret = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) };
    if ret == -1 {
        return Err(SysError::PipeFailed(os_errno()));
    }
    // SAFETY: on success both slots are freshly created, unowned
    // descriptors; adoption order defines the (read, write) pair.
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

/// Whether a descriptor is readable *right now*, sampled without
/// consuming anything (the eventfd-counter probe: `read(2)` would drain
/// the wakeup; `poll(2)` only looks).
///
/// # Errors
/// [`SysError::PollFailed`] with the errno; [`LdpError::Logic`] if the
/// kernel reports an invalid-request condition (a caller bug).
pub fn poll_readable(fd: i32) -> Result<bool> {
    let mut pfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: poll takes an initialized pollfd array pointer, a count
    // matching it, and a zero timeout; the return count is checked and
    // the revents field is read only on a non-error return.
    let ret = unsafe { libc::poll(&mut pfd, 1, 0) };
    if ret == -1 {
        return Err(LdpError::from(SysError::PollFailed(os_errno())));
    }
    if pfd.revents & libc::POLLNVAL != 0 {
        return Err(LdpError::Logic {
            what: "poll_readable: invalid descriptor",
        });
    }
    Ok(pfd.revents & libc::POLLIN != 0)
}

/// Shut a socket down in both directions (`shutdown(SHUT_RDWR)`),
/// unblocking any thread parked in `recv`/`send` on it. Used only for
/// session teardown (dead-peer keepalive expiry, sibling pump wake-up);
/// ordinary data flow never calls it.
///
/// # Errors
/// [`SysError::ShutdownFailed`] with the errno.
pub fn shutdown_socket(fd: i32) -> std::result::Result<(), SysError> {
    // SAFETY: shutdown takes a valid descriptor and a how constant; it
    // owns no pointers.
    let ret = unsafe { libc::shutdown(fd, libc::SHUT_RDWR) };
    if ret == -1 {
        return Err(SysError::ShutdownFailed(os_errno()));
    }
    Ok(())
}

/// The thread's last errno as a plain `i32`.
fn os_errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// Grow a memfd to `new_size` (pools never shrink; call sites enforce
/// monotone growth).
///
/// # Errors
/// [`LdpError::Io`] on `ftruncate` failure.
pub fn grow_memfd(fd: &OwnedFd, new_size: u64) -> Result<()> {
    std::fs::File::from(fd.try_clone().map_err(io("growing memfd"))?)
        .set_len(new_size)
        .map_err(io("growing memfd"))
}

/// Read a descriptor's whole content to EOF (snapshot semantics: the
/// sender must have completed the content before passing — memfd-class
/// files). The file cursor is rewound afterwards.
///
/// # Errors
/// [`LdpError::Io`] on read/seek failure.
pub fn read_whole(fd: &OwnedFd) -> Result<Vec<u8>> {
    use std::io::{Read as _, Seek as _, SeekFrom};
    let mut file = fd_clone(fd)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(io("reading fd content"))?;
    file.seek(SeekFrom::Start(0))
        .map_err(io("rewinding fd content"))?;
    Ok(bytes)
}

/// Read exactly `len` bytes at `offset` (pool-range semantics: the
/// committed window of a pool). The cursor is rewound afterwards.
///
/// # Errors
/// [`LdpError::Io`] on read/seek failure (including short reads —
/// a pool smaller than the client's own declared geometry is a
/// protocol violation, and the relay fails it exactly as the server
/// would).
pub fn read_range(fd: &OwnedFd, offset: u64, len: u64) -> Result<Vec<u8>> {
    use std::io::{Read as _, Seek as _, SeekFrom};
    let mut file = fd_clone(fd)?;
    file.seek(SeekFrom::Start(offset))
        .map_err(io("seeking pool range"))?;
    let mut bytes = vec![0u8; len as usize];
    file.read_exact(&mut bytes)
        .map_err(io("reading pool range"))?;
    file.seek(SeekFrom::Start(0))
        .map_err(io("rewinding pool range"))?;
    Ok(bytes)
}

/// Write `bytes` at `offset` into a descriptor (the far side of
/// [`read_range`]). The cursor is rewound afterwards.
///
/// # Errors
/// [`LdpError::Io`] on write/seek failure.
pub fn write_range(fd: &OwnedFd, offset: u64, bytes: &[u8]) -> Result<()> {
    use std::io::{Seek as _, SeekFrom, Write as _};
    let mut file = fd_clone(fd)?;
    file.seek(SeekFrom::Start(offset))
        .map_err(io("seeking pool update"))?;
    file.write_all(bytes).map_err(io("writing pool update"))?;
    file.seek(SeekFrom::Start(0))
        .map_err(io("rewinding pool update"))?;
    Ok(())
}

/// Fill a fresh memfd with `bytes`, rewound, ready to pass as pool or
/// snapshot content.
///
/// # Errors
/// [`LdpError`] from memfd creation or the fill.
pub fn filled_memfd(name: &str, bytes: &[u8]) -> Result<OwnedFd> {
    use std::io::{Seek as _, Write as _};
    let fd = memfd(name)?;
    let mut sink = std::fs::File::from(fd);
    sink.write_all(bytes).map_err(io("filling memfd"))?;
    sink.flush().map_err(io("flushing memfd"))?;
    sink.rewind().map_err(io("rewinding memfd"))?;
    Ok(sink.into())
}

/// Create an eventfd carrying `counter` (0 leaves it unsignalled).
///
/// # Errors
/// [`LdpError`] from eventfd creation or the counter write.
pub fn eventfd_with(counter: u64) -> Result<OwnedFd> {
    use std::io::Write as _;
    let fd = eventfd()?;
    if counter > 0 {
        std::fs::File::from(fd.try_clone().map_err(io("arming eventfd"))?)
            .write_all(&counter.to_le_bytes())
            .map_err(io("arming eventfd"))?;
    }
    Ok(fd)
}

/// A poll readability probe over an owned fd (borrows it only).
///
/// # Errors
/// See [`poll_readable`].
pub fn owned_poll_readable(fd: &OwnedFd) -> Result<bool> {
    poll_readable(fd.as_raw_fd())
}

/// Clone the file description so the caller can seek/read without
/// disturbing the original's cursor.
fn fd_clone(fd: &OwnedFd) -> Result<std::fs::File> {
    Ok(std::fs::File::from(
        fd.try_clone().map_err(io("duplicating fd"))?,
    ))
}

fn io(what: &'static str) -> impl Fn(std::io::Error) -> LdpError {
    move |e| {
        LdpError::Io(std::sync::Arc::new(std::io::Error::other(format!(
            "ldp-remote sys: {what}: {e}"
        ))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filled_memfd_round_trips() {
        let fd = filled_memfd("remote-sys-test", b"payload").unwrap();
        assert_eq!(read_whole(&fd).unwrap(), b"payload");
    }

    #[test]
    fn interior_nul_name_is_rejected() {
        assert_eq!(memfd("bad\0name").unwrap_err(), SysError::BadName);
    }

    #[test]
    fn eventfd_counter_semantics() {
        let fd = eventfd_with(7).unwrap();
        assert!(owned_poll_readable(&fd).unwrap());
        use std::io::Read as _;
        let mut counter = [0u8; 8];
        std::fs::File::from(fd).read_exact(&mut counter).unwrap();
        assert_eq!(u64::from_le_bytes(counter), 7);

        let cold = eventfd_with(0).unwrap();
        assert!(!owned_poll_readable(&cold).unwrap());
    }

    #[test]
    fn pipe_carries_both_directions() {
        let (r, w) = pipe().unwrap();
        use std::io::{Read as _, Write as _};
        std::fs::File::from(w).write_all(b"through").unwrap();
        let mut got = Vec::new();
        let mut reader = std::fs::File::from(r);
        reader.read_to_end(&mut got).unwrap();
        assert_eq!(got, b"through");
    }

    #[test]
    fn grow_memfd_extends_content() {
        let fd = filled_memfd("grow-test", b"12345678").unwrap();
        grow_memfd(&fd, 16).unwrap();
        assert_eq!(read_whole(&fd).unwrap().len(), 16);
        assert_eq!(read_range(&fd, 8, 8).unwrap(), [0u8; 8]);
    }

    #[test]
    fn range_io_round_trips() {
        let fd = filled_memfd("range-test", &[0u8; 64]).unwrap();
        write_range(&fd, 8, b"middle").unwrap();
        assert_eq!(read_range(&fd, 8, 6).unwrap(), b"middle");
        assert_eq!(read_whole(&fd).unwrap()[8..14], *b"middle");
    }

    #[test]
    fn range_read_past_eof_fails() {
        let fd = filled_memfd("short-pool", &[0u8; 4]).unwrap();
        assert!(read_range(&fd, 0, 16).is_err());
    }

    #[test]
    fn poll_on_closed_fd_reports_error_or_invalid() {
        let fd = eventfd().unwrap();
        let raw = fd.as_raw_fd();
        drop(fd);
        // Either EBADF (error) or POLLNVAL (Logic) — never `Ok`.
        assert!(poll_readable(raw).is_err());
    }
}
