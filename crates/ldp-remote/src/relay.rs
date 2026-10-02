//! Relay internals shared by the edge and hub gateways (not public
//! API): framed send with congestion drain, pipe stream pumps, the
//! LDP header FD-count reader, and teardown plumbing.

#![forbid(unsafe_code)]

use std::io::{ErrorKind, Read as _, Write as _};
use std::net::TcpStream;
use std::os::fd::OwnedFd;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use ldp_core::error::{ErrorCode, LdpError, Result};
use ldp_transport::{FdList, FramedWriter, NoHooks, TransportStream};

use crate::link::{Liveness, SocketSet};
use crate::wire::Envelope;

/// Stream-pump chunk size (bytes): small enough to sit well under any
/// sane envelope cap, large enough to keep a clipboard transfer to a
/// few thousand envelopes per MiB.
pub(crate) const STREAM_CHUNK_BYTES: usize = 16 * 1024;

/// How long session teardown waits for each detached stream pump
/// before letting it go (the pump exits on pipe EOF; a wedged peer
/// would otherwise pin the session join).
pub(crate) const STREAM_PUMP_JOIN_WAIT: Duration = Duration::from_secs(2);

/// Read the FD count out of an LDP message header (LE u16 at offset
/// 14) — the same field [`ldp_transport::frame`] cross-checks against
/// the ancillary array.
///
/// # Errors
/// [`LdpError::Malformed`] for buffers shorter than a header.
pub(crate) fn ldp_fd_count(message: &[u8]) -> Result<u16> {
    if message.len() < ldp_transport::HEADER_BYTES {
        return Err(LdpError::malformed(
            ErrorCode::MalformedMessage,
            format!(
                "remote relay: DATA envelope carries {} bytes, shorter than an LDP header",
                message.len()
            ),
        ));
    }
    Ok(u16::from_le_bytes([
        message[ldp_transport::HEADER_BYTES - 2],
        message[ldp_transport::HEADER_BYTES - 1],
    ]))
}

/// Send one framed message, draining a congested queue (blocking
/// streams perform one `sendmsg` per call — the `ldp-client` pattern).
///
/// # Errors
/// The transport's own errors (socket failure, over-ceiling queues).
pub(crate) fn send_framed(
    writer: &mut FramedWriter<NoHooks>,
    stream: &mut TransportStream,
    message: &[u8],
    fds: &mut FdList,
) -> Result<()> {
    match writer.send_msg(stream, message, fds) {
        Ok(ldp_transport::SendOutcome::Sent) => Ok(()),
        Ok(ldp_transport::SendOutcome::Congested { .. }) => loop {
            match writer.flush(stream) {
                Ok(ldp_transport::FlushOutcome::Drained) => return Ok(()),
                Ok(ldp_transport::FlushOutcome::Congested { .. }) => {}
                Err(e) => return Err(e),
            }
        },
        Err(e) => Err(e),
    }
}

/// Append bytes to a descriptor (pipes: sequential writes, no seek).
///
/// # Errors
/// [`LdpError::Io`] on write failure.
pub(crate) fn write_append(fd: &OwnedFd, bytes: &[u8]) -> Result<()> {
    let mut sink = std::fs::File::from(
        fd.try_clone()
            .map_err(|e| LdpError::Io(Arc::new(std::io::Error::other(e))))?,
    );
    sink.write_all(bytes)
        .map_err(|e| LdpError::Io(Arc::new(std::io::Error::other(e))))?;
    Ok(())
}

/// Read an eventfd's counter snapshot without consuming the wakeup
/// first: poll, then (only if readable) drain the 8-byte counter. The
/// descriptor is going away — the relay ships its state, not itself.
///
/// # Errors
/// [`LdpError::Io`] on poll or read failure.
pub(crate) fn fence_counter(fd: &OwnedFd) -> Result<u64> {
    if !crate::sys::owned_poll_readable(fd)? {
        return Ok(0);
    }
    let mut file = std::fs::File::from(
        fd.try_clone()
            .map_err(|e| LdpError::Io(Arc::new(std::io::Error::other(e))))?,
    );
    let mut counter = [0u8; 8];
    file.read_exact(&mut counter)
        .map_err(|e| LdpError::Io(Arc::new(std::io::Error::other(e))))?;
    Ok(u64::from_le_bytes(counter))
}

/// Tear the session down: mark dead and unblock every pump.
pub(crate) fn teardown(liveness: &Liveness, sockets: &SocketSet) {
    liveness.kill();
    sockets.shutdown_all();
}

/// One pipe pump: read `fd` until EOF, shipping chunks under `id`,
/// then announce `STREAM_FIN`. Exits when the pipe closes (the peer
/// finished, or vanished and its descriptors closed) or when the
/// session's writer thread is gone.
pub(crate) fn stream_pump(read_fd: OwnedFd, id: u32, tx: Sender<Envelope>) {
    let mut file = std::fs::File::from(read_fd);
    let mut buf = vec![0u8; STREAM_CHUNK_BYTES];
    loop {
        match file.read(&mut buf) {
            Ok(0) => {
                let _ = tx.send(Envelope::stream_fin(id));
                return;
            }
            Ok(n) => {
                if tx.send(Envelope::stream_chunk(id, &buf[..n])).is_err() {
                    return;
                }
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(_) => return,
        }
    }
}

/// The session writer loop: serialize envelopes onto the TCP stream.
/// One owner per session, so envelope bytes never interleave. Exits
/// when the channel drains, the stream fails, or the session dies.
pub(crate) fn writer_loop(
    mut tcp: TcpStream,
    rx: Receiver<Envelope>,
    cap: usize,
    sockets: &SocketSet,
    liveness: &Liveness,
) {
    loop {
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(env) => {
                if env.write_to(&mut tcp, cap).is_err() {
                    teardown(liveness, sockets);
                    return;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if liveness.is_dead() {
                    return;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                // Drain what is left, then exit.
                while let Ok(env) = rx.try_recv() {
                    if env.write_to(&mut tcp, cap).is_err() {
                        teardown(liveness, sockets);
                        return;
                    }
                }
                return;
            }
        }
    }
}

/// Join a stream pump, waiting at most [`STREAM_PUMP_JOIN_WAIT`];
/// past the deadline the pump detaches (it exits on its pipe's EOF
/// eventually, and holds nothing but its own read end and a sender).
pub(crate) fn join_pump(handle: JoinHandle<()>) {
    let deadline = std::time::Instant::now() + STREAM_PUMP_JOIN_WAIT;
    while !handle.is_finished() {
        if std::time::Instant::now() >= deadline {
            // Detach: the pump is self-terminating by construction.
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let _ = handle.join();
}

/// A session's collected stream-pump handles (drained at teardown).
#[derive(Default, Debug)]
pub(crate) struct PumpHandles {
    handles: Mutex<Vec<JoinHandle<()>>>,
}

impl PumpHandles {
    /// Record a spawned pump.
    pub(crate) fn add(&self, handle: JoinHandle<()>) {
        self.handles.lock().expect("pump handles lock").push(handle);
    }

    /// Join (or detach) every recorded pump; called once at teardown.
    pub(crate) fn join_all(&self) {
        let drained: Vec<JoinHandle<()>> =
            std::mem::take(&mut *self.handles.lock().expect("pump handles lock"));
        for handle in drained {
            join_pump(handle);
        }
    }
}

/// Reply to a `PING` with a `PONG` (best-effort: the writer may be
/// gone during teardown).
pub(crate) fn answer_ping(env: &Envelope, tx: &Sender<Envelope>) -> Result<()> {
    let nonce = Envelope::parse_nonce(&env.body)?;
    tx.send(Envelope::pong(nonce)).map_err(|_| {
        LdpError::Io(Arc::new(std::io::Error::other(
            "ldp-remote: the writer thread has exited",
        )))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sys;
    use crate::wire::EnvelopeKind;

    #[test]
    fn ldp_fd_count_reads_the_header_field() {
        let mut msg = vec![0u8; 24];
        msg[14..16].copy_from_slice(&3u16.to_le_bytes());
        assert_eq!(ldp_fd_count(&msg).expect("count"), 3);
        assert!(ldp_fd_count(&msg[..10]).is_err());
    }

    #[test]
    fn stream_pump_ships_chunks_then_fin() {
        let (r, w) = sys::pipe().expect("pipe");
        let (tx, rx) = std::sync::mpsc::channel();
        let pump = std::thread::spawn(move || stream_pump(r, 7, tx));
        write_append(&w, b"hello ").expect("write");
        write_append(&w, b"pipe").expect("write");
        drop(w); // EOF
        pump.join().expect("pump");
        let mut collected = Vec::new();
        while let Ok(env) = rx.try_recv() {
            collected.push(env);
        }
        // Pipe reads may coalesce the two writes; assert the joined
        // payload and the terminator instead of a chunk count.
        assert_eq!(collected.last().expect("fin").kind, EnvelopeKind::StreamFin);
        let mut payload = Vec::new();
        for env in &collected[..collected.len() - 1] {
            assert_eq!(env.kind, EnvelopeKind::StreamChunk);
            let (_, bytes) = Envelope::parse_stream_chunk(&env.body).expect("chunk");
            payload.extend_from_slice(bytes);
        }
        assert_eq!(payload, b"hello pipe");
    }

    #[test]
    fn fence_counter_reads_state_without_losing_it() {
        let armed = sys::eventfd_with(9).expect("eventfd");
        assert_eq!(fence_counter(&armed).expect("counter"), 9);
        let cold = sys::eventfd_with(0).expect("eventfd");
        assert_eq!(fence_counter(&cold).expect("counter"), 0);
    }

    #[test]
    fn send_framed_round_trips_a_message_with_fds() {
        let (a, mut b) = ldp_transport::TransportStream::pair().expect("pair");
        let mut a = a;
        let mut writer = FramedWriter::new(ldp_core::limits::Limits::DEFAULT, NoHooks);
        let mut reader = framed_reader();
        let mut msg = vec![0u8; 24];
        msg[0..4].copy_from_slice(&1u32.to_le_bytes());
        msg[14..16].copy_from_slice(&1u16.to_le_bytes());
        let mut fds = FdList::new();
        fds.push(sys::eventfd_with(1).expect("fd"));
        send_framed(&mut writer, &mut a, &msg, &mut fds).expect("send");
        let frame = reader.recv_msg(&mut b).expect("recv");
        assert_eq!(frame.fd_count(), 1);
        assert_eq!(frame.message_bytes(), &msg[..]);
    }

    fn framed_reader() -> ldp_transport::FramedReader {
        ldp_transport::FramedReader::new(ldp_core::limits::Limits::DEFAULT)
    }
}
