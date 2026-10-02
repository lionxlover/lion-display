//! Shared testbench for the ldp-shell conformance suites.
//!
//! Determinism doctrine (no `rand` dependency): the fuzz drivers use
//! the same LCG the client crate's reconnect backoff uses. Every
//! helper is pure so a failing case reproduces from its seed alone.

#![allow(dead_code)] // shared across suites; each uses a subset

use ldp_core::geometry::Rect;
use ldp_core::ids::ObjectId;
use ldp_core::limits::Limits;
use ldp_core::scale::ScaleFactor;
use ldp_protocol::{check_signature, decode, Direction, Message, ValidationMode, REGISTRY};

use ldp_shell::event::ShellEvent;
use ldp_shell::toplevel::PolicyInputs;
use ldp_shell::{DecorationMode, SsdMetrics};

/// A deterministic xorshift64* driver (the same no-`rand` doctrine as
/// ldp-client's backoff jitter, at 64-bit width). Every output bit is
/// well-distributed — an LCG's low bits cycle with period 2^k, which
/// the conformance driver proved the hard way (bit 0 alternates, so a
/// regular below/flip call pattern froze `flip` at one value).
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    /// New driver from a seed (zero is promoted to one — zero is
    /// xorshift's fixed point).
    #[must_use]
    pub const fn seeded(seed: u64) -> Rng {
        if seed == 0 {
            Rng(1)
        } else {
            Rng(seed)
        }
    }

    /// Next raw output.
    #[must_use]
    pub fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// A bounded random value.
    #[must_use]
    pub fn below(&mut self, bound: u64) -> u64 {
        if bound == 0 {
            0
        } else {
            self.next_u64() % bound
        }
    }

    /// A random boolean.
    #[must_use]
    pub fn flip(&mut self) -> bool {
        self.next_u64() & 1 == 1
    }
}

/// The standard test policy inputs (a 1920×1080 output with a
/// 1920×1040 workspace area, SSD metrics, 1× scale).
#[must_use]
pub fn policy_inputs() -> PolicyInputs {
    PolicyInputs {
        workspace_area: Rect::new(0, 0, 1920, 1040),
        output_size: (1920, 1080),
        metrics: SsdMetrics::LION,
        decoration: DecorationMode::Server,
        scale: ScaleFactor::IDENTITY,
        workspace: 0,
        output: Some(ObjectId::from_wire(0x20)),
    }
}

/// Policy inputs at another scale (mixed-DPI suite).
#[must_use]
pub fn policy_inputs_at_scale(scale: ScaleFactor, output: ObjectId) -> PolicyInputs {
    let q = scale.to_q8();
    let w = ScaleFactor::from_q8(q).map_or(1920, |s| s.scale_px_up(960));
    let h = ScaleFactor::from_q8(q).map_or(1080, |s| s.scale_px_up(540));
    PolicyInputs {
        workspace_area: Rect::new(0, 0, w, h.saturating_sub(scale.scale_px_up(40))),
        output_size: (w, h),
        metrics: SsdMetrics::LION,
        decoration: DecorationMode::Server,
        scale,
        workspace: 0,
        output: Some(output),
    }
}

/// A scale factor from a float (test convenience; panics on invalid).
#[must_use]
pub fn scale(v: f32) -> ScaleFactor {
    ScaleFactor::from_f32_lossy(v).unwrap_or(ScaleFactor::IDENTITY)
}

/// Validate an emitted shell event end-to-end: encode → decode →
/// strict signature check against the compiled schema, and opcode
/// agreement with the schema's event table.
///
/// # Panics
///
/// On any mismatch — this is the conformance oracle.
pub fn assert_wire_valid(e: &ShellEvent, object: ObjectId) -> Message {
    let msg = e.to_message(object);
    let limits = Limits::default();
    let bytes = msg.encode(&limits).expect("encode");
    assert_eq!(msg.required_fd_count(), 0, "shell events carry no fds");
    let back = decode(&bytes, 0, &limits, ValidationMode::Strict).expect("decode");
    assert_eq!(back, msg, "shell event did not round-trip");
    let (_, matched) = check_signature(
        &REGISTRY,
        &back,
        e.interface(),
        Direction::Event,
        1,
        ValidationMode::Strict,
    )
    .unwrap_or_else(|err| {
        panic!(
            "{}::{} failed strict signature: {err}",
            e.interface(),
            e.name()
        )
    });
    assert_eq!(matched.name, e.name(), "opcode resolved to the wrong event");
    msg
}

/// Every event of a batch, wire-validated (returns the byte encoding
/// of each for golden comparison).
#[must_use]
pub fn validate_batch(events: &[(ShellEvent, ObjectId)]) -> Vec<Vec<u8>> {
    events
        .iter()
        .map(|(e, o)| {
            assert_wire_valid(e, *o)
                .encode(&Limits::default())
                .expect("encode")
        })
        .collect()
}
