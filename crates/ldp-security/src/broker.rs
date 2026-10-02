//! The session broker — manifests, escalation prompts, token minting.
//!
//! The broker is the sole authority that turns asks into grants
//! (`docs/architecture.md` §16): the server core *enforces* scopes but
//! never *decides* them. As a library component it is hosted in-process
//! here; the deployment may move it behind a socket without touching
//! this logic (all state transitions are pure functions of injected
//! time and the [`PromptDecider`] answer).
//!
//! Escalation flow for `security.request(scope, reason)`:
//!
//! 1. **Manifest gate.** Prompt-class scopes (`screenshot`,
//!    `screen_record`, `input_inject`) and manifest-class scopes must
//!    appear in the app's registered manifest — otherwise
//!    [`DeniedReason::ManifestMissing`]. Prompt-*only* scopes
//!    (`audit_read`) have no manifest prerequisite (administrator flow).
//! 2. **Rate limit.** More than `rate_max` requests inside the window
//!    deny with `RateLimited`. Abuse is audited like every decision.
//! 3. **Prompt.** Prompt-class scopes consult the [`PromptDecider`] —
//!    the brokered UI seam; a denial answers
//!    [`DeniedReason::UserDenied`].
//! 4. **Mint.** A 256-bit token (scope-bounded, TTL'd or session-bound)
//!    joins the grant table and the decision is audited.
//!
//! The manifest baseline (manifest scopes minus the prompt-class ones)
//! is effective at connect time with no token — that is the
//! `capabilities` event's "manifest baseline + accepted tokens" set.

use std::collections::BTreeMap;
use std::collections::VecDeque;

use ldp_core::caps::{Scope, ScopeSet};
use ldp_core::time::Mono;

use crate::chain::{AuditAction, AuditChain, AuditEvent};
use crate::grant::{DeniedReason, GrantTable, SubmitOutcome, TokenSeed};
use crate::manifest::{AppManifest, ManifestError};

/// The brokered user-prompt seam (TCC-style escalation UI).
pub trait PromptDecider {
    /// Answer one escalation prompt.
    fn decide(&mut self, app_id: &str, scope: Scope, reason: &str) -> PromptAnswer;
}

/// A prompt answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptAnswer {
    /// The user allowed it.
    Allow,
    /// The user denied it.
    Deny,
}

/// A decider that answers from a scripted queue — the integration-test
/// harness for prompt UX flows.
#[derive(Debug, Default)]
pub struct ScriptedDecider {
    answers: VecDeque<PromptAnswer>,
}

impl ScriptedDecider {
    /// Queue the given answers in order.
    pub fn new(answers: &[PromptAnswer]) -> Self {
        ScriptedDecider {
            answers: answers.iter().copied().collect(),
        }
    }

    /// Answers not yet consumed.
    pub fn pending(&self) -> usize {
        self.answers.len()
    }
}

impl PromptDecider for ScriptedDecider {
    fn decide(&mut self, _app_id: &str, _scope: Scope, _reason: &str) -> PromptAnswer {
        self.answers.pop_front().unwrap_or(PromptAnswer::Deny)
    }
}

/// Broker tuning.
#[derive(Clone, Debug)]
pub struct BrokerConfig {
    /// Token lifetime for prompted grants (ms); 0 = session-bound.
    pub prompt_ttl_ms: u64,
    /// Maximum escalation requests per app per window.
    pub rate_max: usize,
    /// Rate window (ms).
    pub rate_window_ms: u64,
    /// Retained audit records before the ring drops the oldest.
    pub audit_capacity: usize,
}

impl Default for BrokerConfig {
    fn default() -> Self {
        BrokerConfig {
            prompt_ttl_ms: 600_000,
            rate_max: 3,
            rate_window_ms: 30_000,
            audit_capacity: 4096,
        }
    }
}

/// The escalation outcome handed back to the requesting connection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EscalationOutcome {
    /// Granted: the minted token words and their expiry (None =
    /// session-bound).
    Granted {
        /// 8-word wire form of the minted token.
        words: [u32; 8],
        /// Expiry; `None` = session-bound.
        expires: Option<Mono>,
    },
    /// Denied with the wire-stable reason.
    Denied(DeniedReason),
}

/// The broker: manifest registry + grant table + audit chain + rate
/// limiter. One instance serves a whole LDP session.
pub struct SessionBroker {
    manifests: BTreeMap<String, AppManifest>,
    grants: GrantTable,
    audit: AuditChain,
    rate: BTreeMap<String, VecDeque<Mono>>,
    config: BrokerConfig,
    seed: Box<dyn TokenSeed>,
}

impl SessionBroker {
    /// A broker with the given config and entropy source.
    pub fn new(config: BrokerConfig, seed: Box<dyn TokenSeed>) -> Self {
        SessionBroker {
            manifests: BTreeMap::new(),
            grants: GrantTable::new(),
            audit: AuditChain::new(config.audit_capacity),
            rate: BTreeMap::new(),
            config,
            seed,
        }
    }

    /// Register (or replace) an app's manifest. Returns whether an
    /// earlier manifest was replaced.
    ///
    /// # Errors
    ///
    /// [`ManifestError`] when the manifest is malformed — the broker
    /// refuses to remember an invalid baseline.
    pub fn register_manifest(&mut self, manifest: AppManifest) -> Result<bool, ManifestError> {
        manifest.validate()?;
        Ok(self
            .manifests
            .insert(manifest.app_id.clone(), manifest)
            .is_some())
    }

    /// The manifest-derived baseline: registered scopes the matrix lets
    /// a manifest alone grant (manifest-class rows only — prompt-class
    /// and prompt-only scopes never baseline). Effective at connect
    /// time, no token needed.
    #[must_use]
    pub fn baseline(&self, app_id: &str) -> ScopeSet {
        match self.manifests.get(app_id) {
            Some(m) => m
                .requested
                .iter()
                .filter(|s| crate::matrix::manifest_baselines(*s))
                .fold(ScopeSet::NONE, ScopeSet::with),
            None => ScopeSet::NONE,
        }
    }

    /// The connection's effective scopes at `now`: manifest baseline
    /// plus scopes of its accepted live tokens (the `capabilities`
    /// answer).
    #[must_use]
    pub fn effective_scopes(&self, client: u32, app_id: &str, now: Mono) -> ScopeSet {
        self.baseline(app_id)
            .union(self.grants.effective_scopes(app_id, client, now))
    }

    /// Handle `security.request`.
    ///
    /// `now` is injected (the broker reads no clock); `decider` is the
    /// prompt UI seam. Every outcome appends an audit record.
    pub fn escalate(
        &mut self,
        client: u32,
        app_id: &str,
        scope: Scope,
        reason: &str,
        now: Mono,
        decider: &mut dyn PromptDecider,
    ) -> EscalationOutcome {
        let manifest_hash = self.manifests.get(app_id).map(AppManifest::hash_hex);
        // 1. Manifest gate. Prompt-only scopes skip it.
        let needs_manifest = !matches!(scope, Scope::AuditRead);
        if needs_manifest {
            let listed = self
                .manifests
                .get(app_id)
                .is_some_and(|m| m.requested.contains(scope));
            if !listed {
                return self.denied(client, app_id, scope, now, DeniedReason::ManifestMissing);
            }
        }
        // 2. Rate limit (per app).
        if !self.allow_rate(app_id, now) {
            return self.denied(client, app_id, scope, now, DeniedReason::RateLimited);
        }
        // 3. Prompt for the sensitive scopes.
        if scope.requires_prompt() {
            match decider.decide(app_id, scope, reason) {
                PromptAnswer::Allow => {}
                PromptAnswer::Deny => {
                    return self.denied(client, app_id, scope, now, DeniedReason::UserDenied)
                }
            }
        }
        // 4. Mint.
        let expires = if self.config.prompt_ttl_ms == 0 || !scope.requires_prompt() {
            // Manifest-class grants ride the session (the baseline
            // already lasted to here); prompted grants are TTL'd.
            None
        } else {
            Some(now.saturating_add_ns(self.config.prompt_ttl_ms * 1_000_000))
        };
        let words = self
            .grants
            .mint(self.seed.as_mut(), app_id, ScopeSet::single(scope), expires);
        let detail = format!(
            "{{\"manifest\":{},\"prompt\":{}}}",
            quote(&manifest_hash.clone().unwrap_or_default()),
            if scope.requires_prompt() {
                "\"granted\""
            } else {
                "\"not-required\""
            }
        );
        self.audit.append(AuditEvent {
            ts_ns: now.as_ns(),
            client,
            app_id: app_id.to_owned(),
            scope: Some(scope),
            action: AuditAction::Grant,
            detail,
        });
        EscalationOutcome::Granted { words, expires }
    }

    /// Handle `security.submit_token`.
    ///
    /// Invalid and expired submissions are audited denials (the spec
    /// pins this); a granted submission joins the effective set silently
    /// (the grant was already audited at mint time).
    pub fn submit(
        &mut self,
        client: u32,
        app_id: &str,
        words: [u32; 8],
        now: Mono,
    ) -> SubmitOutcome {
        let outcome = self.grants.submit(app_id, client, words, now);
        if let SubmitOutcome::Denied { reason, scope } = &outcome {
            self.audit.append(AuditEvent {
                ts_ns: now.as_ns(),
                client,
                app_id: app_id.to_owned(),
                scope: *scope,
                action: AuditAction::Deny,
                detail: format!("{{\"reason\":\"{}\"}}", reason.wire_label()),
            });
        }
        outcome
    }

    /// Revoke a scope for an app (settings UI / broker policy). Returns
    /// whether a live grant existed; audited either way.
    pub fn revoke(&mut self, client: u32, app_id: &str, scope: Scope, now: Mono) -> bool {
        let live = self.grants.revoke_scope(app_id, scope, now);
        self.audit.append(AuditEvent {
            ts_ns: now.as_ns(),
            client,
            app_id: app_id.to_owned(),
            scope: Some(scope),
            action: AuditAction::Revoke,
            detail: format!("{{\"live\":{live}}}"),
        });
        live
    }

    /// Record a capture (screenshot / record frame) — the enforcement
    /// side must audit every capture with window attribution.
    pub fn record_capture(
        &mut self,
        client: u32,
        app_id: &str,
        scope: Scope,
        detail: &str,
        now: Mono,
    ) {
        self.audit.append(AuditEvent {
            ts_ns: now.as_ns(),
            client,
            app_id: app_id.to_owned(),
            scope: Some(scope),
            action: AuditAction::Capture,
            detail: detail.to_owned(),
        });
    }

    /// Record an input-injection batch.
    pub fn record_inject(&mut self, client: u32, app_id: &str, count: u32, now: Mono) {
        self.audit.append(AuditEvent {
            ts_ns: now.as_ns(),
            client,
            app_id: app_id.to_owned(),
            scope: Some(Scope::InputInject),
            action: AuditAction::Inject,
            detail: format!("{{\"events\":{count}}}"),
        });
    }

    /// Record bridge ambient-authority usage (foreign client attribution).
    pub fn record_bridge(&mut self, client: u32, app_id: &str, detail: &str, now: Mono) {
        self.audit.append(AuditEvent {
            ts_ns: now.as_ns(),
            client,
            app_id: app_id.to_owned(),
            scope: Some(Scope::Bridge),
            action: AuditAction::Bridge,
            detail: detail.to_owned(),
        });
    }

    /// The audit chain (subscribe / verify / snapshot).
    pub fn audit(&self) -> &AuditChain {
        &self.audit
    }

    /// The grant table (diagnostics).
    pub fn grants(&self) -> &GrantTable {
        &self.grants
    }

    /// The manifest registry (diagnostics).
    pub fn manifest(&self, app_id: &str) -> Option<&AppManifest> {
        self.manifests.get(app_id)
    }

    /// Drop one app entirely (uninstall / logout): grants and manifest
    /// go together. Returns the scopes that were live.
    #[must_use]
    pub fn purge_app(&mut self, app_id: &str) -> ScopeSet {
        self.manifests.remove(app_id);
        self.rate.remove(app_id);
        self.grants.purge_app(app_id)
    }

    fn denied(
        &mut self,
        client: u32,
        app_id: &str,
        scope: Scope,
        now: Mono,
        reason: DeniedReason,
    ) -> EscalationOutcome {
        self.audit.append(AuditEvent {
            ts_ns: now.as_ns(),
            client,
            app_id: app_id.to_owned(),
            scope: Some(scope),
            action: AuditAction::Deny,
            detail: format!("{{\"reason\":\"{}\"}}", reason.wire_label()),
        });
        EscalationOutcome::Denied(reason)
    }

    fn allow_rate(&mut self, app_id: &str, now: Mono) -> bool {
        let window = Mono::from_ms(self.config.rate_window_ms);
        let q = self.rate.entry(app_id.to_owned()).or_default();
        while let Some(front) = q.front() {
            if now.duration_since(*front) > window.as_ns() {
                q.pop_front();
            } else {
                break;
            }
        }
        if q.len() >= self.config.rate_max {
            false
        } else {
            q.push_back(now);
            true
        }
    }
}

impl DeniedReason {
    /// Machine-stable label for audit detail.
    #[must_use]
    pub const fn wire_label(self) -> &'static str {
        match self {
            Self::ManifestMissing => "manifest_missing",
            Self::UserDenied => "user_denied",
            Self::Expired => "expired",
            Self::InvalidToken => "invalid_token",
            Self::RateLimited => "rate_limited",
        }
    }
}

/// JSON string literal (minimal escaping for hex hashes).
fn quote(s: &str) -> String {
    format!("\"{s}\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grant::LcgSeed;

    fn broker() -> SessionBroker {
        SessionBroker::new(BrokerConfig::default(), Box::new(LcgSeed::new(42)))
    }

    fn now(ms: u64) -> Mono {
        Mono::from_ms(ms)
    }

    #[test]
    fn manifest_gate_denies_unlisted_scopes() {
        let mut b = broker();
        b.register_manifest(AppManifest {
            app_id: "org.a".to_owned(),
            version: 1,
            requested: ScopeSet::single(Scope::ClipboardRead),
            sandbox: ldp_core::caps::SandboxFlavor::Sandboxed,
        })
        .unwrap();
        let mut d = ScriptedDecider::new(&[PromptAnswer::Allow]);
        assert_eq!(
            b.escalate(1, "org.a", Scope::Screenshot, "need", now(0), &mut d),
            EscalationOutcome::Denied(DeniedReason::ManifestMissing)
        );
        assert_eq!(d.pending(), 1, "no prompt shown for manifest-missing");
    }

    #[test]
    fn prompted_grant_flow_mints_and_audits() {
        let mut b = broker();
        b.register_manifest(AppManifest {
            app_id: "org.a".to_owned(),
            version: 1,
            requested: ScopeSet::single(Scope::Screenshot),
            sandbox: ldp_core::caps::SandboxFlavor::Sandboxed,
        })
        .unwrap();
        let mut d = ScriptedDecider::new(&[PromptAnswer::Allow, PromptAnswer::Deny]);
        let EscalationOutcome::Granted { words, expires } =
            b.escalate(1, "org.a", Scope::Screenshot, "shot", now(0), &mut d)
        else {
            panic!("expected grant");
        };
        assert!(expires.is_some());
        assert!(matches!(
            b.submit(1, "org.a", words, now(1)),
            SubmitOutcome::Granted(_)
        ));
        assert_eq!(
            b.escalate(1, "org.a", Scope::Screenshot, "again", now(2), &mut d),
            EscalationOutcome::Denied(DeniedReason::UserDenied)
        );
        let records = b.audit().records();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].event.action, AuditAction::Grant);
        assert_eq!(records[1].event.action, AuditAction::Deny);
        assert!(records[1].event.detail.contains("user_denied"));
        assert!(b.audit().verify().ok);
    }

    #[test]
    fn rate_limit_kicks_in_and_slides() {
        let mut b = broker();
        b.register_manifest(AppManifest {
            app_id: "org.a".to_owned(),
            version: 1,
            requested: ScopeSet::single(Scope::ClipboardRead),
            sandbox: ldp_core::caps::SandboxFlavor::Sandboxed,
        })
        .unwrap();
        let mut d = ScriptedDecider::new(&[]);
        for i in 0..3 {
            assert!(matches!(
                b.escalate(1, "org.a", Scope::ClipboardRead, "r", now(i * 1000), &mut d),
                EscalationOutcome::Granted { .. }
            ));
        }
        assert_eq!(
            b.escalate(1, "org.a", Scope::ClipboardRead, "r", now(4000), &mut d),
            EscalationOutcome::Denied(DeniedReason::RateLimited)
        );
        // Window is 30s: at t=31s the oldest request expired.
        assert!(matches!(
            b.escalate(1, "org.a", Scope::ClipboardRead, "r", now(31_000), &mut d),
            EscalationOutcome::Granted { .. }
        ));
    }

    #[test]
    fn baseline_excludes_prompt_class() {
        let mut b = broker();
        let requested = ScopeSet::single(Scope::Screenshot).with(Scope::ClipboardRead);
        b.register_manifest(AppManifest {
            app_id: "org.a".to_owned(),
            version: 1,
            requested,
            sandbox: ldp_core::caps::SandboxFlavor::Sandboxed,
        })
        .unwrap();
        let base = b.baseline("org.a");
        assert!(base.contains(Scope::ClipboardRead));
        assert!(!base.contains(Scope::Screenshot));
    }

    #[test]
    fn audit_read_prompts_without_manifest() {
        let mut b = broker();
        let mut d = ScriptedDecider::new(&[PromptAnswer::Allow]);
        assert!(matches!(
            b.escalate(
                9,
                "org.admin.tool",
                Scope::AuditRead,
                "admin",
                now(0),
                &mut d
            ),
            EscalationOutcome::Granted { .. }
        ));
        assert_eq!(
            b.effective_scopes(9, "org.admin.tool", now(1)),
            ScopeSet::NONE
        );
        // The minted token needs submission to become effective.
    }
}
