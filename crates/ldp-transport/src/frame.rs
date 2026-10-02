//! The frame: one complete received message.
//!
//! Stage 1 of the `docs/protocol.md` §9 validation pipeline — size,
//! alignment, FD-count consistency — and *only* that. Flag bits, tags,
//! and signatures are `ldp-protocol` stages 2–3; the transport hands
//! over raw bytes plus owned FDs and never interprets the payload.
//!
//! [`Frame`] stores the full message (header + payload) as one
//! contiguous buffer so `ldp_protocol::decode(&frame.message_bytes(),
//! frame.fd_count(), …)` needs no copy.

#![forbid(unsafe_code)]

use crate::fd::FdList;
use ldp_core::error::Result;
use ldp_core::limits::Limits;

/// Envelope header size in bytes (`docs/protocol.md` §2).
pub const HEADER_BYTES: usize = 16;

/// Wire alignment unit (`docs/protocol.md` §1).
pub const ALIGN: usize = 8;

/// One complete, framing-validated message.
#[derive(Debug)]
pub struct Frame {
    /// Full message bytes: 16-byte header + aligned payload.
    bytes: Vec<u8>,
    /// The message's ancillary FDs, adopted and owned; exactly
    /// `fd_count()` of them.
    pub fds: FdList,
}

impl Frame {
    /// Wrap validated parts (used by [`FramedReader`](crate::reader)).
    ///
    /// # Panics
    ///
    /// Never in production: the `debug_assert` re-checks the framing
    /// invariants the reader already guaranteed.
    #[must_use]
    pub(crate) fn from_parts(bytes: Vec<u8>, fds: FdList) -> Frame {
        debug_assert!(bytes.len() >= HEADER_BYTES && bytes.len() % ALIGN == 0);
        Frame { bytes, fds }
    }

    /// Full message bytes (header + payload), little-endian fields.
    #[must_use]
    pub fn message_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The 16-byte envelope header.
    ///
    /// # Panics
    ///
    /// Never: every `Frame` is at least one header long by construction.
    #[must_use]
    pub fn header(&self) -> &[u8; HEADER_BYTES] {
        self.bytes[..HEADER_BYTES]
            .try_into()
            .expect("frames always carry a 16-byte header")
    }

    /// Payload bytes (everything past the header).
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.bytes[HEADER_BYTES..]
    }

    /// Header FD count (already cross-checked against the ancillary
    /// array by the reader).
    #[must_use]
    pub fn fd_count(&self) -> u16 {
        header_fd_count(self.header())
    }

    /// Payload length in 8-byte words (header field).
    #[must_use]
    pub fn payload_words(&self) -> u32 {
        header_payload_words(self.header())
    }
}

/// Read `payload_words` (LE u32) from a raw header.
#[must_use]
pub(crate) fn header_payload_words(header: &[u8; HEADER_BYTES]) -> u32 {
    u32::from_le_bytes([header[0], header[1], header[2], header[3]])
}

/// Read `fd_count` (LE u16) from a raw header.
#[must_use]
pub(crate) fn header_fd_count(header: &[u8; HEADER_BYTES]) -> u16 {
    u16::from_le_bytes([header[14], header[15]])
}

/// Stage-1 framing validation: message size and FD count against the
/// negotiated limits. Runs *before* any payload allocation — the
/// no-allocation-before-validation ordering is a security property
/// (`docs/protocol.md` §9, `threat-model.md` §4).
///
/// # Errors
///
/// [`LdpError::Limit`](ldp_core::error::LdpError::Limit) (`limit_exceeded`) for either ceiling.
pub(crate) fn validate_framing(payload_words: u32, fd_count: u16, limits: &Limits) -> Result<()> {
    if !limits.message_fits(u64::from(payload_words)) {
        return Err(crate::error::oversize_frame(u64::from(payload_words)));
    }
    if u32::from(fd_count) > limits.fds_per_message {
        return Err(crate::error::fd_limit(usize::from(fd_count)));
    }
    Ok(())
}

/// Send-side preconditions: never put a structurally impossible message
/// on the wire. The checks mirror the receiver's stage 1 so a local bug
/// surfaces as the same error the peer would report.
///
/// # Errors
///
/// [`LdpError::malformed`](ldp_core::error::LdpError::malformed)(ldp_core::error::LdpError::malformed) for short, misaligned,
/// or FD-inconsistent messages; [`LdpError::Limit`](ldp_core::error::LdpError::Limit) for ceiling
/// violations.
pub(crate) fn validate_send(msg: &[u8], fd_count: u16, limits: &Limits) -> Result<()> {
    if msg.len() < HEADER_BYTES {
        return Err(crate::error::bad_send(
            "message shorter than the 16-byte header",
        ));
    }
    if msg.len() % ALIGN != 0 {
        return Err(crate::error::bad_send(
            "message length is not a multiple of 8",
        ));
    }
    if (msg.len() as u64) > limits.message_bytes {
        return Err(crate::error::oversize_frame(
            (msg.len() as u64 - HEADER_BYTES as u64) / ALIGN as u64,
        ));
    }
    if u32::from(fd_count) > limits.fds_per_message {
        return Err(crate::error::fd_limit(usize::from(fd_count)));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header_for(words: u32, fd_count: u16) -> [u8; HEADER_BYTES] {
        let mut h = [0u8; HEADER_BYTES];
        h[0..4].copy_from_slice(&words.to_le_bytes());
        h[14..16].copy_from_slice(&fd_count.to_le_bytes());
        h
    }

    #[test]
    fn constants_match_the_protocol_and_core() {
        assert_eq!(HEADER_BYTES as u64, Limits::HEADER_BYTES);
        assert_eq!(ALIGN as u64, Limits::ALIGN);
    }

    #[test]
    fn header_field_accessors() {
        let h = header_for(131_070, 64);
        assert_eq!(header_payload_words(&h), 131_070);
        assert_eq!(header_fd_count(&h), 64);
    }

    #[test]
    fn framing_validation_boundaries() {
        let limits = Limits::DEFAULT;
        // Exactly at the ceiling: (1 MiB - 16) / 8 words.
        let max_words = limits.max_payload_words() as u32;
        assert!(validate_framing(max_words, 64, &limits).is_ok());
        assert!(validate_framing(max_words + 1, 0, &limits).is_err());
        // FD ceiling boundary.
        assert!(validate_framing(0, 64, &limits).is_ok());
        assert!(validate_framing(0, 65, &limits).is_err());
        // Zero payload, zero FDs is a valid frame.
        assert!(validate_framing(0, 0, &limits).is_ok());
    }

    #[test]
    fn framing_validation_error_codes() {
        let limits = Limits::DEFAULT;
        let e = validate_framing(200_000, 0, &limits).unwrap_err();
        assert_eq!(
            e.wire_code(),
            Some(ldp_core::error::ErrorCode::LimitExceeded)
        );
        let e = validate_framing(0, 100, &limits).unwrap_err();
        assert_eq!(
            e.wire_code(),
            Some(ldp_core::error::ErrorCode::LimitExceeded)
        );
    }

    #[test]
    fn send_validation_rejects_short_and_misaligned() {
        let limits = Limits::DEFAULT;
        assert!(validate_send(&[0u8; 8], 0, &limits).is_err());
        assert!(validate_send(&[0u8; 20], 0, &limits).is_err());
        assert!(validate_send(&[0u8; 24], 0, &limits).is_ok());
        // Oversized (uses a permissive limit set to build the buffer).
        let big_limits = Limits {
            message_bytes: 256,
            ..Limits::DEFAULT
        };
        assert!(validate_send(&[0u8; 256], 0, &big_limits).is_ok());
        assert!(validate_send(&[0u8; 264], 0, &big_limits).is_err());
        assert!(validate_send(&[0u8; 24], 65, &Limits::DEFAULT).is_err());
    }

    #[test]
    fn frame_accessors_slice_the_buffer() {
        let mut bytes = vec![0u8; 40];
        bytes[0..4].copy_from_slice(&3u32.to_le_bytes());
        bytes[14..16].copy_from_slice(&2u16.to_le_bytes());
        let frame = Frame::from_parts(bytes, FdList::new());
        assert_eq!(frame.payload_words(), 3);
        assert_eq!(frame.fd_count(), 2);
        assert_eq!(frame.message_bytes().len(), 40);
        assert_eq!(frame.payload().len(), 24);
        assert_eq!(frame.header().len(), 16);
        assert!(frame.fds.is_empty());
    }
}
