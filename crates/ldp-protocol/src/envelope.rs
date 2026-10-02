//! The 16-byte message envelope (`docs/protocol.md` §2).
//!
//! ```text
//! offset  size  field
//! ──────  ────  ─────────────
//! 0       4     payload_words (u32)   payload length in 8-byte words
//! 4       4     object_id     (u32)   target object
//! 8       4     opcode        (u32)   request or event opcode
//! 12      2     flags         (u16)
//! 14      2     fd_count      (u16)   FDs in the ancillary array
//! ```
//!
//! All fields little-endian. Reserved flag bits are fatal: extensions are
//! negotiated, never guessed.

use ldp_core::error::{ErrorCode, LdpError, Result};

/// Envelope size in bytes.
pub const HEADER_BYTES: usize = 16;

/// Hint bit 0: dispatch this message first (input-class events).
pub const FLAG_URGENT: u16 = 1 << 0;

/// Hint bit 1: the request expects a correlated reply event.
pub const FLAG_HAS_REPLY: u16 = 1 << 1;

/// Bits 2–15: reserved, must be zero on the wire.
pub const FLAG_RESERVED_MASK: u16 = !(FLAG_URGENT | FLAG_HAS_REPLY);

/// Envelope flags (`docs/protocol.md` §2.1).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default, Debug)]
pub struct Flags(u16);

impl Flags {
    /// No flags set.
    pub const NONE: Flags = Flags(0);

    /// Build from a raw wire word, rejecting reserved bits.
    ///
    /// # Errors
    ///
    /// [`LdpError::Malformed`] with [`ErrorCode::MalformedMessage`] when
    /// any reserved bit is set — unknown flags are fatal by design.
    pub fn from_wire(raw: u16) -> Result<Flags> {
        if raw & FLAG_RESERVED_MASK == 0 {
            Ok(Flags(raw))
        } else {
            Err(LdpError::malformed(
                ErrorCode::MalformedMessage,
                "reserved flag bits set",
            ))
        }
    }

    /// Raw wire word (reserved bits are zero by construction).
    #[must_use]
    pub const fn to_wire(self) -> u16 {
        self.0
    }

    /// Whether `URGENT` is set.
    #[must_use]
    pub const fn is_urgent(self) -> bool {
        self.0 & FLAG_URGENT != 0
    }

    /// Whether `HAS_REPLY` is set.
    #[must_use]
    pub const fn has_reply(self) -> bool {
        self.0 & FLAG_HAS_REPLY != 0
    }

    /// With `URGENT` set (builder-style).
    #[must_use]
    pub const fn with_urgent(self) -> Flags {
        Flags(self.0 | FLAG_URGENT)
    }

    /// With `HAS_REPLY` set (builder-style).
    #[must_use]
    pub const fn with_reply(self) -> Flags {
        Flags(self.0 | FLAG_HAS_REPLY)
    }
}

/// The fixed 16-byte envelope header.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Header {
    /// Payload length in 8-byte words.
    pub payload_words: u32,
    /// Target object ID (raw wire form; identity validation is
    /// server-side, `ldp-core::ids`).
    pub object_id: u32,
    /// Request or event opcode.
    pub opcode: u32,
    /// Envelope flags.
    pub flags: Flags,
    /// Number of FDs carried in the message's ancillary array.
    pub fd_count: u16,
}

impl Header {
    /// Encode into exactly 16 little-endian bytes.
    #[must_use]
    pub fn to_bytes(self) -> [u8; HEADER_BYTES] {
        let mut b = [0u8; HEADER_BYTES];
        b[0..4].copy_from_slice(&self.payload_words.to_le_bytes());
        b[4..8].copy_from_slice(&self.object_id.to_le_bytes());
        b[8..12].copy_from_slice(&self.opcode.to_le_bytes());
        b[12..14].copy_from_slice(&self.flags.to_wire().to_le_bytes());
        b[14..16].copy_from_slice(&self.fd_count.to_le_bytes());
        b
    }

    /// Decode 16 bytes; validates reserved flag bits.
    ///
    /// # Errors
    ///
    /// [`LdpError::Malformed`] when the buffer is short or reserved flag
    /// bits are set.
    pub fn from_bytes(bytes: &[u8]) -> Result<Header> {
        if bytes.len() < HEADER_BYTES {
            return Err(LdpError::malformed(
                ErrorCode::MalformedMessage,
                "frame shorter than the 16-byte header",
            ));
        }
        let word = |o: usize| -> u32 {
            u32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]])
        };
        let flags = Flags::from_wire(u16::from_le_bytes([bytes[12], bytes[13]]))?;
        Ok(Header {
            payload_words: word(0),
            object_id: word(4),
            opcode: word(8),
            flags,
            fd_count: u16::from_le_bytes([bytes[14], bytes[15]]),
        })
    }

    /// Payload size in bytes (`8 × payload_words`).
    #[must_use]
    pub const fn payload_bytes(self) -> u64 {
        8 * self.payload_words as u64
    }

    /// Total frame size in bytes (header + payload).
    #[must_use]
    pub const fn total_bytes(self) -> u64 {
        HEADER_BYTES as u64 + self.payload_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_round_trips() {
        let h = Header {
            payload_words: 131_070,
            object_id: 0x8000_0100,
            opcode: 4,
            flags: Flags::NONE.with_urgent(),
            fd_count: 3,
        };
        let bytes = h.to_bytes();
        assert_eq!(bytes.len(), 16);
        let back = Header::from_bytes(&bytes).unwrap();
        assert_eq!(back, h);
        assert_eq!(h.total_bytes(), 16 + 8 * 131_070);
        assert_eq!(h.payload_bytes(), 1_048_560);
    }

    #[test]
    fn reserved_flag_bits_are_rejected() {
        for raw in [FLAG_RESERVED_MASK, 0x0004, 0x8000, 0xFFFF] {
            assert!(Flags::from_wire(raw).is_err(), "raw {raw:#06x} must fail");
        }
        for raw in [0, FLAG_URGENT, FLAG_HAS_REPLY, FLAG_URGENT | FLAG_HAS_REPLY] {
            let f = Flags::from_wire(raw).unwrap();
            assert_eq!(f.to_wire(), raw);
        }
    }

    #[test]
    fn flag_accessors() {
        let f = Flags::NONE.with_urgent();
        assert!(f.is_urgent());
        assert!(!f.has_reply());
        let f = f.with_reply();
        assert!(f.has_reply() && f.is_urgent());
        assert_eq!(Flags::NONE, Flags::default());
    }

    #[test]
    fn short_buffer_is_an_error() {
        assert!(Header::from_bytes(&[0u8; 15]).is_err());
        assert!(Header::from_bytes(&[]).is_err());
        assert!(Header::from_bytes(&[0u8; 16]).is_ok());
    }

    #[test]
    fn exact_header_layout() {
        // docs/protocol.md §4.1 worked example header: 4 payload words,
        // object 0x80000100, opcode 4, no flags, no FDs.
        let h = Header {
            payload_words: 4,
            object_id: 0x8000_0100,
            opcode: 4,
            flags: Flags::NONE,
            fd_count: 0,
        };
        assert_eq!(
            h.to_bytes(),
            [
                0x04, 0x00, 0x00, 0x00, // payload_words
                0x00, 0x01, 0x00, 0x80, // object_id LE
                0x04, 0x00, 0x00, 0x00, // opcode
                0x00, 0x00, // flags
                0x00, 0x00, // fd_count
            ]
        );
    }
}
