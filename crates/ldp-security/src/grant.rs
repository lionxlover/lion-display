//! The broker's grant table — token minting, submission, revocation.
//!
//! Tokens are 256-bit random values carrying (scope set, expiry). The
//! table is the single source of truth: submissions walk it and compare
//! every entry in constant time ([`AccessToken::constant_time_eq`]), so
//! the submission path has no byte-position-dependent early exit.
//!
//! Rules encoded here (the Phase 16 "forgery/replay fails" exit
//! criterion):
//!
//! * **Forgery** — a token whose bytes match no table entry denies with
//!   [`DeniedReason::InvalidToken`]. With 2^256 space and per-app
//!   64-entry tables, guessing is not an attack.
//! * **Cross-app replay** — a token minted for app A submitted under
//!   app B's identity denies with `InvalidToken` (the app binding is
//!   part of the entry, checked after the bytewise match).
//! * **Revoked resubmission** — denies with `InvalidToken`; the token
//!   is dead even though its bytes still match.
//! * **Expired** — denies with `Expired` at the exact boundary
//!   (`now > expires` denies; `now == expires` is still valid).
//! * **Idempotent resubmission** — re-submitting a live token on the
//!   same connection succeeds and changes nothing (the spec tells
//!   clients to keep tokens for re-submission after reconnect; a
//!   different connection of the *same app* may submit it too — that is
//!   the reconnect case, not a replay).
//!
//! Minting draws entropy from the [`TokenSeed`] seam: deployments plug
//! OS randomness; tests plug [`LcgSeed`] for deterministic corpora.

use std::collections::BTreeSet;

use ldp_core::caps::{Scope, ScopeSet};
use ldp_core::time::Mono;
use ldp_core::token::AccessToken;

/// Why an escalation request or token submission was denied
/// (wire: `ldp.security.denied_reason`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DeniedReason {
    /// The app's manifest does not list the scope.
    ManifestMissing,
    /// The user answered the escalation prompt with no.
    UserDenied,
    /// The token's expiry has passed.
    Expired,
    /// The token does not match any live grant (forgery, cross-app
    /// replay, or revoked).
    InvalidToken,
    /// Too many escalation requests in the window.
    RateLimited,
}

impl DeniedReason {
    /// Wire value (`spec/security.toml`).
    #[must_use]
    pub const fn to_wire(self) -> u32 {
        match self {
            Self::ManifestMissing => 1,
            Self::UserDenied => 2,
            Self::Expired => 3,
            Self::InvalidToken => 4,
            Self::RateLimited => 5,
        }
    }

    /// Parse a wire value.
    #[must_use]
    pub const fn from_wire(v: u32) -> Option<Self> {
        match v {
            1 => Some(Self::ManifestMissing),
            2 => Some(Self::UserDenied),
            3 => Some(Self::Expired),
            4 => Some(Self::InvalidToken),
            5 => Some(Self::RateLimited),
            _ => None,
        }
    }
}

/// Outcome of submitting a token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubmitOutcome {
    /// The token is live; these scopes join the connection's effective set.
    Granted(ScopeSet),
    /// The submission was denied for this reason. `scope` is the
    /// matched grant's scope when a bytewise match existed (expired,
    /// revoked, cross-app), `None` for outright forgery — the audit
    /// trail's scope column for a forgery is empty by construction.
    Denied {
        /// Wire-stable denial reason.
        reason: DeniedReason,
        /// The matched grant's scope, when one matched.
        scope: Option<Scope>,
    },
}

/// Entropy source for token minting. The deployment host plugs OS
/// randomness here; tests plug a deterministic generator.
pub trait TokenSeed {
    /// Fill the 8 words of a fresh token.
    fn fill_words(&mut self, out: &mut [u32; 8]);
}

/// Deterministic splitmix64 generator for tests and golden corpora.
///
/// Splitmix64 has full-period 2^64, avalanche output, and is tiny —
/// good enough to model a strong source for corpora that must be
/// reproducible byte-for-byte across machines.
#[derive(Clone, Debug)]
pub struct LcgSeed {
    state: u64,
}

impl LcgSeed {
    /// A generator from an explicit seed (tests pin exact values).
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        LcgSeed { state: seed }
    }

    /// Next raw 64-bit value.
    #[must_use]
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

impl TokenSeed for LcgSeed {
    fn fill_words(&mut self, out: &mut [u32; 8]) {
        for word in out.iter_mut() {
            *word = self.next_u64() as u32;
        }
    }
}

/// One minted grant.
#[derive(Clone, Debug)]
struct GrantEntry {
    token: AccessToken,
    app_id: String,
    revoked: bool,
    /// Connections that have submitted this token (idempotence +
    /// reconnect bookkeeping).
    submitted_by: BTreeSet<u32>,
}

/// The broker-side table of live grants.
#[derive(Debug, Default)]
pub struct GrantTable {
    entries: Vec<GrantEntry>,
}

impl GrantTable {
    /// An empty table.
    #[must_use]
    pub const fn new() -> Self {
        GrantTable {
            entries: Vec::new(),
        }
    }

    /// Entries currently in the table (live, expired, or revoked).
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the table holds no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Mint a fresh token and record the grant.
    ///
    /// `expires` `None` = session-bound (valid until the broker's
    /// session ends, not the clock).
    #[must_use]
    pub fn mint(
        &mut self,
        seed: &mut dyn TokenSeed,
        app_id: &str,
        scope: ScopeSet,
        expires: Option<Mono>,
    ) -> [u32; 8] {
        let mut words = [0u32; 8];
        seed.fill_words(&mut words);
        // Collision against any live entry: re-draw (256-bit space makes
        // this unreachable in practice; the loop is correctness armor).
        while self.entries.iter().any(|e| e.token.to_words() == words) {
            seed.fill_words(&mut words);
        }
        let token = AccessToken::from_words(words, scope, expires);
        self.entries.push(GrantEntry {
            token,
            app_id: app_id.to_owned(),
            revoked: false,
            submitted_by: BTreeSet::new(),
        });
        words
    }

    /// Validate a submitted token for `(app_id, connection)` at `now`.
    ///
    /// The walk touches every entry with a constant-time byte compare
    /// (no early exit on match or mismatch position), so submission cost
    /// does not leak which entry matched.
    pub fn submit(
        &mut self,
        app_id: &str,
        connection: u32,
        words: [u32; 8],
        now: Mono,
    ) -> SubmitOutcome {
        let probe = AccessToken::from_words(words, ScopeSet::NONE, None);
        let mut found: Option<usize> = None;
        for (i, entry) in self.entries.iter().enumerate() {
            if ct_bytes_eq(&entry.token, &probe) {
                found = Some(i);
                // No break: the walk is uniform in table size by design.
            }
        }
        let Some(i) = found else {
            return SubmitOutcome::Denied {
                reason: DeniedReason::InvalidToken,
                scope: None,
            };
        };
        let entry = &mut self.entries[i];
        if entry.app_id != app_id {
            return SubmitOutcome::Denied {
                reason: DeniedReason::InvalidToken,
                scope: entry.token.scope().iter().next(),
            };
        }
        if entry.revoked {
            return SubmitOutcome::Denied {
                reason: DeniedReason::InvalidToken,
                scope: entry.token.scope().iter().next(),
            };
        }
        if !entry.token.is_valid_at(now) {
            return SubmitOutcome::Denied {
                reason: DeniedReason::Expired,
                scope: entry.token.scope().iter().next(),
            };
        }
        entry.submitted_by.insert(connection);
        SubmitOutcome::Granted(entry.token.scope())
    }

    /// The scopes a connection of `app_id` currently holds through
    /// submitted live tokens (the token half of the effective set).
    #[must_use]
    pub fn effective_scopes(&self, app_id: &str, connection: u32, now: Mono) -> ScopeSet {
        let mut out = ScopeSet::NONE;
        for entry in &self.entries {
            if entry.app_id == app_id
                && entry.submitted_by.contains(&connection)
                && !entry.revoked
                && entry.token.is_valid_at(now)
            {
                out = out.union(entry.token.scope());
            }
        }
        out
    }

    /// Revoke every live grant of `scope` for `app_id`; returns whether
    /// anything was live (drives the `revoked` event).
    pub fn revoke_scope(&mut self, app_id: &str, scope: Scope, _now: Mono) -> bool {
        let mut any = false;
        for entry in &mut self.entries {
            if entry.app_id == app_id && !entry.revoked && entry.token.scope().contains(scope) {
                entry.revoked = true;
                any = true;
            }
        }
        any
    }

    /// Drop every grant bound to one app (app uninstalled / connection
    /// class torn down). Returns the scopes that were live.
    #[must_use]
    pub fn purge_app(&mut self, app_id: &str) -> ScopeSet {
        let mut live = ScopeSet::NONE;
        self.entries.retain(|e| {
            if e.app_id == app_id {
                if !e.revoked {
                    live = live.union(e.token.scope());
                }
                false
            } else {
                true
            }
        });
        live
    }
}

/// Constant-time equality of the secret halves only (scope/expiry are
/// public and differ on the probe by construction).
fn ct_bytes_eq(a: &AccessToken, b: &AccessToken) -> bool {
    let mut diff: u8 = 0;
    for (x, y) in a.as_bytes().iter().zip(b.as_bytes().iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now(ns: u64) -> Mono {
        Mono::from_ns(ns)
    }

    #[test]
    fn mint_and_submit_round_trip() {
        let mut table = GrantTable::new();
        let mut seed = LcgSeed::new(7);
        let base = ScopeSet::single(Scope::ClipboardRead);
        let words = table.mint(&mut seed, "org.a", base, None);
        match table.submit("org.a", 4, words, now(1)) {
            SubmitOutcome::Granted(s) => assert_eq!(s, base),
            SubmitOutcome::Denied { .. } => panic!("unexpected denial"),
        }
        assert_eq!(table.effective_scopes("org.a", 4, now(1)), base);
        assert_eq!(table.effective_scopes("org.a", 5, now(1)), ScopeSet::NONE);
    }

    #[test]
    fn words_are_distinct_across_mints() {
        let mut table = GrantTable::new();
        let mut seed = LcgSeed::new(1);
        let mut minted = BTreeSet::new();
        for _ in 0..1000 {
            let w = table.mint(&mut seed, "app", ScopeSet::NONE, None);
            assert!(minted.insert(w), "token collision in 1000 mints");
        }
        assert_eq!(table.len(), 1000);
    }

    #[test]
    fn revoked_cross_app_and_forged_submissions() {
        let mut table = GrantTable::new();
        let mut seed = LcgSeed::new(11);
        let a_words = table.mint(
            &mut seed,
            "org.a",
            ScopeSet::single(Scope::Screenshot),
            None,
        );
        // Cross-app replay: the stolen token denies under another app's
        // identity, and the audit trail still learns the scope.
        assert_eq!(
            table.submit("org.evil", 8, a_words, now(1)),
            SubmitOutcome::Denied {
                reason: DeniedReason::InvalidToken,
                scope: Some(Scope::Screenshot)
            }
        );
        // Forgery: no bytewise match anywhere in the table.
        let mut forged = a_words;
        forged[7] ^= 1;
        assert_eq!(
            table.submit("org.a", 4, forged, now(1)),
            SubmitOutcome::Denied {
                reason: DeniedReason::InvalidToken,
                scope: None
            }
        );
        // Idempotent resubmission on the same connection.
        assert!(matches!(
            table.submit("org.a", 4, a_words, now(1)),
            SubmitOutcome::Granted(_)
        ));
        // Reconnect: a different connection of the same app may submit.
        assert!(matches!(
            table.submit("org.a", 9, a_words, now(2)),
            SubmitOutcome::Granted(_)
        ));
        assert_eq!(
            table.effective_scopes("org.a", 9, now(2)),
            ScopeSet::single(Scope::Screenshot)
        );
    }

    #[test]
    fn expired_boundary_is_exact() {
        let mut table = GrantTable::new();
        let mut seed = LcgSeed::new(2);
        let w = table.mint(&mut seed, "app", ScopeSet::NONE, Some(now(100)));
        assert!(matches!(
            table.submit("app", 1, w, now(100)),
            SubmitOutcome::Granted(_)
        ));
        assert_eq!(
            table.submit("app", 1, w, now(101)),
            SubmitOutcome::Denied {
                reason: DeniedReason::Expired,
                scope: None
            }
        );
    }

    #[test]
    fn revocation_kills_resubmission() {
        let mut table = GrantTable::new();
        let mut seed = LcgSeed::new(3);
        let w = table.mint(&mut seed, "app", ScopeSet::single(Scope::Screenshot), None);
        assert!(table.revoke_scope("app", Scope::Screenshot, now(5)));
        assert_eq!(
            table.submit("app", 1, w, now(6)),
            SubmitOutcome::Denied {
                reason: DeniedReason::InvalidToken,
                scope: Some(Scope::Screenshot)
            }
        );
        assert!(!table.revoke_scope("app", Scope::Screenshot, now(7)));
    }

    #[test]
    fn purge_app_reports_live_scopes() {
        let mut table = GrantTable::new();
        let mut seed = LcgSeed::new(4);
        let _ = table.mint(&mut seed, "a", ScopeSet::single(Scope::Bridge), None);
        let _ = table.mint(&mut seed, "b", ScopeSet::single(Scope::InputGrab), None);
        let live = table.purge_app("a");
        assert!(live.contains(Scope::Bridge));
        assert!(!live.contains(Scope::InputGrab));
        assert_eq!(table.len(), 1);
    }
}
