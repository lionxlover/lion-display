//! MIT-SHM: segment registry over a host-supplied shared-memory seam.
//!
//! The protocol side (attach/detach/put/get/create-pixmap) is pure
//! state: segment XIDs, read-only flags, and the byte ranges requests
//! name. The actual shared pages belong to the process binary — the
//! crate opens no mappings — supplied through [`ShmHost`]:
//!
//! * [`ShmHost::read_segment`] copies `len` bytes at `offset` out of
//!   the segment the process mapped for `ShmAttach`.
//! * [`ShmHost::write_segment`] copies bytes in (the `ShmGetImage`
//!   reply path).
//!
//! A read past the segment's end answers `None`, which the dispatcher
//! maps to `BadAccess`. `ShmCreatePixmap` snapshots its region into a
//! normal backing store at creation (the subset does not alias
//! pixmap pixels with the segment; PutImage into such a pixmap does
//! not write back — documented deviation, exercised by tests).

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

/// The process-binary seam for actual shared memory.
pub trait ShmHost {
    /// Read `len` bytes at `offset` of segment `seg`; `None` when the
    /// range is outside the mapping.
    fn read_segment(&mut self, seg: u32, offset: usize, len: usize) -> Option<Vec<u8>>;
    /// Write `data` at `offset` of segment `seg`; `None` when the
    /// range is outside the mapping.
    fn write_segment(&mut self, seg: u32, offset: usize, data: &[u8]) -> Option<()>;
    /// An `ShmAttach` arrived: map the SysV segment `shmid` under the
    /// XID `seg` (`read_only` per the request). The default ignores
    /// it — tests that pre-populate segments never see one.
    fn attach_segment(&mut self, seg: u32, shmid: u64, read_only: bool) {
        let _ = (seg, shmid, read_only);
    }
}

/// A host that owns no segments (the default for tests that never
/// touch SHM).
#[derive(Clone, Copy, Debug, Default)]
pub struct NoShm;

impl ShmHost for NoShm {
    fn read_segment(&mut self, _seg: u32, _offset: usize, _len: usize) -> Option<Vec<u8>> {
        None
    }
    fn write_segment(&mut self, _seg: u32, _offset: usize, _data: &[u8]) -> Option<()> {
        None
    }
}

/// The segment registry (ShmAttach/ShmDetach bookkeeping).
#[derive(Clone, Debug, Default)]
pub struct ShmState {
    segments: BTreeMap<u32, bool>, // seg -> read-only
}

impl ShmState {
    /// ShmAttach: register a segment XID.
    ///
    /// # Errors
    /// [`ShmError::Duplicate`] when the XID is already attached.
    pub fn attach(&mut self, seg: u32, read_only: bool) -> Result<(), ShmError> {
        if self.segments.contains_key(&seg) {
            return Err(ShmError::Duplicate(seg));
        }
        self.segments.insert(seg, read_only);
        Ok(())
    }

    /// ShmDetach: drop a segment.
    ///
    /// # Errors
    /// [`ShmError::Unknown`] when the XID is not attached.
    pub fn detach(&mut self, seg: u32) -> Result<(), ShmError> {
        if self.segments.remove(&seg).is_some() {
            Ok(())
        } else {
            Err(ShmError::Unknown(seg))
        }
    }

    /// Whether a segment is attached and writable.
    #[must_use]
    pub fn writable(&self, seg: u32) -> bool {
        self.segments.get(&seg).is_some_and(|&ro| !ro)
    }

    /// Whether a segment is attached at all.
    #[must_use]
    pub fn exists(&self, seg: u32) -> bool {
        self.segments.contains_key(&seg)
    }
}

/// Segment registry errors (mapped to X errors by the dispatcher).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ShmError {
    /// The XID is already attached (`BadAccess`).
    Duplicate(u32),
    /// The XID is not attached (`BadValue`).
    Unknown(u32),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attach_detach_lifecycle() {
        let mut s = ShmState::default();
        s.attach(0x1000, false).unwrap();
        assert_eq!(s.attach(0x1000, false), Err(ShmError::Duplicate(0x1000)));
        assert!(s.exists(0x1000));
        assert!(s.writable(0x1000));
        s.attach(0x1001, true).unwrap();
        assert!(!s.writable(0x1001));
        s.detach(0x1000).unwrap();
        assert_eq!(s.detach(0x1000), Err(ShmError::Unknown(0x1000)));
    }

    #[test]
    fn no_shm_host_answers_none() {
        let mut h = NoShm;
        assert!(h.read_segment(1, 0, 4).is_none());
        assert!(h.write_segment(1, 0, &[0; 4]).is_none());
    }
}
