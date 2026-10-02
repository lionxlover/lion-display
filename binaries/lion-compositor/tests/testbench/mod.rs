//! The Phase 10 integration harness: an in-process compositor plus a
//! driving client over the real transport.
//!
//! [`Testbench::start`] brings the headless compositor up (the full
//! pipeline: mock KMS bring-up, dual scanout FBs, live timeline) and
//! runs its accept loop on a background thread; tests connect with the
//! real [`ldp_client`] library, speak the real protocol, and observe
//! both sides — the client's event stream *and* the compositor's
//! scanout content — against one deterministic virtual clock.

#![allow(dead_code)]

use std::io::Write;
use std::sync::Arc;
use std::time::{Duration, Instant};

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

/// A running in-process compositor.
pub struct Testbench {
    /// The shared world (scanout + scene introspection).
    pub shared: Arc<Shared>,
    /// The listening address.
    pub addr: UnixAddr,
    _accept: std::thread::JoinHandle<()>,
}

static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl Testbench {
    /// Bring up the compositor and its accept loop.
    pub fn start(tag: &str) -> Testbench {
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let config = CompositorConfig {
            socket: format!("lion-it-{tag}-{n}-{}", std::process::id()),
            ..CompositorConfig::default()
        };
        Self::start_with(tag, config)
    }

    /// Bring up a compositor with an explicit config (the hardware
    /// suite injects the reference GL backend this way).
    pub fn start_with(tag: &str, config: CompositorConfig) -> Testbench {
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let _ = n;
        let compositor = Compositor::headless(config).expect("headless bring-up");
        let addr = compositor.addr.clone();
        let shared = Arc::clone(&compositor.shared);
        let accept = std::thread::Builder::new()
            .name(format!("accept-{tag}"))
            .spawn(move || {
                let mut compositor = compositor;
                // Runs until the test process ends; the listener parks
                // in accept between connections.
                let _ = compositor.serve_blocking();
            })
            .expect("accept thread");
        Testbench {
            shared,
            addr,
            _accept: accept,
        }
    }

    /// The compositor's current scanout (premultiplied ARGB words).
    /// Panics on a dark world — the hotplug suite reads dark states
    /// through [`Testbench::world`] instead.
    pub fn scanout(&self) -> Vec<u32> {
        let world = self.shared.world.lock().expect("world lock");
        world.scanout_words().expect("the compositor is lit")
    }

    /// The compositor's current virtual time.
    pub fn now_ns(&self) -> u64 {
        let world = self.shared.world.lock().expect("world lock");
        world.now().as_ns()
    }

    /// Frames rendered so far.
    pub fn frames(&self) -> u64 {
        let world = self.shared.world.lock().expect("world lock");
        world.frames
    }

    /// Read-side access to the whole world.
    pub fn world<R>(&self, f: impl FnOnce(&lion_compositor::World) -> R) -> R {
        let world = self.shared.world.lock().expect("world lock");
        f(&world)
    }

    /// Write-side access to the whole world (the idle/input rigs).
    pub fn world_mut<R>(&self, f: impl FnOnce(&mut lion_compositor::World) -> R) -> R {
        let mut world = self.shared.world.lock().expect("world lock");
        f(&mut world)
    }
}

/// One recorded event (arguments plus any riding fence state).
#[derive(Clone, Debug)]
pub struct Recorded {
    /// The target object's wire id.
    pub target: u32,
    /// The target's interface name.
    pub interface: String,
    /// The event name.
    pub event: String,
    /// The decoded arguments.
    pub args: Vec<Value>,
    /// Whether a riding eventfd fence read as signalled (`None` when
    /// the event carried no descriptor).
    pub fence: Option<bool>,
    /// The snapshot bytes of a `capture_manager.frame` event (`None`
    /// otherwise).
    pub snapshot: Option<Vec<u8>>,
}

/// The collecting event handler.
#[derive(Default)]
pub struct Collector {
    /// Every event in arrival order.
    pub records: Vec<Recorded>,
}

impl EventHandler for Collector {
    fn on_event(&mut self, event: &Event) -> Result<()> {
        // Capture frames: the riding descriptor is a read-once snapshot,
        // not a fence — read its content immediately (the descriptor list
        // closes when the queue drops the event).
        let is_frame = event.interface == "ldp.capture.capture_manager" && event.op.name == "frame";
        let snapshot = if is_frame && !event.fds.is_empty() {
            event.fds.raw_at(0).map(read_snapshot)
        } else {
            None
        };
        // Read a riding fence immediately: the event's descriptor list
        // closes when the queue drops the event.
        let fence = if is_frame {
            None
        } else {
            (!event.fds.is_empty())
                .then(|| {
                    event
                        .fds
                        .raw_at(0)
                        .map(|raw| sys::fence_signalled_raw(raw).unwrap_or(false))
                })
                .flatten()
        };
        self.records.push(Recorded {
            target: event.target.as_u32(),
            interface: event.interface.to_string(),
            event: event.op.name.to_string(),
            args: event.args.clone(),
            fence,
            snapshot,
        });
        Ok(())
    }
}

/// Read a snapshot descriptor's whole content (dup + read-to-EOF, copied
/// out before the queue closes the original).
fn read_snapshot(raw: i32) -> Vec<u8> {
    use std::io::Read as _;
    use std::os::fd::FromRawFd as _;
    // SAFETY: dup(2) of a valid descriptor passed through the event's
    // FD list; the duplicate is adopted by File and closed on drop.
    let dup = unsafe { libc::dup(raw) };
    assert!(dup != -1, "dup of the snapshot descriptor");
    // SAFETY: the dup result is a fresh, unowned descriptor.
    let fd = unsafe { std::os::fd::OwnedFd::from_raw_fd(dup) };
    let mut bytes = Vec::new();
    std::fs::File::from(fd)
        .read_to_end(&mut bytes)
        .expect("read the capture snapshot");
    bytes
}

/// A protocol-driving test client.
pub struct TestClient {
    /// The connection.
    pub conn: Connection,
    /// The event collector.
    pub events: Collector,
}

impl TestClient {
    /// Connect and handshake.
    pub fn connect(addr: &UnixAddr) -> TestClient {
        let conn = Connection::connect(addr).expect("connect + handshake");
        TestClient {
            conn,
            events: Collector::default(),
        }
    }

    /// One sync round-trip (dispatches everything queued before it).
    pub fn sync(&mut self) {
        self.conn.roundtrip(&mut self.events).expect("roundtrip");
    }

    /// Round-trip until `pred` holds over the collected records, with a
    /// wall-clock deadline (each iteration dispatches the previous
    /// pump's output — the wake-point doctrine on the client side).
    pub fn wait_until(&mut self, pred: impl Fn(&Collector) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !pred(&self.events) {
            assert!(
                deadline > Instant::now(),
                "timed out waiting for an event; collected so far: {:#?}",
                self.events.records
            );
            self.sync();
        }
    }

    /// Bind a global and drain its `bound` confirmation.
    pub fn bind(&mut self, interface: &str) -> Proxy {
        let proxy = self.conn.bind(interface).expect("bind");
        self.wait_until(|c| c.records.iter().any(|r| r.event == "bound"));
        proxy
    }

    /// Every recorded event of one name.
    pub fn events_of(&self, name: &str) -> Vec<&Recorded> {
        self.events
            .records
            .iter()
            .filter(|r| r.event == name)
            .collect()
    }

    /// The latest recorded event of one name.
    pub fn last(&self, name: &str) -> &Recorded {
        self.events_of(name)
            .last()
            .unwrap_or_else(|| panic!("no '{name}' event recorded"))
    }
}

// ---- protocol helpers (typed sends over the raw value API) ----------

/// Create a memfd pool of `size` bytes filled with `fill`, returning
/// the owned descriptor.
pub fn memfd_pool(size: u64, fill: u8) -> std::os::fd::OwnedFd {
    let mem = sys::memfd("test-pool").expect("memfd");
    let mut sink = std::fs::File::from(mem);
    sink.write_all(&vec![fill; size as usize])
        .expect("fill pool");
    sink.into()
}

/// `shm.create_pool(fd, size)` through the FD-carrying factory path.
pub fn create_pool(
    client: &mut TestClient,
    shm: &Proxy,
    fd: std::os::fd::OwnedFd,
    size: i64,
) -> Proxy {
    let mut fds = FdList::new();
    fds.push(fd);
    client
        .conn
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
    client: &mut TestClient,
    pool: &Proxy,
    offset: i32,
    w: i32,
    h: i32,
    stride: i32,
    format: u32,
) -> Proxy {
    client
        .conn
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

/// An `array<rect>` argument.
pub fn rect_arg(rects: &[Rect]) -> Value {
    Value::array(
        ArgType::Rect,
        rects
            .iter()
            .map(|r| Primitive::Rect(*r))
            .collect::<Vec<_>>(),
    )
    .expect("rect array")
}

/// `surface.attach(buffer)`.
pub fn attach(client: &mut TestClient, surface: &Proxy, buffer: &Proxy) {
    client
        .conn
        .send_request(surface, "attach", vec![Value::Object(Some(buffer.id()))])
        .expect("attach");
}

/// `surface.damage(rects)`.
pub fn damage(client: &mut TestClient, surface: &Proxy, rects: &[Rect]) {
    client
        .conn
        .send_request(surface, "damage", vec![rect_arg(rects)])
        .expect("damage");
}

/// `surface.commit(cookie)`.
pub fn commit(client: &mut TestClient, surface: &Proxy, cookie: u32) {
    client
        .conn
        .send_request(surface, "commit", vec![Value::Uint32(cookie)])
        .expect("commit");
}

/// `surface.frame(frame_id)`.
pub fn frame(client: &mut TestClient, surface: &Proxy, frame_id: u64) {
    client
        .conn
        .send_request(surface, "frame", vec![Value::Uint64(frame_id)])
        .expect("frame");
}

/// `connection.destroy(proxy)`.
pub fn destroy(client: &mut TestClient, proxy: &Proxy) {
    client.conn.destroy(proxy).expect("destroy");
}
