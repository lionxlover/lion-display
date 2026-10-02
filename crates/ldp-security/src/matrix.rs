//! The permission matrix — the threat-model §3 table as executable policy.
//!
//! Every row of `docs/threat-model.md` §3 is one [`Operation`]; the
//! requirement column is a [`Requirement`]. The three classes map onto
//! the layered policy:
//!
//! * [`Requirement::Free`] — the basic contract; no grant, no check.
//! * [`Requirement::Manifest`] — the scope must be in the connection's
//!   effective set, which the manifest baseline can provide at launch.
//! * [`Requirement::ManifestAndPrompt`] — the sensitive scopes
//!   (`screenshot`, `screen_record`, `input_inject`): the manifest must
//!   list the scope *and* the user must have answered a live prompt
//!   (the token only exists after both).
//! * [`Requirement::Prompt`] — `audit_read`: no manifest prerequisite;
//!   a runtime prompt alone grants it (administrator flow).
//! * [`Requirement::Impossible`] — not exposed on the wire at all; the
//!   dispatcher can never even name the operation.
//!
//! [`decide`] answers the enforcement question for one operation given
//! the app's manifest scopes and the connection's effective scopes.
//! Deny-by-default: unknown operations (added by future specs) deny.

use ldp_core::caps::{Scope, ScopeSet};

/// One row of the permission matrix.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Operation {
    /// Draw into surfaces the client owns.
    RenderOwnWindows,
    /// Read back pixels of buffers the client owns.
    ReadOwnBuffers,
    /// Receive keyboard/pointer events while focused.
    FocusedInput,
    /// Global input grab (beyond transient popup grabs).
    GlobalGrab,
    /// Write the clipboard / primary selection while focused.
    ClipboardWrite,
    /// Read the clipboard (paste).
    ClipboardRead,
    /// Read the primary selection.
    PrimarySelectionRead,
    /// Capture a single window's pixels.
    ScreenshotWindow,
    /// Capture the whole screen.
    ScreenshotScreen,
    /// Capture the screen over time.
    ScreenRecord,
    /// Synthesize input events.
    InputInjection,
    /// Register system-wide shortcuts.
    GlobalShortcuts,
    /// Reconfigure outputs (layout, modes, VRR, HDR policy).
    DisplayReconfig,
    /// Move/focus foreign windows, manage spaces.
    WorkspaceManage,
    /// Subscribe to the a11y event stream (screen readers).
    A11ySubscribe,
    /// Subscribe to the audit stream.
    AuditRead,
    /// Read other clients' window titles — impossible by protocol.
    ForeignTitles,
    /// Map other clients' buffers — impossible by protocol.
    ForeignBuffers,
}

/// The requirement column of one matrix row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Requirement {
    /// Free — no grant.
    Free,
    /// Manifest scope (the baseline can carry it).
    Manifest(Scope),
    /// Runtime prompt alone satisfies (no manifest prerequisite).
    Prompt(Scope),
    /// Manifest scope *and* a runtime prompt.
    ManifestAndPrompt(Scope),
    /// Impossible by protocol.
    Impossible,
}

impl Requirement {
    /// The scope this requirement gates on, if any.
    #[must_use]
    pub const fn scope(self) -> Option<Scope> {
        match self {
            Self::Free | Self::Impossible => None,
            Self::Manifest(s) | Self::Prompt(s) | Self::ManifestAndPrompt(s) => Some(s),
        }
    }
}

/// The enforcement answer for one operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// The operation may proceed.
    Allow,
    /// The operation is denied now, but an escalation request for the
    /// gated scope could still succeed (manifest is in place where the
    /// class demands one). The client should call `security.request`.
    Prompt,
    /// Denied outright (`unauthorized` + audit record).
    Deny,
}

/// The requirement row for one operation.
///
/// This is the literal transcription of the threat-model table; the
/// Phase 16 exit criterion pins every cell of it (tests/matrix.rs).
#[must_use]
pub const fn requirement(op: Operation) -> Requirement {
    use Operation as O;
    use Requirement as R;
    use Scope as S;
    match op {
        O::RenderOwnWindows | O::ReadOwnBuffers | O::FocusedInput | O::ClipboardWrite => R::Free,
        O::GlobalGrab => R::Manifest(S::InputGrab),
        O::ClipboardRead | O::PrimarySelectionRead => R::Manifest(S::ClipboardRead),
        O::ScreenshotWindow | O::ScreenshotScreen => R::ManifestAndPrompt(S::Screenshot),
        O::ScreenRecord => R::ManifestAndPrompt(S::ScreenRecord),
        O::InputInjection => R::ManifestAndPrompt(S::InputInject),
        O::GlobalShortcuts => R::Manifest(S::GlobalShortcut),
        O::DisplayReconfig => R::Manifest(S::ConfigureDisplay),
        O::WorkspaceManage => R::Manifest(S::ManageWorkspaces),
        O::A11ySubscribe => R::Manifest(S::A11yControl),
        O::AuditRead => R::Prompt(S::AuditRead),
        O::ForeignTitles | O::ForeignBuffers => R::Impossible,
    }
}

/// Decide one operation against the app's manifest scopes and the
/// connection's effective scopes (manifest baseline plus accepted tokens).
///
/// Deny-by-default: an operation this build does not know denies.
#[must_use]
pub fn decide(op: Operation, manifest: ScopeSet, effective: ScopeSet) -> Decision {
    match requirement(op) {
        Requirement::Free => Decision::Allow,
        Requirement::Impossible => Decision::Deny,
        Requirement::Manifest(s) => {
            if effective.contains(s) {
                Decision::Allow
            } else {
                Decision::Deny
            }
        }
        Requirement::Prompt(s) => {
            if effective.contains(s) {
                Decision::Allow
            } else {
                // No manifest prerequisite: the prompt is always reachable.
                Decision::Prompt
            }
        }
        Requirement::ManifestAndPrompt(s) => {
            if effective.contains(s) {
                Decision::Allow
            } else if manifest.contains(s) {
                // Manifest is in place; only the live prompt is missing.
                Decision::Prompt
            } else {
                // Escalation would fail with manifest_missing.
                Decision::Deny
            }
        }
    }
}

/// Whether a manifest alone (no prompt, no token) puts `scope` into a
/// connection's baseline: exactly the scopes whose matrix rows are all
/// manifest-class. Prompt-class (M+E) and prompt-only (E) scopes never
/// baseline — a live user decision or an admin token must carry them.
/// Authority-side scopes with no rows (`bridge`, `protected_surface`)
/// answer false: their grants ride tokens minted at launch, audited.
#[must_use]
pub fn manifest_baselines(scope: Scope) -> bool {
    let mut any = false;
    for op in ALL_OPERATIONS {
        match requirement(*op) {
            Requirement::Manifest(s) if s == scope => any = true,
            Requirement::Prompt(s) | Requirement::ManifestAndPrompt(s) if s == scope => {
                return false;
            }
            _ => {}
        }
    }
    any
}

/// Every operation, in threat-model table order (the exhaustive-walk
/// corpus).
pub const ALL_OPERATIONS: &[Operation] = &[
    Operation::RenderOwnWindows,
    Operation::ReadOwnBuffers,
    Operation::FocusedInput,
    Operation::GlobalGrab,
    Operation::ClipboardWrite,
    Operation::ClipboardRead,
    Operation::PrimarySelectionRead,
    Operation::ScreenshotWindow,
    Operation::ScreenshotScreen,
    Operation::ScreenRecord,
    Operation::InputInjection,
    Operation::GlobalShortcuts,
    Operation::DisplayReconfig,
    Operation::WorkspaceManage,
    Operation::A11ySubscribe,
    Operation::AuditRead,
    Operation::ForeignTitles,
    Operation::ForeignBuffers,
];

#[cfg(test)]
mod tests {
    use super::*;

    fn single(s: Scope) -> ScopeSet {
        ScopeSet::single(s)
    }

    fn all_scopes() -> ScopeSet {
        Scope::ALL
            .iter()
            .fold(ScopeSet::NONE, |acc, s| acc.with(*s))
    }

    #[test]
    fn free_rows_are_always_allowed() {
        let full = all_scopes();
        for op in [
            Operation::RenderOwnWindows,
            Operation::ReadOwnBuffers,
            Operation::FocusedInput,
            Operation::ClipboardWrite,
        ] {
            assert_eq!(decide(op, ScopeSet::NONE, ScopeSet::NONE), Decision::Allow);
            assert_eq!(decide(op, full, full), Decision::Allow);
        }
    }

    #[test]
    fn impossible_rows_are_always_denied() {
        let full = all_scopes();
        for op in [Operation::ForeignTitles, Operation::ForeignBuffers] {
            assert_eq!(decide(op, full, full), Decision::Deny);
            assert_eq!(requirement(op), Requirement::Impossible);
        }
    }

    #[test]
    fn manifest_rows_follow_the_effective_set() {
        let op = Operation::ClipboardRead;
        assert_eq!(decide(op, ScopeSet::NONE, ScopeSet::NONE), Decision::Deny);
        assert_eq!(
            decide(op, single(Scope::ClipboardRead), ScopeSet::NONE),
            Decision::Deny
        );
        assert_eq!(
            decide(op, ScopeSet::NONE, single(Scope::ClipboardRead)),
            Decision::Allow
        );
        // Effective without manifest (admin-issued token): still allowed.
        assert_eq!(
            decide(op, ScopeSet::NONE, single(Scope::ClipboardRead)),
            Decision::Allow
        );
    }

    #[test]
    fn manifest_and_prompt_rows_have_three_states() {
        let op = Operation::ScreenshotWindow;
        assert_eq!(decide(op, ScopeSet::NONE, ScopeSet::NONE), Decision::Deny);
        assert_eq!(
            decide(op, single(Scope::Screenshot), ScopeSet::NONE),
            Decision::Prompt
        );
        assert_eq!(
            decide(op, single(Scope::Screenshot), single(Scope::Screenshot)),
            Decision::Allow
        );
    }

    #[test]
    fn prompt_rows_have_two_states() {
        let op = Operation::AuditRead;
        assert_eq!(decide(op, ScopeSet::NONE, ScopeSet::NONE), Decision::Prompt);
        assert_eq!(
            decide(op, single(Scope::AuditRead), ScopeSet::NONE),
            Decision::Prompt
        );
        assert_eq!(
            decide(op, ScopeSet::NONE, single(Scope::AuditRead)),
            Decision::Allow
        );
    }

    #[test]
    fn requirement_scopes_cover_every_scope() {
        // Every scope except the two authority-side ones has at least one
        // matrix row: `bridge` (ambient authority exercised implicitly by
        // the bridge process, not an interactive operation clients
        // request) and `protected_surface` (gates buffer import, not an
        // operation row). The threat-model table lists no rows for them
        // either — deny-by-default covers anything unlisted.
        for scope in Scope::ALL {
            if matches!(scope, Scope::Bridge | Scope::ProtectedSurface) {
                continue;
            }
            let rows = ALL_OPERATIONS
                .iter()
                .filter(|op| requirement(**op).scope() == Some(scope))
                .count();
            assert!(rows >= 1, "scope {scope} has no matrix row");
        }
    }

    #[test]
    fn manifest_and_prompt_class_matches_prompt_doctrine() {
        // The scopes gated behind ManifestAndPrompt rows are exactly the
        // scopes ldp-core flags with requires_prompt() — the E column of
        // the threat-model matrix.
        for op in ALL_OPERATIONS {
            if let Some(s) = requirement(*op).scope() {
                let gated = matches!(requirement(*op), Requirement::ManifestAndPrompt(_));
                assert_eq!(
                    gated,
                    s.requires_prompt(),
                    "ManifestAndPrompt class disagrees with requires_prompt for {op:?}"
                );
            }
        }
    }
}
