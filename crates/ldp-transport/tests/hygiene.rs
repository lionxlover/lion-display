//! Phase 3 exit-criteria suite (serialized): the FD-count leak gate.
//!
//! `count_open_fds` is a *process-wide* snapshot, so every test in this
//! binary serializes on one mutex: while a leak assertion is in flight,
//! no other test may open or close descriptors. `cargo test` runs test
//! *binaries* sequentially, so this mutex is sufficient isolation.
//!
//! Covered here:
//! * 10,000 FD-passing round-trips (`docs/roadmap.md` Phase 3 EC 1),
//! * the malformed-frame corpus — every case rejected with the right
//!   wire error and the reader poisoned (EC 2),
//! * `SO_PEERCRED` identity both directions,
//! * the FD table is exactly unchanged after all of it (EC 4).

#![allow(clippy::similar_names)]

use ldp_core::error::LdpError;
use ldp_core::limits::Limits;
use ldp_transport::{
    count_open_fds, error, sys, FdList, FramedReader, FramedWriter, NoHooks, PeerCreds,
    TransportListener, TransportStream, UnixAddr,
};
use std::os::fd::AsRawFd;
use std::sync::{Mutex, MutexGuard};

/// Serializes every FD-count-sensitive test in this binary.
static HYGIENE: Mutex<()> = Mutex::new(());

fn lock() -> MutexGuard<'static, ()> {
    HYGIENE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A unique abstract address per binary run.
fn addr(tag: &str) -> UnixAddr {
    let name = format!("\u{0}ldp-hygiene-{tag}-{}", std::process::id());
    UnixAddr::abstract_name(name.as_bytes()).unwrap()
}

/// A 24-byte message (1 payload word) with `fd_count` in the header and
/// counter-derived payload so reordering cannot hide.
fn msg(counter: u32, fd_count: u16) -> Vec<u8> {
    let mut m = vec![0u8; 24];
    m[0..4].copy_from_slice(&1u32.to_le_bytes());
    m[4..8].copy_from_slice(&counter.to_le_bytes());
    m[8..12].copy_from_slice(&(counter.wrapping_mul(3) | 1).to_le_bytes());
    m[14..16].copy_from_slice(&fd_count.to_le_bytes());
    m[16..24].copy_from_slice(&counter.to_le_bytes().repeat(2));
    m
}

/// A full listener/client/server triple over an abstract socket.
fn connected(tag: &str) -> (TransportStream, TransportStream, PeerCreds) {
    let a = addr(tag);
    let listener = TransportListener::bind(&a, 4).unwrap();
    let client = TransportStream::connect(&a).unwrap();
    let (server, creds) = listener.accept().unwrap();
    (client, server, creds)
}

#[test]
fn roundtrips_10k_messages_with_fds() {
    const N: u32 = 10_000;
    let _guard = lock();
    let (mut client, mut server, creds) = connected("10k");
    assert_eq!(creds.pid, std::process::id() as i32);
    assert!(creds.same_user());
    // The client verifies the server's identity symmetrically.
    assert_eq!(client.peer_creds().unwrap().pid, std::process::id() as i32);

    let mut writer = FramedWriter::new(Limits::DEFAULT, NoHooks);
    let mut reader = FramedReader::new(Limits::DEFAULT);
    let before = count_open_fds().unwrap();

    for i in 0..N {
        // Nine of ten messages carry 3 FDs; every tenth carries none —
        // both paths stay exercised over the full run.
        let fd_count: u16 = if i % 10 == 0 { 0 } else { 3 };
        round_trip_once(
            &mut writer,
            &mut reader,
            &mut server,
            &mut client,
            i,
            fd_count,
        );
        if i % 500 == 0 {
            assert_eq!(count_open_fds().unwrap(), before, "stable at {i}");
        }
    }
    assert_eq!(
        count_open_fds().unwrap(),
        before,
        "10k round-trips leave the FD table unchanged"
    );
}

#[test]
fn malformed_frames_rejected() {
    let _guard = lock();
    let before = count_open_fds().unwrap();

    // Oversize frame: claims 200k payload words (1.6 MB > 1 MiB).
    {
        let (mut client, mut server, _) = connected("oversize");
        let mut m = vec![0u8; 16];
        m[0..4].copy_from_slice(&200_000u32.to_le_bytes());
        m[14..16].copy_from_slice(&0u16.to_le_bytes());
        server.set_nonblocking(true).unwrap();
        let _ = server.send_chunk(&m, None);
        let mut reader = FramedReader::new(Limits::DEFAULT);
        let e = reader.recv_msg(&mut client).unwrap_err();
        assert_eq!(
            e.wire_code(),
            Some(ldp_core::error::ErrorCode::LimitExceeded)
        );
        assert!(reader.is_poisoned());
        let e2 = reader.recv_msg(&mut client).unwrap_err();
        assert!(matches!(e2, LdpError::Logic { .. }));
    }

    // Header claims 4 FDs, none travel.
    {
        let (mut client, mut server, _) = connected("claim4");
        server.send_chunk(&msg(1, 4), None).unwrap();
        let mut reader = FramedReader::new(Limits::DEFAULT);
        let e = reader.recv_msg(&mut client).unwrap_err();
        assert_eq!(e.wire_code(), Some(ldp_core::error::ErrorCode::FdMismatch));
        assert!(reader.is_poisoned());
    }

    // Header claims 0 FDs, two travel: both must be closed by us.
    {
        let (mut client, mut server, _) = connected("claim0");
        let e1 = sys::eventfd_owned().unwrap();
        let e2 = sys::eventfd_owned().unwrap();
        let mut ctl = sys::ControlBuffer::with_fd_capacity(4);
        let used = ctl
            .encode_rights(&[e1.as_raw_fd(), e2.as_raw_fd()])
            .unwrap();
        server
            .send_chunk(&msg(2, 0), Some(ctl.encoded_prefix(used)))
            .unwrap();
        drop((e1, e2));
        let mut reader = FramedReader::new(Limits::DEFAULT);
        let e = reader.recv_msg(&mut client).unwrap_err();
        assert_eq!(e.wire_code(), Some(ldp_core::error::ErrorCode::FdMismatch));
        drop((client, server));
        assert_eq!(count_open_fds().unwrap(), before, "smuggled fds closed");
    }
}

#[test]
fn malformed_fd_cases_rejected() {
    let _guard = lock();
    let before = count_open_fds().unwrap();

    // fd_count above the per-message limit: the two traveling FDs close.
    {
        let (mut client, mut server, _) = connected("over64");
        let e1 = sys::eventfd_owned().unwrap();
        let e2 = sys::eventfd_owned().unwrap();
        let mut ctl = sys::ControlBuffer::with_fd_capacity(4);
        let used = ctl
            .encode_rights(&[e1.as_raw_fd(), e2.as_raw_fd()])
            .unwrap();
        let m = msg(3, 65);
        server
            .send_chunk(&m, Some(ctl.encoded_prefix(used)))
            .unwrap();
        drop((e1, e2));
        let mut reader = FramedReader::new(Limits::DEFAULT);
        let e = reader.recv_msg(&mut client).unwrap_err();
        assert_eq!(
            e.wire_code(),
            Some(ldp_core::error::ErrorCode::LimitExceeded)
        );
        drop((client, server));
        assert_eq!(count_open_fds().unwrap(), before);
    }

    // FDs attached mid-payload of the next message: kernel closes them
    // (MSG_CTRUNC on a control-less read), reader rejects the message.
    {
        let (mut client, mut server, _) = connected("midpay");
        let e3 = sys::eventfd_owned().unwrap();
        let mut ctl = sys::ControlBuffer::with_fd_capacity(4);
        // Header only, then payload with the smuggled control.
        let m = msg(4, 0);
        server.send_chunk(&m[..16], None).unwrap();
        let used = ctl.encode_rights(&[e3.as_raw_fd()]).unwrap();
        server
            .send_chunk(&m[16..], Some(ctl.encoded_prefix(used)))
            .unwrap();
        drop(e3);
        let mut reader = FramedReader::new(Limits::DEFAULT);
        let e = reader.recv_msg(&mut client).unwrap_err();
        assert_eq!(e.wire_code(), Some(ldp_core::error::ErrorCode::FdMismatch));
        drop((client, server));
        assert_eq!(
            count_open_fds().unwrap(),
            before,
            "kernel closed the smuggled fd"
        );
    }

    // Truncated message then orderly close: a disconnect, not a protocol
    // error — but still no FD leak and no panic.
    {
        let (mut client, mut server, _) = connected("trunc");
        let e = sys::eventfd_owned().unwrap();
        let mut ctl = sys::ControlBuffer::with_fd_capacity(4);
        let used = ctl.encode_rights(&[e.as_raw_fd()]).unwrap();
        let m = msg(5, 1); // 8 payload bytes promised
        server
            .send_chunk(&m[..20], Some(ctl.encoded_prefix(used)))
            .unwrap();
        drop(e);
        drop(server);
        let mut reader = FramedReader::new(Limits::DEFAULT);
        let err = reader.recv_msg(&mut client).unwrap_err();
        assert!(error::is_disconnect(&err), "mid-message close: {err}");
        drop(client);
        assert_eq!(count_open_fds().unwrap(), before);
    }

    assert_eq!(
        count_open_fds().unwrap(),
        before,
        "corpus leaves no fds behind"
    );
}

#[test]
fn zero_length_and_boundary_frames_round_trip() {
    let _guard = lock();
    let before = count_open_fds().unwrap();
    let (mut client, mut server, _) = connected("bounds");

    // Header-only, 64-fd boundary, and a maximal 1 MiB frame. The 1 MiB
    // frame needs several sendmsg chunks on a blocking stream, so the
    // peer reads from a thread while this side drives the flushes.
    let messages: Vec<(u32, u16, Vec<u8>)> = [0u32, 1, 131_070]
        .iter()
        .map(|&words| {
            let fd_count: u16 = if words == 0 { 0 } else { 64 };
            let mut m = vec![0u8; 16 + words as usize * 8];
            m[0..4].copy_from_slice(&words.to_le_bytes());
            m[14..16].copy_from_slice(&fd_count.to_le_bytes());
            for (i, b) in m[16..].iter_mut().enumerate() {
                *b = (i % 247) as u8;
            }
            (words, fd_count, m)
        })
        .collect();

    let (tx, rx) = std::sync::mpsc::channel::<(Vec<u8>, usize)>();
    let peer = std::thread::spawn(move || {
        let mut reader = FramedReader::new(Limits::DEFAULT);
        for _ in 0..3 {
            let frame = reader.recv_msg(&mut client).unwrap();
            tx.send((frame.message_bytes().to_vec(), frame.fds.len()))
                .unwrap();
        }
    });

    let mut writer = FramedWriter::new(Limits::DEFAULT, NoHooks);
    for (_, fd_count, m) in &messages {
        let mut fds = FdList::new();
        for _ in 0..usize::from(*fd_count) {
            fds.push(sys::eventfd_owned().unwrap());
        }
        writer.send_msg(&mut server, m, &mut fds).unwrap();
        // Blocking streams: one chunk per flush; the peer thread drains.
        while matches!(
            writer.flush(&mut server).unwrap(),
            ldp_transport::FlushOutcome::Congested { .. }
        ) {}
    }
    peer.join().unwrap();
    drop(server);

    for (words, fd_count, m) in &messages {
        let (got, got_fds) = rx.recv().unwrap();
        assert_eq!(&got, m, "words={words} payload intact");
        assert_eq!(got_fds, usize::from(*fd_count));
    }
    assert_eq!(
        count_open_fds().unwrap(),
        before,
        "boundary cycle is leak-free"
    );
}

/// One iteration of the 10k loop: send, receive, verify, close.
fn round_trip_once(
    writer: &mut FramedWriter<NoHooks>,
    reader: &mut FramedReader,
    server: &mut TransportStream,
    client: &mut TransportStream,
    counter: u32,
    fd_count: u16,
) {
    let m = msg(counter, fd_count);
    let mut fds = FdList::new();
    for _ in 0..usize::from(fd_count) {
        fds.push(sys::eventfd_owned().unwrap());
    }
    writer.send_msg(server, &m, &mut fds).unwrap();
    assert!(fds.is_empty(), "the writer consumes the batch");

    let frame = reader.recv_msg(client).unwrap();
    assert_eq!(frame.message_bytes(), &m[..], "payload intact at {counter}");
    assert_eq!(frame.fds.len(), usize::from(fd_count));
    // Dropping the frame closes its FDs — the whole loop must return
    // the process to the exact starting count.
    drop(frame);
}

#[test]
fn credentials_are_kernel_verified() {
    let _guard = lock();
    let a = addr("creds");
    let listener = TransportListener::bind(&a, 4).unwrap();
    let client = TransportStream::connect(&a).unwrap();
    let (_server, creds) = listener.accept().unwrap();
    let self_creds = PeerCreds::current();
    assert_eq!(creds, self_creds, "SO_PEERCRED mirrors our identity");
    assert_eq!(client.peer_creds().unwrap(), self_creds);
    // The same verdict from the other side.
    let back = client.peer_creds().unwrap();
    assert_eq!(back.uid, sys::getuid());
    assert_eq!(back.gid, sys::getgid());
}
