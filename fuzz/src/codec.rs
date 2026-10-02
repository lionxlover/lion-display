//! Target: the LDP wire codec (`ldp-protocol`).
//!
//! Per iteration:
//!
//! 1. pick a random `(interface, direction, opcode)` from the compiled
//!    spec and synthesize a *valid* message
//!    ([`ldp_test::conformance::spec_message`]),
//! 2. prove the invariant: strict decode round-trips the message
//!    exactly and strict signature validation resolves it,
//! 3. mutate — structure-aware over the 16-byte envelope
//!    ([`ldp_test::mutate::mutate_message`]) or blind
//!    ([`ldp_test::mutate::mutate_bytes`]) — and push the mutant
//!    through the decoder in strict and tolerant modes with varying
//!    declared FD-table sizes.
//!
//! The fuzzer's job is to make the codec panic or violate the
//! round-trip invariant; every input is seed-addressed, so any finding
//! replays exactly.

use ldp_core::limits::Limits;
use ldp_protocol::schema::Direction;
use ldp_protocol::{check_signature, decode, ValidationMode, REGISTRY};
use ldp_test::conformance::spec_message;
use ldp_test::mutate::{mutate_bytes, mutate_message};
use ldp_test::rng::SplitMix64;

use crate::report::TargetReport;

/// Run the codec fuzzer.
///
/// Deterministic in `(seed, iterations)`.
///
/// # Panics
///
/// Only on a codec bug: a round-trip violation or an encoder failure
/// on spec-valid input. Malformed inputs must come back as structured
/// errors, never panics.
#[allow(clippy::too_many_lines)] // one straight-line harness loop
pub fn run(seed: u64, iterations: u64) -> TargetReport {
    // The op table: every (interface, direction, opcode, version) in
    // the compiled spec, built once.
    let mut ops: Vec<(&'static str, Direction, u32, u32)> = Vec::new();
    for module in REGISTRY.modules() {
        for iface in module.interfaces {
            for op in iface.requests {
                ops.push((iface.name, Direction::Request, op.opcode, iface.version_max));
            }
            for op in iface.events {
                ops.push((iface.name, Direction::Event, op.opcode, iface.version_max));
            }
        }
    }

    let mut rng = SplitMix64::new(seed);
    let mut report = TargetReport {
        target: "codec",
        iterations,
        ..TargetReport::default()
    };
    let limits = Limits::DEFAULT;

    for _ in 0..iterations {
        let (iface, dir, opcode, version) = *rng.pick(&ops);
        let Some(msg) = spec_message(&mut rng, &REGISTRY, iface, dir, opcode) else {
            continue;
        };

        // ---- invariant: valid messages round-trip ------------------
        let bytes = msg
            .encode(&limits)
            .unwrap_or_else(|e| panic!("spec message failed to encode: {e}"));
        let fd_count = msg.required_fd_count();
        let back = decode(&bytes, fd_count, &limits, ValidationMode::Strict)
            .unwrap_or_else(|e| panic!("spec message failed to decode: {e}"));
        assert_eq!(
            back,
            msg,
            "round-trip changed a spec-valid message for {iface} {}",
            dir.as_str()
        );
        report.accepted += 1;
        check_signature(
            &REGISTRY,
            &back,
            iface,
            dir,
            version,
            ValidationMode::Strict,
        )
        .unwrap_or_else(|e| panic!("spec message rejected by its own signature: {e}"));

        // ---- mutation: never a panic, always a structured verdict ---
        let mutant = if rng.next_bool() {
            mutate_message(&mut rng, &bytes)
        } else {
            mutate_bytes(&mut rng, &bytes)
        };
        // The declared FD-table size the receiver would see: the
        // mutant's own header field when it has one, sometimes
        // deliberately one smaller (the FdMismatch shapes).
        let declared = fd_count_of(&mutant);
        let table = if rng.next_bool() {
            declared
        } else {
            declared.saturating_sub(1)
        };
        let mode = if rng.next_bool() {
            ValidationMode::Strict
        } else {
            ValidationMode::Tolerant
        };
        if decode(&mutant, table, &limits, mode).is_ok() {
            report.accepted += 1;
        } else {
            report.rejected += 1;
        }
    }
    report
}

/// The mutant's declared FD count, when its envelope is intact.
fn fd_count_of(msg: &[u8]) -> u32 {
    if msg.len() >= 16 {
        u32::from(u16::from_le_bytes([msg[14], msg[15]]))
    } else {
        0
    }
}
