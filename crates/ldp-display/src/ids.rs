//! KMS object identifiers and the CRTC mask algebra.
//!
//! DRM addresses every object by a driver-chosen `u32` id; these newtypes
//! keep the namespaces apart at the type level so a connector id can never
//! be silently passed where a plane id is expected. [`CrtcMask`] mirrors
//! the kernel's `possible_crtcs` encoding: bit *i* refers to the CRTC at
//! index *i* of the device's resource list.

#![forbid(unsafe_code)]

use core::fmt;

macro_rules! kms_id {
    ($(#[$doc:meta])* $name:ident, $what:literal) => {
        $(#[$doc])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
        #[repr(transparent)]
        pub struct $name(u32);

        impl $name {
            /// The raw KMS object id.
            #[must_use]
            pub const fn raw(self) -> u32 {
                self.0
            }

            /// Construct from a raw id (ids are opaque; zero is the
            /// conventional "none" and is rejected).
            #[must_use]
            pub const fn new(raw: u32) -> Option<Self> {
                if raw == 0 {
                    None
                } else {
                    Some(Self(raw))
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!($what, " #{}"), self.0)
            }
        }
    };
}

kms_id!(
    /// A connector object id (`drmModeConnector`).
    ConnectorId,
    "connector"
);
kms_id!(
    /// A CRTC object id (`drmModeCrtc`).
    CrtcId,
    "crtc"
);
kms_id!(
    /// A plane object id (`drmModePlane`).
    PlaneId,
    "plane"
);
kms_id!(
    /// An encoder object id (`drmModeEncoder`).
    EncoderId,
    "encoder"
);
kms_id!(
    /// A framebuffer object id (`drmModeFB`).
    FbId,
    "fb"
);
kms_id!(
    /// A property object id (`drmModePropertyRes`).
    PropId,
    "prop"
);
kms_id!(
    /// A property blob id (`drmModePropertyBlob`).
    BlobId,
    "blob"
);

/// The DRM object type discriminant used by
/// `drmModeObjectGetProperties` and blob lookups.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum ObjectType {
    /// A CRTC.
    Crtc,
    /// A plane.
    Plane,
    /// A connector.
    Connector,
    /// An encoder.
    Encoder,
    /// A framebuffer (property enums only).
    Framebuffer,
}

impl ObjectType {
    /// The kernel `DRM_OBJECT_*` magic constant used by
    /// `drmModeObjectGetProperties`.
    #[must_use]
    pub const fn kernel(self) -> u32 {
        match self {
            // DRM_MODE_OBJECT_* — deliberately distinctive constants so
            // ioctls catch type confusion (drm_mode.h §624).
            Self::Encoder => 0xe0e0_e0e0,
            Self::Connector => 0xc0c0_c0c0,
            Self::Crtc => 0xcccc_cccc,
            Self::Plane => 0xeeee_eeee,
            Self::Framebuffer => 0xfbfb_fbfb,
        }
    }
}

/// Any KMS object, for property addressing.
///
/// Atomic requests set properties on objects; this enum carries the
/// (object type, id) pair the backend resolves against its catalogs.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum AnyId {
    /// CRTC target.
    Crtc(CrtcId),
    /// Plane target.
    Plane(PlaneId),
    /// Connector target.
    Connector(ConnectorId),
}

impl AnyId {
    /// The object's type discriminant.
    #[must_use]
    pub const fn object_type(self) -> ObjectType {
        match self {
            Self::Crtc(_) => ObjectType::Crtc,
            Self::Plane(_) => ObjectType::Plane,
            Self::Connector(_) => ObjectType::Connector,
        }
    }

    /// The raw id, namespace-free (diagnostics only).
    #[must_use]
    pub const fn raw(self) -> u32 {
        match self {
            Self::Crtc(c) => c.raw(),
            Self::Plane(p) => p.raw(),
            Self::Connector(c) => c.raw(),
        }
    }
}

impl fmt::Display for AnyId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Crtc(c) => fmt::Display::fmt(c, f),
            Self::Plane(p) => fmt::Display::fmt(p, f),
            Self::Connector(c) => fmt::Display::fmt(c, f),
        }
    }
}

/// A `possible_crtcs` bitmask: bit *i* = the CRTC at resource-list
/// index *i*.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
#[repr(transparent)]
pub struct CrtcMask(u32);

impl CrtcMask {
    /// A mask that can address the first `n` CRTCs.
    #[must_use]
    pub const fn covering(n: u32) -> Self {
        if n >= 32 {
            Self(u32::MAX)
        } else {
            Self((1 << n) - 1)
        }
    }

    /// From a raw kernel mask.
    #[must_use]
    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    /// The raw mask.
    #[must_use]
    pub const fn raw(self) -> u32 {
        self.0
    }

    /// Whether CRTC index `i` is addressable.
    #[must_use]
    pub const fn contains(self, i: u32) -> bool {
        if i >= 32 {
            false
        } else {
            self.0 & (1 << i) != 0
        }
    }

    /// Iterate the set CRTC indices, ascending.
    #[must_use]
    pub const fn bits(self) -> BitIter {
        BitIter {
            mask: self.0,
            next: 0,
        }
    }

    /// Number of CRTCs addressable.
    #[must_use]
    pub const fn len(self) -> u32 {
        self.0.count_ones()
    }

    /// Whether no CRTC is addressable.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl fmt::Display for CrtcMask {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:#010x}", self.0)
    }
}

/// Ascending iterator over the indices set in a [`CrtcMask`].
#[derive(Clone, Debug)]
pub struct BitIter {
    mask: u32,
    next: u32,
}

impl Iterator for BitIter {
    type Item = u32;

    fn next(&mut self) -> Option<u32> {
        while self.next < 32 {
            let i = self.next;
            self.next += 1;
            if self.mask & (1 << i) != 0 {
                return Some(i);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_reject_zero() {
        assert!(ConnectorId::new(0).is_none());
        assert!(CrtcId::new(1).is_some());
        assert_eq!(PlaneId::new(42).unwrap().raw(), 42);
        assert_eq!(FbId::new(7).unwrap().to_string(), "fb #7");
    }

    #[test]
    fn object_types_match_kernel_constants() {
        assert_eq!(ObjectType::Connector.kernel(), 0xc0c0_c0c0);
        assert_eq!(ObjectType::Crtc.kernel(), 0xcccc_cccc);
        assert_eq!(ObjectType::Plane.kernel(), 0xeeee_eeee);
        assert_eq!(ObjectType::Encoder.kernel(), 0xe0e0_e0e0);
        assert_eq!(ObjectType::Framebuffer.kernel(), 0xfbfb_fbfb);
        assert_eq!(
            AnyId::Crtc(CrtcId::new(1).unwrap()).object_type(),
            ObjectType::Crtc
        );
    }

    #[test]
    fn crtc_mask_algebra() {
        let mask = CrtcMask::covering(3);
        assert_eq!(mask.raw(), 0b111);
        assert!(mask.contains(0) && mask.contains(2));
        assert!(!mask.contains(3) && !mask.contains(32));
        assert_eq!(mask.bits().collect::<Vec<_>>(), vec![0, 1, 2]);
        assert_eq!(mask.len(), 3);
        assert!(CrtcMask::default().is_empty());
        assert_eq!(CrtcMask::covering(32).raw(), u32::MAX);
        let single = CrtcMask::from_raw(1 << 5);
        assert_eq!(single.bits().collect::<Vec<_>>(), vec![5]);
    }
}
