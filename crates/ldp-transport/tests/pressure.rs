//! Phase 3 exit-criteria suite (pressure): FD bombs and slow peers.
//!
//! Both tests are FD-count sensitive and serialize on one mutex (see
//! `tests/hygiene.rs` for the counting rationale).
//!
//! * **FD bomb**: a hostile sender smuggles 200 FDs in one control
//!   message (the kernel allows up to 253 per cmsg) while the header
//!   claims a legal count. The receiver's control buffer holds at most
//!   `Limits::fds_per_message` (64): the kernel closes the overflow,
//!   the reader closes what was delivered, the connection dies with
//!   `fd_mismatch`, and the process FD table is byte-identical after.
//! * **Slow peer**: a nonblocking writer against a peer that never
//!   reads. Congestion engages, the local queue parks at its ceiling,
//!   further sends are rejected (the server's cue to coalesce or drop),
//!   then the drain verifies ordering and recovery.

use ldp_core::limits::Limits;
use ldp_transport::backpressure::{HookEvent, HookRecorder};
use ldp_transport::{
    count_open_fds, error, sys, FdList, FlushOutcome, FramedReader, FramedWriter, SendOutcome,
    TransportListener, TransportStream, UnixAddr,
};
use std::os::fd::IntoRawFd;
use std::sync::{Mutex, MutexGuard};

/// Serializes FD-count-sensitive tests in this binary.
static PRESSURE: Mutex<()> = Mutex::new(());

fn lock() -> MutexGuard<'static, ()> {
    PRESSURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn addr(tag: &str) -> UnixAddr {
    let name = format!("\u{0}ldp-pressure-{tag}-{}", std::process::id());
    UnixAddr::abstract_name(name.as_bytes()).unwrap()
}

fn connected(tag: &str) -> (TransportStream, TransportStream) {
    let a = addr(tag);
    let listener = TransportListener::bind(&a, 4).unwrap();
    let client = TransportStream::connect(&a).unwrap();
    let (server, _) = listener.accept().unwrap();
    (client, server)
}

/// Build one raw SCM_RIGHTS control message carrying `fds` (native
/// endianness) — the hostile sender's forging tool.
fn forge_rights(fds: &[i32]) -> Vec<u8> {
    let mut ctl = vec![0u8; 16 + fds.len() * 4];
    ctl[0..8].copy_from_slice(&(16 + fds.len() * 4).to_ne_bytes());
    ctl[8..12].copy_from_slice(&libc::SOL_SOCKET.to_ne_bytes());
    ctl[12..16].copy_from_slice(&libc::SCM_RIGHTS.to_ne_bytes());
    for (i, fd) in fds.iter().enumerate() {
        ctl[16 + i * 4..16 + i * 4 + 4].copy_from_slice(&fd.to_ne_bytes());
    }
    ctl
}

#[test]
fn fd_bomb_200_fds_rejected_and_closed() {
    let _guard = lock();
    let before = count_open_fds().unwrap();
    let (mut client, mut server) = connected("bomb");

    // 200 live fds (kernel cap is 253 per cmsg — this send is legal at
    // the syscall layer, which is exactly why receivers must defend).
    let bombs: Vec<i32> = (0..200)
        .map(|_| sys::eventfd_owned().unwrap().into_raw_fd())
        .collect();
    let ctl = forge_rights(&bombs);

    // A header that claims a perfectly legal 2 FDs.
    let mut m = vec![0u8; 24];
    m[0..4].copy_from_slice(&1u32.to_le_bytes());
    m[14..16].copy_from_slice(&2u16.to_le_bytes());
    server.send_chunk(&m, Some(&ctl)).unwrap();
    // The sender side's originals close now; the kernel holds its own
    // references in the socket queue until delivery or socket death.
    for fd in bombs {
        sys::close_raw(fd);
    }

    let mut reader = FramedReader::new(Limits::DEFAULT);
    let e = reader.recv_msg(&mut client).unwrap_err();
    assert_eq!(
        e.wire_code(),
        Some(ldp_core::error::ErrorCode::FdMismatch),
        "control truncation is an fd_mismatch: {e}"
    );
    assert!(reader.is_poisoned());
    // Drop both ends: the kernel releases any queued references.
    drop((server, client, reader));
    assert_eq!(
        count_open_fds().unwrap(),
        before,
        "delivered fds closed by the reader, overflow closed by the kernel"
    );
}

#[test]
fn fd_bomb_above_kernel_cap_never_leaves_the_sender() {
    let _guard = lock();
    // The writer refuses batches above SCM_MAX_FD locally: nothing
    // reaches the socket, so the kernel can never truncate a send.
    let (mut client, mut server) = connected("cap");
    let mut writer = FramedWriter::new(Limits::DEFAULT, ldp_transport::NoHooks);
    let mut m = vec![0u8; 24];
    m[0..4].copy_from_slice(&1u32.to_le_bytes());
    m[14..16].copy_from_slice(&254u16.to_le_bytes());
    let mut fds = FdList::new();
    for _ in 0..254 {
        fds.push(sys::eventfd_owned().unwrap());
    }
    let e = writer.send_msg(&mut server, &m, &mut fds).unwrap_err();
    assert_eq!(
        e.wire_code(),
        Some(ldp_core::error::ErrorCode::LimitExceeded)
    );
    assert_eq!(fds.len(), 254, "rejected batch is not consumed");
    drop(fds);
    // The peer sees nothing: the socket is empty (WouldBlock on a
    // nonblocking stream), so the kernel never received any FDs.
    client.set_nonblocking(true).unwrap();
    let mut buf = [0u8; 8];
    match client.recv_chunk_plain(&mut buf) {
        Err(e) => assert!(error::is_would_block(&e)),
        Ok(r) => panic!("unexpected data: {} bytes", r.bytes),
    }
}

#[test]
fn slow_peer_is_bounded_and_recovers() {
    let _guard = lock();
    let before = count_open_fds().unwrap();
    let (mut client, mut server) = connected("slow");
    server.set_nonblocking(true).unwrap();
    client.set_nonblocking(true).unwrap();

    // 96-byte ceiling: the queue parks almost immediately once the
    // socket buffer fills; further messages are rejected, not buffered.
    // (The message ceiling stays at/below the queue ceiling — a legal
    // message is always queueable, see `BackpressureConfig::from_limits`.)
    let limits = Limits {
        message_bytes: 96,
        event_queue_bytes: 96,
        ..Limits::DEFAULT
    };
    let recorder = HookRecorder::new();
    let mut writer = FramedWriter::new(limits, recorder);
    let mut reader = FramedReader::new(Limits {
        message_bytes: 1 << 20,
        ..Limits::DEFAULT
    });

    let mut sent: u32 = 0;
    let mut rejected = 0;
    for i in 0..20_000u32 {
        let mut m = vec![0u8; 32];
        m[0..4].copy_from_slice(&2u32.to_le_bytes());
        m[4..8].copy_from_slice(&i.to_le_bytes());
        m[14..16].copy_from_slice(&0u16.to_le_bytes());
        m[16..32].copy_from_slice(&i.to_le_bytes().repeat(4));
        match writer.send_msg(&mut server, &m, &mut FdList::new()) {
            Ok(SendOutcome::Sent | SendOutcome::Congested { .. }) => sent += 1,
            Err(e) => {
                assert_eq!(
                    e.wire_code(),
                    Some(ldp_core::error::ErrorCode::LimitExceeded)
                );
                rejected += 1;
                break;
            }
        }
        assert!(writer.pending_bytes() <= 96, "queue respects the ceiling");
    }
    assert!(rejected == 1, "the ceiling must eventually reject");
    assert!(writer.is_congested());
    assert!(writer
        .hooks()
        .events()
        .iter()
        .any(|e| matches!(e, HookEvent::Congestion(_))));

    // Drain: every accepted message arrives exactly once, in order, and
    // the writer recovers (hooks: drain below low water, then recovery).
    let mut received: u32 = 0;
    let mut drained = false;
    for _ in 0..200 {
        if matches!(writer.flush(&mut server).unwrap(), FlushOutcome::Drained) {
            drained = true;
        }
        loop {
            match reader.recv_msg(&mut client) {
                Ok(frame) => {
                    let expect: u32 =
                        u32::from_le_bytes(frame.message_bytes()[4..8].try_into().unwrap());
                    assert_eq!(expect, received, "strict ordering");
                    let payload: [u8; 4] = frame.message_bytes()[16..20].try_into().unwrap();
                    assert_eq!(u32::from_le_bytes(payload), received);
                    received += 1;
                }
                Err(e) if error::is_would_block(&e) => break,
                Err(e) => panic!("{e}"),
            }
        }
        if drained && received >= sent {
            break;
        }
    }
    assert_eq!(received, sent, "no message lost, none duplicated");
    assert!(writer
        .hooks()
        .events()
        .iter()
        .any(|e| matches!(e, HookEvent::Recovery)));
    drop((server, client, reader, writer));
    assert_eq!(
        count_open_fds().unwrap(),
        before,
        "slow-peer cycle leaves the FD table unchanged"
    );
}
