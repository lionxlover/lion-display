//! The TCP session link: authentication, negotiated caps, keepalive,
//! and teardown.
//!
//! One remote session is one TCP connection. Before any protocol byte
//! flows, the edges exchange a `HELLO` carrying a 32-byte bearer token
//! and their envelope-cap offers; the hub compares the token in
//! constant time and answers `HELLO_ACK` with the minima both sides
//! will enforce. A failed handshake costs exactly one round trip.
//!
//! # Liveness
//!
//! TCP itself can take many minutes to notice a dead peer. Sessions
//! that enable [`Keepalive`] run a monitor thread that pings an idle
//! peer and, absent any traffic within the pong deadline, shuts the
//! session's sockets down in both directions — which unblocks every
//! pump thread parked in a blocking syscall, so the session can be
//! joined and its descriptors reclaimed. Monitors are the *only*
//! component allowed to call the `sys` shutdown wrapper during a
//! session's life; ordinary data flow never does.
//!
//! # Teardown discipline
//!
//! Every descriptor-bearing stream of a session is also duplicated
//! into the session's [`SocketSet`]. `SocketSet::shutdown_all` runs at
//! most once, on fds the set itself keeps open for the session's whole
//! lifetime — the use-after-close race (an fd number reused by an
//! unrelated `memfd` between a pump's drop and a sibling's shutdown)
//! is designed out, not hoped away.

#![forbid(unsafe_code)]

use std::net::{TcpStream, ToSocketAddrs};
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc::Sender, Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ldp_core::error::{ErrorCode, LdpError, Result};

use crate::wire::{read_envelope, ByeReason, Envelope, EnvelopeKind, RemoteConfig};

/// Bearer-token length (bytes).
pub const AUTH_TOKEN_BYTES: usize = 32;

/// The negotiated session caps: the minima of both sides' offers.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SessionCaps {
    /// Maximum envelope body (bytes).
    pub max_envelope: u32,
    /// Maximum total pooled shm bytes per session.
    pub pool_total_cap: u64,
}

impl SessionCaps {
    /// The envelope cap as `usize`.
    #[must_use]
    pub fn envelope_cap(&self) -> usize {
        self.max_envelope as usize
    }
}

/// Keepalive policy. `idle_ping` of zero disables the monitor.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Keepalive {
    /// Send a `PING` when no traffic has been seen for this long.
    pub idle_ping: Duration,
    /// Tear the session down when no traffic has been seen for this
    /// long (must exceed `idle_ping`).
    pub pong_deadline: Duration,
}

impl Default for Keepalive {
    fn default() -> Self {
        Keepalive {
            idle_ping: Duration::from_secs(10),
            pong_deadline: Duration::from_secs(20),
        }
    }
}

impl Keepalive {
    /// Whether the monitor runs at all.
    #[must_use]
    pub fn enabled(&self) -> bool {
        self.idle_ping.as_nanos() > 0 && self.pong_deadline > self.idle_ping
    }
}

/// A session's shared liveness clock (ms since the epoch) and dead
/// flag. Every received envelope refreshes it; monitors sample it.
#[derive(Debug, Default)]
pub struct Liveness {
    last_seen_ms: AtomicU64,
    dead: AtomicBool,
}

impl Liveness {
    /// A fresh clock, touched at birth (a session that never saw
    /// traffic would otherwise count as instantly idle).
    #[must_use]
    pub fn new() -> Liveness {
        let this = Liveness::default();
        this.touch();
        this
    }

    /// Mark traffic seen now.
    pub fn touch(&self) {
        self.last_seen_ms.store(now_ms(), Ordering::Release);
    }

    /// Milliseconds since traffic was last seen.
    #[must_use]
    pub fn idle_ms(&self) -> u64 {
        now_ms().saturating_sub(self.last_seen_ms.load(Ordering::Acquire))
    }

    /// Whether the session has been torn down.
    #[must_use]
    pub fn is_dead(&self) -> bool {
        self.dead.load(Ordering::Acquire)
    }

    /// Tear the session down (idempotent).
    pub fn kill(&self) {
        self.dead.store(true, Ordering::Release);
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The session's descriptor set: duplicated, kept open for the
/// session's lifetime, shut down at most once.
#[derive(Debug, Default)]
pub struct SocketSet {
    fds: Mutex<Vec<std::os::fd::OwnedFd>>,
    done: AtomicBool,
}

impl SocketSet {
    /// Duplicate `fd` into the set (the set's copy stays open until
    /// the set drops, even if the caller's own handle closes). Accepts
    /// anything exposing a descriptor: owned fds, `TcpStream`s,
    /// `TransportStream`s.
    ///
    /// # Errors
    /// [`LdpError::Io`] when the descriptor cannot be duplicated.
    pub fn add(&self, fd: &impl std::os::fd::AsFd) -> Result<()> {
        let dup = fd
            .as_fd()
            .try_clone_to_owned()
            .map_err(|e| LdpError::Io(Arc::new(std::io::Error::other(e))))?;
        self.fds.lock().expect("socket set lock").push(dup);
        Ok(())
    }

    /// Shut every descriptor down in both directions, at most once.
    /// Idempotent and best-effort: teardown races are fine *here*
    /// because the set's own duplicates hold every fd number open.
    pub fn shutdown_all(&self) {
        if self.done.swap(true, Ordering::AcqRel) {
            return;
        }
        let fds = self.fds.lock().expect("socket set lock");
        for fd in fds.iter() {
            // Ignore errors: a peer that already closed makes this
            // ENOTCONN, which is exactly the outcome we wanted.
            let _ = crate::sys::shutdown_socket(fd.as_raw_fd());
        }
    }
}

/// The keepalive monitor loop. Runs until the session dies or the
/// deadline passes; sends `PING`s through `out` and tears the sockets
/// down through `sockets` when the peer stays silent.
pub fn keepalive_monitor(
    policy: Keepalive,
    liveness: Arc<Liveness>,
    sockets: Arc<SocketSet>,
    out: EnvelopeSink,
) {
    let mut pending_ping = false;
    loop {
        if liveness.is_dead() {
            return;
        }
        std::thread::sleep(policy.idle_ping.min(Duration::from_secs(1)));
        let idle = liveness.idle_ms();
        if idle >= policy.pong_deadline.as_millis() as u64 {
            // Dead peer: unblock every pump by shutting the sockets
            // down, then let the session join.
            liveness.kill();
            sockets.shutdown_all();
            return;
        }
        if idle >= policy.idle_ping.as_millis() as u64 && !pending_ping {
            if out.send(Envelope::ping(now_ms())).is_ok() {
                pending_ping = true;
            }
        } else if idle < policy.idle_ping.as_millis() as u64 {
            pending_ping = false;
        }
    }
}

/// Where a monitor (or any auxiliary thread) submits envelopes for the
/// session's writer thread to serialize onto the TCP stream. Cloned
/// per thread; a send fails only when the writer thread has exited
/// (the session is ending concurrently).
pub type EnvelopeSink = Sender<Envelope>;

/// Enqueue an envelope onto the session's writer channel.
///
/// # Errors
/// [`LdpError::Io`] when the writer thread has gone away (session
/// ending concurrently).
pub fn enqueue(sink: &EnvelopeSink, env: Envelope) -> Result<()> {
    sink.send(env)
        .map_err(|_| disconnected("the remote writer thread has exited"))
}

fn disconnected(what: &str) -> LdpError {
    LdpError::Io(Arc::new(std::io::Error::other(format!(
        "ldp-remote: {what}"
    ))))
}

/// A HELLO body: token + cap offer.
fn hello_body(token: &[u8; AUTH_TOKEN_BYTES], config: &RemoteConfig) -> Vec<u8> {
    let mut body = Vec::with_capacity(AUTH_TOKEN_BYTES + 12);
    body.extend_from_slice(token);
    body.extend_from_slice(&config.max_envelope.to_le_bytes());
    body.extend_from_slice(&config.pool_total_cap.to_le_bytes());
    body
}

/// A HELLO_ACK body: the negotiated minima.
fn hello_ack_body(caps: &SessionCaps) -> Vec<u8> {
    let mut body = Vec::with_capacity(12);
    body.extend_from_slice(&caps.max_envelope.to_le_bytes());
    body.extend_from_slice(&caps.pool_total_cap.to_le_bytes());
    body
}

/// Parse a HELLO body.
///
/// # Errors
/// [`LdpError::Malformed`] for wrong-length bodies.
pub fn parse_hello(body: &[u8]) -> Result<([u8; AUTH_TOKEN_BYTES], u32, u64)> {
    if body.len() != AUTH_TOKEN_BYTES + 12 {
        return Err(LdpError::malformed(
            ErrorCode::MalformedMessage,
            format!(
                "remote HELLO body is {} bytes, expected {}",
                body.len(),
                AUTH_TOKEN_BYTES + 12
            ),
        ));
    }
    let mut token = [0u8; AUTH_TOKEN_BYTES];
    token.copy_from_slice(&body[..AUTH_TOKEN_BYTES]);
    let max_envelope = u32::from_le_bytes(
        body[AUTH_TOKEN_BYTES..AUTH_TOKEN_BYTES + 4]
            .try_into()
            .expect("4"),
    );
    let pool_total_cap = u64::from_le_bytes(
        body[AUTH_TOKEN_BYTES + 4..AUTH_TOKEN_BYTES + 12]
            .try_into()
            .expect("8"),
    );
    Ok((token, max_envelope, pool_total_cap))
}

/// Parse a HELLO_ACK body.
///
/// # Errors
/// [`LdpError::Malformed`] for wrong-length bodies.
pub fn parse_hello_ack(body: &[u8]) -> Result<SessionCaps> {
    if body.len() != 12 {
        return Err(LdpError::malformed(
            ErrorCode::MalformedMessage,
            format!("remote HELLO_ACK body is {} bytes, expected 12", body.len()),
        ));
    }
    let max_envelope = u32::from_le_bytes(body[0..4].try_into().expect("4"));
    let pool_total_cap = u64::from_le_bytes(body[4..12].try_into().expect("8"));
    Ok(SessionCaps {
        max_envelope,
        pool_total_cap,
    })
}

/// Constant-time equality for fixed-size tokens: the comparison walks
/// every byte regardless of where (or whether) a difference appears.
#[must_use]
pub fn constant_time_eq(a: &[u8; AUTH_TOKEN_BYTES], b: &[u8; AUTH_TOKEN_BYTES]) -> bool {
    let mut diff = 0u8;
    for i in 0..AUTH_TOKEN_BYTES {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

/// The client (edge) side of the handshake: send `HELLO`, demand
/// `HELLO_ACK`, and return the negotiated caps.
///
/// # Errors
/// [`LdpError::Io`] on transport failure; [`LdpError::Protocol`] when
/// the hub refuses (a `BYE`) or answers anything but `HELLO_ACK`.
pub fn handshake_client(
    stream: &mut TcpStream,
    token: &[u8; AUTH_TOKEN_BYTES],
    config: &RemoteConfig,
) -> Result<SessionCaps> {
    Envelope::new(EnvelopeKind::Hello, hello_body(token, config))
        .write_to(stream, config.envelope_cap())?;
    let ack = read_envelope(stream, config.envelope_cap())?;
    match ack.kind {
        EnvelopeKind::HelloAck => parse_hello_ack(&ack.body),
        EnvelopeKind::Bye => Err(LdpError::protocol(
            ErrorCode::Unauthorized,
            None,
            format!(
                "remote hub refused the session: reason {}",
                Envelope::parse_bye(&ack.body).unwrap_or(u32::MAX)
            ),
        )),
        other => Err(LdpError::malformed(
            ErrorCode::MalformedMessage,
            format!("remote hub answered the HELLO with {other:?} instead of HELLO_ACK"),
        )),
    }
}

/// The server (hub) side of the handshake: demand `HELLO` within
/// `deadline`, validate the token in constant time, answer with
/// `HELLO_ACK` carrying the negotiated minima.
///
/// On token mismatch the hub sends `BYE(AuthFailed)` and returns an
/// error; the caller drops the connection.
///
/// # Errors
/// [`LdpError::Io`] on transport failure or handshake timeout;
/// [`LdpError::Protocol`] (`Unauthorized`) on token mismatch;
/// [`LdpError::Malformed`] for non-`HELLO` first envelopes.
pub fn handshake_server(
    stream: &mut TcpStream,
    expected: &[u8; AUTH_TOKEN_BYTES],
    config: &RemoteConfig,
    deadline: Duration,
) -> Result<SessionCaps> {
    stream
        .set_read_timeout(Some(deadline))
        .map_err(|e| LdpError::Io(Arc::new(std::io::Error::other(e))))?;
    let hello = read_envelope(stream, config.envelope_cap());
    stream
        .set_read_timeout(None)
        .map_err(|e| LdpError::Io(Arc::new(std::io::Error::other(e))))?;
    let hello = hello?;
    if hello.kind != EnvelopeKind::Hello {
        // A peer that speaks something else first: refuse and explain.
        let _ = Envelope::bye(ByeReason::AuthFailed).write_to(stream, config.envelope_cap());
        return Err(LdpError::malformed(
            ErrorCode::MalformedMessage,
            format!("remote peer opened with {:?} instead of HELLO", hello.kind),
        ));
    }
    let (token, offered_env, offered_pool) = parse_hello(&hello.body)?;
    if !constant_time_eq(&token, expected) {
        let _ = Envelope::bye(ByeReason::AuthFailed).write_to(stream, config.envelope_cap());
        return Err(LdpError::protocol(
            ErrorCode::Unauthorized,
            None,
            "remote session refused: bearer token mismatch",
        ));
    }
    let (max_envelope, pool_total_cap) = config.negotiate(offered_env, offered_pool);
    let caps = SessionCaps {
        max_envelope,
        pool_total_cap,
    };
    Envelope::new(EnvelopeKind::HelloAck, hello_ack_body(&caps))
        .write_to(stream, config.envelope_cap())?;
    Ok(caps)
}

/// Resolve and dial a TCP address, with a connect deadline.
///
/// # Errors
/// [`LdpError::Io`] on resolution or connection failure.
pub fn dial<A: ToSocketAddrs>(addr: A, deadline: Duration) -> Result<TcpStream> {
    let mut last: Option<std::io::Error> = None;
    for candidate in addr
        .to_socket_addrs()
        .map_err(|e| LdpError::Io(Arc::new(std::io::Error::other(e))))?
    {
        match TcpStream::connect_timeout(&candidate, deadline) {
            Ok(stream) => return Ok(stream),
            Err(e) => last = Some(e),
        }
    }
    Err(LdpError::Io(Arc::new(std::io::Error::other(
        last.unwrap_or_else(|| std::io::Error::other("no addresses resolved")),
    ))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token(fill: u8) -> [u8; AUTH_TOKEN_BYTES] {
        [fill; AUTH_TOKEN_BYTES]
    }

    #[test]
    fn constant_time_eq_matches_and_rejects() {
        assert!(constant_time_eq(&token(3), &token(3)));
        let mut other = token(3);
        other[AUTH_TOKEN_BYTES - 1] ^= 1;
        assert!(!constant_time_eq(&token(3), &other));
    }

    #[test]
    fn hello_bodies_round_trip() {
        let config = RemoteConfig::default();
        let body = hello_body(&token(7), &config);
        let (tok, env, pool) = parse_hello(&body).expect("hello");
        assert_eq!(tok, token(7));
        assert_eq!(env, config.max_envelope);
        assert_eq!(pool, config.pool_total_cap);

        let caps = SessionCaps {
            max_envelope: 1024,
            pool_total_cap: 4096,
        };
        assert_eq!(parse_hello_ack(&hello_ack_body(&caps)).expect("ack"), caps);
        assert!(parse_hello(&body[..body.len() - 1]).is_err());
        assert!(parse_hello_ack(&[0u8; 11]).is_err());
    }

    #[test]
    fn handshake_round_trips_over_loopback() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let expected = token(0xAA);
        let config = RemoteConfig {
            max_envelope: 4096,
            pool_total_cap: 8192,
            ..RemoteConfig::default()
        };
        let offered = RemoteConfig {
            max_envelope: 2048,
            pool_total_cap: 16384,
            ..RemoteConfig::default()
        };

        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            handshake_server(&mut stream, &expected, &config, Duration::from_secs(5))
        });
        let mut client = dial(addr, Duration::from_secs(5)).expect("dial");
        let caps = handshake_client(&mut client, &token(0xAA), &offered).expect("handshake");
        let server_caps = server.join().expect("server thread").expect("handshake");

        // Negotiated minima, both sides agree.
        assert_eq!(caps, server_caps);
        assert_eq!(caps.max_envelope, 2048);
        assert_eq!(caps.pool_total_cap, 8192);
    }

    #[test]
    fn handshake_rejects_a_wrong_token() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let expected = token(1);
        let config = RemoteConfig::default();

        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            handshake_server(&mut stream, &expected, &config, Duration::from_secs(5))
        });
        let mut client = dial(addr, Duration::from_secs(5)).expect("dial");
        let refused = handshake_client(&mut client, &token(2), &config);
        assert!(refused.is_err());
        let server_side = server.join().expect("server thread");
        assert!(server_side.is_err());
    }

    #[test]
    fn handshake_rejects_a_non_hello_opener() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let expected = token(1);
        let config = RemoteConfig::default();

        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            handshake_server(&mut stream, &expected, &config, Duration::from_secs(5))
        });
        let mut client = dial(addr, Duration::from_secs(5)).expect("dial");
        // Speak first with the wrong envelope kind.
        Envelope::ping(1)
            .write_to(&mut client, config.envelope_cap())
            .expect("ping");
        assert!(server.join().expect("server thread").is_err());
    }

    #[test]
    fn socket_set_shuts_down_at_most_once_and_keeps_fds_open() {
        use crate::link::tests_support::unix_pair;
        let set = SocketSet::default();
        let (a, _b) = unix_pair();
        set.add(&a).expect("add");
        set.shutdown_all();
        set.shutdown_all(); // Idempotent.
                            // The set's duplicate still holds the description open even
                            // after the original drops.
        drop(a);
    }

    #[test]
    fn liveness_starts_touched() {
        let live = Liveness::new();
        assert!(live.idle_ms() < 5_000);
        assert!(!live.is_dead());
        live.kill();
        assert!(live.is_dead());
    }

    #[test]
    fn keepalive_policy_gates() {
        let off = Keepalive {
            idle_ping: Duration::ZERO,
            pong_deadline: Duration::from_secs(1),
        };
        assert!(!off.enabled());
        assert!(Keepalive::default().enabled());
    }
}

/// Test/interior shared helpers (not public API).
#[cfg(test)]
pub(crate) mod tests_support {
    use std::os::fd::OwnedFd;

    /// A connected socketpair as two owned fds.
    pub fn unix_pair() -> (OwnedFd, OwnedFd) {
        let (a, b) = std::os::unix::net::UnixStream::pair().expect("socketpair");
        (a.into(), b.into())
    }
}
