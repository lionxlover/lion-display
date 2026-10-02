//! Device events — what the hardware tells us.
//!
//! Three kinds matter to a compositor: page flips (the scanout of a
//! submitted state became visible at a timestamped vblank, with a
//! sequence number to order events), hotplug (connector topology
//! changed; re-probe), and vblank ticks (animation phase anchors where
//! no flip landed).
//!
//! Timestamps are [`Mono`] nanoseconds — the same domain the frame
//! scheduler (§10.3) reasons in; the mock device's injected clock and
//! the kernel's `drmHandleEvent` seconds/microseconds feed the same
//! type, so downstream math is backend-independent.

#![forbid(unsafe_code)]

use std::os::fd::AsRawFd;

use crate::ids::{ConnectorId, CrtcId};
use ldp_core::time::Mono;

/// Extra flags on a page-flip event.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct PageFlipFlags(pub u32);

impl PageFlipFlags {
    /// Nothing notable.
    pub const NONE: Self = Self(0);
}

/// A page-flip completion: the committed state is now on screen.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PageFlipEvent {
    /// Which CRTC flipped.
    pub crtc: CrtcId,
    /// Monotonic flip sequence for this CRTC (starts at 1).
    pub sequence: u64,
    /// The vblank timestamp at which the new state became visible.
    pub timestamp: Mono,
    /// Event flags.
    pub flags: PageFlipFlags,
    /// The out-fence for this flip, when `OUT_FENCE_PTR` was requested.
    pub out_fence: Option<OutFence>,
}

/// A vblank tick with no associated flip.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct VblankEvent {
    /// Which CRTC ticked.
    pub crtc: CrtcId,
    /// Monotonic vblank sequence (counts every vblank, flips or not).
    pub sequence: u64,
    /// The vblank timestamp.
    pub timestamp: Mono,
}

/// A connector topology change; re-probe the connector.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct HotplugEvent {
    /// Which connector changed.
    pub connector: ConnectorId,
}

/// One decoded device event.
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum DeviceEvent {
    /// A page flip completed.
    PageFlip(PageFlipEvent),
    /// A bare vblank (no flip landed this period).
    Vblank(VblankEvent),
    /// Connector status changed (cable, sink power).
    Hotplug(HotplugEvent),
}

/// The out-fence of a committed flip.
///
/// The real DRM backend writes the kernel's sync-file fd into a slot it
/// owns and hands it over as [`Self::Fd`]; the mock device mints
/// deterministic fence tokens and hands them over as [`Self::Token`].
/// Both are "wait for scanout of the committed state" handles; the type
/// keeps the provenance visible so the compositor can route them to the
/// right fence driver (Phase 9's `ldp-gpu` sync layer accepts both).
#[derive(Debug)]
pub enum OutFence {
    /// A real sync-file descriptor (kernel fence).
    Fd(std::os::fd::OwnedFd),
    /// A mock fence token (virtual timeline; ldp-gpu's mock sync driver
    /// can wait on it).
    Token(u64),
}

impl Clone for OutFence {
    fn clone(&self) -> Self {
        // OwnedFd duplication keeps the clone usable; a failed dup is a
        // resource exhaustion error we surface as the tokenless clone of
        // a closed fence, which reads as already-signaled.
        match self {
            Self::Fd(fd) => fd.try_clone().map_or(Self::Token(u64::MAX), Self::Fd),
            Self::Token(t) => Self::Token(*t),
        }
    }
}

impl PartialEq for OutFence {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            // Descriptor identity: same underlying fd number (both sides
            // stem from the same minted fence or unrelated).
            (Self::Fd(a), Self::Fd(b)) => a.as_raw_fd() == b.as_raw_fd(),
            (Self::Token(a), Self::Token(b)) => a == b,
            _ => false,
        }
    }
}

impl Eq for OutFence {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_shapes() {
        let crtc = CrtcId::new(7).unwrap();
        let ev = DeviceEvent::PageFlip(PageFlipEvent {
            crtc,
            sequence: 3,
            timestamp: Mono::from_ns(16_666_666),
            flags: PageFlipFlags::NONE,
            out_fence: Some(OutFence::Token(9)),
        });
        match ev {
            DeviceEvent::PageFlip(f) => {
                assert_eq!(f.sequence, 3);
                assert_eq!(f.timestamp.as_ns(), 16_666_666);
                assert!(matches!(f.out_fence, Some(OutFence::Token(9))));
            }
            _ => panic!("wrong variant"),
        }
        let vb = DeviceEvent::Vblank(VblankEvent {
            crtc,
            sequence: 11,
            timestamp: Mono::from_ns(33_333_332),
        });
        assert_eq!(
            vb,
            DeviceEvent::Vblank(VblankEvent {
                crtc,
                sequence: 11,
                timestamp: Mono::from_ns(33_333_332)
            })
        );
    }
}
