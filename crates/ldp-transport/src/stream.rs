//! The connected stream: an owned `AF_UNIX` `SOCK_STREAM` fd.
//!
//! [`TransportStream`] is the RAII handle one side of a connection holds
//! after [`connect`](TransportStream::connect) or
//! [`accept`](crate::listener::TransportListener::accept). It tracks the
//! blocking mode (so event loops can flip a stream after handshake) and
//! delegates the message syscalls to [`sys`] — including the
//! control-buffer variants the framing layers use for `SCM_RIGHTS`.
//!
//! Partial-write semantics on stream sockets: a `sendmsg` may accept
//! only a prefix of the buffer. When FDs were attached, they transferred
//! with the *first* accepted byte and are gone; the continuation writes
//! carry no control data. [`FramedWriter`](crate::writer::FramedWriter)
//! encodes exactly that discipline.

#![forbid(unsafe_code)]

use crate::addr::UnixAddr;
use crate::sys;
use ldp_core::error::Result;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};

/// A connected `AF_UNIX` stream socket.
///
/// Closing (drop) closes the fd — a dropped stream is an orderly
/// disconnect for the peer mid-read. `SOCK_CLOEXEC` is always set, so an
/// `exec` never leaks the connection to children.
#[derive(Debug)]
pub struct TransportStream {
    fd: OwnedFd,
    nonblocking: bool,
}

impl TransportStream {
    /// Connect (blocking) to `addr` and return the stream.
    ///
    /// The stream starts blocking; call
    /// [`set_nonblocking`](TransportStream::set_nonblocking) to move it
    /// into an event loop.
    ///
    /// # Errors
    ///
    /// [`LdpError::Io`](ldp_core::error::LdpError::Io)(ldp_core::error::LdpError::Io) when nothing
    /// listens (ECONNREFUSED), the path is invalid, or `connect(2)`
    /// fails otherwise.
    pub fn connect(addr: &UnixAddr) -> Result<TransportStream> {
        let fd = sys::raw_socket(false)?;
        sys::raw_connect(fd.as_raw_fd(), addr)?;
        Ok(TransportStream {
            fd,
            nonblocking: false,
        })
    }

    /// An already-connected pair of streams over `socketpair(2)`
    /// (loopback transports, tests, in-process channels).
    ///
    /// # Errors
    ///
    /// [`LdpError::Io`](ldp_core::error::LdpError::Io)(ldp_core::error::LdpError::Io) on socketpair
    /// failure.
    pub fn pair() -> Result<(TransportStream, TransportStream)> {
        let (a, b) = sys::raw_socketpair()?;
        Ok((
            TransportStream {
                fd: a,
                nonblocking: false,
            },
            TransportStream {
                fd: b,
                nonblocking: false,
            },
        ))
    }

    /// Adopt an already-connected fd (must be an `AF_UNIX` stream
    /// socket; verified with `getsockopt(SO_TYPE)`).
    ///
    /// # Errors
    ///
    /// [`LdpError::Io`](ldp_core::error::LdpError::Io)(ldp_core::error::LdpError::Io) when the fd is
    /// not a stream socket. The fd is closed on error.
    pub fn new(fd: OwnedFd) -> Result<TransportStream> {
        let nonblocking = sys::is_nonblocking(fd.as_raw_fd())?;
        sys::assert_stream_socket(fd.as_raw_fd())?;
        Ok(TransportStream { fd, nonblocking })
    }

    /// Flip the blocking mode (`O_NONBLOCK` read-modify-write).
    ///
    /// # Errors
    ///
    /// [`LdpError::Io`](ldp_core::error::LdpError::Io)(ldp_core::error::LdpError::Io) on fcntl failure.
    pub fn set_nonblocking(&mut self, on: bool) -> Result<()> {
        sys::set_nonblocking(self.fd.as_raw_fd(), on)?;
        self.nonblocking = on;
        Ok(())
    }

    /// Whether the stream is currently nonblocking.
    #[must_use]
    pub fn is_nonblocking(&self) -> bool {
        self.nonblocking
    }

    /// The peer's credentials (`SO_PEERCRED`): kernel-verified identity
    /// of whoever holds the other end.
    ///
    /// # Errors
    ///
    /// [`LdpError::Io`](ldp_core::error::LdpError::Io)(ldp_core::error::LdpError::Io) when the socket
    /// is not connected.
    pub fn peer_creds(&self) -> Result<crate::creds::PeerCreds> {
        sys::peer_cred(self.fd.as_raw_fd())
    }

    /// Raw fd for `epoll`/`poll` registration (borrow only; ownership
    /// stays with the stream).
    #[must_use]
    pub fn raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }

    /// One `sendmsg`: `buf` bytes plus optional already-encoded control
    /// data. Returns bytes accepted (see module docs for the
    /// FDs-transfer-with-first-byte rule).
    ///
    /// # Errors
    ///
    /// [`LdpError::Io`](ldp_core::error::LdpError::Io)(ldp_core::error::LdpError::Io) on syscall
    /// failure (EAGAIN when the socket is full and nonblocking —
    /// control data untransferred).
    pub fn send_chunk(&mut self, buf: &[u8], control: Option<&[u8]>) -> Result<usize> {
        sys::send_msg(self.fd.as_raw_fd(), buf, control)
    }

    /// One `recvmsg` into `buf`, collecting ancillary data into
    /// `control`. Zero `bytes` = orderly EOF.
    ///
    /// # Errors
    ///
    /// [`LdpError::Io`](ldp_core::error::LdpError::Io)(ldp_core::error::LdpError::Io) on syscall
    /// failure (EAGAIN when nothing is readable and nonblocking).
    pub fn recv_chunk(
        &mut self,
        buf: &mut [u8],
        control: &mut sys::ControlBuffer,
    ) -> Result<sys::RawRecv> {
        sys::recv_with_control(self.fd.as_raw_fd(), buf, control)
    }

    /// One `recvmsg` into `buf` with no control buffer: ancillary data,
    /// if any, is closed by the kernel and reported as `truncated`.
    ///
    /// # Errors
    ///
    /// [`LdpError::Io`](ldp_core::error::LdpError::Io)(ldp_core::error::LdpError::Io) on syscall
    /// failure.
    pub fn recv_chunk_plain(&mut self, buf: &mut [u8]) -> Result<sys::RawRecv> {
        sys::recv_plain(self.fd.as_raw_fd(), buf)
    }

    /// Give up ownership of the fd (raw consumers).
    #[must_use]
    pub fn into_fd(self) -> OwnedFd {
        self.fd
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pair_streams_are_connected_and_creds_match() {
        let (mut tx, mut rx) = TransportStream::pair().unwrap();
        assert!(!tx.is_nonblocking());
        let creds = rx.peer_creds().unwrap();
        assert_eq!(creds.pid, std::process::id() as i32);
        assert!(creds.same_user());
        tx.send_chunk(b"ping", None).unwrap();
        let mut control = sys::ControlBuffer::with_fd_capacity(4);
        let mut buf = [0u8; 4];
        let got = rx.recv_chunk(&mut buf, &mut control).unwrap();
        assert_eq!(got.bytes, 4);
        assert_eq!(&buf, b"ping");
        assert_eq!(got.control_used, 0);
        assert!(!got.truncated);
    }

    #[test]
    fn adopt_rejects_non_socket() {
        // Regular file fds are not sockets: TransportStream::new must
        // reject them (SO_TYPE is not SOCK_STREAM) and close them.
        let tx: OwnedFd = std::fs::File::open("/dev/null").unwrap().into();
        let rx: OwnedFd = std::fs::File::open("/dev/null").unwrap().into();
        assert!(TransportStream::new(tx).is_err());
        assert!(TransportStream::new(rx).is_err());
    }

    #[test]
    fn adopt_accepts_socketpair_fd_and_tracks_mode() {
        let (fd_a, fd_b) = sys::raw_socketpair().unwrap();
        let raw_a = fd_a.as_raw_fd();
        let mut tx = TransportStream::new(fd_a).unwrap();
        let rx = TransportStream::new(fd_b).unwrap();
        assert!(!tx.is_nonblocking());
        tx.set_nonblocking(true).unwrap();
        assert!(tx.is_nonblocking());
        tx.set_nonblocking(false).unwrap();
        assert!(!tx.is_nonblocking());
        assert_eq!(tx.raw_fd(), raw_a);
        drop(rx);
    }

    #[test]
    fn nonblocking_recv_reports_would_block() {
        let (mut tx, mut rx) = TransportStream::pair().unwrap();
        tx.set_nonblocking(true).unwrap();
        let mut buf = [0u8; 4];
        let err = tx.recv_chunk_plain(&mut buf).unwrap_err();
        assert!(crate::error::is_would_block(&err));
        // The peer side still sees tx healthy connection.
        rx.send_chunk(b"x", None).unwrap();
        let got = tx.recv_chunk_plain(&mut buf).unwrap();
        assert_eq!(got.bytes, 1);
    }

    #[test]
    fn drop_closes_the_stream() {
        // A clean drop of both ends (no IO-safety abort) proves the
        // streams own their fds exactly once; the peer of tx dropped
        // stream sees an orderly EOF (zero bytes, not an error).
        let (tx, mut rx) = TransportStream::pair().unwrap();
        drop(tx);
        let mut buf = [0u8; 4];
        let got = rx.recv_chunk_plain(&mut buf).unwrap();
        assert_eq!(got.bytes, 0, "orderly EOF after the peer drops");
    }
}
