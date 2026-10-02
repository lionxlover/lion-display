//! Target: the server dispatch path (`ldp-server`).
//!
//! A real [`Server`] with the null dispatcher and a real accept loop;
//! each iteration is one fuzzed client connection:
//!
//! * **handshake then fuzz** — a proper `hello`/`welcome`, then a
//!   burst of mutated spec requests, aligned random garbage, forged
//!   target object ids (stale-ID and type-confusion shapes), and
//!   event opcodes smuggled in the request direction,
//! * **garbage first** — bytes before any handshake,
//! * **vanish mid-message** — a partial frame, then the client drops
//!   (the crash-reclamation shape),
//! * **clean session** — handshake, bind, sync (the control group).
//!
//! Invariants: the server never panics, every fuzzed connection dies
//! with a structured verdict or drains cleanly after `sync`, sessions
//! are reclaimed (`live_sessions` returns to zero), and the process
//! FD table returns to its baseline once everything is torn down.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ldp_core::ids::CONNECTION_OBJECT_ID;
use ldp_core::limits::Limits;
use ldp_core::wire::Value;
use ldp_protocol::schema::Direction;
use ldp_protocol::{decode, Message, ValidationMode, REGISTRY};
use ldp_server::{GlobalAdvert, NullDispatcher, Server, ServerConfig};
use ldp_test::mutate::{mutate_bytes, mutate_message};
use ldp_test::rng::SplitMix64;
use ldp_transport::{
    is_would_block, FdList, FramedReader, FramedWriter, NoHooks, TransportListener,
    TransportStream, UnixAddr,
};

use crate::report::TargetReport;

/// Request opcode of `interface.name` from the compiled schema.
fn op(interface: &str, name: &str) -> u32 {
    REGISTRY
        .interface(interface)
        .unwrap()
        .requests
        .iter()
        .find(|r| r.name == name)
        .unwrap()
        .opcode
}

/// Event opcode of `interface.name` from the compiled schema.
fn ev(interface: &str, name: &str) -> u32 {
    REGISTRY
        .interface(interface)
        .unwrap()
        .events
        .iter()
        .find(|r| r.name == name)
        .unwrap()
        .opcode
}

/// A raw-protocol fuzz client with deadline-bounded reads.
struct FuzzClient {
    stream: TransportStream,
    reader: FramedReader,
    writer: FramedWriter<NoHooks>,
}

impl FuzzClient {
    fn connect(addr: &UnixAddr) -> FuzzClient {
        let mut stream = TransportStream::connect(addr).expect("connect");
        stream.set_nonblocking(true).expect("nonblocking");
        FuzzClient {
            stream,
            reader: FramedReader::new(Limits::DEFAULT),
            writer: FramedWriter::without_hooks(Limits::DEFAULT),
        }
    }

    /// Send framed bytes; framing-invalid inputs are counted as
    /// locally-rejected (the sender-side mirror of stage 1).
    fn send_bytes(&mut self, bytes: &[u8]) -> bool {
        self.writer
            .send_msg(&mut self.stream, bytes, &mut FdList::new())
            .is_ok()
    }

    /// Send one message object.
    fn send(&mut self, msg: &Message) -> bool {
        match msg.encode(&Limits::DEFAULT) {
            Ok(bytes) => self.send_bytes(&bytes),
            Err(_) => false,
        }
    }

    /// Read frames until a transport verdict, a drain-quiet deadline,
    /// or the sync marker. Returns the number of frames read and
    /// whether the connection ended with an error.
    fn drain(&mut self, marker: Option<u32>) -> (u64, bool) {
        let mut frames = 0u64;
        // Adaptive quiet window: generous before anything arrives,
        // short once replies are flowing.
        let mut deadline = Instant::now() + Duration::from_millis(60);
        loop {
            match self.reader.recv_msg(&mut self.stream) {
                Ok(frame) => {
                    frames += 1;
                    deadline = Instant::now() + Duration::from_millis(5);
                    if let Ok(m) = decode(
                        frame.message_bytes(),
                        u32::from(frame.fd_count()),
                        &Limits::DEFAULT,
                        ValidationMode::Tolerant,
                    ) {
                        if let Some(cookie) = marker {
                            if m.opcode == ev("ldp.core.connection", "sync_done")
                                && m.object_id == CONNECTION_OBJECT_ID
                                && m.args.first() == Some(&Value::Uint32(cookie))
                            {
                                return (frames, false);
                            }
                        }
                    }
                }
                Err(e) => {
                    if is_would_block(&e) {
                        if Instant::now() >= deadline {
                            return (frames, false);
                        }
                        std::thread::sleep(Duration::from_millis(1));
                    } else {
                        return (frames, true);
                    }
                }
            }
        }
    }
}

/// Run the dispatch fuzzer.
///
/// Deterministic in `(seed, connections)` except for OS scheduling.
///
/// # Panics
///
/// Only on invariant violations: the server failing to accept a
/// healthy connection, a clean session not completing, sessions not
/// reclaimed after teardown, or an FD leak.
#[allow(clippy::too_many_lines)] // one straight-line harness loop
pub fn run(seed: u64, connections: u64) -> TargetReport {
    let fd_baseline = ldp_transport::count_open_fds().expect("fd count");

    // The server under fuzz: standard globals, null dispatcher.
    let config = ServerConfig {
        globals: vec![
            GlobalAdvert::new("ldp.core.output"),
            GlobalAdvert::new("ldp.core.compositor"),
            GlobalAdvert::new("ldp.core.shm"),
        ],
        ..ServerConfig::default()
    };
    let server = Arc::new(Server::new(config).expect("server config"));
    let name = format!("ldp-fuzz-dispatch-{}-{seed}", std::process::id());
    let addr = UnixAddr::abstract_name(name.as_bytes()).expect("abstract name");
    let listener = TransportListener::bind(&addr, 32).expect("bind");
    let stop = Arc::new(AtomicBool::new(false));
    let (loop_server, loop_stop) = (Arc::clone(&server), Arc::clone(&stop));
    let accept = std::thread::Builder::new()
        .name("ldp-fuzz-accept".into())
        .spawn(move || {
            let mut listener = listener;
            loop {
                if loop_stop.load(Ordering::Relaxed) {
                    break;
                }
                match loop_server.accept_session(&mut listener) {
                    Ok(Some(session)) => {
                        if loop_server
                            .spawn_session(session, || NullDispatcher)
                            .is_err()
                        {
                            break;
                        }
                    }
                    Ok(None) => {}
                    Err(_) => break,
                }
            }
        })
        .expect("accept thread");

    // The request/event op tables for message synthesis.
    let mut req_ops: Vec<(&'static str, u32)> = Vec::new();
    let mut event_ops: Vec<(&'static str, u32)> = Vec::new();
    for module in REGISTRY.modules() {
        for iface in module.interfaces {
            for op in iface.requests {
                req_ops.push((iface.name, op.opcode));
            }
            for op in iface.events {
                event_ops.push((iface.name, op.opcode));
            }
        }
    }

    let mut rng = SplitMix64::new(seed);
    let mut report = TargetReport {
        target: "dispatch",
        iterations: connections,
        ..TargetReport::default()
    };

    for _ in 0..connections {
        let mut client = FuzzClient::connect(&addr);
        let shape = rng.below(4);
        match shape {
            0 => {
                // Handshake, then a fuzzed burst.
                hello(&mut client);
                let burst = 4 + rng.below(12);
                for _ in 0..burst {
                    let pick = rng.below(4);
                    let ok = match pick {
                        0 => {
                            // Mutated spec request.
                            let (iface, opcode) = *rng.pick(&req_ops);
                            let Some(msg) = ldp_test::conformance::spec_message(
                                &mut rng,
                                &REGISTRY,
                                iface,
                                Direction::Request,
                                opcode,
                            ) else {
                                continue;
                            };
                            let bytes = msg.encode(&Limits::DEFAULT).expect("spec encode");
                            let mutant = if rng.next_bool() {
                                mutate_message(&mut rng, &bytes)
                            } else {
                                mutate_bytes(&mut rng, &bytes)
                            };
                            client.send_bytes(&mutant)
                        }
                        1 => {
                            // Aligned random garbage.
                            let len = 16 + 8 * rng.below(24);
                            client.send_bytes(&rng.bytes(len))
                        }
                        2 => {
                            // Valid request shape, forged target id:
                            // stale server ids, client-range ids, null.
                            let (iface, opcode) = *rng.pick(&req_ops);
                            let Some(mut msg) = ldp_test::conformance::spec_message(
                                &mut rng,
                                &REGISTRY,
                                iface,
                                Direction::Request,
                                opcode,
                            ) else {
                                continue;
                            };
                            msg.object_id = match rng.below(3) {
                                0 => 0,
                                1 => rng.next_u32() & !(1 << 31),
                                _ => rng.next_u32() | (1 << 31),
                            };
                            client.send(&msg)
                        }
                        _ => {
                            // Event opcode smuggled in the request
                            // direction.
                            let (iface, opcode) = *rng.pick(&event_ops);
                            let Some(mut msg) = ldp_test::conformance::spec_message(
                                &mut rng,
                                &REGISTRY,
                                iface,
                                Direction::Event,
                                opcode,
                            ) else {
                                continue;
                            };
                            msg.object_id = CONNECTION_OBJECT_ID;
                            client.send(&msg)
                        }
                    };
                    if !ok {
                        // The connection is already dead (or the bytes
                        // were framing-invalid): a structured verdict.
                        report.rejected += 1;
                        break;
                    }
                }
                let cookie = rng.next_u32();
                let _ = sync(&mut client, cookie);
                let (frames, errored) = client.drain(Some(cookie));
                report.accepted += frames;
                if errored {
                    report.rejected += 1;
                }
            }
            1 => {
                // Garbage before any handshake.
                let len = 16 + 8 * rng.below(24);
                let garbage = rng.bytes(len);
                if !client.send_bytes(&garbage) {
                    report.rejected += 1;
                }
                let (frames, errored) = client.drain(None);
                report.accepted += frames;
                if errored || frames == 0 {
                    report.rejected += 1;
                }
            }
            2 => {
                // Vanish mid-message: a partial frame, then drop.
                let words = 8 + rng.below(64) as u32;
                let msg = framed_partial(&mut rng, words);
                let cut = 1 + rng.below(msg.len().saturating_sub(1));
                let _ = client.stream.send_chunk(&msg[..cut], None);
                report.rejected += 1;
            }
            _ => {
                // Clean session: the control group.
                hello(&mut client);
                let (frames, errored) = client.drain(None);
                assert!(frames >= 1, "clean session must see its welcome");
                report.accepted += frames;
                assert!(!errored, "clean session must not error");
            }
        }
        drop(client);
    }

    // Teardown: stop the accept loop, wake it, reap everything.
    stop.store(true, Ordering::Relaxed);
    let _ = TransportStream::connect(&addr);
    let _ = accept.join();
    let deadline = Instant::now() + Duration::from_secs(10);
    while server.live_sessions() > 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(
        server.live_sessions(),
        0,
        "sessions must be fully reclaimed after the fuzz run"
    );

    // FD hygiene: the table returns to its pre-server baseline.
    let fd_end = ldp_transport::count_open_fds().expect("fd count");
    assert_eq!(
        fd_baseline, fd_end,
        "FD table changed across the dispatch fuzz run (leak)"
    );
    report
}

fn hello(client: &mut FuzzClient) {
    let msg = Message::new(CONNECTION_OBJECT_ID, op("ldp.core.connection", "hello"))
        .arg(Value::Uint32(1))
        .arg(Value::Bitset(ldp_core::bitset::Bitset128::EMPTY));
    assert!(client.send(&msg), "hello must send");
}

fn sync(client: &mut FuzzClient, cookie: u32) -> bool {
    let msg = Message::new(CONNECTION_OBJECT_ID, op("ldp.core.connection", "sync"))
        .arg(Value::Uint32(cookie));
    client.send(&msg)
}

/// A valid framed message's bytes (for the vanish-mid-message shape).
fn framed_partial(rng: &mut SplitMix64, words: u32) -> Vec<u8> {
    let mut msg = vec![0u8; 16 + words as usize * 8];
    msg[0..4].copy_from_slice(&words.to_le_bytes());
    for b in &mut msg[16..] {
        *b = rng.byte();
    }
    msg
}
