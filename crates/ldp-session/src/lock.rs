//! Lock-screen orchestration — the `locked`/`unlocked` session events
//! and the focus gate.
//!
//! logind fires `Lock()`; the shell maps its lock surface; until that
//! surface is up the session is *Locking* (a deadline may force-blank
//! so a broken locker cannot leave the screen live). In `Locked`, the
//! focus gate admits only lock surfaces — no input, no focus, no
//! interactive surface for anything else. Unlock is requested *by the
//! lock surface* after authentication (an unlock request from a
//! non-lock surface is refused outright).

use ldp_core::time::Mono;

/// The lock state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum LockState {
    /// Session unlocked.
    Unlocked,
    /// Lock requested; waiting for the lock surface to map.
    Locking,
    /// Locked; only lock surfaces are interactive.
    Locked,
    /// Unlock requested; the lock surface is fading out.
    Unlocking,
}

/// Host actions for lock transitions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum LockAction {
    /// Show the lock surface (map + raise + focus).
    ShowLockSurface,
    /// The `locked` session event payload.
    Locked,
    /// Force-blank the outputs (the lock deadline expired).
    ForceBlank,
    /// The `unlocked` session event payload.
    Unlocked,
    /// An unlock attempt was refused (not from the lock surface).
    RefusedUnlock,
}

/// The lock-screen machine.
#[derive(Clone, Debug)]
pub struct LockScreen {
    state: LockState,
    /// Deadline for the lock surface to map (None = wait forever).
    deadline: Option<Mono>,
}

impl Default for LockScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl LockScreen {
    /// An unlocked machine.
    #[must_use]
    pub const fn new() -> Self {
        LockScreen {
            state: LockState::Unlocked,
            deadline: None,
        }
    }

    /// The current state.
    #[must_use]
    pub const fn state(&self) -> LockState {
        self.state
    }

    /// The lock deadline (while Locking).
    #[must_use]
    pub const fn deadline(&self) -> Option<Mono> {
        self.deadline
    }

    /// logind's `Lock()` signal (or the idle policy's lock trigger).
    /// The optional deadline bounds how long a broken locker can keep
    /// the session live before force-blank.
    pub fn lock_requested(&mut self, deadline: Option<Mono>) -> Vec<LockAction> {
        match self.state {
            LockState::Unlocked | LockState::Unlocking => {
                self.state = LockState::Locking;
                self.deadline = deadline;
                vec![LockAction::ShowLockSurface]
            }
            _ => Vec::new(), // already locking/locked: idempotent
        }
    }

    /// The shell's lock surface mapped (and is showing).
    pub fn lock_surface_mapped(&mut self) -> Vec<LockAction> {
        if self.state == LockState::Locking {
            self.state = LockState::Locked;
            self.deadline = None;
            vec![LockAction::Locked]
        } else {
            Vec::new()
        }
    }

    /// Drive the deadline: force-blank if the locker never came up.
    pub fn tick(&mut self, now: Mono) -> Vec<LockAction> {
        if self.state == LockState::Locking && self.deadline.is_some_and(|d| now > d) {
            self.deadline = None;
            vec![LockAction::ForceBlank]
        } else {
            Vec::new()
        }
    }

    /// The focus gate: in `Locked`, only lock surfaces may hold focus
    /// or receive input (the "only the lock surface is interactive"
    /// contract). Unlocking still gates (the fade must complete).
    #[must_use]
    pub fn can_focus(&self, surface_is_lock: bool) -> bool {
        match self.state {
            LockState::Unlocked | LockState::Locking => true,
            LockState::Locked | LockState::Unlocking => surface_is_lock,
        }
    }

    /// An unlock request. `from_lock_surface` must be true — the
    /// authenticated locker is the only path out (requests from any
    /// other surface are refused and auditable).
    pub fn unlock_request(&mut self, from_lock_surface: bool) -> Vec<LockAction> {
        match (self.state, from_lock_surface) {
            (LockState::Locked, true) => {
                self.state = LockState::Unlocked;
                vec![LockAction::Unlocked]
            }
            (LockState::Locked, false) => vec![LockAction::RefusedUnlock],
            _ => Vec::new(),
        }
    }

    /// logind's `Unlock()` signal (administrative unlock — the session
    /// manager already decided; the host tears the lock down).
    pub fn unlock_signal(&mut self) -> Vec<LockAction> {
        if matches!(
            self.state,
            LockState::Locked | LockState::Locking | LockState::Unlocking
        ) {
            self.state = LockState::Unlocked;
            self.deadline = None;
            vec![LockAction::Unlocked]
        } else {
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(m: u64) -> Mono {
        Mono::from_ms(m)
    }

    #[test]
    fn full_lock_cycle() {
        let mut lock = LockScreen::new();
        assert_eq!(
            lock.lock_requested(Some(ms(5000))),
            vec![LockAction::ShowLockSurface]
        );
        assert_eq!(lock.state(), LockState::Locking);
        // While Locking the surface is not up yet: the gate stays open
        // (denying everything before the locker is visible would leave
        // an input-less gap); the gate activates on `Locked`.
        assert!(lock.can_focus(false));
        // A second request while locking is idempotent.
        assert!(lock.lock_requested(None).is_empty());
        assert_eq!(lock.lock_surface_mapped(), vec![LockAction::Locked]);
        assert_eq!(lock.state(), LockState::Locked);
        // Focus gate: only lock surfaces.
        assert!(lock.can_focus(true));
        assert!(!lock.can_focus(false));
        // Only the lock surface may unlock.
        assert_eq!(lock.unlock_request(false), vec![LockAction::RefusedUnlock]);
        assert_eq!(lock.state(), LockState::Locked);
        assert_eq!(lock.unlock_request(true), vec![LockAction::Unlocked]);
        assert_eq!(lock.state(), LockState::Unlocked);
        assert!(lock.can_focus(false));
    }

    #[test]
    fn broken_locker_force_blanks() {
        let mut lock = LockScreen::new();
        lock.lock_requested(Some(ms(1000)));
        assert!(lock.tick(ms(999)).is_empty());
        assert_eq!(lock.tick(ms(1001)), vec![LockAction::ForceBlank]);
        // Still locking (the surface never came up), no repeat blank.
        assert!(lock.tick(ms(2000)).is_empty());
        // The late surface still locks.
        assert_eq!(lock.lock_surface_mapped(), vec![LockAction::Locked]);
    }

    #[test]
    fn no_deadline_waits_forever() {
        let mut lock = LockScreen::new();
        lock.lock_requested(None);
        assert!(lock.tick(ms(u64::MAX / 2)).is_empty());
        assert_eq!(lock.lock_surface_mapped(), vec![LockAction::Locked]);
    }

    #[test]
    fn administrative_unlock_signal() {
        let mut lock = LockScreen::new();
        lock.lock_requested(None);
        lock.lock_surface_mapped();
        assert_eq!(lock.unlock_signal(), vec![LockAction::Unlocked]);
        assert_eq!(lock.state(), LockState::Unlocked);
        assert!(lock.unlock_signal().is_empty());
    }
}
