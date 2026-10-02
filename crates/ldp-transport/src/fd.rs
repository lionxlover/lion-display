//! FD hygiene.
//!
//! `docs/architecture.md` §6: every FD has exactly one owner; received
//! FDs are validated with `fcntl(F_GETFD)` before ownership transfers;
//! the transport closes the FDs of every rejected message. [`FdList`] is
//! the ownership vehicle: raw cmsg numbers are validated and adopted
//! into it immediately after `recvmsg`, and dropping it — on any error
//! path — closes everything it holds.
//!
//! [`count_open_fds`] exists for the leak exit criterion: tests snapshot
//! the process FD table and assert it is unchanged across whole suites
//! (10k round-trips, FD bombs, slow peers).

#![forbid(unsafe_code)]

use std::fs;
use std::os::fd::{OwnedFd, RawFd};

/// Validate a raw FD number with `fcntl(F_GETFD)` before adopting it.
///
/// The kernel only puts live FDs into a received `SCM_RIGHTS` array, but
/// defense in depth is cheap here and the call also detects FDs closed
/// by a racing thread between reception and adoption.
#[must_use]
pub fn validate(raw: RawFd) -> bool {
    crate::sys::fd_is_open(raw)
}

/// Number of open FDs in this process, from `/proc/self/fd`.
///
/// The directory listing itself holds one FD (the `readdir` handle), which
/// is subtracted. Callers asserting stability must serialize concurrent
/// FD churn (the integration tests do so with a mutex) because this is a
/// process-wide count.
///
/// # Errors
///
/// [`std::io::Error`] when `/proc` is unreadable (not mounted).
pub fn count_open_fds() -> std::io::Result<usize> {
    let count = fs::read_dir("/proc/self/fd")?.count();
    Ok(count.saturating_sub(1))
}

/// An owned, ordered batch of FDs — the message ancillary array as one
/// RAII unit.
///
/// Adoption path: raw numbers from a cmsg pass [`validate`] and
/// [`FdList::adopt`]; from then on each FD has exactly one owner (the
/// list). Errors aborting a message drop the list, closing everything.
/// Sending moves FDs out with [`FdList::take_all`] so the kernel
/// consumes them exactly once.
#[derive(Debug, Default)]
pub struct FdList {
    fds: Vec<OwnedFd>,
}

impl FdList {
    /// A new empty batch.
    #[must_use]
    pub const fn new() -> FdList {
        FdList { fds: Vec::new() }
    }

    /// Number of FDs held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.fds.len()
    }

    /// Whether the batch is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.fds.is_empty()
    }

    /// Push an already-owned FD.
    pub fn push(&mut self, fd: OwnedFd) {
        self.fds.push(fd);
    }

    /// Wrap an already-owned batch (the inverse of [`FdList::take_all`]).
    #[must_use]
    pub fn from_vec(fds: Vec<OwnedFd>) -> FdList {
        FdList { fds }
    }

    /// Validate and adopt a raw FD number received in a cmsg.
    ///
    /// On validation failure the raw FD is closed by the syscall layer
    /// — it was kernel-allocated for this process by `recvmsg`, so
    /// somebody must own it and this is the rejection point.
    ///
    /// # Errors
    ///
    /// [`LdpError::Io`](ldp_core::error::LdpError::Io)(ldp_core::error::LdpError::Io) when `F_GETFD`
    /// reports the FD dead.
    pub fn adopt(&mut self, raw: RawFd) -> ldp_core::error::Result<()> {
        self.fds.push(crate::sys::adopt_validated(raw)?);
        Ok(())
    }

    /// Take the whole batch (used when handing FDs to `sendmsg`).
    #[must_use]
    pub fn take_all(&mut self) -> Vec<OwnedFd> {
        std::mem::take(&mut self.fds)
    }

    /// Raw FD numbers for the cmsg encoder (borrowed; ownership stays
    /// with the list until [`FdList::take_all`] hands it over).
    #[must_use]
    pub fn raw_fds(&self) -> Vec<RawFd> {
        self.fds
            .iter()
            .map(std::os::fd::AsRawFd::as_raw_fd)
            .collect()
    }

    /// A single raw FD by position, or `None` when out of range.
    #[must_use]
    pub fn raw_at(&self, index: usize) -> Option<RawFd> {
        self.fds.get(index).map(std::os::fd::AsRawFd::as_raw_fd)
    }

    /// Remove and return the FD at `index` (for extracting a specific
    /// received FD, e.g. the argument at fd-index `k`).
    #[must_use]
    pub fn remove(&mut self, index: usize) -> Option<OwnedFd> {
        if index < self.fds.len() {
            Some(self.fds.remove(index))
        } else {
            None
        }
    }

    /// Iterate borrowed FDs.
    pub fn iter(&self) -> impl Iterator<Item = &OwnedFd> {
        self.fds.iter()
    }

    /// Explicitly close everything now (equivalent to drop, kept for
    /// symmetry with the hygiene narrative).
    pub fn close_all(&mut self) {
        self.fds.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::{AsRawFd, IntoRawFd};

    fn fresh_fd() -> RawFd {
        // A real, live fd created for this test; `FdList` closes it.
        crate::sys::eventfd_owned().unwrap().into_raw_fd()
    }

    #[test]
    fn validate_accepts_live_fds_and_rejects_garbage() {
        let fd = fresh_fd();
        assert!(validate(fd));
        crate::sys::close_raw(fd);
        assert!(!validate(-1));
        assert!(!validate(i32::MAX));
    }

    #[test]
    fn adopt_owns_and_drops_close() {
        // Eight adopted fds; a clean drop of the list (no IO-safety
        // abort) proves single ownership of every one. The leak-count
        // gate lives in tests/hygiene.rs where counts are serialized.
        {
            let mut list = FdList::new();
            for _ in 0..8 {
                list.adopt(fresh_fd()).unwrap();
            }
            assert_eq!(list.len(), 8);
        }
    }

    #[test]
    fn adopt_rejects_dead_fd_and_closes_it() {
        let mut list = FdList::new();
        assert!(list.adopt(-1).is_err());
        assert_eq!(list.len(), 0);
    }

    #[test]
    fn take_all_and_raw_view() {
        let mut list = FdList::new();
        for _ in 0..3 {
            list.adopt(fresh_fd()).unwrap();
        }
        let raws = list.raw_fds();
        assert_eq!(raws.len(), 3);
        let taken = list.take_all();
        assert!(list.is_empty());
        assert_eq!(taken.len(), 3);
        assert!(list.raw_fds().is_empty());
    }

    #[test]
    fn remove_by_index() {
        let mut list = FdList::new();
        let a = fresh_fd();
        let b = fresh_fd();
        list.adopt(a).unwrap();
        list.adopt(b).unwrap();
        assert_eq!(list.raw_at(0), Some(a));
        assert_eq!(list.raw_at(1), Some(b));
        assert_eq!(list.raw_at(2), None);
        let removed = list.remove(0).unwrap();
        assert_eq!(removed.as_raw_fd(), a);
        assert_eq!(list.len(), 1);
        assert!(list.remove(5).is_none());
        drop(removed);
    }
}
