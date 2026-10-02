//! Wire argument model.
//!
//! Mirrors `docs/protocol.md` §4: every message argument is *tagged*, so a
//! receiver can structurally validate a message before consulting the
//! opcode's signature. [`ArgType`] is the tag; [`Value`] is the decoded
//! argument. The codec itself lives in `ldp-protocol` (Phase 2) — this
//! module defines the vocabulary both sides share.
//!
//! Determinism rules enforced here:
//!
//! * floats must be finite ([`Value::Float32`]/[`Value::Float64`]
//!   constructors reject NaN and ±Inf — they never reach the wire),
//! * booleans are exactly 0/1,
//! * strings are UTF-8 and bounded by [`crate::limits::Limits`].

use crate::error::{ErrorCode, LdpError, Result};
use crate::geometry::Rect;

/// Wire type tag of one argument (`docs/protocol.md` §4).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum ArgType {
    /// 32-bit signed integer, sign-extended to the 8-byte slot.
    Int32,
    /// 32-bit unsigned integer.
    Uint32,
    /// 64-bit signed integer.
    Int64,
    /// 64-bit unsigned integer.
    Uint64,
    /// Finite 32-bit float.
    Float32,
    /// Finite 64-bit float.
    Float64,
    /// Boolean, wire bytes exactly 0 or 1.
    Bool,
    /// NUL-terminated UTF-8, bounded length.
    String,
    /// Object reference (0 = null when the argument is nullable).
    Object,
    /// Object ID chosen by the sender (client→server factory requests).
    NewId,
    /// Index into the message's ancillary FD table.
    Fd,
    /// Typed array of scalar/rect elements.
    Array,
    /// `x, y, width, height` block.
    Rect,
    /// Schema-typed enumeration value.
    Enum,
    /// 128-bit capability/flag set.
    Bitset,
    /// Monotonic nanosecond timestamp.
    Ts,
}

impl ArgType {
    /// One-byte wire tag.
    #[must_use]
    pub const fn to_wire(self) -> u8 {
        match self {
            Self::Int32 => 0x01,
            Self::Uint32 => 0x02,
            Self::Int64 => 0x03,
            Self::Uint64 => 0x04,
            Self::Float32 => 0x05,
            Self::Float64 => 0x06,
            Self::Bool => 0x07,
            Self::String => 0x08,
            Self::Object => 0x09,
            Self::NewId => 0x0A,
            Self::Fd => 0x0B,
            Self::Array => 0x0C,
            Self::Rect => 0x0D,
            Self::Enum => 0x0E,
            Self::Bitset => 0x0F,
            Self::Ts => 0x10,
        }
    }

    /// Parse a wire tag; unknown tags are `None` (fatal for the message).
    #[must_use]
    pub const fn from_wire(tag: u8) -> Option<ArgType> {
        match tag {
            0x01 => Some(Self::Int32),
            0x02 => Some(Self::Uint32),
            0x03 => Some(Self::Int64),
            0x04 => Some(Self::Uint64),
            0x05 => Some(Self::Float32),
            0x06 => Some(Self::Float64),
            0x07 => Some(Self::Bool),
            0x08 => Some(Self::String),
            0x09 => Some(Self::Object),
            0x0A => Some(Self::NewId),
            0x0B => Some(Self::Fd),
            0x0C => Some(Self::Array),
            0x0D => Some(Self::Rect),
            0x0E => Some(Self::Enum),
            0x0F => Some(Self::Bitset),
            0x10 => Some(Self::Ts),
            _ => None,
        }
    }

    /// Stable kebab-case name (logs, introspection).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Int32 => "int32",
            Self::Uint32 => "uint32",
            Self::Int64 => "int64",
            Self::Uint64 => "uint64",
            Self::Float32 => "float32",
            Self::Float64 => "float64",
            Self::Bool => "bool",
            Self::String => "string",
            Self::Object => "object",
            Self::NewId => "new_id",
            Self::Fd => "fd",
            Self::Array => "array",
            Self::Rect => "rect",
            Self::Enum => "enum",
            Self::Bitset => "bitset",
            Self::Ts => "ts",
        }
    }

    /// Whether values of this type may appear as array elements: exactly
    /// the types [`Primitive`] can represent (scalars and `rect`).
    ///
    /// Strings, timestamps, objects, enums, bitsets, and nested arrays are
    /// excluded by the v1 wire model (`docs/protocol.md` §4): arrays carry
    /// one explicit element tag, and their elements must be self-describing
    /// primitives.
    #[must_use]
    pub const fn is_array_element(self) -> bool {
        matches!(
            self,
            Self::Int32
                | Self::Uint32
                | Self::Int64
                | Self::Uint64
                | Self::Float32
                | Self::Float64
                | Self::Bool
                | Self::Fd
                | Self::Rect
        )
    }
}

/// A decoded wire argument.
///
/// Variants correspond 1:1 to [`ArgType`]. Schema-typed values
/// ([`Value::Enum`], [`Value::Bitset`]) keep raw numeric payloads here;
/// `ldp-protocol`'s generated enums validate the ranges against the
/// interface signature.
#[derive(Clone, PartialEq, Debug)]
#[non_exhaustive]
pub enum Value {
    /// See [`ArgType::Int32`].
    Int32(i32),
    /// See [`ArgType::Uint32`].
    Uint32(u32),
    /// See [`ArgType::Int64`].
    Int64(i64),
    /// See [`ArgType::Uint64`].
    Uint64(u64),
    /// Finite f32.
    Float32(f32),
    /// Finite f64.
    Float64(f64),
    /// `true`/`false`.
    Bool(bool),
    /// Owned UTF-8 string (no NUL, bounded length).
    String(Box<str>),
    /// Object reference; `None` encodes null.
    Object(Option<crate::ids::ObjectId>),
    /// Client-chosen new object ID.
    NewId(crate::ids::ObjectId),
    /// Index into the ancillary FD table of the same message.
    Fd(u32),
    /// Typed array of primitive values.
    ///
    /// The wire carries an explicit element tag (`docs/protocol.md` §4, tag
    /// `0x0C`) even when the array is empty, so the array's type is fully
    /// determined on the wire — signature validation never has to guess.
    /// [`Value::array`] is the checked constructor.
    Array {
        /// Element type tag declared on the wire.
        element: ArgType,
        /// Elements; each of type `element`.
        items: Box<[Primitive]>,
    },
    /// Geometry block.
    Rect(Rect),
    /// Raw enum payload; validated against the schema later.
    Enum(u32),
    /// 128-bit set (word order defined by the wire encoding, LE words 0..3).
    Bitset(crate::bitset::Bitset128),
    /// Monotonic nanoseconds.
    Ts(u64),
}

/// Primitive (non-nested) array element.
#[derive(Clone, Copy, PartialEq, Debug)]
#[non_exhaustive]
pub enum Primitive {
    /// i32 element.
    Int32(i32),
    /// u32 element.
    Uint32(u32),
    /// i64 element.
    Int64(i64),
    /// u64 element.
    Uint64(u64),
    /// finite f32 element.
    Float32(f32),
    /// finite f64 element.
    Float64(f64),
    /// bool element.
    Bool(bool),
    /// fd-index element.
    Fd(u32),
    /// rect element.
    Rect(Rect),
}

impl Primitive {
    /// The wire tag of this element's type.
    #[must_use]
    pub const fn arg_type(self) -> ArgType {
        match self {
            Self::Int32(_) => ArgType::Int32,
            Self::Uint32(_) => ArgType::Uint32,
            Self::Int64(_) => ArgType::Int64,
            Self::Uint64(_) => ArgType::Uint64,
            Self::Float32(_) => ArgType::Float32,
            Self::Float64(_) => ArgType::Float64,
            Self::Bool(_) => ArgType::Bool,
            Self::Fd(_) => ArgType::Fd,
            Self::Rect(_) => ArgType::Rect,
        }
    }
}

impl Value {
    /// Build a finite f32 value — rejects NaN/±Inf.
    ///
    /// # Errors
    ///
    /// [`LdpError::Malformed`] with [`ErrorCode::MalformedMessage`] when the
    /// value is NaN or infinite (the wire never carries non-finite floats).
    pub fn float32(v: f32) -> Result<Self> {
        if v.is_finite() {
            Ok(Self::Float32(v))
        } else {
            Err(LdpError::malformed(
                ErrorCode::MalformedMessage,
                "non-finite float32 argument",
            ))
        }
    }

    /// Build a finite f64 value — rejects NaN/±Inf.
    ///
    /// # Errors
    ///
    /// [`LdpError::Malformed`] with [`ErrorCode::MalformedMessage`] when the
    /// value is NaN or infinite (the wire never carries non-finite floats).
    pub fn float64(v: f64) -> Result<Self> {
        if v.is_finite() {
            Ok(Self::Float64(v))
        } else {
            Err(LdpError::malformed(
                ErrorCode::MalformedMessage,
                "non-finite float64 argument",
            ))
        }
    }

    /// Build an array value, checking every element against `element`.
    ///
    /// Non-finite float elements are rejected exactly like top-level float
    /// arguments; a mismatched element type is a caller bug, reported as
    /// [`LdpError::Logic`].
    ///
    /// # Errors
    ///
    /// [`LdpError::Malformed`] for non-finite float elements;
    /// [`LdpError::Logic`] when an element's type differs from `element`.
    pub fn array(element: ArgType, items: impl Into<Box<[Primitive]>>) -> Result<Self> {
        let items = items.into();
        for p in items.iter() {
            if p.arg_type() != element {
                return Err(LdpError::Logic {
                    what: "array element type mismatch",
                });
            }
            if let Primitive::Float32(v) = p {
                if !v.is_finite() {
                    return Err(LdpError::malformed(
                        ErrorCode::MalformedMessage,
                        "non-finite float32 array element",
                    ));
                }
            }
            if let Primitive::Float64(v) = p {
                if !v.is_finite() {
                    return Err(LdpError::malformed(
                        ErrorCode::MalformedMessage,
                        "non-finite float64 array element",
                    ));
                }
            }
        }
        Ok(Self::Array { element, items })
    }

    /// The tag of this value's type.
    #[must_use]
    pub fn arg_type(&self) -> ArgType {
        match self {
            Self::Int32(_) => ArgType::Int32,
            Self::Uint32(_) => ArgType::Uint32,
            Self::Int64(_) => ArgType::Int64,
            Self::Uint64(_) => ArgType::Uint64,
            Self::Float32(_) => ArgType::Float32,
            Self::Float64(_) => ArgType::Float64,
            Self::Bool(_) => ArgType::Bool,
            Self::String(_) => ArgType::String,
            Self::Object(_) => ArgType::Object,
            Self::NewId(_) => ArgType::NewId,
            Self::Fd(_) => ArgType::Fd,
            Self::Array { .. } => ArgType::Array,
            Self::Rect(_) => ArgType::Rect,
            Self::Enum(_) => ArgType::Enum,
            Self::Bitset(_) => ArgType::Bitset,
            Self::Ts(_) => ArgType::Ts,
        }
    }

    /// Best-effort human rendering for logs and debuggers (not stable
    /// output; never used in protocol decisions).
    #[must_use]
    pub fn render(&self) -> String {
        match self {
            Self::Int32(v) => format!("int32({v})"),
            Self::Uint32(v) => format!("uint32({v})"),
            Self::Int64(v) => format!("int64({v})"),
            Self::Uint64(v) => format!("uint64({v})"),
            Self::Float32(v) => format!("float32({v})"),
            Self::Float64(v) => format!("float64({v})"),
            Self::Bool(v) => format!("bool({v})"),
            Self::String(s) => format!("string({s:?})"),
            Self::Object(Some(id)) => format!("object({id:?})"),
            Self::Object(None) => "object(null)".to_string(),
            Self::NewId(id) => format!("new_id({id:?})"),
            Self::Fd(i) => format!("fd[{i}]"),
            Self::Array { element, items } => {
                let inner: Vec<String> = items.iter().map(primitive_render).collect();
                format!("array<{}>[{}]", element.as_str(), inner.join(", "))
            }
            Self::Rect(r) => format!("rect({r:?})"),
            Self::Enum(v) => format!("enum#{v}"),
            Self::Bitset(b) => format!("bitset({b:#x})"),
            Self::Ts(ns) => format!("ts({ns})"),
        }
    }
}

fn primitive_render(p: &Primitive) -> String {
    match p {
        Primitive::Int32(v) => format!("{v}"),
        Primitive::Uint32(v) => format!("{v}"),
        Primitive::Int64(v) => format!("{v}"),
        Primitive::Uint64(v) => format!("{v}"),
        Primitive::Float32(v) => format!("{v}"),
        Primitive::Float64(v) => format!("{v}"),
        Primitive::Bool(v) => format!("{v}"),
        Primitive::Fd(i) => format!("fd[{i}]"),
        Primitive::Rect(r) => format!("{r:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn array_element_set_is_primitive_exactly() {
        for tag in 0x01..=0x10u8 {
            let t = ArgType::from_wire(tag).unwrap();
            assert_eq!(
                t.is_array_element(),
                matches!(
                    t,
                    ArgType::Int32
                        | ArgType::Uint32
                        | ArgType::Int64
                        | ArgType::Uint64
                        | ArgType::Float32
                        | ArgType::Float64
                        | ArgType::Bool
                        | ArgType::Fd
                        | ArgType::Rect
                ),
                "element set disagreement for {}",
                t.as_str()
            );
        }
    }

    #[test]
    fn tags_round_trip() {
        for tag in 0x01..=0x10u8 {
            let t = ArgType::from_wire(tag).unwrap_or_else(|| panic!("tag {tag:#x}"));
            assert_eq!(t.to_wire(), tag);
        }
        assert!(ArgType::from_wire(0).is_none());
        assert!(ArgType::from_wire(0x11).is_none());
        assert!(ArgType::from_wire(0xFF).is_none());
    }

    #[test]
    fn nonfinite_floats_rejected() {
        assert!(Value::float32(f32::NAN).is_err());
        assert!(Value::float32(f32::INFINITY).is_err());
        assert!(Value::float32(f32::NEG_INFINITY).is_err());
        assert!(Value::float32(1.5).is_ok());
        assert!(Value::float64(f64::NAN).is_err());
        assert!(Value::float64(f64::INFINITY).is_err());
        assert!(Value::float64(0.25).is_ok());
    }

    #[test]
    fn arg_type_matches_variant() {
        assert_eq!(Value::Int32(-5).arg_type(), ArgType::Int32);
        assert_eq!(Value::Bool(true).arg_type(), ArgType::Bool);
        assert_eq!(Value::Object(None).arg_type(), ArgType::Object);
        assert_eq!(
            Value::array(ArgType::Uint32, vec![Primitive::Uint32(4)])
                .unwrap()
                .arg_type(),
            ArgType::Array
        );
        assert_eq!(Value::Ts(42).arg_type(), ArgType::Ts);
    }

    #[test]
    fn array_constructor_enforces_element_type() {
        // Mismatched element type: caller bug (Logic).
        assert!(matches!(
            Value::array(ArgType::Uint32, vec![Primitive::Int32(1)]),
            Err(LdpError::Logic { .. })
        ));
        // Non-finite float element: wire-invalid (Malformed).
        assert!(Value::array(ArgType::Float32, vec![Primitive::Float32(f32::NAN)]).is_err());
        assert!(Value::array(ArgType::Float64, vec![Primitive::Float64(f64::INFINITY)]).is_err());
        // Matching elements, including an empty array, are fine and keep the
        // declared element tag (needed for exact signature checks).
        let empty = Value::array(ArgType::Rect, Vec::new()).unwrap();
        assert_eq!(
            match &empty {
                Value::Array { element, .. } => Some(*element),
                _ => None,
            },
            Some(ArgType::Rect)
        );
        let rects =
            Value::array(ArgType::Rect, vec![Primitive::Rect(Rect::new(0, 0, 1, 1))]).unwrap();
        assert!(rects.render().contains("array<rect>"));
    }

    #[test]
    fn primitive_arg_type_is_exact() {
        assert_eq!(Primitive::Int32(0).arg_type(), ArgType::Int32);
        assert_eq!(Primitive::Uint32(0).arg_type(), ArgType::Uint32);
        assert_eq!(Primitive::Int64(0).arg_type(), ArgType::Int64);
        assert_eq!(Primitive::Uint64(0).arg_type(), ArgType::Uint64);
        assert_eq!(Primitive::Float32(0.0).arg_type(), ArgType::Float32);
        assert_eq!(Primitive::Float64(0.0).arg_type(), ArgType::Float64);
        assert_eq!(Primitive::Bool(false).arg_type(), ArgType::Bool);
        assert_eq!(Primitive::Fd(0).arg_type(), ArgType::Fd);
        assert_eq!(Primitive::Rect(Rect::EMPTY).arg_type(), ArgType::Rect);
    }

    #[test]
    fn render_is_nonempty_for_all_variants() {
        let vals = [
            Value::Int32(0),
            Value::Uint32(1),
            Value::Int64(-1),
            Value::Uint64(2),
            Value::float32(0.5).unwrap(),
            Value::float64(0.5).unwrap(),
            Value::Bool(false),
            Value::String("hi".into()),
            Value::Object(None),
            Value::NewId(crate::ids::ObjectId::client(7).unwrap()),
            Value::Fd(0),
            Value::array(ArgType::Uint32, Vec::new()).unwrap(),
            Value::Rect(Rect::new(0, 0, 1, 1)),
            Value::Enum(3),
            Value::Bitset(crate::bitset::Bitset128::EMPTY),
            Value::Ts(0),
        ];
        for v in &vals {
            assert!(!v.render().is_empty());
        }
    }
}
