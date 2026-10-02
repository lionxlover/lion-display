//! Target: the transport framing reader (`ldp-transport`).
//!
//! Per iteration, over a **real socketpair**:
//!
//! * optionally send one well-formed message carrying a real
//!   SCM_RIGHTS batch of `/dev/null` descriptors (the accepted FD
//!   path),
//! * build 1–3 further framed messages, optionally corrupt the bytes
//!   (oversize headers, FD-count lies, payload damage), then fragment
//!   deterministically ([`ldp_test::mutate::chunk_stream`]),
//! * deliver all chunks — or only half of them (the crash shape: a
//!   peer vanishing with a message in flight),
//! * read frames until the reader errors or the stream ends — never a
//!   panic, always a structured verdict.
//!
//! Across the whole run the process FD table must return to its
//! baseline: adopted descriptors close on every path (accepted,
//! rejected, poisoned, half-delivered).

use std::os::fd::OwnedFd;

use ldp_core::limits::Limits;
use ldp_test::mutate::{chunk_stream, mutate_bytes};
use ldp_test::rng::SplitMix64;
use ldp_transport::{
    count_open_fds, is_would_block, FdList, FramedReader, FramedWriter, TransportStream,
};

use crate::report::TargetReport;

/// Run the transport fuzzer.
///
/// Deterministic in `(seed, iterations)` except for OS-level recv
/// chunking (the reader's contract covers every split).
///
/// # Panics
///
/// Only on an invariant violation: an FD leak across the run, an
/// accepted frame shorter than its own header, or adopted FDs
/// disagreeing with the declared count.
#[allow(clippy::too_many_lines)] // one straight-line harness loop
pub fn run(seed: u64, iterations: u64) -> TargetReport {
    let mut rng = SplitMix64::new(seed);
    let mut report = TargetReport {
        target: "transport",
        iterations,
        ..TargetReport::default()
    };
    let limits = Limits::DEFAULT;

    // Warm up the FD baseline with one full round-trip pair so lazy
    // runtime state settles before the baseline is taken.
    {
        let (mut tx, mut rx) = TransportStream::pair().expect("socketpair");
        let mut writer = FramedWriter::without_hooks(limits);
        let mut reader = FramedReader::new(limits);
        let msg = framed_message(&mut rng, 4, 0);
        writer
            .send_msg(&mut tx, &msg, &mut FdList::new())
            .expect("warmup send");
        let _ = reader.recv_msg(&mut rx).expect("warmup recv");
    }
    let fd_baseline = count_open_fds().expect("fd count");

    for _ in 0..iterations {
        let (mut tx, mut rx) = TransportStream::pair().expect("socketpair");

        // The real-SCM_RIGHTS shape: one well-formed message with a
        // matching declared FD count — the reader must accept it and
        // adopt exactly that many descriptors.
        let batch = if rng.next_bool() { rng.below(3) + 1 } else { 0 };
        if batch > 0 {
            let words = 1 + rng.below(64) as u32;
            let msg = framed_message(&mut rng, words, batch as u16);
            let mut fds = FdList::new();
            for _ in 0..batch {
                fds.push(devnull_fd());
            }
            let mut writer = FramedWriter::without_hooks(limits);
            writer
                .send_msg(&mut tx, &msg, &mut fds)
                .expect("well-formed FD message must send");
            // The kernel duplicated the descriptors; our copies close
            // with `fds` here.
        }

        // The fuzzed tail: valid frames, sometimes corrupted, then
        // fragmented delivery.
        let mut stream: Vec<u8> = Vec::new();
        for _ in 0..=rng.below(3) {
            let words = rng.below(1024) as u32;
            stream.extend(framed_message(&mut rng, words, 0));
        }
        if rng.next_bool() {
            stream = mutate_bytes(&mut rng, &stream);
        }
        let chunks = chunk_stream(&mut rng, &stream, 6);
        // Crash shape: deliver only half the fragments, then vanish.
        let deliver = if rng.below(4) == 0 {
            chunks.len() / 2
        } else {
            chunks.len()
        };
        for chunk in chunks.iter().take(deliver) {
            let _ = tx.send_chunk(chunk, None);
        }
        drop(tx);

        // Receive until a structured verdict.
        let mut reader = FramedReader::new(limits);
        loop {
            match reader.recv_msg(&mut rx) {
                Ok(frame) => {
                    report.accepted += 1;
                    let bytes = frame.message_bytes();
                    assert!(
                        bytes.len() >= 16 && bytes.len() % 8 == 0,
                        "accepted a structurally impossible frame ({} bytes)",
                        bytes.len()
                    );
                    assert_eq!(
                        frame.fds.len(),
                        usize::from(frame.fd_count()),
                        "adopted FDs must match the declared count"
                    );
                }
                Err(e) => {
                    if is_would_block(&e) {
                        // Blocking sockets should not report this, but
                        // treat it as end-of-input rather than spin.
                        break;
                    }
                    report.rejected += 1;
                    break;
                }
            }
        }
        drop(rx);
    }

    // FD hygiene across the whole run (the leak exit criterion).
    std::thread::sleep(std::time::Duration::from_millis(10));
    let fd_end = count_open_fds().expect("fd count");
    assert_eq!(
        fd_baseline, fd_end,
        "FD table changed across {iterations} transport fuzz iterations (leak)"
    );
    report
}

/// One framed message of `words` payload words declaring `fd_count`.
fn framed_message(rng: &mut SplitMix64, words: u32, fd_count: u16) -> Vec<u8> {
    let mut msg = vec![0u8; 16 + words as usize * 8];
    msg[0..4].copy_from_slice(&words.to_le_bytes());
    msg[14..16].copy_from_slice(&fd_count.to_le_bytes());
    for b in &mut msg[16..] {
        *b = rng.byte();
    }
    msg
}

/// A cheap, always-available descriptor for SCM_RIGHTS traffic.
fn devnull_fd() -> OwnedFd {
    let file = std::fs::File::open("/dev/null").expect("open /dev/null");
    file.into()
}
