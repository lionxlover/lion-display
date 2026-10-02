//! Compact, stable text rendering of wire values and event metadata.
//!
//! The tracer (`ldp-debug trace`) prints one line per dispatched event;
//! `ldp-info` and `ldp-profiler` decode arguments by name into the same
//! vocabulary. The formatting is deliberately unambiguous — quoted
//! strings, tagged objects (`obj#12`), tagged descriptors (`fd[0]`),
//! bitsets as four little-endian hex words — so a printed event can be
//! eyeballed against the spec tables without guessing types.

#![forbid(unsafe_code)]

use std::fmt::Write as _;

use ldp_client::class::EventClass;
use ldp_client::queue::Event;
use ldp_core::wire::{Primitive, Value};

/// Render one wire value.
///
/// Unknown kinds (the wire enum is `#[non_exhaustive]`) render as `?`
/// — the tracer never invents values it cannot decode.
#[must_use]
pub fn format_value(value: &Value) -> String {
    match value {
        Value::Int32(v) => v.to_string(),
        Value::Uint32(v) => v.to_string(),
        Value::Int64(v) => v.to_string(),
        Value::Uint64(v) => v.to_string(),
        Value::Float32(v) => format!("{v:.4}"),
        Value::Float64(v) => format!("{v:.4}"),
        Value::Bool(v) => v.to_string(),
        Value::String(s) => quote(s),
        Value::Object(Some(id)) => format!("obj#{}", id.as_u32()),
        Value::Object(None) => "null".to_owned(),
        Value::NewId(id) => format!("new#{}", id.as_u32()),
        Value::Fd(i) => format!("fd[{i}]"),
        Value::Array { items, .. } => {
            let mut out = String::from("[");
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(&format_primitive(item));
            }
            out.push(']');
            out
        }
        Value::Rect(r) => format!("rect({r})"),
        Value::Enum(v) => format!("enum:{v}"),
        Value::Bitset(b) => format_bitset(*b),
        Value::Ts(v) => format!("ts:{v}ns"),
        _ => "?".to_owned(),
    }
}

/// Render one array element.
///
/// Unknown element kinds (the wire enum is `#[non_exhaustive]`)
/// render as `?` — the tracer never invents values it cannot decode.
#[must_use]
pub fn format_primitive(p: &Primitive) -> String {
    match p {
        Primitive::Int32(v) => v.to_string(),
        Primitive::Uint32(v) => v.to_string(),
        Primitive::Int64(v) => v.to_string(),
        Primitive::Uint64(v) => v.to_string(),
        Primitive::Float32(v) => format!("{v:.4}"),
        Primitive::Float64(v) => format!("{v:.4}"),
        Primitive::Bool(v) => v.to_string(),
        Primitive::Fd(i) => format!("fd[{i}]"),
        Primitive::Rect(r) => format!("rect({r})"),
        _ => "?".to_owned(),
    }
}

/// A 128-bit bitset as `bits:<w0>.<w1>.<w2>.<w3>` (hex words, wire
/// order); empty sets render as `bits:empty`.
#[must_use]
pub fn format_bitset(b: ldp_core::bitset::Bitset128) -> String {
    if b.is_empty() {
        return "bits:empty".to_owned();
    }
    let words = b.to_words();
    format!(
        "bits:{:08x}.{:08x}.{:08x}.{:08x}",
        words[0], words[1], words[2], words[3]
    )
}

/// A quoted, escaped string literal.
#[must_use]
pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The dispatch-lane name of an event class.
#[must_use]
pub fn class_name(class: EventClass) -> &'static str {
    match class {
        EventClass::Input => "input",
        EventClass::Data => "data",
        EventClass::Control => "control",
        EventClass::Configuration => "configuration",
        EventClass::Presentation => "presentation",
    }
}

/// One tracer line: `[seq N] iface.op(args) [class]` plus a fence
/// marker when the event carried a signalled release fence.
///
/// # Errors
/// Never: formatting into a `String` cannot fail; the `fmt::Write`
/// plumbing is honored for clippy's benefit.
pub fn format_event_line(event: &Event, fence: Option<bool>) -> String {
    let mut out = String::with_capacity(96);
    let _ = write!(
        out,
        "[seq {}] {}.{}(",
        event.seq, event.interface, event.op.name
    );
    for (i, (schema, value)) in event.op.args.iter().zip(event.args.iter()).enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        let _ = write!(out, "{}={}", schema.name, format_value(value));
    }
    out.push_str(") [");
    out.push_str(class_name(event.class));
    if event.urgent {
        out.push_str(",urgent");
    }
    out.push(']');
    if let Some(signalled) = fence {
        out.push_str(if signalled {
            " fence:signalled"
        } else {
            " fence:pending"
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalars_render_plain() {
        assert_eq!(format_value(&Value::Int32(-5)), "-5");
        assert_eq!(format_value(&Value::Uint64(1 << 40)), "1099511627776");
        assert_eq!(format_value(&Value::Bool(true)), "true");
        assert_eq!(format_value(&Value::Ts(16_666_667)), "ts:16666667ns");
    }

    #[test]
    fn strings_quote_and_escape() {
        assert_eq!(
            format_value(&Value::String("a\"b\n".into())),
            "\"a\\\"b\\n\""
        );
    }

    #[test]
    fn bitsets_render_words() {
        let b = ldp_core::bitset::Bitset128::single(0).with(33);
        let text = format_bitset(b);
        assert!(text.starts_with("bits:00000001."));
        assert!(text.contains(".00000002."));
        assert_eq!(
            format_bitset(ldp_core::bitset::Bitset128::EMPTY),
            "bits:empty"
        );
    }

    #[test]
    fn arrays_render_elements() {
        let arr = Value::array(
            ldp_core::wire::ArgType::Uint32,
            vec![Primitive::Uint32(1), Primitive::Uint32(2)],
        )
        .unwrap();
        assert_eq!(format_value(&arr), "[1, 2]");
    }

    #[test]
    fn rect_renders_display_form() {
        let r = ldp_core::geometry::Rect::new(3, 4, 100, 200);
        assert_eq!(format_value(&Value::Rect(r)), format!("rect({r})"));
    }

    #[test]
    fn class_names_are_stable() {
        assert_eq!(class_name(EventClass::Input), "input");
        assert_eq!(class_name(EventClass::Presentation), "presentation");
    }
}
