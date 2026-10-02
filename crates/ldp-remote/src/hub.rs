//! The hub gateway: authenticated TCP sessions in, compositor
//! connections out.
//!
//! A hub listens on TCP, demands a `HELLO` within its deadline, and
//! validates the bearer token in constant time (a failed or silent
//! peer costs one round trip and zero sessions). Each authenticated
//! session dials the compositor's real `AF_UNIX` socket as an ordinary
//! local client — the compositor's `SO_PEERCRED` verdict attributes
//! the whole session to the gateway's own account (the single-user
//! remote-display scope, `docs/maturity.md`) — and runs:
//!
//! * an **upstream pump** applying the edge's envelopes: reconstructing
//!   pool memfds (with a per-session byte ceiling), applying
//!   `POOL_UPDATE` windows and `POOL_RESIZE` growth, substituting
//!   streaming pipes, assembling each `DATA` message's ancillary FD
//!   table, and forwarding byte-identical frames,
//! * a **downstream pump** reading the compositor's frames and shipping
//!   descriptor state the wire cannot carry: eventfd fence counters,
//!   snapshot file contents (keymaps), and pipe substitutions — the
//!   mirror image of the edge's upstream work,
//! * a writer thread and an optional keepalive monitor, shared shape
//!   with the edge.
//!
//! # Session shape
//!
//! ```text
//! edge ══TCP══▶ [tcp dup] ──envelopes──▶ [up pump] ──AF_UNIX──▶ compositor
//! edge ◀═TCP══ [writer] ◀──envelopes── [down pump] ◀─AF_UNIX── compositor
//! ```

#![forbid(unsafe_code)]

use std::collections::{HashMap, VecDeque};
use std::net::TcpListener;
use std::os::fd::OwnedFd;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ldp_core::error::{ErrorCode, LdpError, Result};
use ldp_protocol::{decode, Direction, ValidationMode};
use ldp_transport::{
    FdList, Frame, FramedReader, FramedWriter, NoHooks, TransportStream, UnixAddr,
};

use crate::link::{
    enqueue, handshake_server, keepalive_monitor, Keepalive, Liveness, SessionCaps, SocketSet,
    AUTH_TOKEN_BYTES,
};
use crate::relay::{
    answer_ping, fence_counter, ldp_fd_count, send_framed, stream_pump, teardown, write_append,
    writer_loop, PumpHandles,
};
use crate::sys;
use crate::tracker::{FdDisposition, FdSemantics, Tracker};
use crate::wire::{read_envelope, ByeReason, Envelope, EnvelopeKind, RemoteConfig};

/// Hub-side configuration.
#[derive(Clone, Debug)]
pub struct HubConfig {
    /// The TCP address to listen on (`host:port`).
    pub listen: String,
    /// The compositor's `AF_UNIX` address.
    pub compositor: UnixAddr,
    /// The shared bearer token.
    pub token: [u8; AUTH_TOKEN_BYTES],
    /// Envelope caps and LDP framing limits.
    pub config: RemoteConfig,
    /// Keepalive policy (disabled when `idle_ping` is zero).
    pub keepalive: Keepalive,
    /// How long a fresh connection may take to say `HELLO`.
    pub handshake_deadline: Duration,
}

impl Default for HubConfig {
    fn default() -> Self {
        HubConfig {
            listen: String::from("127.0.0.1:6400"),
            compositor: UnixAddr::abstract_name(b"ldp-compositor").expect("abstract name"),
            token: [0u8; AUTH_TOKEN_BYTES],
            config: RemoteConfig::default(),
            keepalive: Keepalive::default(),
            handshake_deadline: Duration::from_secs(5),
        }
    }
}

/// The running hub gateway: a TCP accept loop on a background thread.
pub struct HubGateway {
    local: std::net::SocketAddr,
    active: Arc<Mutex<usize>>,
}

impl HubGateway {
    /// Bind and start serving remote sessions.
    ///
    /// # Errors
    /// [`LdpError::Io`] when the listener cannot bind.
    pub fn start(config: HubConfig) -> Result<HubGateway> {
        let listener = TcpListener::bind(&config.listen)
            .map_err(|e| LdpError::Io(Arc::new(std::io::Error::other(e))))?;
        let local = listener
            .local_addr()
            .map_err(|e| LdpError::Io(Arc::new(std::io::Error::other(e))))?;
        let active = Arc::new(Mutex::new(0usize));
        let active_sessions = Arc::clone(&active);
        std::thread::Builder::new()
            .name("ldp-remote-hub-accept".into())
            .spawn(move || accept_loop(listener, config, active_sessions))
            .map_err(|e| LdpError::Io(Arc::new(std::io::Error::other(e))))?;
        Ok(HubGateway { local, active })
    }

    /// The bound TCP address (port 0 in the config picks an ephemeral
    /// one; tests read it here).
    #[must_use]
    pub fn local_addr(&self) -> std::net::SocketAddr {
        self.local
    }

    /// Sessions currently running.
    #[must_use]
    pub fn active_sessions(&self) -> usize {
        *self.active.lock().expect("active lock")
    }

    /// Wait until no session is running, up to `timeout`.
    #[must_use]
    pub fn wait_quiet(&self, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            if self.active_sessions() == 0 {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        self.active_sessions() == 0
    }
}

/// One reconstructed pool: the memfd plus its accounted size (the
/// per-session ceiling covers growth, not just creation).
#[derive(Debug)]
struct PoolSlot {
    fd: OwnedFd,
    accounted: u64,
}

/// Per-session descriptor state shared between the pumps.
#[derive(Default, Debug)]
struct Stores {
    /// Pools reconstructed from `FD_SEGMENT`s: relay id → memfd.
    pools: HashMap<u32, PoolSlot>,
    /// Streaming sinks the compositor passed (clipboard-class send):
    /// relay id → the original fd far-side chunks get written into.
    sinks: HashMap<u32, OwnedFd>,
    /// Total pooled bytes (the `pool_total_cap` ledger).
    pooled_bytes: u64,
}

fn accept_loop(listener: TcpListener, config: HubConfig, active: Arc<Mutex<usize>>) {
    loop {
        let Ok((mut stream, _peer)) = listener.accept() else {
            return;
        };
        // Authenticate before anything else: a failed or silent peer
        // costs one round trip and no session.
        let Ok(caps) = handshake_server(
            &mut stream,
            &config.token,
            &config.config,
            config.handshake_deadline,
        ) else {
            continue;
        };
        let config = config.clone();
        let active_count = Arc::clone(&active);
        // Count the session from the moment we authenticate it.
        if let Ok(mut guard) = active_count.lock() {
            *guard += 1;
        }
        let spawned = std::thread::Builder::new()
            .name("ldp-remote-hub-session".into())
            .spawn(move || {
                run_session(stream, &config, caps);
                if let Ok(mut guard) = active_count.lock() {
                    *guard -= 1;
                }
            });
        if spawned.is_err() {
            if let Ok(mut guard) = active.lock() {
                *guard -= 1;
            }
        }
    }
}

/// One remote ↔ compositor session.
fn run_session(mut tcp: std::net::TcpStream, config: &HubConfig, caps: SessionCaps) {
    // Dial the compositor like any local client.
    let Ok(compositor) = TransportStream::connect(&config.compositor) else {
        let _ = Envelope::bye(ByeReason::ProtocolError).write_to(&mut tcp, caps.envelope_cap());
        return;
    };

    // Split the compositor stream into read/write halves.
    let comp_fd = compositor.into_fd();
    let Ok(comp_write_fd) = comp_fd.try_clone() else {
        return;
    };
    let Ok(tcp_read) = tcp.try_clone() else {
        return;
    };

    // The teardown set: every descriptor, duplicated and held open.
    let sockets = Arc::new(SocketSet::default());
    let _ = sockets.add(&tcp);
    let _ = sockets.add(&tcp_read);
    let _ = sockets.add(&comp_fd);
    let _ = sockets.add(&comp_write_fd);

    let liveness = Arc::new(Liveness::new());
    let stores = Arc::new(Mutex::new(Stores::default()));
    let pumps = Arc::new(PumpHandles::default());
    let tracker = Arc::new(Mutex::new(Tracker::new()));
    let (tx, rx) = channel::<Envelope>();

    let comp_read = match TransportStream::new(comp_fd) {
        Ok(s) => s,
        Err(_) => return,
    };
    let mut comp_write = match TransportStream::new(comp_write_fd) {
        Ok(s) => s,
        Err(_) => return,
    };

    // Upstream pump: remote envelopes → compositor frames.
    let up = {
        let sockets = Arc::clone(&sockets);
        let liveness = Arc::clone(&liveness);
        let stores = Arc::clone(&stores);
        let pumps = Arc::clone(&pumps);
        let tracker = Arc::clone(&tracker);
        let tx = tx.clone();
        let limits = config.config.limits;
        let cap = caps.envelope_cap();
        let pool_cap = caps.pool_total_cap;
        std::thread::Builder::new()
            .name("ldp-remote-hub-up".into())
            .spawn(move || {
                let mut writer = FramedWriter::new(limits, NoHooks);
                let outcome = up_pump(
                    tcp_read,
                    &mut comp_write,
                    &mut writer,
                    &stores,
                    &pumps,
                    &tracker,
                    &tx,
                    cap,
                    pool_cap,
                    &sockets,
                    &liveness,
                    limits,
                );
                if outcome.is_err() {
                    teardown(&liveness, &sockets);
                }
            })
    };
    let up = match up {
        Ok(handle) => handle,
        Err(_) => return,
    };

    // Downstream pump: compositor frames → remote envelopes.
    let down = {
        let sockets = Arc::clone(&sockets);
        let liveness = Arc::clone(&liveness);
        let stores = Arc::clone(&stores);
        let tracker = Arc::clone(&tracker);
        let tx = tx.clone();
        let limits = config.config.limits;
        std::thread::Builder::new()
            .name("ldp-remote-hub-down".into())
            .spawn(move || {
                let mut reader = FramedReader::new(limits);
                let outcome = down_pump(
                    comp_read,
                    &mut reader,
                    &stores,
                    &tracker,
                    &tx,
                    &sockets,
                    &liveness,
                    limits,
                );
                if outcome.is_err() {
                    teardown(&liveness, &sockets);
                }
            })
    };
    let down = match down {
        Ok(handle) => handle,
        Err(_) => return,
    };

    // Keepalive monitor (optional).
    let monitor = if config.keepalive.enabled() {
        let policy = config.keepalive;
        let sockets = Arc::clone(&sockets);
        let liveness = Arc::clone(&liveness);
        let tx = tx.clone();
        std::thread::Builder::new()
            .name("ldp-remote-hub-keepalive".into())
            .spawn(move || keepalive_monitor(policy, liveness, sockets, tx))
            .ok()
    } else {
        None
    };

    // Writer: the single owner of the TCP stream.
    let writer = {
        let sockets = Arc::clone(&sockets);
        let liveness = Arc::clone(&liveness);
        let cap = caps.envelope_cap();
        std::thread::Builder::new()
            .name("ldp-remote-hub-writer".into())
            .spawn(move || writer_loop(tcp, rx, cap, &sockets, &liveness))
    };
    let writer = match writer {
        Ok(handle) => handle,
        Err(_) => return,
    };

    // Supervise.
    let _ = up.join();
    let _ = down.join();
    if let Some(monitor) = monitor {
        let _ = monitor.join();
    }
    teardown(&liveness, &sockets);
    drop(tx);
    let _ = writer.join();
    pumps.join_all();
}

/// The upstream pump: remote envelopes → compositor frames.
#[allow(clippy::too_many_arguments)]
fn up_pump(
    mut tcp_read: std::net::TcpStream,
    comp_write: &mut TransportStream,
    writer: &mut FramedWriter<NoHooks>,
    stores: &Arc<Mutex<Stores>>,
    pumps: &Arc<PumpHandles>,
    tracker: &Arc<Mutex<Tracker>>,
    tx: &Sender<Envelope>,
    cap: usize,
    pool_cap: u64,
    sockets: &Arc<SocketSet>,
    liveness: &Arc<Liveness>,
    limits: ldp_core::limits::Limits,
) -> Result<()> {
    let mut pending: VecDeque<OwnedFd> = VecDeque::new();
    loop {
        let env = read_envelope(&mut tcp_read, cap)?;
        liveness.touch();
        match env.kind {
            EnvelopeKind::Data => {
                // Learn the request stream's objects (the downstream
                // pump's event interpretation depends on them), and
                // mirror the server's own validation verdicts.
                let declared = usize::from(ldp_fd_count(&env.body)?);
                let msg = match decode(&env.body, declared as u32, &limits, ValidationMode::Strict)
                {
                    Ok(msg) => msg,
                    Err(_) => {
                        let _ = enqueue(tx, Envelope::bye(ByeReason::ProtocolError));
                        teardown(liveness, sockets);
                        return Err(LdpError::malformed(
                            ErrorCode::MalformedMessage,
                            "ldp-remote hub: relayed message failed stage-2 decode",
                        ));
                    }
                };
                if let Err(e) = tracker
                    .lock()
                    .expect("tracker lock")
                    .observe(&msg, Direction::Request)
                {
                    let _ = enqueue(tx, Envelope::bye(ByeReason::ProtocolError));
                    teardown(liveness, sockets);
                    return Err(e);
                }
                if declared != pending.len() {
                    let _ = enqueue(tx, Envelope::bye(ByeReason::ProtocolError));
                    teardown(liveness, sockets);
                    return Err(LdpError::malformed(
                        ErrorCode::FdMismatch,
                        format!(
                            "ldp-remote hub: DATA declares {declared} fds, {} queued",
                            pending.len()
                        ),
                    ));
                }
                let mut fds = FdList::new();
                for fd in pending.drain(..) {
                    fds.push(fd);
                }
                send_framed(writer, comp_write, &env.body, &mut fds)?;
            }
            EnvelopeKind::FdSegment => {
                let (id, content) = Envelope::parse_fd_segment(&env.body)?;
                let bytes = content.len() as u64;
                let mut guard = stores.lock().expect("stores lock");
                guard.pooled_bytes = guard.pooled_bytes.saturating_add(bytes);
                if guard.pooled_bytes > pool_cap {
                    drop(guard);
                    let _ = enqueue(tx, Envelope::bye(ByeReason::CapExceeded));
                    teardown(liveness, sockets);
                    return Err(LdpError::Limit {
                        kind: ldp_core::error::LimitKind::ClientBufferBytes,
                        value: guard_peek(stores),
                    });
                }
                let fd = sys::filled_memfd("ldp-remote-pool", content)?;
                guard.pools.insert(
                    id,
                    PoolSlot {
                        fd: fd
                            .try_clone()
                            .map_err(|e| LdpError::Io(Arc::new(std::io::Error::other(e))))?,
                        accounted: bytes,
                    },
                );
                drop(guard);
                pending.push_back(fd);
            }
            EnvelopeKind::PoolUpdate => {
                let (id, offset, bytes) = Envelope::parse_pool_update(&env.body)?;
                let fd = {
                    let guard = stores.lock().expect("stores lock");
                    let Some(slot) = guard.pools.get(&id) else {
                        drop(guard);
                        teardown(liveness, sockets);
                        return Err(unknown_pool(tx));
                    };
                    slot.fd.try_clone().map_err(clone_err)?
                };
                sys::write_range(&fd, offset, bytes)?;
            }
            EnvelopeKind::PoolResize => {
                let (id, new_size) = Envelope::parse_pool_resize(&env.body)?;
                // Read the slot's state out (accounted size + an fd
                // duplicate), then update the ledger without holding a
                // borrow across it.
                let (accounted, fd) = {
                    let guard = stores.lock().expect("stores lock");
                    let Some(slot) = guard.pools.get(&id) else {
                        drop(guard);
                        teardown(liveness, sockets);
                        return Err(unknown_pool(tx));
                    };
                    (slot.accounted, slot.fd.try_clone().map_err(clone_err)?)
                };
                // Pools never shrink: claims below the accounted size
                // are unreachable through a healthy edge and ignored
                // from a hostile one.
                if new_size > accounted {
                    let delta = new_size - accounted;
                    let over = {
                        let mut guard = stores.lock().expect("stores lock");
                        guard.pooled_bytes = guard.pooled_bytes.saturating_add(delta);
                        guard.pooled_bytes > pool_cap
                    };
                    if over {
                        let _ = enqueue(tx, Envelope::bye(ByeReason::CapExceeded));
                        teardown(liveness, sockets);
                        return Err(LdpError::Limit {
                            kind: ldp_core::error::LimitKind::ClientBufferBytes,
                            value: new_size,
                        });
                    }
                    sys::grow_memfd(&fd, new_size)?;
                    if let Some(slot) = stores.lock().expect("stores lock").pools.get_mut(&id) {
                        slot.accounted = new_size;
                    }
                }
            }
            EnvelopeKind::StreamOpen => {
                let id = Envelope::parse_stream_id(&env.body)?;
                let (read_end, write_end) = sys::pipe()?;
                pending.push_back(write_end);
                let tx = tx.clone();
                if let Ok(handle) = std::thread::Builder::new()
                    .name("ldp-remote-hub-stream".into())
                    .spawn(move || stream_pump(read_end, id, tx))
                {
                    pumps.add(handle);
                }
            }
            EnvelopeKind::StreamChunk => {
                let (id, bytes) = Envelope::parse_stream_chunk(&env.body)?;
                let sink = {
                    let guard = stores.lock().expect("stores lock");
                    guard
                        .sinks
                        .get(&id)
                        .map(|fd| fd.try_clone())
                        .transpose()
                        .map_err(clone_err)?
                };
                let Some(sink) = sink else {
                    teardown(liveness, sockets);
                    return Err(LdpError::Logic {
                        what: "ldp-remote hub: chunk for an unknown stream id",
                    });
                };
                write_append(&sink, bytes)?;
            }
            EnvelopeKind::StreamFin => {
                let id = Envelope::parse_stream_id(&env.body)?;
                stores.lock().expect("stores lock").sinks.remove(&id);
            }
            EnvelopeKind::Fence => {
                // Fences flow downstream only; an edge never sends one.
                teardown(liveness, sockets);
                return Err(LdpError::malformed(
                    ErrorCode::MalformedMessage,
                    "ldp-remote hub: fence envelope in the upstream direction",
                ));
            }
            EnvelopeKind::Ping => answer_ping(&env, tx)?,
            EnvelopeKind::Pong => {
                liveness.touch();
            }
            EnvelopeKind::Bye => {
                teardown(liveness, sockets);
                return Ok(());
            }
            EnvelopeKind::Hello | EnvelopeKind::HelloAck => {
                teardown(liveness, sockets);
                return Err(LdpError::malformed(
                    ErrorCode::MalformedMessage,
                    "ldp-remote hub: handshake envelope outside the handshake",
                ));
            }
        }
    }
}

fn unknown_pool(tx: &Sender<Envelope>) -> LdpError {
    let _ = enqueue(tx, Envelope::bye(ByeReason::ProtocolError));
    LdpError::Logic {
        what: "ldp-remote hub: pool update for an unknown relay id",
    }
}

/// The pool-byte ledger, read under the stores lock.
fn guard_peek(stores: &Arc<Mutex<Stores>>) -> u64 {
    stores.lock().expect("stores lock").pooled_bytes
}

#[allow(clippy::needless_pass_by_value)]
fn clone_err(e: std::io::Error) -> LdpError {
    LdpError::Io(Arc::new(std::io::Error::other(e)))
}

/// The downstream pump: compositor frames → remote envelopes.
#[allow(clippy::too_many_arguments)]
fn down_pump(
    mut comp_read: TransportStream,
    reader: &mut FramedReader,
    stores: &Arc<Mutex<Stores>>,
    tracker: &Arc<Mutex<Tracker>>,
    tx: &Sender<Envelope>,
    sockets: &Arc<SocketSet>,
    liveness: &Arc<Liveness>,
    limits: ldp_core::limits::Limits,
) -> Result<()> {
    loop {
        let frame = reader.recv_msg(&mut comp_read)?;
        liveness.touch();
        handle_frame(frame, stores, tracker, tx, sockets, liveness, limits)?;
    }
}
/// One compositor message: ship its descriptor state, forward.
#[allow(clippy::too_many_arguments)]
fn handle_frame(
    mut frame: Frame,
    stores: &Arc<Mutex<Stores>>,
    tracker: &Arc<Mutex<Tracker>>,
    tx: &Sender<Envelope>,
    sockets: &Arc<SocketSet>,
    liveness: &Arc<Liveness>,
    limits: ldp_core::limits::Limits,
) -> Result<()> {
    if !frame.fds.is_empty() {
        let msg = match decode(
            frame.message_bytes(),
            u32::from(frame.fd_count()),
            &limits,
            ValidationMode::Strict,
        ) {
            Ok(msg) => msg,
            Err(_) => {
                teardown(liveness, sockets);
                return Err(LdpError::malformed(
                    ErrorCode::MalformedMessage,
                    "ldp-remote hub: compositor message failed stage-2 decode",
                ));
            }
        };
        let plan = tracker
            .lock()
            .expect("tracker lock")
            .observe(&msg, Direction::Event)?;
        let carried = frame.fds.take_all();
        match plan.disposition {
            FdDisposition::None => {
                teardown(liveness, sockets);
                return Err(LdpError::malformed(
                    ErrorCode::FdMismatch,
                    "ldp-remote hub: compositor message carries uninterpreted fds",
                ));
            }
            FdDisposition::Unsupported(why) => {
                let _ = enqueue(tx, Envelope::bye(ByeReason::UnsupportedFd));
                teardown(liveness, sockets);
                return Err(LdpError::protocol(
                    ErrorCode::UnsupportedVersion,
                    None,
                    format!("ldp-remote hub: {why}"),
                ));
            }
            FdDisposition::Interpret(semantics) => {
                let indices: Vec<u32> = msg
                    .args
                    .iter()
                    .filter_map(|a| match a {
                        ldp_core::wire::Value::Fd(i) => Some(*i),
                        _ => None,
                    })
                    .collect();
                if indices.len() != semantics.len() || indices.len() != carried.len() {
                    teardown(liveness, sockets);
                    return Err(LdpError::malformed(
                        ErrorCode::FdMismatch,
                        "ldp-remote hub: fd indices and semantics disagree",
                    ));
                }
                for (slot, sem) in indices.iter().zip(&semantics) {
                    let fd = &carried[*slot as usize];
                    match sem {
                        FdSemantics::Fence => {
                            let counter = fence_counter(fd)?;
                            enqueue(tx, Envelope::fence(counter))?;
                        }
                        FdSemantics::Snapshot { relay_id } => {
                            let bytes = sys::read_whole(fd)?;
                            enqueue(tx, Envelope::fd_segment(*relay_id, &bytes))?;
                        }
                        FdSemantics::Stream { relay_id } => {
                            stores
                                .lock()
                                .expect("stores lock")
                                .sinks
                                .insert(*relay_id, fd.try_clone().map_err(clone_err)?);
                            enqueue(tx, Envelope::stream_open(*relay_id))?;
                        }
                        FdSemantics::Pool { .. } => {
                            teardown(liveness, sockets);
                            return Err(LdpError::Logic {
                                what: "ldp-remote hub: pool semantics on an event",
                            });
                        }
                    }
                }
            }
        }
    }
    enqueue(tx, Envelope::data(frame.message_bytes()))
}
