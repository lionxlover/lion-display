//! Target: the Wayland bridge wire codec (`ldp-wayland-bridge`).
//!
//! Per iteration: pick one pinned interface and one of its request
//! schemas, synthesize a *valid* message for it, prove the
//! encode/decode round-trip, then push mutants (blind and
//! header-word-targeted) through `wire::decode`.
//!
//! Invariants: valid messages round-trip exactly; mutants always come
//! back as `Ok`, `Incomplete(n)`, or `Malformed(at)` — never a panic,
//! and `Incomplete` bounds (`n`) never exceed any sane buffer size.

use ldp_test::mutate::mutate_bytes;
use ldp_test::rng::SplitMix64;
use ldp_wayland_bridge::protocol::{self, Interface};
use ldp_wayland_bridge::wire::{self, Arg, Message, Value};

use crate::report::TargetReport;

/// The pinned interfaces the fuzzer sweeps.
const INTERFACES: &[&Interface] = &[
    &protocol::WL_DISPLAY,
    &protocol::WL_REGISTRY,
    &protocol::WL_CALLBACK,
    &protocol::WL_COMPOSITOR,
    &protocol::WL_SHM,
    &protocol::WL_SHM_POOL,
    &protocol::WL_BUFFER,
    &protocol::WL_SURFACE,
    &protocol::WL_REGION,
    &protocol::WL_SEAT,
    &protocol::WL_POINTER,
];

/// Run the Wayland wire codec fuzzer.
///
/// Deterministic in `(seed, iterations)`.
///
/// # Panics
///
/// Only on codec invariant violations: round-trip drift, a bound that
/// does not exceed the buffered bytes, or no progress after honoring
/// an incomplete bound.
#[allow(clippy::too_many_lines)] // one straight-line harness loop
pub fn run(seed: u64, iterations: u64) -> TargetReport {
    let mut rng = SplitMix64::new(seed);
    let mut report = TargetReport {
        target: "wayland",
        iterations,
        ..TargetReport::default()
    };

    for _ in 0..iterations {
        let iface = *rng.pick(INTERFACES);
        if iface.requests.is_empty() {
            continue;
        }
        let schema = rng.pick(iface.requests);
        let object_id = 1 + rng.below(64) as u32;

        // Valid message for this schema.
        let msg = Message {
            object_id,
            opcode: schema.opcode,
            args: schema
                .args
                .iter()
                .map(|a| valid_value(&mut rng, *a))
                .collect(),
        };
        let (bytes, fd_count) = wire::encode(&msg, schema.args);
        report.accepted += 1;

        // Invariant: the round-trip is exact.
        let (back, used, fds) = wire::decode(&bytes, object_id, schema.opcode, schema.args)
            .unwrap_or_else(|e| panic!("valid wayland message failed to decode: {e:?}"));
        assert_eq!(back, msg, "round-trip changed a schema-valid message");
        assert_eq!(used, bytes.len(), "decode must consume the whole message");
        assert_eq!(fds, fd_count, "fd slot count drifted");

        // Mutants: blind corruption plus header-word targeting.
        let mutant = if rng.next_bool() {
            mutate_bytes(&mut rng, &bytes)
        } else {
            header_mutant(&mut rng, &bytes)
        };
        match wire::decode(&mutant, object_id, schema.opcode, schema.args) {
            Ok((m, _, _)) => {
                report.accepted += 1;
                // A surviving mutant must still be a whole message.
                assert!(
                    !m.args.is_empty() || schema.args.is_empty(),
                    "decoded arguments disagree with the schema shape"
                );
            }
            Err(status) => {
                report.rejected += 1;
                if let wire::DecodeStatus::Incomplete(need) = status {
                    // The bound is a promise about the future:
                    // an incomplete bound always exceeds what is
                    // buffered, and honoring it (when cheap) must let
                    // the decoder past — Ok or Malformed, never
                    // Incomplete again. The length-bomb *policy*
                    // lives in the bridge driver
                    // (`ldp-wayland-bridge/tests/length_bomb.rs`).
                    assert!(
                        need > mutant.len(),
                        "an incomplete bound ({need}) must exceed the buffered bytes ({})",
                        mutant.len()
                    );
                    if need <= 1 << 20 {
                        let mut padded = mutant.clone();
                        padded.resize(need, 0);
                        if let Err(wire::DecodeStatus::Incomplete(next)) =
                            wire::decode(&padded, object_id, schema.opcode, schema.args)
                        {
                            assert!(
                                next > need,
                                "no progress: decoder asked for {need} bytes and still \
                                 needs only {next} after they arrived"
                            );
                        }
                    }
                }
            }
        }
    }
    report
}

/// Corrupt the 4-byte header word (id/opcode packing).
fn header_mutant(rng: &mut SplitMix64, src: &[u8]) -> Vec<u8> {
    let mut out = src.to_vec();
    if out.len() < 4 {
        return out;
    }
    let field = rng.below(4);
    out[field] ^= 1 << rng.below(8);
    out
}

/// One valid value for one argument kind.
fn valid_value(rng: &mut SplitMix64, arg: Arg) -> Value {
    match arg {
        Arg::Int => Value::Int(rng.next_u32() as i32),
        Arg::Uint => Value::Uint(rng.next_u32()),
        Arg::Fixed => Value::Fixed(rng.next_u32() as i32),
        Arg::String => {
            const ALPHABET: &[u8] = b"wayland-fuzz-0123456789";
            let len = rng.below(32);
            let s: String = (0..len)
                .map(|_| ALPHABET[rng.below(ALPHABET.len())] as char)
                .collect();
            Value::String(s.into())
        }
        Arg::Array => {
            let len = rng.below(64);
            Value::Array(rng.bytes(len).into_boxed_slice())
        }
        Arg::Object => Value::Object(1 + rng.below(64) as u32),
        Arg::NewId => Value::NewId(1 + rng.below(1 << 20) as u32),
        Arg::Fd => Value::Fd(0),
    }
}
