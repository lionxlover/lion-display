//! The compositor-owned transitions catalog (Phase 47): the
//! system-level motion vocabulary — macOS-feel window choreography
//! that does not depend on applications being well designed.
//!
//! Every UI event that *changes what the user is looking at* — a
//! window opening, a workspace sliding, a display lighting, the
//! session locking — is a motion the compositor can own. The catalog
//! is the doctrine table: one [`TransitionSpec`] per
//! [`TransitionKind`], each naming its spring (stiffness, damping
//! character) after the macOS motion grammar — critically damped for
//! panels and state (fast, smooth, no overshoot), gently bouncy for
//! the playful moves (the dock's spirit).
//!
//! A [`Transition`] is one live instance of a catalog entry: a
//! [`Spring`] integrated at the frame clock
//! (the binary SystemDock advance pattern —
//! motion is a pure function of the timestamps it consumed, bit-
//! reproducible, never timer-driven). The v1 driver animates the
//! layer's **opacity** (the renderer's per-layer alpha, the property
//! the pipeline already speaks end to end — a translucent layer
//! composites, exactly the planes doctrine); geometry springs (the
//! slide, the genie) arrive with the render-thread line, and the
//! catalog's entries for them carry their curves already.
//!
//! # What drives a transition
//!
//! The host advances live transitions at every pump and claims their
//! surface rectangles as repaint — the dock's own vacate doctrine.
//! The settle predicate is the "stop animating, stop damaging"
//! contract: a settled transition reports its terminal value
//! *exactly* (1.0 for an open, 0.0 for a close), so the settled
//! frame is byte-identical with the never-animated one — every
//! pixel oracle the equivalence corpora pin keeps its meaning.
//!
//! # The honest v1 scope
//!
//! The catalog names all nine kinds; the scene wiring drives
//! **window-open fades** today (the mapping commit is the motion's
//! start, and the frame-callback economy — `frame` →
//! `frame_target` → `commit` — is the clock that advances it, with
//! the serve loop's poll cadence self-waking a desktop whose clients
//! all sleep). The close, workspace, app-switch, fullscreen, display
//! and lock kinds are the shell render path's named line: the curves
//! ship, the geometry drivers ride the render-thread roadmap entry.

use crate::spring::Spring;

/// One system-level UI motion.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum TransitionKind {
    /// A window entering the desktop — the open fade.
    WindowOpen,
    /// A window leaving the desktop — the close fade (the
    /// deferred-destroy line's driver).
    WindowClose,
    /// A workspace change — the space slide's fade half.
    WorkspaceChange,
    /// The app switcher's focus handoff.
    AppSwitch,
    /// Entering or leaving fullscreen.
    Fullscreen,
    /// A display lighting up (hotplug, wake).
    DisplayConnect,
    /// A display going dark (unplug, blank).
    DisplayDisconnect,
    /// The session locking — deliberately slower, the security cue.
    Lock,
    /// The session unlocking — quicker, the return to work.
    Unlock,
}

impl TransitionKind {
    /// Every kind the catalog serves, in declaration order (the
    /// catalog completeness test's iteration source).
    pub const ALL: &'static [TransitionKind] = &[
        TransitionKind::WindowOpen,
        TransitionKind::WindowClose,
        TransitionKind::WorkspaceChange,
        TransitionKind::AppSwitch,
        TransitionKind::Fullscreen,
        TransitionKind::DisplayConnect,
        TransitionKind::DisplayDisconnect,
        TransitionKind::Lock,
        TransitionKind::Unlock,
    ];
}

/// One catalog entry: the motion's curve and character.
#[derive(Clone, Copy, Debug)]
pub struct TransitionSpec {
    /// The kind this entry serves.
    pub kind: TransitionKind,
    /// The spring stiffness (urgency; higher = snappier).
    pub stiffness: f32,
    /// The damping character: `None` = critically damped (the macOS
    /// default — no overshoot), `Some(bounce)` = under-damped with
    /// `bounce` in `0.05..=1` scaling the damping below critical.
    pub bounce: Option<f32>,
    /// Whether the v1 opacity driver serves this kind (the geometry
    /// kinds ship their curves; their drivers are the named line).
    pub fade: bool,
    /// The one-line character phrase (reports and diagnostics).
    pub phrase: &'static str,
}

/// The doctrine table — one spec per kind, `const` by construction.
///
/// The curves follow the macOS motion grammar: state changes
/// (fullscreen, workspace, display, lock) critically damped; the
/// window pair damped to a hair under critical for a whisper of
/// life; the playful pair (app switch, unlock) bouncy.
pub struct TransitionCatalog;

impl TransitionCatalog {
    /// Every entry, in declaration order.
    pub const SPECS: &'static [TransitionSpec] = &[
        TransitionSpec {
            kind: TransitionKind::WindowOpen,
            stiffness: 180.0,
            bounce: None,
            fade: true,
            phrase: "the open fade, critically damped",
        },
        TransitionSpec {
            kind: TransitionKind::WindowClose,
            stiffness: 140.0,
            bounce: None,
            fade: true,
            phrase: "the close fade, patient",
        },
        TransitionSpec {
            kind: TransitionKind::WorkspaceChange,
            stiffness: 150.0,
            bounce: Some(0.75),
            fade: true,
            phrase: "the space slide's fade half, gently bouncy",
        },
        TransitionSpec {
            kind: TransitionKind::AppSwitch,
            stiffness: 200.0,
            bounce: Some(0.8),
            fade: true,
            phrase: "the focus handoff, quick with one overshoot",
        },
        TransitionSpec {
            kind: TransitionKind::Fullscreen,
            stiffness: 160.0,
            bounce: None,
            fade: true,
            phrase: "the state change, no drama",
        },
        TransitionSpec {
            kind: TransitionKind::DisplayConnect,
            stiffness: 120.0,
            bounce: None,
            fade: false,
            phrase: "the display lighting, unhurried",
        },
        TransitionSpec {
            kind: TransitionKind::DisplayDisconnect,
            stiffness: 120.0,
            bounce: None,
            fade: false,
            phrase: "the display going dark, unhurried",
        },
        TransitionSpec {
            kind: TransitionKind::Lock,
            stiffness: 90.0,
            bounce: None,
            fade: false,
            phrase: "the security cue, deliberately slow",
        },
        TransitionSpec {
            kind: TransitionKind::Unlock,
            stiffness: 140.0,
            bounce: Some(0.85),
            fade: false,
            phrase: "the return to work, quick and friendly",
        },
    ];

    /// The spec for one kind (the catalog is total).
    ///
    /// # Panics
    ///
    /// Never: the table carries one entry per
    /// [`TransitionKind::ALL`] member and the completeness test pins
    /// it.
    #[must_use]
    pub fn spec(kind: TransitionKind) -> &'static TransitionSpec {
        Self::SPECS
            .iter()
            .find(|s| s.kind == kind)
            .expect("the catalog is total over TransitionKind::ALL")
    }
}

/// One live transition: a catalog curve driving one animated scalar.
///
/// The driver owns the frame clock discipline: [`Transition::advance`]
/// consumes whole milliseconds of *driver* time (the frame loop's
/// now), integrates the spring in fixed substeps, and the curve is
/// therefore a pure function of the timestamp sequence — the same
/// sequence, the same curve, bit for bit (the spring engine's own
/// reproducibility doctrine, applied to choreography).
#[derive(Clone, Debug)]
pub struct Transition {
    kind: TransitionKind,
    spring: Spring,
    /// The direction: `true` animates 0 → 1 (an opening), `false`
    /// 1 → 0 (a closing).
    opening: bool,
    /// The driver timestamp the spring last integrated at (ms).
    last_ms: u64,
}

/// The largest dt one advance may integrate (ms) — a stalled driver
/// (a debug break, a suspended machine) resumes without teleporting
/// the motion; the spring's substep ceiling keeps the energy bounded
/// either way.
pub const ADVANCE_CAP_MS: u64 = 250;

impl Transition {
    /// Begin an *opening* transition of `kind` at `now_ms` — the
    /// animated scalar starts at 0 and springs to 1.
    #[must_use]
    pub fn open(kind: TransitionKind, now_ms: u64) -> Transition {
        Self::begin(kind, true, now_ms)
    }

    /// Begin a *closing* transition of `kind` at `now_ms` — the
    /// scalar starts at 1 and springs to 0.
    #[must_use]
    pub fn close(kind: TransitionKind, now_ms: u64) -> Transition {
        Self::begin(kind, false, now_ms)
    }

    fn begin(kind: TransitionKind, opening: bool, now_ms: u64) -> Transition {
        let spec = TransitionCatalog::spec(kind);
        let target = if opening { 1.0 } else { 0.0 };
        let start = if opening { 0.0 } else { 1.0 };
        let mut spring = match spec.bounce {
            None => Spring::critically_damped(target, spec.stiffness),
            Some(bounce) => Spring::bouncy(target, spec.stiffness, bounce),
        };
        spring.displace(start);
        Transition {
            kind,
            spring,
            opening,
            last_ms: now_ms,
        }
    }

    /// The transition's kind.
    #[must_use]
    pub const fn kind(&self) -> TransitionKind {
        self.kind
    }

    /// The animated opacity this frame: the spring's position for
    /// fading kinds, clamped to the physical domain; a *settled*
    /// transition reports its terminal value **exactly** — the
    /// settled frame is byte-identical with the never-animated one.
    #[must_use]
    pub fn opacity(&self) -> f32 {
        if self.settled() {
            return if self.opening { 1.0 } else { 0.0 };
        }
        self.spring.pos.clamp(0.0, 1.0)
    }

    /// Whether the motion has settled (the "stop damaging"
    /// predicate).
    #[must_use]
    pub fn settled(&self) -> bool {
        self.spring.settled()
    }

    /// Advance to `now_ms` (dt capped at [`ADVANCE_CAP_MS`]).
    /// Returns whether the transition is settled *after* the
    /// advance.
    pub fn advance(&mut self, now_ms: u64) -> bool {
        let dt = now_ms.saturating_sub(self.last_ms).min(ADVANCE_CAP_MS);
        self.last_ms = now_ms;
        if dt == 0 {
            return self.settled();
        }
        self.spring.step(dt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One 60 Hz frame.
    const FRAME_MS: u64 = 1000 / 60;

    #[test]
    fn the_catalog_is_total_and_ordered() {
        assert_eq!(TransitionCatalog::SPECS.len(), TransitionKind::ALL.len());
        for (spec, kind) in TransitionCatalog::SPECS.iter().zip(TransitionKind::ALL) {
            assert_eq!(spec.kind, *kind, "one entry per kind, in order");
        }
        // Every spec's stiffness is sane and every phrase is honest
        // (non-empty).
        for spec in TransitionCatalog::SPECS {
            assert!(spec.stiffness >= 1.0);
            assert!(!spec.phrase.is_empty());
        }
    }

    #[test]
    fn window_kinds_fade_and_hardware_kinds_wait_their_driver() {
        // The v1 opacity driver serves the client-visible kinds.
        assert!(TransitionCatalog::spec(TransitionKind::WindowOpen).fade);
        assert!(TransitionCatalog::spec(TransitionKind::WindowClose).fade);
        assert!(TransitionCatalog::spec(TransitionKind::WorkspaceChange).fade);
        assert!(TransitionCatalog::spec(TransitionKind::AppSwitch).fade);
        assert!(TransitionCatalog::spec(TransitionKind::Fullscreen).fade);
        // The display/lock kinds ship their curves; the drivers are
        // the render-thread line (documented, never hidden).
        assert!(!TransitionCatalog::spec(TransitionKind::DisplayConnect).fade);
        assert!(!TransitionCatalog::spec(TransitionKind::Lock).fade);
    }

    #[test]
    fn the_open_fade_starts_invisible_and_settles_exact() {
        let mut t = Transition::open(TransitionKind::WindowOpen, 0);
        assert!(!t.settled());
        assert_eq!(t.opacity(), 0.0, "the open starts fully transparent");
        // Integrate a generous wall of frames: the fade settles.
        let mut now = 0u64;
        for _ in 0..240 {
            now += FRAME_MS;
            t.advance(now);
            // The opacity never leaves the physical domain mid-flight.
            assert!((0.0..=1.0).contains(&t.opacity()));
            if t.settled() {
                break;
            }
        }
        assert!(t.settled(), "the critically damped open settles within 4s");
        assert_eq!(t.opacity(), 1.0, "settled is EXACT — the byte oracle holds");
    }

    #[test]
    fn the_close_fade_starts_visible_and_settles_to_zero() {
        let mut t = Transition::close(TransitionKind::WindowClose, 100);
        assert_eq!(t.opacity(), 1.0);
        let mut now = 100u64;
        for _ in 0..300 {
            now += FRAME_MS;
            t.advance(now);
            if t.settled() {
                break;
            }
        }
        assert!(t.settled());
        assert_eq!(t.opacity(), 0.0);
    }

    #[test]
    fn the_same_timestamp_sequence_reproduces_the_same_curve() {
        // Advance consumes absolute driver time: feed the same
        // irregular frame sequence twice, collect the curve, and
        // demand bit-for-bit equality.
        let run = || {
            let mut t = Transition::open(TransitionKind::AppSwitch, 0);
            let mut now = 0u64;
            let mut trace = Vec::new();
            for dt in [16u64, 17, 33, 16, 8, 16, 50, 16] {
                now += dt;
                t.advance(now);
                trace.push((t.opacity(), t.settled()));
            }
            trace
        };
        assert_eq!(run(), run(), "bit-for-bit reproducible");
    }

    #[test]
    fn a_stalled_driver_resumes_without_teleporting() {
        let mut t = Transition::open(TransitionKind::WindowOpen, 0);
        t.advance(16);
        let mid = t.opacity();
        // A 10-second stall (a debug break, a suspend): the cap
        // integrates 250 ms, not 10 s — the motion continues from
        // where it was, bounded.
        t.advance(10_000);
        let after = t.opacity();
        assert!(after < 1.0 || t.settled());
        assert!(
            after >= mid - f32::EPSILON,
            "a stall never rewinds the motion"
        );
        // It still settles.
        let mut now = 10_000u64;
        for _ in 0..240 {
            now += FRAME_MS;
            t.advance(now);
            if t.settled() {
                break;
            }
        }
        assert!(t.settled());
    }

    #[test]
    fn the_monotonic_open_never_diminished() {
        // A critically damped open is monotone: the opacity never
        // goes backwards (no overshoot on the way up).
        let mut t = Transition::open(TransitionKind::WindowOpen, 0);
        let mut last = 0.0f32;
        let mut now = 0u64;
        for _ in 0..240 {
            now += FRAME_MS;
            t.advance(now);
            assert!(t.opacity() >= last, "monotone open");
            last = t.opacity();
            if t.settled() {
                break;
            }
        }
        assert!(t.settled());
    }

    #[test]
    fn zero_dt_advance_is_a_settle_check_not_a_step() {
        let mut t = Transition::open(TransitionKind::Fullscreen, 500);
        t.advance(500);
        assert!(!t.settled(), "no time, no progress");
        assert_eq!(t.opacity(), 0.0);
    }
}
