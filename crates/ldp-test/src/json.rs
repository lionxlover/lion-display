//! The minimal JSON writer for machine-readable reports.
//!
//! Hand-rolled for the same reason `ldp-server` hand-rolls its schema
//! JSON: zero dependencies, full control of the output shape, and
//! output that is byte-stable for byte-stable inputs. This is a
//! *writer* only — reports are write-once trees, never re-parsed here.
//!
//! Rules the writer guarantees:
//!
//! * every string is escaped (`"`, `\`, control characters), so any
//!   identifier or message text can be embedded safely,
//! * numbers are integers or finite floats — non-finite floats are a
//!   caller bug and fail loudly,
//! * output is pretty-printed with two-space indentation, stable
//!   field order (declaration order), and a trailing newline.

use std::fmt::Write as _;

/// One JSON value (write side).
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    /// `null`.
    Null,
    /// `true` / `false`.
    Bool(bool),
    /// An integer (durations in ns, counts — exact, no rounding).
    Int(i64),
    /// A finite float.
    Num(f64),
    /// A string (escaped on write).
    Str(String),
    /// An array.
    Arr(Vec<Json>),
    /// An object (field order preserved).
    Obj(Vec<(String, Json)>),
}

impl Json {
    /// A string value.
    #[must_use]
    pub fn str(s: impl Into<String>) -> Json {
        Json::Str(s.into())
    }

    /// An integer value.
    #[must_use]
    pub fn int(i: i64) -> Json {
        Json::Int(i)
    }

    /// A finite float value.
    ///
    /// # Panics
    ///
    /// Panics on non-finite input — a report must never contain `NaN`.
    #[must_use]
    pub fn num(f: f64) -> Json {
        assert!(f.is_finite(), "non-finite float in a report");
        Json::Num(f)
    }

    /// An array value.
    #[must_use]
    pub fn arr(items: Vec<Json>) -> Json {
        Json::Arr(items)
    }

    /// An object value from `(name, value)` pairs.
    #[must_use]
    pub fn obj(fields: Vec<(&str, Json)>) -> Json {
        Json::Obj(fields.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }

    /// Render pretty-printed (two-space indent), with a trailing
    /// newline.
    #[must_use]
    pub fn to_pretty(&self) -> String {
        let mut out = String::new();
        self.write(&mut out, 0);
        out.push('\n');
        out
    }

    fn write(&self, out: &mut String, depth: usize) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(true) => out.push_str("true"),
            Json::Bool(false) => out.push_str("false"),
            Json::Int(i) => {
                let _ = write!(out, "{i}");
            }
            Json::Num(f) => {
                // Shortest representation that round-trips; always
                // carries a decimal point or exponent so the value
                // stays a JSON number.
                let s = f.to_string();
                if s.contains('.') || s.contains('e') || s.contains('E') {
                    out.push_str(&s);
                } else {
                    out.push_str(&s);
                    out.push_str(".0");
                }
            }
            Json::Str(s) => write_escaped(out, s),
            Json::Arr(items) => {
                if items.is_empty() {
                    out.push_str("[]");
                    return;
                }
                out.push_str("[\n");
                for (i, item) in items.iter().enumerate() {
                    indent(out, depth + 1);
                    item.write(out, depth + 1);
                    if i + 1 < items.len() {
                        out.push(',');
                    }
                    out.push('\n');
                }
                indent(out, depth);
                out.push(']');
            }
            Json::Obj(fields) => {
                if fields.is_empty() {
                    out.push_str("{}");
                    return;
                }
                out.push_str("{\n");
                for (i, (k, v)) in fields.iter().enumerate() {
                    indent(out, depth + 1);
                    write_escaped(out, k);
                    out.push_str(": ");
                    v.write(out, depth + 1);
                    if i + 1 < fields.len() {
                        out.push(',');
                    }
                    out.push('\n');
                }
                indent(out, depth);
                out.push('}');
            }
        }
    }
}

fn indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str("  ");
    }
}

fn write_escaped(out: &mut String, s: &str) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalars_render() {
        assert_eq!(Json::Null.to_pretty(), "null\n");
        assert_eq!(Json::Bool(true).to_pretty(), "true\n");
        assert_eq!(Json::int(-5).to_pretty(), "-5\n");
        assert_eq!(Json::num(1.5).to_pretty(), "1.5\n");
        // Integral floats keep their number-ness.
        assert_eq!(Json::num(2.0).to_pretty(), "2.0\n");
    }

    #[test]
    fn strings_escape() {
        let j = Json::str("a\"b\\c\nd\te\u{1}");
        assert_eq!(j.to_pretty(), "\"a\\\"b\\\\c\\nd\\te\\u0001\"\n");
    }

    #[test]
    fn structures_render_indented() {
        let j = Json::obj(vec![
            ("name", Json::str("codec")),
            ("count", Json::int(3)),
            ("nested", Json::obj(vec![("x", Json::int(1))])),
            ("list", Json::arr(vec![Json::int(1), Json::int(2)])),
            ("empty_a", Json::arr(vec![])),
        ]);
        let s = j.to_pretty();
        assert!(s.starts_with("{\n  \"name\": \"codec\",\n"));
        assert!(s.contains("\"nested\": {\n    \"x\": 1\n  }"));
        assert!(s.contains("\"list\": [\n    1,\n    2\n  ]"));
        assert!(s.contains("\"empty_a\": []"));
        assert!(s.ends_with("}\n"));
    }

    #[test]
    #[should_panic(expected = "non-finite")]
    fn non_finite_float_fails_loudly() {
        let _ = Json::num(f64::NAN);
    }

    #[test]
    fn field_order_is_declaration_order() {
        let j = Json::obj(vec![("z", Json::int(1)), ("a", Json::int(2))]);
        assert!(j.to_pretty().contains("\"z\": 1,\n  \"a\": 2"));
    }
}
