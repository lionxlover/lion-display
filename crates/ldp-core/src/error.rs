//! Error taxonomy.
//!
//! The stable wire codes of `docs/protocol.md` §8 live in [`ErrorCode`];
//! [`LdpError`] is the in-memory error shared across crates. Design rules:
//!
//! * protocol *invalidity* (the peer misbehaved) and *denial* (security
//!   said no) are distinct kinds — [`ErrorCode::Unauthorized`] never
//!   carries a why in its message and is always audited by the server,
//! * transport-level framing failures map onto
//!   [`ErrorCode::MalformedMessage`] / [`ErrorCode::FdMismatch`] so the
//!   taxonomy is one list, not two,
//! * errors are `Send + 'static` and cheap to clone (no backtraces on the
//!   hot path; the server logs context at the error site instead).

use core::fmt;
use std::sync::Arc;

/// Stable numeric wire error codes. All of them are fatal to the
/// connection; the server delivers `connection.error(code, object,
/// message)` and closes the socket.
///
/// The numbering is frozen ABI: append, never renumber.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum ErrorCode {
    /// Object ID not bound on this connection.
    InvalidObject,
    /// Generation mismatch: destroyed-and-reused ID.
    StaleObject,
    /// Object exists but is not of the required interface.
    InvalidInterface,
    /// Opcode not defined for the object's interface version.
    InvalidOpcode,
    /// Argument types do not match the opcode signature.
    SignatureMismatch,
    /// Framing, alignment, or tag violation.
    MalformedMessage,
    /// FD count/index inconsistency between header and ancillary data.
    FdMismatch,
    /// Non-UTF-8 or over-length string.
    InvalidString,
    /// Message/array/string/object/FD/resource ceiling exceeded.
    LimitExceeded,
    /// Capability check failed (always audited; message never leaks why).
    Unauthorized,
    /// Request illegal in the object's current state.
    InvalidState,
    /// Value outside the declared domain.
    OutOfRange,
    /// Version negotiation failed.
    UnsupportedVersion,
    /// Buffer/geometry/format/modifier rejected.
    InvalidBuffer,
    /// Internal failure; the client should reconnect.
    ServerError,
}

impl ErrorCode {
    /// Stable wire number (1-based, per `docs/protocol.md` §8).
    #[must_use]
    pub const fn to_wire(self) -> u32 {
        match self {
            Self::InvalidObject => 1,
            Self::StaleObject => 2,
            Self::InvalidInterface => 3,
            Self::InvalidOpcode => 4,
            Self::SignatureMismatch => 5,
            Self::MalformedMessage => 6,
            Self::FdMismatch => 7,
            Self::InvalidString => 8,
            Self::LimitExceeded => 9,
            Self::Unauthorized => 10,
            Self::InvalidState => 11,
            Self::OutOfRange => 12,
            Self::UnsupportedVersion => 13,
            Self::InvalidBuffer => 14,
            Self::ServerError => 15,
        }
    }

    /// Parse a wire number. Unknown codes (future extensions) map to
    /// `None` — receivers must treat them as fatal-but-unnamed.
    #[must_use]
    pub const fn from_wire(code: u32) -> Option<ErrorCode> {
        match code {
            1 => Some(Self::InvalidObject),
            2 => Some(Self::StaleObject),
            3 => Some(Self::InvalidInterface),
            4 => Some(Self::InvalidOpcode),
            5 => Some(Self::SignatureMismatch),
            6 => Some(Self::MalformedMessage),
            7 => Some(Self::FdMismatch),
            8 => Some(Self::InvalidString),
            9 => Some(Self::LimitExceeded),
            10 => Some(Self::Unauthorized),
            11 => Some(Self::InvalidState),
            12 => Some(Self::OutOfRange),
            13 => Some(Self::UnsupportedVersion),
            14 => Some(Self::InvalidBuffer),
            15 => Some(Self::ServerError),
            _ => None,
        }
    }

    /// Stable kebab-case name (used in logs and audit records).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidObject => "invalid_object",
            Self::StaleObject => "stale_object",
            Self::InvalidInterface => "invalid_interface",
            Self::InvalidOpcode => "invalid_opcode",
            Self::SignatureMismatch => "signature_mismatch",
            Self::MalformedMessage => "malformed_message",
            Self::FdMismatch => "fd_mismatch",
            Self::InvalidString => "invalid_string",
            Self::LimitExceeded => "limit_exceeded",
            Self::Unauthorized => "unauthorized",
            Self::InvalidState => "invalid_state",
            Self::OutOfRange => "out_of_range",
            Self::UnsupportedVersion => "unsupported_version",
            Self::InvalidBuffer => "invalid_buffer",
            Self::ServerError => "server_error",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Which protocol limit was exceeded (context for
/// [`ErrorCode::LimitExceeded`]). One finite list, no strings — limits are
/// structural, not free-form.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum LimitKind {
    /// Total message size (header + payload).
    MessageBytes,
    /// FDs carried by one message.
    FdsPerMessage,
    /// Open FDs held by one client.
    ClientFds,
    /// One string argument.
    StringBytes,
    /// Live objects owned by one client.
    ClientObjects,
    /// Rects in one region/damage argument.
    RegionRects,
    /// Queued outbound event bytes for one client.
    EventQueueBytes,
    /// Buffer bytes mapped for one client.
    ClientBufferBytes,
}

impl LimitKind {
    /// Stable kebab-case name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MessageBytes => "message_bytes",
            Self::FdsPerMessage => "fds_per_message",
            Self::ClientFds => "client_fds",
            Self::StringBytes => "string_bytes",
            Self::ClientObjects => "client_objects",
            Self::RegionRects => "region_rects",
            Self::EventQueueBytes => "event_queue_bytes",
            Self::ClientBufferBytes => "client_buffer_bytes",
        }
    }
}

impl fmt::Display for LimitKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The error type shared across LDP crates.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum LdpError {
    /// A protocol error as delivered on the wire: fatal for the connection.
    Protocol {
        /// Stable code.
        code: ErrorCode,
        /// Offending object, when one is implicated.
        object: Option<crate::ids::ObjectId>,
        /// Human-readable diagnostic (safe to log, never shown to end users).
        message: Arc<str>,
    },
    /// A protocol limit was exceeded (fatal, [`ErrorCode::LimitExceeded`]).
    Limit {
        /// Which limit.
        kind: LimitKind,
        /// The offending measured value.
        value: u64,
    },
    /// A peer violated wire encoding rules not tied to a single object.
    Malformed {
        /// Wire code (typically [`ErrorCode::MalformedMessage`]).
        code: ErrorCode,
        /// Diagnostic.
        message: Arc<str>,
    },
    /// Underlying transport I/O failure (socket error, unexpected EOF).
    Io(Arc<std::io::Error>),
    /// A deadline or fence wait timed out (non-fatal at the protocol layer;
    /// callers decide policy).
    TimedOut {
        /// What timed out, in diagnostic form.
        what: &'static str,
    },
    /// An internal invariant was violated — a bug, never untrusted input.
    Logic {
        /// The violated invariant.
        what: &'static str,
    },
}

impl LdpError {
    /// Build a protocol error with a static/owned message.
    pub fn protocol(
        code: ErrorCode,
        object: Option<crate::ids::ObjectId>,
        message: impl Into<String>,
    ) -> Self {
        Self::Protocol {
            code,
            object,
            message: Arc::from(message.into().as_str()),
        }
    }

    /// Build a malformed-wire error.
    pub fn malformed(code: ErrorCode, message: impl Into<String>) -> Self {
        Self::Malformed {
            code,
            message: Arc::from(message.into().as_str()),
        }
    }

    /// Stable wire code if this error is representable on the wire.
    #[must_use]
    pub fn wire_code(&self) -> Option<ErrorCode> {
        match self {
            Self::Protocol { code, .. } | Self::Malformed { code, .. } => Some(*code),
            Self::Limit { .. } => Some(ErrorCode::LimitExceeded),
            Self::Io(_) | Self::TimedOut { .. } | Self::Logic { .. } => None,
        }
    }

    /// Whether the connection must be dropped when this error occurs.
    #[must_use]
    pub fn is_fatal(&self) -> bool {
        matches!(
            self,
            Self::Protocol { .. } | Self::Limit { .. } | Self::Malformed { .. }
        )
    }
}

impl fmt::Display for LdpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Protocol {
                code,
                object,
                message,
            } => {
                write!(f, "protocol error {code}")?;
                if let Some(o) = object {
                    write!(f, " (object {o:?})")?;
                }
                write!(f, ": {message}")
            }
            Self::Limit { kind, value } => write!(f, "limit exceeded: {kind} = {value}"),
            Self::Malformed { code, message } => write!(f, "malformed message ({code}): {message}"),
            Self::Io(e) => write!(f, "transport io error: {e}"),
            Self::TimedOut { what } => write!(f, "timed out waiting for {what}"),
            Self::Logic { what } => write!(f, "internal invariant violated: {what}"),
        }
    }
}

impl std::error::Error for LdpError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(&**e as &(dyn std::error::Error + 'static)),
            _ => None,
        }
    }
}

impl From<std::io::Error> for LdpError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(Arc::new(e))
    }
}

/// Result alias used throughout the LDP crates.
pub type Result<T> = core::result::Result<T, LdpError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_codes_round_trip_and_are_dense() {
        for code in 1..=15u32 {
            let parsed = ErrorCode::from_wire(code).unwrap_or_else(|| {
                panic!("code {code} must parse");
            });
            assert_eq!(parsed.to_wire(), code, "round trip failed for {code}");
        }
        assert!(ErrorCode::from_wire(0).is_none());
        assert!(ErrorCode::from_wire(16).is_none());
        assert!(ErrorCode::from_wire(u32::MAX).is_none());
    }

    #[test]
    fn names_are_unique() {
        let codes = [
            ErrorCode::InvalidObject,
            ErrorCode::StaleObject,
            ErrorCode::InvalidInterface,
            ErrorCode::InvalidOpcode,
            ErrorCode::SignatureMismatch,
            ErrorCode::MalformedMessage,
            ErrorCode::FdMismatch,
            ErrorCode::InvalidString,
            ErrorCode::LimitExceeded,
            ErrorCode::Unauthorized,
            ErrorCode::InvalidState,
            ErrorCode::OutOfRange,
            ErrorCode::UnsupportedVersion,
            ErrorCode::InvalidBuffer,
            ErrorCode::ServerError,
        ];
        let mut names: Vec<&str> = codes.iter().map(|c| c.as_str()).collect();
        names.sort_unstable();
        let count = names.len();
        names.dedup();
        assert_eq!(names.len(), count, "duplicate error names");
    }

    #[test]
    fn fatal_classification() {
        let e = LdpError::protocol(ErrorCode::Unauthorized, None, "scope denied");
        assert!(e.is_fatal());
        assert_eq!(e.wire_code(), Some(ErrorCode::Unauthorized));
        let e = LdpError::Limit {
            kind: LimitKind::MessageBytes,
            value: 2_000_000,
        };
        assert!(e.is_fatal());
        assert_eq!(e.wire_code(), Some(ErrorCode::LimitExceeded));
        let e = LdpError::TimedOut { what: "fence" };
        assert!(!e.is_fatal());
        assert_eq!(e.wire_code(), None);
    }

    #[test]
    fn display_is_informative() {
        let e = LdpError::protocol(ErrorCode::StaleObject, None, "generation mismatch");
        assert!(e.to_string().contains("stale_object"));
        let e = LdpError::Limit {
            kind: LimitKind::RegionRects,
            value: 8192,
        };
        assert!(e.to_string().contains("region_rects"));
    }
}
