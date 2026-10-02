//! Socket addresses.
//!
//! `AF_UNIX` addresses in both Linux flavors: filesystem paths
//! (`$XDG_RUNTIME_DIR/ldp-<session>/ldp.sock` per `docs/architecture.md`
//! §6) and abstract namespace names (no filesystem footprint — used by
//! tests and by isolation-friendly local IPC). Encoding into
//! `sockaddr_un` is exact-length: abstract names occupy
//! `offsetof(sun_path) + 1 + len` bytes so two abstract names where one
//! is a prefix of the other stay distinct.

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

/// Linux `sun_path` capacity in bytes.
const SUN_PATH_LEN: usize = 108;

/// Offset of `sun_path` within `sockaddr_un` (family is `sa_family_t`, 2 bytes).
const SUN_PATH_OFFSET: usize = size_of::<libc::sa_family_t>();

/// An AF_UNIX socket address: filesystem path or abstract name.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum UnixAddr {
    /// A filesystem path (bound as a socket inode; unlinking is the
    /// caller's cleanup responsibility).
    Path(PathBuf),
    /// An abstract-namespace name (leading NUL implied by the variant;
    /// never touches the filesystem).
    Abstract(Vec<u8>),
}

impl UnixAddr {
    /// Abstract-namespace address from a name.
    ///
    /// # Errors
    ///
    /// [`LdpError::Limit`](ldp_core::error::LdpError::Limit)(ldp_core::error::LdpError::Limit) when the
    /// name exceeds 107 bytes (the abstract payload capacity of
    /// `sockaddr_un`).
    pub fn abstract_name(name: &[u8]) -> ldp_core::error::Result<UnixAddr> {
        if name.len() >= SUN_PATH_LEN {
            return Err(ldp_core::error::LdpError::Limit {
                kind: ldp_core::error::LimitKind::StringBytes,
                value: name.len() as u64,
            });
        }
        Ok(UnixAddr::Abstract(name.to_vec()))
    }

    /// Filesystem address from a path.
    ///
    /// # Errors
    ///
    /// [`LdpError::Limit`](ldp_core::error::LdpError::Limit)(ldp_core::error::LdpError::Limit) when the
    /// path (without NUL) exceeds 107 bytes.
    pub fn path(path: impl AsRef<Path>) -> ldp_core::error::Result<UnixAddr> {
        let bytes = path.as_ref().as_os_str().as_encoded_bytes();
        if bytes.len() >= SUN_PATH_LEN {
            return Err(ldp_core::error::LdpError::Limit {
                kind: ldp_core::error::LimitKind::StringBytes,
                value: bytes.len() as u64,
            });
        }
        Ok(UnixAddr::Path(path.as_ref().to_path_buf()))
    }

    /// The canonical LDP session socket path
    /// `$XDG_RUNTIME_DIR/ldp-<session>/ldp.sock` (falls back to
    /// `/tmp` when the variable is unset, matching common Wayland
    /// practice).
    ///
    /// # Errors
    ///
    /// [`LdpError::Limit`](ldp_core::error::LdpError::Limit)(ldp_core::error::LdpError::Limit) when the
    /// composed path exceeds the `sun_path` capacity.
    pub fn session_socket(session: &str) -> ldp_core::error::Result<UnixAddr> {
        let base = std::env::var_os("XDG_RUNTIME_DIR")
            .map_or_else(|| PathBuf::from("/tmp"), PathBuf::from);
        let path = base.join(format!("ldp-{session}")).join("ldp.sock");
        UnixAddr::path(path)
    }

    /// Rendered form for logs (`@name` for abstract, the path otherwise;
    /// abstract names show printable ASCII verbatim).
    #[must_use]
    pub fn display_string(&self) -> String {
        match self {
            UnixAddr::Path(p) => p.display().to_string(),
            UnixAddr::Abstract(n) => {
                let printable: Vec<u8> = n
                    .iter()
                    .copied()
                    .map(|b| {
                        if b.is_ascii_graphic() || b == b' ' {
                            b
                        } else {
                            b'?'
                        }
                    })
                    .collect();
                format!("@{}", String::from_utf8_lossy(&printable))
            }
        }
    }

    /// Byte length of the `sockaddr_un` encoding (exact length, not the
    /// full struct size).
    #[must_use]
    pub fn socklen(&self) -> usize {
        match self {
            UnixAddr::Path(p) => SUN_PATH_OFFSET + p.as_os_str().as_encoded_bytes().len() + 1,
            UnixAddr::Abstract(n) => SUN_PATH_OFFSET + 1 + n.len(),
        }
    }

    /// Fill a `sockaddr_un` (zeroed padding included) and return the
    /// exact `socklen` for the syscall.
    ///
    /// Writing the struct field-by-field keeps this safe: the address
    /// encoding itself never forms raw pointers or `unsafe` blocks.
    #[must_use]
    pub fn fill_sockaddr(&self, sun: &mut libc::sockaddr_un) -> libc::socklen_t {
        sun.sun_family = libc::AF_UNIX as libc::sa_family_t;
        sun.sun_path = [0; SUN_PATH_LEN];
        let len = match self {
            UnixAddr::Path(p) => {
                let bytes = p.as_os_str().as_encoded_bytes();
                put_bytes(&mut sun.sun_path, bytes);
                SUN_PATH_OFFSET + bytes.len() + 1
            }
            UnixAddr::Abstract(n) => {
                // sun_path[0] stays zero (the abstract marker); the name
                // follows it.
                put_bytes(&mut sun.sun_path[1..], n);
                SUN_PATH_OFFSET + 1 + n.len()
            }
        };
        len as libc::socklen_t
    }
}

/// Copy source bytes into a `sun_path` (c_char) slice, widening each byte.
/// Length agreement is the caller's job (bounded by [`SUN_PATH_LEN`]).
fn put_bytes(dst: &mut [libc::c_char], src: &[u8]) {
    for (d, s) in dst.iter_mut().zip(src) {
        *d = *s as libc::c_char;
    }
}

impl std::fmt::Display for UnixAddr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.display_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abstract_names_have_exact_lengths() {
        let a = UnixAddr::abstract_name(b"short").unwrap();
        // Capacity is 107 name bytes (marker takes sun_path[0]).
        assert!(UnixAddr::abstract_name(&[b'x'; 108]).is_err());
        assert!(UnixAddr::abstract_name(&[b'x'; 107]).is_ok());
        // family(2) + marker(1) + name
        assert_eq!(a.socklen(), 2 + 1 + 5);
    }

    #[test]
    fn prefix_abstract_names_stay_distinct() {
        let a = UnixAddr::abstract_name(b"foo").unwrap();
        let b = UnixAddr::abstract_name(b"foobar").unwrap();
        assert_ne!(a, b);
        assert_ne!(a.socklen(), b.socklen());
    }

    #[test]
    fn path_encoding_includes_nul() {
        let p = UnixAddr::path("/tmp/ldp.sock").unwrap();
        assert_eq!(p.socklen(), 2 + "/tmp/ldp.sock".len() + 1);
        let mut sun = libc::sockaddr_un {
            sun_family: 0xFFFF,
            sun_path: [0x5A; 108],
        };
        let len = p.fill_sockaddr(&mut sun);
        assert_eq!(len as usize, p.socklen());
        assert_eq!(
            &sun.sun_path[0..4],
            "/tmp"
                .as_bytes()
                .iter()
                .map(|&b| b as i8)
                .collect::<Vec<_>>()
        );
        assert_eq!(sun.sun_path[len as usize - 2], 0);
        assert!(sun.sun_path[len as usize - 1..].iter().all(|&b| b == 0));
    }

    #[test]
    fn abstract_encoding_keeps_marker_zero() {
        let a = UnixAddr::abstract_name(b"abc").unwrap();
        let mut sun = libc::sockaddr_un {
            sun_family: 0,
            sun_path: [0x5A; 108],
        };
        let len = a.fill_sockaddr(&mut sun);
        assert_eq!(len as usize, 6);
        assert_eq!(sun.sun_path[0], 0);
        assert_eq!(
            &sun.sun_path[1..4],
            b"abc".iter().map(|&b| b as i8).collect::<Vec<_>>()
        );
        // Bytes past the exact length are zero-padded.
        assert!(sun.sun_path[len as usize..].iter().all(|&b| b == 0));
    }

    #[test]
    fn family_field_is_af_unix() {
        let a = UnixAddr::abstract_name(b"z").unwrap();
        let mut sun = libc::sockaddr_un {
            sun_family: 0,
            sun_path: [0; 108],
        };
        let len = a.fill_sockaddr(&mut sun);
        assert_eq!(len, 4);
        assert_eq!(sun.sun_family, libc::AF_UNIX as libc::sa_family_t);
    }

    #[test]
    fn display_strings() {
        let a = UnixAddr::abstract_name(b"abc").unwrap();
        assert_eq!(a.display_string(), "@abc");
        let p = UnixAddr::path("/run/x.sock").unwrap();
        assert_eq!(p.display_string(), "/run/x.sock");
        let weird = UnixAddr::abstract_name(&[0xFF, b'a']).unwrap();
        assert_eq!(weird.display_string(), "@?a");
    }

    #[test]
    fn session_socket_honors_runtime_dir() {
        std::env::set_var("XDG_RUNTIME_DIR", "/run/user/1000");
        let s = UnixAddr::session_socket("seat0").unwrap();
        assert_eq!(s.display_string(), "/run/user/1000/ldp-seat0/ldp.sock");
        std::env::remove_var("XDG_RUNTIME_DIR");
        let s = UnixAddr::session_socket("seat0").unwrap();
        assert_eq!(s.display_string(), "/tmp/ldp-seat0/ldp.sock");
        std::env::set_var("XDG_RUNTIME_DIR", "/run/user/1000");
    }

    #[test]
    fn long_paths_rejected() {
        let long = format!("/tmp/{}", "a".repeat(120));
        assert!(UnixAddr::path(&long).is_err());
    }
}
