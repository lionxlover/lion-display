//! The audit-log file format: the JSONL persistence of the
//! hash-chained audit ring (`docs/threat-model.md` §5).
//!
//! One line per record, exactly the shape
//! [`ldp_security::AuditChain::jsonl_line`] writes:
//!
//! ```json
//! {"seq":1,"ts_ns":42,"client":3,"app_id":"org.example.app",
//!  "scope":4,"action":1,"detail":"{...}","digest":"ab.."}
//! ```
//!
//! [`ChainLog`] is the producer side: it appends to an in-memory
//! [`AuditChain`] and writes each line synchronously (append-only,
//! flush-per-record — an audit log that loses its tail on a crash is
//! not an audit log). [`load`] is the consumer side the `ldp-audit`
//! tool runs: parse every line, rebuild the chain, and let
//! [`AuditChain::verify`] recompute the digests.
//!
//! Honest verification semantics, pinned by tests:
//!
//! * any *retroactive edit*, *reorder*, or *insertion* breaks the
//!   digest chain at the first touched record;
//! * a *truncated or wholly rewritten* tail is internally consistent
//!   by construction (SHA-256 is unkeyed) — detecting it requires an
//!   external checkpoint. [`LoadedLog`] therefore exposes the head
//!   digest for comparison against an operator-held checkpoint
//!   (`ldp-audit --expect-head`), and the loader itself detects the
//!   one case the file *can* see: a chain whose lines do not start at
//!   seq 1 or skip sequence numbers.

#![forbid(unsafe_code)]

use std::io::{BufRead, Write as _};
use std::path::Path;

use ldp_core::caps::Scope;
use ldp_security::chain::{AuditAction, AuditChain, AuditEvent, ChainedRecord};

/// A problem found while producing or consuming an audit log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuditLogError {
    /// The underlying file could not be opened, read, or written.
    Io(String),
    /// A line is not valid JSONL for this format (line numbers are
    /// 1-based; the message names the field).
    Malformed {
        /// The offending line number.
        line: usize,
        /// What was wrong.
        message: String,
    },
    /// Sequence numbers are not contiguous from 1.
    NonContiguous {
        /// The line where the gap or wrong start was noticed.
        line: usize,
        /// The seq found there.
        found: u64,
        /// The seq expected there.
        expected: u64,
    },
}

impl std::fmt::Display for AuditLogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(m) => write!(f, "audit log io: {m}"),
            Self::Malformed { line, message } => write!(f, "line {line}: {message}"),
            Self::NonContiguous {
                line,
                found,
                expected,
            } => write!(
                f,
                "line {line}: seq {found} where {expected} was expected \
                 (records must be contiguous from 1)"
            ),
        }
    }
}

impl std::error::Error for AuditLogError {}

/// The producer side: an unbounded in-memory chain plus the
/// append-only file it persists to, one flushed line per record.
///
/// The broker-side wiring (writing `$XDG_STATE_HOME/ldp/audit.jsonl`
/// synchronously per decision) composes this with the Phase 16
/// session broker; both directions of the format are pinned here and
/// exercised by the `ldp-audit` suites.
pub struct ChainLog {
    chain: AuditChain,
    out: std::fs::File,
}

impl ChainLog {
    /// Create (or truncate) the log at `path`.
    ///
    /// # Errors
    /// [`AuditLogError::Io`] when the file cannot be created.
    pub fn create(path: &Path) -> Result<ChainLog, AuditLogError> {
        let out = std::fs::File::create(path)
            .map_err(|e| AuditLogError::Io(format!("create {}: {e}", path.display())))?;
        Ok(ChainLog {
            chain: AuditChain::new(0),
            out,
        })
    }

    /// Append one event: extend the chain, write and flush the line.
    /// Returns the record's sequence number.
    ///
    /// # Errors
    /// [`AuditLogError::Io`] when the line cannot be written or
    /// flushed — the chain has already advanced, so a failed append
    /// means the file is *behind* the chain and must be treated as
    /// suspect (the caller stops logging).
    pub fn append(&mut self, event: AuditEvent) -> Result<u64, AuditLogError> {
        let seq = self.chain.append(event);
        let line = AuditChain::jsonl_line(
            self.chain
                .records()
                .last()
                .ok_or_else(|| AuditLogError::Io("chain lost its record".to_owned()))?,
        );
        writeln!(self.out, "{line}")
            .and_then(|()| self.out.flush())
            .map_err(|e| AuditLogError::Io(format!("append seq {seq}: {e}")))?;
        Ok(seq)
    }

    /// The producing chain (checkpoint access: `head()`).
    #[must_use]
    pub fn chain(&self) -> &AuditChain {
        &self.chain
    }
}

/// A loaded audit log: the rebuilt chain and the verification verdict.
#[derive(Clone, Debug)]
pub struct LoadedLog {
    /// The chain rebuilt from the file (anchor = genesis; head = the
    /// last line's digest; unbounded capacity).
    pub chain: AuditChain,
    /// Number of records (lines) loaded.
    pub records: usize,
    /// The verification verdict over the rebuilt chain.
    pub verdict: ldp_security::chain::ChainResult,
}

impl LoadedLog {
    /// The head digest as lowercase hex (the checkpoint comparison
    /// `ldp-audit --expect-head` runs).
    #[must_use]
    pub fn head_hex(&self) -> String {
        ldp_security::sha256::hex32(&self.chain.head())
    }
}

/// Parse and verify an audit log file.
///
/// Every line must parse in this exact format; the set of sequence
/// numbers must be exactly `1..=N` (a gap is deleted lines, a repeat
/// is duplicated lines — both structural damage; *reordering* keeps
/// the set intact and is allowed through, because the hash chain
/// catches it: every digest links to its predecessor, so any swap
/// breaks verification). The returned [`LoadedLog`] carries the chain
/// and its [`AuditChain::verify`] verdict (callers decide what a
/// broken verdict means — `ldp-audit` exits nonzero).
///
/// # Errors
/// [`AuditLogError`] on IO trouble, malformed lines, or a
/// non-contiguous sequence-number set.
pub fn load(path: &Path) -> Result<LoadedLog, AuditLogError> {
    let file = std::fs::File::open(path)
        .map_err(|e| AuditLogError::Io(format!("open {}: {e}", path.display())))?;
    let reader = std::io::BufReader::new(file);
    let mut records: Vec<ChainedRecord> = Vec::new();
    for (index, line) in reader.lines().enumerate() {
        let line = line.map_err(|e| AuditLogError::Io(format!("read line {}: {e}", index + 1)))?;
        if line.is_empty() {
            continue;
        }
        let record = parse_line(&line).map_err(|message| AuditLogError::Malformed {
            line: index + 1,
            message,
        })?;
        records.push(record);
    }
    // Structural check: sort (seq, file line) pairs and walk the
    // expected 1..=N — the first divergence names the offending line.
    // A reorder diverges never (the multiset is complete); a deletion
    // or duplication diverges exactly where the damage starts.
    let mut seq_lines: Vec<(u64, usize)> = records
        .iter()
        .enumerate()
        .map(|(i, r)| (r.seq, i + 1))
        .collect();
    seq_lines.sort_unstable_by_key(|(seq, _)| *seq);
    for (position, (seq, line)) in seq_lines.iter().enumerate() {
        let expected = u64::try_from(position + 1).unwrap_or(u64::MAX);
        if *seq != expected {
            return Err(AuditLogError::NonContiguous {
                line: *line,
                found: *seq,
                expected,
            });
        }
    }
    let head = records.last().map_or([0u8; 32], |r| r.digest);
    let chain = AuditChain::from_records(records, [0u8; 32], head, 0, 0);
    let verdict = chain.verify();
    let records = chain.records().len();
    Ok(LoadedLog {
        chain,
        records,
        verdict,
    })
}

/// Parse one JSONL line in the exact write-order of `jsonl_line`.
fn parse_line(line: &str) -> Result<ChainedRecord, String> {
    let mut parser = LineParser::new(line);
    parser.open()?;
    let seq = parser.number("seq")?;
    let ts_ns = parser.number("ts_ns")?;
    let client = parser.number("client")? as u32;
    let app_id = parser.string("app_id")?;
    let scope = parser.scope("scope")?;
    let action = parser.action("action")?;
    let detail = parser.string("detail")?;
    let digest = parser.digest("digest")?;
    parser.end()?;
    Ok(ChainedRecord {
        seq,
        event: AuditEvent {
            ts_ns,
            client,
            app_id,
            scope,
            action,
            detail,
        },
        digest,
    })
}

/// A cursor over one line: `"key":value` pairs in the fixed order.
struct LineParser<'a> {
    rest: &'a str,
}

impl<'a> LineParser<'a> {
    fn new(line: &'a str) -> LineParser<'a> {
        LineParser { rest: line }
    }

    /// Consume the leading `{` of the JSONL object.
    fn open(&mut self) -> Result<(), String> {
        let rest = self.rest.trim_start_matches(' ');
        let Some(rest) = rest.strip_prefix('{') else {
            let found: String = rest.chars().take(12).collect();
            return Err(format!("expected '{{' at start of line, found {found:?}"));
        };
        self.rest = rest;
        Ok(())
    }

    /// Consume `[,"]key":` (the optional comma is the field
    /// separator of the JSONL shape [`AuditChain::jsonl_line`]
    /// writes).
    fn key(&mut self, expected: &str) -> Result<(), String> {
        let rest = self.rest.trim_start_matches(' ');
        let rest = rest
            .strip_prefix(',')
            .unwrap_or(rest)
            .trim_start_matches(' ');
        let Some(rest) = rest.strip_prefix('"') else {
            let found: String = rest.chars().take(12).collect();
            return Err(format!("expected key '\"{expected}\"', found {found:?}"));
        };
        let key_len = rest.find('"').ok_or("unterminated key string")?;
        let key = &rest[..key_len];
        if key != expected {
            return Err(format!("expected key \"{expected}\", found \"{key}\""));
        }
        let rest = &rest[key_len + 1..];
        let Some(rest) = rest.strip_prefix(':') else {
            return Err(format!("no ':' after key \"{expected}\""));
        };
        self.rest = rest;
        Ok(())
    }

    /// Consume the numeric value itself (the caller consumed the key).
    fn digits(&mut self) -> Result<u64, String> {
        let rest = self.rest.trim_start_matches(' ');
        let end = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        if end == 0 {
            let found: String = rest.chars().take(8).collect();
            return Err(format!("expected a number, found {found:?}"));
        }
        let value = rest[..end]
            .parse::<u64>()
            .map_err(|e| format!("number overflows u64: {e}"))?;
        self.rest = &rest[end..];
        Ok(value)
    }

    /// Consume `N` (u64) after `"key":`.
    fn number(&mut self, key: &str) -> Result<u64, String> {
        self.key(key)?;
        self.digits()
            .map_err(|m| format!("field '{key}' is not a number: {m}"))
    }

    /// Consume a JSON string after `"key":`.
    fn string(&mut self, key: &str) -> Result<String, String> {
        self.key(key)?;
        let decoded = parse_json_string(self.rest).map_err(|m| format!("field '{key}': {m}"))?;
        self.rest = decoded.1;
        Ok(decoded.0)
    }

    /// Consume the scope field (number or `null`).
    fn scope(&mut self, key: &str) -> Result<Option<Scope>, String> {
        self.key(key)?;
        let rest = self.rest.trim_start_matches(' ');
        if let Some(rest) = rest.strip_prefix("null") {
            self.rest = rest;
            return Ok(None);
        }
        if rest.starts_with('"') {
            return Err(format!("field '{key}' must be a number or null"));
        }
        let wire = self
            .digits()
            .map_err(|m| format!("field '{key}' is not a number: {m}"))? as u32;
        Scope::from_wire(wire)
            .map(Some)
            .ok_or_else(|| format!("field '{key}': unknown scope wire value {wire}"))
    }

    /// Consume the action field and map it to [`AuditAction`].
    fn action(&mut self, key: &str) -> Result<AuditAction, String> {
        let wire = self.number(key)? as u32;
        AuditAction::from_wire(wire)
            .ok_or_else(|| format!("field '{key}': unknown action wire value {wire}"))
    }

    /// Consume `"key":"<64 hex chars>"` into a digest.
    fn digest(&mut self, key: &str) -> Result<[u8; 32], String> {
        let hex = self.string(key)?;
        parse_hex32(&hex).ok_or_else(|| {
            format!(
                "field '{key}': expected 64 hex characters, found {} chars",
                hex.len()
            )
        })
    }

    /// Consume the closing `}` and any trailing whitespace.
    fn end(&mut self) -> Result<(), String> {
        let rest = self.rest.trim_start_matches(' ');
        let Some(rest) = rest.strip_prefix('}') else {
            let found: String = rest.chars().take(12).collect();
            return Err(format!("expected '}}' at end of line, found {found:?}"));
        };
        self.rest = rest;
        if self.rest.trim_start_matches(' ').is_empty() {
            return Ok(());
        }
        Err(format!("trailing garbage {:?}", self.rest))
    }
}

/// Parse a JSON string literal at the front of `s`; returns the
/// decoded string and the rest after the closing quote.
fn parse_json_string(s: &str) -> Result<(String, &str), String> {
    let Some(rest) = s.strip_prefix('"') else {
        return Err(format!("expected '\"', found {:?}", &s[..s.len().min(8)]));
    };
    let mut out = String::new();
    let mut chars = rest.char_indices();
    while let Some((index, c)) = chars.next() {
        match c {
            '"' => return Ok((out, &rest[index + 1..])),
            '\\' => {
                let (_, esc) = chars
                    .next()
                    .ok_or_else(|| "escape at end of string".to_owned())?;
                match esc {
                    '"' => out.push('"'),
                    '\\' => out.push('\\'),
                    '/' => out.push('/'),
                    'n' => out.push('\n'),
                    'r' => out.push('\r'),
                    't' => out.push('\t'),
                    'b' => out.push('\u{8}'),
                    'f' => out.push('\u{c}'),
                    'u' => {
                        let mut hex = String::with_capacity(4);
                        for _ in 0..4 {
                            let (_, digit) = chars
                                .next()
                                .ok_or_else(|| "\\u escape needs 4 digits".to_owned())?;
                            hex.push(digit);
                        }
                        let code = u32::from_str_radix(&hex, 16)
                            .map_err(|e| format!("bad \\u escape '{hex}': {e}"))?;
                        let ch = char::from_u32(code)
                            .ok_or_else(|| format!("\\u{code:04x} is not a character"))?;
                        out.push(ch);
                    }
                    other => return Err(format!("unknown escape '\\{other}'")),
                }
            }
            c if (c as u32) < 0x20 => {
                return Err(format!("raw control character U+{:04X}", c as u32));
            }
            c => out.push(c),
        }
    }
    Err("unterminated string".to_owned())
}

/// Parse 64 lowercase-or-uppercase hex characters into a digest.
fn parse_hex32(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 {
        return None;
    }
    let bytes = s.as_bytes();
    let mut digest = [0u8; 32];
    for i in 0..32 {
        let hi = hex_nibble(bytes[2 * i])?;
        let lo = hex_nibble(bytes[2 * i + 1])?;
        digest[i] = (hi << 4) | lo;
    }
    Some(digest)
}

/// One hex digit to its value.
fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(action: AuditAction, client: u32, ts: u64) -> AuditEvent {
        AuditEvent {
            ts_ns: ts,
            client,
            app_id: "org.example.app".to_owned(),
            scope: Some(Scope::Screenshot),
            action,
            detail: "{\"prompt\":\"granted\"}".to_owned(),
        }
    }

    fn temp_path(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir();
        dir.join(format!(
            "ldp-tools-audit-{tag}-{}-{}",
            std::process::id(),
            line!()
        ))
    }

    #[test]
    fn write_then_load_verifies() {
        let path = temp_path("roundtrip");
        let mut log = ChainLog::create(&path).unwrap();
        for i in 0..25u64 {
            log.append(event(AuditAction::Grant, 1, i * 1000)).unwrap();
        }
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.records, 25);
        assert!(loaded.verdict.ok);
        assert_eq!(loaded.verdict.records, 25);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn field_edit_breaks_verification() {
        let path = temp_path("tamper-field");
        let mut log = ChainLog::create(&path).unwrap();
        for i in 0..10 {
            log.append(event(AuditAction::Deny, 2, i)).unwrap();
        }
        let text = std::fs::read_to_string(&path).unwrap();
        let tampered = text.replace("\"client\":2", "\"client\":7");
        std::fs::write(&path, tampered).unwrap();
        let loaded = load(&path).unwrap();
        assert!(!loaded.verdict.ok);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn deleted_middle_line_is_non_contiguous() {
        let path = temp_path("tamper-delete");
        let mut log = ChainLog::create(&path).unwrap();
        for i in 0..10 {
            log.append(event(AuditAction::Revoke, 3, i)).unwrap();
        }
        let text = std::fs::read_to_string(&path).unwrap();
        let mut lines: Vec<&str> = text.lines().collect();
        lines.remove(4);
        std::fs::write(&path, lines.join("\n")).unwrap();
        let err = load(&path).unwrap_err();
        assert!(matches!(err, AuditLogError::NonContiguous { .. }));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn reordered_lines_break_verification() {
        let path = temp_path("tamper-swap");
        let mut log = ChainLog::create(&path).unwrap();
        for i in 0..10 {
            log.append(event(AuditAction::Capture, 4, i)).unwrap();
        }
        let text = std::fs::read_to_string(&path).unwrap();
        let mut lines: Vec<&str> = text.lines().collect();
        lines.swap(2, 3);
        std::fs::write(&path, lines.join("\n")).unwrap();
        let loaded = load(&path).unwrap();
        assert!(!loaded.verdict.ok);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn truncated_tail_needs_a_checkpoint() {
        let path = temp_path("tamper-truncate");
        let mut log = ChainLog::create(&path).unwrap();
        for i in 0..10 {
            log.append(event(AuditAction::Inject, 5, i)).unwrap();
        }
        let full_head = log.chain().head();
        let head_hex = ldp_security::sha256::hex32(&full_head);
        let text = std::fs::read_to_string(&path).unwrap();
        let truncated: String = text.lines().take(6).fold(String::new(), |mut acc, l| {
            acc.push_str(l);
            acc.push('\n');
            acc
        });
        std::fs::write(&path, truncated).unwrap();
        // Internally consistent — the documented blind spot…
        let loaded = load(&path).unwrap();
        assert!(loaded.verdict.ok);
        // …which the operator-held checkpoint catches.
        assert_ne!(loaded.head_hex(), head_hex);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn malformed_line_is_reported_with_its_number() {
        let path = temp_path("malformed");
        let mut log = ChainLog::create(&path).unwrap();
        log.append(event(AuditAction::Grant, 1, 1)).unwrap();
        log.append(event(AuditAction::Grant, 1, 2)).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        // Break something unique to line 2 (its seq); "action":1 sits
        // on both lines and would misattribute the damage to line 1.
        let broken = text.replace("\"seq\":2", "\"seq\":x");
        std::fs::write(&path, broken).unwrap();
        let err = load(&path).unwrap_err();
        assert!(matches!(err, AuditLogError::Malformed { line: 2, .. }));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn escaping_survives_the_round_trip() {
        let path = temp_path("escape");
        let mut log = ChainLog::create(&path).unwrap();
        let mut e = event(AuditAction::Bridge, 9, 7);
        e.app_id = "org.a\"b\\c".to_owned();
        e.detail = "{\"k\":\"line\nbreaked\"}".to_owned();
        e.scope = None;
        log.append(e).unwrap();
        let loaded = load(&path).unwrap();
        assert!(loaded.verdict.ok);
        let record = &loaded.chain.records()[0];
        assert_eq!(record.event.app_id, "org.a\"b\\c");
        assert_eq!(record.event.detail, "{\"k\":\"line\nbreaked\"}");
        assert!(record.event.scope.is_none());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn empty_file_loads_as_an_empty_chain() {
        let path = temp_path("empty");
        let mut log = ChainLog::create(&path).unwrap();
        log.append(event(AuditAction::Grant, 1, 1)).unwrap();
        drop(log);
        std::fs::write(&path, "").unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.records, 0);
        assert!(loaded.verdict.ok);
        assert_eq!(loaded.head_hex(), "0".repeat(64));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn unknown_scope_and_action_are_rejected() {
        assert!(parse_line("{\"seq\":1,\"ts_ns\":1,\"client\":1,\"app_id\":\"a\",\"scope\":99,\"action\":1,\"detail\":\"x\",\"digest\":\"0\"}").is_err());
        assert!(parse_line("{\"seq\":1,\"ts_ns\":1,\"client\":1,\"app_id\":\"a\",\"scope\":1,\"action\":9,\"detail\":\"x\",\"digest\":\"0\"}").is_err());
    }

    #[test]
    fn hex_round_trips() {
        let digest: [u8; 32] = core::array::from_fn(|i| (i * 7) as u8);
        let hex = ldp_security::sha256::hex32(&digest);
        assert_eq!(parse_hex32(&hex), Some(digest));
        assert_eq!(parse_hex32("zz"), None);
        assert_eq!(parse_hex32(&"a".repeat(63)), None);
    }
}
