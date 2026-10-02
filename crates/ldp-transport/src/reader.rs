//! The framing reader: bytes and FDs in, validated frames out.
//!
//! Reads are message-exact: 16 header bytes first (accumulating any
//! ancillary FDs the peer attached to the message head), then exactly
//! `payload_words * 8` payload bytes with the control buffer detached —
//! so ancillary data can never sneak past the reader mid-message
//! (MSG_CTRUNC is reported and the smuggled FDs are closed by the
//! kernel).
//!
//! Stage-1 rules enforced here (`docs/protocol.md` §2, §9):
//!
//! * `16 + 8 * payload_words <= Limits::message_bytes`, checked *before*
//!   the payload buffer exists (no allocation from untrusted sizes),
//! * `fd_count <= Limits::fds_per_message`, ditto,
//! * header `fd_count` == FDs actually received in the ancillary array —
//!   a mismatch is fatal (`fd_mismatch`) and every FD of the rejected
//!   message is closed.
//!
//! Any framing failure poisons the reader: the first error is returned,
//! later calls return
//! [`LdpError::Logic`] — protocol
//! errors are fatal to the connection, resynchronization is not
//! attempted. A [`WouldBlock`](crate::error::is_would_block) error is *not* a
//! failure: mid-message state (including received FDs) is retained and
//! the next call resumes.

#![forbid(unsafe_code)]

use crate::error::{control_truncated, eof_clean, eof_mid, fd_mismatch, poisoned};
use crate::fd::FdList;
use crate::frame::{
    header_fd_count, header_payload_words, validate_framing, Frame, ALIGN, HEADER_BYTES,
};
use crate::stream::TransportStream;
use crate::sys::ControlBuffer;
use ldp_core::error::{LdpError, Result};
use ldp_core::limits::Limits;

/// The framed message reader for one connection.
#[derive(Debug)]
pub struct FramedReader {
    limits: Limits,
    control: ControlBuffer,
    /// Message under construction: header first, then payload.
    buf: Vec<u8>,
    /// Bytes of `buf` filled so far.
    filled: usize,
    /// Total message size once the header has been parsed (0 until then).
    needed: usize,
    /// FDs received for the message in flight.
    inflight: FdList,
    /// Set by the first framing failure; all later calls fail fast.
    poisoned: bool,
}

impl FramedReader {
    /// A reader under `limits`. The control buffer is sized for
    /// `Limits::fds_per_message` FDs — anything larger the peer sends
    /// arrives as MSG_CTRUNC and is rejected.
    #[must_use]
    pub fn new(limits: Limits) -> FramedReader {
        let max_fds = usize::try_from(limits.fds_per_message.max(1)).unwrap_or(64);
        FramedReader {
            control: ControlBuffer::with_fd_capacity(max_fds),
            limits,
            buf: Vec::new(),
            filled: 0,
            needed: 0,
            inflight: FdList::new(),
            poisoned: false,
        }
    }

    /// Receive one complete message.
    ///
    /// Blocking streams return a frame or fail; nonblocking streams may
    /// return [`WouldBlock`](crate::error::is_would_block) with the partial
    /// message retained — retry on readability.
    ///
    /// # Errors
    ///
    /// [`LdpError::Limit`] / [`LdpError::malformed`](ldp_core::error::LdpError::malformed)
    /// for stage-1 violations (fatal for the connection; the reader is
    /// poisoned); [`LdpError::Io`] on disconnect or transient
    /// nonblocking empty.
    ///
    /// # Panics
    ///
    /// Never on peer input: the internal `expect`s guard invariants the
    /// state machine itself maintains (a complete header is 16 bytes).
    pub fn recv_msg(&mut self, stream: &mut TransportStream) -> Result<Frame> {
        if self.poisoned {
            return Err(poisoned());
        }
        loop {
            if self.filled < HEADER_BYTES {
                // Header phase: ancillary FDs may arrive with any of the
                // header's bytes (the sender attaches them to byte 0).
                if self.buf.len() < HEADER_BYTES {
                    self.buf.resize(HEADER_BYTES, 0);
                }
                let got = stream
                    .recv_chunk(&mut self.buf[self.filled..HEADER_BYTES], &mut self.control)?;
                if got.bytes == 0 {
                    let e = if self.filled == 0 && self.inflight.is_empty() {
                        eof_clean()
                    } else {
                        eof_mid("header")
                    };
                    return Err(self.fail(e));
                }
                if let Err(e) = self
                    .control
                    .parse_rights_into(got.control_used, &mut self.inflight)
                {
                    return Err(self.fail(e));
                }
                if got.truncated {
                    return Err(self.fail(control_truncated("header")));
                }
                self.filled += got.bytes;
                if self.filled < HEADER_BYTES {
                    continue;
                }
                // Header complete: validate before allocating the payload.
                let header: &[u8; HEADER_BYTES] = self.buf[..HEADER_BYTES]
                    .try_into()
                    .expect("header length checked");
                let words = header_payload_words(header);
                let fd_count = header_fd_count(header);
                if let Err(e) = validate_framing(words, fd_count, &self.limits) {
                    return Err(self.fail(e));
                }
                self.needed = HEADER_BYTES + words as usize * ALIGN;
                self.buf.resize(self.needed, 0);
                continue;
            }
            // Payload phase: no control buffer — smuggled FDs cause
            // MSG_CTRUNC and are closed by the kernel.
            if self.filled < self.needed {
                let want = self.needed - self.filled;
                let got =
                    stream.recv_chunk_plain(&mut self.buf[self.filled..self.filled + want])?;
                if got.bytes == 0 {
                    return Err(self.fail(eof_mid("payload")));
                }
                if got.truncated {
                    return Err(self.fail(control_truncated("payload")));
                }
                self.filled += got.bytes;
            }
            if self.filled == self.needed {
                let declared = header_fd_count(
                    self.buf[..HEADER_BYTES]
                        .try_into()
                        .expect("header length checked"),
                );
                if self.inflight.len() != usize::from(declared) {
                    return Err(self.fail(fd_mismatch(declared, self.inflight.len())));
                }
                let bytes = std::mem::take(&mut self.buf);
                let fds = std::mem::take(&mut self.inflight);
                self.filled = 0;
                self.needed = 0;
                return Ok(Frame::from_parts(bytes, fds));
            }
        }
    }

    /// Fail fatally: poison, drop in-flight FDs, reset partial state.
    fn fail(&mut self, e: LdpError) -> LdpError {
        self.poisoned = true;
        self.inflight.close_all();
        self.buf.clear();
        self.filled = 0;
        self.needed = 0;
        e
    }

    /// Whether a framing error already killed this reader.
    #[must_use]
    pub fn is_poisoned(&self) -> bool {
        self.poisoned
    }

    /// FDs held for a partially received message (diagnostics).
    #[must_use]
    pub fn inflight_fds(&self) -> usize {
        self.inflight.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::is_disconnect;
    use crate::error::is_would_block;
    use std::os::fd::AsRawFd;

    fn msg_with(words: u32, fd_count: u16) -> Vec<u8> {
        let mut msg = vec![0u8; HEADER_BYTES + words as usize * ALIGN];
        msg[0..4].copy_from_slice(&words.to_le_bytes());
        msg[14..16].copy_from_slice(&fd_count.to_le_bytes());
        for (i, rx) in msg[HEADER_BYTES..].iter_mut().enumerate() {
            *rx = (i % 251) as u8;
        }
        msg
    }

    fn raw_pair() -> (TransportStream, TransportStream) {
        TransportStream::pair().unwrap()
    }

    #[test]
    fn empty_frame_round_trips() {
        let (mut tx, mut rx) = raw_pair();
        let mut writer = crate::writer::FramedWriter::without_hooks(Limits::DEFAULT);
        let mut reader = FramedReader::new(Limits::DEFAULT);
        let msg = msg_with(0, 0);
        writer.send_msg(&mut tx, &msg, &mut FdList::new()).unwrap();
        let f = reader.recv_msg(&mut rx).unwrap();
        assert_eq!(f.message_bytes(), &msg[..]);
        assert!(f.payload().is_empty());
        assert_eq!(f.payload_words(), 0);
        assert_eq!(f.fd_count(), 0);
    }

    #[test]
    fn several_frames_in_order() {
        let (mut tx, mut rx) = raw_pair();
        let mut writer = crate::writer::FramedWriter::without_hooks(Limits::DEFAULT);
        let mut reader = FramedReader::new(Limits::DEFAULT);
        for words in [0u32, 1, 5, 64, 3] {
            let msg = msg_with(words, 0);
            writer.send_msg(&mut tx, &msg, &mut FdList::new()).unwrap();
        }
        for words in [0u32, 1, 5, 64, 3] {
            let f = reader.recv_msg(&mut rx).unwrap();
            assert_eq!(f.payload_words(), words);
            assert_eq!(f.payload().len(), words as usize * ALIGN);
        }
    }

    #[test]
    fn oversized_frame_rejected_and_poisons() {
        let (mut tx, mut rx) = raw_pair();
        // Nonblocking sender: the 1.6 MB frame fills the socket buffer
        // (the header reaches the peer) and then reports EAGAIN instead
        // of blocking forever — the peer only ever reads the header.
        tx.set_nonblocking(true).unwrap();
        let _ = tx.send_chunk(&msg_with(200_000, 0), None);
        let mut reader = FramedReader::new(Limits::DEFAULT);
        let fd0 = reader.recv_msg(&mut rx).unwrap_err();
        assert_eq!(
            fd0.wire_code(),
            Some(ldp_core::error::ErrorCode::LimitExceeded)
        );
        assert!(reader.is_poisoned());
        // Later calls fail fast without touching the socket.
        let e2 = reader.recv_msg(&mut rx).unwrap_err();
        assert!(matches!(e2, LdpError::Logic { .. }));
    }

    #[test]
    fn fd_count_over_limit_rejected_and_closed() {
        let (mut tx, mut rx) = raw_pair();
        // Header claims 65 FDs but only 2 travel with it.
        let msg = msg_with(1, 65);
        let mut ctl = ControlBuffer::with_fd_capacity(4);
        let e1 = crate::sys::eventfd_owned().unwrap();
        let e2 = crate::sys::eventfd_owned().unwrap();
        let used = ctl
            .encode_rights(&[e1.as_raw_fd(), e2.as_raw_fd()])
            .unwrap();
        tx.send_chunk(&msg, Some(ctl.encoded_prefix(used))).unwrap();
        drop(e1);
        drop(e2);
        let mut reader = FramedReader::new(Limits::DEFAULT);
        let err = reader.recv_msg(&mut rx).unwrap_err();
        assert_eq!(
            err.wire_code(),
            Some(ldp_core::error::ErrorCode::LimitExceeded)
        );
        // The two received FDs were adopted and closed on the fail path —
        // the leak-count gate lives in tests/hygiene.rs (serialized).
    }

    #[test]
    fn fd_count_mismatch_rejected_and_closed() {
        let (mut tx, mut rx) = raw_pair();
        let msg = msg_with(1, 3); // claims 3
        let mut ctl = ControlBuffer::with_fd_capacity(4);
        let e1 = crate::sys::eventfd_owned().unwrap();
        let e2 = crate::sys::eventfd_owned().unwrap();
        let used = ctl
            .encode_rights(&[e1.as_raw_fd(), e2.as_raw_fd()])
            .unwrap();
        tx.send_chunk(&msg, Some(ctl.encoded_prefix(used))).unwrap();
        drop(e1);
        drop(e2);
        let mut reader = FramedReader::new(Limits::DEFAULT);
        let err = reader.recv_msg(&mut rx).unwrap_err();
        assert_eq!(
            err.wire_code(),
            Some(ldp_core::error::ErrorCode::FdMismatch)
        );
    }

    #[test]
    fn declared_zero_but_fds_sent_rejected_and_closed() {
        let (mut tx, mut rx) = raw_pair();
        let msg = msg_with(0, 0);
        let mut ctl = ControlBuffer::with_fd_capacity(4);
        let fd0 = crate::sys::eventfd_owned().unwrap();
        let used = ctl.encode_rights(&[fd0.as_raw_fd()]).unwrap();
        tx.send_chunk(&msg, Some(ctl.encoded_prefix(used))).unwrap();
        drop(fd0);
        let mut reader = FramedReader::new(Limits::DEFAULT);
        let err = reader.recv_msg(&mut rx).unwrap_err();
        assert_eq!(
            err.wire_code(),
            Some(ldp_core::error::ErrorCode::FdMismatch)
        );
    }

    #[test]
    fn mid_payload_fds_rejected() {
        let (mut tx, mut rx) = raw_pair();
        // A well-formed 2-fd message, then smuggled FDs attached to tx
        // payload byte of the NEXT message.
        let mut ctl = ControlBuffer::with_fd_capacity(4);
        let e1 = crate::sys::eventfd_owned().unwrap();
        let e2 = crate::sys::eventfd_owned().unwrap();
        let used = ctl
            .encode_rights(&[e1.as_raw_fd(), e2.as_raw_fd()])
            .unwrap();
        let msg = msg_with(2, 2);
        tx.send_chunk(&msg, Some(ctl.encoded_prefix(used))).unwrap();
        drop(e1);
        drop(e2);
        // Smuggle: 16-byte header of another message (fd_count 0), then
        // send 8 payload bytes WITH control data.
        let m2 = msg_with(1, 0);
        tx.send_chunk(&m2[..HEADER_BYTES], None).unwrap();
        let e3 = crate::sys::eventfd_owned().unwrap();
        let used2 = ctl.encode_rights(&[e3.as_raw_fd()]).unwrap();
        tx.send_chunk(&m2[HEADER_BYTES..], Some(ctl.encoded_prefix(used2)))
            .unwrap();
        drop(e3);

        let mut reader = FramedReader::new(Limits::DEFAULT);
        // First message is fine (its two FDs are ours to drop).
        let f = reader.recv_msg(&mut rx).unwrap();
        assert_eq!(f.fds.len(), 2);
        drop(f);
        // Second message: FDs attached mid-payload -> CTRUNC (kernel
        // closes e3) -> fd_mismatch family error, reader poisoned.
        let err = reader.recv_msg(&mut rx).unwrap_err();
        assert!(err.is_fatal());
        assert_eq!(
            err.wire_code(),
            Some(ldp_core::error::ErrorCode::FdMismatch)
        );
        // The kernel closed the smuggled fd on MSG_CTRUNC (no leak path).
    }

    #[test]
    fn truncation_mid_message_reports_disconnect() {
        let (mut tx, mut rx) = raw_pair();
        let msg = msg_with(8, 0);
        tx.send_chunk(&msg[..HEADER_BYTES + 8], None).unwrap(); // half the payload
        drop(tx); // orderly close with tx message in flight
        let mut reader = FramedReader::new(Limits::DEFAULT);
        let fd0 = reader.recv_msg(&mut rx).unwrap_err();
        assert!(is_disconnect(&fd0));
    }

    #[test]
    fn clean_close_between_messages() {
        let (tx, mut rx) = raw_pair();
        drop(tx);
        let mut reader = FramedReader::new(Limits::DEFAULT);
        let fd0 = reader.recv_msg(&mut rx).unwrap_err();
        assert!(is_disconnect(&fd0));
    }

    #[test]
    fn nonblocking_resumes_partial_messages() {
        let (mut tx, mut rx) = raw_pair();
        tx.set_nonblocking(true).unwrap();
        rx.set_nonblocking(true).unwrap();
        let mut reader = FramedReader::new(Limits::DEFAULT);
        // Nothing to read yet.
        let fd0 = reader.recv_msg(&mut rx).unwrap_err();
        assert!(is_would_block(&fd0));
        // Send in fragments: header, then payload halves.
        let msg = msg_with(2, 0);
        tx.send_chunk(&msg[..8], None).unwrap();
        let fd0 = reader.recv_msg(&mut rx).unwrap_err();
        assert!(is_would_block(&fd0), "half tx header must retain state");
        tx.send_chunk(&msg[8..HEADER_BYTES], None).unwrap();
        let fd0 = reader.recv_msg(&mut rx).unwrap_err();
        assert!(is_would_block(&fd0), "header only must retain state");
        tx.send_chunk(&msg[HEADER_BYTES..HEADER_BYTES + 8], None)
            .unwrap();
        let fd0 = reader.recv_msg(&mut rx).unwrap_err();
        assert!(is_would_block(&fd0));
        tx.send_chunk(&msg[HEADER_BYTES + 8..], None).unwrap();
        let f = reader.recv_msg(&mut rx).unwrap();
        assert_eq!(f.message_bytes(), &msg[..]);
        assert_eq!(reader.inflight_fds(), 0);
    }

    #[test]
    fn max_boundary_frame_round_trips() {
        let (mut tx, mut rx) = raw_pair();
        let limits = Limits {
            message_bytes: 256,
            ..Limits::DEFAULT
        };
        // (256 - 16) / 8 = 30 words — exactly at the ceiling.
        let msg = msg_with(30, 0);
        let mut writer = crate::writer::FramedWriter::without_hooks(limits);
        let mut reader = FramedReader::new(limits);
        writer.send_msg(&mut tx, &msg, &mut FdList::new()).unwrap();
        let f = reader.recv_msg(&mut rx).unwrap();
        assert_eq!(f.message_bytes(), &msg[..]);
        // One word more is refused by the reader.
        let too_big = msg_with(31, 0);
        tx.send_chunk(&too_big, None).unwrap();
        let fd0 = reader.recv_msg(&mut rx).unwrap_err();
        assert_eq!(
            fd0.wire_code(),
            Some(ldp_core::error::ErrorCode::LimitExceeded)
        );
    }
}
