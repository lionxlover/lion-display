//! # ldp-transport — AF_UNIX transport for LDP
//!
//! The socket layer under the wire codec (`docs/architecture.md` §6):
//!
//! * [`addr`] — [`UnixAddr`]: filesystem and abstract-namespace socket
//!   addresses,
//! * [`listener`] / [`stream`] — accept and connect sides of one
//!   `AF_UNIX` `SOCK_STREAM` connection,
//! * [`creds`] — [`PeerCreds`]: the `SO_PEERCRED` identity of the peer
//!   (pid/uid/gid), read before any protocol data is processed,
//! * [`fd`] — FD hygiene: validation with `fcntl(F_GETFD)` before
//!   ownership transfer, owned FD batches that close on every rejection
//!   path, and process FD counting for leak assertions,
//! * [`sys`] — the *only* module with `unsafe`: thin, individually
//!   documented syscall wrappers (`sendmsg`/`recvmsg` with `SCM_RIGHTS`,
//!   `accept4`, `getsockopt(SO_PEERCRED)`) and a safe, bytewise cmsg
//!   codec,
//! * [`frame`] / [`reader`] / [`writer`] — message framing per
//!   `docs/protocol.md` §1–2: one message = one `sendmsg` (16-byte
//!   header + 8-byte-aligned payload, FDs riding the same call), with
//!   the reader cross-checking the header FD count against the
//!   ancillary array and closing every FD of a rejected message,
//! * [`backpressure`] — writer hooks and watermarks so a slow peer
//!   parks its own event queue instead of growing server memory.
//!
//! # Safety policy
//!
//! `unsafe` exists only in [`sys`] as documented blocks (CONTRIBUTING.md
//! rule 3); every other module is `#![forbid(unsafe_code)]`. The crate
//! is Linux-only: `SO_PEERCRED`, `accept4`, and abstract sockets are
//! Linux interfaces.
//!
//! # Validation-stage contract
//!
//! The reader implements stage 1 of the `docs/protocol.md` §9 pipeline —
//! size, alignment, and fd-count consistency — and *nothing* else:
//! flag bits, tags, and signatures belong to `ldp-protocol` stages 2–3.
//! A framing error is fatal for the connection; the reader is poisoned
//! afterwards and subsequent calls return
//! [`LdpError::Logic`](ldp_core::error::LdpError::Logic).
//!
//! ```
//! use ldp_core::limits::Limits;
//! use ldp_transport::{FdList, FramedReader, FramedWriter, NoHooks, TransportListener, TransportStream, UnixAddr};
//!
//! # fn main() -> ldp_core::error::Result<()> {
//! let name = format!("\u{0}ldp-doc-{}", std::process::id());
//! let addr = UnixAddr::abstract_name(name.as_bytes())?;
//! let listener = TransportListener::bind(&addr, 8)?;
//! let mut client = TransportStream::connect(&addr)?;
//! let (mut server, creds) = listener.accept()?;
//! assert_eq!(creds.pid, std::process::id() as i32);
//!
//! // One message: 16-byte header (payload_words=2, fd_count=2) + 16 bytes.
//! let mut msg = [0u8; 32];
//! msg[0..4].copy_from_slice(&2u32.to_le_bytes());   // payload_words
//! msg[14..16].copy_from_slice(&2u16.to_le_bytes()); // fd_count
//! let mut fds = FdList::new();
//! fds.push(std::fs::File::open("/dev/null")?.into());
//! fds.push(std::fs::File::open("/dev/null")?.into());
//!
//! let mut writer = FramedWriter::new(Limits::DEFAULT, NoHooks);
//! let mut reader = FramedReader::new(Limits::DEFAULT);
//! writer.send_msg(&mut server, &msg, &mut fds)?;
//! let frame = reader.recv_msg(&mut client)?;
//! assert_eq!(frame.message_bytes(), &msg[..]);
//! assert_eq!(frame.fds.len(), 2);
//! assert_eq!(frame.fd_count(), 2);
//! # Ok(())
//! # }
//! ```

#![cfg_attr(not(target_os = "linux"), compile_error("ldp-transport is Linux-only: SO_PEERCRED, accept4, and abstract AF_UNIX sockets are Linux interfaces"))]
#![deny(missing_docs)]

pub mod addr;
pub mod backpressure;
pub mod creds;
pub mod error;
pub mod fd;
pub mod frame;
pub mod listener;
pub mod reader;
pub mod stream;
pub mod sys;
pub mod writer;

pub use addr::UnixAddr;
pub use backpressure::{BackpressureConfig, HookEvent, HookRecorder, NoHooks, WriterHooks};
pub use creds::PeerCreds;
pub use error::{is_disconnect, is_would_block};
pub use fd::{count_open_fds, FdList};
pub use frame::{Frame, ALIGN, HEADER_BYTES};
pub use listener::TransportListener;
pub use reader::FramedReader;
pub use stream::TransportStream;
pub use sys::{ControlBuffer, RawRecv};
pub use writer::{FlushOutcome, FramedWriter, SendOutcome};

/// Default listen backlog (`SOMAXCONN` is capped per-kernel; 64 is the
/// classic portable value).
pub const DEFAULT_BACKLOG: i32 = 64;

/// Kernel ceiling on FDs passed in one `SCM_RIGHTS` control message
/// (`SCM_MAX_FD`, 253 on Linux). The protocol's per-message limit
/// (`Limits::fds_per_message`, 64) is far below it; the writer refuses
/// larger batches locally so the kernel never silently truncates a
/// *send*.
pub const SCM_MAX_FD: usize = 253;
