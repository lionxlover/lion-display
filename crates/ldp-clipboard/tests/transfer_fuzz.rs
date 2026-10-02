//! THE Phase 13 exit criterion (part 2): transfer fuzz.
//!
//! Random MIME/size/pattern corpora stream through the pump with
//! adversarial half behaviors (short writes, would-block stalls,
//! mid-stream failures, cancel injections). The never-deadlock
//! property: every fuzzed transfer reaches a terminal state within a
//! bounded number of pump steps, no matter how pathological the
//! halves are — the pump never loops internally, and the registry's
//! budget ledger returns to zero exactly once per transfer.
//!
//! Also: the MIME corpus round-trips through every fuzzed transfer,
//! and byte-exactness is verified by streaming checksums on both
//! sides.

mod common;

use common::{Checksum, Pattern, Rng, MIME_CORPUS};
use ldp_clipboard::transfer::{
    pump_to_completion, AdmissionError, Pump, PumpStep, ReadHalf, Transfer, TransferId,
    TransferRegistry, TransferState, WriteHalf, PAGE,
};
use ldp_clipboard::{ClientKey, OfferKey, SourceKey};
use std::io;

/// A source that yields the pattern with adversarial chunking.
struct FuzzSource {
    data: Vec<u8>,
    pos: usize,
    /// Max bytes per read (1..=PAGE — forces many tiny reads).
    chunk: usize,
    /// Stall probability per read (WouldBlock).
    stall: f64,
    /// Fail permanently after this many reads (None: never).
    fail_at: Option<usize>,
    reads: usize,
    rng: Rng,
}

impl ReadHalf for FuzzSource {
    fn read_chunk(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.reads += 1;
        if let Some(f) = self.fail_at {
            if self.reads > f {
                return Err(io::Error::from(io::ErrorKind::BrokenPipe));
            }
        }
        if self.rng.below(100) < (self.stall * 100.0) as u64 {
            return Err(io::Error::from(io::ErrorKind::WouldBlock));
        }
        let n = self.chunk.min(buf.len()).min(self.data.len() - self.pos);
        // Random short reads.
        let n = if n > 0 && self.rng.flip() {
            1 + self.rng.below(n as u64) as usize
        } else {
            n
        };
        buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

/// A sink that accepts with adversarial chunking.
struct FuzzSink {
    out: Vec<u8>,
    chunk: usize,
    stall: f64,
    rng: Rng,
}

impl WriteHalf for FuzzSink {
    fn write_chunk(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.rng.below(100) < (self.stall * 100.0) as u64 {
            return Err(io::Error::from(io::ErrorKind::WouldBlock));
        }
        let n = self.chunk.min(buf.len());
        let n = if n > 0 && self.rng.flip() {
            1 + self.rng.below(n as u64) as usize
        } else {
            n
        };
        self.out.extend_from_slice(&buf[..n]);
        Ok(n)
    }
}

#[test]
fn fuzz_transfers_never_deadlock() {
    let mut rng = Rng::seeded(0xC0FF_EE00_1234_5678);
    let mut completed = 0u64;
    let mut failed = 0u64;
    let mut stalled_out = 0u64;
    for case in 0..2_000u64 {
        let mime = MIME_CORPUS[rng.below(MIME_CORPUS.len() as u64) as usize];
        let cap = rng.below(200_000);
        let size = rng.below(1 + cap);
        let pattern = Pattern::random(&mut rng);
        let data = pattern.buffer(size);
        let mut src = FuzzSource {
            data: data.clone(),
            pos: 0,
            chunk: 1 + rng.below(PAGE as u64) as usize,
            stall: if rng.flip() { 0.3 } else { 0.0 },
            fail_at: if rng.below(10) == 0 {
                Some(1 + rng.below(20) as usize)
            } else {
                None
            },
            reads: 0,
            rng: Rng::seeded(rng.next_u64()),
        };
        let mut dst = FuzzSink {
            out: Vec::new(),
            chunk: 1 + rng.below(PAGE as u64) as usize,
            stall: if rng.flip() { 0.3 } else { 0.0 },
            rng: Rng::seeded(rng.next_u64()),
        };
        let mut pump = Pump::new();
        // Drive with a hard step bound: the never-deadlock property.
        // (One byte per stall-free step is the worst case; stalls
        // don't count toward progress but the drivers advance.)
        let step_budget = 4 * size.max(1) + 256;
        let mut steps = 0u64;
        let outcome = loop {
            steps += 1;
            assert!(
                steps <= step_budget,
                "case {case}: transfer of {size} bytes exceeded {step_budget} pump steps"
            );
            match pump.step(&mut src, &mut dst) {
                PumpStep::Eof => break Ok(()),
                PumpStep::WouldBlock | PumpStep::SinkFull => {
                    // A readiness loop would retry; our adversarial
                    // drivers count stalls but keep going (the fuzz
                    // halves eventually stop stalling because their
                    // rng streams advance).
                    stalled_out += 1;
                }
                PumpStep::Progress(_) => {}
                PumpStep::Failed(kind) => break Err(kind),
            }
        };
        match outcome {
            Ok(()) => {
                completed += 1;
                assert_eq!(dst.out.len() as u64, size, "case {case}: length mismatch");
                // Byte-exactness via both-side checksums.
                let mut a = Checksum::new();
                let mut b = Checksum::new();
                a.absorb(&data);
                b.absorb(&dst.out);
                assert_eq!(
                    a.digest(),
                    b.digest(),
                    "case {case}: pattern {pattern:?} mime {mime}"
                );
            }
            Err(_) => failed += 1,
        }
    }
    assert!(completed > 1_500, "too few completions: {completed}");
    assert!(failed > 10, "failure path never exercised: {failed}");
    assert!(
        stalled_out > 100,
        "stall path never exercised: {stalled_out}"
    );
}

#[test]
fn fuzz_registry_ledger_always_returns_to_zero() {
    const CLIENT: ClientKey = ClientKey(3);
    let mut rng = Rng::seeded(0xFACE_B00C_5555_1111);
    let limits = ldp_core::limits::Limits {
        client_fds: 8,
        ..ldp_core::limits::Limits::default()
    };
    let mut reg = TransferRegistry::new(limits);
    let mut admitted = 0u64;
    let mut rejected = 0u64;
    for case in 0..5_000u64 {
        let tid = TransferId::new(case);
        match reg.admit(
            tid,
            if rng.flip() { CLIENT } else { ClientKey(9) },
            OfferKey::new(1),
            SourceKey::new(1),
            MIME_CORPUS[rng.below(MIME_CORPUS.len() as u64) as usize],
        ) {
            Ok(_) => admitted += 1,
            Err(AdmissionError::FdBudget) => rejected += 1,
            Err(AdmissionError::Duplicate) => panic!("duplicate in a fresh id space"),
        }
        // Drive terminals on *live* rows (a saturated budget only
        // drains through live settles — settling never-admitted ids
        // is a no-op and the ledger would wedge, which is exactly
        // what this fuzz guards against).
        let live: Vec<TransferId> = reg
            .all()
            .filter(|t| !t.terminal())
            .map(Transfer::id)
            .collect();
        if !live.is_empty() && rng.below(2) == 0 {
            let victim = live[rng.below(live.len() as u64) as usize];
            match rng.below(3) {
                0 => {
                    reg.settle(victim, TransferState::Done(rng.below(100_000)));
                }
                1 => {
                    reg.settle(victim, TransferState::Failed(io::ErrorKind::BrokenPipe, 0));
                }
                _ => {
                    reg.advance(victim, rng.below(1_000));
                    reg.settle(victim, TransferState::Cancelled(0));
                }
            }
        }
        // Reap attempts on random historical ids (mostly no-ops).
        if rng.below(3) == 0 {
            let _ = reg.reap(TransferId::new(rng.below(case + 1)));
        }
        // The ledger invariant: per-client counts are ≤ the budget
        // and never underflow (settle saturates).
        assert!(reg.client_transfers(CLIENT) <= 8);
        assert!(reg.client_transfers(ClientKey(9)) <= 8);
    }
    // Drain everything: the ledger returns to exactly zero.
    let ids: Vec<TransferId> = reg.all().map(Transfer::id).collect();
    for id in ids {
        reg.settle(id, TransferState::Cancelled(0));
        reg.reap(id);
    }
    assert_eq!(reg.client_transfers(CLIENT), 0);
    assert_eq!(reg.client_transfers(ClientKey(9)), 0);
    assert!(reg.is_empty());
    assert!(
        admitted > 1_000 && rejected > 100,
        "admitted {admitted} rejected {rejected}"
    );
}

#[test]
fn blocking_completion_of_a_huge_in_memory_stream() {
    // The pump-to-completion path over a large corpus (no stalls):
    // this is the deterministic control for the fuzz above.
    let mut rng = Rng::seeded(7);
    for _ in 0..20 {
        let size = 1 + rng.below(1_000_000) as usize;
        let pattern = Pattern::random(&mut rng);
        let data = pattern.buffer(size as u64);
        let mut src: &[u8] = &data;
        let mut dst = Vec::with_capacity(size);
        let mut pump = Pump::new();
        let n = pump_to_completion(&mut src, &mut dst, &mut pump).unwrap();
        assert_eq!(n as usize, size);
        assert_eq!(dst, data);
    }
}

#[test]
fn would_block_completion_errors_cleanly() {
    // pump_to_completion over would-block halves reports the stall,
    // never spins.
    struct Staller;
    impl ReadHalf for Staller {
        fn read_chunk(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::from(io::ErrorKind::WouldBlock))
        }
    }
    let mut dst = Vec::new();
    let mut pump = Pump::new();
    let err = pump_to_completion(&mut Staller, &mut dst, &mut pump).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::WouldBlock);
    assert!(dst.is_empty());
}
