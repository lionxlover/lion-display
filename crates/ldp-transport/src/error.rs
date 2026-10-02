//! Transport error helpers.
//!
//! The crate reuses [`LdpError`] as its error type — the taxonomy is one
//! list, not two (`ldp-core::error` design rules). This module collects
//! the constructors for the framing failures of `docs/protocol.md` §9
//! stage 1 plus the two classification predicates event loops need:
//! [`is_would_block`] (retry later) and [`is_disconnect`] (peer gone).

#![forbid(unsafe_code)]

use ldp_core::error::{ErrorCode, LdpError, LimitKind};

/// A frame exceeded the negotiated message ceiling
/// ([`LimitKind::MessageBytes`], the offending total size included).
#[must_use]
pub fn oversize_frame(payload_words: u64) -> LdpError {
    LdpError::Limit {
        kind: LimitKind::MessageBytes,
        value: ldp_core::limits::Limits::HEADER_BYTES
            + ldp_core::limits::Limits::ALIGN * payload_words,
    }
}

/// A frame carried more FDs than the per-message limit allows.
#[must_use]
pub fn fd_limit(count: usize) -> LdpError {
    LdpError::Limit {
        kind: LimitKind::FdsPerMessage,
        value: count as u64,
    }
}

/// Header FD count and received ancillary FD count disagree.
///
/// Maps to [`ErrorCode::FdMismatch`]; every FD of the message is closed
/// by the caller before the error surfaces.
#[must_use]
pub fn fd_mismatch(expected: u16, received: usize) -> LdpError {
    LdpError::malformed(
        ErrorCode::FdMismatch,
        format!("header declares {expected} FDs, ancillary array carries {received}"),
    )
}

/// Ancillary data arrived attached mid-message (after the first byte) or
/// was truncated by the kernel because it exceeded the receive control
/// buffer — both are FD-bomb / consistency violations
/// ([`ErrorCode::FdMismatch`]).
#[must_use]
pub fn control_truncated(unit: &str) -> LdpError {
    LdpError::malformed(
        ErrorCode::FdMismatch,
        format!("ancillary data truncated or misplaced mid-{unit}"),
    )
}

/// The pending-outbound ceiling was exceeded; the caller may coalesce,
/// drop, or fail the connection ([`LimitKind::EventQueueBytes`]).
#[must_use]
pub fn overflow(attempted: u64) -> LdpError {
    LdpError::Limit {
        kind: LimitKind::EventQueueBytes,
        value: attempted,
    }
}

/// The peer closed the socket between messages (clean disconnect).
#[must_use]
pub fn eof_clean() -> LdpError {
    LdpError::Io(std::sync::Arc::new(std::io::Error::new(
        std::io::ErrorKind::UnexpectedEof,
        "peer closed the connection",
    )))
}

/// The peer closed the socket with a message in flight.
#[must_use]
pub fn eof_mid(unit: &str) -> LdpError {
    LdpError::Io(std::sync::Arc::new(std::io::Error::new(
        std::io::ErrorKind::UnexpectedEof,
        format!("peer closed the connection mid-{unit}"),
    )))
}

/// The reader already reported a fatal framing error; the connection it
/// belonged to must be dropped, never resynced.
#[must_use]
pub fn poisoned() -> LdpError {
    LdpError::Logic {
        what: "framing reader is poisoned: a framing error already killed this connection",
    }
}

/// A caller-supplied buffer violates the send-side preconditions
/// (shorter than the header, not 8-byte aligned, or its header FD count
/// disagrees with the FD batch) — refuse to put it on the wire.
#[must_use]
pub fn bad_send(reason: &str) -> LdpError {
    LdpError::malformed(
        ErrorCode::MalformedMessage,
        format!("refusing to send: {reason}"),
    )
}

/// The syscall layer returned something a stream socket never returns
/// for a non-empty buffer (`sendmsg` accepting zero bytes).
#[must_use]
pub fn zero_progress() -> LdpError {
    LdpError::Io(std::sync::Arc::new(std::io::Error::new(
        std::io::ErrorKind::WriteZero,
        "sendmsg accepted zero bytes of a non-empty buffer",
    )))
}

/// `true` when a nonblocking operation reports "try again later" — the
/// caller keeps its state and retries on readiness.
#[must_use]
pub fn is_would_block(e: &LdpError) -> bool {
    matches!(e, LdpError::Io(io) if io.kind() == std::io::ErrorKind::WouldBlock)
}

/// `true` when the peer is gone (clean EOF, reset, abort, broken pipe) —
/// nothing more can be read or written.
#[must_use]
pub fn is_disconnect(e: &LdpError) -> bool {
    use std::io::ErrorKind::{BrokenPipe, ConnectionAborted, ConnectionReset, UnexpectedEof};
    matches!(
        e,
        LdpError::Io(io)
            if matches!(
                io.kind(),
                UnexpectedEof | ConnectionReset | ConnectionAborted | BrokenPipe
            )
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversize_maps_to_limit_exceeded() {
        let e = oversize_frame(140_000);
        assert!(
            matches!(e, LdpError::Limit { kind: LimitKind::MessageBytes, value } if value == 16 + 8 * 140_000)
        );
        assert_eq!(e.wire_code(), Some(ErrorCode::LimitExceeded));
        assert!(e.is_fatal());
    }

    #[test]
    fn fd_limit_and_mismatch_map_to_wire_codes() {
        let e = fd_limit(200);
        assert!(matches!(
            e,
            LdpError::Limit {
                kind: LimitKind::FdsPerMessage,
                value: 200
            }
        ));
        assert_eq!(e.wire_code(), Some(ErrorCode::LimitExceeded));
        let e = fd_mismatch(4, 0);
        assert_eq!(e.wire_code(), Some(ErrorCode::FdMismatch));
        assert!(e.is_fatal());
        assert!(e.to_string().contains("declares 4"));
    }

    #[test]
    fn overflow_maps_to_event_queue_limit() {
        let e = overflow(3_000_000);
        assert!(matches!(
            e,
            LdpError::Limit {
                kind: LimitKind::EventQueueBytes,
                value: 3_000_000
            }
        ));
        assert_eq!(e.wire_code(), Some(ErrorCode::LimitExceeded));
    }

    #[test]
    fn eof_and_would_block_classification() {
        assert!(is_disconnect(&eof_clean()));
        assert!(is_disconnect(&eof_mid("payload")));
        assert!(!is_disconnect(&overflow(1)));
        let would_block = LdpError::Io(std::sync::Arc::new(std::io::Error::from(
            std::io::ErrorKind::WouldBlock,
        )));
        assert!(is_would_block(&would_block));
        assert!(!is_would_block(&eof_clean()));
        assert!(!is_disconnect(&would_block));
        assert!(!is_would_block(&eof_clean()));
    }

    #[test]
    fn poisoned_is_logic_not_wire() {
        let e = poisoned();
        assert!(matches!(e, LdpError::Logic { .. }));
        assert!(!e.is_fatal());
        assert_eq!(e.wire_code(), None);
    }

    #[test]
    fn bad_send_is_malformed() {
        let e = bad_send("length not a multiple of 8");
        assert_eq!(e.wire_code(), Some(ErrorCode::MalformedMessage));
    }
}
