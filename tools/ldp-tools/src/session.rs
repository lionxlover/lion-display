//! The tool-side session driver: connect, bootstrap, bind, buffer
//! factories, frame feedback — the protocol choreography every tool
//! and example shares.
//!
//! This is the Phase 10 testbench's `TestClient` promoted to a public
//! library shape: one [`ToolSession`] owns the [`Connection`] and an
//! event [`Collector`] as sibling fields (so round-trips borrow them
//! disjointly), records every dispatched event with its riding-fence
//! state, and exposes the high-level operations the tools need —
//! global discovery, gated binds, shm pools with a writable client
//! handle, surface commit cycles with decoded presentation feedback,
//! and schema introspection.
//!
//! Doctrine carried over from the compositor's own suites: waiting is
//! always `roundtrip`-driven with a wall-clock deadline, release
//! fences are read immediately at dispatch (the queue closes the
//! descriptors afterwards), and binding an interface the server never
//! advertised is refused locally instead of letting the protocol kill
//! the connection.

#![forbid(unsafe_code)]

use std::io::Write as _;
use std::time::{Duration, Instant};

use ldp_client::core_api::EventHandler;
use ldp_client::queue::Event;
use ldp_client::{ClientConfig, Connection, Proxy};
use ldp_core::geometry::Rect;
use ldp_core::wire::{ArgType, Primitive, Value};
use ldp_transport::{FdList, UnixAddr};

use crate::error::{Result, ToolError};
use crate::sys;

/// DRM fourcc for `XRGB8888` — the one buffer format the Phase 10
/// vertical slice composites.
pub const FORMAT_XRGB8888: u32 = 0x3432_5258;
/// The ARGB8888 fourcc ("AR24") — the translucent family.
pub const FORMAT_ARGB8888: u32 = 0x3432_5241;

/// Default wall-clock deadline for protocol waits (generous for CI
/// scheduling noise; the headless pipeline answers in microseconds).
///
/// Public because the examples wait on the same budget the tools do.
pub const WAIT: Duration = Duration::from_secs(10);

/// One recorded event: identity, decoded arguments, and the riding
/// release-fence state (`None` when the event carried no descriptor).
/// A `capture_manager.frame` snapshot's *content* is read immediately
/// at dispatch (read-once semantics — the descriptor itself closes
/// with the queue's event).
#[derive(Clone, Debug)]
pub struct RecordedEvent {
    /// The dispatch sequence number.
    pub seq: u64,
    /// The target object's wire id.
    pub target: u32,
    /// The target's interface name.
    pub interface: String,
    /// The event name.
    pub event: String,
    /// Argument names, schema order.
    pub arg_names: Vec<&'static str>,
    /// Decoded arguments, schema order.
    pub args: Vec<Value>,
    /// Whether a riding eventfd fence read as signalled.
    pub fence: Option<bool>,
    /// The read-once snapshot content of a `capture_manager.frame`
    /// event (`None` for every other event).
    pub snapshot: Option<Vec<u8>>,
}

impl RecordedEvent {
    /// The argument named `name` (schema order is the lookup key).
    #[must_use]
    pub fn arg(&self, name: &str) -> Option<&Value> {
        self.arg_names
            .iter()
            .position(|n| *n == name)
            .and_then(|i| self.args.get(i))
    }

    /// A `uint*`/`ts` argument as `u64`.
    #[must_use]
    pub fn u64_arg(&self, name: &str) -> Option<u64> {
        match self.arg(name) {
            Some(Value::Uint32(v)) => Some(u64::from(*v)),
            Some(Value::Uint64(v) | Value::Ts(v)) => Some(*v),
            _ => None,
        }
    }

    /// A `uint32` argument as `u32`.
    #[must_use]
    pub fn u32_arg(&self, name: &str) -> Option<u32> {
        match self.arg(name) {
            Some(Value::Uint32(v)) => Some(*v),
            _ => None,
        }
    }

    /// An `int32` argument as `i32`.
    #[must_use]
    pub fn i32_arg(&self, name: &str) -> Option<i32> {
        match self.arg(name) {
            Some(Value::Int32(v)) => Some(*v),
            _ => None,
        }
    }

    /// A `string` argument.
    #[must_use]
    pub fn str_arg(&self, name: &str) -> Option<&str> {
        match self.arg(name) {
            Some(Value::String(s)) => Some(s),
            _ => None,
        }
    }
}

/// The collecting event handler: one record per dispatched event.
#[derive(Default)]
pub struct Collector {
    /// Every event in arrival order.
    pub records: Vec<RecordedEvent>,
}

impl EventHandler for Collector {
    fn on_event(&mut self, event: &Event) -> ldp_core::error::Result<()> {
        // A capture frame's riding descriptor is a read-once snapshot,
        // not a fence: read its content immediately (pread — the shared
        // offset never moves), before the descriptor closes with the
        // queue's event.
        let is_frame = event.interface == "ldp.capture.capture_manager" && event.op.name == "frame";
        let snapshot = if is_frame && !event.fds.is_empty() {
            event.fds.raw_at(0).map(sys::read_all_raw).transpose()?
        } else {
            None
        };
        // Read a riding fence immediately: the event's descriptor list
        // closes when the queue drops the event (never for capture
        // frames — their descriptor is content, not a fence).
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
        self.records.push(RecordedEvent {
            seq: event.seq,
            target: event.target.as_u32(),
            interface: event.interface.to_owned(),
            event: event.op.name.to_owned(),
            arg_names: event.op.args.iter().map(|a| a.name).collect(),
            args: event.args.clone(),
            fence,
            snapshot,
        });
        Ok(())
    }
}

/// One advertised global, decoded from the registry replay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Advertised {
    /// The interface's fully qualified name.
    pub interface: String,
    /// Lowest version the server supports.
    pub version_min: u32,
    /// Highest version the server supports.
    pub version_max: u32,
}

/// A decoded `surface.presented` event.
#[derive(Clone, Copy, Debug)]
pub struct PresentedSample {
    /// The client-chosen frame identifier.
    pub frame: u64,
    /// Presentation (vblank/flip) timestamp, monotonic ns.
    pub ts_ns: u64,
    /// Measured refresh interval, ns.
    pub refresh_ns: u64,
    /// The presented-flags bitset.
    pub flags: ldp_core::bitset::Bitset128,
}

/// A decoded `surface.frame_target` event.
#[derive(Clone, Copy, Debug)]
pub struct FrameTargetSample {
    /// The client-chosen frame identifier.
    pub frame: u64,
    /// Absolute monotonic deadline for the commit, ns.
    pub target_ns: u64,
    /// Current refresh interval, ns.
    pub refresh_ns: u64,
    /// Remaining render budget at emission, ns.
    pub budget_ns: u64,
    /// The presentation-mode enum value.
    pub mode: u32,
}

/// An shm pool with a client-side writable handle: the memfd is
/// duplicated before being handed to the server, so the tool can keep
/// painting into the shared pages between commits.
pub struct ShmPool {
    /// The pool proxy (`shm_pool` object).
    pub proxy: Proxy,
    /// The client's writable handle into the same pages.
    pub file: std::fs::File,
    /// Pool size in bytes.
    pub size: u64,
}

impl ShmPool {
    /// Overwrite `bytes` at `offset` (the client-side of the shm
    /// contract: write, then commit with damage).
    ///
    /// # Errors
    /// [`ToolError::Io`] on seek/write failure.
    pub fn write_at(&mut self, offset: u64, bytes: &[u8]) -> Result<()> {
        use std::io::Seek as _;
        let mut file = &self.file;
        file.seek(std::io::SeekFrom::Start(offset))?;
        file.write_all(bytes)?;
        Ok(())
    }
}

/// A surface with its ping-pong buffers — the frame-cycle state the
/// tracer, profiler, and examples all drive.
pub struct FrameCycle {
    /// The surface proxy.
    pub surface: Proxy,
    /// The pool the buffers come from.
    pub pool: ShmPool,
    /// The buffers (two or more: re-attaching a buffer still in scanout
    /// is what `buffer.release` exists to prevent).
    pub buffers: Vec<Proxy>,
    next: usize,
}

impl FrameCycle {
    /// The buffer for the next commit (round-robin).
    #[must_use]
    pub fn next_buffer(&mut self) -> &Proxy {
        let buffer = &self.buffers[self.next];
        self.next = (self.next + 1) % self.buffers.len();
        buffer
    }

    /// Buffer stride for `width` × XRGB8888.
    #[must_use]
    pub fn stride(width: u32) -> u32 {
        width.saturating_mul(4)
    }
}

/// A connected, bootstrapped tool session.
pub struct ToolSession {
    conn: Connection,
    events: Collector,
    registry: Option<Proxy>,
    bound: std::collections::HashMap<String, Proxy>,
    globals: Vec<Advertised>,
}

impl ToolSession {
    /// Connect and handshake, requesting the introspection option (the
    /// compositor grants it by default; `ldp-info`/`ldp-validate` need
    /// it, and the other tools are unaffected).
    ///
    /// # Errors
    /// Connection, transport, or handshake failures.
    pub fn connect(addr: &UnixAddr) -> Result<ToolSession> {
        let config = ClientConfig::default().with_introspection();
        let conn = Connection::connect_with(config, addr)?;
        Ok(ToolSession {
            conn,
            events: Collector::default(),
            registry: None,
            bound: std::collections::HashMap::new(),
            globals: Vec::new(),
        })
    }

    /// The server's `welcome` (available after [`connect`](Self::connect)).
    #[must_use]
    pub fn welcome(&self) -> Option<&ldp_client::core_api::Welcome> {
        self.conn.welcome()
    }

    /// The negotiated limits.
    #[must_use]
    pub fn limits(&self) -> ldp_core::limits::Limits {
        self.conn.limits()
    }

    /// Get the registry and replay the globals; every live tool begins
    /// here. Idempotent.
    ///
    /// # Errors
    /// Protocol errors from the round-trip.
    pub fn bootstrap(&mut self) -> Result<()> {
        if self.registry.is_none() {
            let registry = self.conn.registry()?;
            self.registry = Some(registry);
        }
        self.roundtrip()?;
        let registry_id = self.registry.as_ref().map_or(0, |p| p.id().as_u32());
        self.globals = self
            .events
            .records
            .iter()
            .filter(|r| r.event == "global" && r.target == registry_id)
            .filter_map(|r| {
                let interface = r.str_arg("interface")?.to_owned();
                Some(Advertised {
                    interface,
                    version_min: r.u32_arg("version_min")?,
                    version_max: r.u32_arg("version_max")?,
                })
            })
            .collect();
        Ok(())
    }

    /// The advertised globals (after bootstrap).
    #[must_use]
    pub fn globals(&self) -> &[Advertised] {
        &self.globals
    }

    /// Whether `interface` is advertised.
    #[must_use]
    pub fn has_global(&self, interface: &str) -> bool {
        self.globals.iter().any(|g| g.interface == interface)
    }

    /// One round-trip (dispatches everything queued before the sync).
    ///
    /// # Errors
    /// Transport or protocol failures, including server-side fatal
    /// errors raised against this connection.
    pub fn roundtrip(&mut self) -> Result<()> {
        let mut events = std::mem::take(&mut self.events);
        let outcome = self.conn.roundtrip(&mut events);
        self.events = events;
        outcome?;
        Ok(())
    }

    /// Round-trip until `pred` holds over the collected records, with a
    /// wall-clock deadline.
    ///
    /// # Errors
    /// Protocol errors propagate; a timeout returns `Ok(false)`.
    pub fn wait_until(
        &mut self,
        pred: impl Fn(&[RecordedEvent]) -> bool,
        timeout: Duration,
    ) -> Result<bool> {
        let deadline = Instant::now() + timeout;
        loop {
            if pred(&self.events.records) {
                return Ok(true);
            }
            if deadline <= Instant::now() {
                return Ok(false);
            }
            self.roundtrip()?;
        }
    }

    /// Every record of one event name.
    #[must_use]
    pub fn events_of(&self, name: &str) -> Vec<&RecordedEvent> {
        self.events
            .records
            .iter()
            .filter(|r| r.event == name)
            .collect()
    }

    /// The latest record of one event name.
    #[must_use]
    pub fn last_event(&self, name: &str) -> Option<&RecordedEvent> {
        self.events.records.iter().rev().find(|r| r.event == name)
    }

    /// All collected records (tracer summary, test assertions).
    #[must_use]
    pub fn records(&self) -> &[RecordedEvent] {
        &self.events.records
    }

    /// Bind an advertised global (cached); waits for the `bound`
    /// confirmation.
    ///
    /// # Errors
    /// [`ToolError::Logic`] when the interface is not advertised (the
    /// local guard: binding an unadvertised global is a fatal protocol
    /// error by design, and tools degrade honestly instead).
    pub fn bind(&mut self, interface: &str) -> Result<Proxy> {
        if let Some(proxy) = self.bound.get(interface) {
            return Ok(proxy.clone());
        }
        if !self.has_global(interface) {
            return Err(ToolError::Logic(format!(
                "interface '{interface}' is not advertised by this server \
                 (advertised: {})",
                self.globals
                    .iter()
                    .map(|g| g.interface.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        let proxy = self.conn.bind(interface)?;
        self.wait_until(
            |records| {
                records
                    .iter()
                    .any(|r| r.event == "bound" && r.str_arg("interface") == Some(interface))
            },
            WAIT,
        )?;
        self.bound.insert(interface.to_owned(), proxy.clone());
        Ok(proxy)
    }

    /// An `array<rect>` argument value.
    #[must_use]
    pub fn rect_arg(rects: &[Rect]) -> Value {
        Value::array(
            ArgType::Rect,
            rects
                .iter()
                .map(|r| Primitive::Rect(*r))
                .collect::<Vec<_>>(),
        )
        .unwrap_or(Value::Array {
            element: ArgType::Rect,
            items: Box::new([]),
        })
    }

    /// Create an shm pool of `size` bytes (zero-filled) with a
    /// client-side writable handle.
    ///
    /// # Errors
    /// memfd creation or protocol errors.
    pub fn create_pool(&mut self, size: u64) -> Result<ShmPool> {
        let shm = self.bind("ldp.core.shm")?;
        let fd = sys::memfd("ldp-tools-pool")?;
        let writer = std::fs::File::from(fd.try_clone()?);
        let mut fds = FdList::new();
        fds.push(fd);
        let proxy = self.conn.create_object_fd(
            &shm,
            "create_pool",
            vec![
                Value::Fd(0),
                Value::Int64(i64::try_from(size).unwrap_or(i64::MAX)),
            ],
            &mut fds,
        )?;
        self.roundtrip()?;
        Ok(ShmPool {
            proxy,
            file: writer,
            size,
        })
    }

    /// Create an shm pool pre-filled with one byte value.
    ///
    /// # Errors
    /// See [`create_pool`](Self::create_pool).
    pub fn create_pool_filled(&mut self, size: u64, fill: u8) -> Result<ShmPool> {
        let shm = self.bind("ldp.core.shm")?;
        let fd = sys::filled_memfd(
            "ldp-tools-pool",
            &vec![fill; usize::try_from(size).unwrap_or(0)],
        )?;
        let writer = std::fs::File::from(fd.try_clone()?);
        let mut fds = FdList::new();
        fds.push(fd);
        let proxy = self.conn.create_object_fd(
            &shm,
            "create_pool",
            vec![
                Value::Fd(0),
                Value::Int64(i64::try_from(size).unwrap_or(i64::MAX)),
            ],
            &mut fds,
        )?;
        self.roundtrip()?;
        Ok(ShmPool {
            proxy,
            file: writer,
            size,
        })
    }

    /// `shm_pool.create_buffer(offset, w, h, stride, format)`.
    ///
    /// # Errors
    /// Protocol errors (a stride/format violation is fatal by design).
    pub fn create_buffer(
        &mut self,
        pool: &Proxy,
        offset: i32,
        width: i32,
        height: i32,
        stride: i32,
        format: u32,
    ) -> Result<Proxy> {
        Ok(self.conn.create_object(
            pool,
            "create_buffer",
            vec![
                Value::Int32(offset),
                Value::Int32(width),
                Value::Int32(height),
                Value::Int32(stride),
                Value::Uint32(format),
            ],
        )?)
    }

    /// A surface with `n_buffers` ping-pong XRGB8888 buffers of
    /// `width`×`height`, pools pre-filled with `fill`.
    ///
    /// # Errors
    /// See [`create_pool_filled`](Self::create_pool_filled) and
    /// [`create_buffer`](Self::create_buffer).
    pub fn setup_surface(
        &mut self,
        width: u32,
        height: u32,
        n_buffers: usize,
        fill: u8,
    ) -> Result<FrameCycle> {
        self.setup_surface_format(width, height, n_buffers, fill, FORMAT_XRGB8888)
    }

    /// A surface with an explicit buffer format (the translucent
    /// ARGB8888 family for glass panels; XRGB8888 for content).
    ///
    /// # Errors
    /// See [`create_pool_filled`](Self::create_pool_filled) and
    /// [`create_buffer`](Self::create_buffer).
    pub fn setup_surface_format(
        &mut self,
        width: u32,
        height: u32,
        n_buffers: usize,
        fill: u8,
        format: u32,
    ) -> Result<FrameCycle> {
        let stride = FrameCycle::stride(width);
        // The pool spans every buffer: ping-pong needs the spare
        // pages while the first buffer rides in scanout.
        let bytes = u64::from(stride) * u64::from(height) * n_buffers as u64;
        let pool = self.create_pool_filled(bytes, fill)?;
        let mut buffers = Vec::with_capacity(n_buffers);
        for i in 0..n_buffers {
            let offset =
                i32::try_from(u64::from(stride) * u64::from(height) * i as u64).unwrap_or(i32::MAX);
            let buffer = self.create_buffer(
                &pool.proxy,
                offset,
                i32::try_from(width).unwrap_or(i32::MAX),
                i32::try_from(height).unwrap_or(i32::MAX),
                i32::try_from(stride).unwrap_or(i32::MAX),
                format,
            )?;
            self.roundtrip()?;
            buffers.push(buffer);
        }
        let compositor = self.bind("ldp.core.compositor")?;
        let surface = self
            .conn
            .create_object(&compositor, "create_surface", vec![])?;
        self.roundtrip()?;
        Ok(FrameCycle {
            surface,
            pool,
            buffers,
            next: 0,
        })
    }

    /// `surface.frame(frame_id)` — register presentation feedback.
    ///
    /// # Errors
    /// Protocol errors.
    pub fn frame(&mut self, surface: &Proxy, frame_id: u64) -> Result<()> {
        self.conn
            .send_request(surface, "frame", vec![Value::Uint64(frame_id)])?;
        Ok(())
    }

    /// `surface.attach(buffer)`.
    ///
    /// # Errors
    /// Protocol errors.
    pub fn attach(&mut self, surface: &Proxy, buffer: &Proxy) -> Result<()> {
        self.conn
            .send_request(surface, "attach", vec![Value::Object(Some(buffer.id()))])?;
        Ok(())
    }

    /// `surface.damage(rects)`.
    ///
    /// # Errors
    /// Protocol errors.
    pub fn damage(&mut self, surface: &Proxy, rects: &[Rect]) -> Result<()> {
        self.conn
            .send_request(surface, "damage", vec![Self::rect_arg(rects)])?;
        Ok(())
    }

    /// `surface.commit(cookie)`.
    ///
    /// # Errors
    /// Protocol errors.
    pub fn commit(&mut self, surface: &Proxy, cookie: u32) -> Result<()> {
        self.conn
            .send_request(surface, "commit", vec![Value::Uint32(cookie)])?;
        Ok(())
    }

    /// One full frame cycle: register, attach the next buffer, damage,
    /// commit — and wait for the commit to go live.
    ///
    /// # Errors
    /// Protocol or timeout failures (timeout leaves the cycle
    /// half-latched; callers treat that as an error).
    pub fn commit_frame(
        &mut self,
        cycle: &mut FrameCycle,
        frame_id: u64,
        damage: &[Rect],
    ) -> Result<()> {
        let surface = cycle.surface.clone();
        self.frame(&surface, frame_id)?;
        let cookie = u32::try_from(frame_id).unwrap_or(0);
        let buffer = cycle.next_buffer().clone();
        self.attach(&surface, &buffer)?;
        self.damage(&surface, damage)?;
        self.commit(&surface, cookie)?;
        let surface_id = surface.id().as_u32();
        let ok = self.wait_until(
            |records| {
                records.iter().any(|r| {
                    r.event == "committed"
                        && r.target == surface_id
                        && r.u32_arg("cookie") == Some(cookie)
                })
            },
            WAIT,
        )?;
        if !ok {
            return Err(ToolError::Logic(format!(
                "commit {cookie} never went live (no committed event)"
            )));
        }
        Ok(())
    }

    /// Wait for the presentation verdict of `frame_id` on `surface`.
    ///
    /// # Errors
    /// Protocol errors propagate; returns `None` on timeout.
    pub fn wait_presented(
        &mut self,
        surface: &Proxy,
        frame_id: u64,
        timeout: Duration,
    ) -> Result<Option<PresentedSample>> {
        let surface_id = surface.id().as_u32();
        let ok = self.wait_until(
            |records| {
                records.iter().any(|r| {
                    r.target == surface_id
                        && r.u64_arg("frame") == Some(frame_id)
                        && (r.event == "presented" || r.event == "frame_dropped")
                })
            },
            timeout,
        )?;
        if !ok {
            return Ok(None);
        }
        let found = self.events.records.iter().rev().find(|r| {
            r.target == surface_id
                && r.u64_arg("frame") == Some(frame_id)
                && (r.event == "presented" || r.event == "frame_dropped")
        });
        let Some(found) = found else {
            return Ok(None);
        };
        if found.event == "frame_dropped" {
            return Ok(None);
        }
        Ok(Some(PresentedSample {
            frame: frame_id,
            ts_ns: found.u64_arg("ts").unwrap_or(0),
            refresh_ns: found.u64_arg("refresh").unwrap_or(0),
            flags: match found.arg("flags") {
                Some(Value::Bitset(b)) => *b,
                _ => ldp_core::bitset::Bitset128::EMPTY,
            },
        }))
    }

    /// The `frame_target` contract for `frame_id`, if the server
    /// emitted one for it.
    #[must_use]
    pub fn frame_target(&self, surface: &Proxy, frame_id: u64) -> Option<FrameTargetSample> {
        let surface_id = surface.id().as_u32();
        self.events
            .records
            .iter()
            .find(|r| {
                r.event == "frame_target"
                    && r.target == surface_id
                    && r.u64_arg("frame") == Some(frame_id)
            })
            .map(|r| FrameTargetSample {
                frame: frame_id,
                target_ns: r.u64_arg("target").unwrap_or(0),
                refresh_ns: r.u64_arg("refresh").unwrap_or(0),
                budget_ns: r.u64_arg("budget").unwrap_or(0),
                mode: r.u32_arg("mode").unwrap_or(0),
            })
    }

    /// `registry.introspect(interface)` — the schema events, as
    /// `(interface name, JSON payload)` pairs. The empty name streams
    /// the whole protocol.
    ///
    /// # Errors
    /// Protocol errors (a denial is fatal: the connection requested
    /// the introspection option at hello).
    pub fn introspect(&mut self, interface: &str) -> Result<Vec<(String, String)>> {
        let registry = self.registry.clone().ok_or_else(|| {
            ToolError::Logic("introspect before bootstrap (no registry proxy)".to_owned())
        })?;
        let before = self.events.records.len();
        self.conn.send_request(
            &registry,
            "introspect",
            vec![Value::String(interface.to_owned().into())],
        )?;
        let expected: usize = if interface.is_empty() {
            ldp_protocol::MODULES
                .iter()
                .map(|m| m.interfaces.len())
                .sum()
        } else {
            1
        };
        let registry_id = registry.id().as_u32();
        let ok = self.wait_until(
            |records| {
                records[before.min(records.len())..]
                    .iter()
                    .filter(|r| r.event == "schema" && r.target == registry_id)
                    .count()
                    >= expected
            },
            WAIT,
        )?;
        if !ok {
            return Err(ToolError::Logic(format!(
                "introspect('{interface}'): expected {expected} schema events, timed out"
            )));
        }
        let pairs = self.events.records[before..]
            .iter()
            .filter(|r| r.event == "schema" && r.target == registry_id)
            .filter_map(|r| {
                Some((
                    r.arg("interface").and_then(|v| match v {
                        Value::String(s) => Some(s.to_string()),
                        _ => None,
                    })?,
                    r.arg("json").and_then(|v| match v {
                        Value::String(s) => Some(s.to_string()),
                        _ => None,
                    })?,
                ))
            })
            .collect();
        Ok(pairs)
    }

    /// Destroy a proxy (`connection.destroy`), draining the cookie.
    ///
    /// # Errors
    /// Protocol errors.
    pub fn destroy(&mut self, proxy: &Proxy) -> Result<()> {
        self.conn.destroy(proxy)?;
        Ok(())
    }

    /// The raw connection (escape hatch for tool-specific requests).
    pub fn connection(&mut self) -> &mut Connection {
        &mut self.conn
    }
}
