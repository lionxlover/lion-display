//! The shared integration driver: the Phase 10 testbench shape as a
//! library.
//!
//! [`CompositorHandle`] brings the headless compositor up (full
//! pipeline: mock KMS bring-up, dual scanout FBs, live timeline) and
//! runs its accept loop on a background thread. [`Client`] is the
//! protocol driver: real [`ldp_client`] connections, real memfd pools
//! through the FD-carrying factory path, and the full frame cycle
//! (`frame → attach → damage → commit → committed → presented`).

use std::collections::VecDeque;
use std::io::Write as _;
use std::sync::{Arc, Mutex};

use ldp_client::core_api::EventHandler;
use ldp_client::queue::Event;
use ldp_client::{Connection, Proxy};
use ldp_core::error::Result;
use ldp_core::geometry::Rect;
use ldp_core::wire::{ArgType, Primitive, Value};
use ldp_transport::{FdList, UnixAddr};

use lion_compositor::server::{Compositor, CompositorConfig};
use lion_compositor::sys;
use lion_compositor::Shared;

/// A running in-process compositor (accept loop on a background
/// thread; runs until the test process ends).
pub struct CompositorHandle {
    /// The shared world (scanout + scene introspection + the device).
    pub shared: Arc<Shared>,
    /// The listening abstract address.
    pub addr: UnixAddr,
    _accept: std::thread::JoinHandle<()>,
}

static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The one cross-suite gate: the stress, crash, and hotplug suites
/// all assert process-wide FD counts and scene state, so at most one
/// of them may run at a time *in this process* (other test binaries
/// are separate processes and unaffected).
pub static SUITE_GATE: Mutex<()> = Mutex::new(());

impl CompositorHandle {
    /// Bring up the compositor and its accept loop.
    pub fn start(tag: &str) -> CompositorHandle {
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let config = CompositorConfig {
            socket: format!("ldp-itx-{tag}-{n}-{}", std::process::id()),
            ..CompositorConfig::default()
        };
        let compositor = Compositor::headless(config).expect("headless bring-up");
        let addr = compositor.addr.clone();
        let shared = Arc::clone(&compositor.shared);
        let accept = std::thread::Builder::new()
            .name(format!("ldp-itx-accept-{tag}"))
            .spawn(move || {
                let mut compositor = compositor;
                // Runs until the test process ends; the listener parks
                // in accept between connections.
                let _ = compositor.serve_blocking();
            })
            .expect("accept thread");
        CompositorHandle {
            shared,
            addr,
            _accept: accept,
        }
    }

    /// Frames rendered so far.
    pub fn frames(&self) -> u64 {
        let world = self.shared.world.lock().expect("world lock");
        world.frames
    }

    /// The compositor's current virtual time (ns).
    pub fn now_ns(&self) -> u64 {
        let world = self.shared.world.lock().expect("world lock");
        world.now().as_ns()
    }

    /// Read-side access to the whole world.
    pub fn world<R>(&self, f: impl FnOnce(&lion_compositor::World) -> R) -> R {
        let world = self.shared.world.lock().expect("world lock");
        f(&world)
    }

    /// Write-side access to the whole world (hotplug injection and
    /// other device steering).
    pub fn world_mut<R>(&self, f: impl FnOnce(&mut lion_compositor::World) -> R) -> R {
        let mut world = self.shared.world.lock().expect("world lock");
        f(&mut world)
    }
}

/// One recorded event.
#[derive(Clone, Debug)]
pub struct Recorded {
    /// The event name.
    pub event: String,
    /// The decoded arguments.
    pub args: Vec<Value>,
}

/// The collecting event handler.
///
/// Bounded: long-running suites (the 15-minute stress gate observes
/// millions of events) keep only the most recent records — every
/// predicate in these suites matches within a few events of arrival —
/// plus a total counter.
#[derive(Default)]
pub struct Recorder {
    /// The most recent events, arrival order (bounded to
    /// [`Recorder::CAP`]).
    pub records: VecDeque<Recorded>,
    /// Total events ever recorded.
    pub total: u64,
}

impl Recorder {
    /// The retention cap.
    pub const CAP: usize = 256;
}

impl EventHandler for Recorder {
    fn on_event(&mut self, event: &Event) -> Result<()> {
        if self.records.len() == Self::CAP {
            self.records.pop_front();
        }
        self.records.push_back(Recorded {
            event: event.op.name.to_string(),
            args: event.args.clone(),
        });
        self.total += 1;
        Ok(())
    }
}

/// A protocol-driving client.
pub struct Client {
    /// The connection (handshake completed by `connect`).
    pub conn: Connection,
    /// The event recorder.
    pub rec: Recorder,
}

impl Client {
    /// Connect and handshake.
    pub fn connect(addr: &UnixAddr) -> Client {
        let conn = Connection::connect(addr).expect("connect + handshake");
        Client {
            conn,
            rec: Recorder::default(),
        }
    }

    /// One sync round-trip (dispatches everything queued before it).
    pub fn sync(&mut self) {
        self.conn.roundtrip(&mut self.rec).expect("roundtrip");
    }

    /// Round-trip until `pred` holds, with a wall-clock deadline.
    pub fn wait_until(&mut self, pred: impl Fn(&Recorder) -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !pred(&self.rec) {
            assert!(
                deadline > std::time::Instant::now(),
                "timed out waiting for an event; collected so far: {:#?}",
                self.rec.records
            );
            self.sync();
        }
    }

    /// Bind a global and drain its `bound` confirmation.
    pub fn bind(&mut self, interface: &str) -> Proxy {
        let proxy = self.conn.bind(interface).expect("bind");
        self.wait_until(|rec| rec.records.iter().any(|r| r.event == "bound"));
        proxy
    }

    /// Every recorded event of one name.
    pub fn events_of(&self, name: &str) -> Vec<&Recorded> {
        self.rec
            .records
            .iter()
            .filter(|r| r.event == name)
            .collect()
    }

    /// One raw request.
    pub fn send(&mut self, proxy: &Proxy, op: &str, args: Vec<Value>) {
        self.conn
            .send_request(proxy, op, args)
            .unwrap_or_else(|e| panic!("request '{op}' failed: {e}"));
    }

    /// `shm.create_pool(fd, size)` through the FD-carrying factory
    /// path; the pool is `size` bytes of `fill`.
    pub fn create_pool(&mut self, shm: &Proxy, size: i64, fill: u8) -> Proxy {
        let mem = sys::memfd("ldp-integration-pool").expect("memfd");
        let mut sink = std::fs::File::from(mem);
        sink.write_all(&vec![fill; size as usize])
            .expect("fill pool");
        let fd: std::os::fd::OwnedFd = sink.into();
        let mut fds = FdList::new();
        fds.push(fd);
        self.conn
            .create_object_fd(
                shm,
                "create_pool",
                vec![Value::Fd(0), Value::Int64(size)],
                &mut fds,
            )
            .expect("create_pool")
    }

    /// `shm_pool.create_buffer(offset, w, h, stride, format)`.
    pub fn create_buffer(
        &mut self,
        pool: &Proxy,
        offset: i32,
        w: i32,
        h: i32,
        stride: i32,
        format: u32,
    ) -> Proxy {
        self.conn
            .create_object(
                pool,
                "create_buffer",
                vec![
                    Value::Int32(offset),
                    Value::Int32(w),
                    Value::Int32(h),
                    Value::Int32(stride),
                    Value::Uint32(format),
                ],
            )
            .expect("create_buffer")
    }

    /// `compositor.create_surface()`.
    pub fn create_surface(&mut self, compositor: &Proxy) -> Proxy {
        self.conn
            .create_object(compositor, "create_surface", vec![])
            .expect("create_surface")
    }

    /// `surface.frame(frame_id)`.
    pub fn frame(&mut self, surface: &Proxy, frame_id: u64) {
        self.send(surface, "frame", vec![Value::Uint64(frame_id)]);
    }

    /// `surface.attach(buffer)`.
    pub fn attach(&mut self, surface: &Proxy, buffer: &Proxy) {
        self.send(surface, "attach", vec![Value::Object(Some(buffer.id()))]);
    }

    /// `surface.damage(rects)`.
    pub fn damage(&mut self, surface: &Proxy, rects: &[Rect]) {
        let value = Value::array(
            ArgType::Rect,
            rects
                .iter()
                .map(|r| Primitive::Rect(*r))
                .collect::<Vec<_>>(),
        )
        .expect("rect array");
        self.send(surface, "damage", vec![value]);
    }

    /// `surface.commit(cookie)`.
    pub fn commit(&mut self, surface: &Proxy, cookie: u32) {
        self.send(surface, "commit", vec![Value::Uint32(cookie)]);
    }

    /// `connection.destroy(proxy)`.
    pub fn destroy(&mut self, proxy: &Proxy) {
        self.conn.destroy(proxy).expect("destroy");
    }

    /// The 2×2 XRGB8888 quad pool size (stride 8).
    pub const QUAD_POOL_BYTES: i64 = 16;

    /// A full healthy session — the canary: bind, pool, buffer,
    /// surface, one committed frame, the presentation verdict, and a
    /// clean teardown. Used by every suite to prove the server is
    /// unwedged after abuse.
    pub fn canary_session(handle: &CompositorHandle) {
        let mut client = Client::connect(&handle.addr);
        let shm = client.bind("ldp.core.shm");
        client.wait_until(|rec| rec.records.iter().filter(|r| r.event == "format").count() >= 2);
        let pool = client.create_pool(&shm, Self::QUAD_POOL_BYTES, 0x40);
        let buffer = client.create_buffer(&pool, 0, 2, 2, 8, 0x3432_5258);
        let compositor = client.bind("ldp.core.compositor");
        let surface = client.create_surface(&compositor);

        client.frame(&surface, 1);
        client.wait_until(|rec| rec.records.iter().any(|r| r.event == "frame_target"));
        client.attach(&surface, &buffer);
        client.damage(&surface, &[Rect::new(0, 0, 2, 2)]);
        client.commit(&surface, 0xCAFE);
        client.wait_until(|rec| {
            rec.records
                .iter()
                .any(|r| r.event == "committed" && r.args[0] == Value::Uint32(0xCAFE))
        });
        client.wait_until(|rec| rec.records.iter().any(|r| r.event == "presented"));

        client.destroy(&buffer);
        client.destroy(&surface);
        client.wait_until(|rec| rec.records.iter().any(|r| r.event == "destroyed"));
        client.destroy(&pool);
    }
}
