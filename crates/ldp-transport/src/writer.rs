//! The framing writer: one message, one `sendmsg` (plus tail resumes).
//!
//! `docs/protocol.md` §1: one message = one `sendmsg` with its FDs
//! riding the same call. On a stream socket a `sendmsg` may accept only
//! a prefix; the ancillary data transfers with the *first accepted
//! byte*, so continuation writes carry no control data. This writer
//! encodes that discipline exactly:
//!
//! * [`FramedWriter::send_msg`] validates the message locally (never
//!   put structurally impossible bytes on the wire), appends it to the
//!   outbound queue, and attempts to hand everything to the socket,
//! * queued FD batches ([`FdList`]) attach at their message's first
//!   byte — the chunk carrying that byte is capped so ancillary data
//!   never rides the wrong message,
//! * the queue is bounded by [`BackpressureConfig::ceiling`] and the
//!   queued-FD count by `Limits::client_fds`: a slow peer parks its own
//!   queue at those bounds (`on_overflow`) instead of growing server
//!   memory,
//! * on success the caller's original FDs are closed here — the kernel
//!   duplicated them into the socket at send time; ownership transferred.
//!
//! The queue holds unsent *tails* only; each successful `sendmsg`
//! removes exactly what the kernel accepted. `SCM_RIGHTS` never
//! consumes the sender's descriptors — that is why "transfer" means
//! "responsibility to close".
//!
//! Chunk policy by stream mode: **nonblocking** streams drain while the
//! kernel accepts bytes (one `send_msg` may issue several `sendmsg`s);
//! **blocking** streams perform exactly one `sendmsg` per call — a
//! blocking socket would otherwise park the sending thread inside a
//! loop that only the peer can unblock. Callers on blocking streams
//! drive large messages with repeated [`FramedWriter::flush`] while the
//! peer reads (see the boundary tests).

#![forbid(unsafe_code)]

use crate::backpressure::{BackpressureConfig, WriterHooks};
use crate::error::{bad_send, fd_mismatch, is_would_block, overflow, zero_progress};
use crate::fd::FdList;
use crate::frame::{validate_send, HEADER_BYTES};
use crate::stream::TransportStream;
use crate::sys::ControlBuffer;
use ldp_core::error::Result;
use ldp_core::limits::Limits;
use std::collections::VecDeque;

/// The minimum dead-prefix length before unsent-front compaction is
/// worth a single `memmove` (see [`FramedWriter`]'s `sent` cursor).
/// Below this the dead prefix is bounded slack — at most 64 KiB of
/// already-sent bytes sit at the front of the queue between
/// compactions, never more, because compaction also fires whenever
/// the dead prefix reaches half the buffer.
const COMPACT_MIN_BYTES: usize = 64 * 1024;

/// Result of [`FramedWriter::send_msg`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SendOutcome {
    /// The whole queue (this message and any prior tail) reached the
    /// socket.
    Sent,
    /// The socket refused the remainder; `pending_bytes` stay queued and
    /// the caller should retry [`FramedWriter::flush`] on writability.
    Congested {
        /// Bytes queued locally for this peer right now.
        pending_bytes: u64,
    },
}

/// Result of [`FramedWriter::flush`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FlushOutcome {
    /// The queue drained completely.
    Drained,
    /// The socket refused the remainder; retry on writability.
    Congested {
        /// Bytes still queued locally.
        pending_bytes: u64,
    },
}

/// The framed message writer for one connection.
///
/// The type parameter is the observer ([`WriterHooks`]); construct with
/// [`FramedWriter::new`]`(limits, hooks)` or use [`NoHooks`] via
/// [`FramedWriter::without_hooks`].
///
/// [`NoHooks`]: crate::backpressure::NoHooks
#[derive(Debug)]
pub struct FramedWriter<H: WriterHooks> {
    limits: Limits,
    config: BackpressureConfig,
    hooks: H,
    control: ControlBuffer,
    /// Unsent bytes (tails of one or more messages), bounded by the ceiling.
    out: Vec<u8>,
    /// How many bytes at the front of `out` the kernel already
    /// accepted — the amortized-O(1) alternative to draining on every
    /// chunk. The dead prefix is compacted only when it is both
    /// ≥ [`COMPACT_MIN_BYTES`] and at least half the buffer, so each
    /// compaction moves at most as many bytes as it reclaims and a
    /// congested queue pays ≤ one byte-move per byte sent (the
    /// historical `drain(..n)`-per-chunk memmoved the whole tail on
    /// every EAGAIN-sized accept: a 2 MiB ceiling draining at 4 KiB
    /// chunks copied ~500 bytes per byte sent).
    sent: usize,
    /// FD batches waiting to attach, as byte offsets counted from the
    /// **unsent front** (`self.sent`): each entry's message starts that
    /// many bytes past the first byte the kernel has not accepted yet.
    /// (Relative-to-front is what compaction preserves for free.)
    attachments: VecDeque<(usize, FdList)>,
    /// Total FDs across `attachments`, bounded by `Limits::client_fds`.
    queued_fds: usize,
    congested: bool,
    below_low: bool,
}

impl<H: WriterHooks> FramedWriter<H> {
    /// A writer under `limits` with the given observer.
    #[must_use]
    pub fn new(limits: Limits, hooks: H) -> FramedWriter<H> {
        let max_fds = usize::try_from(limits.fds_per_message.max(1)).unwrap_or(64);
        FramedWriter {
            config: BackpressureConfig::from_limits(&limits),
            control: ControlBuffer::with_fd_capacity(max_fds),
            limits,
            hooks,
            out: Vec::new(),
            sent: 0,
            attachments: VecDeque::new(),
            queued_fds: 0,
            congested: false,
            below_low: true,
        }
    }

    /// Bytes queued locally (the backpressure measure).
    #[must_use]
    pub fn pending_bytes(&self) -> u64 {
        self.pending() as u64
    }

    /// Unsent byte count (the length past the `sent` cursor).
    fn pending(&self) -> usize {
        self.out.len() - self.sent
    }

    /// FDs queued for not-yet-sent message heads.
    #[must_use]
    pub fn queued_fds(&self) -> usize {
        self.queued_fds
    }

    /// Whether the socket currently refuses bytes for this peer.
    #[must_use]
    pub fn is_congested(&self) -> bool {
        self.congested
    }

    /// The observer (for inspection in tests and servers).
    #[must_use]
    pub fn hooks(&self) -> &H {
        &self.hooks
    }

    /// Mutable access to the observer.
    pub fn hooks_mut(&mut self) -> &mut H {
        &mut self.hooks
    }

    /// Queue one message and push bytes toward the socket.
    ///
    /// Local preconditions (all checked before anything touches the
    /// wire or the queue): length ≥ 16 and a multiple of 8, within the
    /// message ceiling; the batch within the FD ceiling; the header's
    /// `fd_count` field equal to `fds.len()`.
    ///
    /// On success or congestion the message (and its FDs) are consumed:
    /// the FD batch is closed after the kernel duplicated it. On error
    /// the connection should be dropped; on an
    /// [`on_overflow`](WriterHooks::on_overflow) rejection nothing is
    /// consumed. On a *blocking* stream at most one `sendmsg` happens
    /// per call — interleave [`FramedWriter::flush`] with peer reads for
    /// messages larger than the socket buffer.
    ///
    /// # Errors
    ///
    /// [`LdpError::Limit`](ldp_core::error::LdpError::Limit) when the byte ceiling or the queued-FD bound
    /// would be exceeded (the message is *not* consumed — coalesce or
    /// drop); [`LdpError::malformed`](ldp_core::error::LdpError::malformed)(ldp_core::error::LdpError::malformed) for
    /// structurally invalid messages; [`LdpError::Io`](ldp_core::error::LdpError::Io) on socket
    /// failure.
    pub fn send_msg(
        &mut self,
        stream: &mut TransportStream,
        msg: &[u8],
        fds: &mut FdList,
    ) -> Result<SendOutcome> {
        let declared = parse_fd_count(msg)?;
        validate_send(msg, declared, &self.limits)?;
        if usize::from(declared) != fds.len() {
            return Err(fd_mismatch(declared, fds.len()));
        }
        // Drain what we can first so the bounds apply to the real queue
        // (an uncongested socket may have emptied it entirely).
        if self.pending() > 0 {
            self.attempt_send(stream)?;
        }
        // Bound the queue: bytes by the ceiling, FDs by the client budget.
        let total = self.pending() + msg.len();
        if total as u64 > self.config.ceiling {
            self.hooks.on_overflow(total as u64, self.config.ceiling);
            return Err(overflow(total as u64));
        }
        if self.queued_fds + fds.len() > self.limits.client_fds as usize {
            self.hooks.on_overflow(total as u64, self.config.ceiling);
            return Err(crate::error::fd_limit(self.queued_fds + fds.len()));
        }
        let at = self.pending();
        self.out.extend_from_slice(msg);
        if !fds.is_empty() {
            let batch = FdList::from_vec(fds.take_all());
            self.queued_fds += batch.len();
            self.attachments.push_back((at, batch));
        }
        self.attempt_send(stream)
    }

    /// Push queued bytes toward the socket (call on writability).
    ///
    /// # Errors
    ///
    /// [`LdpError::Io`](ldp_core::error::LdpError::Io) on socket failure (drop the connection).
    pub fn flush(&mut self, stream: &mut TransportStream) -> Result<FlushOutcome> {
        let outcome = self.attempt_send(stream)?;
        Ok(match outcome {
            SendOutcome::Sent => FlushOutcome::Drained,
            SendOutcome::Congested { pending_bytes } => FlushOutcome::Congested { pending_bytes },
        })
    }

    /// The unsent-front compaction: reclaim the dead prefix only when
    /// it is both at least [`COMPACT_MIN_BYTES`] long and at least half
    /// the buffer. Each compaction then moves at most the number of
    /// bytes it reclaims (amortized ≤ 1 byte-move per byte sent), the
    /// live tail's `Vec` capacity is reused (no reallocation churn),
    /// and a small queue never pays a move at all. Attachment offsets
    /// are relative to the unsent front, so compaction does not
    /// disturb them.
    fn compact_if_ripe(&mut self) {
        if self.sent >= COMPACT_MIN_BYTES && self.sent >= self.out.len() - self.sent {
            self.out.drain(..self.sent);
            self.sent = 0;
        }
    }

    /// The sending loop: chunk-capped, control-attached, EAGAIN-safe.
    ///
    /// Chunk policy depends on the stream mode: a *nonblocking* stream
    /// drains while the kernel accepts bytes (event-loop friendly); a
    /// *blocking* stream performs exactly one `sendmsg` per call — a
    /// blocking socket would otherwise park this thread inside a loop
    /// that only the peer (usually behind the same caller) can unblock.
    fn attempt_send(&mut self, stream: &mut TransportStream) -> Result<SendOutcome> {
        let single_chunk = !stream.is_nonblocking();
        while self.pending() > 0 {
            // Where this chunk must stop: at the next attachment boundary
            // (its first byte needs a control-carrying sendmsg of its own)
            // or at the end of the queue. Attachment offsets count from
            // the unsent front (`self.sent`), exactly as the cursor sees it.
            let (chunk_end, attach_now) = match self.attachments.front() {
                Some((off, _)) if *off == 0 => (
                    self.attachments
                        .get(1)
                        .map_or(self.pending(), |(next, _)| *next),
                    true,
                ),
                Some((off, _)) => (*off, false),
                None => (self.pending(), false),
            };
            debug_assert!(chunk_end > 0 && chunk_end <= self.pending());
            let control = if attach_now {
                let batch = &self.attachments.front().expect("checked above").1;
                let raws = batch.raw_fds();
                let used = self.control.encode_rights(&raws)?;
                Some(self.control.encoded_prefix(used))
            } else {
                None
            };
            let n = match stream.send_chunk(&self.out[self.sent..self.sent + chunk_end], control) {
                Ok(n) => n,
                Err(e) if is_would_block(&e) => break,
                Err(e) => return Err(e),
            };
            if n == 0 {
                return Err(zero_progress());
            }
            if attach_now {
                // The kernel duplicated these FDs into the socket with
                // the first accepted byte; close our originals now.
                if let Some((_, batch)) = self.attachments.pop_front() {
                    self.queued_fds -= batch.len();
                }
            }
            // Advance the cursor, not the buffer: the dead prefix is
            // reclaimed by compaction below, amortized O(1) per byte.
            self.sent += n;
            for (off, _) in &mut self.attachments {
                *off = off.saturating_sub(n);
            }
            self.compact_if_ripe();
            let pending = self.pending() as u64;
            if pending >= self.config.low_water {
                self.below_low = false;
            } else if !self.below_low {
                self.below_low = true;
                self.hooks.on_drain(pending);
            }
            if single_chunk {
                break;
            }
        }
        // State-based transitions: fire exactly once per crossing.
        let exit_pending = self.pending() as u64;
        if exit_pending > 0 {
            if !self.congested {
                self.congested = true;
                self.hooks.on_congestion(exit_pending);
            }
        } else if self.congested {
            self.congested = false;
            self.hooks.on_recovery();
        }
        Ok(if self.pending() == 0 {
            SendOutcome::Sent
        } else {
            SendOutcome::Congested {
                pending_bytes: exit_pending,
            }
        })
    }
}

impl FramedWriter<crate::backpressure::NoHooks> {
    /// A writer without an observer (the plain case).
    #[must_use]
    pub fn without_hooks(limits: Limits) -> FramedWriter<crate::backpressure::NoHooks> {
        FramedWriter::new(limits, crate::backpressure::NoHooks)
    }
}

/// Read the header `fd_count` from a candidate message (send-side
/// consistency check).
fn parse_fd_count(msg: &[u8]) -> Result<u16> {
    if msg.len() < HEADER_BYTES {
        return Err(bad_send("message shorter than the 16-byte header"));
    }
    let bytes: [u8; 2] = [msg[14], msg[15]];
    Ok(u16::from_le_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backpressure::{HookEvent, HookRecorder};
    use crate::frame::ALIGN;

    fn msg_with(words: u32, fd_count: u16, payload_fill: u8) -> Vec<u8> {
        let mut msg = vec![0u8; HEADER_BYTES + words as usize * ALIGN];
        msg[0..4].copy_from_slice(&words.to_le_bytes());
        msg[14..16].copy_from_slice(&fd_count.to_le_bytes());
        for (i, rx) in msg[HEADER_BYTES..].iter_mut().enumerate() {
            *rx = payload_fill.wrapping_add(i as u8);
        }
        msg
    }

    fn fds(n: usize) -> FdList {
        let mut l = FdList::new();
        for _ in 0..n {
            l.push(crate::sys::eventfd_owned().unwrap());
        }
        l
    }

    #[test]
    fn rejects_short_misaligned_and_oversize() {
        let (mut tx, _b) = TransportStream::pair().unwrap();
        let mut writer = FramedWriter::without_hooks(Limits::DEFAULT);
        let mut empty = FdList::new();
        assert!(writer.send_msg(&mut tx, &[0u8; 8], &mut empty).is_err());
        assert!(writer.send_msg(&mut tx, &[0u8; 20], &mut empty).is_err());
        let huge = msg_with(200_000, 0, 1);
        assert!(writer.send_msg(&mut tx, &huge, &mut empty).is_err());
        // Nothing was queued by the rejections.
        assert_eq!(writer.pending_bytes(), 0);
    }

    #[test]
    fn rejects_fd_count_desync() {
        let (mut tx, _b) = TransportStream::pair().unwrap();
        let mut writer = FramedWriter::without_hooks(Limits::DEFAULT);
        let mut two = fds(2);
        let msg = msg_with(1, 3, 0); // header claims 3
        let e = writer.send_msg(&mut tx, &msg, &mut two).unwrap_err();
        assert_eq!(e.wire_code(), Some(ldp_core::error::ErrorCode::FdMismatch));
        // The batch was not consumed.
        assert_eq!(two.len(), 2);
    }

    #[test]
    fn simple_send_reaches_peer_whole() {
        let (mut tx, mut rx) = TransportStream::pair().unwrap();
        let mut writer = FramedWriter::without_hooks(Limits::DEFAULT);
        let mut reader = crate::reader::FramedReader::new(Limits::DEFAULT);
        let msg = msg_with(2, 0, 0xAB);
        let mut none = FdList::new();
        assert!(matches!(
            writer.send_msg(&mut tx, &msg, &mut none).unwrap(),
            SendOutcome::Sent
        ));
        let frame = reader.recv_msg(&mut rx).unwrap();
        assert_eq!(frame.message_bytes(), &msg[..]);
    }

    #[test]
    fn overflow_rejects_without_consuming() {
        let (mut tx, _b) = TransportStream::pair().unwrap();
        // Nonblocking writer + never-reading peer: once the socket buffer
        // fills, the local queue grows 32 bytes at tx time until the 96-byte
        // ceiling rejects the next message.
        tx.set_nonblocking(true).unwrap();
        // Nonblocking writer + never-reading peer: once the socket buffer
        // fills, the local queue grows 32 bytes at tx time until the
        // 96-byte ceiling rejects the next message. (The message ceiling
        // must not exceed the queue ceiling — a legal message is always
        // queueable, see `BackpressureConfig::from_limits`.)
        let limits = Limits {
            message_bytes: 96,
            event_queue_bytes: 96,
            ..Limits::DEFAULT
        };
        let recorder = HookRecorder::new();
        let mut writer = FramedWriter::new(limits, recorder);
        let mut overflowed = false;
        for i in 0..20_000 {
            let msg = msg_with(2, 0, i as u8); // 32 bytes
            match writer.send_msg(&mut tx, &msg, &mut FdList::new()) {
                Ok(_) => {}
                Err(e) => {
                    assert_eq!(
                        e.wire_code(),
                        Some(ldp_core::error::ErrorCode::LimitExceeded)
                    );
                    overflowed = true;
                    break;
                }
            }
        }
        assert!(overflowed, "the 96-byte ceiling must reject eventually");
        assert!(
            writer.pending_bytes() <= 96,
            "queue must respect the ceiling"
        );
        assert!(writer
            .hooks()
            .events()
            .iter()
            .any(|e| matches!(e, HookEvent::Overflow(_, 96))));
    }

    #[test]
    fn queued_fd_bound_rejects_unbounded_batches() {
        let (mut tx, _b) = TransportStream::pair().unwrap();
        // Non-blocking writer + never-reading peer -> queue accumulates.
        tx.set_nonblocking(true).unwrap();
        let limits = Limits {
            message_bytes: 64,
            event_queue_bytes: 1 << 20,
            client_fds: 4,
            ..Limits::DEFAULT
        };
        let mut writer = FramedWriter::new(limits, HookRecorder::new());
        let mut rejected = false;
        for i in 0..30_000 {
            let mut batch = fds(2);
            let msg = msg_with(1, 2, i as u8);
            if let Err(e) = writer.send_msg(&mut tx, &msg, &mut batch) {
                assert_eq!(
                    e.wire_code(),
                    Some(ldp_core::error::ErrorCode::LimitExceeded)
                );
                rejected = true;
                break;
            }
        }
        assert!(rejected, "queued-fd bound (client_fds=4) must reject");
        assert!(writer.queued_fds() <= 4);
    }

    #[test]
    fn hooks_fire_congestion_drain_recovery() {
        let (mut tx, mut rx) = TransportStream::pair().unwrap();
        tx.set_nonblocking(true).unwrap();
        rx.set_nonblocking(true).unwrap();
        let limits = Limits {
            message_bytes: 1 << 20,
            event_queue_bytes: 1 << 20,
            ..Limits::DEFAULT
        };
        let recorder = HookRecorder::new();
        let mut writer = FramedWriter::new(limits, recorder);
        // Fill until congested, counting messages.
        let mut sent = 0;
        while !writer.is_congested() && sent < 20_000 {
            let msg = msg_with(8192, 0, sent as u8);
            if let SendOutcome::Congested { .. } =
                writer.send_msg(&mut tx, &msg, &mut FdList::new()).unwrap()
            {
                sent += 1;
                break;
            }
            sent += 1;
        }
        assert!(writer.is_congested(), "socket should fill after ~200 KiB");
        // Alternate flushing and reading until the queue drains; every
        // sent message must arrive exactly once.
        let mut reader = crate::reader::FramedReader::new(Limits {
            message_bytes: 1 << 20,
            ..Limits::DEFAULT
        });
        let mut received = 0;
        let mut drained = false;
        for _ in 0..100 {
            if let FlushOutcome::Drained = writer.flush(&mut tx).unwrap() {
                drained = true;
                break;
            }
            loop {
                match reader.recv_msg(&mut rx) {
                    Ok(_) => received += 1,
                    Err(e) if is_would_block(&e) => break,
                    Err(e) => panic!("{e}"),
                }
            }
        }
        assert!(drained, "queue should drain once the peer reads");
        loop {
            match reader.recv_msg(&mut rx) {
                Ok(_) => received += 1,
                Err(e) if is_would_block(&e) => break,
                Err(e) => panic!("{e}"),
            }
        }
        assert_eq!(received, sent, "every message arrives exactly once");
        let events = writer.hooks().events();
        assert!(events.iter().any(|e| matches!(e, HookEvent::Congestion(_))));
        assert!(events.iter().any(|e| matches!(e, HookEvent::Recovery)));
    }
}
