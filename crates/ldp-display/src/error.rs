//! Error taxonomy for the display backend.
//!
//! Every failure mode of [`crate`] collapses into [`DisplayError`]; the
//! atomic-commit rejections carry a typed [`RejectReason`] so callers can
//! react (retry after the pending flip, split the commit, ask for a
//! mode-set) instead of string-matching errno text. Raw errno values from
//! the libdrm layer are preserved in [`DisplayError::System`] for
//! diagnostics where no typed reason exists.

#![forbid(unsafe_code)]

use core::fmt;
use std::path::PathBuf;

/// Why an atomic commit was rejected — the typed half of
/// [`DisplayError::AtomicReject`].
///
/// The reasons mirror the checks real DRM performs in
/// `drm_atomic_check_only` and its callees, so the mock device and the
/// libdrm backend reject the same request for the same reason.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum RejectReason {
    /// A referenced object (CRTC/plane/connector/FB) does not exist.
    UnknownObject,
    /// A referenced property does not exist on that object.
    UnknownProperty,
    /// The property exists but its type rejects the value shape
    /// (blob into a range, object id into a blob, ...).
    PropertyType,
    /// A range property value lies outside `[min, max]`.
    ValueOutOfRange,
    /// An enum property value is not one of its tokens.
    InvalidEnumToken,
    /// A blob reference does not resolve to a live blob.
    BlobNotFound,
    /// The request writes an immutable property.
    Immutable,
    /// `NONBLOCK` was requested and a page flip is still pending on the
    /// CRTC (kernel `EBUSY`).
    Busy,
    /// The commit changes pipeline state that requires `ALLOW_MODESET`
    /// (mode change, active toggle, plane theft, connector rebinding).
    NeedsModeset,
    /// The plane's `possible_crtcs` mask does not include the CRTC.
    PlaneNotPossible,
    /// The framebuffer format is not in the plane's format list.
    FormatUnsupported,
    /// The framebuffer modifier is not supported for that format on the
    /// plane (the `IN_FORMATS` blob bit is clear).
    ModifierUnsupported,
    /// `SRC_*` (16.16 fixed point) extends past the framebuffer edges.
    SrcOutOfBounds,
    /// `SRC_W/H` or `CRTC_W/H` is zero while the plane is being enabled.
    ZeroSize,
    /// The mode blob is invalid or not offered by the connector.
    ModeNotSupported,
    /// The connector is already bound to a different CRTC.
    ConnectorBound,
    /// The connector is not connected to anything.
    ConnectorDisconnected,
    /// A CRTC was enabled without a connector bound to it.
    MissingConnector,
    /// `IN_FENCE_FD` is negative-other-than-−1 or not a live fence.
    InvalidInFence,
    /// The framebuffer handle set does not resolve.
    FbNotFound,
    /// The commit is internally inconsistent (same property twice, empty).
    Malformed,
}

impl RejectReason {
    /// The kernel errno this reason maps back to when it originates from
    /// real DRM. `Some(0)` never occurs; `None` means the check is
    /// client-side pre-flight only.
    #[must_use]
    pub const fn kernel_errno(self) -> Option<i32> {
        match self {
            Self::UnknownObject
            | Self::UnknownProperty
            | Self::PropertyType
            | Self::ValueOutOfRange
            | Self::InvalidEnumToken
            | Self::BlobNotFound
            | Self::Immutable
            | Self::NeedsModeset
            | Self::PlaneNotPossible
            | Self::FormatUnsupported
            | Self::ModifierUnsupported
            | Self::SrcOutOfBounds
            | Self::ZeroSize
            | Self::ModeNotSupported
            | Self::ConnectorBound
            | Self::ConnectorDisconnected
            | Self::MissingConnector
            | Self::InvalidInFence
            | Self::FbNotFound
            | Self::Malformed => Some(22), // EINVAL
            Self::Busy => Some(16), // EBUSY
        }
    }
}

impl fmt::Display for RejectReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::UnknownObject => "unknown object",
            Self::UnknownProperty => "unknown property",
            Self::PropertyType => "property type mismatch",
            Self::ValueOutOfRange => "value out of range",
            Self::InvalidEnumToken => "invalid enum token",
            Self::BlobNotFound => "blob not found",
            Self::Immutable => "immutable property",
            Self::Busy => "flip pending on crtc (nonblocking)",
            Self::NeedsModeset => "modeset required",
            Self::PlaneNotPossible => "plane cannot attach to crtc",
            Self::FormatUnsupported => "format unsupported by plane",
            Self::ModifierUnsupported => "modifier unsupported for format",
            Self::SrcOutOfBounds => "source rect outside framebuffer",
            Self::ZeroSize => "zero source or destination size",
            Self::ModeNotSupported => "mode not supported by connector",
            Self::ConnectorBound => "connector bound elsewhere",
            Self::ConnectorDisconnected => "connector disconnected",
            Self::MissingConnector => "active crtc without connector",
            Self::InvalidInFence => "invalid in-fence",
            Self::FbNotFound => "framebuffer not found",
            Self::Malformed => "malformed request",
        };
        f.write_str(s)
    }
}

/// Every failure of the display backend.
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum DisplayError {
    /// The shared library (`libdrm.so.2`, `libudev.so.1`) could not be
    /// loaded; the runtime layer is unavailable on this machine.
    LibraryLoad {
        /// SONAME that failed to load.
        library: &'static str,
    },
    /// The library loaded but a required symbol is missing (ABI too old).
    MissingSymbol {
        /// SONAME being searched.
        library: &'static str,
        /// The missing symbol name.
        symbol: &'static str,
    },
    /// `open()` on the device node failed (permissions, no DRM, absent
    /// node on headless machines).
    DeviceOpen {
        /// Path that was tried.
        path: PathBuf,
        /// Raw errno.
        errno: i32,
    },
    /// A KMS ioctl returned an error the typed reasons do not cover.
    System {
        /// Raw errno.
        errno: i32,
        /// What was being attempted.
        while_doing: &'static str,
    },
    /// The atomic commit failed validation; the device state is unchanged
    /// (all-or-nothing).
    AtomicReject {
        /// Typed reason.
        reason: RejectReason,
        /// Human context (object/property names involved).
        detail: String,
    },
    /// A queried object/property/blob/FB does not exist.
    NotFound {
        /// What was looked up.
        what: &'static str,
        /// The identifier that failed.
        id: u64,
    },
    /// Blob payload does not parse (mode blobs, `IN_FORMATS`, EDID).
    BadBlob {
        /// The blob kind.
        what: &'static str,
    },
    /// EDID structure is malformed (header, checksum, length).
    BadEdid {
        /// First offending offset.
        offset: usize,
    },
    /// Backlight sysfs read/write failed.
    BacklightIo {
        /// sysfs path.
        path: PathBuf,
        /// Raw errno (I/O) or -1 for parse failures.
        errno: i32,
    },
}

impl fmt::Display for DisplayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LibraryLoad { library } => write!(f, "cannot load {library}"),
            Self::MissingSymbol { library, symbol } => {
                write!(f, "{library} is missing symbol {symbol}")
            }
            Self::DeviceOpen { path, errno } => {
                write!(f, "cannot open device {}: errno {errno}", path.display())
            }
            Self::System { errno, while_doing } => {
                write!(f, "drm error while {while_doing}: errno {errno}")
            }
            Self::AtomicReject { reason, detail } => {
                write!(f, "atomic reject: {reason} ({detail})")
            }
            Self::NotFound { what, id } => write!(f, "{what} {id} not found"),
            Self::BadBlob { what } => write!(f, "malformed {what} blob"),
            Self::BadEdid { offset } => write!(f, "malformed EDID at byte {offset}"),
            Self::BacklightIo { path, errno } => {
                write!(f, "backlight io on {}: errno {errno}", path.display())
            }
        }
    }
}

impl std::error::Error for DisplayError {}

/// Convenience alias used across the crate.
pub type Result<T, E = DisplayError> = core::result::Result<T, E>;
