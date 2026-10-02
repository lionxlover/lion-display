//! Wayland wire primitives: the object-id/opcode header, argument
//! framing, and the checked decoder.
//!
//! The wire format (pinned here, byte-exact, by tests):
//!
//! * Every message starts with one `u32`: the target object's id in
//!   the **upper 24 bits** and the opcode in the **lower 8**. Object
//!   ids therefore live in `1..=0x00ff_ffff`; 0 is invalid.
//! * Arguments follow in declaration order. Integer and fixed types
//!   are 4 bytes; strings are a `u32` byte length, the bytes, and a
//!   terminating NUL, padded so the next argument starts 4-aligned;
//!   arrays are a `u32` byte length, the bytes, and the same padding;
//!   object ids and new ids are 4 bytes; file descriptors ride the
//!   ancillary channel in order of appearance and contribute nothing
//!   to the byte stream.
//! * The decoder needs the message's signature (from the pinned
//!   schema table) to know argument boundaries — Wayland headers
//!   carry no length. Decoding stops at the first out-of-bounds or
//!   malformed field and reports how many bytes a complete frame
//!   would still need.
//!
//! The subset speaks one byte order: the protocol's native little
//! endian, everywhere, both directions.

#![forbid(unsafe_code)]

/// The maximum object id (`0x00ff_ffff`); ids above are invalid on
/// the wire — the 24-bit field cannot carry them.
pub const MAX_OBJECT_ID: u32 = 0x00ff_ffff;

/// The bridge's message-size ceiling in bytes: one LDP
/// `large_messages` frame.
///
/// A Wayland message larger than an LDP large frame could never be
/// forwarded whole, so the driver rejects length claims past this
/// bound instead of buffering toward them (the length-bomb guard —
/// same doctrine as the X11 bridge's `max_request_bytes()`).
#[must_use]
pub fn max_message_bytes() -> u64 {
    ldp_core::limits::Limits::LARGE_MESSAGES.message_bytes
}

/// One argument's wire type.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Arg {
    /// `int` — signed 32-bit.
    Int,
    /// `uint` — unsigned 32-bit.
    Uint,
    /// `fixed` — 24.8 signed fixed point.
    Fixed,
    /// NUL-terminated length-prefixed string.
    String,
    /// Length-prefixed byte array.
    Array,
    /// An existing object reference (0 = null).
    Object,
    /// A newly created object id (client → server only in this
    /// subset's schemas).
    NewId,
    /// A file descriptor (ancillary; zero byte-stream footprint).
    Fd,
}

impl Arg {
    /// The signature character.
    #[must_use]
    pub const fn sig(self) -> char {
        match self {
            Arg::Int => 'i',
            Arg::Uint => 'u',
            Arg::Fixed => 'f',
            Arg::String => 's',
            Arg::Array => 'a',
            Arg::Object => 'o',
            Arg::NewId => 'n',
            Arg::Fd => 'h',
        }
    }
}

/// A complete message signature: opcode + argument types.
#[derive(Clone, Debug)]
pub struct MessageSchema {
    /// The opcode (low 8 bits of the header).
    pub opcode: u32,
    /// The operation name (diagnostics).
    pub name: &'static str,
    /// The argument types in order.
    pub args: &'static [Arg],
}

/// Build the header word for one message.
#[must_use]
pub const fn header(object_id: u32, opcode: u32) -> u32 {
    (object_id << 8) | (opcode & 0xff)
}

/// Decode one `u32` from the stream at `at`.
#[must_use]
fn get_u32(buf: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]])
}

/// One decoded message.
#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    /// The target object id.
    pub object_id: u32,
    /// The opcode.
    pub opcode: u32,
    /// The decoded arguments.
    pub args: Vec<Value>,
}

/// One decoded argument.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// Signed 32-bit.
    Int(i32),
    /// Unsigned 32-bit.
    Uint(u32),
    /// 24.8 fixed point (the raw wire bits).
    Fixed(i32),
    /// A valid UTF-8 string (the NUL is consumed, not stored).
    String(Box<str>),
    /// A byte array.
    Array(Box<[u8]>),
    /// An object reference (0 = null).
    Object(u32),
    /// A new object id.
    NewId(u32),
    /// A file descriptor (its position in the ancillary sequence).
    Fd(u32),
}

/// Why a decode stopped (the `Err` side of [`decode`]; `Ok` means
/// the message decoded completely).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DecodeStatus {
    /// More bytes are needed: the total the message will occupy.
    Incomplete(usize),
    /// A malformed field at the given offset.
    Malformed(usize),
}

/// Encode one message.
///
/// Strings are NUL-terminated and padded; fds contribute nothing to
/// the byte stream but take ancillary slots (the returned count).
/// Argument values must already match the schema (the dispatcher
/// validates before encoding); mismatches encode as zero words. (The
/// flat scalar arms are deliberately separate: the bound types
/// differ.)
#[allow(clippy::match_same_arms)]
#[must_use]
pub fn encode(msg: &Message, schema: &[Arg]) -> (Vec<u8>, u32) {
    let mut out = Vec::with_capacity(8 + schema.len() * 4);
    out.extend_from_slice(&header(msg.object_id, msg.opcode).to_le_bytes());
    let mut fd_slot = 0u32;
    for (i, arg) in schema.iter().enumerate() {
        let v = msg.args.get(i).cloned().unwrap_or(Value::Uint(0));
        match (*arg, v) {
            (Arg::Int, Value::Int(x)) => out.extend_from_slice(&x.to_le_bytes()),
            (Arg::Uint, Value::Uint(x)) => out.extend_from_slice(&x.to_le_bytes()),
            (Arg::Fixed, Value::Fixed(x)) => out.extend_from_slice(&x.to_le_bytes()),
            (Arg::String, Value::String(s)) => {
                // Length counts the NUL terminator.
                let nul_len = s.len() + 1;
                out.extend_from_slice(&(nul_len as u32).to_le_bytes());
                out.extend_from_slice(s.as_bytes());
                out.push(0);
                while out.len() % 4 != 0 {
                    out.push(0);
                }
            }
            (Arg::Array, Value::Array(a)) => {
                out.extend_from_slice(&(a.len() as u32).to_le_bytes());
                out.extend_from_slice(&a);
                while out.len() % 4 != 0 {
                    out.push(0);
                }
            }
            (Arg::Object, Value::Object(id)) | (Arg::NewId, Value::NewId(id)) => {
                out.extend_from_slice(&id.to_le_bytes());
            }
            (Arg::Fd, Value::Fd(_)) => {
                fd_slot += 1;
            }
            // Signature mismatches are the dispatcher's validated
            // contract; encode a zero word to keep the stream aligned.
            _ => out.extend_from_slice(&0u32.to_le_bytes()),
        }
    }
    (out, fd_slot)
}

/// Decode one message from `buf` against `schema`.
///
/// Returns the message, the bytes consumed, and the fd count. Only
/// whole messages decode: a partial frame reports
/// [`DecodeStatus::Incomplete`] with the total size it will need
/// (the caller keeps the tail buffered).
///
/// # Errors
/// [`DecodeStatus`] per the decoder's contract: `Malformed` for a
/// bad object id, an unterminated string, or a non-UTF-8 payload.
pub fn decode(
    buf: &[u8],
    object_id: u32,
    opcode: u32,
    schema: &[Arg],
) -> Result<(Message, usize, u32), DecodeStatus> {
    if buf.len() < 4 {
        return Err(DecodeStatus::Incomplete(4));
    }
    let mut at = 4usize;
    let mut args = Vec::with_capacity(schema.len());
    let mut fds = 0u32;
    for arg in schema {
        match arg {
            Arg::Int => {
                if buf.len() < at + 4 {
                    return Err(DecodeStatus::Incomplete(at + 4));
                }
                args.push(Value::Int(get_u32(buf, at) as i32));
                at += 4;
            }
            Arg::Uint => {
                if buf.len() < at + 4 {
                    return Err(DecodeStatus::Incomplete(at + 4));
                }
                args.push(Value::Uint(get_u32(buf, at)));
                at += 4;
            }
            Arg::Fixed => {
                if buf.len() < at + 4 {
                    return Err(DecodeStatus::Incomplete(at + 4));
                }
                args.push(Value::Fixed(get_u32(buf, at) as i32));
                at += 4;
            }
            Arg::String => {
                if buf.len() < at + 4 {
                    return Err(DecodeStatus::Incomplete(at + 4));
                }
                let len = get_u32(buf, at) as usize;
                if len == 0 {
                    // The length counts the NUL: zero means no
                    // terminator at all.
                    return Err(DecodeStatus::Malformed(at));
                }
                let total = at + 4 + len;
                if buf.len() < total {
                    return Err(DecodeStatus::Incomplete(total));
                }
                let bytes = &buf[at + 4..total];
                if bytes[len - 1] != 0 {
                    return Err(DecodeStatus::Malformed(at));
                }
                let s = std::str::from_utf8(&bytes[..len - 1])
                    .map_err(|_| DecodeStatus::Malformed(at))?;
                args.push(Value::String(s.into()));
                at = total + pad4(len);
            }
            Arg::Array => {
                if buf.len() < at + 4 {
                    return Err(DecodeStatus::Incomplete(at + 4));
                }
                let len = get_u32(buf, at) as usize;
                let total = at + 4 + len;
                if buf.len() < total {
                    return Err(DecodeStatus::Incomplete(total));
                }
                args.push(Value::Array(buf[at + 4..total].into()));
                at = total + pad4(len);
            }
            Arg::Object | Arg::NewId => {
                if buf.len() < at + 4 {
                    return Err(DecodeStatus::Incomplete(at + 4));
                }
                let id = get_u32(buf, at);
                if id > MAX_OBJECT_ID {
                    return Err(DecodeStatus::Malformed(at));
                }
                if *arg == Arg::NewId && id == 0 {
                    return Err(DecodeStatus::Malformed(at));
                }
                args.push(if *arg == Arg::Object {
                    Value::Object(id)
                } else {
                    Value::NewId(id)
                });
                at += 4;
            }
            Arg::Fd => {
                args.push(Value::Fd(fds));
                fds += 1;
            }
        }
    }
    Ok((
        Message {
            object_id,
            opcode,
            args,
        },
        at,
        fds,
    ))
}

/// Padding to the 4-byte boundary after `n` bytes.
#[must_use]
pub const fn pad4(n: usize) -> usize {
    (4 - (n & 3)) & 3
}

#[cfg(test)]
mod tests {
    use super::*;

    const GET_REGISTRY: &[Arg] = &[Arg::Uint, Arg::NewId];

    #[test]
    fn header_packs_id_and_opcode() {
        // wl_display(1).get_registry(opcode 1) → new id 2.
        assert_eq!(header(1, 1), 0x0000_0101);
        let mut buf = Vec::new();
        buf.extend_from_slice(&header(1, 1).to_le_bytes());
        buf.extend_from_slice(&1u32.to_le_bytes()); // version
        buf.extend_from_slice(&2u32.to_le_bytes()); // new_id
        let (msg, used, fds) = decode(&buf, 1, 1, GET_REGISTRY).unwrap();
        assert_eq!((msg.object_id, msg.opcode), (1, 1));
        assert_eq!(msg.args, vec![Value::Uint(1), Value::NewId(2)]);
        assert_eq!((used, fds), (12, 0));
        // Round trip.
        let (back, fd_count) = encode(&msg, GET_REGISTRY);
        assert_eq!(back, buf);
        assert_eq!(fd_count, 0);
    }

    #[test]
    fn strings_are_nul_terminated_and_padded() {
        let schema = &[Arg::String];
        let mut buf = Vec::new();
        buf.extend_from_slice(&header(4, 2).to_le_bytes());
        buf.extend_from_slice(&4u32.to_le_bytes()); // "abc" + NUL
        buf.extend_from_slice(b"abc\0");
        let (msg, used, _) = decode(&buf, 4, 2, schema).unwrap();
        assert_eq!(msg.args, vec![Value::String("abc".into())]);
        assert_eq!(used, 12); // header 4 + len 4 + 4 payload (no pad)
                              // A string needing padding: "abcd" + NUL = 5 → pad to 8.
        let mut buf = Vec::new();
        buf.extend_from_slice(&header(4, 2).to_le_bytes());
        buf.extend_from_slice(&5u32.to_le_bytes());
        buf.extend_from_slice(b"abcd\0\0\0\0");
        let (msg, used, _) = decode(&buf, 4, 2, schema).unwrap();
        assert_eq!(msg.args, vec![Value::String("abcd".into())]);
        assert_eq!(used, 16);
    }

    #[test]
    fn partial_frames_report_the_need() {
        let schema = &[Arg::Uint, Arg::NewId];
        let buf = [0x01, 0x01, 0x00, 0x00, 0x01]; // 5 of 12 bytes
                                                  // The first argument alone still needs 8 bytes in total.
        assert_eq!(decode(&buf, 1, 1, schema), Err(DecodeStatus::Incomplete(8)));
    }

    #[test]
    fn unterminated_and_non_utf8_strings_reject() {
        let schema = &[Arg::String];
        let mut buf = Vec::new();
        buf.extend_from_slice(&header(4, 2).to_le_bytes());
        buf.extend_from_slice(&4u32.to_le_bytes());
        buf.extend_from_slice(b"abcX"); // no NUL
        assert_eq!(decode(&buf, 4, 2, schema), Err(DecodeStatus::Malformed(4)));
        // Non-UTF-8.
        let mut buf = Vec::new();
        buf.extend_from_slice(&header(4, 2).to_le_bytes());
        buf.extend_from_slice(&4u32.to_le_bytes());
        buf.extend_from_slice(&[0xff, 0xfe, 0xfd, 0]);
        assert_eq!(decode(&buf, 4, 2, schema), Err(DecodeStatus::Malformed(4)));
    }

    #[test]
    fn object_ids_above_24_bits_reject() {
        let schema = &[Arg::NewId];
        let mut buf = Vec::new();
        buf.extend_from_slice(&header(1, 0).to_le_bytes());
        buf.extend_from_slice(&0x0100_0000u32.to_le_bytes());
        assert_eq!(decode(&buf, 1, 0, schema), Err(DecodeStatus::Malformed(4)));
    }

    #[test]
    fn fds_take_ancillary_slots_only() {
        // wl_shm.create_pool(fd, size, new_id): the fd contributes
        // zero bytes but one ancillary slot.
        let schema = &[Arg::Fd, Arg::Int, Arg::NewId];
        let msg = Message {
            object_id: 3,
            opcode: 1,
            args: vec![Value::Fd(0), Value::Int(4096), Value::NewId(9)],
        };
        let (bytes, fds) = encode(&msg, schema);
        // The fd contributes zero byte-stream footprint: header + int
        // + new_id = 12 bytes.
        assert_eq!(bytes.len(), 12, "fd rides ancillary, not bytes");
        assert_eq!(fds, 1);
        let (back, used, fd_count) = decode(&bytes, 3, 1, schema).unwrap();
        assert_eq!(used, 12);
        assert_eq!(fd_count, 1);
        assert_eq!(back.args[0], Value::Fd(0));
    }
}
