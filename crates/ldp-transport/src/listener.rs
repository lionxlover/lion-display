//! The accept side: bind, listen, accept with credentials.
//!
//! `docs/architecture.md` §6: one listener at
//! `$XDG_RUNTIME_DIR/ldp-<session>/ldp.sock`; `SO_PEERCRED` is read at
//! accept time, *before* any protocol data is processed. Accepted
//! sockets arrive `SOCK_CLOEXEC` (never leak connections to children)
//! and inherit the listener's blocking mode.
//!
//! Filesystem-path cleanup is the caller's job: dropping a listener
//! unlinks nothing (an unlink helper would race reconnecting clients;
//! session managers own the socket directory).

#![forbid(unsafe_code)]

use crate::addr::UnixAddr;
use crate::creds::PeerCreds;
use crate::stream::TransportStream;
use ldp_core::error::Result;
use ldp_core::limits::Limits;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};

/// A bound, listening AF_UNIX stream socket.
#[derive(Debug)]
pub struct TransportListener {
    fd: OwnedFd,
    nonblocking: bool,
}

impl TransportListener {
    /// Bind and listen on `addr` with `backlog` (see
    /// [`DEFAULT_BACKLOG`](crate::DEFAULT_BACKLOG)). The listener (and
    /// later accepts) start blocking; flip with
    /// [`set_nonblocking`](TransportListener::set_nonblocking) for event
    /// loops.
    ///
    /// # Errors
    ///
    /// [`LdpError::Io`](ldp_core::error::LdpError::Io)(ldp_core::error::LdpError::Io) on `bind(2)` /
    /// `listen(2)` failure (EADDRINUSE for a live filesystem path; the
    /// parent directory must already exist).
    pub fn bind(addr: &UnixAddr, backlog: i32) -> Result<TransportListener> {
        let fd = crate::sys::raw_socket(false)?;
        crate::sys::raw_bind(fd.as_raw_fd(), addr)?;
        crate::sys::raw_listen(fd.as_raw_fd(), backlog)?;
        Ok(TransportListener {
            fd,
            nonblocking: false,
        })
    }

    /// Accept one connection and read its credentials.
    ///
    /// The returned stream matches this listener's blocking mode. On a
    /// nonblocking listener with nothing pending this fails with
    /// [`WouldBlock`](crate::error::is_would_block).
    ///
    /// # Errors
    ///
    /// [`LdpError::Io`](ldp_core::error::LdpError::Io)(ldp_core::error::LdpError::Io) on `accept4(2)`
    /// failure, or when credential retrieval fails (the accepted fd is
    /// closed in that case — a connection without identity is refused
    /// before any byte of it is read).
    pub fn accept(&self) -> Result<(TransportStream, PeerCreds)> {
        let fd = crate::sys::raw_accept4(self.fd.as_raw_fd(), self.nonblocking)?;
        let creds = match crate::sys::peer_cred(fd.as_raw_fd()) {
            Ok(c) => c,
            Err(e) => {
                // Drop closes the accepted fd: identity must precede I/O.
                drop(fd);
                return Err(e);
            }
        };
        let stream = TransportStream::new(fd)?;
        Ok((stream, creds))
    }

    /// Flip the accept (and inherited) blocking mode.
    ///
    /// # Errors
    ///
    /// [`LdpError::Io`](ldp_core::error::LdpError::Io)(ldp_core::error::LdpError::Io) on fcntl failure.
    pub fn set_nonblocking(&mut self, on: bool) -> Result<()> {
        crate::sys::set_nonblocking(self.fd.as_raw_fd(), on)?;
        self.nonblocking = on;
        Ok(())
    }

    /// Whether accepts are currently nonblocking.
    #[must_use]
    pub fn is_nonblocking(&self) -> bool {
        self.nonblocking
    }

    /// Raw fd for `epoll` registration (borrow only).
    #[must_use]
    pub fn raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }

    /// The negotiated limits this listener will hand to accepted
    /// connections (documented convenience — limits themselves live in
    /// the handshake, `docs/protocol.md` §6).
    #[must_use]
    pub fn default_limits() -> Limits {
        Limits::DEFAULT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn abstract_addr(tag: &str) -> UnixAddr {
        let name = format!("ldp-listener-{tag}-{}", std::process::id());
        UnixAddr::abstract_name(name.as_bytes()).unwrap()
    }

    #[test]
    fn accept_returns_peer_credentials() {
        let addr = abstract_addr("creds");
        let listener = TransportListener::bind(&addr, 4).unwrap();
        let client = TransportStream::connect(&addr).unwrap();
        let (server, creds) = listener.accept().unwrap();
        assert_eq!(creds.pid, std::process::id() as i32);
        assert!(creds.same_user());
        // The client can also verify the server's identity.
        let server_creds = client.peer_creds().unwrap();
        assert_eq!(server_creds.pid, std::process::id() as i32);
        drop(server);
    }

    #[test]
    fn abstract_bind_is_session_scoped() {
        let a = abstract_addr("distinct");
        let b = abstract_addr("distinct2");
        let l1 = TransportListener::bind(&a, 2).unwrap();
        // The same name again fails; a distinct name coexists.
        assert!(TransportListener::bind(&a, 2).is_err());
        let _l2 = TransportListener::bind(&b, 2).unwrap();
        drop(l1);
    }

    #[test]
    fn nonblocking_accept_reports_would_block() {
        let addr = abstract_addr("nb");
        let mut listener = TransportListener::bind(&addr, 2).unwrap();
        listener.set_nonblocking(true).unwrap();
        let e = listener.accept().unwrap_err();
        assert!(crate::error::is_would_block(&e));
        // After a connect arrives, accept succeeds and the stream is
        // nonblocking.
        let _client = TransportStream::connect(&addr).unwrap();
        let (stream, _creds) = listener.accept().unwrap();
        assert!(stream.is_nonblocking());
    }

    #[test]
    fn filesystem_path_listener_round_trip() {
        let path = std::env::temp_dir().join(format!("ldp-test-{}.sock", std::process::id()));
        let addr = UnixAddr::path(&path).unwrap();
        let listener = TransportListener::bind(&addr, 2).unwrap();
        let _client = TransportStream::connect(&addr).unwrap();
        let (_server, creds) = listener.accept().unwrap();
        assert_eq!(creds.pid, std::process::id() as i32);
        drop(listener);
        std::fs::remove_file(&path).unwrap();
    }
}
