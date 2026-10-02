//! The semantic scene (Phase 47): the identity vocabulary that turns
//! protocol surfaces into scene objects the compositor can *reason
//! about* — not merely rectangles with pixels.
//!
//! The LionOS display architecture's core claim is that a surface is
//! more than its buffer: it optionally describes **what it
//! represents** (a semantic role), **what it may expose** (a security
//! class), and **how the machine should spend its frame budget on it**
//! (a scene profile). All three are client *claims* carried over the
//! shell protocol (`toplevel.set_semantic_role`,
//! `toplevel.set_security_class`, `toplevel.set_scene_profile`,
//! appended after the frozen v1 opcodes exactly as Phase 45's
//! `set_material` was); the server stays the authority — it validates
//! the domain, keeps its own invariants (the lock role floors the
//! security class at `Protected`; a tooltip never animates in), and
//! resolves every claim against what the machine can actually honor.
//!
//! The three enums are pure vocabulary: no I/O, no wire code, the
//! same `from_wire` convention the shell crate's enums serve
//! (the shell crate Material enum is the precedent). The
//! *enforcement* seams live where the semantics have teeth:
//!
//! * [`SecurityClass`] → the capture path (a `Protected`/`System`
//!   surface redacts out of `capture_manager.grab` frames while the
//!   display keeps showing it — the compositor controls what becomes
//!   visible, which is exactly why it is the right enforcement point),
//! * [`SceneProfile`] → the deadline frame scheduler (the per-surface
//!   frame-budget floor: the admission predicate a registration must
//!   pass before it may target the imminent vblank — the ladder the
//!   adaptive scheduler's doctrine names),
//! * [`SemanticRole`] → the transitions catalog (which kinds of
//!   surfaces animate in, and which must appear instantly — a tooltip
//!   that fades is a broken tooltip).
//!
//! # The priority ladder (documented, each rung enforced)
//!
//! The adaptive scheduler's ordering — input latency, the active
//! application, animation, video, the cursor, compositor effects,
//! background applications — is not a single knob in this codebase; it
//! is the name for mechanisms that already exist, one per rung:
//!
//! | rung | mechanism |
//! |------|-----------|
//! | input latency | the deadline contract itself (`target_vblank - submit_cost - flip_latency`), plus `PresentationMode::Immediate` for tearing-tolerant surfaces |
//! | active application | the per-surface budget floor (this phase): a `Gaming`/`Creative` surface holds admission rights the background does not |
//! | animation | the spring transitions catalog (this phase): compositor-owned motion claims repaint at pump cadence |
//! | video | the VRR window and the refresh-*opportunity* targeting (Phase 39) |
//! | the cursor | the reserved cursor plane in every output's inventory |
//! | compositor effects | the Liquid tier resolution — the quality budget the server owns |
//! | background applications | the occlusion quiescing (Phase 45): a fully occluded surface's frame requests park unanswered |
//!
//! The profile's budget is the one number the ladder adds up to: how
//! much remaining commit time a deadline must offer before the walk
//! retargets the *next* vblank. [`SceneProfile::Gaming`] floors it at
//! 1 ms — the admission a latency-critical client needs to keep
//! hitting the imminent flip; [`SceneProfile::Creative`] floors it at
//! 4 ms — stable pacing over single-frame latency, the editor's
//! doctrine; [`SceneProfile::Desktop`] adds no floor — the operator's
//! configured policy *is* the desktop doctrine, byte-identical to the
//! pre-Phase-47 scheduler.

use ldp_core::caps::Scope;

/// What a surface *is* beyond its structure — the client's identity
/// claim, the semantic half of the role the shell's constructors
/// (toplevel/popup) already carry structurally.
///
/// The claim has teeth, not vibes: tooltips and overlays are excluded
/// from the transitions catalog (they must appear and vanish
/// instantly — a menu that fades in has already failed its user), and
/// the lock role floors the surface's security class at `Protected`
/// (the lock screen is the one surface whose pixels are *never*
/// capturable, by construction).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SemanticRole {
    /// A plain window — the default claim, everything the shell's
    /// toplevel already means.
    #[default]
    Window,
    /// A transient dialog attached to the desktop's flow — the
    /// transitions catalog's `WindowOpen` member, the sheet's spirit.
    Dialog,
    /// A tooltip — instant appearance, no transition, never the
    /// pacing grid's concern.
    Tooltip,
    /// A floating overlay (a media HUD, a cheat sheet) — instant, like
    /// the tooltip, but stacked with the windows.
    Overlay,
    /// The lock surface — the security floor applies (`Protected` at
    /// minimum, the capture path redacts it whatever the client
    /// claims below that).
    Lock,
}

impl SemanticRole {
    /// From the wire value (the spec's `semantic_role` enum, 1..=5).
    /// `None` names every value the schema does not carry — the
    /// the shell crate Material from_wire convention.
    #[must_use]
    pub const fn from_wire(value: u32) -> Option<SemanticRole> {
        Some(match value {
            1 => SemanticRole::Window,
            2 => SemanticRole::Dialog,
            3 => SemanticRole::Tooltip,
            4 => SemanticRole::Overlay,
            5 => SemanticRole::Lock,
            _ => return None,
        })
    }

    /// The wire value.
    #[must_use]
    pub const fn wire(self) -> u32 {
        match self {
            SemanticRole::Window => 1,
            SemanticRole::Dialog => 2,
            SemanticRole::Tooltip => 3,
            SemanticRole::Overlay => 4,
            SemanticRole::Lock => 5,
        }
    }

    /// Whether the transitions catalog animates this role's mapping
    /// (the instant-appearance doctrine for the ephemeral roles).
    #[must_use]
    pub const fn transitions_eligible(self) -> bool {
        !matches!(self, SemanticRole::Tooltip | SemanticRole::Overlay)
    }
}

/// What a surface may expose — the security ladder the
/// security-aware compositor enforces at the one seam it owns
/// outright: what becomes *visible* (the capture and screen-share
/// paths).
///
/// `Normal` and `Private` capture (the private tier is the class the
/// broker-era screen-share negotiation will consult — classification
/// today, honest and documented); `Protected` and `System` redact out
/// of every client-visible capture frame while the display itself
/// keeps showing the content — the user sees their window, the
/// screenshot does not.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub enum SecurityClass {
    /// Ordinary content — the default; captures whole.
    #[default]
    Normal,
    /// Sensitive content — captures locally, classifies for the
    /// screen-share negotiation (the broker-era line).
    Private,
    /// Protected content — never composited into a client-visible
    /// capture frame; the display keeps showing it. The HDCP-class
    /// doctrine at the compositor's own gate.
    Protected,
    /// System content — the server chrome's class: redacts exactly
    /// like `Protected`, and reserved for surfaces the *server*
    /// dresses (the claim a client makes, the server may keep).
    System,
}

/// What the capture path does with a surface of a given class.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CaptureRule {
    /// The surface's pixels ride the capture frame.
    Capturable,
    /// The surface's rectangle paints black in the capture frame —
    /// the redaction the security ladder demands.
    Redact,
}

impl SecurityClass {
    /// From the wire value (the spec's `security_class` enum, 1..=4).
    /// `None` names every value the schema does not carry.
    #[must_use]
    pub const fn from_wire(value: u32) -> Option<SecurityClass> {
        Some(match value {
            1 => SecurityClass::Normal,
            2 => SecurityClass::Private,
            3 => SecurityClass::Protected,
            4 => SecurityClass::System,
            _ => return None,
        })
    }

    /// The wire value.
    #[must_use]
    pub const fn wire(self) -> u32 {
        match self {
            SecurityClass::Normal => 1,
            SecurityClass::Private => 2,
            SecurityClass::Protected => 3,
            SecurityClass::System => 4,
        }
    }

    /// The capture path's rule for this class.
    #[must_use]
    pub const fn capture_rule(self) -> CaptureRule {
        match self {
            SecurityClass::Normal | SecurityClass::Private => CaptureRule::Capturable,
            SecurityClass::Protected | SecurityClass::System => CaptureRule::Redact,
        }
    }

    /// The security module's scope this class names — the bridge from
    /// the per-surface ladder to the manifest vocabulary
    /// (`Scope::ProtectedSurface` is the "render into protected
    /// (HDCP-class) content" scope the security spec has carried
    /// since v1; the compositor-side class is the per-surface truth
    /// that scope gates).
    #[must_use]
    pub const fn scope(self) -> Option<Scope> {
        match self {
            SecurityClass::Protected | SecurityClass::System => Some(Scope::ProtectedSurface),
            SecurityClass::Normal | SecurityClass::Private => None,
        }
    }

    /// The floor the lock role imposes (the lock surface is never
    /// capturable, whatever a client claims beneath).
    #[must_use]
    pub const fn lock_floor(self) -> SecurityClass {
        match self {
            SecurityClass::Normal | SecurityClass::Private => SecurityClass::Protected,
            SecurityClass::Protected | SecurityClass::System => self,
        }
    }
}

/// How the machine spends its frame budget on one surface — the
/// adaptive frame scheduler's per-surface doctrine.
///
/// The profile's one enforced number is the **budget floor**: the
/// minimum remaining commit time a deadline contract must offer
/// before the walk retargets the next vblank
/// (`max(config.min_commit_lead_ns, profile.budget_ns())` at the
/// registration build). Everything else the ladder names is already
/// served by its own mechanism — see the module docs' table.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub enum SceneProfile {
    /// The desktop doctrine: no added floor — the operator's
    /// configured policy *is* the policy (byte-identical to the
    /// pre-Phase-47 scheduler for every surface that claims nothing).
    #[default]
    Desktop,
    /// The creative doctrine: a 4 ms floor — stable pacing over
    /// single-frame latency (an editor that misses is worse than an
    /// editor that is one frame late; the floor hands the client
    /// deadlines it can actually meet, which is what stability means
    /// here).
    Creative,
    /// The gaming doctrine: a 1 ms floor — the tightest admission
    /// that still guarantees a makeable deadline. Latency comes from
    /// the shallow depth and the fast escalation, not from handing a
    /// client a vblank it cannot make.
    Gaming,
}

impl SceneProfile {
    /// From the wire value (the spec's `scene_profile` enum, 1..=3).
    /// `None` names every value the schema does not carry.
    #[must_use]
    pub const fn from_wire(value: u32) -> Option<SceneProfile> {
        Some(match value {
            1 => SceneProfile::Desktop,
            2 => SceneProfile::Creative,
            3 => SceneProfile::Gaming,
            _ => return None,
        })
    }

    /// The wire value.
    #[must_use]
    pub const fn wire(self) -> u32 {
        match self {
            SceneProfile::Desktop => 1,
            SceneProfile::Creative => 2,
            SceneProfile::Gaming => 3,
        }
    }

    /// The frame-budget floor this profile adds to the admission
    /// predicate (ns). `Desktop` adds none — the operator's config is
    /// the doctrine.
    #[must_use]
    pub const fn budget_ns(self) -> u64 {
        match self {
            SceneProfile::Desktop => 0,
            SceneProfile::Creative => 4_000_000,
            SceneProfile::Gaming => 1_000_000,
        }
    }

    /// The profile's rank in the latency-vs-stability tradeoff
    /// (lower = latency-first). Introspection and report ordering;
    /// the *behavior* is [`SceneProfile::budget_ns`].
    #[must_use]
    pub const fn priority(self) -> u8 {
        match self {
            SceneProfile::Gaming => 0,
            SceneProfile::Creative => 1,
            SceneProfile::Desktop => 2,
        }
    }

    /// The one-word doctrine phrase.
    #[must_use]
    pub const fn phrase(self) -> &'static str {
        match self {
            SceneProfile::Desktop => "desktop",
            SceneProfile::Creative => "creative",
            SceneProfile::Gaming => "gaming",
        }
    }
}

/// The per-surface semantic bundle — one claim set, one map entry in
/// the server's scene (the `material_requests` pattern), dying with
/// the surface.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Semantics {
    /// The identity claim (`None`: the surface claimed nothing —
    /// the plain `Window` doctrine serves).
    pub role: Option<SemanticRole>,
    /// The security class (default `Normal`).
    pub security: SecurityClass,
    /// The scene profile (default `Desktop`).
    pub profile: SceneProfile,
}

impl Semantics {
    /// The effective security class under the server's invariants:
    /// the lock role floors at `Protected` — the one claim a client
    /// cannot talk its way below.
    #[must_use]
    pub const fn effective_security(&self) -> SecurityClass {
        match self.role {
            Some(SemanticRole::Lock) => self.security.lock_floor(),
            _ => self.security,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_roundtrips_every_value() {
        for role in [
            SemanticRole::Window,
            SemanticRole::Dialog,
            SemanticRole::Tooltip,
            SemanticRole::Overlay,
            SemanticRole::Lock,
        ] {
            assert_eq!(SemanticRole::from_wire(role.wire()), Some(role));
        }
        for sec in [
            SecurityClass::Normal,
            SecurityClass::Private,
            SecurityClass::Protected,
            SecurityClass::System,
        ] {
            assert_eq!(SecurityClass::from_wire(sec.wire()), Some(sec));
            assert_eq!(SceneProfile::from_wire(sec.wire() + 100), None);
        }
        for profile in [
            SceneProfile::Desktop,
            SceneProfile::Creative,
            SceneProfile::Gaming,
        ] {
            assert_eq!(SceneProfile::from_wire(profile.wire()), Some(profile));
        }
        // Out-of-domain: every value the schema does not carry.
        assert_eq!(SemanticRole::from_wire(0), None);
        assert_eq!(SemanticRole::from_wire(6), None);
        assert_eq!(SecurityClass::from_wire(0), None);
        assert_eq!(SecurityClass::from_wire(5), None);
        assert_eq!(SceneProfile::from_wire(0), None);
        assert_eq!(SceneProfile::from_wire(4), None);
    }

    #[test]
    fn the_security_ladder_redacts_at_protected() {
        assert_eq!(
            SecurityClass::Normal.capture_rule(),
            CaptureRule::Capturable
        );
        assert_eq!(
            SecurityClass::Private.capture_rule(),
            CaptureRule::Capturable
        );
        assert_eq!(SecurityClass::Protected.capture_rule(), CaptureRule::Redact);
        assert_eq!(SecurityClass::System.capture_rule(), CaptureRule::Redact);
        // The ladder is ordered: Normal < Private < Protected < System.
        assert!(SecurityClass::Normal < SecurityClass::Protected);
        assert!(SecurityClass::Private < SecurityClass::System);
    }

    #[test]
    fn the_scope_bridge_names_protected_only() {
        assert_eq!(
            SecurityClass::Protected.scope(),
            Some(Scope::ProtectedSurface)
        );
        assert_eq!(SecurityClass::System.scope(), Some(Scope::ProtectedSurface));
        assert_eq!(SecurityClass::Normal.scope(), None);
        assert_eq!(SecurityClass::Private.scope(), None);
    }

    #[test]
    fn the_lock_role_floors_the_security_class() {
        let claimed = Semantics {
            role: Some(SemanticRole::Lock),
            security: SecurityClass::Normal,
            profile: SceneProfile::Desktop,
        };
        assert_eq!(claimed.effective_security(), SecurityClass::Protected);
        // A lock claiming System keeps System (the floor is a floor).
        let system_lock = Semantics {
            role: Some(SemanticRole::Lock),
            security: SecurityClass::System,
            profile: SceneProfile::Desktop,
        };
        assert_eq!(system_lock.effective_security(), SecurityClass::System);
        // No lock role: the claim stands as sent.
        let plain = Semantics {
            role: None,
            security: SecurityClass::Normal,
            profile: SceneProfile::Desktop,
        };
        assert_eq!(plain.effective_security(), SecurityClass::Normal);
    }

    #[test]
    fn ephemeral_roles_never_transition() {
        assert!(SemanticRole::Window.transitions_eligible());
        assert!(SemanticRole::Dialog.transitions_eligible());
        assert!(SemanticRole::Lock.transitions_eligible());
        assert!(!SemanticRole::Tooltip.transitions_eligible());
        assert!(!SemanticRole::Overlay.transitions_eligible());
    }

    #[test]
    fn the_budget_ladder_is_ordered() {
        // Gaming floors tighter than Creative; Desktop adds nothing.
        assert_eq!(SceneProfile::Desktop.budget_ns(), 0);
        assert_eq!(SceneProfile::Gaming.budget_ns(), 1_000_000);
        assert_eq!(SceneProfile::Creative.budget_ns(), 4_000_000);
        // The priority rank inverts with the latency doctrine.
        assert!(SceneProfile::Gaming.priority() < SceneProfile::Creative.priority());
        assert!(SceneProfile::Creative.priority() < SceneProfile::Desktop.priority());
        // The ladder order (Desktop < Creative < Gaming) is the enum's.
        assert!(SceneProfile::Desktop < SceneProfile::Creative);
        assert!(SceneProfile::Creative < SceneProfile::Gaming);
    }
}
