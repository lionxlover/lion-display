//! A physical-bound sanity check for the transport benchmark numbers.
//!
//! The suite once reported ~496 GiB/s for a 64 KiB socketpair
//! round-trip — sixty times what the hardware can do — because the
//! benchmark divided a single round-trip's duration by an
//! `ops_per_run` the closure never performed. This example measures
//! the round-trip directly and asserts the rate lands inside
//! physically possible bounds, so that whole class of harness bug
//! fails loudly instead of publishing nonsense.
//!
//! Run: `cargo run -p ldp-test --release --example transport_check`

use ldp_core::limits::Limits;
use ldp_transport::{FdList, FramedReader, FramedWriter, TransportStream};

/// A framed message of `words` payload words.
fn framed_message(words: u32) -> Vec<u8> {
    let mut msg = vec![0u8; 16 + words as usize * 8];
    msg[0..4].copy_from_slice(&words.to_le_bytes());
    msg[14..16].copy_from_slice(&0u16.to_le_bytes());
    msg
}

fn main() {
    const N: usize = 2000;
    for words in [128u32, 8192] {
        let msg = framed_message(words);
        let (mut tx, mut rx) = TransportStream::pair().expect("pair");
        let mut writer = FramedWriter::without_hooks(Limits::DEFAULT);
        let mut reader = FramedReader::new(Limits::DEFAULT);
        // Warmup.
        writer.send_msg(&mut tx, &msg, &mut FdList::new()).unwrap();
        let _ = reader.recv_msg(&mut rx).unwrap();

        let t0 = std::time::Instant::now();
        for _ in 0..N {
            writer.send_msg(&mut tx, &msg, &mut FdList::new()).unwrap();
            let frame = reader.recv_msg(&mut rx).unwrap();
            assert_eq!(frame.payload_words(), words);
        }
        let dt = t0.elapsed();
        let mibs = N as f64 * msg.len() as f64 / dt.as_secs_f64() / (1024.0 * 1024.0);
        let per_trip_us = dt.as_secs_f64() * 1e6 / N as f64;
        println!("{words:5} words: {per_trip_us:7.2} us/trip, {mibs:8.0} MiB/s");
        // Physical bounds: a real sendmsg+recvmsg pair with two copies
        // can neither crawl below 50 MiB/s on this hardware class nor
        // exceed 64 GiB/s (roughly L1/L2 copy speed for both copies).
        assert!(mibs > 50.0, "suspiciously slow: {mibs} MiB/s");
        assert!(mibs < 64.0 * 1024.0, "physically impossible: {mibs} MiB/s");
    }
    println!("transport rates are physically sane");
}
