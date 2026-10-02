//! Peer credentials (`SO_PEERCRED`).
//!
//! `docs/architecture.md` §5: the server reads the peer identity
//! *before* any protocol data is processed; the `welcome` event then
//! carries the security verdict derived from it. [`PeerCreds`] is that
//! identity: the kernel's answer, unforgeable by the client.

#![forbid(unsafe_code)]

use ldp_core::error::Result;
use std::os::fd::RawFd;

/// The `SO_PEERCRED` identity of a connected AF_UNIX socket.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct PeerCreds {
    /// Process ID of the peer (0 when the peer is in another PID
    /// namespace and its ID is not visible here).
    pub pid: i32,
    /// Real UID of the peer process.
    pub uid: u32,
    /// Real GID of the peer process.
    pub gid: u32,
}

impl PeerCreds {
    /// Credentials of *this* process (used to predict what the peer
    /// should see after `connect`).
    #[must_use]
    pub fn current() -> PeerCreds {
        PeerCreds {
            pid: std::process::id() as i32,
            uid: crate::sys::getuid(),
            gid: crate::sys::getgid(),
        }
    }

    /// Read `SO_PEERCRED` from a connected socket.
    ///
    /// # Errors
    ///
    /// [`LdpError::Io`](ldp_core::error::LdpError::Io) on `getsockopt` failure (not a connected
    /// AF_UNIX socket, or the kernel predates 2.2 — both impossible in
    /// practice on supported targets).
    pub fn of_socket(fd: RawFd) -> Result<PeerCreds> {
        crate::sys::peer_cred(fd)
    }

    /// Whether the peer runs under the same UID (a quick same-user check
    /// used by policy layers before manifests are even consulted).
    #[must_use]
    pub fn same_user(self) -> bool {
        self.uid == PeerCreds::current().uid
    }
}

impl std::fmt::Display for PeerCreds {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "pid {} uid {} gid {}", self.pid, self.uid, self.gid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ldp_core::error::LdpError;
    use std::os::fd::AsRawFd;

    #[test]
    fn current_matches_process_identity() {
        let c = PeerCreds::current();
        assert_eq!(c.pid, std::process::id() as i32);
        assert!(c.uid < u32::MAX, "uid should be a real id");
    }

    #[test]
    fn same_user_holds_for_self() {
        assert!(PeerCreds::current().same_user());
        assert!(!PeerCreds {
            pid: 1,
            uid: u32::MAX,
            gid: 0
        }
        .same_user());
    }

    #[test]
    fn display_informative() {
        let c = PeerCreds {
            pid: 42,
            uid: 1000,
            gid: 1000,
        };
        assert_eq!(c.to_string(), "pid 42 uid 1000 gid 1000");
    }

    #[test]
    fn peer_cred_of_non_socket_fails() {
        // A regular file fd is not a socket: getsockopt must fail without
        // UB. Create one, try, close.
        let file = std::fs::File::open("/dev/null").unwrap();
        let err = PeerCreds::of_socket(file.as_raw_fd());
        assert!(matches!(err, Err(LdpError::Io(_))));
    }
}
