//! Permission gates for data exchange.
//!
//! The policy (architecture §16, deny-by-default):
//!
//! | Operation | Gate |
//! |---|---|
//! | *Setting* the selection / primary selection | none — any client with a valid seat serial may own the clipboard. Ownership is not a read of anyone's data. |
//! | *Receiving* clipboard / primary data (`receive` on a selection offer) | `Scope::ClipboardRead` — manifest baseline or an escalated token. |
//! | *Receiving* a DnD drop payload | none — the user's drop *is* the authorization (user-intent-driven, like a file picker grant); the receiver is the surface the user chose. |
//! | Serving a transfer (the source side) | none — the client is disclosing its own data to a peer it chose to publish to. |
//!
//! The gate consults a [`ScopeSet`] (the manifest baseline the broker
//! verified) and optionally an [`AccessToken`] grant (the escalation
//! path). Every decision returns an audit record so denials join the
//! audit trail exactly like grants (§16.4).

#![forbid(unsafe_code)]

use ldp_core::caps::{Scope, ScopeSet};
use ldp_core::token::AccessToken;

/// The operation a gate question is about.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GateOp {
    /// `receive` on a selection or primary-selection offer.
    ClipboardRead,
    /// `receive` on a drag offer (user-intent authorized).
    DropReceive,
}

impl GateOp {
    /// The scope this operation needs (`None`: no scope — always
    /// allowed).
    #[must_use]
    pub const fn required_scope(self) -> Option<Scope> {
        match self {
            GateOp::ClipboardRead => Some(Scope::ClipboardRead),
            GateOp::DropReceive => None,
        }
    }
}

/// The answer to a gate question.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum GateDecision {
    /// The operation may proceed.
    Allowed {
        /// Why: manifest baseline, escalated token, or user intent.
        basis: AllowBasis,
    },
    /// The operation is denied.
    Denied {
        /// The scope that was missing (for the audit record and the
        /// client-facing error).
        missing: Scope,
    },
}

/// Why an operation was allowed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AllowBasis {
    /// The manifest baseline carries the scope.
    Manifest,
    /// An escalated token carries the scope (the runtime grant).
    Token,
    /// No scope needed (user intent / own data).
    Unrestricted,
}

impl GateDecision {
    /// Whether this is `Allowed`.
    #[must_use]
    pub const fn is_allowed(&self) -> bool {
        matches!(self, GateDecision::Allowed { .. })
    }
}

/// One gate check (evaluated; the audit trail is built from these).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct GateRecord {
    /// The operation asked about.
    pub op: GateOp,
    /// The decision reached.
    pub decision: GateDecision,
}

/// Evaluate one gate question.
///
/// `manifest` is the client's verified baseline scope set; `token`
/// is the optional escalated grant presented with the request (the
/// dispatcher validates token authenticity *before* this policy runs;
/// here only the scope coverage matters).
#[must_use]
pub fn check(
    op: GateOp,
    manifest: ScopeSet,
    token: Option<&AccessToken>,
) -> (GateDecision, GateRecord) {
    let Some(scope) = op.required_scope() else {
        let decision = GateDecision::Allowed {
            basis: AllowBasis::Unrestricted,
        };
        return (decision.clone(), GateRecord { op, decision });
    };
    if manifest.contains(scope) {
        let decision = GateDecision::Allowed {
            basis: AllowBasis::Manifest,
        };
        return (decision.clone(), GateRecord { op, decision });
    }
    if let Some(t) = token {
        if t.scope().contains(scope) {
            let decision = GateDecision::Allowed {
                basis: AllowBasis::Token,
            };
            return (decision.clone(), GateRecord { op, decision });
        }
    }
    let decision = GateDecision::Denied { missing: scope };
    (decision.clone(), GateRecord { op, decision })
}

/// The JSONL audit line for a gate record (hash-chaining lives in
/// `ldp-server`'s audit log; this renders the data-exchange line).
#[must_use]
pub fn audit_line(client: u64, record: &GateRecord) -> String {
    let (verb, scope) = match &record.decision {
        GateDecision::Allowed { basis } => (
            match basis {
                AllowBasis::Manifest => "allow-manifest",
                AllowBasis::Token => "allow-token",
                AllowBasis::Unrestricted => "allow-unrestricted",
            },
            record.op.required_scope().map_or("none", |s| s.as_str()),
        ),
        GateDecision::Denied { missing } => ("deny", missing.as_str()),
    };
    format!(
        "{{\"client\":{client},\"op\":\"{}\",\"scope\":\"{scope}\",\"decision\":\"{verb}\"}}",
        match record.op {
            GateOp::ClipboardRead => "clipboard_read",
            GateOp::DropReceive => "drop_receive",
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token_with(scopes: ScopeSet) -> AccessToken {
        AccessToken::from_words(
            [1, 2, 3, 4, 5, 6, 7, 8],
            scopes,
            Some(ldp_core::time::Mono::from_ms(60_000)),
        )
    }

    #[test]
    fn deny_by_default_for_clipboard_read() {
        let (d, r) = check(GateOp::ClipboardRead, ScopeSet::NONE, None);
        assert!(!d.is_allowed());
        assert_eq!(
            r.decision,
            GateDecision::Denied {
                missing: Scope::ClipboardRead
            }
        );
    }

    #[test]
    fn manifest_baseline_allows() {
        let set = ScopeSet::single(Scope::ClipboardRead);
        let (d, r) = check(GateOp::ClipboardRead, set, None);
        assert!(d.is_allowed());
        assert_eq!(
            r.decision,
            GateDecision::Allowed {
                basis: AllowBasis::Manifest
            }
        );
    }

    #[test]
    fn token_escalation_allows() {
        let t = token_with(ScopeSet::single(Scope::ClipboardRead));
        let (d, _) = check(GateOp::ClipboardRead, ScopeSet::NONE, Some(&t));
        assert!(matches!(
            d,
            GateDecision::Allowed {
                basis: AllowBasis::Token
            }
        ));
        // A token with the wrong scope does not.
        let t2 = token_with(ScopeSet::single(Scope::ConfigureDisplay));
        let (d2, _) = check(GateOp::ClipboardRead, ScopeSet::NONE, Some(&t2));
        assert!(!d2.is_allowed());
    }

    #[test]
    fn drop_receive_is_user_intent_authorized() {
        let (d, _) = check(GateOp::DropReceive, ScopeSet::NONE, None);
        assert!(d.is_allowed());
        assert_eq!(GateOp::DropReceive.required_scope(), None);
    }

    #[test]
    fn audit_lines_render() {
        let (_, denied) = check(GateOp::ClipboardRead, ScopeSet::NONE, None);
        let line = audit_line(4, &denied);
        assert_eq!(
            line,
            "{\"client\":4,\"op\":\"clipboard_read\",\"scope\":\"clipboard_read\",\"decision\":\"deny\"}"
        );
        let allowed = check(
            GateOp::ClipboardRead,
            ScopeSet::single(Scope::ClipboardRead),
            None,
        )
        .1;
        assert!(audit_line(5, &allowed).contains("allow-manifest"));
        let drop = check(GateOp::DropReceive, ScopeSet::NONE, None).1;
        assert!(audit_line(6, &drop).contains("allow-unrestricted"));
        assert!(audit_line(6, &drop).contains("drop_receive"));
    }
}
