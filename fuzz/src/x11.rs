//! Target: the X11 bridge request framing (`ldp-x11-bridge`).
//!
//! Per iteration: build a stream of 2–6 well-formed X11 requests
//! (short 4-byte-header framing, occasionally a BIG-REQUESTS escape),
//! in a random endianness; corrupt it
//! ([`ldp_test::mutate::mutate_bytes`]); then repeatedly extract
//! requests ([`frame_request`]) until the stream errors or ends.
//!
//! Invariants: never a panic; a consumed request always claims between
//! 4 bytes and the whole remaining buffer; the oversize and
//! zero-length escapes come back as structured `WireError`s.

use ldp_test::mutate::mutate_bytes;
use ldp_test::rng::SplitMix64;
use ldp_x11_bridge::wire::{frame_request, Endian};

use crate::report::TargetReport;

/// Run the X11 framing fuzzer.
///
/// Deterministic in `(seed, iterations)`.
///
/// # Panics
///
/// Only on framing invariant violations: a consumed size outside the
/// buffer or disagreeing with the payload length.
#[allow(clippy::too_many_lines)] // one straight-line harness loop
pub fn run(seed: u64, iterations: u64) -> TargetReport {
    let mut rng = SplitMix64::new(seed);
    let mut report = TargetReport {
        target: "x11",
        iterations,
        ..TargetReport::default()
    };

    for _ in 0..iterations {
        let endian = if rng.next_bool() {
            Endian::Lsb
        } else {
            Endian::Msb
        };
        let bigreq = rng.next_bool();

        // The corpus: well-formed requests in this endianness.
        let mut buf: Vec<u8> = Vec::new();
        let count = 2 + rng.below(5);
        for _ in 0..count {
            let opcode = rng.byte();
            let data = rng.byte();
            if bigreq && rng.below(4) == 0 {
                // BIG-REQUESTS escape: 16-byte header, u64 length at
                // 8..16 (in units of 4 bytes).
                let units = 4 + rng.below(64) as u64;
                buf.push(opcode);
                buf.push(data);
                buf.extend(put_u16(endian, 0));
                buf.extend([0u8; 4]); // unused length field for escapes
                buf.extend(put_u64(endian, units));
                let payload = (units as usize * 4).saturating_sub(16);
                buf.extend(rng.bytes(payload));
            } else {
                // Short framing: u16 length (4-byte units) at 2..4.
                let units = 1 + rng.below(64) as u16;
                buf.push(opcode);
                buf.push(data);
                buf.extend(put_u16(endian, units));
                let payload = units as usize * 4 - 4;
                buf.extend(rng.bytes(payload));
            }
        }

        // Corrupt, then extract as much as the framer accepts.
        let mutant = mutate_bytes(&mut rng, &buf);
        let mut at = 0usize;
        while at < mutant.len() {
            let Ok((framed, used)) = frame_request(&mutant[at..], endian, bigreq) else {
                report.rejected += 1;
                break;
            };
            report.accepted += 1;
            assert!(
                used >= 4,
                "a framed request must consume at least its header"
            );
            assert!(
                at + used <= mutant.len(),
                "a framed request consumed past the buffer"
            );
            let header = if framed.payload.len() + 4 == used {
                4
            } else {
                16
            };
            assert_eq!(
                framed.payload.len() + header,
                used,
                "payload + header must equal the consumed size"
            );
            at += used;
        }
    }
    report
}

/// A u16 in the given endianness.
fn put_u16(endian: Endian, value: u16) -> [u8; 2] {
    endian.put_u16(value)
}

/// A u64 as two endianness-ordered u32 words.
fn put_u64(endian: Endian, value: u64) -> [u8; 8] {
    let low = u32::try_from(value & 0xFFFF_FFFF).expect("masked");
    let high = u32::try_from(value >> 32).expect("shifted");
    let [a0, a1, a2, a3] = endian.put_u32(high);
    let [b0, b1, b2, b3] = endian.put_u32(low);
    [a0, a1, a2, a3, b0, b1, b2, b3]
}
