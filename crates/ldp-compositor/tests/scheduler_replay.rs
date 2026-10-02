//! Phase 7 exit criterion: determinism replay.
//!
//! Every property scenario doubles as a recording: the input stream and
//! its outputs serialize to the self-describing checksummed format,
//! decode losslessly, and replay to the identical event vector. Same
//! seed, same bytes, same decisions — across runs and formats.

#[path = "sched/mod.rs"]
mod sched;

use ldp_compositor::replay::{record, replay, Recording, ReplayError, SchedInput};
use ldp_compositor::scheduler::{SchedEvent, SchedulerConfig};
use ldp_compositor::SurfaceId;
use ldp_core::time::{Mono, RefreshInterval};
use sched::{
    scenario_adaptive, scenario_adaptive_vsync_control, scenario_drift_lock, scenario_immediate,
    scenario_multi_client, scenario_nominal_jitter, scenario_slow_client, scenario_stall_resync,
    Scenario, SIXTY,
};

fn scenarios() -> Vec<(&'static str, Scenario)> {
    vec![
        ("nominal_jitter", scenario_nominal_jitter(1)),
        ("drift_lock", scenario_drift_lock(1)),
        ("slow_client", scenario_slow_client(1)),
        ("stall_resync", scenario_stall_resync(1)),
        ("immediate", scenario_immediate(1)),
        ("adaptive", scenario_adaptive(1)),
        ("adaptive_control", scenario_adaptive_vsync_control(1)),
        ("multi_client", scenario_multi_client(1)),
    ]
}

#[test]
fn every_scenario_replays_exactly() {
    for (name, scenario) in scenarios() {
        let recording = Recording {
            config: scenario.config,
            nominal_ns: scenario.nominal_ns,
            inputs: scenario.inputs.clone(),
            outputs: scenario.outputs.clone(),
        };
        let bytes = recording.to_bytes();
        let decoded = Recording::from_bytes(&bytes)
            .unwrap_or_else(|e| panic!("{name}: decode failed: {e:?}"));
        assert_eq!(decoded, recording, "{name}: lossy decode");
        assert_eq!(decoded.to_bytes(), bytes, "{name}: non-canonical encode");
        let replayed = replay(&decoded).unwrap_or_else(|e| panic!("{name}: replay failed: {e:?}"));
        assert_eq!(replayed, scenario.outputs, "{name}: replay diverged");
    }
}

#[test]
fn same_seed_same_bytes() {
    // Determinism across full re-runs (new driver, new RNGs).
    let a = scenario_slow_client(7);
    let b = scenario_slow_client(7);
    let ra = Recording {
        config: a.config,
        nominal_ns: a.nominal_ns,
        inputs: a.inputs,
        outputs: a.outputs,
    };
    let rb = Recording {
        config: b.config,
        nominal_ns: b.nominal_ns,
        inputs: b.inputs,
        outputs: b.outputs,
    };
    assert_eq!(ra.to_bytes(), rb.to_bytes());
    // Different seeds differ.
    let c = scenario_slow_client(8);
    let rc = Recording {
        config: c.config,
        nominal_ns: c.nominal_ns,
        inputs: c.inputs,
        outputs: c.outputs,
    };
    assert_ne!(ra.to_bytes(), rc.to_bytes());
}

#[test]
fn tampered_recording_is_flagged() {
    let scenario = scenario_nominal_jitter(2);
    let mut recording = Recording {
        config: scenario.config,
        nominal_ns: scenario.nominal_ns,
        inputs: scenario.inputs,
        outputs: scenario.outputs,
    };
    // Tamper with the recorded outputs; replay flags the divergence.
    assert!(!recording.outputs.is_empty());
    if let Some(slot) = recording
        .outputs
        .iter_mut()
        .find(|e| matches!(e, SchedEvent::Presented { .. }))
    {
        *slot = SchedEvent::FrameDropped {
            surface: SurfaceId::from_raw(42),
            frame: 999,
            reason: ldp_core::time::FrameDropReason::Superseded,
        };
    }
    match replay(&recording) {
        Err(ReplayError::Divergence(at)) => assert!(at < recording.outputs.len()),
        other => panic!("expected divergence, got {other:?}"),
    }
}

#[test]
fn record_and_replay_are_interchangeable() {
    // record() and a hand-built Recording decode/replay identically.
    let scenario = scenario_drift_lock(3);
    let via_record = record(
        scenario.config,
        RefreshInterval::from_ns(scenario.nominal_ns).unwrap(),
        scenario.inputs.clone(),
    )
    .unwrap();
    let hand = Recording {
        config: scenario.config,
        nominal_ns: scenario.nominal_ns,
        inputs: scenario.inputs,
        outputs: scenario.outputs,
    };
    assert_eq!(via_record, hand);
    assert_eq!(via_record.to_bytes(), hand.to_bytes());
}

#[test]
fn recording_sizes_are_bounded() {
    // A recording is a compact artifact: roughly 3 words per input plus
    // the outputs, no hidden blowup.
    for (name, scenario) in scenarios() {
        let bytes = Recording {
            config: scenario.config,
            nominal_ns: scenario.nominal_ns,
            inputs: scenario.inputs.clone(),
            outputs: scenario.outputs.clone(),
        }
        .to_bytes();
        let bound = scenario.inputs.len() * 40 + scenario.outputs.len() * 64 + 256;
        assert!(
            bytes.len() <= bound,
            "{name}: {} bytes for {} inputs / {} outputs",
            bytes.len(),
            scenario.inputs.len(),
            scenario.outputs.len()
        );
    }
}

#[test]
fn golden_stream_replays() {
    // A tiny hand-written stream round-trips and replays byte-exactly.
    let inputs = vec![
        SchedInput::FrameRequest {
            surface: SurfaceId::from_raw(1),
            frame: 1,
            ts: Mono::from_ns(SIXTY + 1_000),
        },
        SchedInput::Flip {
            ts: Mono::from_ns(SIXTY),
        },
        SchedInput::Commit {
            surface: SurfaceId::from_raw(1),
            ts: Mono::from_ns(SIXTY + 2_000),
        },
        SchedInput::Flip {
            ts: Mono::from_ns(2 * SIXTY),
        },
    ];
    let mut sorted = inputs.clone();
    sorted.sort_by_key(|i| match *i {
        SchedInput::Flip { ts }
        | SchedInput::FrameRequest { ts, .. }
        | SchedInput::Commit { ts, .. }
        | SchedInput::SetVisibility { ts, .. }
        | SchedInput::SetMode { ts, .. }
        | SchedInput::SetProfile { ts, .. }
        | SchedInput::Park { ts }
        | SchedInput::Resume { ts } => ts.as_ns(),
    });
    let recording = record(
        SchedulerConfig::default(),
        RefreshInterval::from_ns(SIXTY).unwrap(),
        sorted,
    )
    .unwrap();
    assert_eq!(recording.outputs.len(), 2);
    let bytes = recording.to_bytes();
    let decoded = Recording::from_bytes(&bytes).unwrap();
    assert_eq!(replay(&decoded).unwrap(), recording.outputs);
    let _ = inputs;
}
