//! Shared integration-test harness: an `ldp-server` driven from the
//! client side, plus a scripted dispatcher for the event-flood tests.
//!
//! The server side mirrors `ldp-server`'s own `common/mod.rs` (accept
//! loop on its own thread, stopped by waking the listener); the client
//! side is the library under test — no raw-codec mock here, on purpose:
//! the exit criteria exercise the *client library*, not a second
//! hand-rolled protocol implementation.

#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ldp_core::error::Result;
use ldp_core::wire::Value;
use ldp_server::{DispatchCtx, Dispatcher, GlobalAdvert, IncomingRequest, Server, ServerConfig};
use ldp_transport::{TransportListener, TransportStream, UnixAddr};

static ADDR_SEQ: AtomicU64 = AtomicU64::new(0);

/// A unique abstract address name per call (tests run in parallel).
pub fn addr_name(tag: &str) -> String {
    let n = ADDR_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("ldp-cli-{tag}-{}-{n}", std::process::id())
}

/// A unique abstract address per call.
pub fn addr(tag: &str) -> UnixAddr {
    UnixAddr::abstract_name(addr_name(tag).as_bytes()).unwrap()
}

/// The default test advertisement: the factories the client tests bind.
pub fn factories() -> Vec<GlobalAdvert> {
    vec![
        GlobalAdvert::new("ldp.core.output"),
        GlobalAdvert::new("ldp.core.compositor"),
        GlobalAdvert::new("ldp.core.shm"),
        GlobalAdvert::new("ldp.input.seat"),
    ]
}

/// A controllable in-thread server: accept loop on its own thread,
/// stoppable by connecting a dummy client (wakes the blocked accept).
pub struct ServerHarness {
    /// The address clients connect to.
    pub addr: UnixAddr,
    server: Arc<Server>,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl ServerHarness {
    /// Start with the [`NullDispatcher`](ldp_server::NullDispatcher).
    pub fn start(tag: &str, config: ServerConfig) -> ServerHarness {
        ServerHarness::start_with(tag, config, ldp_server::NullDispatcher)
    }

    /// Start with a per-session dispatcher instance.
    pub fn start_with<D>(tag: &str, config: ServerConfig, dispatcher: D) -> ServerHarness
    where
        D: Dispatcher + Clone + Send + Sync + 'static,
    {
        let server = Arc::new(Server::new(config).unwrap());
        let a = addr(tag);
        let listener = TransportListener::bind(&a, 32).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let (loop_server, loop_stop) = (Arc::clone(&server), Arc::clone(&stop));
        let join = std::thread::Builder::new()
            .name("ldp-cli-accept".into())
            .spawn(move || {
                let mut listener = listener;
                let dispatcher = dispatcher;
                loop {
                    if loop_stop.load(Ordering::Relaxed) {
                        break;
                    }
                    match loop_server.accept_session(&mut listener) {
                        Ok(Some(session)) => {
                            // One dispatcher clone per session: the scripted
                            // dispatcher keeps cross-session counters in
                            // shared Arcs.
                            let per_session = dispatcher.clone();
                            if loop_server
                                .spawn_session(session, move || per_session)
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
            .unwrap();
        ServerHarness {
            addr: a,
            server,
            stop,
            join: Some(join),
        }
    }

    /// Live session count.
    pub fn live(&self) -> u32 {
        self.server.live_sessions()
    }

    /// Poll until exactly `expected` sessions are live, or `timeout`
    /// elapses.
    pub fn wait_live(&self, expected: u32, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self.live() == expected {
                return true;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        self.live() == expected
    }

    /// Poll until no session is live.
    pub fn wait_live_zero(&self, timeout: Duration) -> bool {
        self.wait_live(0, timeout)
    }

    /// Stop the accept loop and wait for it.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Wake the blocked accept with a throwaway connection.
        let _ = TransportStream::connect(&self.addr);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

/// The minimal factory dispatcher: answers `compositor.create_surface`
/// and `seat.get_pointer` by creating the objects; everything else is
/// consumed silently (the [`ldp_server::NullDispatcher`] behavior).
///
/// [`ldp_server::NullDispatcher`]: ldp_server::NullDispatcher
#[derive(Clone, Copy, Debug, Default)]
pub struct FactoryDispatcher;

impl Dispatcher for FactoryDispatcher {
    fn on_request(
        &mut self,
        ctx: &mut DispatchCtx<'_>,
        request: &IncomingRequest<'_>,
    ) -> Result<()> {
        match (request.interface, request.op.name) {
            ("ldp.core.compositor", "create_surface") => {
                let Value::NewId(id) = request.args[0] else {
                    unreachable!("stage 3 guarantees the new_id shape");
                };
                ctx.create_object(id, "ldp.core.surface", 1)?;
                Ok(())
            }
            ("ldp.input.seat", "get_pointer") => {
                let Value::NewId(id) = request.args[0] else {
                    unreachable!("stage 3 guarantees the new_id shape");
                };
                ctx.create_object(id, "ldp.input.pointer", 1)?;
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

/// What the scripted dispatcher does with one `surface.commit`.
#[derive(Clone, Copy, Debug)]
pub struct FloodPlan {
    /// frame_target events before the pointer motion.
    pub pre_motion: u32,
    /// frame_target events after the pointer motion.
    pub post_motion: u32,
    /// presentation events after the flood (same lane, different op).
    pub post_presented: u32,
    /// configuration events after everything: must overtake the
    /// presentation backlog but never the motion.
    pub post_config: u32,
}

impl FloodPlan {
    /// A flood around one motion event.
    #[must_use]
    pub fn symmetric(n: u32) -> FloodPlan {
        FloodPlan {
            pre_motion: n,
            post_motion: n,
            post_presented: n / 4,
            post_config: 8,
        }
    }

    /// Total events one commit produces.
    #[must_use]
    pub const fn total(self) -> u32 {
        1 + self.pre_motion + self.post_motion + self.post_presented + self.post_config
    }
}

/// Cross-session bookkeeping of the scripted dispatcher.
#[derive(Default)]
struct ScriptShared {
    surfaces: Mutex<Vec<u32>>,
    pointers: Mutex<Vec<u32>>,
    floods: Mutex<u64>,
}

/// The scripted dispatcher of the class-lane tests: creates surface and
/// pointer objects on the factory requests, and answers every
/// `surface.commit` with a full [`FloodPlan`] of events on the
/// committing surface and the first pointer.
#[derive(Clone)]
pub struct FloodDispatcher {
    plan: FloodPlan,
    shared: Arc<ScriptShared>,
}

impl FloodDispatcher {
    /// A dispatcher answering commits with `plan`.
    #[must_use]
    pub fn new(plan: FloodPlan) -> FloodDispatcher {
        FloodDispatcher {
            plan,
            shared: Arc::new(ScriptShared::default()),
        }
    }

    /// How many floods were emitted.
    pub fn floods(&self) -> u64 {
        *self.shared.floods.lock().unwrap()
    }
}

impl Dispatcher for FloodDispatcher {
    fn on_request(
        &mut self,
        ctx: &mut DispatchCtx<'_>,
        request: &IncomingRequest<'_>,
    ) -> Result<()> {
        match (request.interface, request.op.name) {
            ("ldp.core.compositor", "create_surface") => {
                let Value::NewId(id) = request.args[0] else {
                    unreachable!("stage 3 guarantees the new_id shape");
                };
                ctx.create_object(id, "ldp.core.surface", 1)?;
                self.shared.surfaces.lock().unwrap().push(id.as_u32());
                Ok(())
            }
            ("ldp.input.seat", "get_pointer") => {
                let Value::NewId(id) = request.args[0] else {
                    unreachable!("stage 3 guarantees the new_id shape");
                };
                ctx.create_object(id, "ldp.input.pointer", 1)?;
                self.shared.pointers.lock().unwrap().push(id.as_u32());
                Ok(())
            }
            ("ldp.core.surface", "commit") => {
                let surface = request.object;
                let pointer = self
                    .shared
                    .pointers
                    .lock()
                    .unwrap()
                    .first()
                    .copied()
                    .map(ldp_core::ids::ObjectId::from_wire)
                    .ok_or(ldp_core::error::LdpError::Logic {
                        what: "flood test needs a pointer before commit",
                    })?;
                *self.shared.floods.lock().unwrap() += 1;
                let plan = &self.plan;
                let mut frame: u64 = 0;
                for _ in 0..plan.pre_motion {
                    emit_frame_target(ctx, surface, frame)?;
                    frame += 1;
                }
                emit_motion(ctx, pointer, 1.5, 2.5)?;
                for _ in 0..plan.post_motion {
                    emit_frame_target(ctx, surface, frame)?;
                    frame += 1;
                }
                for _ in 0..plan.post_presented {
                    emit_presented(ctx, surface, frame)?;
                    frame += 1;
                }
                for _ in 0..plan.post_config {
                    emit_preferred_scale(ctx, surface)?;
                }
                // The committed ack is a configuration-class event.
                let Value::Uint32(cookie) = request.args[0] else {
                    unreachable!("stage 3 guarantees the cookie shape");
                };
                ctx.emit(surface, "committed", vec![Value::Uint32(cookie)])
            }
            _ => Ok(()),
        }
    }
}

fn emit_frame_target(
    ctx: &mut DispatchCtx<'_>,
    surface: ldp_core::ids::ObjectId,
    frame: u64,
) -> Result<()> {
    ctx.emit(
        surface,
        "frame_target",
        vec![
            Value::Uint64(frame),
            Value::Ts(1_000_000_000),
            Value::Uint64(16_666_666),
            Value::Uint64(8_000_000),
            Value::Enum(1), // presentation_mode::vsync
        ],
    )
}

fn emit_presented(
    ctx: &mut DispatchCtx<'_>,
    surface: ldp_core::ids::ObjectId,
    frame: u64,
) -> Result<()> {
    ctx.emit(
        surface,
        "presented",
        vec![
            Value::Uint64(frame),
            Value::Ts(1_000_016_666),
            Value::Uint64(16_666_666),
            Value::Bitset(ldp_core::bitset::Bitset128::single(0)), // vblank
        ],
    )
}

fn emit_motion(
    ctx: &mut DispatchCtx<'_>,
    pointer: ldp_core::ids::ObjectId,
    x: f32,
    y: f32,
) -> Result<()> {
    ctx.emit(
        pointer,
        "motion",
        vec![Value::Float32(x), Value::Float32(y)],
    )
}

fn emit_preferred_scale(ctx: &mut DispatchCtx<'_>, surface: ldp_core::ids::ObjectId) -> Result<()> {
    ctx.emit(surface, "preferred_scale", vec![Value::Uint32(256)])
}
