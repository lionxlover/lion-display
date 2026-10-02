//! Security scopes and the capability model.
//!
//! The wire scope list lives in `spec/security.toml` (enum `scope`, bitset
//! `scopes`); this module is its typed Rust mirror plus the sandbox
//! context delivered in `welcome`. Enforcement happens in `ldp-server` /
//! `ldp-security`; the types here are the shared vocabulary.

use crate::bitset::Bitset128;
use core::fmt;

/// One privilege scope. Mirrors `ldp.security.scope` (wire values 1..=12,
/// frozen ABI). See `docs/threat-model.md` §3 for the permission matrix:
/// which scopes are manifest-only, which additionally require a runtime
/// escalation prompt, and which are impossible.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum Scope {
    /// Capture a single window's pixels.
    Screenshot,
    /// Capture the screen over time.
    ScreenRecord,
    /// Synthesize input events.
    InputInject,
    /// Register system-wide shortcuts.
    GlobalShortcut,
    /// Read clipboard / primary selection.
    ClipboardRead,
    /// Global input grab (beyond transient popup grabs).
    InputGrab,
    /// Display layout, modes, VRR, HDR policy.
    ConfigureDisplay,
    /// Move/focus foreign windows; space management.
    ManageWorkspaces,
    /// Drive accessibility services (screen readers).
    A11yControl,
    /// Subscribe to the audit stream.
    AuditRead,
    /// Compat bridge ambient authority.
    Bridge,
    /// Render into protected (HDCP-class) content.
    ProtectedSurface,
}

impl Scope {
    /// All scopes defined by protocol v1.
    pub const ALL: [Scope; 12] = [
        Scope::Screenshot,
        Scope::ScreenRecord,
        Scope::InputInject,
        Scope::GlobalShortcut,
        Scope::ClipboardRead,
        Scope::InputGrab,
        Scope::ConfigureDisplay,
        Scope::ManageWorkspaces,
        Scope::A11yControl,
        Scope::AuditRead,
        Scope::Bridge,
        Scope::ProtectedSurface,
    ];

    /// Wire enum value (1-based).
    #[must_use]
    pub const fn to_wire(self) -> u32 {
        match self {
            Self::Screenshot => 1,
            Self::ScreenRecord => 2,
            Self::InputInject => 3,
            Self::GlobalShortcut => 4,
            Self::ClipboardRead => 5,
            Self::InputGrab => 6,
            Self::ConfigureDisplay => 7,
            Self::ManageWorkspaces => 8,
            Self::A11yControl => 9,
            Self::AuditRead => 10,
            Self::Bridge => 11,
            Self::ProtectedSurface => 12,
        }
    }

    /// Parse a wire enum value; unknown values are `None` and must be
    /// ignored by receivers.
    #[must_use]
    pub const fn from_wire(v: u32) -> Option<Scope> {
        match v {
            1 => Some(Self::Screenshot),
            2 => Some(Self::ScreenRecord),
            3 => Some(Self::InputInject),
            4 => Some(Self::GlobalShortcut),
            5 => Some(Self::ClipboardRead),
            6 => Some(Self::InputGrab),
            7 => Some(Self::ConfigureDisplay),
            8 => Some(Self::ManageWorkspaces),
            9 => Some(Self::A11yControl),
            10 => Some(Self::AuditRead),
            11 => Some(Self::Bridge),
            12 => Some(Self::ProtectedSurface),
            _ => None,
        }
    }

    /// Bit index inside a [`ScopeSet`] (wire value minus one).
    #[must_use]
    pub const fn bit(self) -> u32 {
        self.to_wire() - 1
    }

    /// Stable kebab-case name (audit records, logs).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Screenshot => "screenshot",
            Self::ScreenRecord => "screen_record",
            Self::InputInject => "input_inject",
            Self::GlobalShortcut => "global_shortcut",
            Self::ClipboardRead => "clipboard_read",
            Self::InputGrab => "input_grab",
            Self::ConfigureDisplay => "configure_display",
            Self::ManageWorkspaces => "manage_workspaces",
            Self::A11yControl => "a11y_control",
            Self::AuditRead => "audit_read",
            Self::Bridge => "bridge",
            Self::ProtectedSurface => "protected_surface",
        }
    }

    /// Whether the layered policy requires a runtime user prompt *in
    /// addition to* a manifest grant for this scope (the "E" column of the
    /// threat-model matrix). Manifest-only scopes answer `false`.
    #[must_use]
    pub const fn requires_prompt(self) -> bool {
        matches!(
            self,
            Self::Screenshot | Self::ScreenRecord | Self::InputInject
        )
    }
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A set of scopes (wire: `ldp.security.scopes` bitset).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
#[repr(transparent)]
pub struct ScopeSet {
    bits: Bitset128,
}

impl ScopeSet {
    /// The empty (deny-everything) set — the default state of any client.
    pub const NONE: ScopeSet = ScopeSet {
        bits: Bitset128::EMPTY,
    };

    /// Set containing exactly one scope.
    #[must_use]
    pub const fn single(scope: Scope) -> ScopeSet {
        ScopeSet {
            bits: Bitset128::single(scope.bit()),
        }
    }

    /// With `scope` added.
    #[must_use]
    pub const fn with(self, scope: Scope) -> ScopeSet {
        ScopeSet {
            bits: self.bits.with(scope.bit()),
        }
    }

    /// With `scope` removed.
    #[must_use]
    pub const fn without(self, scope: Scope) -> ScopeSet {
        ScopeSet {
            bits: self.bits.without(scope.bit()),
        }
    }

    /// Whether `scope` is present.
    #[must_use]
    pub const fn contains(self, scope: Scope) -> bool {
        self.bits.test(scope.bit())
    }

    /// Whether every scope of `other` is present.
    #[must_use]
    pub const fn contains_all(self, other: ScopeSet) -> bool {
        self.bits.contains(other.bits)
    }

    /// Union.
    #[must_use]
    pub const fn union(self, other: ScopeSet) -> ScopeSet {
        ScopeSet {
            bits: self.bits.union(other.bits),
        }
    }

    /// Intersection.
    #[must_use]
    pub const fn intersect(self, other: ScopeSet) -> ScopeSet {
        ScopeSet {
            bits: self.bits.intersect(other.bits),
        }
    }

    /// Whether no scope is held.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.bits.is_empty()
    }

    /// Iterate held scopes.
    pub fn iter(self) -> impl Iterator<Item = Scope> {
        self.bits.iter_indices().filter_map(Scope::from_wire_bit)
    }

    /// Encode to the four wire words.
    #[must_use]
    pub const fn to_words(self) -> [u32; 4] {
        self.bits.to_words()
    }

    /// Decode from the four wire words; bits above the defined range are
    /// dropped (unknown scopes must be ignored, not fatal).
    #[must_use]
    pub const fn from_words(words: [u32; 4]) -> ScopeSet {
        ScopeSet {
            bits: Bitset128::from_words(words).masked_to(Scope::ALL.len() as u32),
        }
    }
}

impl fmt::Display for ScopeSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_empty() {
            return f.write_str("(none)");
        }
        let names: Vec<&str> = self.iter().map(Scope::as_str).collect();
        f.write_str(&names.join("|"))
    }
}

impl Scope {
    /// Helper used by [`ScopeSet::iter`]: bit index to scope.
    const fn from_wire_bit(bit: u32) -> Option<Scope> {
        Scope::from_wire(bit + 1)
    }
}

/// The sandbox flavor of a connecting client (wire: `ldp.core.sandbox_flavor`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum SandboxFlavor {
    /// No confinement.
    Unconfined,
    /// Classic Unix confinement (separate uid/group only).
    Classic,
    /// Portal-mediated (documented subset of ambient authority).
    Portal,
    /// Fully sandboxed (namespaces/seccomp; everything goes through grants).
    Sandboxed,
}

impl SandboxFlavor {
    /// Wire value.
    #[must_use]
    pub const fn to_wire(self) -> u32 {
        match self {
            Self::Unconfined => 1,
            Self::Classic => 2,
            Self::Portal => 3,
            Self::Sandboxed => 4,
        }
    }

    /// Parse a wire value.
    #[must_use]
    pub const fn from_wire(v: u32) -> Option<SandboxFlavor> {
        match v {
            1 => Some(Self::Unconfined),
            2 => Some(Self::Classic),
            3 => Some(Self::Portal),
            4 => Some(Self::Sandboxed),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_wire_round_trip() {
        for s in Scope::ALL {
            assert_eq!(Scope::from_wire(s.to_wire()), Some(s));
        }
        assert!(Scope::from_wire(0).is_none());
        assert!(Scope::from_wire(13).is_none());
    }

    #[test]
    fn prompt_scopes_are_the_sensitive_subset() {
        for s in Scope::ALL {
            let expected = matches!(
                s,
                Scope::Screenshot | Scope::ScreenRecord | Scope::InputInject
            );
            assert_eq!(s.requires_prompt(), expected);
        }
    }

    #[test]
    fn scope_set_algebra() {
        let set = ScopeSet::NONE
            .with(Scope::ClipboardRead)
            .with(Scope::Bridge);
        assert!(set.contains(Scope::ClipboardRead));
        assert!(set.contains(Scope::Bridge));
        assert!(!set.contains(Scope::Screenshot));
        assert!(set.contains_all(ScopeSet::single(Scope::ClipboardRead)));
        assert!(!set.contains_all(ScopeSet::single(Scope::InputInject)));
        assert!(!set.is_empty());
        assert!(ScopeSet::NONE.is_empty());

        let removed = set.without(Scope::ClipboardRead);
        assert_eq!(removed, ScopeSet::single(Scope::Bridge));

        let union = ScopeSet::single(Scope::AuditRead).union(set);
        assert!(union.contains(Scope::AuditRead) && union.contains(Scope::Bridge));
    }

    #[test]
    fn scope_set_wire_round_trip() {
        let set = ScopeSet::NONE
            .with(Scope::Screenshot)
            .with(Scope::ConfigureDisplay)
            .with(Scope::ProtectedSurface);
        let words = set.to_words();
        assert_eq!(ScopeSet::from_words(words), set);
        // Unknown high bits are dropped.
        let mut wide = words;
        wide[1] = 0xFFFF_FFFF; // bits 32..63 — beyond the 12 defined scopes
        let parsed = ScopeSet::from_words(wide);
        assert_eq!(parsed, set, "unknown scopes must be ignored");
    }

    #[test]
    fn iteration_yields_known_scopes_only() {
        let set = ScopeSet::from_words([0xFFFF_FFFF, 0, 0, 0]);
        let count = set.iter().count();
        assert_eq!(count, Scope::ALL.len());
    }

    #[test]
    fn display_forms() {
        assert_eq!(ScopeSet::NONE.to_string(), "(none)");
        let set = ScopeSet::single(Scope::Screenshot).with(Scope::ScreenRecord);
        assert_eq!(set.to_string(), "screenshot|screen_record");
    }

    #[test]
    fn sandbox_flavor_wire() {
        for v in 1..=4u32 {
            let f = SandboxFlavor::from_wire(v).unwrap();
            assert_eq!(f.to_wire(), v);
        }
        assert!(SandboxFlavor::from_wire(5).is_none());
    }
}
