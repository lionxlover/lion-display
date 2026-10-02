//! Grabs: implicit (button holds) and explicit (popups), plus the
//! capability-gated keyboard grab.
//!
//! The grab question is "who receives pointer events right now", and
//! the answer has three layers (architecture §14):
//!
//! * an **explicit grab** (a popup taking the pointer) overrides
//!   everything until dismissed;
//! * an **implicit grab** (any button held) pins the pointer to the
//!   surface it pressed on — releasing the last button ends it;
//! * otherwise focus follows the hit test.
//!
//! [`GrabModel`] is the pure state machine for those layers. It never
//! decides *policy* (who may take an explicit grab is the shell's
//! call; whether the keyboard grab is allowed is the security
//! layer's — the caller checks the capability token and only then
//! calls [`GrabModel::grab_keyboard`]; the token model itself arrives
//! with the Phase 16 broker).
//!
//! Dismissal: destroying or unmapping a surface dismisses any grab it
//! holds ([`GrabModel::surface_gone`]), the honest compositor-side
//! reflex that keeps a dead popup from swallowing the pointer.

#![forbid(unsafe_code)]

use crate::focus::SurfaceKey;

/// Why an explicit grab ended.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DismissReason {
    /// The grabbing surface is gone (destroy/unmap).
    SurfaceGone,
    /// The shell dismissed it (another popup, focus change).
    ShellDismissed,
}

/// Which layer holds the pointer grab.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GrabKind {
    /// Buttons held on the surface (ends with the last release).
    Implicit,
    /// An explicit grab (ends by dismissal).
    Explicit,
}

/// The active pointer grab.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PointerGrab {
    /// The grabbing surface.
    pub surface: SurfaceKey,
    /// Which layer.
    pub kind: GrabKind,
    /// The buttons currently held (bitmask, bit `n` = button code
    /// `n`; only the implicit layer tracks these).
    pub buttons: u32,
}

/// Grab state for one seat's pointer and keyboard.
#[derive(Clone, Debug, Default)]
pub struct GrabModel {
    pointer: Option<PointerGrab>,
    keyboard: Option<SurfaceKey>,
}

impl GrabModel {
    /// No grabs held.
    #[must_use]
    pub fn new() -> GrabModel {
        GrabModel::default()
    }

    /// The active pointer grab.
    #[must_use]
    pub fn pointer(&self) -> Option<&PointerGrab> {
        self.pointer.as_ref()
    }

    /// The active keyboard grab.
    #[must_use]
    pub fn keyboard(&self) -> Option<SurfaceKey> {
        self.keyboard
    }

    /// A button pressed on `target`: begins an implicit grab (or
    /// extends the held-button set of the running one).
    pub fn pointer_press(&mut self, button: u32, target: SurfaceKey) {
        match &mut self.pointer {
            Some(g) if g.kind == GrabKind::Implicit => g.buttons |= 1 << (button % 32),
            Some(_) => {
                // An explicit grab already owns the pointer; the press
                // belongs to the grabber and does not start anything.
            }
            None => {
                self.pointer = Some(PointerGrab {
                    surface: target,
                    kind: GrabKind::Implicit,
                    buttons: 1 << (button % 32),
                });
            }
        }
    }

    /// A button released: ends the implicit grab when no buttons
    /// remain held. Returns whether the grab ended.
    pub fn pointer_release(&mut self, button: u32) -> bool {
        if let Some(g) = &mut self.pointer {
            if g.kind == GrabKind::Implicit {
                g.buttons &= !(1 << (button % 32));
                if g.buttons == 0 {
                    self.pointer = None;
                    return true;
                }
            }
        }
        false
    }

    /// Take the pointer explicitly (popup). Replaces any running
    /// grab — the shell has already decided.
    pub fn take_pointer(&mut self, target: SurfaceKey) {
        self.pointer = Some(PointerGrab {
            surface: target,
            kind: GrabKind::Explicit,
            buttons: 0,
        });
    }

    /// Dismiss an explicit grab (`Ok(reason)` when one ended).
    ///
    /// # Errors
    /// The reason back when no explicit grab was held (the caller
    /// asked for a dismissal that had nothing to dismiss).
    pub fn dismiss_explicit(
        &mut self,
        reason: DismissReason,
    ) -> Result<DismissReason, DismissReason> {
        if matches!(self.pointer, Some(g) if g.kind == GrabKind::Explicit) {
            self.pointer = None;
            Ok(reason)
        } else {
            Err(reason)
        }
    }

    /// The effective pointer target: the grab surface when grabbed,
    /// the hit-test result otherwise.
    #[must_use]
    pub fn pointer_target(&self, hit: Option<SurfaceKey>) -> Option<SurfaceKey> {
        match &self.pointer {
            Some(g) => Some(g.surface),
            None => hit,
        }
    }

    /// Take the keyboard grab (capability-gated at the caller; the
    /// token check is the security layer's job).
    pub fn grab_keyboard(&mut self, target: SurfaceKey) {
        self.keyboard = Some(target);
    }

    /// Release the keyboard grab.
    pub fn release_keyboard(&mut self) {
        self.keyboard = None;
    }

    /// The effective keyboard target: the grab surface when grabbed,
    /// the shell focus otherwise.
    #[must_use]
    pub fn keyboard_target(&self, shell_focus: Option<SurfaceKey>) -> Option<SurfaceKey> {
        self.keyboard.or(shell_focus)
    }

    /// A surface went away: dismiss every grab it held. Returns
    /// whether anything was dismissed.
    pub fn surface_gone(&mut self, key: SurfaceKey) -> bool {
        let mut dismissed = false;
        if let Some(g) = &self.pointer {
            if g.surface == key {
                self.pointer = None;
                dismissed = true;
            }
        }
        if self.keyboard == Some(key) {
            self.keyboard = None;
            dismissed = true;
        }
        dismissed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BTN_LEFT: u32 = 0x110;
    const BTN_RIGHT: u32 = 0x111;

    #[test]
    fn implicit_grab_spans_held_buttons() {
        let mut g = GrabModel::new();
        let a = SurfaceKey::new(1);
        // Press left: implicit grab on the press surface.
        g.pointer_press(BTN_LEFT, a);
        assert_eq!(g.pointer().map(|p| p.surface), Some(a));
        assert_eq!(g.pointer().unwrap().kind, GrabKind::Implicit);
        // While held, the target is pinned regardless of hit tests.
        assert_eq!(g.pointer_target(Some(SurfaceKey::new(2))), Some(a));
        assert_eq!(g.pointer_target(None), Some(a));
        // A second button extends the hold...
        g.pointer_press(BTN_RIGHT, SurfaceKey::new(2));
        // ...but the grab target stays the original press surface.
        assert_eq!(g.pointer().map(|p| p.surface), Some(a));
        // Releasing one button keeps the grab.
        assert!(!g.pointer_release(BTN_LEFT));
        assert!(g.pointer().is_some());
        // Releasing the last button ends it.
        assert!(g.pointer_release(BTN_RIGHT));
        assert!(g.pointer().is_none());
        // And the target returns to the hit test.
        assert_eq!(
            g.pointer_target(Some(SurfaceKey::new(2))),
            Some(SurfaceKey::new(2))
        );
    }

    #[test]
    fn explicit_grab_overrides_and_dismisses() {
        let mut g = GrabModel::new();
        let a = SurfaceKey::new(1);
        let b = SurfaceKey::new(2);
        // An implicit grab is running...
        g.pointer_press(BTN_LEFT, a);
        // ...an explicit grab replaces it wholesale.
        g.take_pointer(b);
        assert_eq!(g.pointer().map(|p| p.surface), Some(b));
        assert_eq!(g.pointer().unwrap().kind, GrabKind::Explicit);
        // Button releases do not end an explicit grab.
        assert!(!g.pointer_release(BTN_LEFT));
        assert!(g.pointer().is_some());
        // Shell dismissal ends it.
        assert_eq!(
            g.dismiss_explicit(DismissReason::ShellDismissed),
            Ok(DismissReason::ShellDismissed)
        );
        assert!(g.pointer().is_none());
        // Dismissing with nothing held is the honest Err.
        assert_eq!(
            g.dismiss_explicit(DismissReason::ShellDismissed),
            Err(DismissReason::ShellDismissed)
        );
    }

    #[test]
    fn keyboard_grab_gates_at_the_caller() {
        let mut g = GrabModel::new();
        let a = SurfaceKey::new(1);
        let shell = SurfaceKey::new(2);
        // Without a grab, the shell focus routes.
        assert_eq!(g.keyboard_target(Some(shell)), Some(shell));
        // The caller (having checked the capability token) grabs.
        g.grab_keyboard(a);
        assert_eq!(g.keyboard_target(Some(shell)), Some(a));
        assert_eq!(g.keyboard_target(None), Some(a));
        g.release_keyboard();
        assert_eq!(g.keyboard_target(Some(shell)), Some(shell));
    }

    #[test]
    fn surface_gone_dismisses_its_grabs() {
        let mut g = GrabModel::new();
        let a = SurfaceKey::new(1);
        g.take_pointer(a);
        g.grab_keyboard(a);
        assert!(g.surface_gone(a));
        assert!(g.pointer().is_none());
        assert_eq!(g.keyboard(), None);
        assert!(!g.surface_gone(SurfaceKey::new(9)));
    }
}
