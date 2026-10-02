//! The remote wire protocol: 16-byte envelopes around LDP messages.
//!
//! One envelope is one unit of remote state transfer:
//!
//! ```text
//! offset  size  field
//!      0     4  magic     u32 LE  "LDR1" (0x3152444C)
//!      4     1  version   u8      (1)
//!      5     1  kind      u8      (EnvelopeKind)
//!      6     2  reserved  u16     (must be 0 on receive)
//!      8     4  length    u32 LE  (body bytes, 0..=max_envelope)
//!     12     4  future    u32     (must be 0 on receive; reserved for
//!                                  forward-compatible feature bits)
//!     16   ...  body
//! ```
//!
//! The length is validated against the negotiated cap **before** the
//! body allocation — a claimed 2 GiB body costs the attacker exactly
//! one 16-byte header read, never an allocation (`tests/` corpora
//! re-prove it). Reserved bits must be zero so a future version can
//! give them meaning without ambiguity on old peers.

#![forbid(unsafe_code)]

use std::io::{Read, Write};

use ldp_core::error::{ErrorCode, LdpError, Result};

/// Envelope magic: `"LDR1"` little-endian.
pub const MAGIC: u32 = u32::from_le_bytes(*b"LDR1");

/// Wire protocol version this crate speaks.
pub const WIRE_VERSION: u8 = 1;

/// Envelope header size in bytes.
pub const WIRE_HEADER_BYTES: usize = 16;

/// One envelope kind. Values are frozen once released.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum EnvelopeKind {
    /// Authentication + feature negotiation: 32-byte token, u32
    /// max_envelope, u32 pool cap.
    Hello = 1,
    /// The hub's reply: u32 max_envelope, u32 pool cap (negotiated
    /// minima).
    HelloAck = 2,
    /// One LDP message (16-byte LDP header + aligned payload), raw and
    /// byte-identical to the local wire.
    Data = 3,
    /// FD content as bytes: u32 relay id + u64 size + `size` bytes.
    /// Reconstructed as a memfd by the receiver.
    FdSegment = 4,
    /// Pool bytes refreshed before a commit: u32 relay id + u64 offset
    /// + u64 length + `length` bytes.
    PoolUpdate = 5,
    /// Pool growth: u32 relay id + u64 new size.
    PoolResize = 6,
    /// An eventfd fence's counter snapshot: u64 counter.
    Fence = 7,
    /// Pipe stream open (replaces one fd of the next DATA): u32 relay
    /// id.
    StreamOpen = 8,
    /// Pipe stream chunk: u32 relay id + u32 length + `length` bytes.
    StreamChunk = 9,
    /// Pipe stream end (EOF): u32 relay id.
    StreamFin = 10,
    /// Liveness probe: u64 nonce.
    Ping = 11,
    /// Liveness reply: u64 nonce.
    Pong = 12,
    /// Graceful or diagnostic close: u32 reason code.
    Bye = 13,
}

impl EnvelopeKind {
    /// Parse from the wire byte.
    ///
    /// # Errors
    /// [`LdpError::Malformed`] for unknown values.
    pub fn from_wire(raw: u8) -> Result<EnvelopeKind> {
        Ok(match raw {
            1 => Self::Hello,
            2 => Self::HelloAck,
            3 => Self::Data,
            4 => Self::FdSegment,
            5 => Self::PoolUpdate,
            6 => Self::PoolResize,
            7 => Self::Fence,
            8 => Self::StreamOpen,
            9 => Self::StreamChunk,
            10 => Self::StreamFin,
            11 => Self::Ping,
            12 => Self::Pong,
            13 => Self::Bye,
            other => {
                return Err(LdpError::malformed(
                    ErrorCode::MalformedMessage,
                    format!("remote wire: unknown envelope kind {other}"),
                ))
            }
        })
    }
}

/// `BYE` reason codes (diagnostic; never interpreted for control).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u32)]
pub enum ByeReason {
    /// The peer closed cleanly.
    Clean = 0,
    /// A descriptor class the remote layer does not relay (GPU
    /// descriptors — see the crate docs).
    UnsupportedFd = 1,
    /// The peer violated the envelope ordering contract.
    ProtocolError = 2,
    /// Authentication failed.
    AuthFailed = 3,
    /// The peer exceeded a negotiated cap.
    CapExceeded = 4,
    /// Keepalive expired.
    DeadPeer = 5,
}

/// Relay-side configuration: the caps every envelope is validated
/// against. Defaults are sized for 4K-class pixel work.
#[derive(Clone, Copy, Debug)]
pub struct RemoteConfig {
    /// Largest envelope body accepted/produced (bytes). A 4K XRGB8888
    /// frame is ~33 MiB; 64 MiB leaves headroom for larger strides.
    pub max_envelope: u32,
    /// Total pooled shm bytes a session may hold remotely (bytes).
    pub pool_total_cap: u64,
    /// LDP framing limits mirrored onto the relayed messages.
    pub limits: ldp_core::limits::Limits,
}

impl Default for RemoteConfig {
    fn default() -> Self {
        Self {
            max_envelope: 64 * 1024 * 1024,
            pool_total_cap: 256 * 1024 * 1024,
            limits: ldp_core::limits::Limits::DEFAULT,
        }
    }
}

impl RemoteConfig {
    /// `max_envelope` as `usize` (never exceeds `u32` range).
    #[must_use]
    pub fn envelope_cap(&self) -> usize {
        self.max_envelope as usize
    }

    /// Negotiate the session caps with a peer's offer: both sides run
    /// the same minima.
    #[must_use]
    pub fn negotiate(&self, offered: u32, offered_pool: u64) -> (u32, u64) {
        (
            self.max_envelope.min(offered),
            self.pool_total_cap.min(offered_pool),
        )
    }
}

/// One encoded envelope.
#[derive(Clone, Debug)]
pub struct Envelope {
    /// The kind.
    pub kind: EnvelopeKind,
    /// The body bytes.
    pub body: Vec<u8>,
}

impl Envelope {
    /// Build an envelope from a body.
    #[must_use]
    pub fn new(kind: EnvelopeKind, body: Vec<u8>) -> Envelope {
        Envelope { kind, body }
    }

    /// Build a `PING` with a nonce.
    #[must_use]
    pub fn ping(nonce: u64) -> Envelope {
        Envelope::new(EnvelopeKind::Ping, nonce.to_le_bytes().to_vec())
    }

    /// Build a `PONG` with a nonce.
    #[must_use]
    pub fn pong(nonce: u64) -> Envelope {
        Envelope::new(EnvelopeKind::Pong, nonce.to_le_bytes().to_vec())
    }

    /// Build a `BYE` with a reason.
    #[must_use]
    pub fn bye(reason: ByeReason) -> Envelope {
        Envelope::new(EnvelopeKind::Bye, (reason as u32).to_le_bytes().to_vec())
    }

    /// Build a `DATA` envelope around one raw LDP message.
    #[must_use]
    pub fn data(message: &[u8]) -> Envelope {
        Envelope::new(EnvelopeKind::Data, message.to_vec())
    }

    /// Build an `FD_SEGMENT` from whole-file content.
    #[must_use]
    pub fn fd_segment(relay_id: u32, content: &[u8]) -> Envelope {
        let mut body = Vec::with_capacity(12 + content.len());
        body.extend_from_slice(&relay_id.to_le_bytes());
        body.extend_from_slice(&(content.len() as u64).to_le_bytes());
        body.extend_from_slice(content);
        Envelope::new(EnvelopeKind::FdSegment, body)
    }

    /// Build a `POOL_UPDATE`.
    #[must_use]
    pub fn pool_update(relay_id: u32, offset: u64, bytes: &[u8]) -> Envelope {
        let mut body = Vec::with_capacity(20 + bytes.len());
        body.extend_from_slice(&relay_id.to_le_bytes());
        body.extend_from_slice(&offset.to_le_bytes());
        body.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        body.extend_from_slice(bytes);
        Envelope::new(EnvelopeKind::PoolUpdate, body)
    }

    /// Build a `POOL_RESIZE`.
    #[must_use]
    pub fn pool_resize(relay_id: u32, new_size: u64) -> Envelope {
        let mut body = Vec::with_capacity(12);
        body.extend_from_slice(&relay_id.to_le_bytes());
        body.extend_from_slice(&new_size.to_le_bytes());
        Envelope::new(EnvelopeKind::PoolResize, body)
    }

    /// Build a `FENCE` from an eventfd counter snapshot.
    #[must_use]
    pub fn fence(counter: u64) -> Envelope {
        Envelope::new(EnvelopeKind::Fence, counter.to_le_bytes().to_vec())
    }

    /// Build a `STREAM` open.
    #[must_use]
    pub fn stream_open(relay_id: u32) -> Envelope {
        Envelope::new(EnvelopeKind::StreamOpen, relay_id.to_le_bytes().to_vec())
    }

    /// Build a `STREAM` chunk.
    #[must_use]
    pub fn stream_chunk(relay_id: u32, bytes: &[u8]) -> Envelope {
        let mut body = Vec::with_capacity(8 + bytes.len());
        body.extend_from_slice(&relay_id.to_le_bytes());
        body.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        body.extend_from_slice(bytes);
        Envelope::new(EnvelopeKind::StreamChunk, body)
    }

    /// Build a `STREAM` fin.
    #[must_use]
    pub fn stream_fin(relay_id: u32) -> Envelope {
        Envelope::new(EnvelopeKind::StreamFin, relay_id.to_le_bytes().to_vec())
    }

    /// Decode an `FD_SEGMENT` body.
    ///
    /// # Errors
    /// [`LdpError::Malformed`] when the body is not 12 + size bytes.
    pub fn parse_fd_segment(body: &[u8]) -> Result<(u32, &[u8])> {
        // parse_prefix validates id + declared size + carried size in
        // one pass and returns exactly (id, content).
        parse_prefix(body, 12)
    }

    /// Decode a `POOL_UPDATE` body.
    ///
    /// # Errors
    /// [`LdpError::Malformed`] when the body is not 20 + length bytes.
    pub fn parse_pool_update(body: &[u8]) -> Result<(u32, u64, &[u8])> {
        if body.len() < 20 {
            return Err(short("POOL_UPDATE", 20, body.len()));
        }
        let id = u32::from_le_bytes(body[0..4].try_into().expect("4 bytes"));
        let offset = u64::from_le_bytes(body[4..12].try_into().expect("8 bytes"));
        let length = u64::from_le_bytes(body[12..20].try_into().expect("8 bytes"));
        let bytes = &body[20..];
        if length as usize != bytes.len() {
            return Err(LdpError::malformed(
                ErrorCode::MalformedMessage,
                format!(
                    "remote wire: POOL_UPDATE claims {length} bytes, carries {}",
                    bytes.len()
                ),
            ));
        }
        Ok((id, offset, bytes))
    }

    /// Decode a `POOL_RESIZE` body.
    ///
    /// # Errors
    /// [`LdpError::Malformed`] for bodies that are not 12 bytes.
    pub fn parse_pool_resize(body: &[u8]) -> Result<(u32, u64)> {
        if body.len() != 12 {
            return Err(short("POOL_RESIZE", 12, body.len()));
        }
        let id = u32::from_le_bytes(body[0..4].try_into().expect("4 bytes"));
        let size = u64::from_le_bytes(body[4..12].try_into().expect("8 bytes"));
        Ok((id, size))
    }

    /// Decode a `FENCE` body.
    ///
    /// # Errors
    /// [`LdpError::Malformed`] for bodies that are not 8 bytes.
    pub fn parse_fence(body: &[u8]) -> Result<u64> {
        if body.len() != 8 {
            return Err(short("FENCE", 8, body.len()));
        }
        Ok(u64::from_le_bytes(body[0..8].try_into().expect("8 bytes")))
    }

    /// Decode a `STREAM` open / fin body (both are the relay id).
    ///
    /// # Errors
    /// [`LdpError::Malformed`] for bodies that are not 4 bytes.
    pub fn parse_stream_id(body: &[u8]) -> Result<u32> {
        if body.len() != 4 {
            return Err(short("STREAM", 4, body.len()));
        }
        Ok(u32::from_le_bytes(body[0..4].try_into().expect("4 bytes")))
    }

    /// Decode a `STREAM` chunk body.
    ///
    /// # Errors
    /// [`LdpError::Malformed`] when the body is not 8 + length bytes.
    pub fn parse_stream_chunk(body: &[u8]) -> Result<(u32, &[u8])> {
        if body.len() < 8 {
            return Err(short("STREAM_CHUNK", 8, body.len()));
        }
        let id = u32::from_le_bytes(body[0..4].try_into().expect("4 bytes"));
        let length = u32::from_le_bytes(body[4..8].try_into().expect("4 bytes"));
        let bytes = &body[8..];
        if length as usize != bytes.len() {
            return Err(LdpError::malformed(
                ErrorCode::MalformedMessage,
                format!(
                    "remote wire: STREAM_CHUNK claims {length} bytes, carries {}",
                    bytes.len()
                ),
            ));
        }
        Ok((id, bytes))
    }

    /// Decode a `PING` / `PONG` body.
    ///
    /// # Errors
    /// [`LdpError::Malformed`] for bodies that are not 8 bytes.
    pub fn parse_nonce(body: &[u8]) -> Result<u64> {
        if body.len() != 8 {
            return Err(short("NONCE", 8, body.len()));
        }
        Ok(u64::from_le_bytes(body[0..8].try_into().expect("8 bytes")))
    }

    /// Decode a `BYE` body.
    ///
    /// # Errors
    /// [`LdpError::Malformed`] for bodies that are not 4 bytes.
    pub fn parse_bye(body: &[u8]) -> Result<u32> {
        if body.len() != 4 {
            return Err(short("BYE", 4, body.len()));
        }
        Ok(u32::from_le_bytes(body[0..4].try_into().expect("4 bytes")))
    }

    /// Encode onto a stream: 16-byte header + body.
    ///
    /// # Errors
    /// [`LdpError::Limit`] when the body exceeds `cap`;
    /// [`LdpError::Io`] on write failure.
    pub fn write_to<W: Write>(&self, sink: &mut W, cap: usize) -> Result<()> {
        if self.body.len() > cap {
            return Err(envelope_limit(self.body.len() as u64));
        }
        let mut header = [0u8; WIRE_HEADER_BYTES];
        header[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        header[4] = WIRE_VERSION;
        header[5] = self.kind as u8;
        // header[6..8] reserved zero; header[12..16] future zero.
        header[8..12].copy_from_slice(&(self.body.len() as u32).to_le_bytes());
        sink.write_all(&header)
            .map_err(io("sending remote envelope header"))?;
        sink.write_all(&self.body)
            .map_err(io("sending remote envelope body"))?;
        Ok(())
    }
}

/// Read exactly one envelope from a blocking stream.
///
/// The length field is validated against `cap` **before** the body
/// allocation: a lying peer never costs more than the 16-byte header.
///
/// # Errors
/// [`LdpError::Malformed`] for bad magic/version/reserved bits/kind or
/// an over-cap length; [`LdpError::Io`] on read failure (including
/// clean EOF, reported as a disconnect).
pub fn read_envelope<R: Read>(source: &mut R, cap: usize) -> Result<Envelope> {
    let mut header = [0u8; WIRE_HEADER_BYTES];
    source
        .read_exact(&mut header)
        .map_err(io("reading remote envelope header"))?;
    let magic = u32::from_le_bytes(header[0..4].try_into().expect("4 bytes"));
    if magic != MAGIC {
        return Err(LdpError::malformed(
            ErrorCode::MalformedMessage,
            format!("remote wire: bad magic {magic:#010x}"),
        ));
    }
    if header[4] != WIRE_VERSION {
        return Err(LdpError::malformed(
            ErrorCode::UnsupportedVersion,
            format!(
                "remote wire: version {} unsupported (local {WIRE_VERSION})",
                header[4]
            ),
        ));
    }
    let kind = EnvelopeKind::from_wire(header[5])?;
    if header[6] != 0 || header[7] != 0 {
        return Err(LdpError::malformed(
            ErrorCode::MalformedMessage,
            "remote wire: reserved header bits set",
        ));
    }
    if header[12..16] != [0, 0, 0, 0] {
        return Err(LdpError::malformed(
            ErrorCode::MalformedMessage,
            "remote wire: future header bits set",
        ));
    }
    let length = u32::from_le_bytes(header[8..12].try_into().expect("4 bytes")) as usize;
    if length > cap {
        return Err(envelope_limit(length as u64));
    }
    let mut body = vec![0u8; length];
    source
        .read_exact(&mut body)
        .map_err(io("reading remote envelope body"))?;
    Ok(Envelope { kind, body })
}

/// Decode a fixed-size prefix (relay id + payload carrying its own
/// declared size).
fn parse_prefix(body: &[u8], min: usize) -> Result<(u32, &[u8])> {
    if body.len() < min {
        return Err(short("FD_SEGMENT", min, body.len()));
    }
    let id = u32::from_le_bytes(body[0..4].try_into().expect("4 bytes"));
    let size = u64::from_le_bytes(body[4..12].try_into().expect("8 bytes"));
    let rest = &body[12..];
    if size as usize != rest.len() {
        return Err(LdpError::malformed(
            ErrorCode::MalformedMessage,
            format!(
                "remote wire: FD_SEGMENT claims {size} bytes, carries {}",
                rest.len()
            ),
        ));
    }
    Ok((id, rest))
}

fn short(what: &str, want: usize, got: usize) -> LdpError {
    LdpError::malformed(
        ErrorCode::MalformedMessage,
        format!("remote wire: {what} body is {got} bytes, expected {want}"),
    )
}

fn io(what: &'static str) -> impl Fn(std::io::Error) -> LdpError {
    move |e| {
        LdpError::Io(std::sync::Arc::new(std::io::Error::other(format!(
            "{what}: {e}"
        ))))
    }
}

/// The negotiated envelope cap was exceeded (read or write side).
fn envelope_limit(value: u64) -> LdpError {
    LdpError::Limit {
        kind: ldp_core::error::LimitKind::EventQueueBytes,
        value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(env: &Envelope, cap: usize) -> Envelope {
        let mut bytes = Vec::new();
        env.write_to(&mut bytes, cap).expect("encode");
        let mut cursor = std::io::Cursor::new(&bytes);
        read_envelope(&mut cursor, cap).expect("decode")
    }

    #[test]
    fn every_kind_round_trips() {
        let samples = [
            Envelope::ping(7),
            Envelope::pong(7),
            Envelope::bye(ByeReason::Clean),
            Envelope::data(&[0u8; 24]),
            Envelope::fd_segment(3, b"pool-bytes"),
            Envelope::pool_update(3, 4096, b"pixels"),
            Envelope::pool_resize(3, 8192),
            Envelope::fence(1),
            Envelope::stream_open(5),
            Envelope::stream_chunk(5, b"chunk"),
            Envelope::stream_fin(5),
        ];
        for env in samples {
            let back = round_trip(&env, 1024);
            assert_eq!(back.kind, env.kind);
            assert_eq!(back.body, env.body);
        }
    }

    #[test]
    fn body_parsers_reject_short_and_lying_bodies() {
        assert!(Envelope::parse_pool_update(&[0u8; 19]).is_err());
        let mut lying = Vec::new();
        lying.extend_from_slice(&3u32.to_le_bytes());
        lying.extend_from_slice(&0u64.to_le_bytes());
        lying.extend_from_slice(&9u64.to_le_bytes()); // claims 9
        lying.extend_from_slice(b"12345678"); // carries 8
        assert!(Envelope::parse_pool_update(&lying).is_err());
        assert!(Envelope::parse_stream_chunk(&[0u8; 7]).is_err());
        // An 8-byte body is a VALID empty chunk (id + length 0).
        assert!(Envelope::parse_fence(&[0u8; 7]).is_err());
        assert!(Envelope::parse_nonce(&[0u8; 9]).is_err());
        assert!(Envelope::parse_bye(&[0u8; 5]).is_err());
        assert!(Envelope::parse_stream_id(&[0u8; 3]).is_err());
        assert!(Envelope::parse_pool_resize(&[0u8; 11]).is_err());
        assert!(Envelope::parse_fd_segment(&[0u8; 11]).is_err());
    }

    #[test]
    fn body_parsers_decode_the_canonical_forms() {
        let seg = Envelope::fd_segment(9, b"0123456789");
        let (_, content) = Envelope::parse_fd_segment(&seg.body).expect("fd segment");
        assert_eq!(content, b"0123456789");

        let upd = Envelope::pool_update(2, 77, b"xyz");
        let (id, off, bytes) = Envelope::parse_pool_update(&upd.body).expect("pool update");
        assert_eq!((id, off), (2, 77));
        assert_eq!(bytes, b"xyz");

        let res = Envelope::pool_resize(2, 99);
        let (id, size) = Envelope::parse_pool_resize(&res.body).expect("pool resize");
        assert_eq!((id, size), (2, 99));

        assert_eq!(Envelope::parse_fence(&Envelope::fence(5).body).unwrap(), 5);
        assert_eq!(Envelope::parse_nonce(&Envelope::ping(11).body).unwrap(), 11);
        assert_eq!(
            Envelope::parse_bye(&Envelope::bye(ByeReason::DeadPeer).body).unwrap(),
            ByeReason::DeadPeer as u32
        );
        assert_eq!(
            Envelope::parse_stream_id(&Envelope::stream_open(4).body).unwrap(),
            4
        );
        let chunk = Envelope::stream_chunk(4, b"ab");
        let (id, bytes) = Envelope::parse_stream_chunk(&chunk.body).expect("chunk");
        assert_eq!(id, 4);
        assert_eq!(bytes, b"ab");
    }

    #[test]
    fn bad_magic_is_rejected_before_any_body_read() {
        let mut bytes = Vec::new();
        Envelope::ping(1).write_to(&mut bytes, 64).expect("encode");
        bytes[0] = b'X';
        let mut cursor = std::io::Cursor::new(&bytes);
        assert!(read_envelope(&mut cursor, 64).is_err());
    }

    #[test]
    fn version_mismatch_is_rejected() {
        let mut bytes = Vec::new();
        Envelope::ping(1).write_to(&mut bytes, 64).expect("encode");
        bytes[4] = 2;
        let mut cursor = std::io::Cursor::new(&bytes);
        assert!(read_envelope(&mut cursor, 64).is_err());
    }

    #[test]
    fn reserved_and_future_bits_must_be_zero() {
        for at in [6usize, 7, 12, 13, 14, 15] {
            let mut bytes = Vec::new();
            Envelope::ping(1).write_to(&mut bytes, 64).expect("encode");
            bytes[at] = 1;
            let mut cursor = std::io::Cursor::new(&bytes);
            assert!(
                read_envelope(&mut cursor, 64).is_err(),
                "bit at offset {at} must be rejected"
            );
        }
    }

    #[test]
    fn length_bomb_is_rejected_without_allocation() {
        // A ~2 GiB claim against a 1 KiB cap: the reader must fail on
        // the header alone. The body would never fit the pipe anyway;
        // the property under test is fail-fast, not memory use.
        let mut header = [0u8; WIRE_HEADER_BYTES];
        header[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        header[4] = WIRE_VERSION;
        header[5] = EnvelopeKind::Data as u8;
        header[8..12].copy_from_slice(&2_000_000_000u32.to_le_bytes());
        let mut cursor = std::io::Cursor::new(&header);
        let err = read_envelope(&mut cursor, 1024).expect_err("must reject");
        assert!(matches!(err, LdpError::Limit { .. }));
    }

    #[test]
    fn oversized_body_write_is_refused_locally() {
        let env = Envelope::data(&vec![0u8; 4096]);
        let mut sink = Vec::new();
        assert!(env.write_to(&mut sink, 1024).is_err());
        assert!(sink.is_empty());
    }

    #[test]
    fn unknown_kind_is_rejected() {
        assert!(EnvelopeKind::from_wire(0).is_err());
        assert!(EnvelopeKind::from_wire(14).is_err());
        assert!(EnvelopeKind::from_wire(255).is_err());
    }

    #[test]
    fn negotiated_caps_are_minima() {
        let cfg = RemoteConfig {
            max_envelope: 1024,
            pool_total_cap: 4096,
            ..RemoteConfig::default()
        };
        assert_eq!(cfg.negotiate(512, 8192), (512, 4096));
        assert_eq!(cfg.negotiate(2048, 2048), (1024, 2048));
    }
}
