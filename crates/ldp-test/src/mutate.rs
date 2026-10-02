//! Deterministic byte- and message-level mutators.
//!
//! The fuzzers' entire "strategy" lives here: given a *valid* input and
//! RNG budget, produce a mutated variant. Every operator is pure — the
//! same `(seed, input)` pair always yields the same mutant, so a fuzzer
//! finding is replayable by seed alone.
//!
//! Two levels are provided:
//!
//! * [`mutate_bytes`] — structure-blind: flips, splices, truncations,
//!   extensions over any byte buffer (used on raw streams and foreign
//!   wire formats).
//! * [`mutate_message`] — structure-aware over the 16-byte LDP
//!   envelope (`ldp_protocol::envelope` layout): header field
//!   corruption (payload words, object id, opcode, reserved flag bits,
//!   FD count), payload corruption, and mid-message truncation — the
//!   shapes the validation pipeline's stages 1–3 exist to reject.
//! * [`chunk_stream`] — deterministic fragmentation of a valid byte
//!   stream into delivery chunks (the transport fuzzer's partial-
//!   message shapes).

use crate::rng::SplitMix64;

/// Maximum bytes a mutant may grow beyond its source.
const GROW_LIMIT: usize = 64;

/// Apply 1–4 structure-blind mutations to `src`.
///
/// The result is always a fresh buffer; `src` is untouched. Length can
/// shrink to zero or grow by at most `GROW_LIMIT` (64) bytes.
#[must_use]
pub fn mutate_bytes(rng: &mut SplitMix64, src: &[u8]) -> Vec<u8> {
    let mut out = src.to_vec();
    let ops = 1 + rng.below(4);
    for _ in 0..ops {
        if out.is_empty() {
            out.push(rng.byte());
            continue;
        }
        match rng.below(6) {
            0 => {
                // Bit flip at a random position.
                let i = rng.below(out.len());
                out[i] ^= 1 << rng.below(8);
            }
            1 => {
                // Byte replacement (biased to interesting values).
                let i = rng.below(out.len());
                out[i] = interesting_byte(rng);
            }
            2 => {
                // Splice: overwrite a run with another (possibly
                // overlapping) run of the source. An empty source has
                // nothing to splice from — the run becomes pure random
                // bytes (the degenerate arm the `k >= take` side already
                // draws), and the modulo below must never evaluate
                // against a zero length.
                let dst = rng.below(out.len());
                let room = (out.len() - dst).min(8);
                let len = 1 + rng.below(room);
                let (src_at, take) = if src.is_empty() {
                    (0, 0)
                } else {
                    let at = rng.below(src.len());
                    (at, 1 + rng.below((src.len() - at).min(8)))
                };
                for k in 0..len {
                    out[dst + k] = if k < take {
                        src[(src_at + k) % src.len()]
                    } else {
                        rng.byte()
                    };
                }
            }
            3 => {
                // Truncate (keep at least nothing — empty is valid).
                let at = rng.below(out.len() + 1);
                out.truncate(at);
            }
            4 => {
                // Extend with up to GROW_LIMIT/2 random bytes.
                let add = 1 + rng.below(GROW_LIMIT / 2);
                if out.len() + add <= src.len() + GROW_LIMIT {
                    out.extend((0..add).map(|_| rng.byte()));
                }
            }
            _ => {
                // Duplicate a run in place (classic length-confusion
                // shape).
                let at = rng.below(out.len());
                let room = (out.len() - at).min(8);
                let len = 1 + rng.below(room);
                let run = out[at..at + len].to_vec();
                let tail = out[at..].to_vec();
                out.truncate(at);
                out.extend(run);
                out.extend(tail);
                if out.len() > src.len() + GROW_LIMIT {
                    out.truncate(src.len() + GROW_LIMIT);
                }
            }
        }
    }
    out
}

/// A byte biased toward protocol-interesting values.
fn interesting_byte(rng: &mut SplitMix64) -> u8 {
    const INTERESTING: [u8; 16] = [
        0x00, 0x01, 0x07, 0x08, 0x0F, 0x10, 0x7F, 0x80, 0xFF, 0xFE, 0xAA, 0x55, 0x08, 0x10, 0xE0,
        0x0F,
    ];
    if rng.next_bool() {
        *rng.pick(&INTERESTING)
    } else {
        rng.byte()
    }
}

/// One structure-aware mutation over a framed LDP message.
///
/// `src` must be at least one envelope long (16 bytes) — the caller
/// holds a valid encoded message. The mutant is a fresh buffer; the
/// header layout knowledge comes from `ldp_protocol::envelope`:
/// `payload_words@0..4`, `object_id@4..8`, `opcode@8..12`,
/// `flags@12..14`, `fd_count@14..16`.
///
/// # Panics
///
/// Panics when `src` is shorter than the 16-byte envelope — harness
/// misuse, fail loudly.
#[must_use]
pub fn mutate_message(rng: &mut SplitMix64, src: &[u8]) -> Vec<u8> {
    assert!(src.len() >= 16, "mutate_message needs a full envelope");
    let mut out = src.to_vec();
    match rng.below(7) {
        0 => {
            // Corrupt payload_words: uniform over interesting scales —
            // tiny, exactly-fitting, one-past, and huge.
            let words: u32 = match rng.below(4) {
                0 => 0,
                1 => (out.len() as u32 - 16) / 8,     // exact fit
                2 => (out.len() as u32 - 16) / 8 + 1, // one word past
                _ => rng.next_u32() >> rng.below(16).min(20),
            };
            out[0..4].copy_from_slice(&words.to_le_bytes());
        }
        1 => {
            // Corrupt the object id: random client/server-range ids
            // and reserved shapes.
            let id = match rng.below(3) {
                0 => 0,
                1 => rng.next_u32() & !(1 << 31), // client range
                _ => rng.next_u32() | (1 << 31),  // server range
            };
            out[4..8].copy_from_slice(&id.to_le_bytes());
        }
        2 => {
            // Corrupt the opcode: small valid-ish, one-past-max, huge.
            let opcode: u32 = match rng.below(3) {
                0 => 1 + rng.below(64) as u32,
                1 => 1 << rng.below(24).min(31),
                _ => rng.next_u32(),
            };
            out[8..12].copy_from_slice(&opcode.to_le_bytes());
        }
        3 => {
            // Corrupt flags: reserved bits must be fatal — hit them
            // deliberately, plus the two defined bits.
            let flags: u16 = if rng.next_bool() {
                (rng.next_u32() as u16) & !(0x0003) // some reserved bit set
            } else {
                rng.next_u32() as u16 & 0x0003 // urgent / reply combos
            };
            out[12..14].copy_from_slice(&flags.to_le_bytes());
        }
        4 => {
            // Corrupt the declared FD count: mismatch shapes.
            let fds: u16 = match rng.below(3) {
                0 => 0,
                1 => 1 + rng.below(64) as u16,
                _ => u16::MAX,
            };
            out[14..16].copy_from_slice(&fds.to_le_bytes());
        }
        5 => {
            // Payload corruption at a random offset.
            if out.len() > 16 {
                let i = 16 + rng.below(out.len() - 16);
                out[i] ^= 1 << rng.below(8);
                if rng.next_bool() && out.len() > i + 1 {
                    out[i + 1] = interesting_byte(rng);
                }
            }
        }
        _ => {
            // Mid-message truncation (header-internal or payload
            // loss) — then sometimes re-pad so lengths stay aligned
            // while contents lie.
            let at = rng.below(out.len() + 1);
            out.truncate(at);
            if rng.next_bool() {
                while out.len() % 8 != 0 {
                    out.push(rng.byte());
                }
            }
        }
    }
    out
}

/// Deterministically fragment `src` into 1..=`max_chunks` chunks.
///
/// Each chunk is non-empty (the sole exception: an empty `src` yields
/// one empty chunk, preserving the concatenation contract); concatenation
/// reproduces `src` exactly. Used to drive the framing reader through
/// partial-delivery shapes.
///
/// # Panics
///
/// Panics when `src` is non-empty and `max_chunks` is zero.
#[must_use]
pub fn chunk_stream(rng: &mut SplitMix64, src: &[u8], max_chunks: usize) -> Vec<Vec<u8>> {
    if src.len() < 2 {
        // A zero- or one-byte stream has no interior cut point: it
        // delivers as one chunk. (The v0.12 fuzz soak at 8x scale
        // found the one-byte case panicking: `want_chunks` is always
        // at least 2, so the interior-cut draw `below(len - 1)` ran
        // against a zero. Found by the transport fuzzer's mutated
        // streams — a truncation mutant can park the stream at
        // exactly one byte.)
        return vec![src.to_vec()];
    }
    // Interior cuts only: the final cut is always src.len(), so the
    // chunk count is interior_cuts + 1 <= max_chunks.
    let want_chunks = 1 + rng.below(max_chunks.max(1));
    let mut cuts: Vec<usize> = (0..want_chunks - 1)
        .map(|_| 1 + rng.below(src.len() - 1))
        .collect();
    cuts.push(src.len());
    cuts.sort_unstable();
    cuts.dedup();
    let mut chunks = Vec::new();
    let mut prev = 0;
    for cut in cuts {
        if cut > prev {
            chunks.push(src[prev..cut].to_vec());
        }
        prev = cut;
    }
    if chunks.is_empty() {
        chunks.push(src.to_vec());
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutate_bytes_is_seed_deterministic() {
        let src = vec![1u8; 64];
        let mut a = SplitMix64::new(11);
        let mut b = SplitMix64::new(11);
        for _ in 0..200 {
            assert_eq!(mutate_bytes(&mut a, &src), mutate_bytes(&mut b, &src));
        }
    }

    #[test]
    fn mutate_bytes_never_exceeds_growth_limit() {
        let src = vec![0u8; 32];
        let mut r = SplitMix64::new(3);
        for _ in 0..500 {
            let m = mutate_bytes(&mut r, &src);
            assert!(m.len() <= src.len() + GROW_LIMIT);
        }
    }

    #[test]
    fn mutate_bytes_sometimes_changes_length_and_sometimes_not() {
        let src = vec![5u8; 128];
        let mut r = SplitMix64::new(21);
        let mut changed = false;
        let mut same = false;
        for _ in 0..200 {
            let m = mutate_bytes(&mut r, &src);
            if m.len() == src.len() {
                same = true;
            } else {
                changed = true;
            }
        }
        assert!(changed && same, "operators must exercise both shapes");
    }

    #[test]
    fn mutate_message_is_seed_deterministic_and_header_aware() {
        let msg: Vec<u8> = (0..48u8).collect(); // header + 32 payload
        let mut a = SplitMix64::new(77);
        let mut b = SplitMix64::new(77);
        for _ in 0..300 {
            let ma = mutate_message(&mut a, &msg);
            let mb = mutate_message(&mut b, &msg);
            assert_eq!(ma, mb);
            assert!(ma.len() <= msg.len() + GROW_LIMIT);
        }
    }

    #[test]
    fn mutate_message_mutations_are_visible() {
        // Over many seeds every header field must be hit at least once.
        let msg: Vec<u8> = (0..40u8).collect();
        let mut r = SplitMix64::new(1000);
        let mut words = 0;
        let mut obj = 0;
        let mut opcode = 0;
        let mut flags = 0;
        let mut fds = 0;
        let mut payload = 0;
        for _ in 0..700 {
            let m = mutate_message(&mut r, &msg);
            if m.len() >= 16 {
                if m[0..4] != msg[0..4] {
                    words += 1;
                }
                if m[4..8] != msg[4..8] {
                    obj += 1;
                }
                if m[8..12] != msg[8..12] {
                    opcode += 1;
                }
                if m[12..14] != msg[12..14] {
                    flags += 1;
                }
                if m[14..16] != msg[14..16] {
                    fds += 1;
                }
                if m.len() > 16 && m[16..] != msg[16..m.len().min(msg.len())] {
                    payload += 1;
                }
            }
        }
        assert!(words > 0 && obj > 0 && opcode > 0 && flags > 0 && fds > 0 && payload > 0);
    }

    #[test]
    #[should_panic(expected = "full envelope")]
    fn mutate_message_rejects_short_input() {
        let mut r = SplitMix64::new(1);
        let _ = mutate_message(&mut r, &[0u8; 8]);
    }

    #[test]
    fn chunk_stream_reassembles_exactly() {
        let src: Vec<u8> = (0..200u8).collect();
        let mut r = SplitMix64::new(9);
        for max in 1..8 {
            for _ in 0..100 {
                let chunks = chunk_stream(&mut r, &src, max);
                assert!(chunks.len() <= max.max(1));
                let joined: Vec<u8> = chunks.concat();
                assert_eq!(joined, src);
            }
        }
    }

    #[test]
    fn chunk_stream_empty_source() {
        let mut r = SplitMix64::new(2);
        let chunks = chunk_stream(&mut r, &[], 4);
        assert_eq!(chunks, vec![Vec::<u8>::new()]);
    }

    #[test]
    fn chunk_stream_single_byte_source_never_panics() {
        // The v0.12 fuzz-soak finding: a one-byte source has no
        // interior cut, and the pre-fix draw ran `below(0)` for every
        // want-chunks value (always >= 2). One byte delivers as one
        // chunk, concatenation exact, at every cap.
        let mut r = SplitMix64::new(0x5011_1122);
        for _ in 0..500 {
            for max in 1..8 {
                let chunks = chunk_stream(&mut r, &[0xAB], max);
                assert_eq!(chunks, vec![vec![0xAB]]);
            }
        }
    }

    #[test]
    fn mutate_bytes_empty_source_never_panics() {
        // The splice case's latent sibling: a mutant lineage that
        // truncates to zero can become the next generation's source,
        // and the pre-fix splice drew `below(src.len())` against zero.
        // Empty sources stay seed-deterministic and bounded.
        let mut a = SplitMix64::new(0xDE0B_0A12);
        let mut b = SplitMix64::new(0xDE0B_0A12);
        for _ in 0..300 {
            let ma = mutate_bytes(&mut a, &[]);
            let mb = mutate_bytes(&mut b, &[]);
            assert_eq!(ma, mb);
            assert!(ma.len() <= GROW_LIMIT);
        }
    }
}
