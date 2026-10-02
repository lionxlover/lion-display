//! Syscall layer — the *only* module containing `unsafe`.
//!
//! Thin wrappers over the Linux socket ABI the transport needs:
//! `socket`/`bind`/`listen`/`accept4`/`connect`, `sendmsg`/`recvmsg`
//! with `SCM_RIGHTS` ancillary data, `getsockopt(SO_PEERCRED)`, and
//! `fcntl` flag management. The `libc` crate marks these wrappers
//! `unsafe` (their pointer arguments carry the risk), so each call site
//! here is an `unsafe` block with a `SAFETY` comment stating its
//! invariant (CONTRIBUTING.md rule 3). Every other module in the crate
//! is `#![forbid(unsafe_code)]` — this file is the audit boundary.
//!
//! The cmsg codec ([`ControlBuffer`]) walks control messages bytewise
//! in native endianness, so parsing never forms raw pointers.
//!
//! EINTR policy: `sendmsg`, `recvmsg`, and `accept4` retry transparently
//! (EINTR is raised before any transfer, so re-issuing the call is
//! lossless). `connect` is not retried — an interrupted connect must be
//! resolved with `SO_ERROR`/poll by the caller, which never happens on
//! the local fast path this crate targets.

use ldp_core::error::{ErrorCode, LdpError, Result};
use std::os::fd::{FromRawFd, OwnedFd, RawFd};

/// One `cmsg` header's size (`sizeof(cmsghdr)`).
const CMSG_HDR_SIZE: usize = size_of::<libc::cmsghdr>();
/// Alignment unit of `cmsghdr` (and of cmsg payloads on the wire).
const CMSG_ALIGN_UNIT: usize = align_of::<libc::cmsghdr>();

/// `CMSG_ALIGN` — round up to the cmsg alignment unit.
const fn cmsg_align(len: usize) -> usize {
    (len + CMSG_ALIGN_UNIT - 1) & !(CMSG_ALIGN_UNIT - 1)
}

/// Offset of cmsg payload data (`CMSG_DATA` semantics).
const CMSG_DATA_OFF: usize = cmsg_align(CMSG_HDR_SIZE);

/// Bytes of control buffer needed for `n` FDs (`CMSG_SPACE(n * 4)`).
#[must_use]
pub const fn cmsg_space_for_fds(n: usize) -> usize {
    cmsg_align(CMSG_HDR_SIZE) + cmsg_align(n * size_of::<libc::c_int>())
}

/// Result of one `recvmsg`: bytes moved, control bytes received, and
/// whether the kernel truncated ancillary data (MSG_CTRUNC — for
/// `SCM_RIGHTS` the truncated FDs were closed by the kernel).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RawRecv {
    /// Payload bytes received (0 = orderly EOF).
    pub bytes: usize,
    /// Length of the ancillary data actually received (valid prefix of
    /// the control buffer; 0 when no control buffer was supplied).
    pub control_used: usize,
    /// MSG_CTRUNC: ancillary data exceeded the receive control buffer.
    pub truncated: bool,
}

/// A `u64`-word-backed, cmsg-aligned control buffer.
///
/// Alignment: `Vec<u64>` guarantees 8-byte alignment, which is `>=`
/// `align_of::<cmsghdr>()` on every Linux ABI (8 on LP64, 4 on ILP32).
/// One buffer serves both directions: [`ControlBuffer::encode_rights`]
/// fills it for a send, [`ControlBuffer::parse_rights_into`] walks it
/// after a receive.
#[derive(Debug)]
pub struct ControlBuffer {
    words: Vec<u64>,
}

impl ControlBuffer {
    /// A buffer sized to carry `max_fds` FDs in one `SCM_RIGHTS`
    /// message.
    #[must_use]
    pub fn with_fd_capacity(max_fds: usize) -> ControlBuffer {
        let bytes = cmsg_space_for_fds(max_fds).next_multiple_of(8);
        ControlBuffer {
            words: vec![0u64; bytes / 8],
        }
    }

    /// Total byte capacity.
    #[must_use]
    pub fn byte_capacity(&self) -> usize {
        self.words.len() * 8
    }

    /// Encode `fds` as one `SCM_RIGHTS` control message; returns the
    /// controllen to hand to `sendmsg`. An empty batch encodes to zero
    /// control bytes (plain send).
    ///
    /// # Errors
    ///
    /// [`LdpError::Limit`] when the batch exceeds
    /// [`SCM_MAX_FD`](crate::SCM_MAX_FD) (the kernel's per-cmsg cap) or
    /// this buffer's capacity.
    pub fn encode_rights(&mut self, fds: &[RawFd]) -> Result<usize> {
        if fds.is_empty() {
            return Ok(0);
        }
        if fds.len() > crate::SCM_MAX_FD {
            return Err(crate::error::fd_limit(fds.len()));
        }
        let data_len = size_of_val(fds);
        let total = CMSG_DATA_OFF + data_len;
        if total > self.byte_capacity() {
            return Err(crate::error::fd_limit(fds.len()));
        }
        let bytes = self.as_bytes_mut();
        // cmsghdr { cmsg_len: usize, cmsg_level: c_int, cmsg_type: c_int }
        let usize_len = size_of::<usize>();
        bytes[0..usize_len].copy_from_slice(&total.to_ne_bytes());
        bytes[usize_len..usize_len + 4].copy_from_slice(&libc::SOL_SOCKET.to_ne_bytes());
        bytes[usize_len + 4..CMSG_HDR_SIZE].copy_from_slice(&libc::SCM_RIGHTS.to_ne_bytes());
        for (slot, fd) in fds.iter().enumerate() {
            let off = CMSG_DATA_OFF + slot * size_of::<libc::c_int>();
            bytes[off..off + 4].copy_from_slice(&(*fd as libc::c_int).to_ne_bytes());
        }
        Ok(total)
    }

    /// The first `len` bytes as the controllen slice for a send.
    #[must_use]
    pub fn encoded_prefix(&self, len: usize) -> &[u8] {
        &self.as_bytes()[..len]
    }

    /// Walk every cmsg in the first `used` bytes, adopting each
    /// `SCM_RIGHTS` FD into `out`.
    ///
    /// Adoption is incremental: on a mid-walk failure the FDs adopted so
    /// far stay in `out` (and are closed by its owner), so nothing leaks.
    ///
    /// # Errors
    ///
    /// [`LdpError::malformed`] for non-`SCM_RIGHTS` ancillary data or
    /// structurally impossible cmsg headers (neither is producible by a
    /// sane kernel; both are peer-attack surfaces).
    pub fn parse_rights_into(&self, used: usize, out: &mut crate::fd::FdList) -> Result<()> {
        let bytes = self.as_bytes();
        let usize_len = size_of::<usize>();
        let mut off = 0usize;
        while off < used {
            if off + CMSG_HDR_SIZE > used {
                return Err(LdpError::malformed(
                    ErrorCode::FdMismatch,
                    "trailing control bytes shorter than a cmsg header",
                ));
            }
            let mut len_buf = [0u8; size_of::<usize>()];
            len_buf.copy_from_slice(&bytes[off..off + usize_len]);
            let cmsg_len = usize::from_ne_bytes(len_buf);
            let mut buf4 = [0u8; 4];
            buf4.copy_from_slice(&bytes[off + usize_len..off + usize_len + 4]);
            let level = i32::from_ne_bytes(buf4);
            buf4.copy_from_slice(&bytes[off + usize_len + 4..off + CMSG_HDR_SIZE]);
            let cmsg_type = i32::from_ne_bytes(buf4);
            if cmsg_len < CMSG_DATA_OFF || off + cmsg_len > used {
                return Err(LdpError::malformed(
                    ErrorCode::FdMismatch,
                    "cmsg length inconsistent with the control buffer",
                ));
            }
            let data_len = cmsg_len - CMSG_DATA_OFF;
            if data_len % size_of::<libc::c_int>() != 0 {
                return Err(LdpError::malformed(
                    ErrorCode::FdMismatch,
                    "cmsg payload is not a whole number of FDs",
                ));
            }
            if level != libc::SOL_SOCKET || cmsg_type != libc::SCM_RIGHTS {
                return Err(LdpError::malformed(
                    ErrorCode::FdMismatch,
                    "non-SCM_RIGHTS ancillary data on an LDP socket",
                ));
            }
            for slot in 0..data_len / size_of::<libc::c_int>() {
                let at = off + CMSG_DATA_OFF + slot * size_of::<libc::c_int>();
                let mut fd = [0u8; 4];
                fd.copy_from_slice(&bytes[at..at + 4]);
                let raw = i32::from_ne_bytes(fd);
                out.adopt(raw)?;
            }
            off += cmsg_align(cmsg_len);
        }
        Ok(())
    }

    /// Borrow the whole buffer as bytes (immutable view).
    #[must_use]
    fn as_bytes(&self) -> &[u8] {
        // SAFETY: `words` is a live Vec<u64> of len N; recasting its
        // buffer to u8 and taking N*8 bytes stays inside the same
        // allocation, and u8 has no alignment requirement.
        unsafe {
            std::slice::from_raw_parts(self.words.as_ptr().cast::<u8>(), self.byte_capacity())
        }
    }

    /// Borrow the whole buffer as bytes (mutable view, recv side).
    fn as_bytes_mut(&mut self) -> &mut [u8] {
        // SAFETY: same as `as_bytes`, with a mutable borrow of the live
        // Vec; no aliasing is created because both borrows cannot coexist.
        unsafe {
            std::slice::from_raw_parts_mut(
                self.words.as_mut_ptr().cast::<u8>(),
                self.byte_capacity(),
            )
        }
    }
}

fn last_os_error() -> LdpError {
    LdpError::Io(std::sync::Arc::new(std::io::Error::last_os_error()))
}

fn io_err(kind: std::io::ErrorKind, message: &str) -> LdpError {
    LdpError::Io(std::sync::Arc::new(std::io::Error::new(kind, message)))
}

/// Create an `AF_UNIX` `SOCK_STREAM` socket with `SOCK_CLOEXEC` (and
/// `SOCK_NONBLOCK` when `nonblocking`). Ownership returns as an
/// [`OwnedFd`] — the caller's drop closes it.
///
/// # Errors
///
/// [`LdpError::Io`] on `socket(2)` failure (EMFILE/ENFILE exhaustion).
pub fn raw_socket(nonblocking: bool) -> Result<OwnedFd> {
    let mut flags = libc::SOCK_STREAM | libc::SOCK_CLOEXEC;
    if nonblocking {
        flags |= libc::SOCK_NONBLOCK;
    }
    // SAFETY: `socket` dereferences no user pointers; the returned fd is
    // owned by this function from here on.
    let fd = unsafe { libc::socket(libc::AF_UNIX, flags, 0) };
    if fd < 0 {
        return Err(last_os_error());
    }
    // SAFETY: `fd` is a live socket this function owns; the returned
    // OwnedFd closes it exactly once.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// `bind(2)` an AF_UNIX address (exact socklen: abstract names keep
/// their prefix-distinct length).
///
/// # Errors
///
/// [`LdpError::Io`] on failure (EADDRINUSE for a live filesystem path).
pub fn raw_bind(fd: RawFd, addr: &crate::addr::UnixAddr) -> Result<()> {
    let mut sun = libc::sockaddr_un {
        sun_family: 0,
        sun_path: [0; 108],
    };
    let len = addr.fill_sockaddr(&mut sun);
    // SAFETY: `sun` is a genuinely `sockaddr_un`-typed stack object
    // (correct alignment), `len` <= sizeof(sockaddr_un) by construction,
    // and the object outlives the call.
    let rc = unsafe { libc::bind(fd, std::ptr::addr_of!(sun).cast::<libc::sockaddr>(), len) };
    if rc != 0 {
        return Err(last_os_error());
    }
    Ok(())
}

/// `listen(2)`.
///
/// # Errors
///
/// [`LdpError::Io`] on failure.
pub fn raw_listen(fd: RawFd, backlog: i32) -> Result<()> {
    // SAFETY: no user pointers are dereferenced.
    if unsafe { libc::listen(fd, backlog) } != 0 {
        return Err(last_os_error());
    }
    Ok(())
}

/// `connect(2)` to an AF_UNIX address. Not EINTR-retried (see module
/// docs).
///
/// # Errors
///
/// [`LdpError::Io`] on failure (ECONNREFUSED when nothing listens).
pub fn raw_connect(fd: RawFd, addr: &crate::addr::UnixAddr) -> Result<()> {
    let mut sun = libc::sockaddr_un {
        sun_family: 0,
        sun_path: [0; 108],
    };
    let len = addr.fill_sockaddr(&mut sun);
    // SAFETY: same contract as `raw_bind`; the object outlives the call.
    let rc = unsafe { libc::connect(fd, std::ptr::addr_of!(sun).cast::<libc::sockaddr>(), len) };
    if rc != 0 {
        return Err(last_os_error());
    }
    Ok(())
}

/// `accept4(2)` with `SOCK_CLOEXEC` (plus `SOCK_NONBLOCK` when
/// `nonblocking`), EINTR-retried. Ownership of the accepted fd returns
/// as an [`OwnedFd`].
///
/// # Errors
///
/// [`LdpError::Io`] on failure (EAGAIN when no connection is pending on
/// a nonblocking listener).
pub fn raw_accept4(fd: RawFd, nonblocking: bool) -> Result<OwnedFd> {
    let mut flags = libc::SOCK_CLOEXEC;
    if nonblocking {
        flags |= libc::SOCK_NONBLOCK;
    }
    loop {
        // SAFETY: the null addr/len pair is explicitly allowed by
        // accept4(2); no user pointers are dereferenced.
        let got = unsafe { libc::accept4(fd, std::ptr::null_mut(), std::ptr::null_mut(), flags) };
        if got >= 0 {
            // SAFETY: `got` is a freshly accepted socket this function
            // owns; the returned OwnedFd closes it exactly once.
            return Ok(unsafe { OwnedFd::from_raw_fd(got) });
        }
        let err = std::io::Error::last_os_error();
        if err.kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        return Err(LdpError::Io(std::sync::Arc::new(err)));
    }
}

/// `sendmsg(2)` with optional already-encoded control data, EINTR-retried.
///
/// Returns the number of payload bytes accepted. When control data was
/// supplied and at least one byte was accepted, the ancillary data has
/// been transferred (attached to the first accepted byte) — the caller
/// must treat the FDs as consumed.
///
/// # Errors
///
/// [`LdpError::Io`] on failure (EAGAIN on a full nonblocking socket —
/// nothing, including control data, was transferred then).
pub fn send_msg(fd: RawFd, buf: &[u8], control: Option<&[u8]>) -> Result<usize> {
    let mut iov = libc::iovec {
        // The iovec ABI wants a mutable pointer; `sendmsg` only reads it.
        iov_base: buf.as_ptr().cast_mut().cast::<libc::c_void>(),
        iov_len: buf.len(),
    };
    loop {
        // SAFETY: an all-zero `msghdr` is the valid "no name, no
        // control" state; the iov and control pointers assigned below
        // borrow `buf` / `control`, which outlive the call.
        let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        if let Some(ctl) = control {
            // `sendmsg` only reads the control data.
            msg.msg_control = ctl.as_ptr().cast_mut().cast::<libc::c_void>();
            msg.msg_controllen = ctl.len();
        }
        // SAFETY: syscall wrapper — fd is a live socket and every buffer
        // `msg` points to is valid for the duration of the call.
        let n = unsafe { libc::sendmsg(fd, &msg, 0) };
        if n >= 0 {
            return Ok(n as usize);
        }
        let err = std::io::Error::last_os_error();
        if err.kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        return Err(LdpError::Io(std::sync::Arc::new(err)));
    }
}

/// `recvmsg(2)` into a control buffer, EINTR-retried.
///
/// A zero `bytes` result is an orderly EOF. `RawRecv::control_used`
/// bytes of the buffer were written by the kernel.
///
/// # Errors
///
/// [`LdpError::Io`] on failure (EAGAIN when nothing is readable on a
/// nonblocking socket).
pub fn recv_with_control(
    fd: RawFd,
    buf: &mut [u8],
    control: &mut ControlBuffer,
) -> Result<RawRecv> {
    let mut iov = libc::iovec {
        iov_base: buf.as_mut_ptr().cast::<libc::c_void>(),
        iov_len: buf.len(),
    };
    loop {
        // SAFETY: zeroed msghdr is the valid "no name" state; the iov
        // and control pointers borrow `buf` / `control`, which outlive
        // the call.
        let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        {
            let bytes = control.mut_bytes_for_recv();
            msg.msg_control = bytes.as_mut_ptr().cast::<libc::c_void>();
            msg.msg_controllen = bytes.len();
        }
        // SAFETY: syscall wrapper — fd is a live socket and every buffer
        // `msg` points to is valid for the duration of the call.
        let n = unsafe { libc::recvmsg(fd, &mut msg, 0) };
        let truncated = msg.msg_flags & libc::MSG_CTRUNC != 0;
        // `msg_controllen` is `size_t` on every Linux libc family this
        // stack serves (glibc and musl alike) — the field is its own
        // `usize`, no conversion owed.
        let control_used = msg.msg_controllen;
        if n >= 0 {
            return Ok(RawRecv {
                bytes: n as usize,
                control_used,
                truncated,
            });
        }
        let err = std::io::Error::last_os_error();
        if err.kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        return Err(LdpError::Io(std::sync::Arc::new(err)));
    }
}

/// `recvmsg(2)` with no control buffer, EINTR-retried.
///
/// Ancillary data pending at the read position cannot be received: the
/// kernel closes those FDs and reports [`RawRecv::truncated`].
///
/// # Errors
///
/// [`LdpError::Io`] on failure (EAGAIN on a nonblocking socket).
pub fn recv_plain(fd: RawFd, buf: &mut [u8]) -> Result<RawRecv> {
    let mut iov = libc::iovec {
        iov_base: buf.as_mut_ptr().cast::<libc::c_void>(),
        iov_len: buf.len(),
    };
    loop {
        // SAFETY: zeroed msghdr leaves msg_control null and
        // msg_controllen zero — the documented no-ancillary receive;
        // `iov` points into `buf` which outlives the call.
        let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        // SAFETY: syscall wrapper — fd is a live socket and `iov` points
        // into `buf` for the duration of the call.
        let n = unsafe { libc::recvmsg(fd, &mut msg, 0) };
        let truncated = msg.msg_flags & libc::MSG_CTRUNC != 0;
        if n >= 0 {
            return Ok(RawRecv {
                bytes: n as usize,
                control_used: 0,
                truncated,
            });
        }
        let err = std::io::Error::last_os_error();
        if err.kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        return Err(LdpError::Io(std::sync::Arc::new(err)));
    }
}

/// `getsockopt(SO_PEERCRED)` on a connected AF_UNIX socket.
///
/// # Errors
///
/// [`LdpError::Io`] when the fd is not a connected AF_UNIX stream.
pub fn peer_cred(fd: RawFd) -> Result<crate::creds::PeerCreds> {
    let mut cred = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: `cred` is a ucred-sized stack object and `len` is its
    // size; getsockopt writes within those bounds.
    let rc = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            std::ptr::addr_of_mut!(cred).cast::<libc::c_void>(),
            &mut len,
        )
    };
    if rc != 0 {
        return Err(last_os_error());
    }
    Ok(crate::creds::PeerCreds {
        pid: cred.pid,
        uid: cred.uid,
        gid: cred.gid,
    })
}

/// Set or clear `O_NONBLOCK` (read-modify-write through `F_GETFL`).
///
/// # Errors
///
/// [`LdpError::Io`] on fcntl failure.
pub fn set_nonblocking(fd: RawFd, on: bool) -> Result<()> {
    // SAFETY: `fcntl` with `F_GETFL` dereferences no user pointers.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(last_os_error());
    }
    let wanted = if on {
        flags | libc::O_NONBLOCK
    } else {
        flags & !libc::O_NONBLOCK
    };
    if wanted == flags {
        return Ok(());
    }
    // SAFETY: `fcntl` with `F_SETFL` takes the flag set by value.
    if unsafe { libc::fcntl(fd, libc::F_SETFL, wanted) } < 0 {
        return Err(last_os_error());
    }
    Ok(())
}

/// Whether `O_NONBLOCK` is currently set on the fd.
///
/// # Errors
///
/// [`LdpError::Io`] on `F_GETFL` failure (fd not open).
pub fn is_nonblocking(fd: RawFd) -> Result<bool> {
    // SAFETY: `fcntl` with `F_GETFL` dereferences no user pointers.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(last_os_error());
    }
    Ok(flags & libc::O_NONBLOCK != 0)
}

/// Whether `fcntl(F_GETFD)` reports the raw fd open (the hygiene check
/// before ownership transfer, `docs/architecture.md` §6).
#[must_use]
pub fn fd_is_open(raw: RawFd) -> bool {
    // SAFETY: `fcntl` with `F_GETFD` dereferences no user pointers.
    (unsafe { libc::fcntl(raw, libc::F_GETFD) }) != -1
}

/// Validate a raw fd with `fcntl(F_GETFD)` and adopt it as an
/// [`OwnedFd`]. On validation failure the raw fd is closed here — this
/// is the single ownership hand-off point for kernel-issued descriptors
/// (cmsg arrays), so rejection paths never leak.
///
/// # Errors
///
/// [`LdpError::Io`] when the fd is dead.
pub fn adopt_validated(raw: RawFd) -> Result<OwnedFd> {
    if !fd_is_open(raw) {
        // SAFETY: best-effort close of a number that just failed
        // F_GETFD; EBADF is ignored by intent.
        unsafe { libc::close(raw) };
        return Err(last_os_error());
    }
    // SAFETY: `raw` just passed F_GETFD validation and ownership is
    // transferred here; the returned OwnedFd closes it exactly once.
    Ok(unsafe { OwnedFd::from_raw_fd(raw) })
}

/// Best-effort explicit close for hygiene paths (callers pass numbers
/// they know are live or already-forgotten).
pub fn close_raw(raw: RawFd) {
    // SAFETY: `close` takes a plain integer; the caller accepts
    // best-effort semantics (double close is the caller's bug to avoid).
    unsafe { libc::close(raw) };
}

/// Real UID of this process (`getuid(2)`).
#[must_use]
pub fn getuid() -> u32 {
    // SAFETY: `getuid` takes no arguments and touches no user pointers.
    unsafe { libc::getuid() }
}

/// Real GID of this process (`getgid(2)`).
#[must_use]
pub fn getgid() -> u32 {
    // SAFETY: `getgid` takes no arguments and touches no user pointers.
    unsafe { libc::getgid() }
}

/// Create an `eventfd` (counter) with `EFD_CLOEXEC` — the wakeup
/// primitive `docs/protocol.md` §11 prescribes for poll-integrated
/// waits, and the standard test stand-in for a real buffer fd.
///
/// # Errors
///
/// [`LdpError::Io`] on `eventfd(2)` failure.
pub fn eventfd_owned() -> Result<OwnedFd> {
    // SAFETY: `eventfd` dereferences no user pointers; the returned fd
    // is owned by this function.
    let fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC) };
    if fd < 0 {
        return Err(last_os_error());
    }
    // SAFETY: `fd` is a live eventfd this function owns; the returned
    // OwnedFd closes it exactly once.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Verify an fd is an `AF_UNIX` `SOCK_STREAM` socket via
/// `getsockopt(SO_TYPE)` — the adoption check for
/// `TransportStream::new`.
///
/// # Errors
///
/// [`LdpError::Io`] when the fd is not a stream socket.
pub fn assert_stream_socket(fd: RawFd) -> Result<()> {
    let mut ty: libc::c_int = 0;
    let mut len = size_of::<libc::c_int>() as libc::socklen_t;
    // SAFETY: `ty` is an int-sized buffer and `len` its size.
    let rc = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_TYPE,
            std::ptr::addr_of_mut!(ty).cast::<libc::c_void>(),
            &mut len,
        )
    };
    if rc != 0 || ty != libc::SOCK_STREAM as libc::c_int {
        return Err(io_err(
            std::io::ErrorKind::InvalidInput,
            "fd is not an AF_UNIX stream socket",
        ));
    }
    Ok(())
}

/// `socketpair(2)` with `SOCK_CLOEXEC` (loopback and test helper).
/// Ownership returns as two [`OwnedFd`]s.
///
/// # Errors
///
/// [`LdpError::Io`] on failure.
pub fn raw_socketpair() -> Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0 as libc::c_int; 2];
    // SAFETY: `fds` is a two-int buffer, exactly what socketpair writes.
    let rc = unsafe {
        libc::socketpair(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_CLOEXEC,
            0,
            fds.as_mut_ptr(),
        )
    };
    if rc != 0 {
        return Err(last_os_error());
    }
    // SAFETY: both entries are live socketpair ends owned by this
    // function; each OwnedFd closes its fd exactly once.
    let a = unsafe { OwnedFd::from_raw_fd(fds[0]) };
    let b = unsafe { OwnedFd::from_raw_fd(fds[1]) };
    Ok((a, b))
}

impl ControlBuffer {
    /// Mutable byte view for the recv path (crate-internal).
    fn mut_bytes_for_recv(&mut self) -> &mut [u8] {
        self.as_bytes_mut()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fd::FdList;
    use std::os::fd::{AsRawFd, IntoRawFd};

    #[test]
    fn cmsg_space_matches_kernel_layout() {
        // 64 FDs -> CMSG_SPACE(256) = align(16) + align(256) = 272.
        assert_eq!(cmsg_space_for_fds(64), 272);
        // 1 FD -> CMSG_SPACE(4) = align(16) + align(4) = 16 + 8 = 24.
        assert_eq!(cmsg_space_for_fds(1), 24);
        // Data offset equals the aligned header size (16 on LP64).
        assert!(CMSG_DATA_OFF >= size_of::<libc::cmsghdr>());
    }

    #[test]
    fn encode_then_parse_round_trips() {
        let mut ctl = ControlBuffer::with_fd_capacity(4);
        // Raw numbers leaked from OwnedFds so that `parse_rights_into`
        // (which adopts) becomes their single owner — parsing kernel-
        // issued cmsg numbers is the production path; this mirrors it.
        let fds: Vec<RawFd> = (0..4)
            .map(|_| eventfd_owned().unwrap().into_raw_fd())
            .collect();
        let used = ctl.encode_rights(&fds).unwrap();
        assert_eq!(used, CMSG_DATA_OFF + 4 * 4);
        let mut out = FdList::new();
        ctl.parse_rights_into(used, &mut out).unwrap();
        assert_eq!(out.len(), 4);
        assert_eq!(out.raw_fds(), fds);
    }

    #[test]
    fn encode_rejects_over_capacity_and_kernel_cap() {
        let mut small = ControlBuffer::with_fd_capacity(2);
        let fds: Vec<RawFd> = vec![0, 1, 2];
        assert!(small.encode_rights(&fds).is_err());
        let mut big = ControlBuffer::with_fd_capacity(300);
        let many: Vec<RawFd> = vec![0; 254];
        assert!(big.encode_rights(&many).is_err());
        assert!(big.encode_rights(&many[..253]).is_ok());
    }

    #[test]
    fn empty_encode_is_zero_control() {
        let mut ctl = ControlBuffer::with_fd_capacity(4);
        assert_eq!(ctl.encode_rights(&[]).unwrap(), 0);
    }

    #[test]
    fn parse_rejects_trailing_garbage() {
        let mut ctl = ControlBuffer::with_fd_capacity(4);
        // Real fds (raw, single-owner via adoption): the walk adopts the
        // first cmsg's FDs, then hits fd_a trailing byte count that cannot
        // hold fd_a header and errors — the partial adoption still closes
        // exactly once via the FdList. Never forge low fd numbers in
        // tests: 3/4 are live harness fds and would be stolen.
        let fd_a = eventfd_owned().unwrap().into_raw_fd();
        let fd_b = eventfd_owned().unwrap().into_raw_fd();
        let used = ctl.encode_rights(&[fd_a, fd_b]).unwrap();
        let mut out = FdList::new();
        assert!(ctl.parse_rights_into(used + 1, &mut out).is_err());
        assert_eq!(out.len(), 2, "adopted before the trailing-bytes error");
    }

    #[test]
    fn parse_rejects_non_rights_type() {
        let mut ctl = ControlBuffer::with_fd_capacity(4);
        let bytes = ctl.mut_bytes_for_recv();
        // Forge fd_a cmsg with SCM_CREDENTIALS type and fd_a plausible length.
        let total = CMSG_DATA_OFF + 8;
        bytes[0..8].copy_from_slice(&total.to_ne_bytes());
        bytes[8..12].copy_from_slice(&libc::SOL_SOCKET.to_ne_bytes());
        bytes[12..16].copy_from_slice(&libc::SCM_CREDENTIALS.to_ne_bytes());
        let mut out = FdList::new();
        assert!(ctl.parse_rights_into(total, &mut out).is_err());
    }

    #[test]
    fn parse_walks_multiple_cmsgs() {
        let mut ctl = ControlBuffer::with_fd_capacity(8);
        // The forged second cmsg carries fd_a third real fd (leaked to fd_a
        // raw number), so adoption succeeds and FdList::drop closes all
        // of them exactly once.
        let fd_a = eventfd_owned().unwrap().into_raw_fd();
        let fd_b = eventfd_owned().unwrap().into_raw_fd();
        let fd_c = eventfd_owned().unwrap().into_raw_fd();
        let used1 = ctl.encode_rights(&[fd_a, fd_b]).unwrap();
        let bytes = ctl.mut_bytes_for_recv();
        let off = cmsg_align(used1);
        let total = CMSG_DATA_OFF + 4;
        bytes[off..off + 8].copy_from_slice(&total.to_ne_bytes());
        bytes[off + 8..off + 12].copy_from_slice(&libc::SOL_SOCKET.to_ne_bytes());
        bytes[off + 12..off + 16].copy_from_slice(&libc::SCM_RIGHTS.to_ne_bytes());
        bytes[off + 16..off + 20].copy_from_slice(&fd_c.to_ne_bytes());
        let mut out = FdList::new();
        ctl.parse_rights_into(off + total, &mut out).unwrap();
        assert_eq!(out.raw_fds(), vec![fd_a, fd_b, fd_c]);
    }

    #[test]
    fn parse_rejects_bogus_cmsg_len() {
        let mut ctl = ControlBuffer::with_fd_capacity(4);
        let bytes = ctl.mut_bytes_for_recv();
        // cmsg_len smaller than fd_a header is impossible.
        bytes[0..8].copy_from_slice(&4usize.to_ne_bytes());
        let mut out = FdList::new();
        assert!(ctl.parse_rights_into(20, &mut out).is_err());
    }

    #[test]
    fn adopt_validated_owns_and_closes() {
        assert!(adopt_validated(-1).is_err(), "dead number rejected");
        // Single ownership hand-off: leak into fd_a raw number, then adopt.
        let raw = eventfd_owned().unwrap().into_raw_fd();
        let adopted = adopt_validated(raw).unwrap();
        assert_eq!(adopted.as_raw_fd(), raw);
        drop(adopted);
        // A clean drop (no IO-safety abort) proves single ownership; the
        // number may be reused by concurrent tests, so no liveness check.
    }
}
