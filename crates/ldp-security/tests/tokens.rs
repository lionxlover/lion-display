//! THE Phase 16 exit criterion (part 2): token forgery and replay fail.
//!
//! Four corpora over the grant table:
//!
//! 1. **Scale**: 2,000 LCG-minted tokens, all distinct, all submittable
//!    by their owning app; effective scopes accumulate exactly.
//! 2. **Forgery**: 2,000 single-word mutations (LCG-chosen word, XOR
//!    masks) plus all-zero and all-ones probes — every one denies with
//!    `invalid_token` and no scope attribution.
//! 3. **Replay**: cross-app submission of stolen tokens denies with the
//!    *stolen* scope in the audit trail; revoked resubmission denies;
//!    same-app resubmission (reconnect) is legal by design.
//! 4. **Expiry sweep**: the boundary is exact per token across a staged
//!    expiry lattice; the broker's prompted TTL arithmetic lands on the
//!    same boundary.

use ldp_core::caps::{SandboxFlavor, Scope, ScopeSet};
use ldp_core::time::Mono;
use ldp_security::broker::{BrokerConfig, PromptAnswer, ScriptedDecider, SessionBroker};
use ldp_security::grant::{DeniedReason, GrantTable, LcgSeed, SubmitOutcome};
use ldp_security::manifest::AppManifest;
use ldp_security::EscalationOutcome;

/// Deterministic LCG for corpus steering (same discipline as the
/// scheduler/tearing corpora).
struct Corpus(u64);

impl Corpus {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

fn now(ns: u64) -> Mono {
    Mono::from_ns(ns)
}

fn manifest_for(app: &str, scopes: ScopeSet) -> AppManifest {
    AppManifest {
        app_id: app.to_owned(),
        version: 1,
        requested: scopes,
        sandbox: SandboxFlavor::Sandboxed,
    }
}

#[test]
fn corpus_two_thousand_mints_are_distinct_and_submittable() {
    let mut table = GrantTable::new();
    let mut seed = LcgSeed::new(0x00C0_FFEE);
    let mut unique = std::collections::BTreeSet::new();
    let scopes = [
        Scope::Screenshot,
        Scope::ScreenRecord,
        Scope::InputInject,
        Scope::ClipboardRead,
        Scope::InputGrab,
        Scope::AuditRead,
    ];
    let mut expected_effective = ScopeSet::NONE;
    for i in 0..2_000u32 {
        let scope = scopes[(i % scopes.len() as u32) as usize];
        let set = ScopeSet::single(scope);
        let words = table.mint(&mut seed, "org.corpus", set, Some(now(u64::MAX / 2)));
        assert!(unique.insert(words), "collision at mint {i}");
        assert!(matches!(
            table.submit("org.corpus", 1, words, now(i as u64)),
            SubmitOutcome::Granted(_)
        ));
        expected_effective = expected_effective.union(set);
    }
    assert_eq!(table.len(), 2000);
    assert_eq!(
        table.effective_scopes("org.corpus", 1, now(10_000)),
        expected_effective
    );
    // Another connection of the same app starts empty.
    assert_eq!(
        table.effective_scopes("org.corpus", 2, now(10_000)),
        ScopeSet::NONE
    );
}

#[test]
fn forgery_corpus_never_matches() {
    let mut table = GrantTable::new();
    let mut seed = LcgSeed::new(0x0BAD_F00D);
    let mut steer = Corpus(7);
    let mut tokens = Vec::new();
    for _ in 0..2_000 {
        tokens.push(table.mint(
            &mut seed,
            "org.victim",
            ScopeSet::single(Scope::Screenshot),
            None,
        ));
    }
    // Single-word mutations.
    for words in &tokens {
        let word = (steer.next() % 8) as usize;
        let mask = 1u32 << (steer.next() % 32);
        let mut forged = *words;
        forged[word] ^= mask;
        // Skip the (probability-zero) case the mutation lands back on a
        // minted token: with 2000 of 2^256, impossible unless the mask
        // rounds to zero, which XOR with a single bit never does.
        assert_eq!(
            table.submit("org.victim", 1, forged, now(0)),
            SubmitOutcome::Denied {
                reason: DeniedReason::InvalidToken,
                scope: None
            }
        );
    }
    // All-zero / all-ones / structural probes.
    for probe in [[0u32; 8], [u32::MAX; 8], [1, 0, 0, 0, 0, 0, 0, 0]] {
        assert_eq!(
            table.submit("org.victim", 1, probe, now(0)),
            SubmitOutcome::Denied {
                reason: DeniedReason::InvalidToken,
                scope: None
            }
        );
    }
    // Wrong length is a wire-level error before the table ever sees it;
    // here the type system enforces it (8 words is the only form).
    let _ = steer.next();
}

#[test]
fn replay_corpus_cross_app_and_revoked_fail() {
    let mut table = GrantTable::new();
    let mut seed = LcgSeed::new(0x0000_5EED);
    let apps = ["org.a", "org.b", "org.c"];
    let mut tokens = Vec::new();
    for app in apps {
        tokens.push(table.mint(&mut seed, app, ScopeSet::single(Scope::InputInject), None));
    }
    // Cross-app replay: app i submitting app j's token.
    for (i, words) in tokens.iter().enumerate() {
        for (j, app) in apps.iter().enumerate() {
            if i == j {
                continue;
            }
            assert_eq!(
                table.submit(app, 1, *words, now(0)),
                SubmitOutcome::Denied {
                    reason: DeniedReason::InvalidToken,
                    // The audit trail still attributes the stolen scope.
                    scope: Some(Scope::InputInject)
                },
                "app {app} replaying app {}'s token",
                apps[i]
            );
        }
    }
    // Revocation kills the token even for its owner.
    assert!(table.revoke_scope("org.a", Scope::InputInject, now(1)));
    assert_eq!(
        table.submit("org.a", 1, tokens[0], now(2)),
        SubmitOutcome::Denied {
            reason: DeniedReason::InvalidToken,
            scope: Some(Scope::InputInject)
        }
    );
    // Reconnect (new connection, same app, live token) is legal.
    assert!(matches!(
        table.submit("org.b", 9, tokens[1], now(3)),
        SubmitOutcome::Granted(_)
    ));
}

#[test]
fn expiry_lattice_is_exact() {
    let mut table = GrantTable::new();
    let mut seed = LcgSeed::new(0x1234);
    for i in 0..8u64 {
        let words = table.mint(
            &mut seed,
            "org.clock",
            ScopeSet::single(Scope::ScreenRecord),
            Some(now(100 + i)),
        );
        // Valid through the expiry instant, denied one nanosecond later.
        assert!(matches!(
            table.submit("org.clock", 1, words, now(100 + i)),
            SubmitOutcome::Granted(_)
        ));
        assert_eq!(
            table.submit("org.clock", 1, words, now(101 + i)),
            SubmitOutcome::Denied {
                reason: DeniedReason::Expired,
                scope: Some(Scope::ScreenRecord)
            }
        );
    }
}

#[test]
fn broker_prompted_ttl_lands_on_the_same_boundary() {
    let config = BrokerConfig {
        prompt_ttl_ms: 1000,
        ..BrokerConfig::default()
    };
    let mut broker = SessionBroker::new(config, Box::new(LcgSeed::new(99)));
    broker
        .register_manifest(manifest_for("org.ttl", ScopeSet::single(Scope::Screenshot)))
        .unwrap();
    let mut decider = ScriptedDecider::new(&[PromptAnswer::Allow]);
    let EscalationOutcome::Granted { words, expires } = broker.escalate(
        1,
        "org.ttl",
        Scope::Screenshot,
        "shoot",
        now(5_000),
        &mut decider,
    ) else {
        panic!("expected grant");
    };
    let exp = expires.expect("prompted grants are TTL'd");
    assert_eq!(exp.as_ns(), 5_000 + 1_000 * 1_000_000);
    assert!(matches!(
        broker.submit(1, "org.ttl", words, exp),
        SubmitOutcome::Granted(_)
    ));
    assert_eq!(
        broker.submit(1, "org.ttl", words, Mono::from_ns(exp.as_ns() + 1)),
        SubmitOutcome::Denied {
            reason: DeniedReason::Expired,
            scope: Some(Scope::Screenshot)
        }
    );
    // Session-bound (ttl 0) prompted grants never expire by clock.
    let mut broker2 = SessionBroker::new(
        BrokerConfig {
            prompt_ttl_ms: 0,
            ..BrokerConfig::default()
        },
        Box::new(LcgSeed::new(100)),
    );
    broker2
        .register_manifest(manifest_for("org.ttl", ScopeSet::single(Scope::Screenshot)))
        .unwrap();
    let mut decider2 = ScriptedDecider::new(&[PromptAnswer::Allow]);
    let EscalationOutcome::Granted { words, expires } = broker2.escalate(
        1,
        "org.ttl",
        Scope::Screenshot,
        "shoot",
        now(0),
        &mut decider2,
    ) else {
        panic!("expected grant");
    };
    assert!(expires.is_none());
    assert!(matches!(
        broker2.submit(1, "org.ttl", words, now(u64::MAX / 4)),
        SubmitOutcome::Granted(_)
    ));
}

#[test]
fn broker_revoke_reports_and_audits() {
    let mut broker = SessionBroker::new(BrokerConfig::default(), Box::new(LcgSeed::new(101)));
    broker
        .register_manifest(manifest_for("org.r", ScopeSet::single(Scope::Screenshot)))
        .unwrap();
    let mut decider = ScriptedDecider::new(&[PromptAnswer::Allow, PromptAnswer::Allow]);
    let EscalationOutcome::Granted { words, .. } =
        broker.escalate(1, "org.r", Scope::Screenshot, "r", now(0), &mut decider)
    else {
        panic!("grant");
    };
    assert!(matches!(
        broker.submit(1, "org.r", words, now(1)),
        SubmitOutcome::Granted(_)
    ));
    assert!(broker.revoke(1, "org.r", Scope::Screenshot, now(2)));
    // The revoked token denies on resubmission.
    assert_eq!(
        broker.submit(1, "org.r", words, now(3)),
        SubmitOutcome::Denied {
            reason: DeniedReason::InvalidToken,
            scope: Some(Scope::Screenshot)
        }
    );
    // Revoking again reports nothing live.
    assert!(!broker.revoke(1, "org.r", Scope::Screenshot, now(4)));
    // Audit order: grant, revoke (live), the resubmission denial, and
    // the second revoke attempt (audited even with nothing live).
    let records = broker.audit().records();
    let actions: Vec<_> = records.iter().map(|r| r.event.action).collect();
    assert_eq!(
        actions,
        vec![
            ldp_security::AuditAction::Grant,
            ldp_security::AuditAction::Revoke,
            ldp_security::AuditAction::Deny,
            ldp_security::AuditAction::Revoke,
        ]
    );
    assert!(broker.audit().verify().ok);
}

#[test]
fn purge_app_drops_everything_together() {
    let mut broker = SessionBroker::new(BrokerConfig::default(), Box::new(LcgSeed::new(102)));
    broker
        .register_manifest(manifest_for(
            "org.gone",
            ScopeSet::single(Scope::ClipboardRead).with(Scope::Screenshot),
        ))
        .unwrap();
    let mut decider = ScriptedDecider::new(&[PromptAnswer::Allow]);
    let EscalationOutcome::Granted { words, .. } =
        broker.escalate(1, "org.gone", Scope::Screenshot, "s", now(0), &mut decider)
    else {
        panic!("grant");
    };
    assert!(matches!(
        broker.submit(1, "org.gone", words, now(1)),
        SubmitOutcome::Granted(_)
    ));
    let live = broker.purge_app("org.gone");
    assert!(live.contains(Scope::Screenshot));
    // The baseline is gone with the manifest; the token is dead.
    assert_eq!(broker.baseline("org.gone"), ScopeSet::NONE);
    // The token is gone with the entry: no bytewise match remains, so
    // the resubmission denies like a forgery (no scope attribution).
    assert_eq!(
        broker.submit(1, "org.gone", words, now(2)),
        SubmitOutcome::Denied {
            reason: DeniedReason::InvalidToken,
            scope: None
        }
    );
    assert_eq!(
        broker.effective_scopes(1, "org.gone", now(3)),
        ScopeSet::NONE
    );
}
