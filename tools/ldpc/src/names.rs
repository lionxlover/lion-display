//! Identifier utilities shared by the generators.
//!
//! Spec names are `snake_case` by rule; code generation turns them into
//! Rust identifiers of three shapes: module names (escaped snake), type
//! names (CamelCase), and const names (UPPER_SNAKE). Everything here is
//! pure string math with exhaustive unit tests — generator correctness
//! depends on these being exact.

/// Strict and reserved Rust keywords that can legally appear via raw
/// identifiers (`r#match` etc.).
const RAW_SAFE_KEYWORDS: &[&str] = &[
    "as", "async", "await", "box", "break", "const", "continue", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "if", "impl", "in", "let", "loop", "macro", "match", "mod", "move",
    "mut", "pub", "ref", "return", "static", "struct", "super", "trait", "true", "type", "unsafe",
    "use", "where", "while", "abstract", "become", "final", "override", "typeof", "unsized",
    "virtual", "yield", "do", "try", "gen",
];

/// Names that can never be raw identifiers and have no faithful Rust spelling.
const UNREPRESENTABLE: &[&str] = &["self", "Self", "super", "crate"];

/// Whether `s` is a Rust keyword (raw-identifier representable or not).
#[must_use]
pub fn is_keyword(s: &str) -> bool {
    RAW_SAFE_KEYWORDS.contains(&s) || UNREPRESENTABLE.contains(&s)
}

/// Whether `s` can never appear as a Rust identifier.
#[must_use]
pub fn is_unrepresentable(s: &str) -> bool {
    UNREPRESENTABLE.contains(&s)
}

/// Escape `s` as a Rust identifier (raw-identifier form for keywords).
///
/// Callers must have rejected [`is_unrepresentable`] names beforehand.
#[must_use]
pub fn escape_ident(s: &str) -> String {
    if is_keyword(s) {
        format!("r#{s}")
    } else {
        s.to_string()
    }
}

/// Whether `s` matches the spec's snake_case name rule: `[a-z_][a-z0-9_]*`.
#[must_use]
pub fn is_snake_name(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Convert a snake_case spec name to UpperCamelCase (`rotate_90` -> `Rotate90`).
#[must_use]
pub fn to_camel(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for part in s.split('_') {
        if part.is_empty() {
            continue;
        }
        let mut cs = part.chars();
        if let Some(first) = cs.next() {
            out.push(first.to_ascii_uppercase());
            out.push_str(cs.as_str());
        }
    }
    out
}

/// Convert a snake_case spec name to UPPER_SNAKE (`frame_target` -> `FRAME_TARGET`).
#[must_use]
pub fn to_upper_snake(s: &str) -> String {
    s.to_ascii_uppercase()
}

/// Escape a string into a Rust string literal body (quotes/backslash/newline).
///
/// Multi-line docs are handled by the doc-comment emitter instead; this is
/// for data fields such as `doc: "..."`.
#[must_use]
pub fn str_lit(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Escape rustdoc-hostile characters in documentation text (`[` starts
/// link syntax, `<` starts HTML tags) so spec prose renders literally.
#[must_use]
pub fn rustdoc_escape(s: &str) -> String {
    s.replace('[', "\\[").replace('<', "\\<")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camel_conversions() {
        assert_eq!(to_camel("rotate_90"), "Rotate90");
        assert_eq!(to_camel("identity"), "Identity");
        assert_eq!(to_camel("frame_drop_reason"), "FrameDropReason");
        assert_eq!(to_camel("a11y"), "A11y");
        assert_eq!(to_camel("keymap_format"), "KeymapFormat");
        assert_eq!(to_camel("_leading"), "Leading");
        assert_eq!(to_camel("double__underscore"), "DoubleUnderscore");
    }

    #[test]
    fn upper_snake_conversions() {
        assert_eq!(to_upper_snake("frame_target"), "FRAME_TARGET");
        assert_eq!(to_upper_snake("hello"), "HELLO");
        assert_eq!(to_upper_snake("sync_done"), "SYNC_DONE");
    }

    #[test]
    fn keyword_handling() {
        assert!(is_keyword("match"));
        assert!(!is_keyword("r#gen"));
        assert!(is_keyword("gen"));
        assert!(is_unrepresentable("crate"));
        assert!(!is_keyword("surface"));
        assert_eq!(escape_ident("match"), "r#match");
        assert_eq!(escape_ident("surface"), "surface");
    }

    #[test]
    fn snake_rule() {
        assert!(is_snake_name("shm_pool"));
        assert!(is_snake_name("a11y"));
        assert!(is_snake_name("_x"));
        assert!(!is_snake_name("Rotate90"));
        assert!(!is_snake_name("2fa"));
        assert!(!is_snake_name(""));
        assert!(!is_snake_name("has-dash"));
        assert!(!is_snake_name("dot.name"));
    }

    #[test]
    fn literal_escaping() {
        assert_eq!(str_lit("plain"), "\"plain\"");
        assert_eq!(str_lit("a\"b"), "\"a\\\"b\"");
        assert_eq!(str_lit("back\\slash"), "\"back\\\\slash\"");
        assert_eq!(str_lit("line\nbreak"), "\"line\\nbreak\"");
    }

    #[test]
    fn rustdoc_escapes_link_and_html_syntax() {
        assert_eq!(
            rustdoc_escape("see [connection.destroy]"),
            "see \\[connection.destroy]"
        );
        assert_eq!(rustdoc_escape("array<rect>"), "array\\<rect>");
        assert_eq!(rustdoc_escape("plain text"), "plain text");
    }
}
