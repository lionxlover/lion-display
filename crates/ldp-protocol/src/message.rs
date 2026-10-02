//! Whole-message encoding: envelope + tagged arguments.
//!
//! [`Message`] is the in-memory form between the codec and dispatch. The
//! FD table itself is transport-owned (`ldp-transport`, Phase 3); the
//! message carries only its *size* — `fd` arguments are indices checked
//! against it, and the encoder derives the count from the arguments, so
//! FD/argument desynchronization is impossible by construction.

use ldp_core::error::{ErrorCode, LdpError, Result};
use ldp_core::limits::Limits;
use ldp_core::wire::{ArgType, Primitive, Value};

use crate::bytes::{Reader, Writer};
use crate::codec::{decode_arg, encode_arg};
use crate::envelope::{Flags, Header, HEADER_BYTES};

/// How aggressively to validate. Tolerant is the protocol default (the
/// wire contract's forward-compatibility rules); strict is for tooling,
/// conformance suites, and fuzzing.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub enum ValidationMode {
    /// Wire-contract defaults: unknown enum values and undeclared bitset
    /// bits are structurally valid (ignored at the semantic layer);
    /// padding content is not inspected.
    #[default]
    Tolerant,
    /// Tooling mode: additionally rejects nonzero padding, undeclared
    /// enum values, and bits above a bitset's declared width.
    Strict,
}

/// One complete message.
#[derive(Clone, PartialEq, Debug)]
pub struct Message {
    /// Target object (raw wire ID; identity is connection state).
    pub object_id: u32,
    /// Request or event opcode.
    pub opcode: u32,
    /// Envelope flags.
    pub flags: Flags,
    /// Decoded arguments, in order.
    pub args: Vec<Value>,
}

impl Message {
    /// Start building a message.
    #[must_use]
    pub const fn new(object_id: u32, opcode: u32) -> Message {
        Message {
            object_id,
            opcode,
            flags: Flags::NONE,
            args: Vec::new(),
        }
    }

    /// Mark `URGENT` (builder-style).
    #[must_use]
    pub const fn urgent(mut self) -> Message {
        self.flags = self.flags.with_urgent();
        self
    }

    /// Mark `HAS_REPLY` (builder-style).
    #[must_use]
    pub const fn reply(mut self) -> Message {
        self.flags = self.flags.with_reply();
        self
    }

    /// Append one argument (builder-style).
    #[must_use]
    pub fn arg(mut self, v: Value) -> Message {
        self.args.push(v);
        self
    }

    /// The FD-table size this message's arguments require: one past the
    /// highest `fd` index (0 when no FDs are referenced). The encoder
    /// writes exactly this into the header.
    #[must_use]
    pub fn required_fd_count(&self) -> u32 {
        let mut max: Option<u32> = None;
        for v in &self.args {
            let idx = match v {
                Value::Fd(i) => Some(*i),
                Value::Array { element, items } if *element == ArgType::Fd => items
                    .iter()
                    .map(|p| match p {
                        Primitive::Fd(i) => *i,
                        _ => 0,
                    })
                    .max(),
                _ => None,
            };
            if let Some(i) = idx {
                max = Some(max.map_or(i, |m: u32| m.max(i)));
            }
        }
        max.map_or(0, |m| m + 1)
    }

    /// Encode into framed bytes (header + payload).
    ///
    /// # Errors
    ///
    /// [`LdpError::Limit`] when the result would exceed
    /// `limits.message_bytes` or a string argument exceeds
    /// `limits.string_bytes`; [`LdpError::Logic`] for unrepresentable
    /// values (caller bugs — see [`crate::codec`]).
    pub fn encode(&self, limits: &Limits) -> Result<Vec<u8>> {
        let fd_count = self.required_fd_count();
        if fd_count > limits.fds_per_message {
            return Err(LdpError::Limit {
                kind: ldp_core::error::LimitKind::FdsPerMessage,
                value: fd_count as u64,
            });
        }
        let mut w = Writer::new();
        for v in &self.args {
            encode_arg(&mut w, v, limits)?;
        }
        let payload = w.into_bytes();
        debug_assert_eq!(payload.len() % 8, 0);
        let total = HEADER_BYTES as u64 + payload.len() as u64;
        if total > limits.message_bytes {
            return Err(LdpError::Limit {
                kind: ldp_core::error::LimitKind::MessageBytes,
                value: total,
            });
        }
        let header = Header {
            payload_words: (payload.len() / 8) as u32,
            object_id: self.object_id,
            opcode: self.opcode,
            flags: self.flags,
            fd_count: fd_count as u16,
        };
        let mut bytes = Vec::with_capacity(total as usize);
        bytes.extend_from_slice(&header.to_bytes());
        bytes.extend_from_slice(&payload);
        Ok(bytes)
    }
}

/// Decode one framed message (stages 1–2 of `docs/protocol.md` §9:
/// framing and structural decode; the signature check is
/// [`crate::check_signature`]).
///
/// `ancillary_fds` is the number of FDs actually received in the
/// message's SCM_RIGHTS array — it must equal the header's `fd_count`
/// (mismatch is fatal, `fd_mismatch`).
///
/// # Errors
///
/// * [`ErrorCode::MalformedMessage`] — framing, alignment, tag, string
///   structure, or bool/float domain violations,
/// * [`ErrorCode::FdMismatch`] — fd_count/ancillary disagreement or an
///   out-of-range `fd` index,
/// * [`ErrorCode::InvalidString`] — non-UTF-8 strings,
/// * [`ErrorCode::LimitExceeded`] — message size, FD count, string
///   length, or region-rect ceilings.
pub fn decode(
    bytes: &[u8],
    ancillary_fds: u32,
    limits: &Limits,
    mode: ValidationMode,
) -> Result<Message> {
    let header = Header::from_bytes(bytes)?;
    if header.total_bytes() != bytes.len() as u64 {
        return Err(LdpError::malformed(
            ErrorCode::MalformedMessage,
            "frame length does not equal header + payload",
        ));
    }
    if header.total_bytes() > limits.message_bytes {
        return Err(LdpError::Limit {
            kind: ldp_core::error::LimitKind::MessageBytes,
            value: header.total_bytes(),
        });
    }
    let fd_count = u32::from(header.fd_count);
    if fd_count != ancillary_fds {
        return Err(LdpError::malformed(
            ErrorCode::FdMismatch,
            "header fd_count disagrees with the ancillary array",
        ));
    }
    if fd_count > limits.fds_per_message {
        return Err(LdpError::Limit {
            kind: ldp_core::error::LimitKind::FdsPerMessage,
            value: fd_count as u64,
        });
    }
    let mut r = Reader::new(&bytes[HEADER_BYTES..]);
    let mut args = Vec::new();
    while r.remaining() > 0 {
        args.push(decode_arg(&mut r, fd_count, limits, mode)?);
    }
    Ok(Message {
        object_id: header.object_id,
        opcode: header.opcode,
        flags: header.flags,
        args,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(object: u32, opcode: u32, args: &[Value]) -> Vec<u8> {
        let mut m = Message::new(object, opcode);
        for a in args {
            m = m.arg(a.clone());
        }
        m.encode(&Limits::DEFAULT).unwrap()
    }

    #[test]
    fn round_trip_multi_arg_message() {
        let m = Message::new(0x8000_0100, 4)
            .arg(Value::Int32(10))
            .arg(Value::Int32(20))
            .arg(Value::Uint32(100))
            .arg(Value::Uint32(50));
        let bytes = m.encode(&Limits::DEFAULT).unwrap();
        // Worked example: 4 scalar units = 32 payload bytes.
        assert_eq!(bytes.len(), 48);
        let back = decode(&bytes, 0, &Limits::DEFAULT, ValidationMode::Strict).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn zero_arg_message_is_header_only() {
        let m = Message::new(1, 3).reply();
        let bytes = m.encode(&Limits::DEFAULT).unwrap();
        assert_eq!(bytes.len(), 16);
        let back = decode(&bytes, 0, &Limits::DEFAULT, ValidationMode::Tolerant).unwrap();
        assert_eq!(back, m);
        assert!(back.flags.has_reply());
    }

    #[test]
    fn fd_count_is_derived_from_arguments() {
        let m = Message::new(2, 1)
            .arg(Value::Fd(0))
            .arg(Value::array(ArgType::Fd, vec![Primitive::Fd(2)]).unwrap());
        assert_eq!(m.required_fd_count(), 3);
        let bytes = m.encode(&Limits::DEFAULT).unwrap();
        let back = decode(&bytes, 3, &Limits::DEFAULT, ValidationMode::Tolerant).unwrap();
        assert_eq!(back, m);
        // Ancillary disagreement is fatal.
        let err = decode(&bytes, 2, &Limits::DEFAULT, ValidationMode::Tolerant).unwrap_err();
        assert_eq!(err.wire_code(), Some(ErrorCode::FdMismatch));
    }

    #[test]
    fn frame_length_mismatch_and_oversize_are_distinct() {
        let bytes = frame(1, 1, &[Value::Uint32(1)]);
        // Trailing garbage: total != 16 + 8*words.
        let mut long = bytes.clone();
        long.push(0);
        let err = decode(&long, 0, &Limits::DEFAULT, ValidationMode::Tolerant).unwrap_err();
        assert_eq!(err.wire_code(), Some(ErrorCode::MalformedMessage));
        // Exact frame but over a tight limit.
        let tight = Limits {
            message_bytes: 16,
            ..Limits::DEFAULT
        };
        let err = decode(&bytes, 0, &tight, ValidationMode::Tolerant).unwrap_err();
        assert_eq!(err.wire_code(), Some(ErrorCode::LimitExceeded));
    }

    #[test]
    fn fd_ceiling_is_enforced_on_encode() {
        let m = Message::new(2, 1).arg(Value::Fd(63));
        assert_eq!(m.required_fd_count(), 64);
        assert!(m.encode(&Limits::DEFAULT).is_ok());
        let m = Message::new(2, 1).arg(Value::Fd(64));
        let err = m.encode(&Limits::DEFAULT).unwrap_err();
        assert_eq!(err.wire_code(), Some(ErrorCode::LimitExceeded));
    }

    #[test]
    fn encode_rejects_oversize_messages() {
        let tight = Limits {
            message_bytes: 24,
            ..Limits::DEFAULT
        };
        let m = Message::new(1, 1)
            .arg(Value::Uint32(1))
            .arg(Value::Uint32(2));
        let err = m.encode(&tight).unwrap_err();
        assert_eq!(err.wire_code(), Some(ErrorCode::LimitExceeded));
    }
}
