//! VT (virtual terminal) switch orchestration — the compositor-side
//! choreography around logind's device pause/resume pair.
//!
//! A switch *away* arrives as `PauseDevice`: reason `pause` wants the
//! compositor to stop scanning out, drop DRM master, and ack with
//! `PauseDeviceComplete`; reason `force` skips the ack (logind already
//! revoked the device); `gone` means the device vanished. The switch
//! *back* arrives as `ResumeDevice` with a fresh fd — the compositor
//! re-inits the GPU and resumes rendering (timeline re-anchoring is
//! the power layer's job then).

use crate::logind::{PauseReason, SessionEvent};

/// The VT choreography state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum VtState {
    /// This session's VT is active.
    Active,
    /// A pause arrived (ack pending for `pause`-reason devices).
    Pausing,
    /// Switched away.
    Inactive,
    /// A resume arrived; reinit in progress.
    Resuming,
}

/// Actions the compositor must perform for one transition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum VtAction {
    /// Stop scanning out and drop DRM master.
    ReleaseDrmMaster,
    /// Send `PauseDeviceComplete` for the device.
    AckPause {
        /// Device major.
        major: u32,
        /// Device minor.
        minor: u32,
    },
    /// Re-init the GPU with the fresh fd.
    ReacquireDevice {
        /// Device major.
        major: u32,
        /// Device minor.
        minor: u32,
        /// The fresh fd token.
        fd: u32,
    },
}

/// The VT switcher: pure event -> actions mapping with the state
/// machine enforced (an `Active` session ignores stray resumes; an
/// `Inactive` one ignores pauses).
#[derive(Clone, Debug)]
pub struct VtSwitcher {
    state: VtState,
}

impl Default for VtSwitcher {
    fn default() -> Self {
        Self::new()
    }
}

impl VtSwitcher {
    /// A switcher starting active.
    #[must_use]
    pub const fn new() -> Self {
        VtSwitcher {
            state: VtState::Active,
        }
    }

    /// The current state.
    #[must_use]
    pub const fn state(&self) -> VtState {
        self.state
    }

    /// Map one session event to zero or more actions.
    #[must_use]
    pub fn on_event(&mut self, event: &SessionEvent) -> Vec<VtAction> {
        match event {
            SessionEvent::DevicePaused {
                major,
                minor,
                reason,
                needs_ack,
            } => {
                if self.state == VtState::Inactive {
                    return Vec::new(); // stray pause while already away
                }
                self.state = VtState::Pausing;
                let mut actions = vec![VtAction::ReleaseDrmMaster];
                if *needs_ack {
                    actions.push(VtAction::AckPause {
                        major: *major,
                        minor: *minor,
                    });
                } else {
                    // force/gone: logind revoked or removed the device —
                    // no ack path exists.
                    debug_assert!(matches!(reason, PauseReason::Force | PauseReason::Gone));
                }
                actions
            }
            SessionEvent::DeviceResumed { major, minor, fd } => {
                if self.state == VtState::Active {
                    return Vec::new(); // stray resume while active
                }
                self.state = VtState::Resuming;
                vec![VtAction::ReacquireDevice {
                    major: *major,
                    minor: *minor,
                    fd: *fd,
                }]
            }
            _ => Vec::new(),
        }
    }

    /// The host reports the pause dance complete (device quiesced) —
    /// the VT is now switched away.
    pub fn switch_away_complete(&mut self) {
        if self.state == VtState::Pausing {
            self.state = VtState::Inactive;
        }
    }

    /// The host reports reinit complete — back to active.
    pub fn switch_back_complete(&mut self) {
        if self.state == VtState::Resuming {
            self.state = VtState::Active;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pause_ack_resume_cycle() {
        let mut vt = VtSwitcher::new();
        let pause = SessionEvent::DevicePaused {
            major: 226,
            minor: 0,
            reason: PauseReason::Pause,
            needs_ack: true,
        };
        assert_eq!(
            vt.on_event(&pause),
            vec![
                VtAction::ReleaseDrmMaster,
                VtAction::AckPause {
                    major: 226,
                    minor: 0
                },
            ]
        );
        assert_eq!(vt.state(), VtState::Pausing);
        vt.switch_away_complete();
        assert_eq!(vt.state(), VtState::Inactive);
        let resume = SessionEvent::DeviceResumed {
            major: 226,
            minor: 0,
            fd: 7,
        };
        assert_eq!(
            vt.on_event(&resume),
            vec![VtAction::ReacquireDevice {
                major: 226,
                minor: 0,
                fd: 7
            }]
        );
        vt.switch_back_complete();
        assert_eq!(vt.state(), VtState::Active);
    }

    #[test]
    fn force_pause_has_no_ack() {
        let mut vt = VtSwitcher::new();
        let force = SessionEvent::DevicePaused {
            major: 226,
            minor: 0,
            reason: PauseReason::Force,
            needs_ack: false,
        };
        assert_eq!(vt.on_event(&force), vec![VtAction::ReleaseDrmMaster]);
        vt.switch_away_complete();
        // Stray events while away are ignored.
        assert!(vt.on_event(&force).is_empty());
    }

    #[test]
    fn stray_resume_while_active_is_ignored() {
        let mut vt = VtSwitcher::new();
        let resume = SessionEvent::DeviceResumed {
            major: 1,
            minor: 2,
            fd: 3,
        };
        assert!(vt.on_event(&resume).is_empty());
        assert_eq!(vt.state(), VtState::Active);
        // Non-VT events never touch the state.
        assert!(vt.on_event(&SessionEvent::LockRequested).is_empty());
        assert_eq!(vt.state(), VtState::Active);
    }
}
