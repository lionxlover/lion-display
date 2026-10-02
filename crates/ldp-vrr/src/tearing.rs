//! The tearing gate: a separate opt-in, never a VRR side effect.
//!
//! Tearing (an async flip that hands the scanout pointer over mid-frame)
//! requires **all three** of:
//!
//! 1. the surface explicitly opts in by scheduling
//!    [`PresentationMode::Immediate`] ("tearing allowed, lowest
//!    latency (games)"),
//! 2. the output can async-flip (the KMS `PAGE_FLIP_ASYNC` path),
//! 3. the session permits tearing on this output.
//!
//! The decision is a three-way conjunction that **does not read VRR
//! state at all** — the independence property the Phase 15 exit
//! criterion pins: enabling adaptive sync can never produce a torn
//! frame, and opting into tearing never enables the window. The
//! scheduler marks presentations torn only through its immediate-mode
//! path ([`ldp_core::time::PresentationFlags::torn`]); adaptive and vsync
//! presentations are structurally tear-free.

use ldp_core::time::PresentationMode;

/// The inputs to the tearing gate.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TearingInputs {
    /// How the surface schedules its commits.
    pub mode: PresentationMode,
    /// Whether the output/CRTC supports async page flips.
    pub async_flip_capable: bool,
    /// Whether the session permits tearing on this output (a
    /// conservative default of `true`; the Phase 16 security broker may
    /// tighten it per-client).
    pub session_permits: bool,
}

impl Default for TearingInputs {
    fn default() -> Self {
        Self {
            mode: PresentationMode::Vsync,
            async_flip_capable: false,
            session_permits: true,
        }
    }
}

/// The gate's output.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TearingDecision {
    /// The frame may be presented torn (async flip).
    pub allowed: bool,
    /// The commit must carry the async-flip flag — the KMS seam: the
    /// embedder sets `PAGE_FLIP_ASYNC` on the atomic request for this
    /// surface's flips (and only then).
    pub requires_async_flip: bool,
    /// Why, when disallowed (audit trail mirroring the policy table).
    pub rationale: TearingRationale,
}

/// Why tearing was allowed or refused.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum TearingRationale {
    /// Opt-in honored: immediate surface + async-flip + permission.
    OptIn,
    /// The surface did not ask for immediate mode.
    NotRequested,
    /// The surface asked, but the output cannot async-flip.
    Unsupported,
    /// The surface asked and the output can, but the session forbids it.
    Forbidden,
}

/// The gate itself. Pure; VRR state is deliberately not an input.
#[must_use]
pub const fn tearing_decision(inputs: &TearingInputs) -> TearingDecision {
    if !matches!(inputs.mode, PresentationMode::Immediate) {
        return TearingDecision {
            allowed: false,
            requires_async_flip: false,
            rationale: TearingRationale::NotRequested,
        };
    }
    if !inputs.async_flip_capable {
        return TearingDecision {
            allowed: false,
            requires_async_flip: false,
            rationale: TearingRationale::Unsupported,
        };
    }
    if !inputs.session_permits {
        return TearingDecision {
            allowed: false,
            requires_async_flip: false,
            rationale: TearingRationale::Forbidden,
        };
    }
    TearingDecision {
        allowed: true,
        requires_async_flip: true,
        rationale: TearingRationale::OptIn,
    }
}

/// Whether a presentation mode is structurally tear-free under this
/// gate (every mode except immediate, which the gate may still refuse).
#[must_use]
pub const fn mode_is_tear_free(mode: PresentationMode) -> bool {
    !matches!(mode, PresentationMode::Immediate)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs(mode: PresentationMode, cap: bool, permits: bool) -> TearingInputs {
        TearingInputs {
            mode,
            async_flip_capable: cap,
            session_permits: permits,
        }
    }

    #[test]
    fn full_conjunction_required() {
        for mode in [
            PresentationMode::Vsync,
            PresentationMode::Adaptive,
            PresentationMode::Immediate,
        ] {
            for cap in [false, true] {
                for permits in [false, true] {
                    let d = tearing_decision(&inputs(mode, cap, permits));
                    assert_eq!(
                        d.allowed,
                        mode == PresentationMode::Immediate && cap && permits
                    );
                    assert_eq!(d.requires_async_flip, d.allowed);
                }
            }
        }
    }

    #[test]
    fn rationale_ladder() {
        // Refusal reasons in precedence order.
        assert_eq!(
            tearing_decision(&inputs(PresentationMode::Vsync, true, true)).rationale,
            TearingRationale::NotRequested
        );
        assert_eq!(
            tearing_decision(&inputs(PresentationMode::Adaptive, true, true)).rationale,
            TearingRationale::NotRequested
        );
        assert_eq!(
            tearing_decision(&inputs(PresentationMode::Immediate, false, true)).rationale,
            TearingRationale::Unsupported
        );
        assert_eq!(
            tearing_decision(&inputs(PresentationMode::Immediate, true, false)).rationale,
            TearingRationale::Forbidden
        );
        assert_eq!(
            tearing_decision(&inputs(PresentationMode::Immediate, true, true)).rationale,
            TearingRationale::OptIn
        );
    }

    #[test]
    fn tear_free_modes() {
        assert!(mode_is_tear_free(PresentationMode::Vsync));
        assert!(mode_is_tear_free(PresentationMode::Adaptive));
        assert!(!mode_is_tear_free(PresentationMode::Immediate));
    }

    #[test]
    fn defaults_are_conservative() {
        let d = TearingInputs::default();
        assert_eq!(d.mode, PresentationMode::Vsync);
        assert!(!d.async_flip_capable);
        assert!(d.session_permits);
        assert!(!tearing_decision(&d).allowed);
    }
}
