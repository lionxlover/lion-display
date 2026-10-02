//! Commit flags and plane coordinate encodings.
//!
//! The atomic commit flag word (checked against `drm_mode.h`: page-flip
//! event `0x01`, async `0x02`, TEST_ONLY `0x0100`, NONBLOCK `0x0200`,
//! ALLOW_MODESET `0x0400`), the 16.16 fixed-point source-crop encoding
//! with its pixel-space constructor, the signed destination rectangle,
//! and the legacy DPMS enum. These are the value *encodings* of atomic
//! requests; the request builder itself is [`crate::atomic`].

#![forbid(unsafe_code)]

use crate::error::{DisplayError, RejectReason, Result};

/// Commit flag bits.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct CommitFlags(pub u32);

impl CommitFlags {
    /// Request a page-flip event per touched active CRTC.
    pub const PAGE_FLIP_EVENT: Self = Self(0x01);
    /// Flip as soon as the new buffer is ready, tearing be damned —
    /// the async-flip path (`DRM_MODE_PAGE_FLIP_ASYNC`). Surfaces opt
    /// in through `presentation_mode = immediate`; the ldp-vrr
    /// tearing gate decides whether the flag may be set at all.
    pub const PAGE_FLIP_ASYNC: Self = Self(0x02);
    /// Commit asynchronously; a busy CRTC yields `EBUSY` (mapped to
    /// [`RejectReason::Busy`]) instead of blocking.
    pub const NONBLOCK: Self = Self(0x0200);
    /// Allow the commit to change the mode, ACTIVE, or plane/connector
    /// bindings — anything that stalls the pipe.
    pub const ALLOW_MODESET: Self = Self(0x0400);
    /// Validate only; no state changes.
    pub const TEST_ONLY: Self = Self(0x0100);

    /// Whether TEST_ONLY is set.
    #[must_use]
    pub const fn is_test_only(self) -> bool {
        self.0 & Self::TEST_ONLY.0 != 0
    }

    /// Whether NONBLOCK is set.
    #[must_use]
    pub const fn is_nonblock(self) -> bool {
        self.0 & Self::NONBLOCK.0 != 0
    }

    /// Whether ALLOW_MODESET is set.
    #[must_use]
    pub const fn allows_modeset(self) -> bool {
        self.0 & Self::ALLOW_MODESET.0 != 0
    }

    /// Whether page-flip events were requested.
    #[must_use]
    pub const fn wants_flip_event(self) -> bool {
        self.0 & Self::PAGE_FLIP_EVENT.0 != 0
    }

    /// Whether the async-flip (tearing) path was requested.
    #[must_use]
    pub const fn is_async_flip(self) -> bool {
        self.0 & Self::PAGE_FLIP_ASYNC.0 != 0
    }
}

/// Flag words combine by union (the usual bitset convention).
impl core::ops::BitOr for CommitFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

/// In-place union.
impl core::ops::BitOrAssign for CommitFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

/// Source crop in pixels (converted to 16.16 fixed point on the wire).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SrcRect {
    /// Crop origin x, pixels.
    pub x: u32,
    /// Crop origin y, pixels.
    pub y: u32,
    /// Crop width, pixels (> 0).
    pub w: u32,
    /// Crop height, pixels (> 0).
    pub h: u32,
}

impl SrcRect {
    /// Validate: positive size.
    ///
    /// # Errors
    /// [`RejectReason::ZeroSize`] when `w` or `h` is zero.
    pub fn new(x: u32, y: u32, w: u32, h: u32) -> Result<Self> {
        if w == 0 || h == 0 {
            return Err(DisplayError::AtomicReject {
                reason: RejectReason::ZeroSize,
                detail: "source crop".into(),
            });
        }
        Ok(Self { x, y, w, h })
    }

    /// The 16.16 wire value of x.
    #[must_use]
    pub fn wire_x(self) -> u64 {
        u64::from(self.x) << 16
    }
    /// The 16.16 wire value of y.
    #[must_use]
    pub fn wire_y(self) -> u64 {
        u64::from(self.y) << 16
    }
    /// The 16.16 wire value of w.
    #[must_use]
    pub fn wire_w(self) -> u64 {
        u64::from(self.w) << 16
    }
    /// The 16.16 wire value of h.
    #[must_use]
    pub fn wire_h(self) -> u64 {
        u64::from(self.h) << 16
    }
}

/// Destination rectangle on the CRTC (signed position).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DstRect {
    /// Destination origin x.
    pub x: i32,
    /// Destination origin y.
    pub y: i32,
    /// Destination width.
    pub w: u32,
    /// Destination height.
    pub h: u32,
}

impl DstRect {
    /// Validate: positive size.
    ///
    /// # Errors
    /// [`RejectReason::ZeroSize`] when `w` or `h` is zero.
    pub fn new(x: i32, y: i32, w: u32, h: u32) -> Result<Self> {
        if w == 0 || h == 0 {
            return Err(DisplayError::AtomicReject {
                reason: RejectReason::ZeroSize,
                detail: "destination rect".into(),
            });
        }
        Ok(Self { x, y, w, h })
    }
}

/// DPMS power state (the legacy connector enum).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum DpmsState {
    /// On.
    On,
    /// Standby.
    Standby,
    /// Suspend.
    Suspend,
    /// Off.
    Off,
}

impl DpmsState {
    /// Enum wire value.
    #[must_use]
    pub const fn wire(self) -> u64 {
        match self {
            Self::On => 0,
            Self::Standby => 1,
            Self::Suspend => 2,
            Self::Off => 3,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_point_wire_values() {
        let src = SrcRect::new(3, 7, 640, 480).unwrap();
        assert_eq!(src.wire_x(), 0x3_0000);
        assert_eq!(src.wire_y(), 0x7_0000);
        assert_eq!(src.wire_w(), 640 << 16);
        assert_eq!(src.wire_h(), 480 << 16);
    }
    #[test]
    fn flag_constants_match_kernel() {
        assert_eq!(CommitFlags::PAGE_FLIP_EVENT.0, 0x01);
        assert_eq!(CommitFlags::PAGE_FLIP_ASYNC.0, 0x02);
        assert_eq!(CommitFlags::TEST_ONLY.0, 0x0100);
        assert_eq!(CommitFlags::NONBLOCK.0, 0x0200);
        assert_eq!(CommitFlags::ALLOW_MODESET.0, 0x0400);
    }

    #[test]
    fn dpms_wire_values() {
        assert_eq!(DpmsState::On.wire(), 0);
        assert_eq!(DpmsState::Standby.wire(), 1);
        assert_eq!(DpmsState::Suspend.wire(), 2);
        assert_eq!(DpmsState::Off.wire(), 3);
    }
}
