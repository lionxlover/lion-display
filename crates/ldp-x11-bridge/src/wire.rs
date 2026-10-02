//! X11 wire primitives: byte order, framing, and 32-byte envelopes.
//!
//! The X11 core protocol is byte-order agnostic: the client picks its
//! preferred order in the first setup byte (`'l'` LSB-first, `'B'`
//! MSB-first) and every multi-byte field in both directions follows it
//! for the whole connection. Requests are 4-byte aligned; the length
//! field of the header counts 4-byte units including the header. A
//! length of exactly 0 means the BIG-REQUESTS escape: a 4-byte pad
//! followed by a full 64-bit length in units of 4 bytes (including the
//! 16-byte prefix).
//!
//! Replies, events and errors are fixed 32-byte envelopes (replies carry
//! a 4-byte word count for anything beyond the first 32 bytes). This
//! module owns the encode/decode of those envelopes' headers and the
//! checked primitive readers/writers; higher layers fill payload fields.
//!
//! One deliberate simplification, documented here and pinned by tests:
//! generic events (code 35+, the "xge" extended form) are not part of
//! the Phase 17 subset — every event the bridge emits is a classic
//! 32-byte core event carrying the 16-bit sequence number in its
//! header (KeymapNotify excepted: no sequence at all).

#![forbid(unsafe_code)]

use ldp_core::limits::Limits;

/// A connection's byte order, fixed by the first setup byte.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Endian {
    /// LSB first (`'l'` = 0x6c): the common Linux client order.
    Lsb,
    /// MSB first (`'B'` = 0x42).
    Msb,
}

impl Endian {
    /// Map the setup byte; anything else is a fatal handshake violation.
    ///
    /// # Errors
    /// [`WireError::BadByteOrder`] for any byte other than 0x6c / 0x42 —
    /// the connection cannot continue and must be torn down.
    pub fn from_setup_byte(byte: u8) -> Result<Self, WireError> {
        match byte {
            0x6c => Ok(Endian::Lsb),
            0x42 => Ok(Endian::Msb),
            other => Err(WireError::BadByteOrder(other)),
        }
    }

    /// Encode one `u16` in this order.
    #[must_use]
    pub fn put_u16(self, v: u16) -> [u8; 2] {
        match self {
            Endian::Lsb => v.to_le_bytes(),
            Endian::Msb => v.to_be_bytes(),
        }
    }

    /// Encode one `u32` in this order.
    #[must_use]
    pub fn put_u32(self, v: u32) -> [u8; 4] {
        match self {
            Endian::Lsb => v.to_le_bytes(),
            Endian::Msb => v.to_be_bytes(),
        }
    }

    /// Decode one `u16` at `at` (bounds-checked by the caller).
    #[must_use]
    pub fn get_u16(self, buf: &[u8], at: usize) -> u16 {
        let b = [buf[at], buf[at + 1]];
        match self {
            Endian::Lsb => u16::from_le_bytes(b),
            Endian::Msb => u16::from_be_bytes(b),
        }
    }

    /// Decode one `u32` at `at` (bounds-checked by the caller).
    #[must_use]
    pub fn get_u32(self, buf: &[u8], at: usize) -> u32 {
        let b = [buf[at], buf[at + 1], buf[at + 2], buf[at + 3]];
        match self {
            Endian::Lsb => u32::from_le_bytes(b),
            Endian::Msb => u32::from_be_bytes(b),
        }
    }

    /// Decode one `i16` at `at` (bounds-checked by the caller).
    #[must_use]
    pub fn get_i16(self, buf: &[u8], at: usize) -> i16 {
        self.get_u16(buf, at) as i16
    }

    /// Decode one `i32` at `at` (bounds-checked by the caller).
    #[must_use]
    pub fn get_i32(self, buf: &[u8], at: usize) -> i32 {
        self.get_u32(buf, at) as i32
    }
}

/// Fatal wire violations (the connection dies; legal X errors are not
/// Rust errors — they travel to the client as error events).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum WireError {
    /// The setup byte-order byte was neither `'l'` nor `'B'`.
    BadByteOrder(u8),
    /// A request length is not a whole number of 4-byte units.
    UnalignedLength(u64),
    /// The framed request is shorter than its own 4-byte header.
    ShortHeader,
    /// A 0-length header with the BIG-REQUESTS escape not enabled.
    ZeroLength,
    /// The request exceeds the bridge's request-size ceiling.
    Oversize(u64),
}

/// Padding needed to bring `n` bytes up to a 4-byte boundary.
#[must_use]
pub const fn pad4(n: usize) -> usize {
    (4 - (n & 3)) & 3
}

/// One framed request: opcode, the header's second data byte, and the
/// payload *after* the (4- or 16-byte) header, padding included.
#[derive(Clone, Copy, Debug)]
pub struct FramedRequest<'a> {
    /// Major opcode (0 = core, 128+ = extension).
    pub opcode: u8,
    /// Second header byte (minor opcode for extensions, else flags).
    pub data: u8,
    /// Bytes after the request header, including trailing padding.
    pub payload: &'a [u8],
}

/// Extract one complete request from `buf`, honoring BIG-REQUESTS.
///
/// Returns the framed request and the number of bytes consumed. Only
/// whole requests are consumed: a trailing partial request stays in the
/// buffer for the next read. `endian` is the connection's negotiated
/// order (the setup handshake runs before any request framing).
///
/// # Errors
/// [`WireError`] for the fatal framing violations above.
pub fn frame_request(
    buf: &[u8],
    endian: Endian,
    bigreq: bool,
) -> Result<(FramedRequest<'_>, usize), WireError> {
    if buf.len() < 4 {
        return Err(WireError::ShortHeader);
    }
    let short_len = endian.get_u16(buf, 2) as u64;
    let opcode = buf[0];
    let data = buf[1];
    if short_len == 0 {
        if !bigreq {
            return Err(WireError::ZeroLength);
        }
        if buf.len() < 16 {
            return Err(WireError::ShortHeader);
        }
        let len = escape_u64(buf, endian);
        let bytes = len.saturating_mul(4);
        if bytes > max_request_bytes() {
            return Err(WireError::Oversize(bytes));
        }
        if bytes < 16 || buf.len() < bytes as usize {
            return Err(WireError::ShortHeader);
        }
        Ok((
            FramedRequest {
                opcode,
                data,
                payload: &buf[16..bytes as usize],
            },
            bytes as usize,
        ))
    } else {
        let bytes = short_len * 4;
        if buf.len() < bytes as usize {
            return Err(WireError::ShortHeader);
        }
        Ok((
            FramedRequest {
                opcode,
                data,
                payload: &buf[4..bytes as usize],
            },
            bytes as usize,
        ))
    }
}

/// Decode the BIG-REQUESTS 64-bit length at bytes 8..16 in the
/// connection's byte order.
#[must_use]
fn escape_u64(buf: &[u8], endian: Endian) -> u64 {
    match endian {
        Endian::Lsb => {
            let lo = u64::from(endian.get_u32(buf, 8));
            let hi = u64::from(endian.get_u32(buf, 12));
            (hi << 32) | lo
        }
        Endian::Msb => {
            let w1 = u64::from(endian.get_u32(buf, 8));
            let w2 = u64::from(endian.get_u32(buf, 12));
            (w1 << 32) | w2
        }
    }
}

/// The bridge's request-size ceiling (64 MiB, the LDP `large_messages`
/// frame ceiling — an X request larger than an LDP frame could never be
/// forwarded whole).
#[must_use]
pub fn max_request_bytes() -> u64 {
    Limits::LARGE_MESSAGES.message_bytes
}

/// Monotonic request sequence counter (wraps at 16 bits, full 32-bit
/// value preserved for event envelopes).
#[derive(Clone, Copy, Debug, Default)]
pub struct Seq {
    full: u32,
}

impl Seq {
    /// Advance past one request; returns the sequence number assigned
    /// to it (the number replies and errors it triggers carry).
    #[allow(clippy::should_implement_trait)] // not an Iterator: hands
    // out opaque request numbers, not a stream of items
    #[must_use]
    pub fn next(&mut self) -> u32 {
        self.full = self.full.wrapping_add(1);
        self.full
    }

    /// The current full-width counter (last assigned number).
    #[must_use]
    pub const fn current(self) -> u32 {
        self.full
    }

    /// The low 16 bits as they appear in envelope headers.
    #[must_use]
    pub const fn low16(self) -> u16 {
        (self.full & 0xffff) as u16
    }
}

/// Append a 32-byte reply envelope header (success byte 1).
///
/// `payload_words` counts 4-byte units following the fixed 32 bytes.
pub fn reply_header(out: &mut Vec<u8>, endian: Endian, seq: u32, payload_words: u32) {
    out.push(1);
    out.push(0);
    out.extend_from_slice(&endian.put_u16((seq & 0xffff) as u16));
    out.extend_from_slice(&endian.put_u32(payload_words));
}

/// Append a 32-byte error envelope: code, offending value, opcodes.
pub fn error_envelope(
    out: &mut Vec<u8>,
    endian: Endian,
    seq: u32,
    code: u8,
    bad_value: u32,
    minor: u8,
    major: u8,
) {
    out.push(0);
    out.push(code);
    out.extend_from_slice(&endian.put_u16((seq & 0xffff) as u16));
    out.extend_from_slice(&endian.put_u32(bad_value));
    out.extend_from_slice(&endian.put_u16(minor as u16));
    out.push(major);
    out.extend_from_slice(&[0; 21]);
}

/// Append a core event envelope prefix: code, detail, 16-bit sequence.
/// The caller fills the remaining 20 bytes of payload, then closes with
/// [`event_tail`]. Both operate on an **isolated event buffer** (the
/// dispatcher appends the finished 32-byte event to its output stream).
pub fn event_prefix(out: &mut Vec<u8>, endian: Endian, code: u8, detail: u8, seq: u32) {
    out.push(code);
    out.push(detail);
    out.extend_from_slice(&endian.put_u16((seq & 0xffff) as u16));
}

/// Close an **isolated event buffer** opened by [`event_prefix`]:
/// zero-pad the payload to the full 32-byte envelope. Core events
/// carry only the 16-bit sequence number (in the header written by
/// `event_prefix`); the trailing bytes are unused. KeymapNotify is
/// exempt from prefix/tail entirely (its 31 key bytes run to the end).
pub fn event_tail(out: &mut Vec<u8>) {
    debug_assert!(out.len() <= 32, "event payload overflow");
    while out.len() < 32 {
        out.push(0);
    }
}

/// Encode a STRING8 with 4-byte padding.
pub fn put_string8(out: &mut Vec<u8>, s: &[u8]) {
    out.extend_from_slice(s);
    for _ in 0..pad4(s.len()) {
        out.push(0);
    }
}

/// Read a length-prefixed region from a payload at `at` with `len`
/// bytes plus padding; returns (slice, next_offset).
///
/// # Errors
/// [`WireError::ShortHeader`] when the payload ends early.
pub fn take_bytes(payload: &[u8], at: usize, len: usize) -> Result<(&[u8], usize), WireError> {
    let end = at.checked_add(len).ok_or(WireError::Oversize(len as u64))?;
    if end > payload.len() {
        return Err(WireError::ShortHeader);
    }
    let next = end + pad4(len);
    if next > payload.len() {
        // The trailing pad of the final field may be elided only when
        // the request itself ends exactly at the data; X requires the
        // pad, so a missing pad is a short request.
        return Err(WireError::ShortHeader);
    }
    Ok((&payload[at..end], next))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_order_negotiation() {
        assert_eq!(Endian::from_setup_byte(0x6c), Ok(Endian::Lsb));
        assert_eq!(Endian::from_setup_byte(0x42), Ok(Endian::Msb));
        assert_eq!(Endian::from_setup_byte(0), Err(WireError::BadByteOrder(0)));
    }

    #[test]
    fn short_framing_stays_partial() {
        // Promises 16 bytes (length field 4), buffer holds 5: incomplete.
        let buf = [1u8, 0, 4, 0, 9];
        assert!(matches!(
            frame_request(&buf, Endian::Lsb, false),
            Err(WireError::ShortHeader)
        ));
        // Sub-header alone is incomplete too.
        assert!(matches!(
            frame_request(&buf[..3], Endian::Lsb, false),
            Err(WireError::ShortHeader)
        ));
        // Complete 8-byte request consumes exactly 8.
        let buf = [1u8, 0, 2, 0, 9, 0, 0, 0];
        let (framed, used) = frame_request(&buf, Endian::Lsb, false).unwrap();
        assert_eq!((framed.opcode, framed.data), (1, 0));
        assert_eq!(used, 8);
        assert_eq!(framed.payload.len(), 4);
    }

    #[test]
    fn bigreq_escape_requires_enable() {
        let mut buf = vec![20u8, 0, 0, 0];
        buf.extend_from_slice(&[0; 4]);
        let total: u64 = 5; // 20 bytes
        buf.extend_from_slice(&total.to_le_bytes());
        buf.extend_from_slice(&[0; 4]);
        assert!(matches!(
            frame_request(&buf, Endian::Lsb, false),
            Err(WireError::ZeroLength)
        ));
        let (framed, used) = frame_request(&buf, Endian::Lsb, true).unwrap();
        assert_eq!((framed.opcode, framed.data), (20, 0));
        assert_eq!(used, 20);
        assert_eq!(framed.payload.len(), 4);
    }

    #[test]
    fn bigreq_decode_follows_connection_endian() {
        let mut buf = vec![20u8, 0, 0, 0, 0, 0, 0, 0];
        let total: u64 = 6;
        buf.extend_from_slice(&total.to_be_bytes());
        buf.extend_from_slice(&[0; 8]);
        let (_, used) = frame_request(&buf, Endian::Msb, true).unwrap();
        assert_eq!(used, 24);
        // The same buffer read LSB-first escapes nothing sane: 6 as BE
        // reads as 6 << 56 units -> oversize.
        assert!(matches!(
            frame_request(&buf, Endian::Lsb, true),
            Err(WireError::Oversize(_) | WireError::ShortHeader)
        ));
    }

    #[test]
    fn bigreq_oversize_rejected() {
        let mut buf = vec![20u8, 0, 0, 0, 0, 0, 0, 0];
        let total: u64 = max_request_bytes() / 4 + 4;
        buf.extend_from_slice(&total.to_le_bytes());
        assert!(matches!(
            frame_request(&buf, Endian::Lsb, true),
            Err(WireError::Oversize(_))
        ));
    }

    #[test]
    fn sequence_wraps_low_bits_only() {
        let mut s = Seq::default();
        for _ in 0..3 {
            let _ = s.next();
        }
        assert_eq!(s.current(), 3);
        assert_eq!(s.low16(), 3);
        let mut s = Seq { full: 0xffff };
        let n = s.next();
        assert_eq!(n, 0x1_0000);
        assert_eq!(s.low16(), 0);
    }

    #[test]
    fn envelopes_are_32_bytes() {
        let mut err = Vec::new();
        error_envelope(&mut err, Endian::Lsb, 7, 3, 0x1234, 0, 8);
        assert_eq!(err.len(), 32);
        assert_eq!(err[0], 0);
        assert_eq!(err[1], 3);
        assert_eq!(&err[2..4], &7u16.to_le_bytes());
        assert_eq!(&err[4..8], &0x1234u32.to_le_bytes());
        assert_eq!(err[10], 8);

        let mut reply = Vec::new();
        reply_header(&mut reply, Endian::Msb, 9, 0);
        assert_eq!(reply.len(), 8);

        // Events are built in their own buffer: prefix + payload + tail.
        let mut ev = Vec::new();
        event_prefix(&mut ev, Endian::Msb, 21, 0, 9);
        ev.extend_from_slice(&[0; 24]);
        event_tail(&mut ev);
        assert_eq!(ev.len(), 32);
        assert_eq!(&ev[..4], &[21, 0, 0, 9]);
    }

    #[test]
    fn strings_pad_to_four() {
        let mut out = Vec::new();
        put_string8(&mut out, b"abc");
        assert_eq!(&out, b"abc\0");
        let mut out = Vec::new();
        put_string8(&mut out, b"abcd");
        assert_eq!(&out, b"abcd");
        let mut out = Vec::new();
        put_string8(&mut out, b"abcde");
        assert_eq!(out.len(), 8);
    }

    #[test]
    fn take_bytes_enforces_padding() {
        let p = [1u8, 2, 3, 4, 5, 6, 7, 8];
        // Field at 1 of length 3: slice [2,3,4], next field at 1+3+1pad.
        let (s, next) = take_bytes(&p, 1, 3).unwrap();
        assert_eq!(s, &[2, 3, 4]);
        assert_eq!(next, 5);
        let (s, next) = take_bytes(&p, 0, 2).unwrap();
        assert_eq!(s, &[1, 2]);
        assert_eq!(next, 4);
        assert!(take_bytes(&p, 6, 4).is_err());
        // A field whose pad would overrun the payload is short.
        let short = [1u8, 2, 3, 4, 5, 6, 7];
        assert!(take_bytes(&short, 4, 3).is_err());
    }
}
