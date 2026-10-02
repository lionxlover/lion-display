//! Tagged argument codec (`docs/protocol.md` §4).
//!
//! Every argument is one **unit**: a tag byte, the value, zero padding to
//! the next multiple of 8. Unit sizes: scalars with 4-byte payloads
//! (int32/uint32/float32/bool/object/new_id/fd/enum) are one word;
//! 8-byte payloads (int64/uint64/float64/ts) two; `rect` and `bitset`
//! three; strings `5 + len + 1` rounded up; arrays a 16-byte header
//! (tag, element tag, count, padding) followed by one full unit per
//! element — so elements stay 8-byte aligned and empty arrays keep
//! their type.
//!
//! Security property (`docs/protocol.md` §9): every length field is
//! bounds-checked against the remaining buffer *before* any allocation
//! proportional to untrusted sizes.

use ldp_core::bitset::Bitset128;
use ldp_core::error::{ErrorCode, LdpError, LimitKind, Result};
use ldp_core::geometry::Rect;
use ldp_core::ids::ObjectId;
use ldp_core::limits::Limits;
use ldp_core::wire::{ArgType, Primitive, Value};

use crate::bytes::{round_up8, Reader, Writer};
use crate::message::ValidationMode;

/// Encode one argument unit, appending to `wr`.
///
/// # Errors
///
/// [`LdpError::Logic`] for values that cannot be represented (null
/// new_id, array element/type disagreement — caller bugs, never wire
/// input); [`LdpError::Limit`] when a string exceeds the limit.
pub(crate) fn encode_arg(wr: &mut Writer, v: &Value, limits: &Limits) -> Result<()> {
    match v {
        Value::Int32(x) => {
            wr.tag(ArgType::Int32);
            wr.u32(*x as u32);
            wr.pad_to8();
        }
        Value::Uint32(x) => {
            wr.tag(ArgType::Uint32);
            wr.u32(*x);
            wr.pad_to8();
        }
        Value::Int64(x) => {
            wr.tag(ArgType::Int64);
            wr.u64(*x as u64);
            wr.pad_to8();
        }
        Value::Uint64(x) => {
            wr.tag(ArgType::Uint64);
            wr.u64(*x);
            wr.pad_to8();
        }
        Value::Float32(x) => {
            wr.tag(ArgType::Float32);
            wr.u32(x.to_bits());
            wr.pad_to8();
        }
        Value::Float64(x) => {
            wr.tag(ArgType::Float64);
            wr.u64(x.to_bits());
            wr.pad_to8();
        }
        Value::Bool(b) => {
            wr.tag(ArgType::Bool);
            wr.u8(u8::from(*b));
            wr.pad_to8();
        }
        Value::String(s) => encode_string(wr, s, limits)?,
        Value::Object(id) => {
            wr.tag(ArgType::Object);
            wr.u32(id.map_or(0, ObjectId::as_u32));
            wr.pad_to8();
        }
        Value::NewId(id) => {
            if id.as_u32() == 0 {
                return Err(LdpError::Logic {
                    what: "new_id may not be the null object ID",
                });
            }
            wr.tag(ArgType::NewId);
            wr.u32(id.as_u32());
            wr.pad_to8();
        }
        Value::Fd(index) => {
            wr.tag(ArgType::Fd);
            wr.u32(*index);
            wr.pad_to8();
        }
        Value::Array { element, items } => encode_array(wr, *element, items, limits)?,
        Value::Rect(r) => {
            wr.tag(ArgType::Rect);
            wr.u32(r.x as u32);
            wr.u32(r.y as u32);
            wr.u32(r.w);
            wr.u32(r.h);
            wr.pad_to8();
        }
        Value::Enum(v) => {
            wr.tag(ArgType::Enum);
            wr.u32(*v);
            wr.pad_to8();
        }
        Value::Bitset(b) => {
            wr.tag(ArgType::Bitset);
            for word in b.to_words() {
                wr.u32(word);
            }
            wr.pad_to8();
        }
        Value::Ts(ns) => {
            wr.tag(ArgType::Ts);
            wr.u64(*ns);
            wr.pad_to8();
        }
        // `Value` is #[non_exhaustive]: future ldp-core variants are a
        // compile-time (not wire) concern — refuse them loudly.
        _ => {
            return Err(LdpError::Logic {
                what: "argument value outside the codec vocabulary",
            });
        }
    }
    Ok(())
}

fn encode_string(wr: &mut Writer, s: &str, limits: &Limits) -> Result<()> {
    let bytes = s.as_bytes();
    if bytes.len() > limits.string_bytes as usize {
        return Err(LdpError::Limit {
            kind: LimitKind::StringBytes,
            value: bytes.len() as u64,
        });
    }
    wr.tag(ArgType::String);
    wr.u32(bytes.len() as u32);
    wr.bytes(bytes);
    wr.u8(0); // NUL terminator
    wr.pad_to8();
    Ok(())
}

fn encode_array(
    wr: &mut Writer,
    element: ArgType,
    items: &[Primitive],
    limits: &Limits,
) -> Result<()> {
    wr.tag(ArgType::Array);
    wr.u32(u32::from(element.to_wire()));
    wr.u32(items.len() as u32);
    wr.pad_to8(); // 9-byte header pads to 16; elements stay aligned
    for item in items {
        if item.arg_type() != element {
            return Err(LdpError::Logic {
                what: "array element type mismatch",
            });
        }
        let pv = primitive_to_value(item)?;
        encode_arg(wr, &pv, limits)?;
    }
    Ok(())
}

fn primitive_to_value(p: &Primitive) -> Result<Value> {
    match *p {
        Primitive::Int32(v) => Ok(Value::Int32(v)),
        Primitive::Uint32(v) => Ok(Value::Uint32(v)),
        Primitive::Int64(v) => Ok(Value::Int64(v)),
        Primitive::Uint64(v) => Ok(Value::Uint64(v)),
        Primitive::Float32(v) => Ok(Value::Float32(v)),
        Primitive::Float64(v) => Ok(Value::Float64(v)),
        Primitive::Bool(v) => Ok(Value::Bool(v)),
        Primitive::Fd(v) => Ok(Value::Fd(v)),
        Primitive::Rect(r) => Ok(Value::Rect(r)),
        // `Primitive` is #[non_exhaustive]: a future variant has no wire
        // encoding here yet — refuse loudly rather than corrupt.
        _ => Err(LdpError::Logic {
            what: "array element outside the codec vocabulary",
        }),
    }
}

/// Decode one argument unit (tag byte + payload). `fd_count` is the
/// header's FD-table size; `fd` indices are checked against it here
/// (structural stage).
pub(crate) fn decode_arg(
    rd: &mut Reader<'_>,
    fd_count: u32,
    limits: &Limits,
    mode: ValidationMode,
) -> Result<Value> {
    let raw = rd.u8()?;
    let Some(ty) = ArgType::from_wire(raw) else {
        return Err(LdpError::malformed(
            ErrorCode::MalformedMessage,
            "unknown argument tag",
        ));
    };
    decode_value(ty, rd, fd_count, limits, mode)
}

/// Decode the payload of one unit whose tag is already known.
fn decode_value(
    ty: ArgType,
    rd: &mut Reader<'_>,
    fd_count: u32,
    limits: &Limits,
    mode: ValidationMode,
) -> Result<Value> {
    match ty {
        ArgType::Int32 => {
            let v = rd.u32()? as i32;
            rd.pad(3, mode)?;
            Ok(Value::Int32(v))
        }
        ArgType::Uint32 => {
            let v = rd.u32()?;
            rd.pad(3, mode)?;
            Ok(Value::Uint32(v))
        }
        ArgType::Int64 => {
            let v = rd.u64()? as i64;
            rd.pad(7, mode)?;
            Ok(Value::Int64(v))
        }
        ArgType::Uint64 => {
            let v = rd.u64()?;
            rd.pad(7, mode)?;
            Ok(Value::Uint64(v))
        }
        ArgType::Float32 => {
            let v = f32::from_bits(rd.u32()?);
            rd.pad(3, mode)?;
            check_finite(v.is_finite(), "float32")?;
            Ok(Value::Float32(v))
        }
        ArgType::Float64 => {
            let v = f64::from_bits(rd.u64()?);
            rd.pad(7, mode)?;
            check_finite(v.is_finite(), "float64")?;
            Ok(Value::Float64(v))
        }
        ArgType::Bool => {
            let b = rd.u8()?;
            rd.pad(6, mode)?;
            if b > 1 {
                return Err(LdpError::malformed(
                    ErrorCode::MalformedMessage,
                    "bool wire byte is neither 0 nor 1",
                ));
            }
            Ok(Value::Bool(b == 1))
        }
        ArgType::String => decode_string(rd, limits, mode),
        ArgType::Object | ArgType::NewId => {
            let raw = rd.u32()?;
            rd.pad(3, mode)?;
            Ok(if ty == ArgType::Object {
                Value::Object(if raw == 0 {
                    None
                } else {
                    Some(ObjectId::from_wire(raw))
                })
            } else {
                Value::NewId(ObjectId::from_wire(raw))
            })
        }
        ArgType::Fd => {
            let index = rd.u32()?;
            rd.pad(3, mode)?;
            if index >= fd_count {
                return Err(LdpError::malformed(
                    ErrorCode::FdMismatch,
                    "fd argument index outside the ancillary table",
                ));
            }
            Ok(Value::Fd(index))
        }
        ArgType::Array => decode_array(rd, fd_count, limits, mode),
        ArgType::Rect => {
            let x = rd.u32()? as i32;
            let y = rd.u32()? as i32;
            let w = rd.u32()?;
            let h = rd.u32()?;
            rd.pad(7, mode)?;
            Ok(Value::Rect(Rect::new(x, y, w, h)))
        }
        ArgType::Enum => {
            let v = rd.u32()?;
            rd.pad(3, mode)?;
            // Unknown values are tolerated here (forward compatibility);
            // strict range checking belongs to signature validation.
            Ok(Value::Enum(v))
        }
        ArgType::Bitset => {
            let mut words = [0u32; 4];
            for word in &mut words {
                *word = rd.u32()?;
            }
            rd.pad(7, mode)?;
            Ok(Value::Bitset(Bitset128::from_words(words)))
        }
        ArgType::Ts => {
            let v = rd.u64()?;
            rd.pad(7, mode)?;
            Ok(Value::Ts(v))
        }
        // `ArgType` is #[non_exhaustive]: unknown tags were already
        // rejected at the tag byte; this arm is unreachable in practice.
        _ => Err(LdpError::Logic {
            what: "argument type outside the codec vocabulary",
        }),
    }
}

fn decode_string(rd: &mut Reader<'_>, limits: &Limits, mode: ValidationMode) -> Result<Value> {
    let len = rd.u32()? as usize;
    if len > limits.string_bytes as usize {
        return Err(LdpError::Limit {
            kind: LimitKind::StringBytes,
            value: len as u64,
        });
    }
    if rd.remaining() < len + 1 {
        return Err(LdpError::malformed(
            ErrorCode::MalformedMessage,
            "string overruns the payload",
        ));
    }
    let bytes = rd.take(len)?;
    let nul = rd.u8()?;
    if nul != 0 {
        return Err(LdpError::malformed(
            ErrorCode::MalformedMessage,
            "string is not NUL-terminated",
        ));
    }
    let s = std::str::from_utf8(bytes)
        .map_err(|_| LdpError::malformed(ErrorCode::InvalidString, "string is not valid UTF-8"))?;
    let unit = 1 + 4 + len + 1;
    rd.pad(round_up8(unit) - unit, mode)?;
    Ok(Value::String(s.into()))
}

fn decode_array(
    rd: &mut Reader<'_>,
    fd_count: u32,
    limits: &Limits,
    mode: ValidationMode,
) -> Result<Value> {
    let raw_tag = rd.u32()?;
    let count = rd.u32()? as usize;
    rd.pad(7, mode)?; // header: 9 bytes -> 16
    let element = u8::try_from(raw_tag).ok().and_then(ArgType::from_wire);
    let Some(element) = element else {
        return Err(LdpError::malformed(
            ErrorCode::MalformedMessage,
            "array carries an unknown element tag",
        ));
    };
    if !element.is_array_element() {
        return Err(LdpError::malformed(
            ErrorCode::MalformedMessage,
            "array element type is not a scalar or rect",
        ));
    }
    // Every element needs at least 8 bytes: bound the count before
    // allocating anything proportional to it.
    if count > rd.remaining() / 8 {
        return Err(LdpError::malformed(
            ErrorCode::MalformedMessage,
            "array count exceeds the remaining payload",
        ));
    }
    if element == ArgType::Rect && count > limits.region_rects as usize {
        return Err(LdpError::Limit {
            kind: LimitKind::RegionRects,
            value: count as u64,
        });
    }
    let mut items = Vec::with_capacity(count.min(rd.remaining() / 8));
    for _ in 0..count {
        let etag = rd.u8()?;
        if etag != element.to_wire() {
            return Err(LdpError::malformed(
                ErrorCode::MalformedMessage,
                "array element tag disagrees with the declared element type",
            ));
        }
        let v = decode_value(element, rd, fd_count, limits, mode)?;
        let p = value_to_primitive(&v);
        items.push(p);
    }
    Ok(Value::Array {
        element,
        items: items.into_boxed_slice(),
    })
}

fn value_to_primitive(v: &Value) -> Primitive {
    match *v {
        Value::Int32(x) => Primitive::Int32(x),
        Value::Uint32(x) => Primitive::Uint32(x),
        Value::Int64(x) => Primitive::Int64(x),
        Value::Uint64(x) => Primitive::Uint64(x),
        Value::Float32(x) => Primitive::Float32(x),
        Value::Float64(x) => Primitive::Float64(x),
        Value::Bool(x) => Primitive::Bool(x),
        Value::Fd(x) => Primitive::Fd(x),
        Value::Rect(x) => Primitive::Rect(x),
        // Unreachable in practice: decode_value is only called with
        // primitive element types (is_array_element guarantees it), and
        // this is an internal invariant, never untrusted input.
        _ => unreachable!("non-primitive array element"),
    }
}

fn check_finite(ok: bool, what: &str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(LdpError::malformed(
            ErrorCode::MalformedMessage,
            format!("non-finite {what} value"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::needless_pass_by_value)] // test helper: consumes to re-encode
    fn rt(v: Value) -> Value {
        let mut wr = Writer::new();
        encode_arg(&mut wr, &v, &Limits::DEFAULT).unwrap();
        let bytes = wr.into_bytes();
        assert_eq!(bytes.len() % 8, 0, "units are 8-byte multiples");
        let mut rd = Reader::new(&bytes);
        let back = decode_arg(&mut rd, 1, &Limits::DEFAULT, ValidationMode::Strict).unwrap();
        assert_eq!(rd.remaining(), 0);
        back
    }

    #[test]
    fn scalars_round_trip_with_exact_sizes() {
        assert_eq!(rt(Value::Int32(-5)), Value::Int32(-5));
        assert_eq!(rt(Value::Uint32(7)), Value::Uint32(7));
        assert_eq!(rt(Value::Int64(-9)), Value::Int64(-9));
        assert_eq!(rt(Value::Uint64(9)), Value::Uint64(9));
        assert_eq!(
            rt(Value::float32(0.5).unwrap()),
            Value::float32(0.5).unwrap()
        );
        assert_eq!(
            rt(Value::float64(0.25).unwrap()),
            Value::float64(0.25).unwrap()
        );
        assert_eq!(rt(Value::Bool(true)), Value::Bool(true));
        assert_eq!(rt(Value::Ts(12345)), Value::Ts(12345));
        assert_eq!(rt(Value::Enum(3)), Value::Enum(3));
        assert_eq!(rt(Value::Fd(0)), Value::Fd(0));
        assert_eq!(rt(Value::Object(None)), Value::Object(None));
        let id = ObjectId::client(0x20).unwrap();
        assert_eq!(
            rt(Value::Object(Some(id))),
            Value::Object(Some(ObjectId::from_wire(0x20)))
        );
        assert_eq!(rt(Value::NewId(id)), Value::NewId(id));
        let r = Rect::new(1, 2, 3, 4);
        assert_eq!(rt(Value::Rect(r)), Value::Rect(r));
        let b = Bitset128::single(0).with(127);
        assert_eq!(rt(Value::Bitset(b)), Value::Bitset(b));
        assert_eq!(rt(Value::String("ldp".into())), Value::String("ldp".into()));
    }

    #[test]
    fn unit_sizes_are_exact() {
        let sizes = |v: &Value| {
            let mut wr = Writer::new();
            encode_arg(&mut wr, v, &Limits::DEFAULT).unwrap();
            wr.into_bytes().len()
        };
        assert_eq!(sizes(&Value::Uint32(1)), 8);
        assert_eq!(sizes(&Value::Int64(1)), 16);
        assert_eq!(sizes(&Value::Ts(1)), 16);
        assert_eq!(sizes(&Value::float64(1.0).unwrap()), 16);
        assert_eq!(sizes(&Value::Rect(Rect::new(0, 0, 1, 1))), 24);
        assert_eq!(sizes(&Value::Bitset(Bitset128::EMPTY)), 24);
        assert_eq!(sizes(&Value::String("hi".into())), 8); // 5+2+1 = 8
        assert_eq!(sizes(&Value::String("hello".into())), 16); // 5+5+1 = 11 -> 16
        assert_eq!(
            sizes(&Value::array(ArgType::Uint32, Vec::<Primitive>::new()).unwrap()),
            16
        );
        let one = Value::array(ArgType::Uint32, vec![Primitive::Uint32(4)]).unwrap();
        assert_eq!(sizes(&one), 24); // 16 header + 8 element
    }

    #[test]
    fn arrays_round_trip_typed_even_when_empty() {
        let empty = Value::array(ArgType::Rect, Vec::<Primitive>::new()).unwrap();
        assert_eq!(rt(empty.clone()), empty);
        let rects = Value::array(
            ArgType::Rect,
            vec![
                Primitive::Rect(Rect::new(0, 0, 10, 10)),
                Primitive::Rect(Rect::new(1, 1, 2, 2)),
            ],
        )
        .unwrap();
        assert_eq!(
            rt(rects),
            Value::array(
                ArgType::Rect,
                vec![
                    Primitive::Rect(Rect::new(0, 0, 10, 10)),
                    Primitive::Rect(Rect::new(1, 1, 2, 2))
                ]
            )
            .unwrap()
        );
        let fds = Value::array(ArgType::Fd, vec![Primitive::Fd(0)]).unwrap();
        assert_eq!(
            rt(fds),
            Value::array(ArgType::Fd, vec![Primitive::Fd(0)]).unwrap()
        );
    }

    #[test]
    fn fd_index_out_of_table_is_fd_mismatch() {
        let mut wr = Writer::new();
        encode_arg(&mut wr, &Value::Fd(3), &Limits::DEFAULT).unwrap();
        let bytes = wr.into_bytes();
        let mut rd = Reader::new(&bytes);
        let err = decode_arg(&mut rd, 3, &Limits::DEFAULT, ValidationMode::Tolerant).unwrap_err();
        assert_eq!(err.wire_code(), Some(ErrorCode::FdMismatch));
        let mut rd = Reader::new(&bytes);
        assert!(decode_arg(&mut rd, 4, &Limits::DEFAULT, ValidationMode::Tolerant).is_ok());
    }

    #[test]
    fn strict_mode_rejects_nonzero_padding() {
        let mut wr = Writer::new();
        encode_arg(&mut wr, &Value::Uint32(7), &Limits::DEFAULT).unwrap();
        let mut bytes = wr.into_bytes();
        bytes[7] = 0xAA; // last pad byte of the unit
        let mut rd = Reader::new(&bytes);
        let err = decode_arg(&mut rd, 0, &Limits::DEFAULT, ValidationMode::Strict).unwrap_err();
        assert_eq!(err.wire_code(), Some(ErrorCode::MalformedMessage));
        // Tolerant mode accepts (the wire contract allows any pad content).
        let mut rd = Reader::new(&bytes);
        assert!(decode_arg(&mut rd, 0, &Limits::DEFAULT, ValidationMode::Tolerant).is_ok());
    }

    #[test]
    fn array_count_bomb_is_rejected_before_allocation() {
        // Header only, count claims u32::MAX: must fail without allocating.
        let mut bytes = vec![ArgType::Array.to_wire()];
        bytes.extend_from_slice(&(u32::from(ArgType::Uint32.to_wire())).to_le_bytes());
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        bytes.resize(16, 0);
        let mut rd = Reader::new(&bytes);
        let err = decode_arg(&mut rd, 0, &Limits::DEFAULT, ValidationMode::Tolerant).unwrap_err();
        assert_eq!(err.wire_code(), Some(ErrorCode::MalformedMessage));
    }
}
