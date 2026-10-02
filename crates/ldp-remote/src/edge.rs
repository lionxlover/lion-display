//! The edge gateway: local clients in, one authenticated TCP session
//! each, out.
//!
//! An edge listens on an ordinary LDP `AF_UNIX` address — filesystem
//! or abstract — so **unmodified clients** connect to it exactly as
//! they would connect to a compositor (the `ldp-client` connection
//! flow, the `ldp-tools` session driver, every example: all work
//! unchanged). Each accepted client gets:
//!
//! * a dialed TCP session to the hub, authenticated with the shared
//!   bearer token and negotiated to common caps (a refused hub
//!   fast-fails, exactly like dialing a dead compositor socket),
//! * an **upstream pump** that reads framed messages, decodes them
//!   through the same schema registry the server uses, and ships the
//!   descriptor state the wire cannot carry: whole pool contents at
//!   `create_pool`, the attached buffer's window before every
//!   `surface.commit`, pool growth, snapshot file contents, and pipe
//!   substitutions,
//! * a **downstream pump** that applies the hub's envelopes:
//!   reconstructing memfds and eventfd fences, assembling each `DATA`
//!   message's ancillary FD table from the pending queue, substituting
//!   streaming pipes, and answering `PING`s,
//! * a **writer thread** that serializes every outbound envelope onto
//!   the TCP stream (a single owner: envelopes never interleave), and
//!   an optional keepalive monitor that tears dead sessions down on
//!   deadline.
//!
//! # Session shape
//!
//! ```text
//! client ──AF_UNIX──▶ [up pump] ──envelopes──▶ [writer] ══TCP══▶ hub
//! client ◀─AF_UNIX── [down pump] ◀─envelopes── [tcp dup] ══TCP══◀ hub
//! ```

#![forbid(unsafe_code)]

use std::collections::{HashMap, VecDeque};
use std::os::fd::OwnedFd;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ldp_core::error::{ErrorCode, LdpError, Result};
use ldp_core::wire::Value;
use ldp_protocol::{decode, Direction, ValidationMode};
use ldp_transport::{
    FdList, Frame, FramedReader, FramedWriter, NoHooks, TransportListener, TransportStream,
    UnixAddr,
};

use crate::link::{
    dial, enqueue, handshake_client, keepalive_monitor, Keepalive, Liveness, SocketSet,
    AUTH_TOKEN_BYTES,
};
use crate::relay::{
    answer_ping, ldp_fd_count, send_framed, stream_pump, teardown, write_append, writer_loop,
    PumpHandles,
};
use crate::sys;
use crate::tracker::{FdDisposition, FdSemantics, ShipBefore, Tracker};
use crate::wire::{read_envelope, ByeReason, Envelope, EnvelopeKind, RemoteConfig};

/// Edge-side configuration.
#[derive(Clone, Debug)]
pub struct EdgeConfig {
    /// The local `AF_UNIX` address clients dial (filesystem or
    /// abstract).
    pub listen: UnixAddr,
    /// The hub's TCP address (`host:port`).
    pub remote: String,
    /// The shared bearer token.
    pub token: [u8; AUTH_TOKEN_BYTES],
    /// Envelope caps and LDP framing limits.
    pub config: RemoteConfig,
    /// Keepalive policy (disabled when `idle_ping` is zero).
    pub keepalive: Keepalive,
    /// TCP connect deadline.
    pub connect_deadline: Duration,
}

impl Default for EdgeConfig {
    fn default() -> Self {
        EdgeConfig {
            listen: UnixAddr::abstract_name(b"ldp-remote-edge").expect("abstract name"),
            remote: String::from("127.0.0.1:6400"),
            token: [0u8; AUTH_TOKEN_BYTES],
            config: RemoteConfig::default(),
            keepalive: Keepalive::default(),
            connect_deadline: Duration::from_secs(5),
        }
    }
}

/// The running edge gateway: an accept loop on a background thread.
pub struct EdgeGateway {
    addr: UnixAddr,
    active: Arc<Mutex<usize>>,
}

impl EdgeGateway {
    /// Bind and start serving local clients.
    ///
    /// # Errors
    /// [`LdpError::Io`] when the listener cannot bind.
    pub fn start(config: EdgeConfig) -> Result<EdgeGateway> {
        let listener = TransportListener::bind(&config.listen, ldp_transport::DEFAULT_BACKLOG)?;
        let addr = config.listen.clone();
        let active = Arc::new(Mutex::new(0usize));
        let active_sessions = Arc::clone(&active);
        std::thread::Builder::new()
            .name("ldp-remote-edge-accept".into())
            .spawn(move || accept_loop(listener, config, active_sessions))
            .map_err(|e| LdpError::Io(Arc::new(std::io::Error::other(e))))?;
        Ok(EdgeGateway { addr, active })
    }

    /// The address clients dial.
    #[must_use]
    pub fn addr(&self) -> &UnixAddr {
        &self.addr
    }

    /// Sessions currently running (accepted, not yet torn down).
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

/// Per-session descriptor state shared between the two pumps.
#[derive(Default)]
struct Stores {
    /// Pools the client passed: relay id → the local fd (kept for
    /// per-commit range reads; the same file description the client
    /// keeps painting into between commits).
    pools: HashMap<u32, OwnedFd>,
    /// Streaming sinks the client passed (clipboard-class receive):
    /// relay id → the original fd far-side chunks get written into.
    sinks: HashMap<u32, OwnedFd>,
}

fn accept_loop(listener: TransportListener, config: EdgeConfig, active: Arc<Mutex<usize>>) {
    loop {
        let Ok((client, _creds)) = listener.accept() else {
            // The listener is gone only when the process is.
            return;
        };
        let config = config.clone();
        let active_count = Arc::clone(&active);
        // Count the session from the moment we accept it (before the
        // spawn): `active_sessions` is observable the instant
        // `accept` returns.
        if let Ok(mut guard) = active_count.lock() {
            *guard += 1;
        }
        let spawned = std::thread::Builder::new()
            .name("ldp-remote-edge-session".into())
            .spawn(move || {
                run_session(client, &config);
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

/// One client ↔ hub session: dial, authenticate, pump, tear down.
fn run_session(client: TransportStream, config: &EdgeConfig) {
    // Fast-fail paths (hub unreachable, token refused): the local
    // client sees a dropped connection, exactly like dialing a dead
    // compositor socket.
    let Ok(mut tcp) = dial(&config.remote, config.connect_deadline) else {
        return;
    };
    let Ok(caps) = handshake_client(&mut tcp, &config.token, &config.config) else {
        return;
    };

    // Split the client stream into read/write halves (two dups of one
    // description) so the pumps own disjoint directions.
    let client_fd = client.into_fd();
    let Ok(client_write_fd) = client_fd.try_clone() else {
        return;
    };
    let Ok(tcp_read) = tcp.try_clone() else {
        return;
    };

    // The teardown set: every descriptor of the session, duplicated
    // and held open for the session's lifetime (races designed out).
    let sockets = Arc::new(SocketSet::default());
    let _ = sockets.add(&tcp);
    let _ = sockets.add(&tcp_read);
    let _ = sockets.add(&client_fd);
    let _ = sockets.add(&client_write_fd);

    let liveness = Arc::new(Liveness::new());
    let stores = Arc::new(Mutex::new(Stores::default()));
    let pumps = Arc::new(PumpHandles::default());
    let (tx, rx) = channel::<Envelope>();

    let client_read = match TransportStream::new(client_fd) {
        Ok(s) => s,
        Err(_) => return,
    };
    let mut client_write = match TransportStream::new(client_write_fd) {
        Ok(s) => s,
        Err(_) => return,
    };

    // Upstream pump: client → hub.
    let up = {
        let sockets = Arc::clone(&sockets);
        let liveness = Arc::clone(&liveness);
        let stores = Arc::clone(&stores);
        let tx = tx.clone();
        let limits = config.config.limits;
        std::thread::Builder::new()
            .name("ldp-remote-edge-up".into())
            .spawn(move || {
                let mut reader = FramedReader::new(limits);
                let mut tracker = Tracker::new();
                let outcome = up_pump(
                    client_read,
                    &mut reader,
                    &mut tracker,
                    &stores,
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
    let up = match up {
        Ok(handle) => handle,
        Err(_) => return,
    };

    // Downstream pump: hub → client.
    let down = {
        let sockets = Arc::clone(&sockets);
        let liveness = Arc::clone(&liveness);
        let stores = Arc::clone(&stores);
        let pumps = Arc::clone(&pumps);
        let tx = tx.clone();
        let limits = config.config.limits;
        let cap = caps.envelope_cap();
        std::thread::Builder::new()
            .name("ldp-remote-edge-down".into())
            .spawn(move || {
                let mut writer = FramedWriter::new(limits, NoHooks);
                let outcome = down_pump(
                    tcp_read,
                    &mut client_write,
                    &mut writer,
                    &stores,
                    &pumps,
                    &tx,
                    cap,
                    &sockets,
                    &liveness,
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
            .name("ldp-remote-edge-keepalive".into())
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
            .name("ldp-remote-edge-writer".into())
            .spawn(move || writer_loop(tcp, rx, cap, &sockets, &liveness))
    };
    let writer = match writer {
        Ok(handle) => handle,
        Err(_) => return,
    };

    // Supervise: when both pumps are done the session is over.
    let _ = up.join();
    let _ = down.join();
    if let Some(monitor) = monitor {
        let _ = monitor.join();
    }
    teardown(&liveness, &sockets);
    drop(tx);
    let _ = writer.join();
    pumps.join_all();
    // Stores drop here: every pool and sink descriptor closes.
}

/// The upstream pump: framed client messages → remote envelopes.
#[allow(clippy::too_many_arguments)]
fn up_pump(
    mut client_read: TransportStream,
    reader: &mut FramedReader,
    tracker: &mut Tracker,
    stores: &Arc<Mutex<Stores>>,
    tx: &Sender<Envelope>,
    sockets: &Arc<SocketSet>,
    liveness: &Arc<Liveness>,
    limits: ldp_core::limits::Limits,
) -> Result<()> {
    loop {
        let frame = reader.recv_msg(&mut client_read)?;
        liveness.touch();
        handle_frame(frame, tracker, stores, tx, sockets, liveness, limits)?;
    }
}

/// One client message: interpret, ship descriptor state, forward.
fn handle_frame(
    mut frame: Frame,
    tracker: &mut Tracker,
    stores: &Arc<Mutex<Stores>>,
    tx: &Sender<Envelope>,
    sockets: &Arc<SocketSet>,
    liveness: &Arc<Liveness>,
    limits: ldp_core::limits::Limits,
) -> Result<()> {
    // Stage 1–2 validation, exactly what the server would run.
    let msg = match decode(
        frame.message_bytes(),
        u32::from(frame.fd_count()),
        &limits,
        ValidationMode::Strict,
    ) {
        Ok(msg) => msg,
        Err(_) => {
            // The server would reject this message and kill the
            // connection; the relay mirrors that verdict rather than
            // forwarding bytes it cannot vouch for.
            let _ = enqueue(tx, Envelope::bye(ByeReason::ProtocolError));
            teardown(liveness, sockets);
            return Err(LdpError::malformed(
                ErrorCode::MalformedMessage,
                "ldp-remote edge: client message failed stage-2 decode",
            ));
        }
    };
    let plan = match tracker.observe(&msg, Direction::Request) {
        Ok(plan) => plan,
        Err(e) => {
            let _ = enqueue(tx, Envelope::bye(ByeReason::ProtocolError));
            teardown(liveness, sockets);
            return Err(e);
        }
    };

    // Pre-ship pool state (the write-then-commit contract, preserved).
    match plan.ship_before {
        ShipBefore::Nothing => {}
        ShipBefore::PoolUpdate {
            relay_id,
            offset,
            len,
        } => {
            let fd = {
                let guard = stores.lock().expect("stores lock");
                guard
                    .pools
                    .get(&relay_id)
                    .map(|fd| fd.try_clone())
                    .transpose()
                    .map_err(|e| LdpError::Io(Arc::new(std::io::Error::other(e))))?
            };
            let Some(fd) = fd else {
                let _ = enqueue(tx, Envelope::bye(ByeReason::ProtocolError));
                teardown(liveness, sockets);
                return Err(LdpError::Logic {
                    what: "ldp-remote edge: commit references an unshipped pool",
                });
            };
            let bytes = sys::read_range(&fd, offset, len)?;
            enqueue(tx, Envelope::pool_update(relay_id, offset, &bytes))?;
        }
        ShipBefore::PoolResize { relay_id, new_size } => {
            enqueue(tx, Envelope::pool_resize(relay_id, new_size))?;
        }
    }

    // Descriptor work: take the frame's FDs apart (swap_remove keeps
    // every remaining slot reachable — each referenced index is
    // visited exactly once).
    let mut carried = frame.fds.take_all();
    match plan.disposition {
        FdDisposition::None => {
            // Unreferenced FDs (a lying fd_count): the server's own
            // reader cross-check would reject them; drop them here and
            // end the session the same way.
            if !carried.is_empty() {
                let _ = enqueue(tx, Envelope::bye(ByeReason::ProtocolError));
                teardown(liveness, sockets);
                return Err(LdpError::malformed(
                    ErrorCode::FdMismatch,
                    "ldp-remote edge: fds on a message the tracker did not interpret",
                ));
            }
        }
        FdDisposition::Unsupported(why) => {
            let _ = enqueue(tx, Envelope::bye(ByeReason::UnsupportedFd));
            teardown(liveness, sockets);
            return Err(LdpError::protocol(
                ErrorCode::UnsupportedVersion,
                None,
                format!("ldp-remote edge: {why}"),
            ));
        }
        FdDisposition::Interpret(semantics) => {
            let indices = fd_indices(&msg);
            if indices.len() != semantics.len() || indices.len() != carried.len() {
                let _ = enqueue(tx, Envelope::bye(ByeReason::ProtocolError));
                teardown(liveness, sockets);
                return Err(LdpError::malformed(
                    ErrorCode::FdMismatch,
                    "ldp-remote edge: fd indices and semantics disagree",
                ));
            }
            for (slot, sem) in indices.iter().zip(&semantics) {
                let fd = carried.swap_remove(*slot as usize);
                match sem {
                    FdSemantics::Pool { relay_id, size } => {
                        let bytes = sys::read_range(&fd, 0, *size)?;
                        stores
                            .lock()
                            .expect("stores lock")
                            .pools
                            .insert(*relay_id, fd);
                        enqueue(tx, Envelope::fd_segment(*relay_id, &bytes))?;
                    }
                    FdSemantics::Snapshot { relay_id } => {
                        let bytes = sys::read_whole(&fd)?;
                        // The fd's content is now on the wire; the
                        // descriptor itself has no further role.
                        enqueue(tx, Envelope::fd_segment(*relay_id, &bytes))?;
                    }
                    FdSemantics::Stream { relay_id } => {
                        // Keep the original as this side's sink; the
                        // hub substitutes a local pipe into the
                        // message it forwards.
                        stores
                            .lock()
                            .expect("stores lock")
                            .sinks
                            .insert(*relay_id, fd);
                        enqueue(tx, Envelope::stream_open(*relay_id))?;
                    }
                    FdSemantics::Fence => {
                        let _ = enqueue(tx, Envelope::bye(ByeReason::UnsupportedFd));
                        teardown(liveness, sockets);
                        return Err(LdpError::protocol(
                            ErrorCode::UnsupportedVersion,
                            None,
                            "ldp-remote edge: client-side eventfd fences are not relayable",
                        ));
                    }
                }
            }
        }
    }

    // Finally: the message itself, byte-identical.
    enqueue(tx, Envelope::data(frame.message_bytes()))
}

/// The fd indices a decoded message references, ascending, deduped.
fn fd_indices(msg: &ldp_protocol::Message) -> Vec<u32> {
    let mut out: Vec<u32> = msg
        .args
        .iter()
        .filter_map(|a| match a {
            Value::Fd(i) => Some(*i),
            _ => None,
        })
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// The downstream pump: remote envelopes → framed client messages.
#[allow(clippy::too_many_arguments)]
fn down_pump(
    mut tcp_read: std::net::TcpStream,
    client_write: &mut TransportStream,
    writer: &mut FramedWriter<NoHooks>,
    stores: &Arc<Mutex<Stores>>,
    pumps: &Arc<PumpHandles>,
    tx: &Sender<Envelope>,
    cap: usize,
    sockets: &Arc<SocketSet>,
    liveness: &Arc<Liveness>,
) -> Result<()> {
    let mut pending: VecDeque<OwnedFd> = VecDeque::new();
    loop {
        let env = read_envelope(&mut tcp_read, cap)?;
        liveness.touch();
        match env.kind {
            EnvelopeKind::Data => {
                let declared = ldp_fd_count(&env.body)?;
                if usize::from(declared) != pending.len() {
                    let _ = enqueue(tx, Envelope::bye(ByeReason::ProtocolError));
                    teardown(liveness, sockets);
                    return Err(LdpError::malformed(
                        ErrorCode::FdMismatch,
                        format!(
                            "ldp-remote edge: DATA declares {declared} fds, {} queued",
                            pending.len()
                        ),
                    ));
                }
                let mut fds = FdList::new();
                for fd in pending.drain(..) {
                    fds.push(fd);
                }
                send_framed(writer, client_write, &env.body, &mut fds)?;
            }
            EnvelopeKind::FdSegment => {
                let (id, content) = Envelope::parse_fd_segment(&env.body)?;
                let fd = sys::filled_memfd("ldp-remote-snapshot", content)?;
                let _ = id; // Downstream snapshots ride the message, no
                            // later updates reference them.
                pending.push_back(fd);
            }
            EnvelopeKind::Fence => {
                let counter = Envelope::parse_fence(&env.body)?;
                pending.push_back(sys::eventfd_with(counter)?);
            }
            EnvelopeKind::StreamOpen => {
                let id = Envelope::parse_stream_id(&env.body)?;
                let (read_end, write_end) = sys::pipe()?;
                pending.push_back(write_end);
                let tx = tx.clone();
                if let Ok(handle) = std::thread::Builder::new()
                    .name("ldp-remote-edge-stream".into())
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
                        .map_err(|e| LdpError::Io(Arc::new(std::io::Error::other(e))))?
                };
                let Some(sink) = sink else {
                    teardown(liveness, sockets);
                    return Err(LdpError::Logic {
                        what: "ldp-remote edge: chunk for an unknown stream id",
                    });
                };
                write_append(&sink, bytes)?;
            }
            EnvelopeKind::StreamFin => {
                let id = Envelope::parse_stream_id(&env.body)?;
                stores.lock().expect("stores lock").sinks.remove(&id);
            }
            EnvelopeKind::Ping => answer_ping(&env, tx)?,
            EnvelopeKind::Pong => {
                liveness.touch();
            }
            EnvelopeKind::PoolUpdate | EnvelopeKind::PoolResize => {
                // Pool state flows upstream only; a hub never sends it.
                teardown(liveness, sockets);
                return Err(LdpError::malformed(
                    ErrorCode::MalformedMessage,
                    "ldp-remote edge: pool envelope in the downstream direction",
                ));
            }
            EnvelopeKind::Bye => {
                let _ = Envelope::parse_bye(&env.body);
                teardown(liveness, sockets);
                return Ok(());
            }
            EnvelopeKind::Hello | EnvelopeKind::HelloAck => {
                teardown(liveness, sockets);
                return Err(LdpError::malformed(
                    ErrorCode::MalformedMessage,
                    "ldp-remote edge: handshake envelope outside the handshake",
                ));
            }
        }
    }
}
