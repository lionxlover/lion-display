//! Client-side error surface.
//!
//! The client distinguishes three failure families that call for
//! different application reactions:
//!
//! * [`ClientError::Ldp`] — the underlying layer error (codec, framing,
//!   limits, syscalls). Most are fatal for the connection; inspect the
//!   [`LdpError`] shape to decide.
//! * [`ClientError::Disconnected`] — the server went away. The kind
//!   tells the application whether state was lost mid-stream
//!   ([`DisconnectKind::Crash`]) or the shutdown was orderly
//!   ([`DisconnectKind::Clean`]).
//! * [`ClientError::ServerError`] — the server delivered a fatal
//!   `connection.error`. The connection is closed afterwards; the code
//!   and message explain the violation.
//!
//! Library misuse (sending before the handshake, allocating past the ID
//! space, double-destroy) is surfaced as dedicated variants so bugs in
//! application code do not masquerade as protocol failures.

use ldp_core::error::{ErrorCode, LdpError};
use std::fmt;

/// Why the connection ended.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DisconnectKind {
    /// The server closed the socket between messages (orderly EOF):
    /// every observed event was complete. Application state built from
    /// events is consistent up to the last delivered one.
    Clean,
    /// The server vanished mid-message (EOF or reset with a frame in
    /// flight): at least one truncated message was observed. Treat all
    /// session state as suspect.
    Crash,
    /// A transport failure that is not a peer disconnect (errno-level
    /// errors other than EOF/EPIPE).
    Transport,
    /// The connection was torn down locally: [`ClientError::ServerError`]
    /// was already delivered to the handler before this is reported.
    LocalDrop,
}

impl fmt::Display for DisconnectKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Clean => f.write_str("clean disconnect"),
            Self::Crash => f.write_str("peer crash mid-message"),
            Self::Transport => f.write_str("transport failure"),
            Self::LocalDrop => f.write_str("dropped locally after a fatal error"),
        }
    }
}

/// Everything that can go wrong on the client side.
#[derive(Clone, Debug)]
pub enum ClientError {
    /// An error from the layer below (codec / framing / transport /
    /// limits). Framing and codec failures are fatal for the connection
    /// per `docs/protocol.md` §8–9.
    Ldp(LdpError),
    /// The server disconnected. Reported once per connection lifetime;
    /// later sends and reads keep returning this error.
    Disconnected(DisconnectKind),
    /// The server sent `connection.error(code, object_id, message)` —
    /// fatal by definition; the socket is closed right after.
    ServerError {
        /// The wire taxonomy code (stable numeric meaning).
        code: ErrorCode,
        /// Offending object ID, 0 when none.
        object: u32,
        /// Human-readable diagnostic (not for display to end users).
        message: Box<str>,
    },
    /// An event arrived for an object this client has no proxy for
    /// (never bound, or already destroyed with confirmation). Under the
    /// v1 contract that is a server bug or a protocol violation; the
    /// client treats it as fatal, mirroring the server's own defense.
    UnexpectedEvent {
        /// The wire object ID the event targeted.
        target: u32,
    },
    /// The client-side object-ID space is exhausted (2^31 - 2 IDs
    /// allocated on one connection). A connection doing this many
    /// `new_id`s should reuse objects instead.
    IdExhausted,
    /// A request referenced a proxy this connection does not know
    /// (destroyed, revoked, or built for another connection).
    UnknownProxy {
        /// The wire object ID the caller tried to use.
        target: u32,
    },
    /// The request operation does not exist on the proxy's interface at
    /// its pinned version (a caller bug caught before the wire —
    /// the server would reject it with `invalid_opcode`).
    NoSuchRequest {
        /// Interface the proxy carries.
        interface: Box<str>,
        /// Request name the caller asked for.
        request: Box<str>,
    },
    /// Connection-state misuse: the operation requires the handshake
    /// (or forbids re-running it).
    HandshakeState(&'static str),
    /// A send was attempted after the connection died; see
    /// [`ClientError::Disconnected`] for the original cause.
    ConnectionDead,
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ldp(e) => write!(f, "ldp layer error: {e}"),
            Self::Disconnected(kind) => write!(f, "disconnected: {kind}"),
            Self::ServerError {
                code,
                object,
                message,
            } => write!(f, "server error {code:?} on object {object}: {message}"),
            Self::UnexpectedEvent { target } => {
                write!(f, "event for unknown object {target:#010x}")
            }
            Self::IdExhausted => f.write_str("client object-ID space exhausted"),
            Self::UnknownProxy { target } => {
                write!(f, "no proxy for object {target:#010x} on this connection")
            }
            Self::NoSuchRequest { interface, request } => {
                write!(
                    f,
                    "interface '{interface}' has no request '{request}' at the proxy's version"
                )
            }
            Self::HandshakeState(what) => f.write_str(what),
            Self::ConnectionDead => f.write_str("connection is closed (see the disconnect error)"),
        }
    }
}

impl std::error::Error for ClientError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Ldp(e) => Some(e),
            _ => None,
        }
    }
}

impl From<LdpError> for ClientError {
    fn from(e: LdpError) -> ClientError {
        ClientError::Ldp(e)
    }
}

/// Client-side result type.
pub type Result<T, E = ClientError> = core::result::Result<T, E>;

/// Classify a transport-layer error as a disconnect kind.
///
/// Mirrors the server's classification (`ldp-server` session): an
/// orderly EOF between messages is [`DisconnectKind::Clean`]; EOF
/// mid-message or EPIPE is [`DisconnectKind::Crash`]; anything else is
/// [`DisconnectKind::Transport`]. A
/// [`WouldBlock`](ldp_transport::error::is_would_block) error is not a
/// disconnect at all and maps to [`DisconnectKind::Transport`] only if
/// the caller insists on classifying it.
pub fn classify_disconnect(e: &LdpError) -> DisconnectKind {
    if let LdpError::Io(io) = e {
        match io.kind() {
            std::io::ErrorKind::UnexpectedEof => {
                // The transport's mid-message EOF messages carry
                // "mid-"; clean EOFs do not.
                if io.to_string().contains("mid-") {
                    DisconnectKind::Crash
                } else {
                    DisconnectKind::Clean
                }
            }
            std::io::ErrorKind::BrokenPipe => DisconnectKind::Crash,
            _ => DisconnectKind::Transport,
        }
    } else {
        DisconnectKind::Transport
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ldp_transport::error::{eof_clean, eof_mid};

    #[test]
    fn display_covers_every_variant() {
        let cases: Vec<ClientError> = vec![
            ClientError::Ldp(LdpError::Logic { what: "boom" }),
            ClientError::Disconnected(DisconnectKind::Clean),
            ClientError::ServerError {
                code: ErrorCode::InvalidState,
                object: 0,
                message: "twice".into(),
            },
            ClientError::UnexpectedEvent { target: 7 },
            ClientError::IdExhausted,
            ClientError::UnknownProxy { target: 9 },
            ClientError::NoSuchRequest {
                interface: "ldp.core.surface".into(),
                request: "nope".into(),
            },
            ClientError::HandshakeState("the handshake must complete first"),
            ClientError::ConnectionDead,
        ];
        for c in &cases {
            assert!(!c.to_string().is_empty());
        }
    }

    #[test]
    fn eof_classification_matches_transport_shapes() {
        assert_eq!(classify_disconnect(&eof_clean()), DisconnectKind::Clean);
        assert_eq!(
            classify_disconnect(&eof_mid("header")),
            DisconnectKind::Crash
        );
        let pipe = LdpError::Io(std::io::Error::from_raw_os_error(32).into()); // EPIPE
        assert_eq!(classify_disconnect(&pipe), DisconnectKind::Crash);
        let io = LdpError::Io(std::io::Error::from_raw_os_error(22).into()); // EINVAL
        assert_eq!(classify_disconnect(&io), DisconnectKind::Transport);
    }
}
