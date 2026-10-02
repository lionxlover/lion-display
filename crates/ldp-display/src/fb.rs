//! Framebuffer registration specs — the `drmModeAddFB2` request model.
//!
//! An [`FbSpec`] names the GEM handles, pitches, and offsets of a
//! buffer's planes plus its format; registering it (with or without a
//! modifier, the `drmModeAddFB2WithModifiers` distinction) mints the
//! [`FbId`](crate::ids::FbId) atomic requests reference from
//! `FB_ID`. Validation here is the cheap structural half — plane arity
//! against the format, nonzero geometry — mirroring the kernel's
//! pre-ioctl checks so malformed buffers fail before a driver sees them.

#![forbid(unsafe_code)]

use crate::error::{DisplayError, RejectReason, Result};
use ldp_core::buffer::FourCC;

/// A framebuffer as `drmModeAddFB2` sees it.
///
/// Handles are GEM object handles (driver namespace); mock devices mint
/// them through their own allocator.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FbSpec {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Pixel format.
    pub format: FourCC,
    /// Per-plane GEM handles (unused planes zero).
    pub handles: [u32; 4],
    /// Per-plane row pitches (unused planes zero).
    pub pitches: [u32; 4],
    /// Per-plane byte offsets (unused planes zero).
    pub offsets: [u32; 4],
}

impl FbSpec {
    /// Single-plane constructor with pitch and offset.
    #[must_use]
    pub fn single(
        width: u32,
        height: u32,
        format: FourCC,
        handle: u32,
        pitch: u32,
        offset: u32,
    ) -> Self {
        Self {
            width,
            height,
            format,
            handles: [handle, 0, 0, 0],
            pitches: [pitch, 0, 0, 0],
            offsets: [offset, 0, 0, 0],
        }
    }

    /// Planar constructor: per-plane `(handle, pitch, offset)` triples
    /// (`drmModeAddFB2`'s native shape — NV12 and friends). The
    /// slices must not exceed four entries; missing planes are zero.
    ///
    /// # Panics
    ///
    /// Never in release shapes: panics only when a slice carries more
    /// than the ABI's four plane slots (a caller bug by construction).
    #[must_use]
    pub fn planar(width: u32, height: u32, format: FourCC, planes: &[(u32, u32, u32)]) -> Self {
        let mut handles = [0u32; 4];
        let mut pitches = [0u32; 4];
        let mut offsets = [0u32; 4];
        for (i, &(handle, pitch, offset)) in planes.iter().enumerate().take(4) {
            handles[i] = handle;
            pitches[i] = pitch;
            offsets[i] = offset;
        }
        Self {
            width,
            height,
            format,
            handles,
            pitches,
            offsets,
        }
    }

    /// Build from one imported GEM handle plus a buffer geometry's
    /// plane layouts — the dma-buf registration shape (one object,
    /// per-plane offset/stride addressing). This is the constructor
    /// the scanout walk feeds: the pool's fd imports to a handle, the
    /// buffer's validated geometry names the planes.
    #[must_use]
    pub fn from_layout(
        width: u32,
        height: u32,
        format: FourCC,
        handle: u32,
        layouts: &[ldp_core::buffer::PlaneLayout],
    ) -> Self {
        let planes: Vec<(u32, u32, u32)> = layouts
            .iter()
            .map(|l| (handle, l.stride, l.offset))
            .collect();
        Self::planar(width, height, format, &planes)
    }

    /// Validity: positive geometry, plane-1..N nonzero for planar formats.
    ///
    /// # Errors
    /// [`RejectReason::Malformed`] with detail on the first problem.
    pub fn validate(&self) -> Result<()> {
        if self.width == 0 || self.height == 0 {
            return Err(DisplayError::AtomicReject {
                reason: RejectReason::Malformed,
                detail: "fb with zero extent".into(),
            });
        }
        let count = self.format.plane_count();
        for p in 0..count {
            let i = p as usize;
            if self.handles[i] == 0 || self.pitches[i] == 0 {
                return Err(DisplayError::AtomicReject {
                    reason: RejectReason::Malformed,
                    detail: format!("plane {p} missing handle/pitch"),
                });
            }
        }
        for i in count as usize..4 {
            if self.handles[i] != 0 {
                return Err(DisplayError::AtomicReject {
                    reason: RejectReason::Malformed,
                    detail: format!("plane {i} set beyond format arity"),
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fb_spec_validation() {
        let ok = FbSpec::single(64, 64, FourCC::XRGB8888, 1, 256, 0);
        ok.validate().unwrap();
        let zero = FbSpec::single(0, 64, FourCC::XRGB8888, 1, 256, 0);
        assert!(zero.validate().is_err());
        // NV12 needs two planes.
        let mut nv12 = FbSpec::single(64, 64, FourCC::NV12, 1, 64, 0);
        assert!(nv12.validate().is_err());
        nv12.handles[1] = 2;
        nv12.pitches[1] = 64;
        nv12.offsets[1] = 64 * 64;
        nv12.validate().unwrap();
        // A third handle set beyond NV12's arity.
        nv12.handles[2] = 3;
        assert!(nv12.validate().is_err());
    }
}
