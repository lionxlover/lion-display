//! Recording codec tests.
//!
//! Round-trip losslessness, canonical encoding, corruption detection
//! (checksum, truncation, bad magic, unknown version), structural
//! rejection of out-of-order inputs and zero refresh, and the empty
//! recording.

use ldp_compositor::replay::{record, replay, Recording, ReplayError};
use ldp_compositor::replay_codec::fnv1a;
use ldp_compositor::sched_types::SchedInput;
use ldp_compositor::scheduler::SchedulerConfig;
use ldp_compositor::SurfaceId;
use ldp_core::time::{Mono, RefreshInterval};

const SIXTY: u64 = 16_666_666;

fn nominal() -> RefreshInterval {
    RefreshInterval::from_ns(SIXTY).unwrap()
}

const MAGIC_LEN: usize = 7;

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
fn round_trip_is_lossless() {
    let recording = record(SchedulerConfig::default(), nominal(), sample_inputs()).unwrap();
    let bytes = recording.to_bytes();
    let decoded = Recording::from_bytes(&bytes).unwrap();
    assert_eq!(decoded, recording);
    // Encoding is canonical: re-encode gives identical bytes.
    assert_eq!(decoded.to_bytes(), bytes);
}
#[test]
fn corruption_is_detected() {
    let recording = record(SchedulerConfig::default(), nominal(), sample_inputs()).unwrap();
    let mut bytes = recording.to_bytes();
    // Flip one payload byte (not the trailing checksum).
    let idx = bytes.len() / 2;
    bytes[idx] ^= 0xff;
    assert_eq!(
        Recording::from_bytes(&bytes).err(),
        Some(ReplayError::Checksum)
    );
    // Truncation below the header floor is detected.
    let bytes = recording.to_bytes();
    assert_eq!(
        Recording::from_bytes(&bytes[..10]).err(),
        Some(ReplayError::Truncated)
    );
    // Bad magic is detected.
    let mut bytes = recording.to_bytes();
    bytes[0] = b'X';
    assert_eq!(
        Recording::from_bytes(&bytes).err(),
        Some(ReplayError::BadMagic)
    );
    // Unknown version is detected.
    let mut bytes = recording.to_bytes();
    bytes[MAGIC_LEN] = 0x02;
    // Repair the checksum so only the version differs.
    let split = bytes.len() - 8;
    let checksum = fnv1a(&bytes[..split]);
    bytes[split..].copy_from_slice(&checksum.to_le_bytes());
    assert_eq!(
        Recording::from_bytes(&bytes).err(),
        Some(ReplayError::BadVersion)
    );
}
#[test]
fn out_of_order_inputs_rejected_at_decode() {
    let mut recording = record(SchedulerConfig::default(), nominal(), sample_inputs()).unwrap();
    recording.inputs[2] = SchedInput::Flip {
        ts: Mono::from_ns(10),
    };
    // In-memory replay panics via the scheduler's ordering assert;
    // the decoder rejects it structurally instead.
    let bytes = recording.to_bytes();
    assert_eq!(
        Recording::from_bytes(&bytes).err(),
        Some(ReplayError::BadValue)
    );
}
#[test]
fn zero_refresh_recording_is_bad_value() {
    let mut recording = record(SchedulerConfig::default(), nominal(), vec![]).unwrap();
    recording.nominal_ns = 0;
    assert_eq!(replay(&recording).err(), Some(ReplayError::BadValue));
    let bytes = recording.to_bytes();
    assert_eq!(
        Recording::from_bytes(&bytes).err(),
        Some(ReplayError::BadValue)
    );
}
#[test]
fn empty_recording_round_trips() {
    let recording = record(SchedulerConfig::default(), nominal(), vec![]).unwrap();
    assert!(recording.outputs.is_empty());
    let bytes = recording.to_bytes();
    let decoded = Recording::from_bytes(&bytes).unwrap();
    assert_eq!(decoded.inputs, vec![]);
    assert_eq!(replay(&decoded).unwrap(), vec![]);
}
