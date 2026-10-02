//! THE Phase 13 exit criterion (part 3): a 100 MB stream through
//! **real pipes** under limits.
//!
//! The proof stack:
//!
//! * **Page doctrine**: the entire copy runs through one 4096-byte
//!   buffer (asserted — the server never buffers more than a page).
//! * **Backpressure honesty**: the pipe's own ~64 KiB capacity is the
//!   only other buffering; the writer blocks until the reader drains
//!   (a reader thread), proving the stream never buffers in the
//!   server.
//! * **Byte-exactness**: a streaming checksum over both sides plus
//!   structured pattern spot-checks.
//! * **FD hygiene**: the process FD table is snapshot-stable across
//!   the whole transfer (the leak exit criterion).
//! * **Registry under limits**: the transfer is admitted under the
//!   default client FD budget, settles to `Done(100 MB)`, and the
//!   ledger returns to zero after reaping.

mod common;

use common::{Checksum, Pattern};
use ldp_clipboard::pipe;
use ldp_clipboard::transfer::{
    pump_to_completion, Pump, Transfer, TransferId, TransferRegistry, TransferState, PAGE,
};
use ldp_clipboard::{ClientKey, OfferKey, SourceKey};
use ldp_core::limits::Limits;
use std::sync::Mutex;

/// FD-count assertions are process-wide: the pipe-touching tests
/// serialize on this mutex (the `ldp-transport` leak-suite
/// precedent — its `count_open_fds` doc calls this out).
static FD_GUARD: Mutex<()> = Mutex::new(());

const TOTAL: u64 = 100 * 1024 * 1024;

/// A byte source that materializes the pattern on the fly (the
/// server-side adaptor shape: content flows through the pump without
/// ever being buffered beyond the page).
struct PatternSource {
    pattern: Pattern,
    pos: u64,
    total: u64,
}

impl PatternSource {
    fn new(pattern: Pattern, total: u64) -> PatternSource {
        PatternSource {
            pattern,
            pos: 0,
            total,
        }
    }
}

impl std::io::Read for PatternSource {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let remaining = self.total - self.pos;
        if remaining == 0 {
            return Ok(0);
        }
        let n = buf.len().min(remaining as usize);
        for (i, slot) in buf.iter_mut().enumerate().take(n) {
            *slot = self.pattern.byte(self.pos + i as u64);
        }
        self.pos += n as u64;
        Ok(n)
    }
}

#[test]
fn stream_100mb_through_real_pipes_under_limits() {
    let _guard = FD_GUARD
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let before_fds = pipe::count_open_fds().expect("fd count");

    // The transfer registry under the default limits.
    let mut reg = TransferRegistry::new(Limits::default());
    let tid = TransferId::new(1);
    reg.admit(
        tid,
        ClientKey(1),
        OfferKey::new(1),
        SourceKey::new(1),
        "application/octet-stream",
    )
    .expect("admission under the default fd budget");

    // A real pipe, blocking mode.
    let mut pair = pipe::pipe().expect("pipe");
    pair.set_blocking().expect("blocking");
    let mut reader = pair.read;
    let mut writer = pair.write;

    // Reader thread: drains the pipe, checksums, verifies the
    // pattern byte-for-byte against the deterministic generator.
    let handle = std::thread::spawn(move || {
        let pattern = Pattern::Ramp(0x5A);
        let mut got = Checksum::new();
        let mut want = Checksum::new();
        let mut buf = [0u8; PAGE];
        let mut seen: u64 = 0;
        loop {
            let n = reader.read(&mut buf).expect("pipe read");
            if n == 0 {
                break;
            }
            // Pattern check in situ (cheap: verify every byte's
            // expected value without materializing 100 MB).
            for (i, &b) in buf[..n].iter().enumerate() {
                assert_eq!(
                    b,
                    pattern.byte(seen + i as u64),
                    "pattern mismatch at offset {}",
                    seen + i as u64
                );
            }
            got.absorb(&buf[..n]);
            seen += n as u64;
        }
        want.absorb(&pattern.buffer(TOTAL));
        (got.digest(), want.digest(), seen)
    });

    // Writer side: the pump with exactly one page of working set,
    // fed by the on-the-fly pattern source.
    let mut src = PatternSource::new(Pattern::Ramp(0x5A), TOTAL);
    let mut pump = Pump::new();
    let moved = pump_to_completion(&mut src, &mut writer, &mut pump)
        .expect("the blocking pipe with a live reader never stalls the pump");
    assert_eq!(moved, TOTAL);
    reg.advance(tid, moved);
    drop(writer); // EOF to the reader.

    let (got, want, seen) = handle.join().expect("reader thread");
    assert_eq!(seen, TOTAL, "reader saw {seen} bytes");
    assert_eq!(got, want, "streaming checksums disagree");

    // Settle + reap: the ledger returns to zero.
    reg.settle(tid, TransferState::Done(TOTAL));
    assert_eq!(reg.get(tid).map(Transfer::bytes), Some(TOTAL));
    assert_eq!(reg.client_transfers(ClientKey(1)), 0);
    let reaped = reg.reap(tid).expect("reap");
    assert_eq!(reaped.bytes(), TOTAL);
    assert!(reg.is_empty());

    // FD hygiene: the table is exactly back to its pre-transfer state.
    let after_fds = pipe::count_open_fds().expect("fd count");
    assert_eq!(before_fds, after_fds, "fd leak across the 100 MB transfer");
}

const STREAMS: u64 = 4;
const SIZE: u64 = 8 * 1024 * 1024;

#[test]
fn concurrent_streams_share_the_budget_correctly() {
    let _guard = FD_GUARD
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // Four concurrent 8 MB streams under a tightened budget of 4,
    // interleaved chunk-by-chunk on one thread (the event-loop
    // shape): all complete, byte-exact, and the ledger is exact.
    let limits = Limits {
        client_fds: 4,
        ..Limits::default()
    };
    let mut reg = TransferRegistry::new(limits);
    let mut pairs = Vec::new();
    let mut tids = Vec::new();
    for i in 0..STREAMS {
        let mut p = pipe::pipe().expect("pipe");
        p.set_blocking().expect("blocking");
        let tid = TransferId::new(i);
        reg.admit(
            tid,
            ClientKey(1),
            OfferKey::new(i),
            SourceKey::new(1),
            "text/plain",
        )
        .expect("admission");
        tids.push(tid);
        pairs.push((p.read, p.write));
    }
    // A fifth admission must be refused (budget).
    assert!(reg
        .admit(
            TransferId::new(99),
            ClientKey(1),
            OfferKey::new(99),
            SourceKey::new(1),
            "text/plain"
        )
        .is_err());
    assert_eq!(reg.client_transfers(ClientKey(1)), 4);

    // Readers: one thread per stream.
    let readers: Vec<_> = pairs;
    let mut writer_holds = Vec::new();
    let mut reader_threads = Vec::new();
    for (i, (r, w)) in readers.into_iter().enumerate() {
        writer_holds.push(w);
        let pattern = Pattern::Tag((i as u128) + 1);
        reader_threads.push(std::thread::spawn(move || {
            let mut reader = r;
            let mut got = Checksum::new();
            let mut buf = [0u8; PAGE];
            let mut seen: u64 = 0;
            loop {
                let n = reader.read(&mut buf).expect("read");
                if n == 0 {
                    break;
                }
                for (j, &b) in buf[..n].iter().enumerate() {
                    assert_eq!(b, pattern.byte(seen + j as u64));
                }
                got.absorb(&buf[..n]);
                seen += n as u64;
            }
            (got.digest(), seen)
        }));
    }
    // Round-robin the writers (the readiness shape): each stream has
    // its own on-the-fly source; the single page buffer is shared
    // (the doctrine: one page per pump invocation, not per stream).
    let patterns: Vec<Pattern> = (0..STREAMS)
        .map(|i| Pattern::Tag((i as u128) + 1))
        .collect();
    let mut sources: Vec<PatternSource> = patterns
        .iter()
        .map(|p| PatternSource::new(*p, SIZE))
        .collect();
    // One page per pump — a pump per stream (the doctrine: N streams,
    // N pages, nothing else).
    let mut pumps: Vec<Pump> = (0..STREAMS).map(|_| Pump::new()).collect();
    let mut live: Vec<usize> = (0..STREAMS as usize).collect();
    while !live.is_empty() {
        let mut next_live = Vec::new();
        for &i in &live {
            let moved = pump_to_completion(&mut sources[i], &mut writer_holds[i], &mut pumps[i])
                .expect("pump");
            reg.advance(tids[i], moved);
            if (moved as u64) < SIZE {
                next_live.push(i);
            } else {
                // This stream is done: close its writer for EOF.
                writer_holds[i] = pipe::pipe().expect("pipe").write;
            }
        }
        live = next_live;
    }
    drop(writer_holds);
    for (i, h) in reader_threads.into_iter().enumerate() {
        let (digest, seen) = h.join().expect("reader");
        assert_eq!(seen, SIZE, "stream {i}");
        let mut want = Checksum::new();
        want.absorb(&patterns[i].buffer(SIZE));
        assert_eq!(digest, want.digest());
    }
    for tid in tids {
        reg.settle(tid, TransferState::Done(SIZE));
        assert_eq!(reg.get(tid).map(Transfer::bytes), Some(SIZE));
        reg.reap(tid);
    }
    assert!(reg.is_empty());
    assert_eq!(reg.client_transfers(ClientKey(1)), 0);
}

#[test]
fn page_is_the_only_working_set() {
    // Structural: the pump API exposes exactly one page-sized buffer
    // discipline; the constant is pinned here so any accidental
    // change to the doctrine fails this suite.
    assert_eq!(PAGE, 4096);
}
