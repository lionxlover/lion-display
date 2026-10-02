//! Pipe FDs: the one audited syscall boundary of this crate.
//!
//! `docs/architecture.md` §15: payloads stream over pipe FDs; the
//! server never buffers more than a page. This module creates pipes
//! ([`pipe`]), sets blocking mode, and performs raw
//! read/write and the blocking write-all helper on the halves — the
//! primitives [`crate::transfer`]'s pump drives.
//!
//! Safety doctrine (the `ldp-input`/`ldp-display` precedent): the
//! crate is `#![forbid(unsafe_code)]` in every module *except this
//! one*; every `unsafe` block below carries a `SAFETY` note, the FDs
//! live in [`OwnedFd`] (RAII — one owner, closed exactly once), and
//! no raw number escapes this module except transiently for the
//! duration of a single syscall.
//!
//! All halves are created `O_CLOEXEC | O_NONBLOCK` (never leak into
//! children, never block the event loop); [`PipePair::set_blocking`]
//! opts specific halves into blocking mode for thread-driven copies.

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

/// A created pipe: read half + write half.
#[derive(Debug)]
pub struct PipePair {
    /// The reading end.
    pub read: PipeRead,
    /// The writing end.
    pub write: PipeWrite,
}

/// The reading half of a pipe.
#[derive(Debug)]
pub struct PipeRead(OwnedFd);

/// The writing half of a pipe.
#[derive(Debug)]
pub struct PipeWrite(OwnedFd);

impl PipeRead {
    /// The raw fd for the duration of a syscall (never stored).
    fn raw(&self) -> RawFd {
        self.0.as_raw_fd()
    }

    /// Unwrap into the owning fd (adoption into other owners).
    #[must_use]
    pub fn into_fd(self) -> OwnedFd {
        self.0
    }

    /// Read up to `buf.len()` bytes (nonblocking unless
    /// [`PipePair::set_blocking`] was used).
    ///
    /// `Ok(0)` = EOF (all write ends closed).
    ///
    /// # Errors
    ///
    /// `WouldBlock` when empty and nonblocking; anything else
    /// propagates.
    pub fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // SAFETY: `read(2)` on an owned, live fd; the buffer is a
        // valid exclusive borrow for the call's duration; the return
        // value bounds how many bytes were written into it.
        let n = unsafe {
            libc::read(
                self.raw(),
                buf.as_mut_ptr().cast::<libc::c_void>(),
                buf.len(),
            )
        };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(n as usize)
    }

    /// Read repeatedly, blocking, until the buffer fills or EOF.
    ///
    /// # Errors
    ///
    /// Propagates hard errors; `WouldBlock` is retried (the fd must
    /// be in blocking mode for this to ever finish on an empty pipe).
    pub fn read_blocking(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut filled = 0usize;
        while filled < buf.len() {
            match self.read(&mut buf[filled..]) {
                Ok(0) => break,
                Ok(n) => filled += n,
                // Interrupted is retried by falling through.
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(filled)
    }
}

impl PipeWrite {
    /// The raw fd for the duration of a syscall (never stored).
    fn raw(&self) -> RawFd {
        self.0.as_raw_fd()
    }

    /// Unwrap into the owning fd (adoption into other owners).
    #[must_use]
    pub fn into_fd(self) -> OwnedFd {
        self.0
    }

    /// Write as much of `buf` as the pipe takes now (partial write
    /// possible; nonblocking unless [`PipePair::set_blocking`]).
    ///
    /// # Errors
    ///
    /// `WouldBlock` when full and nonblocking; `BrokenPipe` when
    /// every read end is gone.
    pub fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // SAFETY: `write(2)` on an owned, live fd; the buffer is a
        // valid shared borrow for the call's duration.
        let n = unsafe { libc::write(self.raw(), buf.as_ptr().cast::<libc::c_void>(), buf.len()) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(n as usize)
    }

    /// Write all of `buf`, blocking until it drains (the pipe's own
    /// backpressure — this is how the 100 MB EC stream stays honest).
    ///
    /// # Errors
    ///
    /// Propagates hard errors; `Interrupted` and `WouldBlock` (fd in
    /// nonblocking mode) are retried.
    pub fn write_all_blocking(&mut self, buf: &[u8]) -> io::Result<()> {
        let mut written = 0usize;
        while written < buf.len() {
            match self.write(&buf[written..]) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "pipe accepted 0 bytes",
                    ));
                }
                Ok(n) => written += n,
                // Interrupted / WouldBlock (nonblocking mode) are
                // retried by falling through.
                Err(e)
                    if e.kind() == io::ErrorKind::Interrupted
                        || e.kind() == io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}

/// Create a pipe with `O_CLOEXEC | O_NONBLOCK` on both halves.
///
/// # Errors
///
/// `EMFILE`/`ENFILE` (fd exhaustion) and friends propagate.
pub fn pipe() -> io::Result<PipePair> {
    let mut fds = [0 as libc::c_int; 2];
    // SAFETY: `pipe2(2)` with a valid two-int out array and no
    // flags beyond O_CLOEXEC|O_NONBLOCK; on failure nothing is
    // written that we adopt.
    let r = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) };
    if r != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: both numbers were just filled by the successful pipe2
    // call; each is adopted exactly once into its own OwnedFd (the
    // single-owner doctrine).
    let read = unsafe { OwnedFd::from_raw_fd(fds[0]) };
    let write = unsafe { OwnedFd::from_raw_fd(fds[1]) };
    Ok(PipePair {
        read: PipeRead(read),
        write: PipeWrite(write),
    })
}

/// Set one fd's `O_NONBLOCK` off (blocking mode) or on.
///
/// # Errors
///
/// `fcntl` failures propagate (fd closed by a racing thread).
pub fn set_blocking(fd: &OwnedFd, blocking: bool) -> io::Result<()> {
    let raw = fd.as_raw_fd();
    // SAFETY: `fcntl(F_GETFL)` on an owned, live fd; no out-parameters.
    let flags = unsafe { libc::fcntl(raw, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    let wanted = if blocking {
        flags & !libc::O_NONBLOCK
    } else {
        flags | libc::O_NONBLOCK
    };
    if wanted == flags {
        return Ok(());
    }
    // SAFETY: `fcntl(F_SETFL)` on the same owned fd with flags built
    // from the kernel's own F_GETFL result.
    let r = unsafe { libc::fcntl(raw, libc::F_SETFL, wanted) };
    if r < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

impl PipePair {
    /// Put both halves into blocking mode (thread-driven copies).
    ///
    /// # Errors
    ///
    /// Propagates `fcntl` failures; on failure the pair may be half
    /// switched (the caller usually drops it).
    pub fn set_blocking(&mut self) -> io::Result<()> {
        set_blocking(&self.read.0, true)?;
        set_blocking(&self.write.0, true)?;
        Ok(())
    }
}

/// The std-io bridges: with these, the blanket impls in
/// `crate::transfer` adapt the halves to the pump traits (the sys
/// seam adapts to the policy layer, never the reverse).
impl io::Read for PipeRead {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        PipeRead::read(self, buf)
    }
}

impl io::Write for PipeWrite {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        PipeWrite::write(self, buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(()) // Pipes have no userspace buffer to flush.
    }
}

/// Count open FDs in this process (the leak assertion primitive for
/// the EC suite; from `/proc`, no syscalls on other processes).
///
/// # Errors
///
/// When `/proc` is unreadable.
pub fn count_open_fds() -> io::Result<usize> {
    let n = std::fs::read_dir("/proc/self/fd")?.count();
    Ok(n.saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    // `count_open_fds` is a *process-wide* snapshot (the same doctrine
    // `ldp-transport`'s hygiene suite documents): while a leak
    // assertion is in flight, no other test in this binary may open
    // or close descriptors. Every test in this module serializes on
    // one mutex — the latent race surfaced on a machine whose core
    // count re-ordered the default parallel scheduler (the v0.10.5
    // seal run's own catch).
    static PIPES: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn pipe_round_trip_small() {
        let _guard = PIPES.lock().expect("test serialization");
        let mut p = pipe().unwrap();
        let msg = b"hello, ldp clipboard";
        p.write.write(msg).unwrap();
        let mut buf = [0u8; 64];
        let n = p.read.read(&mut buf).unwrap();
        assert_eq!(&buf[..n], msg);
    }

    #[test]
    fn nonblocking_empty_read_is_would_block() {
        let _guard = PIPES.lock().expect("test serialization");
        let mut p = pipe().unwrap();
        let mut buf = [0u8; 8];
        assert_eq!(
            p.read.read(&mut buf).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn eof_when_write_end_dropped() {
        let _guard = PIPES.lock().expect("test serialization");
        let p = pipe().unwrap();
        let PipePair { read, write } = p;
        let mut read = read;
        drop(write);
        let mut buf = [0u8; 8];
        match read.read(&mut buf) {
            Ok(0) => {}
            other => panic!("expected EOF, got {other:?}"),
        }
    }

    #[test]
    fn blocking_pair_full_round_trip() {
        let _guard = PIPES.lock().expect("test serialization");
        let mut p = pipe().unwrap();
        p.set_blocking().unwrap();
        let payload: Vec<u8> = (0u8..=255).cycle().take(256 * 100).collect();
        // Drain in a reader thread: the pipe would otherwise fill.
        let mut reader = p.read;
        let expect = payload.clone();
        let handle = std::thread::spawn(move || {
            let mut got = Vec::new();
            let mut buf = [0u8; 512];
            loop {
                let n = reader.read_blocking(&mut buf).unwrap();
                if n == 0 {
                    break;
                }
                got.extend_from_slice(&buf[..n]);
            }
            got
        });
        // Chunked writes through the blocking path.
        for chunk in payload.chunks(1000) {
            p.write.write_all_blocking(chunk).unwrap();
        }
        drop(p.write);
        assert_eq!(handle.join().unwrap(), expect);
    }

    #[test]
    fn fd_counting_works() {
        let _guard = PIPES.lock().expect("test serialization");
        let before = count_open_fds().unwrap();
        {
            let _p = pipe().unwrap();
            assert_eq!(count_open_fds().unwrap(), before + 2);
        }
        assert_eq!(count_open_fds().unwrap(), before);
    }
}
