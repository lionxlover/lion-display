//! THE Phase 16 exit criterion (part 3): the audit chain verifies —
//! and every tampering strategy fails.
//!
//! Layers:
//!
//! 1. **Growth**: verify stays green at every append through 1,000
//!    records, with exact record counts.
//! 2. **Tamper corpus**: field flips (each canonical field), pairwise
//!    reorders, tail truncation, forged appends — every one breaks
//!    verification at or before the touched record.
//! 3. **Ring window**: eviction keeps the chain verifiable from the
//!    anchor; drop counts and retained seqs are exact.
//! 4. **Broker flow**: a full escalation session (grants, denials,
//!    revocation, captures, injections, bridge usage) lands in the
//!    chain in order, verifies, and filters by class exactly; every
//!    JSONL line round-trips structurally.

use ldp_core::caps::{SandboxFlavor, Scope, ScopeSet};
use ldp_core::time::Mono;
use ldp_security::broker::{BrokerConfig, PromptAnswer, ScriptedDecider, SessionBroker};
use ldp_security::chain::{AuditAction, AuditChain, AuditEvent, AuditFilter};
use ldp_security::grant::LcgSeed;
use ldp_security::manifest::AppManifest;

fn event(action: AuditAction, client: u32, ts: u64) -> AuditEvent {
    AuditEvent {
        ts_ns: ts,
        client,
        app_id: "org.example.app".to_owned(),
        scope: Some(Scope::Screenshot),
        action,
        detail: "{\"k\":\"v\"}".to_owned(),
    }
}

/// Rebuild the chain with tampered records — the persisted-store path
/// an attacker controls (the JSONL reader's untrusted input).
fn tampered(
    chain: &AuditChain,
    mutate: impl FnOnce(&mut Vec<ldp_security::ChainedRecord>),
) -> AuditChain {
    let mut recs = chain.records().to_vec();
    mutate(&mut recs);
    AuditChain::from_records(
        recs,
        chain.anchor(),
        chain.head(),
        chain.capacity(),
        chain.dropped(),
    )
}

#[test]
fn chain_verifies_through_a_thousand_appends() {
    let mut chain = AuditChain::new(0);
    for i in 0..1000u64 {
        chain.append(event(AuditAction::Grant, (i % 7) as u32, i * 1000));
        if i % 97 == 0 {
            let r = chain.verify();
            assert!(r.ok, "verify broke at append {i}");
            assert_eq!(r.records, i + 1);
        }
    }
    let r = chain.verify();
    assert!(r.ok);
    assert_eq!(r.records, 1000);
    assert_eq!(chain.records().len(), 1000);
    assert_eq!(chain.dropped(), 0);
}

#[test]
fn every_field_flip_breaks_the_chain() {
    let mut chain = AuditChain::new(0);
    for i in 0..12 {
        chain.append(event(AuditAction::Deny, 3, i * 10));
    }
    assert!(chain.verify().ok);
    // Scalar fields of every record.
    for idx in 0..12usize {
        for field in 0..4u8 {
            let broken = tampered(&chain, |recs| {
                let e = &mut recs[idx].event;
                match field {
                    0 => e.ts_ns ^= 1,
                    1 => e.client ^= 1,
                    2 => {
                        e.scope = e
                            .scope
                            .map(Scope::to_wire)
                            .and_then(|w| Scope::from_wire(w ^ 1));
                    }
                    3 => e.action = AuditAction::from_wire(e.action.to_wire() % 6 + 1).unwrap(),
                    _ => unreachable!(),
                }
            });
            assert!(
                !broken.verify().ok,
                "field {field} of record {idx} flipped silently"
            );
        }
        // String fields: replace the first byte of app_id and detail.
        for which in [0usize, 1usize] {
            let broken = tampered(&chain, |recs| {
                let e = &mut recs[idx].event;
                match which {
                    0 => e.app_id = format!("X{}", &e.app_id[1..]),
                    _ => e.detail = format!("X{}", &e.detail[1..]),
                }
            });
            assert!(
                !broken.verify().ok,
                "string field {which} of record {idx} flipped silently"
            );
        }
    }
}

#[test]
fn reorder_truncation_and_forged_append_fail() {
    let mut chain = AuditChain::new(0);
    for i in 0..10 {
        chain.append(event(AuditAction::Revoke, 4, i));
    }
    // Pairwise swaps of adjacent records.
    for i in 0..9usize {
        let swapped = tampered(&chain, |recs| recs.swap(i, i + 1));
        assert!(!swapped.verify().ok, "swap at {i} undetected");
    }
    // Tail truncation against the retained head.
    for cut in 1..10usize {
        let cut_chain = tampered(&chain, |recs| recs.truncate(10 - cut));
        assert!(!cut_chain.verify().ok, "truncation of {cut} undetected");
    }
    // Forged append: a record claiming to extend the chain.
    let forged = tampered(&chain, |recs| {
        recs.push(ldp_security::ChainedRecord {
            seq: 11,
            event: event(AuditAction::Grant, 4, 99),
            digest: [0xAB; 32],
        });
    });
    assert!(!forged.verify().ok);
    // Digest substitution in place.
    let digested = tampered(&chain, |recs| recs[5].digest = [0xCD; 32]);
    assert!(!digested.verify().ok);
}

#[test]
fn ring_window_stays_verifiable_across_evictions() {
    let mut chain = AuditChain::new(10);
    for i in 0..100u64 {
        chain.append(event(AuditAction::Capture, 5, i));
        assert!(chain.verify().ok, "ring broke at append {i}");
    }
    assert_eq!(chain.records().len(), 10);
    assert_eq!(chain.dropped(), 90);
    assert_eq!(chain.records()[0].seq, 91);
    assert_eq!(chain.records()[9].seq, 100);
    // The anchor moved off genesis (it is the digest of the last evicted
    // record, seq 90) — the retained window re-verifies from it.
    assert_ne!(chain.anchor(), [0u8; 32]);
    assert!(chain.verify().ok);
}

#[test]
fn filtering_by_class_is_exact() {
    let mut chain = AuditChain::new(0);
    let actions = [
        AuditAction::Grant,
        AuditAction::Deny,
        AuditAction::Revoke,
        AuditAction::Capture,
        AuditAction::Inject,
        AuditAction::Bridge,
    ];
    for i in 0..60 {
        chain.append(event(actions[i % actions.len()], 6, i as u64));
    }
    let grants = chain.filtered(AuditFilter::GRANTS);
    assert_eq!(grants.len(), 10);
    assert!(grants.iter().all(|r| r.event.action == AuditAction::Grant));
    let captures = chain.filtered(AuditFilter::CAPTURES);
    assert_eq!(captures.len(), 20, "captures class includes bridge");
    assert!(captures
        .iter()
        .all(|r| matches!(r.event.action, AuditAction::Capture | AuditAction::Bridge)));
    let none = chain.filtered(AuditFilter::NONE);
    assert!(none.is_empty());
    let all = chain.filtered(AuditFilter::ALL);
    assert_eq!(all.len(), 60);
}

#[test]
fn jsonl_lines_are_structurally_round_trippable() {
    let mut chain = AuditChain::new(0);
    for i in 0..25 {
        let mut e = event(AuditAction::Inject, 7, i);
        e.detail = format!("{{\"events\":{},\"note\":\"n{}\"}}", i * 3, i);
        chain.append(e);
    }
    let mut prev_seq = 0u64;
    for record in chain.records() {
        let line = AuditChain::jsonl_line(record);
        assert!(line.starts_with(&format!("{{\"seq\":{},", record.seq)));
        assert!(line.ends_with('}'));
        assert!(record.seq > prev_seq);
        prev_seq = record.seq;
        // The digest is hex-quoted and 64 wide.
        let tail = line.trim_end_matches('}');
        assert!(tail.ends_with(&format!(
            "\"digest\":\"{}\"",
            ldp_security::sha256::hex32(&record.digest)
        )));
    }
}

#[test]
fn full_broker_session_lands_in_the_chain_in_order() {
    let mut broker = SessionBroker::new(BrokerConfig::default(), Box::new(LcgSeed::new(7)));
    broker
        .register_manifest(AppManifest {
            app_id: "org.session".to_owned(),
            version: 2,
            requested: ScopeSet::single(Scope::Screenshot).with(Scope::ClipboardRead),
            sandbox: SandboxFlavor::Portal,
        })
        .unwrap();
    let t0 = Mono::from_ns(1000);
    let mut decider = ScriptedDecider::new(&[PromptAnswer::Allow, PromptAnswer::Deny]);
    // Grant via prompt, then a user denial, then a manifest-missing denial.
    let ldp_security::EscalationOutcome::Granted { words, .. } = broker.escalate(
        1,
        "org.session",
        Scope::Screenshot,
        "shoot",
        t0,
        &mut decider,
    ) else {
        panic!("grant");
    };
    assert!(matches!(
        broker.escalate(
            1,
            "org.session",
            Scope::Screenshot,
            "again",
            t0,
            &mut decider
        ),
        ldp_security::EscalationOutcome::Denied(ldp_security::DeniedReason::UserDenied)
    ));
    assert_eq!(
        broker.escalate(
            1,
            "org.session",
            Scope::ScreenRecord,
            "no manifest",
            t0,
            &mut decider
        ),
        ldp_security::EscalationOutcome::Denied(ldp_security::DeniedReason::ManifestMissing)
    );
    // Submission + enforcement-side records.
    assert!(matches!(
        broker.submit(1, "org.session", words, t0),
        ldp_security::SubmitOutcome::Granted(_)
    ));
    broker.record_capture(
        1,
        "org.session",
        Scope::Screenshot,
        "{\"window\":\"toplevel\"}",
        t0,
    );
    broker.record_inject(1, "org.session", 12, t0);
    broker.record_bridge(2, "org.x11.bridge", "{\"client\":\"xclock\"}", t0);
    broker.revoke(1, "org.session", Scope::Screenshot, t0);

    let records = broker.audit().records();
    let actions: Vec<_> = records.iter().map(|r| r.event.action).collect();
    assert_eq!(
        actions,
        vec![
            AuditAction::Grant,
            AuditAction::Deny,
            AuditAction::Deny,
            AuditAction::Capture,
            AuditAction::Inject,
            AuditAction::Bridge,
            AuditAction::Revoke,
        ]
    );
    // Order of arrival, one client per record, scope attribution exact.
    assert_eq!(records[0].event.client, 1);
    assert_eq!(records[5].event.client, 2);
    assert_eq!(records[3].event.scope, Some(Scope::Screenshot));
    assert_eq!(records[4].event.scope, Some(Scope::InputInject));
    assert_eq!(records[5].event.scope, Some(Scope::Bridge));
    // The whole session verifies.
    let result = broker.audit().verify();
    assert!(result.ok);
    assert_eq!(result.records, 7);
    // Filtering the session by class.
    assert_eq!(broker.audit().filtered(AuditFilter::GRANTS).len(), 1);
    assert_eq!(broker.audit().filtered(AuditFilter::DENIALS).len(), 2);
    assert_eq!(broker.audit().filtered(AuditFilter::REVOCATIONS).len(), 1);
    assert_eq!(broker.audit().filtered(AuditFilter::CAPTURES).len(), 2);
    assert_eq!(broker.audit().filtered(AuditFilter::INJECTIONS).len(), 1);
}
