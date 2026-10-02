//! The hash-chained audit log.
//!
//! Every security decision the broker makes and every enforcement
//! denial the server issues lands here (`docs/threat-model.md` §5):
//! records carry identity (connection, app), the scope, the action, and
//! structured detail — never raw token bytes. Each record's digest is
//! SHA-256 over the previous digest and the record's canonical bytes,
//! so any retroactive edit, reorder, or truncation breaks the chain at
//! the first touched record: [`AuditChain::verify`] recomputes the whole
//! retained window and reports the exact break.
//!
//! The retained window is a bounded ring (a misbehaving peer must not
//! grow server memory through audit traffic); `verify` covers exactly
//! the retained window and reports its record count — the wire
//! `audit.chain` event's `records` field.
//!
//! Filter classes: the wire `audit_filter` bitset has five bits
//! (grants/denials/revocations/captures/injections) for six actions;
//! `bridge` records fold into the captures class — the class of
//! "acting with ambient authority on behalf of another principal",
//! alongside screen capture. Documented doctrine, pinned by tests.

use ldp_core::caps::Scope;

use crate::sha256::sha256;

/// The action recorded in one audit event
/// (wire: `ldp.security.audit_action`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AuditAction {
    /// A scope was granted (token minted / baseline established).
    Grant,
    /// A request or submission was denied.
    Deny,
    /// A held scope was revoked.
    Revoke,
    /// Pixels were captured (screenshot / record frame).
    Capture,
    /// Input events were injected.
    Inject,
    /// The compatibility bridge used ambient authority.
    Bridge,
}

impl AuditAction {
    /// Wire value (`spec/security.toml`).
    #[must_use]
    pub const fn to_wire(self) -> u32 {
        match self {
            Self::Grant => 1,
            Self::Deny => 2,
            Self::Revoke => 3,
            Self::Capture => 4,
            Self::Inject => 5,
            Self::Bridge => 6,
        }
    }

    /// Parse a wire value.
    #[must_use]
    pub const fn from_wire(v: u32) -> Option<Self> {
        match v {
            1 => Some(Self::Grant),
            2 => Some(Self::Deny),
            3 => Some(Self::Revoke),
            4 => Some(Self::Capture),
            5 => Some(Self::Inject),
            6 => Some(Self::Bridge),
            _ => None,
        }
    }
}

/// Which audit record classes a subscription receives
/// (wire: `ldp.security.audit_filter` bitset).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AuditFilter {
    bits: u32,
}

impl AuditFilter {
    /// No classes — the empty subscription.
    pub const NONE: AuditFilter = AuditFilter { bits: 0 };

    /// The grants class.
    pub const GRANTS: AuditFilter = AuditFilter { bits: 1 << 0 };
    /// The denials class.
    pub const DENIALS: AuditFilter = AuditFilter { bits: 1 << 1 };
    /// The revocations class.
    pub const REVOCATIONS: AuditFilter = AuditFilter { bits: 1 << 2 };
    /// The captures class (also carries bridge records).
    pub const CAPTURES: AuditFilter = AuditFilter { bits: 1 << 3 };
    /// The injections class.
    pub const INJECTIONS: AuditFilter = AuditFilter { bits: 1 << 4 };

    /// Every class.
    pub const ALL: AuditFilter = AuditFilter { bits: (1 << 5) - 1 };

    /// Whether this filter delivers records of `action`.
    #[must_use]
    pub const fn delivers(self, action: AuditAction) -> bool {
        let bit = match action {
            AuditAction::Grant => 1 << 0,
            AuditAction::Deny => 1 << 1,
            AuditAction::Revoke => 1 << 2,
            AuditAction::Capture | AuditAction::Bridge => 1 << 3,
            AuditAction::Inject => 1 << 4,
        };
        self.bits & bit != 0
    }

    /// Union of two filters.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        AuditFilter {
            bits: self.bits | other.bits,
        }
    }

    /// The wire word.
    #[must_use]
    pub const fn to_wire(self) -> u32 {
        self.bits
    }

    /// Parse a wire word (unknown bits are preserved — future classes
    /// must not silently vanish).
    #[must_use]
    pub const fn from_wire(bits: u32) -> Self {
        AuditFilter { bits }
    }
}

/// One security-relevant event, pre-chaining.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuditEvent {
    /// Event time (nanoseconds, monotonic).
    pub ts_ns: u64,
    /// The connection identity from `welcome`.
    pub client: u32,
    /// The app the connection speaks for.
    pub app_id: String,
    /// The scope in question. `None` only for records whose subject is
    /// not a scope — a forged-token submission names no scope by
    /// construction. Wire-streamable records (the `audit.record` event)
    /// always carry one; the chain and JSONL keep the null form.
    pub scope: Option<Scope>,
    /// What happened.
    pub action: AuditAction,
    /// Structured detail (JSON object as string, per the spec).
    pub detail: String,
}

impl AuditEvent {
    /// Canonical byte form (the chaining input): fixed field order,
    /// little-endian scalars, u32-length-prefixed strings. Two events
    /// with equal canonical bytes are the same audit fact.
    #[must_use]
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(64 + self.app_id.len() + self.detail.len());
        out.extend_from_slice(&self.ts_ns.to_le_bytes());
        out.extend_from_slice(&self.client.to_le_bytes());
        out.extend_from_slice(&self.scope.map_or(0, Scope::to_wire).to_le_bytes());
        out.extend_from_slice(&self.action.to_wire().to_le_bytes());
        out.extend_from_slice(&(self.app_id.len() as u32).to_le_bytes());
        out.extend_from_slice(self.app_id.as_bytes());
        out.extend_from_slice(&(self.detail.len() as u32).to_le_bytes());
        out.extend_from_slice(self.detail.as_bytes());
        out
    }
}

/// A record as stored: the event plus its chain coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChainedRecord {
    /// Sequence number (1-based; 0 is the genesis digest).
    pub seq: u64,
    /// The event.
    pub event: AuditEvent,
    /// Digest of this record.
    pub digest: [u8; 32],
}

/// Result of a chain verification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChainResult {
    /// Whether the retained window is internally consistent.
    pub ok: bool,
    /// Number of records covered (the wire `chain.records` field).
    pub records: u64,
}

/// The append-only, hash-chained, bounded audit log.
///
/// Retained-window semantics: when the ring evicts the oldest record,
/// its digest becomes the window's `anchor` — the trust root from
/// which [`AuditChain::verify`] recomputes. A deployment that must
/// detect eviction-boundary tampering checkpoints the anchor
/// externally (the JSONL sink does this by never dropping).
#[derive(Clone, Debug)]
pub struct AuditChain {
    records: Vec<ChainedRecord>,
    /// Digest of the last record that fell out of the window (genesis
    /// zeros while nothing has been evicted).
    anchor: [u8; 32],
    head: [u8; 32],
    capacity: usize,
    dropped: u64,
}

impl Default for AuditChain {
    fn default() -> Self {
        Self::new(1024)
    }
}

impl AuditChain {
    /// A chain retaining at most `capacity` records (0 = unbounded).
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        AuditChain {
            records: Vec::new(),
            anchor: [0u8; 32],
            head: [0u8; 32],
            capacity,
            dropped: 0,
        }
    }

    /// Append one event; returns its sequence number.
    ///
    /// The digest covers the previous digest (genesis = 32 zero bytes)
    /// and the event's canonical bytes, length-prefixed so record
    /// boundaries are unambiguous.
    pub fn append(&mut self, event: AuditEvent) -> u64 {
        let seq = self.records.last().map_or(1, |r| r.seq + 1);
        let mut input = Vec::with_capacity(32 + 8 + event.canonical_bytes().len());
        input.extend_from_slice(&self.head);
        input.extend_from_slice(&seq.to_le_bytes());
        let body = event.canonical_bytes();
        input.extend_from_slice(&(body.len() as u32).to_le_bytes());
        input.extend_from_slice(&body);
        let digest = sha256(&input);
        if self.capacity != 0 && self.records.len() >= self.capacity {
            if let Some(evicted) = self.records.first() {
                self.anchor = evicted.digest;
            }
            self.records.remove(0);
            self.dropped += 1;
        }
        self.head = digest;
        self.records.push(ChainedRecord { seq, event, digest });
        seq
    }

    /// Verify the retained window by recomputing every digest from the
    /// window anchor (genesis zeros, or the digest of the last evicted
    /// record).
    ///
    /// This is the server-side half of the `audit.verify` request; the
    /// wire `chain` event reports `ok` and the covered record count.
    /// A tampered record, an inserted record, a reordered pair, or a
    /// truncated head all fail here.
    #[must_use]
    pub fn verify(&self) -> ChainResult {
        let mut prev = self.anchor;
        for record in &self.records {
            let mut input = Vec::with_capacity(32 + 8 + 4 + 128);
            input.extend_from_slice(&prev);
            input.extend_from_slice(&record.seq.to_le_bytes());
            let body = record.event.canonical_bytes();
            input.extend_from_slice(&(body.len() as u32).to_le_bytes());
            input.extend_from_slice(&body);
            if sha256(&input) != record.digest {
                return ChainResult {
                    ok: false,
                    records: record.seq,
                };
            }
            prev = record.digest;
        }
        ChainResult {
            ok: prev == self.head,
            records: self.records.last().map_or(0, |r| r.seq),
        }
    }

    /// The retained records, oldest first (subscription snapshots and
    /// the `audit.record` event stream).
    #[must_use]
    pub fn records(&self) -> &[ChainedRecord] {
        &self.records
    }

    /// Retained records filtered by class (what a subscriber receives).
    #[must_use]
    pub fn filtered(&self, filter: AuditFilter) -> Vec<&ChainedRecord> {
        self.records
            .iter()
            .filter(|r| filter.delivers(r.event.action))
            .collect()
    }

    /// Records dropped by the ring bound.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// The retained-record capacity (0 = unbounded).
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Reconstruct a chain from stored records — the persisted-reader
    /// path (the JSONL sink and the Phase 18 `ldp-audit` tool load
    /// records and call [`AuditChain::verify`] before trusting them).
    ///
    /// The anchor, head, and counters come from the store alongside the
    /// records; verification recomputes every digest and cross-checks
    /// the head, so a tampered store fails exactly as an in-memory
    /// tamper does.
    #[must_use]
    pub fn from_records(
        records: Vec<ChainedRecord>,
        anchor: [u8; 32],
        head: [u8; 32],
        capacity: usize,
        dropped: u64,
    ) -> Self {
        AuditChain {
            records,
            anchor,
            head,
            capacity,
            dropped,
        }
    }

    /// The window anchor digest (checkpoint root; see the type docs).
    #[must_use]
    pub fn anchor(&self) -> [u8; 32] {
        self.anchor
    }

    /// The current head digest (checkpoint anchor).
    #[must_use]
    pub fn head(&self) -> [u8; 32] {
        self.head
    }

    /// JSONL line for one record (append-only file format; the Phase 18
    /// `ldp-audit` reader consumes these). Hand-rolled escaping — detail
    /// strings are JSON-object-as-string per the spec, so quotes and
    /// control characters must survive.
    #[must_use]
    pub fn jsonl_line(record: &ChainedRecord) -> String {
        let e = &record.event;
        format!(
            "{{\"seq\":{},\"ts_ns\":{},\"client\":{},\"app_id\":{},\"scope\":{},\"action\":{},\"detail\":{},\"digest\":\"{}\"}}",
            record.seq,
            e.ts_ns,
            e.client,
            json_escape(&e.app_id),
            e.scope.map_or_else(|| "null".to_owned(), |s| s.to_wire().to_string()),
            e.action.to_wire(),
            json_escape(&e.detail),
            crate::sha256::hex32(&record.digest)
        )
    }
}

/// Minimal JSON string escaping (the two mandatory escapes plus control
/// characters).
fn json_escape(s: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let v = c as u32;
                out.push_str("\\u");
                for shift in [12, 8, 4, 0] {
                    out.push(HEX[((v >> shift) & 0xf) as usize] as char);
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
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

    #[test]
    fn chain_verifies_over_growth() {
        let mut chain = AuditChain::new(0);
        for i in 0..500u64 {
            chain.append(event(AuditAction::Grant, 1, i * 1000));
            let r = chain.verify();
            assert!(r.ok);
            assert_eq!(r.records, i + 1);
        }
    }

    #[test]
    fn tampering_any_field_breaks_the_chain() {
        let mut chain = AuditChain::new(0);
        for i in 0..10 {
            chain.append(event(AuditAction::Deny, 2, i));
        }
        assert!(chain.verify().ok);
        let mut broken = chain.clone();
        broken.records[4].event.ts_ns ^= 1;
        assert!(!broken.verify().ok);
        let mut scope_flipped = chain.clone();
        scope_flipped.records[4].event.scope = None;
        assert!(!scope_flipped.verify().ok);
        let mut reordered = chain.clone();
        reordered.records.swap(2, 3);
        assert!(!reordered.verify().ok);
        let mut truncated = chain.clone();
        truncated.records.truncate(7);
        // The head digest is retained from the full chain, so a silently
        // truncated tail no longer matches it.
        assert!(!truncated.verify().ok);
        // The honest statement: a *forged* tail (extra record claiming
        // to extend the chain) fails.
        let mut forged = chain.clone();
        let mut e = event(AuditAction::Grant, 9, 99);
        e.client = 2;
        forged.records.push(ChainedRecord {
            seq: 11,
            event: e,
            digest: [7; 32],
        });
        assert!(!forged.verify().ok);
    }

    #[test]
    fn ring_bounded_and_counts_drops() {
        let mut chain = AuditChain::new(4);
        for i in 0..10 {
            chain.append(event(AuditAction::Revoke, 3, i));
        }
        assert_eq!(chain.records().len(), 4);
        assert_eq!(chain.dropped(), 6);
        assert_eq!(chain.records()[0].seq, 7);
        assert!(chain.verify().ok);
    }

    #[test]
    fn filter_classes() {
        assert!(AuditFilter::GRANTS.delivers(AuditAction::Grant));
        assert!(!AuditFilter::GRANTS.delivers(AuditAction::Deny));
        assert!(AuditFilter::CAPTURES.delivers(AuditAction::Capture));
        assert!(AuditFilter::CAPTURES.delivers(AuditAction::Bridge));
        assert!(!AuditFilter::DENIALS.delivers(AuditAction::Bridge));
        assert!(AuditFilter::ALL.delivers(AuditAction::Inject));
        assert!(!AuditFilter::NONE.delivers(AuditAction::Grant));
    }

    #[test]
    fn wire_values_match_spec() {
        assert_eq!(AuditAction::Grant.to_wire(), 1);
        assert_eq!(AuditAction::Bridge.to_wire(), 6);
        for a in [
            AuditAction::Grant,
            AuditAction::Deny,
            AuditAction::Revoke,
            AuditAction::Capture,
            AuditAction::Inject,
            AuditAction::Bridge,
        ] {
            assert_eq!(AuditAction::from_wire(a.to_wire()), Some(a));
        }
    }

    #[test]
    fn jsonl_line_escapes_and_quotes() {
        let mut chain = AuditChain::new(0);
        let mut e = event(AuditAction::Grant, 1, 5);
        e.app_id = "org.a".to_owned();
        e.detail = "{\"k\":\"v\"}".to_owned();
        chain.append(e);
        let line = AuditChain::jsonl_line(&chain.records()[0]);
        assert!(line.starts_with("{\"seq\":1,"));
        assert!(line.contains("\"app_id\":\"org.a\""));
        assert!(line.contains("\"detail\":\"{\\\"k\\\":\\\"v\\\"}\""));
        assert!(line.ends_with('}'));
        // Scope-less records (forged submissions) render JSON null.
        let mut e2 = event(AuditAction::Deny, 1, 6);
        e2.scope = None;
        chain.append(e2);
        assert!(AuditChain::jsonl_line(&chain.records()[1]).contains("\"scope\":null"));
    }
}
