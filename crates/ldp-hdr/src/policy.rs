//! Output-mode policy: SDR / PQ / HLG selection with hysteresis.
//!
//! The decision is a **pure function** of the output's capabilities
//! and the current surface stack ([`decide`]) — the
//! documented table:
//!
//! | Output caps | Stack | Mode |
//! |---|---|---|
//! | no HDR | anything | SDR (HDR content is tone-mapped by the renderer) |
//! | PQ+HLG | any PQ surface | PQ (the interchange format wins) |
//! | HLG only | PQ or HLG surface | HLG |
//! | HDR-capable | all-SDR stack | SDR |
//!
//! Mode *switching* is not pure: [`ModeController`] adds dwell-based
//! hysteresis — a new target mode must hold for `dwell` consecutive
//! evaluations before the output actually switches, so a lone HDR
//! popup (or a HDR surface closing) cannot flap the panel's mode frame
//! to frame. The controller is deterministic: identical input
//! sequences produce identical mode sequences.
//!
//! Mixed stacks: PQ content present wins over HLG (PQ is the
//! interchange EOTF; HLG content converts losslessly to PQ, not the
//! reverse — documented in architecture §12).

use ldp_core::color::Luminance;

/// The output's color/HDR mode.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OutputMode {
    /// Classic SDR (HDR content, if any, is tone-mapped).
    Sdr,
    /// HDR with the PQ (ST 2084) EOTF.
    HdrPq,
    /// HDR with the HLG (BT.2100) EOTF.
    HdrHlg,
}

/// The output's advertised capabilities.
///
/// Boolean by nature — a capability set (the same shape the wire's
/// `color_caps` bitset carries, in policy form).
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct OutputCaps {
    /// Any HDR support at all (PQ or HLG).
    pub hdr: bool,
    /// PQ (ST 2084) support.
    pub pq: bool,
    /// HLG support.
    pub hlg: bool,
    /// The panel's peak luminance.
    pub max_luminance: Luminance,
    /// Wide-gamut (BT.2020-class) reproduction.
    pub wide_gamut: bool,
}

impl OutputCaps {
    /// A classic SDR-only output.
    #[must_use]
    pub const fn sdr_only() -> Self {
        Self {
            hdr: false,
            pq: false,
            hlg: false,
            max_luminance: Luminance::from_nits(300),
            wide_gamut: false,
        }
    }

    /// A typical HDR display (PQ + HLG, 600-nit peak, wide gamut).
    #[must_use]
    pub const fn hdr_display() -> Self {
        Self {
            hdr: true,
            pq: true,
            hlg: true,
            max_luminance: Luminance::from_nits(600),
            wide_gamut: true,
        }
    }
}

/// What the current surface stack contains.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct StackSummary {
    /// Any PQ (ST 2084) surface.
    pub any_pq: bool,
    /// Any HLG surface.
    pub any_hlg: bool,
    /// The stack's maximum content luminance (0 = unknown/SDR).
    pub max_content_luminance: Luminance,
    /// Any wide-gamut surface.
    pub any_wide_gamut: bool,
}

impl StackSummary {
    /// An all-SDR stack.
    #[must_use]
    pub const fn all_sdr() -> Self {
        Self {
            any_pq: false,
            any_hlg: false,
            max_content_luminance: Luminance::from_nits(0),
            any_wide_gamut: false,
        }
    }
}

/// The pure decision table (see the module docs).
#[must_use]
pub fn decide(caps: &OutputCaps, stack: &StackSummary) -> OutputMode {
    if !caps.hdr {
        return OutputMode::Sdr;
    }
    if stack.any_pq {
        if caps.pq {
            OutputMode::HdrPq
        } else if caps.hlg {
            OutputMode::HdrHlg
        } else {
            OutputMode::Sdr
        }
    } else if stack.any_hlg {
        if caps.hlg {
            OutputMode::HdrHlg
        } else if caps.pq {
            OutputMode::HdrPq
        } else {
            OutputMode::Sdr
        }
    } else {
        OutputMode::Sdr
    }
}

/// The stateful mode controller with dwell hysteresis.
#[derive(Clone, Copy, Debug)]
pub struct ModeController {
    current: OutputMode,
    target: OutputMode,
    pending_for: u32,
    dwell: u32,
}

impl ModeController {
    /// A controller starting in `initial`, requiring `dwell`
    /// consecutive evaluations before switching (`dwell = 0` switches
    /// immediately — pure mode following).
    #[must_use]
    pub const fn new(initial: OutputMode, dwell: u32) -> Self {
        Self {
            current: initial,
            target: initial,
            pending_for: 0,
            dwell,
        }
    }

    /// The currently active mode.
    #[must_use]
    pub const fn mode(&self) -> OutputMode {
        self.current
    }

    /// Feed one evaluation; returns the active mode after it.
    pub fn step(&mut self, caps: &OutputCaps, stack: &StackSummary) -> OutputMode {
        let want = decide(caps, stack);
        if want == self.current {
            // Agreement resets any pending switch.
            self.pending_for = 0;
            self.target = self.current;
            return self.current;
        }
        if want == self.target {
            self.pending_for = self.pending_for.saturating_add(1);
        } else {
            // A new disagreement starts counting fresh.
            self.target = want;
            self.pending_for = 1;
        }
        if self.pending_for > self.dwell {
            self.current = self.target;
            self.pending_for = 0;
        }
        self.current
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hdr_stack() -> StackSummary {
        StackSummary {
            any_pq: true,
            any_hlg: false,
            max_content_luminance: Luminance::from_nits(1000),
            any_wide_gamut: true,
        }
    }

    #[test]
    fn decision_table_is_exhaustively_pinned() {
        // Every combination of the input predicates.
        let caps_set = [
            OutputCaps::sdr_only(),
            OutputCaps::hdr_display(),
            OutputCaps {
                hdr: true,
                pq: false,
                hlg: true,
                max_luminance: Luminance::from_nits(1000),
                wide_gamut: true,
            },
            OutputCaps {
                hdr: true,
                pq: true,
                hlg: false,
                max_luminance: Luminance::from_nits(400),
                wide_gamut: false,
            },
            OutputCaps {
                hdr: true,
                pq: false,
                hlg: false,
                max_luminance: Luminance::from_nits(600),
                wide_gamut: true,
            },
        ];
        let stacks = [
            StackSummary::all_sdr(),
            hdr_stack(),
            StackSummary {
                any_pq: false,
                any_hlg: true,
                max_content_luminance: Luminance::from_nits(1000),
                any_wide_gamut: false,
            },
            StackSummary {
                any_pq: true,
                any_hlg: true,
                max_content_luminance: Luminance::from_nits(4000),
                any_wide_gamut: true,
            },
        ];
        for caps in &caps_set {
            for stack in &stacks {
                let mode = decide(caps, stack);
                let expected = if !caps.hdr {
                    OutputMode::Sdr
                } else if stack.any_pq {
                    if caps.pq {
                        OutputMode::HdrPq
                    } else if caps.hlg {
                        OutputMode::HdrHlg
                    } else {
                        OutputMode::Sdr
                    }
                } else if stack.any_hlg {
                    if caps.hlg {
                        OutputMode::HdrHlg
                    } else if caps.pq {
                        OutputMode::HdrPq
                    } else {
                        OutputMode::Sdr
                    }
                } else {
                    OutputMode::Sdr
                };
                assert_eq!(mode, expected, "decide({caps:?}, {stack:?})");
                // Pure: same inputs, same answer.
                assert_eq!(decide(caps, stack), mode);
            }
        }
    }

    #[test]
    fn pq_wins_over_hlg_on_full_caps() {
        let caps = OutputCaps::hdr_display();
        let mixed = StackSummary {
            any_pq: true,
            any_hlg: true,
            max_content_luminance: Luminance::from_nits(1000),
            any_wide_gamut: true,
        };
        assert_eq!(decide(&caps, &mixed), OutputMode::HdrPq);
    }

    #[test]
    fn dwell_hysteresis_prevents_flapping() {
        // Alternating stacks must not flip the mode more than once per
        // dwell window — with dwell 3, alternating input leaves the
        // mode constant forever.
        let caps = OutputCaps::hdr_display();
        let mut ctl = ModeController::new(OutputMode::Sdr, 3);
        for i in 0..40 {
            let stack = if i % 2 == 0 {
                StackSummary::all_sdr()
            } else {
                hdr_stack()
            };
            assert_eq!(ctl.step(&caps, &stack), OutputMode::Sdr, "step {i}");
        }
        // A sustained HDR stack switches after dwell+1 evaluations.
        let mut ctl = ModeController::new(OutputMode::Sdr, 2);
        let stack = hdr_stack();
        assert_eq!(ctl.step(&caps, &stack), OutputMode::Sdr);
        assert_eq!(ctl.step(&caps, &stack), OutputMode::Sdr);
        assert_eq!(ctl.step(&caps, &stack), OutputMode::HdrPq);
        assert_eq!(ctl.step(&caps, &stack), OutputMode::HdrPq);
        // A single SDR blip does not switch back...
        assert_eq!(ctl.step(&caps, &StackSummary::all_sdr()), OutputMode::HdrPq);
        assert_eq!(ctl.step(&caps, &stack), OutputMode::HdrPq);
        // ...but a sustained SDR stack does.
        assert_eq!(ctl.step(&caps, &StackSummary::all_sdr()), OutputMode::HdrPq);
        assert_eq!(ctl.step(&caps, &StackSummary::all_sdr()), OutputMode::HdrPq);
        assert_eq!(ctl.step(&caps, &StackSummary::all_sdr()), OutputMode::Sdr);
    }

    #[test]
    fn zero_dwell_follows_immediately() {
        let caps = OutputCaps::hdr_display();
        let mut ctl = ModeController::new(OutputMode::Sdr, 0);
        assert_eq!(ctl.step(&caps, &hdr_stack()), OutputMode::HdrPq);
        assert_eq!(ctl.step(&caps, &StackSummary::all_sdr()), OutputMode::Sdr);
    }
}
