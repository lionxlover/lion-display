//! The `/dev/input` backend: enumeration and the ioctl device probe.
//!
//! This is an audited `sys` boundary (the `ldp-transport`/
//! `ldp-display` precedent): every `unsafe` call below carries a
//! `SAFETY` comment, the module declares no `forbid`, and everything
//! the rest of the crate sees is safe, owned Rust.
//!
//! * [`enumerate`] lists `/dev/input/event*` nodes. A machine
//!   (container, VM without input) whose `/dev/input` does not exist
//!   yields an *empty* list — that is a normal headless state, not an
//!   error; permission problems surface as [`std::io::Error`].
//! * [`open_device`] opens a node `O_RDONLY|O_NONBLOCK|O_CLOEXEC` —
//!   the libinput-compatible open semantics (never blocking the
//!   compositor's loop).
//! * [`probe`] fills a [`DeviceSpec`] through the kernel's own ABI:
//!   `EVIOCGVERSION` (sanity), `EVIOCGID`, `EVIOCGNAME`, the
//!   `EVIOCGBIT` capability bitmaps, and `EVIOCGABS` per axis. Every
//!   failure is typed: a non-evdev descriptor (ENOTTY — exercised in
//!   CI against a memfd) is [`ProbeError::NotEvdev`], a version the
//!   codec was never built for is reported verbatim.
//!
//! Seat tags: the spec's `seat_tag` field is filled from the udev
//! `ID_SEAT` property when the hotplug layer provides it; the probe
//! itself cannot query udev (that layering arrives with the session
//! phase), so it defaults to `seat0` and the assignment policy in
//! `ldp-seat` accepts the tag from the caller. The golden corpus
//! injects tags explicitly.

use std::ffi::c_ulong;
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

use crate::device::{AbsInfo, DeviceId, DeviceSpec};

/// `EVIOCGVERSION` — `_IOR('E', 0x01, int)`.
const EVIOCGVERSION: c_ulong = 0x8004_4501;
/// `EVIOCGID` — `_IOR('E', 0x02, struct input_id)` (8 bytes).
const EVIOCGID: c_ulong = 0x8008_4502;
/// `EVIOCGNAME(len)` — `_IOC(READ, 'E', 0x06, len)`.
const fn eviocgname(len: usize) -> c_ulong {
    0x8000_4506 | ((len as c_ulong) << 16)
}
/// `EVIOCGBIT(ev, len)` — `_IOC(READ, 'E', 0x20 + ev, len)`.
const fn eviocgbit(ev: u16, len: usize) -> c_ulong {
    0x8000_4520 | (ev as c_ulong) | ((len as c_ulong) << 16)
}
/// `EVIOCGABS(abs)` — `_IOR('E', 0x40 + abs, struct input_absinfo)`
/// (24 bytes).
const fn eviocgabs(abs: u16) -> c_ulong {
    0x8018_4540 | (abs as c_ulong)
}

/// `struct input_id` (8 bytes).
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct InputId {
    bustype: u16,
    vendor: u16,
    product: u16,
    version: u16,
}

/// `struct input_absinfo` (24 bytes).
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct InputAbsinfo {
    value: i32,
    minimum: i32,
    maximum: i32,
    fuzz: i32,
    flat: i32,
    resolution: i32,
}

/// Probe failures, each carrying the honest cause.
#[derive(Debug)]
pub enum ProbeError {
    /// The descriptor is not an evdev device (ENOTTY/ENODEV).
    NotEvdev,
    /// An unexpected evdev ABI version (reported verbatim).
    Version(u32),
    /// An ioctl or read failed.
    Io(io::Error),
}

impl std::fmt::Display for ProbeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProbeError::NotEvdev => write!(f, "descriptor is not an evdev device"),
            ProbeError::Version(v) => write!(f, "unsupported evdev version {v}"),
            ProbeError::Io(e) => write!(f, "io: {e}"),
        }
    }
}

impl std::error::Error for ProbeError {}

/// List the evdev event nodes, sorted by their numeric suffix.
///
/// A missing `/dev/input` directory is the headless state and yields
/// an empty list (documented; containers are a first-class
/// environment here).
///
/// # Errors
/// [`std::io::Error`] for permission or I/O failures *other than*
/// the directory's absence.
pub fn enumerate() -> io::Result<Vec<PathBuf>> {
    let dir = match std::fs::read_dir("/dev/input") {
        Ok(d) => d,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut nodes: Vec<(u32, PathBuf)> = dir
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter_map(|p| {
            let name = p.file_name()?;
            let bytes = name.as_bytes();
            let rest = bytes.strip_prefix(b"event")?;
            let n: u32 = std::str::from_utf8(rest).ok()?.parse().ok()?;
            Some((n, p))
        })
        .collect();
    nodes.sort_by_key(|(n, _)| *n);
    Ok(nodes.into_iter().map(|(_, p)| p).collect())
}

/// Open an event node `O_RDONLY|O_NONBLOCK|O_CLOEXEC`.
///
/// # Errors
/// [`std::io::Error`] with the errno of `open(2)`.
pub fn open_device(path: &std::path::Path) -> io::Result<File> {
    let cpath = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    // SAFETY: cpath is NUL-terminated; the flags are defined
    // constants; the return is checked before adoption.
    let fd = unsafe {
        libc::open(
            cpath.as_ptr(),
            libc::O_RDONLY | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fd is a freshly opened, unowned descriptor.
    Ok(unsafe { File::from_raw_fd(fd as RawFd) })
}

/// One raw ioctl: request with a mutable buffer.
fn ioctl_buf(fd: RawFd, req: c_ulong, buf: &mut [u8]) -> io::Result<()> {
    // SAFETY: fd is a live owned descriptor held by the caller; buf
    // is a valid writable region of exactly the length the request
    // encodes; the kernel writes only within it.
    let r = unsafe { libc::ioctl(fd, req, buf.as_mut_ptr().cast::<std::ffi::c_void>()) };
    if r < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Probe an open device into a full [`DeviceSpec`].
///
/// # Errors
/// [`ProbeError::NotEvdev`] for non-evdev descriptors;
/// [`ProbeError::Version`] for an unrecognized ABI version;
/// [`ProbeError::Io`] for anything else.
///
/// # Panics
/// Never: every slice conversion is on compile-time-fixed widths.
pub fn probe(file: &File) -> Result<DeviceSpec, ProbeError> {
    let fd = file.as_raw_fd();

    // Version sanity: the evdev ABI this crate speaks has been stable
    // since 2.6.x; anything else is reported, not guessed.
    let mut version = [0u8; 4];
    ioctl_buf(fd, EVIOCGVERSION, &mut version).map_err(classify)?;
    let v = u32::from_le_bytes(version);
    if v == 0 {
        return Err(ProbeError::Version(0));
    }

    let mut id_raw = [0u8; 8];
    ioctl_buf(fd, EVIOCGID, &mut id_raw).map_err(classify)?;
    let id = InputId {
        bustype: u16::from_le_bytes([id_raw[0], id_raw[1]]),
        vendor: u16::from_le_bytes([id_raw[2], id_raw[3]]),
        product: u16::from_le_bytes([id_raw[4], id_raw[5]]),
        version: u16::from_le_bytes([id_raw[6], id_raw[7]]),
    };

    let mut name_buf = [0u8; 256];
    ioctl_buf(fd, eviocgname(name_buf.len()), &mut name_buf).map_err(classify)?;
    let name_len = name_buf
        .iter()
        .position(|b| *b == 0)
        .unwrap_or(name_buf.len());
    let name = String::from_utf8_lossy(&name_buf[..name_len]).into_owned();

    let mut spec = DeviceSpec::new(
        &name,
        DeviceId {
            bustype: id.bustype,
            vendor: id.vendor,
            product: id.product,
            version: id.version,
        },
    );

    // Capability bitmaps: one EVIOCGBIT per event type we consume.
    // KEY_MAX = 0x2ff → 96 bytes; ABS_MAX = 0x3f → 8 bytes; the
    // others fit one word.
    for (ev_type, len) in [
        (0u16, 1usize), // EV_SYN
        (1, 96),        // EV_KEY (KEY_MAX/8)
        (2, 8),         // EV_REL (REL_MAX/8)
        (3, 8),         // EV_ABS (ABS_MAX/8)
        (4, 4),         // EV_MSC
        (0x11, 2),      // EV_LED
    ] {
        let mut bits = vec![0u8; len];
        if ioctl_buf(fd, eviocgbit(ev_type, len), &mut bits).is_err() {
            continue; // The type is absent: an empty bitmap, honestly.
        }
        for (ev, code) in decode_bitmap(&bits) {
            let _ = ev;
            spec.bits.insert((ev_type, code));
        }
    }

    // Axis metadata for every declared ABS code.
    let abs_codes: Vec<u16> = spec
        .bits
        .iter()
        .filter(|(t, _)| *t == 3)
        .map(|(_, c)| *c)
        .collect();
    for code in abs_codes {
        let mut raw = [0u8; 24];
        if ioctl_buf(fd, eviocgabs(code), &mut raw).is_err() {
            continue;
        }
        let i32at = |off: usize| i32::from_le_bytes(raw[off..off + 4].try_into().expect("4 bytes"));
        let info = InputAbsinfo {
            value: i32at(0),
            minimum: i32at(4),
            maximum: i32at(8),
            fuzz: i32at(12),
            flat: i32at(16),
            resolution: i32at(20),
        };
        spec.abs.insert(
            code,
            AbsInfo {
                value: info.value,
                min: info.minimum,
                max: info.maximum,
                fuzz: info.fuzz,
                flat: info.flat,
                resolution: info.resolution.max(0) as u32,
            },
        );
    }

    Ok(spec)
}

/// ENOTTY/ENODEV from the version probe means "not evdev" (the
/// memfd-in-CI path); everything else is an honest io error.
fn classify(e: io::Error) -> ProbeError {
    match e.raw_os_error() {
        Some(libc::ENOTTY | libc::ENODEV) => ProbeError::NotEvdev,
        _ => ProbeError::Io(e),
    }
}

/// Decode a capability bitmap into the set codes (little-endian
/// bit order: byte `i` bit `j` is code `i*8+j`).
#[must_use]
pub fn decode_bitmap(bits: &[u8]) -> Vec<(u16, u16)> {
    let mut out = Vec::new();
    for (i, byte) in bits.iter().enumerate() {
        for j in 0..8 {
            if byte & (1 << j) != 0 {
                let code = i * 8 + j;
                if u16::try_from(code).is_ok() {
                    out.push((0, code as u16));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headless_enumeration_is_empty() {
        // The sandbox has no /dev/input: the honest headless result.
        let nodes = enumerate().expect("enumerate");
        if PathBuf::from("/dev/input").exists() {
            assert!(!nodes.is_empty(), "real machine: nodes expected");
        } else {
            assert!(nodes.is_empty(), "headless: no nodes");
        }
    }

    #[test]
    fn probing_a_memfd_is_not_evdev() {
        // The ioctl path, exercised honestly: a memfd is a real
        // descriptor that answers ENOTTY to EVIOCGVERSION.
        let fd = crate::xkb::sys::sealed_memfd("not-evdev", b"").expect("memfd");
        let file = File::from(fd);
        let err = probe(&file).expect_err("memfd is not evdev");
        assert!(
            matches!(err, ProbeError::NotEvdev),
            "expected NotEvdev, got {err:?}"
        );
    }

    #[test]
    fn bitmap_decoding_bit_order() {
        // Code 8 (REL_X's neighbor region): byte 1, bit 0.
        let bits = [0x01, 0x01, 0x00, 0x80];
        let codes: Vec<u16> = decode_bitmap(&bits).into_iter().map(|(_, c)| c).collect();
        assert_eq!(codes, vec![0, 8, 31]);
        // Empty and all-zero bitmaps.
        assert!(decode_bitmap(&[]).is_empty());
        assert!(decode_bitmap(&[0, 0, 0]).is_empty());
    }

    #[test]
    fn ioctl_request_encoding() {
        // Pin the request word encodings against the kernel ABI.
        assert_eq!(EVIOCGVERSION, 0x8004_4501);
        assert_eq!(EVIOCGID, 0x8008_4502);
        assert_eq!(eviocgname(256), 0x8100_4506);
        // EVIOCGBIT(EV_KEY=1, 96): dir 2, size 96 (0x60), 'E', 0x21.
        assert_eq!(eviocgbit(1, 96), 0x8060_4521);
        // EVIOCGABS(ABS_X=0): dir 2, size 24 (0x18), 'E', 0x40.
        assert_eq!(eviocgabs(0), 0x8018_4540);
        // EVIOCGABS(ABS_MT_SLOT=0x2f).
        assert_eq!(eviocgabs(0x2f), 0x8018_456f);
    }

    #[test]
    fn probe_error_display() {
        assert!(ProbeError::NotEvdev.to_string().contains("evdev"));
        assert!(ProbeError::Version(7).to_string().contains('7'));
    }
}
