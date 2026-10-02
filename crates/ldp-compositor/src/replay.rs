//! Determinism replay harness for the frame scheduler.
//!
//! A [`Recording`] captures everything that influenced one scheduler
//! session — the config, the nominal refresh, the ordered input stream,
//! and the emission stream it produced — in a self-describing,
//! checksummed binary format with no external dependencies (the byte
//! level lives in [`crate::replay_codec`]). Replaying a recording
//! re-runs the pure scheduler and verifies the outputs match
//! event-for-event; a stored artifact is therefore self-verifying
//! evidence of a timeline (`ldp-debug` in Phase 18 consumes exactly
//! this format).
//!
//! Determinism is structural, not hoped for: the scheduler never reads
//! a clock, all arithmetic is integer, per-surface iteration is
//! ordered, and the input stream carries every timestamp. The same
//! bytes in, the same events out — across processes and releases (the
//! format is versioned; a version mismatch is an explicit error).

use ldp_core::time::RefreshInterval;

use crate::sched_types::SchedEvent;
use crate::scheduler::{ConfigError, FrameScheduler};

// Re-export: the input vocabulary stays reachable at
// `ldp_compositor::replay::SchedInput`.
pub use crate::sched_types::SchedInput;

/// produced.
#[derive(Clone, PartialEq, Debug)]
pub struct Recording {
    /// The policy in force during the session.
    pub config: crate::scheduler::SchedulerConfig,
    /// The output's advertised refresh (ns).
    pub nominal_ns: u64,
    /// The ordered input stream.
    pub inputs: Vec<SchedInput>,
    /// The emission stream the session produced.
    pub outputs: Vec<SchedEvent>,
}

/// Why a recording could not be decoded or replayed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ReplayError {
    /// Not a recording (bad magic).
    BadMagic,
    /// Unsupported format version.
    BadVersion,
    /// Bytes ran out mid-field.
    Truncated,
    /// Trailing garbage after the last field.
    Trailing,
    /// Checksum mismatch: the artifact is corrupt.
    Checksum,
    /// Unknown input tag.
    UnknownInputTag(u8),
    /// Unknown event tag.
    UnknownEventTag(u8),
    /// Unknown presentation mode value.
    UnknownMode(u8),
    /// Unknown drop reason value.
    UnknownReason(u8),
    /// A structurally invalid value (e.g. zero refresh).
    BadValue,
    /// Replayed outputs diverge from the recorded ones (first index).
    Divergence(usize),
}

/// concatenated emissions.
///
/// # Errors
/// [`ConfigError`] when the config is malformed.
pub fn run(
    config: crate::scheduler::SchedulerConfig,
    nominal: RefreshInterval,
    inputs: &[SchedInput],
) -> Result<Vec<SchedEvent>, ConfigError> {
    let mut scheduler = FrameScheduler::new(nominal, config)?;
    let mut outputs = Vec::new();
    for input in inputs {
        match *input {
            SchedInput::Flip { ts } => scheduler.observe_flip(ts),
            SchedInput::FrameRequest { surface, frame, ts } => {
                scheduler.frame_request(surface, frame, ts);
            }
            SchedInput::Commit { surface, ts } => scheduler.commit(surface, ts),
            SchedInput::SetVisibility {
                surface,
                hidden,
                ts,
            } => {
                scheduler.set_visibility(surface, hidden, ts);
            }
            SchedInput::SetMode { surface, mode, ts } => scheduler.set_mode(surface, mode, ts),
            SchedInput::SetProfile {
                surface,
                profile,
                ts,
            } => scheduler.set_profile(surface, profile, ts),
            SchedInput::Park { ts } => scheduler.park(ts),
            SchedInput::Resume { ts } => scheduler.resume(ts),
        }
        outputs.append(&mut scheduler.drain());
    }
    Ok(outputs)
}

/// Record a session: run it and capture inputs + outputs together.
///
/// # Errors
/// [`ConfigError`] when the config is malformed.
pub fn record(
    config: crate::scheduler::SchedulerConfig,
    nominal: RefreshInterval,
    inputs: Vec<SchedInput>,
) -> Result<Recording, ConfigError> {
    let outputs = run(config, nominal, &inputs)?;
    Ok(Recording {
        config,
        nominal_ns: nominal.as_ns(),
        inputs,
        outputs,
    })
}

/// Replay a decoded recording and verify it reproduces the recorded
/// outputs exactly.
///
/// # Errors
/// [`ReplayError::Divergence`] with the first differing index, or a
/// decode-time error variant.
pub fn replay(recording: &Recording) -> Result<Vec<SchedEvent>, ReplayError> {
    let nominal = RefreshInterval::from_ns(recording.nominal_ns).ok_or(ReplayError::BadValue)?;
    let replayed =
        run(recording.config, nominal, &recording.inputs).map_err(|_| ReplayError::BadValue)?;
    if replayed == recording.outputs {
        Ok(replayed)
    } else {
        let at = replayed
            .iter()
            .zip(&recording.outputs)
            .position(|(a, b)| a != b)
            .unwrap_or(replayed.len().min(recording.outputs.len()));
        Err(ReplayError::Divergence(at))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sched_types::SchedInput;
    use crate::scheduler::SchedulerConfig;
    use crate::surface::SurfaceId;
    use ldp_core::time::FrameDropReason;
    use ldp_core::time::Mono;

    const SIXTY: u64 = 16_666_666;

    fn nominal() -> RefreshInterval {
        RefreshInterval::from_ns(SIXTY).unwrap()
    }

    fn sample_inputs() -> Vec<SchedInput> {
        vec![
            SchedInput::FrameRequest {
                surface: SurfaceId::from_raw(1),
                frame: 1,
                ts: Mono::from_ns(1_000),
            },
            SchedInput::Commit {
                surface: SurfaceId::from_raw(1),
                ts: Mono::from_ns(2_000),
            },
            SchedInput::Flip {
                ts: Mono::from_ns(SIXTY),
            },
            SchedInput::FrameRequest {
                surface: SurfaceId::from_raw(1),
                frame: 2,
                ts: Mono::from_ns(SIXTY + 1_000),
            },
            SchedInput::SetVisibility {
                surface: SurfaceId::from_raw(1),
                hidden: true,
                ts: Mono::from_ns(SIXTY + 2_000),
            },
            SchedInput::Commit {
                surface: SurfaceId::from_raw(1),
                ts: Mono::from_ns(SIXTY + 3_000),
            },
            SchedInput::SetVisibility {
                surface: SurfaceId::from_raw(1),
                hidden: false,
                ts: Mono::from_ns(SIXTY + 4_000),
            },
            SchedInput::Flip {
                ts: Mono::from_ns(2 * SIXTY),
            },
            SchedInput::Park {
                ts: Mono::from_ns(2 * SIXTY + 1_000),
            },
            SchedInput::Resume {
                ts: Mono::from_ns(2 * SIXTY + 2_000),
            },
        ]
    }

    #[test]
    fn replay_reproduces_outputs() {
        let recording = record(SchedulerConfig::default(), nominal(), sample_inputs()).unwrap();
        let outputs = replay(&recording).unwrap();
        assert_eq!(outputs, recording.outputs);
        assert!(!outputs.is_empty());
    }
    #[test]
    fn run_twice_is_identical() {
        let a = run(SchedulerConfig::default(), nominal(), &sample_inputs()).unwrap();
        let b = run(SchedulerConfig::default(), nominal(), &sample_inputs()).unwrap();
        assert_eq!(a, b);
    }
    #[test]
    fn tampered_outputs_diverge() {
        let mut recording = record(SchedulerConfig::default(), nominal(), sample_inputs()).unwrap();
        assert!(!recording.outputs.is_empty());
        // Tamper with the recorded output stream; replay must flag the
        // first differing index.
        recording.outputs[0] = SchedEvent::FrameDropped {
            surface: SurfaceId::from_raw(9),
            frame: 99,
            reason: FrameDropReason::Superseded,
        };
        assert_eq!(replay(&recording).err(), Some(ReplayError::Divergence(0)));
    }
}
