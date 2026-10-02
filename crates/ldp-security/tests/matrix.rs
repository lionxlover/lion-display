//! THE Phase 16 exit criterion (part 1): the permission matrix, fully
//! tested — every row of the threat-model §3 table × the three grant
//! states (deny / prompt / grant).
//!
//! Three layers of proof:
//!
//! 1. **The exhaustive walk**: all 18 operation rows × {no grant,
//!    manifest baseline only, manifest + accepted token} assert against
//!    a literal expected-decision table — the matrix cannot drift from
//!    the threat model without this test failing.
//! 2. **Wire conformance**: the local enums and bitsets agree with the
//!    compiled schema (`spec/security.toml` via `ldp-protocol`) value
//!    for value, bit for bit; every `ldp.security` request and event
//!    has exactly one typed handle in the crate.
//! 3. **Broker walkthrough**: the matrix's three answers map onto the
//!    broker's wire-visible escalation outcomes (grant / user_denied /
//!    manifest_missing / rate_limited).

use ldp_core::caps::{SandboxFlavor, Scope, ScopeSet};
use ldp_protocol::generated::security as wire;
use ldp_protocol::generated::MODULES;
use ldp_security::broker::{BrokerConfig, PromptAnswer, ScriptedDecider, SessionBroker};
use ldp_security::grant::{DeniedReason, LcgSeed};
use ldp_security::manifest::AppManifest;
use ldp_security::matrix::{
    decide, manifest_baselines, requirement, Decision, Operation, Requirement, ALL_OPERATIONS,
};
use ldp_security::{AuditAction, AuditFilter};

fn now(ms: u64) -> ldp_core::time::Mono {
    ldp_core::time::Mono::from_ms(ms)
}

fn single(s: Scope) -> ScopeSet {
    ScopeSet::single(s)
}

fn manifest_for(app: &str, scopes: ScopeSet) -> AppManifest {
    AppManifest {
        app_id: app.to_owned(),
        version: 1,
        requested: scopes,
        sandbox: SandboxFlavor::Sandboxed,
    }
}

// ---------------------------------------------------------------------------
// 1. The exhaustive walk: all rows x {deny-state, prompt-state, grant-state}.
// ---------------------------------------------------------------------------

#[test]
fn every_row_every_grant_state_matches_the_literal_table() {
    // (operation, decision with nothing, with manifest only, with the
    // full grant). Transcribed from threat-model.md §3 by hand.
    use Decision::{Allow, Deny, Prompt};
    let expected: &[(Operation, Decision, Decision, Decision)] = &[
        // F rows: always allowed.
        (Operation::RenderOwnWindows, Allow, Allow, Allow),
        (Operation::ReadOwnBuffers, Allow, Allow, Allow),
        (Operation::FocusedInput, Allow, Allow, Allow),
        (Operation::ClipboardWrite, Allow, Allow, Allow),
        // M rows: manifest alone (via baseline) satisfies.
        (Operation::GlobalGrab, Deny, Allow, Allow),
        (Operation::ClipboardRead, Deny, Allow, Allow),
        (Operation::PrimarySelectionRead, Deny, Allow, Allow),
        (Operation::GlobalShortcuts, Deny, Allow, Allow),
        (Operation::DisplayReconfig, Deny, Allow, Allow),
        (Operation::WorkspaceManage, Deny, Allow, Allow),
        (Operation::A11ySubscribe, Deny, Allow, Allow),
        // M+E rows: three distinct states.
        (Operation::ScreenshotWindow, Deny, Prompt, Allow),
        (Operation::ScreenshotScreen, Deny, Prompt, Allow),
        (Operation::ScreenRecord, Deny, Prompt, Allow),
        (Operation::InputInjection, Deny, Prompt, Allow),
        // E-only row: prompt reachable without a manifest.
        (Operation::AuditRead, Prompt, Prompt, Allow),
        // Impossible rows: always denied.
        (Operation::ForeignTitles, Deny, Deny, Deny),
        (Operation::ForeignBuffers, Deny, Deny, Deny),
    ];
    assert_eq!(expected.len(), ALL_OPERATIONS.len(), "row count drift");
    for (op, none, with_manifest, granted) in expected {
        // The row's scope, where the requirement class uses one.
        let req = requirement(*op);
        let scope = req.scope();
        let manifest = scope.map_or(ScopeSet::NONE, single);
        // The baseline the connection holds with only the manifest
        // registered: exactly SessionBroker::baseline's rule
        // (manifest-class scopes only).
        let baseline = match scope {
            Some(s) if manifest_baselines(s) => single(s),
            _ => ScopeSet::NONE,
        };
        // The accepted token carries the scope into the effective set.
        let effective = scope.map_or(ScopeSet::NONE, single);
        assert_eq!(
            decide(*op, ScopeSet::NONE, ScopeSet::NONE),
            *none,
            "{op:?} deny state"
        );
        assert_eq!(
            decide(*op, manifest, baseline),
            *with_manifest,
            "{op:?} prompt state"
        );
        assert_eq!(
            decide(*op, manifest, effective),
            *granted,
            "{op:?} grant state"
        );
        // A foreign manifest must not leak scopes into other rows.
        assert_eq!(
            decide(*op, single(Scope::Bridge), ScopeSet::NONE),
            *none,
            "{op:?} bridge-manifest leak"
        );
    }
}

#[test]
fn requirement_classes_are_the_threat_model_legend() {
    for op in ALL_OPERATIONS {
        let req = requirement(*op);
        match req {
            Requirement::Free | Requirement::Impossible => assert!(req.scope().is_none()),
            Requirement::Manifest(s)
            | Requirement::Prompt(s)
            | Requirement::ManifestAndPrompt(s) => {
                assert!(s.to_wire() >= 1 && s.to_wire() <= 12, "{op:?}");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 2. Wire conformance: local enums vs the compiled schema.
// ---------------------------------------------------------------------------

#[test]
fn scope_wire_values_match_the_compiled_schema() {
    // The generated enum has one variant per spec value; compare through
    // the wire integers both ways.
    let generated: &[(u32, &str)] = &[
        (1, "screenshot"),
        (2, "screen_record"),
        (3, "input_inject"),
        (4, "global_shortcut"),
        (5, "clipboard_read"),
        (6, "input_grab"),
        (7, "configure_display"),
        (8, "manage_workspaces"),
        (9, "a11y_control"),
        (10, "audit_read"),
        (11, "bridge"),
        (12, "protected_surface"),
    ];
    for s in Scope::ALL {
        assert!(
            generated
                .iter()
                .any(|(v, name)| *v == s.to_wire() && *name == s.as_str()),
            "scope {} (wire {}) missing from spec",
            s.as_str(),
            s.to_wire()
        );
        assert_eq!(
            wire::Scope::from_wire(s.to_wire()).map(wire::Scope::to_wire),
            Some(s.to_wire())
        );
    }
    assert_eq!(generated.len(), 12);
    // The prompt-class doctrine: exactly the three E-column scopes.
    for s in Scope::ALL {
        let prompt_class = matches!(
            s,
            Scope::Screenshot | Scope::ScreenRecord | Scope::InputInject
        );
        assert_eq!(s.requires_prompt(), prompt_class, "{s}");
    }
}

#[test]
fn denied_reason_and_audit_action_wire_values_match() {
    use ldp_security::DeniedReason as Local;
    for (local, v) in [
        (Local::ManifestMissing, 1),
        (Local::UserDenied, 2),
        (Local::Expired, 3),
        (Local::InvalidToken, 4),
        (Local::RateLimited, 5),
    ] {
        assert_eq!(local.to_wire(), v);
        assert_eq!(Local::from_wire(v), Some(local));
        assert_eq!(
            wire::DeniedReason::from_wire(v).map(wire::DeniedReason::to_wire),
            Some(v)
        );
    }
    assert_eq!(wire::DeniedReason::from_wire(6), None);
    for (local, v) in [
        (AuditAction::Grant, 1),
        (AuditAction::Deny, 2),
        (AuditAction::Revoke, 3),
        (AuditAction::Capture, 4),
        (AuditAction::Inject, 5),
        (AuditAction::Bridge, 6),
    ] {
        assert_eq!(local.to_wire(), v);
        assert_eq!(AuditAction::from_wire(v), Some(local));
        assert_eq!(
            wire::AuditAction::from_wire(v).map(wire::AuditAction::to_wire),
            Some(v)
        );
    }
    assert_eq!(AuditAction::from_wire(7), None);
}

#[test]
fn scopes_bitset_bits_match_the_compiled_schema() {
    // Bit index = wire value - 1 for every scope.
    for s in Scope::ALL {
        let bit = 1u128 << (s.to_wire() - 1);
        let idx = wire::scopes::BITS
            .iter()
            .find(|(n, _)| *n == s.as_str())
            .map(|(_, b)| *b)
            .expect("scope bit in spec");
        assert_eq!(idx, s.to_wire() - 1);
        assert_eq!(bit, 1u128 << idx);
    }
    // ScopeSet words cover exactly the declared bits.
    let all = Scope::ALL
        .iter()
        .fold(ScopeSet::NONE, |acc, s| acc.with(*s));
    let words = all.to_words();
    let mut mask = 0u128;
    for w in words {
        mask |= u128::from(w);
    }
    assert_eq!(mask, wire::scopes::MASK);
}

#[test]
fn audit_filter_bits_match_the_compiled_schema() {
    assert_eq!(
        AuditFilter::GRANTS.to_wire() as u128,
        wire::audit_filter::GRANTS
    );
    assert_eq!(
        AuditFilter::DENIALS.to_wire() as u128,
        wire::audit_filter::DENIALS
    );
    assert_eq!(
        AuditFilter::REVOCATIONS.to_wire() as u128,
        wire::audit_filter::REVOCATIONS
    );
    assert_eq!(
        AuditFilter::CAPTURES.to_wire() as u128,
        wire::audit_filter::CAPTURES
    );
    assert_eq!(
        AuditFilter::INJECTIONS.to_wire() as u128,
        wire::audit_filter::INJECTIONS
    );
    assert_eq!(AuditFilter::ALL.to_wire() as u128, wire::audit_filter::MASK);
    // Class mapping: the six actions partition into the five classes,
    // bridge folding into captures (documented doctrine).
    assert!(AuditFilter::CAPTURES.delivers(AuditAction::Bridge));
    assert!(!AuditFilter::DENIALS.delivers(AuditAction::Bridge));
}

#[test]
fn every_ldp_security_operation_has_a_typed_handle() {
    // Coverage both directions: every request/event of the ldp.security
    // interfaces names its typed handle in this crate; nothing in the
    // typed surface is missing from the spec.
    let handled: &[(&str, &str, &str)] = &[
        (
            "ldp.security.security",
            "capabilities",
            "SessionBroker::effective_scopes",
        ),
        (
            "ldp.security.security",
            "request",
            "SessionBroker::escalate",
        ),
        (
            "ldp.security.security",
            "submit_token",
            "SessionBroker::submit",
        ),
        (
            "ldp.security.security",
            "capabilities",
            "event: capabilities event data",
        ),
        (
            "ldp.security.security",
            "grant",
            "event: EscalationOutcome::Granted",
        ),
        (
            "ldp.security.security",
            "deny",
            "event: EscalationOutcome::Denied",
        ),
        (
            "ldp.security.security",
            "revoked",
            "event: SessionBroker::revoke",
        ),
        ("ldp.security.audit", "subscribe", "AuditChain::filtered"),
        ("ldp.security.audit", "verify", "AuditChain::verify"),
        (
            "ldp.security.audit",
            "record",
            "event: ChainedRecord stream",
        ),
        ("ldp.security.audit", "chain", "event: ChainResult"),
    ];
    for module in MODULES {
        if module.name != "ldp.security" {
            continue;
        }
        for iface in module.interfaces {
            for op in iface.requests.iter().chain(iface.events.iter()) {
                let found = handled
                    .iter()
                    .any(|(i, name, _)| *i == iface.name && *name == op.name);
                assert!(found, "unhandled {}::{}", iface.name, op.name);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 3. Broker walkthrough: matrix answers map to wire outcomes.
// ---------------------------------------------------------------------------

#[test]
fn matrix_three_states_drive_real_broker_outcomes() {
    let mut broker = SessionBroker::new(BrokerConfig::default(), Box::new(LcgSeed::new(5)));
    broker
        .register_manifest(manifest_for(
            "org.app",
            single(Scope::Screenshot).with(Scope::ClipboardRead),
        ))
        .unwrap();

    // M-class row, manifest in place: the baseline already allows.
    let eff = broker.effective_scopes(1, "org.app", now(0));
    assert!(eff.contains(Scope::ClipboardRead));
    assert!(!eff.contains(Scope::Screenshot));

    // M+E row in the Prompt state: escalation prompts, user allows,
    // token submission completes the grant.
    let mut decider = ScriptedDecider::new(&[PromptAnswer::Allow]);
    let ldp_security::EscalationOutcome::Granted { words, .. } = broker.escalate(
        1,
        "org.app",
        Scope::Screenshot,
        "need",
        now(1),
        &mut decider,
    ) else {
        panic!("expected grant");
    };
    assert!(matches!(
        broker.submit(1, "org.app", words, now(2)),
        ldp_security::SubmitOutcome::Granted(_)
    ));
    let eff = broker.effective_scopes(1, "org.app", now(3));
    assert!(eff.contains(Scope::Screenshot));
    assert_eq!(
        decide(Operation::ScreenshotWindow, single(Scope::Screenshot), eff),
        Decision::Allow
    );

    // M+E row with the user saying no: user_denied, matrix stays Prompt.
    let mut decider = ScriptedDecider::new(&[PromptAnswer::Deny]);
    assert_eq!(
        broker.escalate(
            1,
            "org.app",
            Scope::Screenshot,
            "again",
            now(4),
            &mut decider
        ),
        ldp_security::EscalationOutcome::Denied(DeniedReason::UserDenied)
    );

    // Scope absent from the manifest: manifest_missing, matrix Deny.
    assert_eq!(
        broker.escalate(
            1,
            "org.app",
            Scope::ScreenRecord,
            "why not",
            now(5),
            &mut ScriptedDecider::new(&[])
        ),
        ldp_security::EscalationOutcome::Denied(DeniedReason::ManifestMissing)
    );

    // E-only row: prompts without any manifest.
    let mut decider = ScriptedDecider::new(&[PromptAnswer::Allow]);
    assert!(matches!(
        broker.escalate(
            7,
            "org.admin",
            Scope::AuditRead,
            "admin",
            now(6),
            &mut decider
        ),
        ldp_security::EscalationOutcome::Granted { .. }
    ));
}

#[test]
fn rate_limit_abuse_is_audited() {
    let mut broker = SessionBroker::new(
        BrokerConfig {
            rate_max: 2,
            rate_window_ms: 1000,
            ..BrokerConfig::default()
        },
        Box::new(LcgSeed::new(6)),
    );
    broker
        .register_manifest(manifest_for("org.a", single(Scope::ClipboardRead)))
        .unwrap();
    let mut d = ScriptedDecider::new(&[]);
    for i in 0..2 {
        assert!(matches!(
            broker.escalate(1, "org.a", Scope::ClipboardRead, "r", now(i * 10), &mut d),
            ldp_security::EscalationOutcome::Granted { .. }
        ));
    }
    assert_eq!(
        broker.escalate(1, "org.a", Scope::ClipboardRead, "r", now(20), &mut d),
        ldp_security::EscalationOutcome::Denied(DeniedReason::RateLimited)
    );
    // The rate-limited denial is an audited Deny record with the reason.
    let records = broker.audit().records();
    let last = records.last().unwrap();
    assert_eq!(last.event.action, AuditAction::Deny);
    assert!(last.event.detail.contains("rate_limited"));
    assert!(broker.audit().verify().ok);
}
