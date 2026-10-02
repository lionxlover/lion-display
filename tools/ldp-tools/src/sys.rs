//! The audited syscall layer for the tool family.
//!
//! Every `unsafe` in this crate lives here, each call carrying a
//! `SAFETY` comment — the `ldp-transport`/`lion-compositor` precedent.
//! All other modules are `#![forbid(unsafe_code)]`, so the crate root
//! does not forbid (a forbid cannot be lifted per module).
//!
//! Tools need exactly two syscalls the standard library does not
//! expose:
//!
//! * [`memfd`] — anonymous shared-memory files for client-side shm
//!   pools (`ldp.core.shm.create_pool`); pixels are written through the
//!   safe [`std::fs::File`] wrapper after creation.
//! * [`fence_signalled_raw`] — reading a `buffer.release` eventfd fence
//!   borrowed from an event's descriptor list (readability means "the
//!   compositor is done reading the buffer").

use std::ffi::CString;
use std::os::fd::{FromRawFd, OwnedFd};

use ldp_core::error::{LdpError, Result};

/// A raw-syscall failure, classified.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SysError {
    /// `memfd_create` failed (errno captured).
    MemfdFailed(i32),
    /// A fence read failed with an errno other than `EAGAIN`.
    FenceReadFailed(i32),
    /// The memfd name contained an interior NUL.
    BadName,
}

impl SysError {
    /// The captured errno (0 for the name error).
    #[must_use]
    pub fn errno(self) -> i32 {
        match self {
            Self::MemfdFailed(e) | Self::FenceReadFailed(e) => e,
            Self::BadName => 0,
        }
    }
}

impl std::fmt::Display for SysError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MemfdFailed(e) => write!(f, "memfd_create failed (errno {e})"),
            Self::FenceReadFailed(e) => write!(f, "fence read failed (errno {e})"),
            Self::BadName => f.write_str("memfd name contains an interior NUL"),
        }
    }
}

impl std::error::Error for SysError {}

impl From<SysError> for LdpError {
    fn from(e: SysError) -> LdpError {
        LdpError::Io(std::sync::Arc::new(std::io::Error::other(format!(
            "ldp-tools sys: {e}"
        ))))
    }
}

/// Create an anonymous shared-memory file (`memfd_create`), owned.
///
/// The name is cosmetic (it shows up in `/proc/<pid>/fd` listings);
/// the descriptor is close-on-exec.
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
        return Err(SysError::MemfdFailed(
            std::io::Error::last_os_error().raw_os_error().unwrap_or(0),
        ));
    }
    // SAFETY: `ret` is a freshly created, unowned descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(ret) })
}

/// Whether an eventfd fence (referenced by raw descriptor) is
/// signalled — the borrow-only form for events whose descriptor the
/// caller does not own (the event queue closes it after dispatch).
///
/// # Safety of the borrow
/// The descriptor must be valid for the duration of the call (the
/// owning `FdList` outlives it).
///
/// # Errors
/// [`LdpError::Io`] on a failed read other than `EAGAIN` (which is
/// "not signalled").
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

/// A memfd filled with `bytes`, ready to hand to
/// `shm.create_pool(fd, size)`.
///
/// # Errors
/// [`LdpError::Io`] on write failures; see [`memfd`] for creation
/// failures.
pub fn filled_memfd(name: &str, bytes: &[u8]) -> Result<OwnedFd> {
    use std::io::{Seek as _, Write as _};
    let fd = memfd(name)?;
    let mut sink = std::fs::File::from(fd);
    sink.write_all(bytes).map_err(|e| {
        LdpError::Io(std::sync::Arc::new(std::io::Error::other(format!(
            "filling memfd '{name}': {e}"
        ))))
    })?;
    sink.flush().map_err(|e| {
        LdpError::Io(std::sync::Arc::new(std::io::Error::other(format!(
            "flushing memfd '{name}': {e}"
        ))))
    })?;
    // Rewind so every consumer (tests, later readers, the pool
    // exporter duplicating the descriptor) starts at byte 0 — the
    // cursor rides along with duplicated fds.
    sink.rewind().map_err(|e| {
        LdpError::Io(std::sync::Arc::new(std::io::Error::other(format!(
            "rewinding memfd '{name}': {e}"
        ))))
    })?;
    Ok(sink.into())
}

/// Read a descriptor's whole content **without moving its offset**
/// (read-once snapshot content: capture frames, keymaps, ICC profiles).
///
/// `pread`-based: the original descriptor's shared file position is
/// untouched, so the event queue's later close is the only disposition.
///
/// # Errors
/// [`LdpError::Io`] on read failures.
pub fn read_all_raw(fd: i32) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    // Heap chunk (clippy's large-array limit); 64 KiB amortizes the
    // pread calls over multi-megabyte frames.
    let mut chunk = vec![0u8; 65_536];
    let chunk = &mut chunk[..];
    let mut offset: usize = 0;
    loop {
        // SAFETY: pread(2) of a valid fd into the chunk buffer at a
        // known byte offset; the return is the count read (-1 on error),
        // and pread never moves the descriptor's file position.
        let got = unsafe {
            libc::pread(
                fd,
                chunk.as_mut_ptr().cast(),
                chunk.len(),
                offset as libc::off_t,
            )
        };
        if got == -1 {
            return Err(LdpError::Io(std::sync::Arc::new(
                std::io::Error::last_os_error(),
            )));
        }
        if got == 0 {
            return Ok(out);
        }
        let got = got as usize;
        out.extend_from_slice(&chunk[..got]);
        offset += got;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read as _;

    #[test]
    fn filled_memfd_round_trips() {
        let fd = filled_memfd("tools-sys-test", b"payload-bytes").unwrap();
        let mut file = std::fs::File::from(fd);
        let mut read_back = Vec::new();
        file.read_to_end(&mut read_back).unwrap();
        assert_eq!(read_back, b"payload-bytes");
    }

    #[test]
    fn read_all_raw_reads_a_snapshot_without_consuming_the_original() {
        let fd = filled_memfd("tools-sys-snap", b"frame-bytes").unwrap();
        // First read: through the raw interface.
        let first = read_all_raw(std::os::fd::AsRawFd::as_raw_fd(&fd)).unwrap();
        assert_eq!(first, b"frame-bytes");
        // The original descriptor still delivers (the duplicate shared
        // its offset but reads-to-EOF here again from wherever it stands;
        // a fresh read of a rewound snapshot returns the same bytes).
        let second = read_all_raw(std::os::fd::AsRawFd::as_raw_fd(&fd)).unwrap();
        assert_eq!(second, b"frame-bytes");
    }

    #[test]
    fn interior_nul_name_is_rejected() {
        assert_eq!(memfd("bad\0name").unwrap_err(), SysError::BadName);
    }
}
