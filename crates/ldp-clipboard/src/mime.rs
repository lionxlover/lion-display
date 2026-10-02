//! MIME types: validation, canonical form, and offer-list negotiation.
//!
//! The `ldp.data` wire surface carries MIME types as strings in three
//! places: `data_source.offer`, `data_source.target`, and
//! `data_offer.receive`. Every one of them passes through [`Mime`]
//! first — validation happens *before* the string is stored anywhere,
//! so a malformed type can never reach the offer lists (the
//! validation-before-allocation doctrine of `docs/protocol.md` §9).
//!
//! The accepted grammar is the RFC 6838 subset the ecosystem actually
//! uses: `type "/" subtype (";" parameter)*` where type and subtype
//! are restricted tokens (lowercase-normalized) and parameters are
//! `key=value` pairs with the value optionally quoted. Length is
//! capped by [`Limits::string_bytes`] because a MIME string is a
//! protocol string first.
//!
//! Matching follows the clipboard conventions the ecosystem settled
//! on: equality is on the canonical (lowercased, parameter-sorted)
//! form, and `text/plain` parameters are folded so a source offering
//! `text/plain;charset=utf-8` satisfies a receiver asking for
//! `text/plain` and vice versa. Everything else is exact — the
//! server never guesses at `image/*` style wildcards (a client that
//! wants that behavior lists the types it wants; the offer lists are
//! small and enumerable).

#![forbid(unsafe_code)]

use ldp_core::limits::Limits;

/// Maximum parameters on one MIME type (structural guard: parameter
/// lists are not a DoS surface because the whole string is already
/// bounded by `Limits::string_bytes`; this keeps parsing O(1)-ish in
/// the parameter count).
pub const MAX_PARAMS: usize = 8;

/// A validated MIME type.
///
/// Canonical form: `type/subtype` with parameters sorted by key and
/// rendered `key=value` (value quoted only when it contains a
/// non-token character, e.g. `text/plain;charset=utf-8` stays bare
/// but `application/foo;bar="a b"` keeps its quotes).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Mime {
    /// Lowercased type token (`"text"`).
    pub type_: String,
    /// Lowercased subtype token (`"plain"`).
    pub subtype: String,
    /// Parameters, key-lowercased, value preserved; kept sorted by
    /// key so equality is order-insensitive.
    pub params: Vec<(String, String)>,
}

/// Why a MIME string was rejected.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MimeError {
    /// Empty string, or empty type/subtype token.
    EmptyToken,
    /// A token carries a character outside the RFC token set.
    BadChar,
    /// Missing `/` between type and subtype.
    MissingSlash,
    /// More than one `/` (a MIME type has exactly one).
    ExtraSlash,
    /// A parameter is not `key=value`, or a bare `;` with nothing
    /// after it.
    BadParam,
    /// The whole string exceeds `Limits::string_bytes`.
    TooLong,
    /// More than [`MAX_PARAMS`] parameters.
    TooManyParams,
    /// An unterminated quoted parameter value.
    UnterminatedQuote,
}

impl std::fmt::Display for MimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            MimeError::EmptyToken => "empty MIME token",
            MimeError::BadChar => "invalid character in MIME token",
            MimeError::MissingSlash => "MIME type missing '/'",
            MimeError::ExtraSlash => "MIME type has more than one '/'",
            MimeError::BadParam => "malformed MIME parameter",
            MimeError::TooLong => "MIME string exceeds the string limit",
            MimeError::TooManyParams => "too many MIME parameters",
            MimeError::UnterminatedQuote => "unterminated quoted parameter value",
        };
        f.write_str(s)
    }
}

impl std::error::Error for MimeError {}

/// One token character per RFC 6838: alphanumerics and the portable
/// subset of specials. Uppercase is accepted and normalized away.
const fn is_token_char(c: char) -> bool {
    matches!(c,
        'a'..='z' | 'A'..='Z' | '0'..='9'
        | '!' | '#' | '$' | '&' | '\'' | '+' | '-' | '.'
        | '^' | '_' | '`' | '|' | '~')
}

fn is_token(s: &str) -> bool {
    !s.is_empty() && s.chars().all(is_token_char)
}

impl Mime {
    /// Parse and validate a MIME string under `limits`.
    ///
    /// # Errors
    ///
    /// [`MimeError`] describing the first rule the string violates.
    ///
    /// # Panics
    ///
    /// Never (the internal `expect` guards a slash count already
    /// checked immediately above it).
    pub fn parse(s: &str, limits: &Limits) -> Result<Mime, MimeError> {
        if s.len() as u64 > limits.string_bytes as u64 {
            return Err(MimeError::TooLong);
        }
        let (essence, rest) = match s.split_once(';') {
            Some((e, r)) => (e, Some(r)),
            None => (s, None),
        };
        let slashes = essence.matches('/').count();
        if slashes == 0 {
            return Err(MimeError::MissingSlash);
        }
        if slashes > 1 {
            return Err(MimeError::ExtraSlash);
        }
        let (type_, subtype) = essence.split_once('/').expect("count checked above");
        if type_.is_empty() || subtype.is_empty() {
            return Err(MimeError::EmptyToken);
        }
        if !is_token(type_) || !is_token(subtype) {
            return Err(MimeError::BadChar);
        }
        let mut params: Vec<(String, String)> = Vec::new();
        if let Some(rest) = rest {
            for raw in rest.split(';') {
                if raw.is_empty() {
                    return Err(MimeError::BadParam);
                }
                let (k, v) = raw.split_once('=').ok_or(MimeError::BadParam)?;
                if !is_token(k) {
                    return Err(MimeError::BadChar);
                }
                let value = if let Some(stripped) = v.strip_prefix('"') {
                    let inner = stripped
                        .strip_suffix('"')
                        .ok_or(MimeError::UnterminatedQuote)?;
                    if inner.contains('"') {
                        return Err(MimeError::UnterminatedQuote);
                    }
                    inner.to_owned()
                } else {
                    if !is_token(v) {
                        return Err(MimeError::BadChar);
                    }
                    v.to_owned()
                };
                params.push((k.to_ascii_lowercase(), value));
                if params.len() > MAX_PARAMS {
                    return Err(MimeError::TooManyParams);
                }
            }
            params.sort();
            params.dedup();
        }
        Ok(Mime {
            type_: type_.to_ascii_lowercase(),
            subtype: subtype.to_ascii_lowercase(),
            params,
        })
    }

    /// The canonical string (the [`Display`] form): `type/subtype;key=value…`.
    ///
    /// [`Display`]: std::fmt::Display
    #[must_use]
    pub fn canonical(&self) -> String {
        self.to_string()
    }

    /// The essence (`type/subtype`) without parameters.
    #[must_use]
    pub fn essence(&self) -> String {
        format!("{}/{}", self.type_, self.subtype)
    }

    /// Parameter lookup (key already lowercase).
    #[must_use]
    pub fn param(&self, key: &str) -> Option<&str> {
        self.params
            .binary_search_by(|(k, _)| k.as_str().cmp(key))
            .ok()
            .map(|i| self.params[i].1.as_str())
    }

    /// Whether `self` satisfies a request for `want`.
    ///
    /// Exact canonical equality always matches. Additionally, when
    /// both sides are `text/plain`, the charset parameters are folded:
    /// a missing charset means `utf-8` (the LDP default — the wire is
    /// UTF-8 end to end), and the usual `utf8`/`utf-8`/`us-ascii`
    /// aliases compare equal. No other essence ever matches a
    /// different essence, and non-charset parameters still require
    /// exact equality.
    #[must_use]
    pub fn satisfies(&self, want: &Mime) -> bool {
        if self == want {
            return true;
        }
        if self.essence() != "text/plain" || want.essence() != "text/plain" {
            return false;
        }
        if self.param("charset") == want.param("charset") {
            // Same charset (possibly both absent): the remaining
            // parameters must also agree.
            return self
                .params
                .iter()
                .filter(|(k, _)| k != "charset")
                .eq(want.params.iter().filter(|(k, _)| k != "charset"));
        }
        let a = self.param("charset").unwrap_or("utf-8");
        let b = want.param("charset").unwrap_or("utf-8");
        charset_alias(a) == charset_alias(b)
            && self
                .params
                .iter()
                .filter(|(k, _)| k != "charset")
                .eq(want.params.iter().filter(|(k, _)| k != "charset"))
    }

    /// The offered type a receiver should request, given the
    /// receiver's preference order.
    ///
    /// The first preference the offer list satisfies wins (preferences
    /// are most-wanted-first). Returns `None` when nothing matches —
    /// the receiver should then not call `receive` at all (and an
    /// `accept("")` tells the source nothing will be requested).
    #[must_use]
    pub fn negotiate<'a>(offered: &'a [Mime], preferences: &[Mime]) -> Option<&'a Mime> {
        preferences
            .iter()
            .find(|want| offered.iter().any(|o| o.satisfies(want)))
            .and_then(|want| offered.iter().find(|o| o.satisfies(want)))
    }
}

impl std::fmt::Display for Mime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.type_, self.subtype)?;
        for (k, v) in &self.params {
            if is_token(v) {
                write!(f, ";{k}={v}")?;
            } else {
                write!(f, ";{k}=\"{v}\"")?;
            }
        }
        Ok(())
    }
}

/// Fold charset aliases: `utf8` ≡ `utf-8` (case-insensitive), and
/// `us-ascii` ≡ `ansi_x3.4-1968` (the RFC name for the same thing).
fn charset_alias(c: &str) -> &str {
    // The ASCII-compatible family folds together for text/plain
    // negotiation: UTF-8 is a superset of ASCII, so a UTF-8 source
    // serves an ASCII request (and the wire itself is UTF-8 end to
    // end). Other encodings (iso-8859-*, utf-16, ...) stay distinct.
    if c.eq_ignore_ascii_case("utf8")
        || c.eq_ignore_ascii_case("utf-8")
        || c.eq_ignore_ascii_case("us-ascii")
        || c.eq_ignore_ascii_case("ansi_x3.4-1968")
        || c.eq_ignore_ascii_case("ascii")
    {
        "utf-8"
    } else {
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(s: &str) -> Mime {
        Mime::parse(s, &Limits::default()).unwrap()
    }

    #[test]
    fn parses_and_canonicalizes() {
        assert_eq!(
            m("text/plain;charset=utf-8").to_string(),
            "text/plain;charset=utf-8"
        );
        // Parameter order is canonicalized.
        assert_eq!(
            m("text/plain;charset=utf-8;foo=bar").to_string(),
            "text/plain;charset=utf-8;foo=bar"
        );
        // Keys fold to lowercase; values keep their case (RFC:
        // parameter values are case-sensitive in general — charset
        // equality is handled by folding in `satisfies`).
        assert_eq!(
            m("TEXT/Plain;FOO=bar;Charset=UTF-8").to_string(),
            "text/plain;charset=UTF-8;foo=bar"
        );
        assert_eq!(m("image/png").essence(), "image/png");
        assert_eq!(m("image/png").params.len(), 0);
    }

    #[test]
    fn rejects_malformed() {
        let l = Limits::default();
        for bad in [
            "",
            "text",
            "/plain",
            "text/",
            "te xt/plain",
            "text//plain",
            "text/plain;",
            "text/plain;=v",
            "text/plain;k",
            "text/plain;k=\"v",
            "text/plain;k=\"v\"x",
        ] {
            assert!(Mime::parse(bad, &l).is_err(), "{bad:?} parsed");
        }
        let mut long = String::from("text/plain");
        for i in 0..9 {
            long.push_str(";p");
            long.push_str(&i.to_string());
            long.push_str("=v");
        }
        assert_eq!(
            Mime::parse(&long, &l).unwrap_err(),
            MimeError::TooManyParams
        );
        let huge = format!("text/{}x", "a".repeat(5000));
        assert_eq!(Mime::parse(&huge, &l).unwrap_err(), MimeError::TooLong);
    }

    #[test]
    fn quoted_values_round_trip() {
        let v = m("application/x-test;desc=\"hello world\"");
        assert_eq!(v.param("desc"), Some("hello world"));
        assert_eq!(v.to_string(), "application/x-test;desc=\"hello world\"");
    }

    #[test]
    fn text_plain_charset_folding() {
        let offered = [m("text/plain;charset=utf-8")];
        assert!(offered[0].satisfies(&m("text/plain")));
        assert!(m("text/plain").satisfies(&offered[0]));
        assert!(offered[0].satisfies(&m("text/plain;charset=UTF-8")));
        assert!(offered[0].satisfies(&m("text/plain;charset=utf8")));
        assert!(offered[0].satisfies(&m("text/plain;charset=us-ascii")));
        assert!(!offered[0].satisfies(&m("text/plain;charset=iso-8859-1")));
        assert!(!offered[0].satisfies(&m("text/html")));
        assert!(!offered[0].satisfies(&m("text/plain;charset=utf-8;x=1")));
        let aliased = m("text/plain;charset=utf-8");
        assert!(aliased.satisfies(&m("text/plain;charset=utf8")));
    }

    #[test]
    fn negotiation_respects_preference_order() {
        let offered = [
            m("text/plain;charset=utf-8"),
            m("image/png"),
            m("text/html"),
        ];
        let prefs_a = [m("image/png"), m("text/plain")];
        assert_eq!(Mime::negotiate(&offered, &prefs_a), Some(&offered[1]));
        let prefs_b = [m("text/html")];
        assert_eq!(Mime::negotiate(&offered, &prefs_b), Some(&offered[2]));
        let prefs_c = [m("application/json")];
        assert_eq!(Mime::negotiate(&offered, &prefs_c), None);
        // A preference satisfied by folding still returns the offered
        // string (what `receive` must carry).
        assert_eq!(
            Mime::negotiate(&offered, &[m("text/plain")]),
            Some(&offered[0])
        );
    }

    #[test]
    fn duplicate_params_collapse() {
        let v = m("text/plain;charset=utf-8;charset=utf-8");
        assert_eq!(v.params.len(), 1);
    }
}
