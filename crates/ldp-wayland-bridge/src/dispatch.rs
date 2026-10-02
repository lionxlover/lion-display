//! The compositor-side connection state machine.
//!
//! A [`Client`] is one connected Wayland client: its object table,
//! the registry replay, the request dispatch across the whole subset,
//! and the event stream back. The process binary owns the Unix socket
//! and the ancillary fd channel; the crate stays pure — pools are
//! read through the [`Host`] seam and the keymap blob arrives the
//! same way.
//!
//! Doctrine (documented, pinned by tests):
//!
//! * **Protocol violations are fatal.** Every malformed request
//!   queues exactly one `wl_display.error` and kills the connection
//!   (the process closes the socket). Legal-but-wrong requests that
//!   Wayland answers with interface-specific errors get those errors
//!   instead where the spec defines them.
//! * **Ids are single-use.** A client id may not collide, and a
//!   destroyed client id frees only after the server's
//!   `wl_display.delete_id` confirms it.
//! * **Serials are monotonic.** One counter feeds sync callbacks,
//!   configures, and every input event — the seat serial discipline.
//! * **Version pinning.** `bind` with any version other than the
//!   advertised one errors; the bridge speaks exactly one dialect.

#![forbid(unsafe_code)]
// Geometry plumbing restates the protocol's single-character field
// vocabulary (x, y, w, h); the module is flat protocol state.
#![allow(clippy::many_single_char_names)]

use std::collections::BTreeMap;

use ldp_core::geometry::Rect;

use crate::protocol::{self, Interface};
use crate::state::{
    formats, Anchor, Popup, Positioner, Role, ShmPool, Surface, Toplevel, WlBuffer, XdgPhase,
    XdgSurface,
};
use crate::wire::{self, Arg, DecodeStatus, Message, Value};

/// Process-binary seam: pool memory and the seat keymap.
pub trait Host {
    /// Read `len` bytes at `offset` of pool `pool` (the segment the
    /// process mapped for that wl_shm_pool); `None` when out of
    /// bounds.
    fn read_pool(&mut self, pool: u32, offset: usize, len: usize) -> Option<Vec<u8>>;
    /// The ancillary fd index for the keyboard keymap event.
    fn keymap_fd(&mut self) -> u32;
    /// The keymap blob (xkb v1 text).
    fn keymap(&mut self) -> Vec<u8>;
    /// A `wl_shm.create_pool` arrived: adopt the descriptor at
    /// ancillary index `fd_index` (of the same feed batch) as pool
    /// `pool` of `size` bytes. The default ignores it — tests that
    /// pre-populate pools never see one.
    fn create_pool(&mut self, pool: u32, fd_index: u32, size: u64) {
        let _ = (pool, fd_index, size);
    }
}

/// A host that owns nothing (tests that never map buffers).
#[derive(Clone, Copy, Debug, Default)]
pub struct NoHost;

impl Host for NoHost {
    fn read_pool(&mut self, _pool: u32, _offset: usize, _len: usize) -> Option<Vec<u8>> {
        None
    }
    fn keymap_fd(&mut self) -> u32 {
        0
    }
    fn keymap(&mut self) -> Vec<u8> {
        Vec::new()
    }
}

/// The `wl_display` error codes the subset emits.
pub mod display_error {
    /// The object id is unknown or has the wrong type.
    pub const INVALID_OBJECT: u32 = 0;
    /// The request is well-formed but semantically wrong.
    pub const INVALID_ARGUMENT: u32 = 1;
    /// The bridge cannot satisfy the request.
    pub const IMPLEMENTATION: u32 = 3;
}

/// One client-side object.
#[derive(Clone, Debug)]
enum Obj {
    /// The bootstrap `wl_display` (id 1).
    Display,
    /// The registry.
    Registry,
    /// A pending sync/frame callback.
    Callback,
    /// The compositor global.
    Compositor,
    /// The shm global.
    Shm,
    /// The seat global.
    Seat,
    /// The shell global.
    WmBase,
    /// A shm pool.
    ShmPool(ShmPool),
    /// A buffer.
    Buffer(WlBuffer),
    /// A surface.
    Surface(Surface),
    /// A region.
    Region(Vec<Rect>),
    /// A pointer device.
    Pointer { focus: u32 },
    /// A keyboard device.
    Keyboard { focus: u32 },
    /// A touch device.
    Touch,
    /// An xdg_positioner.
    Positioner(Positioner),
    /// An xdg_surface.
    XdgSurface(XdgSurface),
    /// An xdg_toplevel.
    Toplevel(Toplevel),
    /// An xdg_popup.
    Popup(Popup),
}

impl Obj {
    /// The interface this object speaks.
    #[must_use]
    fn iface(&self) -> &'static Interface {
        match self {
            Obj::Display => &protocol::WL_DISPLAY,
            Obj::Registry => &protocol::WL_REGISTRY,
            Obj::Callback => &protocol::WL_CALLBACK,
            Obj::Compositor => &protocol::WL_COMPOSITOR,
            Obj::Shm => &protocol::WL_SHM,
            Obj::Seat => &protocol::WL_SEAT,
            Obj::WmBase => &protocol::XDG_WM_BASE,
            Obj::ShmPool(_) => &protocol::WL_SHM_POOL,
            Obj::Buffer(_) => &protocol::WL_BUFFER,
            Obj::Surface(_) => &protocol::WL_SURFACE,
            Obj::Region(_) => &protocol::WL_REGION,
            Obj::Pointer { .. } => &protocol::WL_POINTER,
            Obj::Keyboard { .. } => &protocol::WL_KEYBOARD,
            Obj::Touch => &protocol::WL_TOUCH,
            Obj::Positioner(_) => &protocol::XDG_POSITIONER,
            Obj::XdgSurface(_) => &protocol::XDG_SURFACE,
            Obj::Toplevel(_) => &protocol::XDG_TOPLEVEL,
            Obj::Popup(_) => &protocol::XDG_POPUP,
        }
    }
}

/// Why the connection was torn down.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Fatal {
    /// The `wl_display.error` code sent to the client.
    pub code: u32,
    /// The object the error names.
    pub object: u32,
    /// The diagnostic (also rides the error event).
    pub message: String,
}

/// One connected Wayland client.
pub struct Client {
    objects: BTreeMap<u32, Obj>,
    inbuf: Vec<u8>,
    /// Encoded events + their ancillary fd counts, in order.
    out: Vec<(Vec<u8>, u32)>,
    /// The monotonic serial feeding callbacks, configures, input.
    serial: u32,
    /// The last wm_base ping serial awaiting a pong.
    ping: Option<u32>,
    host: Box<dyn Host>,
    /// The teardown reason, if the client died.
    fatal: Option<Fatal>,
    /// Client-destroyed ids not yet confirmed with delete_id.
    graveyard: Vec<u32>,
    /// The pointer's last surface-space position (for enter coords).
    pointer_pos: (i32, i32),
    /// Time injected with input (the driver's clock word).
    time: u32,
    /// Keyboards that already got their keymap (send-once).
    keymap_state: std::collections::BTreeSet<u32>,
}

impl Client {
    /// A fresh client (the `wl_display` object pre-bound at id 1).
    pub fn new(host: Box<dyn Host>) -> Client {
        let mut objects = BTreeMap::new();
        objects.insert(1, Obj::Display);
        Client {
            objects,
            inbuf: Vec::new(),
            out: Vec::new(),
            serial: 0,
            ping: None,
            host,
            fatal: None,
            graveyard: Vec::new(),
            pointer_pos: (0, 0),
            time: 0,
            keymap_state: std::collections::BTreeSet::new(),
        }
    }

    /// Feed raw socket bytes.
    ///
    /// # Errors
    /// The fatal `wl_display.error` reason: the process must close
    /// the connection (the error event is already queued).
    ///
    /// # Panics
    /// Never: every `unreachable!` sits behind a schema-validated
    /// argument pattern the decoder just produced.
    #[allow(clippy::too_many_lines)] // one flat routing table over the subset
    pub fn feed(&mut self, bytes: &[u8]) -> Result<(), Fatal> {
        if self.fatal.is_some() {
            return Err(self.fatal.clone().unwrap());
        }
        // Total-buffer ceiling (defense in depth): a stream that
        // keeps growing the input buffer without ever completing a
        // message is a length bomb — the per-message check below
        // catches the known shapes, this guard catches any future
        // schema or decoder drift. Same doctrine as the X11 bridge's
        // `max_request_bytes()`: a foreign message that could never
        // fit an LDP large frame can never be forwarded whole.
        let ceiling = wire::max_message_bytes() as usize;
        if self.inbuf.len().saturating_add(bytes.len()) > ceiling {
            self.kill(
                display_error::INVALID_ARGUMENT,
                1,
                "request stream exceeds the bridge message ceiling",
            );
            return Err(self.fatal.clone().unwrap());
        }
        self.inbuf.extend_from_slice(bytes);
        loop {
            if self.inbuf.len() < 4 {
                return Ok(());
            }
            let header =
                u32::from_le_bytes([self.inbuf[0], self.inbuf[1], self.inbuf[2], self.inbuf[3]]);
            let object_id = header >> 8;
            let opcode = header & 0xff;
            let Some(obj) = self.objects.get(&object_id) else {
                self.kill(display_error::INVALID_OBJECT, object_id, "unknown object");
                return Err(self.fatal.clone().unwrap());
            };
            let iface = obj.iface();
            let Some(schema) = protocol::request(iface, opcode) else {
                self.kill(
                    display_error::INVALID_OBJECT,
                    object_id,
                    "no such request on interface",
                );
                return Err(self.fatal.clone().unwrap());
            };
            let args_schema: &'static [Arg] = schema.args;
            match wire::decode(&self.inbuf, object_id, opcode, args_schema) {
                Ok((msg, used, _fds)) => {
                    self.inbuf.drain(..used);
                    self.dispatch(object_id, opcode, &msg)?;
                    if let Some(f) = &self.fatal {
                        return Err(f.clone());
                    }
                }
                Err(DecodeStatus::Incomplete(need)) => {
                    // A message that can never fit the bridge ceiling
                    // is rejected now, not buffered forever: a
                    // declared string/array length past the ceiling
                    // is a length bomb (surfaced by the Phase 19
                    // fuzzer).
                    if need > ceiling {
                        self.kill(
                            display_error::INVALID_ARGUMENT,
                            object_id,
                            "request length exceeds the bridge message ceiling",
                        );
                        return Err(self.fatal.clone().unwrap());
                    }
                    return Ok(());
                }
                Err(DecodeStatus::Malformed(at)) => {
                    let at = at.min(self.inbuf.len());
                    let _ = at;
                    self.kill(
                        display_error::INVALID_ARGUMENT,
                        object_id,
                        "malformed request",
                    );
                    return Err(self.fatal.clone().unwrap());
                }
            }
        }
    }

    /// Take the pending event stream: concatenated wire bytes and the
    /// total ancillary fd count.
    pub fn take_output(&mut self) -> (Vec<u8>, u32) {
        let mut bytes = Vec::new();
        let mut fds = 0u32;
        for (b, f) in std::mem::take(&mut self.out) {
            bytes.extend_from_slice(&b);
            fds += f;
        }
        (bytes, fds)
    }

    /// The client's current teardown state.
    #[must_use]
    pub fn fatal(&self) -> Option<&Fatal> {
        self.fatal.as_ref()
    }

    /// Queue an event on an object (encoding now; failures are
    /// programmer errors, caught by the schema check).
    fn emit(&mut self, object_id: u32, opcode: u32, args: Vec<Value>) {
        let schema = {
            let Some(obj) = self.objects.get(&object_id) else {
                return;
            };
            protocol::event(obj.iface(), opcode)
        };
        let Some(schema) = schema else {
            return;
        };
        let msg = Message {
            object_id,
            opcode,
            args,
        };
        let (bytes, fds) = wire::encode(&msg, schema.args);
        self.out.push((bytes, fds));
    }

    /// Queue a bare event by interface (for objects the table may not
    /// hold, e.g. after teardown) — unused in the subset but keeps
    /// `emit` honest for delete_id.
    fn emit_raw(
        &mut self,
        iface: &'static Interface,
        object_id: u32,
        opcode: u32,
        args: Vec<Value>,
    ) {
        let Some(schema) = protocol::event(iface, opcode) else {
            return;
        };
        let msg = Message {
            object_id,
            opcode,
            args,
        };
        let (bytes, fds) = wire::encode(&msg, schema.args);
        self.out.push((bytes, fds));
    }

    /// Kill the connection with one `wl_display.error` (opcode 0:
    /// object, code, message).
    fn kill(&mut self, code: u32, object: u32, message: &str) {
        self.emit_raw(
            &protocol::WL_DISPLAY,
            1,
            0,
            vec![
                Value::Object(object),
                Value::Uint(code),
                Value::String(message.into()),
            ],
        );
        self.fatal = Some(Fatal {
            code,
            object,
            message: message.to_string(),
        });
    }

    /// Bump and return the serial.
    fn next_serial(&mut self) -> u32 {
        self.serial = self.serial.wrapping_add(1);
        self.serial
    }

    // ---- object lifecycle ----

    /// Insert a client-created object; id reuse before delete_id (or
    /// plain collision) is fatal.
    fn create(&mut self, id: u32, obj: Obj) -> Result<(), Fatal> {
        if id == 0 || id > wire::MAX_OBJECT_ID {
            self.kill(display_error::INVALID_OBJECT, id, "invalid object id");
            return Err(self.fatal.clone().unwrap());
        }
        if self.objects.contains_key(&id) {
            self.kill(
                display_error::INVALID_OBJECT,
                id,
                "object id already in use",
            );
            return Err(self.fatal.clone().unwrap());
        }
        if self.graveyard.contains(&id) {
            self.kill(
                display_error::INVALID_OBJECT,
                id,
                "object id reused before delete_id",
            );
            return Err(self.fatal.clone().unwrap());
        }
        self.objects.insert(id, obj);
        Ok(())
    }

    /// A client-destroyed object: remove it and confirm with
    /// `delete_id` (the id frees for reuse only after this).
    fn destroy(&mut self, id: u32) {
        if self.objects.remove(&id).is_some() {
            self.graveyard.push(id);
            self.emit_raw(&protocol::WL_DISPLAY, 1, 1, vec![Value::Uint(id)]);
        }
    }

    /// A server-side destroy (no delete_id — the server chose it).
    fn server_destroy(&mut self, id: u32) {
        self.objects.remove(&id);
    }

    // ---- request dispatch ----

    /// Route one decoded request. One flat table over the subset —
    /// the length is the vocabulary's price.
    #[allow(clippy::too_many_lines)]
    fn dispatch(&mut self, object_id: u32, opcode: u32, msg: &Message) -> Result<(), Fatal> {
        match opcode_mnemonic(object_id, opcode, &self.objects) {
            Mn::DisplaySync => {
                let Value::NewId(cb) = msg.args[0] else {
                    unreachable!("schema-validated")
                };
                self.create(cb, Obj::Callback)?;
                let serial = self.next_serial();
                self.emit(cb, 0, vec![Value::Uint(serial)]);
                self.server_destroy(cb);
                self.emit_raw(&protocol::WL_DISPLAY, 1, 1, vec![Value::Uint(cb)]);
                self.graveyard.retain(|&g| g != cb);
            }
            Mn::DisplayGetRegistry => {
                let Value::NewId(registry) = msg.args[0] else {
                    unreachable!()
                };
                self.create(registry, Obj::Registry)?;
                for (i, iface) in protocol::ALL_GLOBALS.iter().enumerate() {
                    let name = i as u32 + 1;
                    self.emit(
                        registry,
                        0,
                        vec![
                            Value::Uint(name),
                            Value::String(iface.name.into()),
                            Value::Uint(iface.version),
                        ],
                    );
                }
            }
            Mn::RegistryBind => {
                let Value::Uint(name) = msg.args[0] else {
                    unreachable!()
                };
                let Value::String(interface) = msg.args[1].clone() else {
                    unreachable!()
                };
                let Value::Uint(version) = msg.args[2] else {
                    unreachable!()
                };
                let Value::NewId(id) = msg.args[3] else {
                    unreachable!()
                };
                let idx = usize::try_from(name).ok().and_then(|n| n.checked_sub(1));
                let Some(iface) = idx.and_then(|i| protocol::ALL_GLOBALS.get(i).copied()) else {
                    self.kill(display_error::INVALID_OBJECT, object_id, "no such global");
                    return Err(self.fatal.clone().unwrap());
                };
                if interface.as_ref() != iface.name {
                    self.kill(
                        display_error::INVALID_OBJECT,
                        object_id,
                        "global is not that interface",
                    );
                    return Err(self.fatal.clone().unwrap());
                }
                if version != iface.version {
                    self.kill(
                        display_error::INVALID_ARGUMENT,
                        object_id,
                        "version must equal the advertised one",
                    );
                    return Err(self.fatal.clone().unwrap());
                }
                let obj = match iface.name {
                    "wl_compositor" => Obj::Compositor,
                    "wl_shm" => Obj::Shm,
                    "wl_seat" => Obj::Seat,
                    "xdg_wm_base" => Obj::WmBase,
                    _ => unreachable!(),
                };
                self.create(id, obj)?;
                if iface.name == "wl_shm" {
                    for f in formats::ACCEPTED {
                        self.emit(id, 0, vec![Value::Uint(*f)]);
                    }
                }
                if iface.name == "wl_seat" {
                    // pointer + keyboard + touch capabilities.
                    self.emit(id, 0, vec![Value::Uint(7)]);
                    self.emit(id, 1, vec![Value::String("seat0".into())]);
                }
            }
            Mn::CompositorCreateSurface => {
                let Value::NewId(id) = msg.args[0] else {
                    unreachable!()
                };
                self.create(id, Obj::Surface(Surface::default()))?;
            }
            Mn::CompositorCreateRegion => {
                let Value::NewId(id) = msg.args[0] else {
                    unreachable!()
                };
                self.create(id, Obj::Region(Vec::new()))?;
            }
            // The plain destroy family: one semantics for every
            // interface (role stacks have their own teardown paths).
            Mn::RegionDestroy
            | Mn::SurfaceDestroy
            | Mn::PoolDestroy
            | Mn::BufferDestroy
            | Mn::PositionerDestroy
            | Mn::SeatRelease
            | Mn::PointerRelease
            | Mn::KeyboardRelease
            | Mn::TouchRelease
            | Mn::WmBaseDestroy => {
                self.destroy(object_id);
            }
            Mn::RegionAdd | Mn::RegionSubtract => {
                let (x, y, w, h) = int4(&msg.args);
                let Some(Obj::Region(rects)) = self.objects.get_mut(&object_id) else {
                    unreachable!()
                };
                let r = Rect::new(x, y, w.unsigned_abs(), h.unsigned_abs());
                let subtract = opcode == 1;
                if subtract {
                    let mut next = Vec::new();
                    for existing in rects.iter() {
                        for piece in &existing.subtract(r) {
                            next.push(*piece);
                        }
                    }
                    *rects = next;
                } else {
                    rects.push(r);
                }
            }
            Mn::ShmCreatePool => {
                let Value::Fd(fd_index) = msg.args[0] else {
                    unreachable!()
                };
                let Value::Int(size) = msg.args[1] else {
                    unreachable!()
                };
                let Value::NewId(id) = msg.args[2] else {
                    unreachable!()
                };
                if size < 0 {
                    self.kill(
                        display_error::INVALID_ARGUMENT,
                        object_id,
                        "negative pool size",
                    );
                    return Err(self.fatal.clone().unwrap());
                }
                // The process layer adopts the descriptor now (the
                // index addresses this batch's ancillary array).
                self.host.create_pool(id, fd_index, size as u64);
                self.create(id, Obj::ShmPool(ShmPool { size, alive: true }))?;
            }
            Mn::PoolCreateBuffer => {
                let Value::NewId(id) = msg.args[0] else {
                    unreachable!()
                };
                let Value::Int(offset) = msg.args[1] else {
                    unreachable!()
                };
                let (w, h) = (int_at(&msg.args, 2), int_at(&msg.args, 3));
                let Value::Int(stride) = msg.args[4] else {
                    unreachable!()
                };
                let Value::Uint(format) = msg.args[5] else {
                    unreachable!()
                };
                let pool_size = match self.objects.get(&object_id) {
                    Some(Obj::ShmPool(p)) if p.alive => p.size,
                    _ => {
                        self.kill(display_error::INVALID_OBJECT, object_id, "pool is dead");
                        return Err(self.fatal.clone().unwrap());
                    }
                };
                if !formats::ACCEPTED.contains(&format) {
                    self.kill(display_error::INVALID_ARGUMENT, object_id, "unknown format");
                    return Err(self.fatal.clone().unwrap());
                }
                if w <= 0 || h <= 0 || stride < w * 4 {
                    self.kill(
                        display_error::INVALID_ARGUMENT,
                        object_id,
                        "bad buffer geometry",
                    );
                    return Err(self.fatal.clone().unwrap());
                }
                let end = i64::from(offset) + i64::from(stride) * i64::from(h);
                if end > i64::from(pool_size) || offset < 0 {
                    self.kill(
                        display_error::INVALID_ARGUMENT,
                        object_id,
                        "buffer overruns the pool",
                    );
                    return Err(self.fatal.clone().unwrap());
                }
                self.create(
                    id,
                    Obj::Buffer(WlBuffer {
                        pool: object_id,
                        offset,
                        width: w,
                        height: h,
                        stride,
                        format,
                    }),
                )?;
            }
            Mn::PoolResize => {
                let Value::Int(size) = msg.args[0] else {
                    unreachable!()
                };
                let Some(Obj::ShmPool(p)) = self.objects.get_mut(&object_id) else {
                    unreachable!()
                };
                if size < p.size {
                    self.kill(
                        display_error::INVALID_ARGUMENT,
                        object_id,
                        "pools only grow",
                    );
                    return Err(self.fatal.clone().unwrap());
                }
                p.size = size;
            }
            Mn::SurfaceAttach => {
                let Value::Object(buffer) = msg.args[0] else {
                    unreachable!()
                };
                let (dx, dy) = (int_at(&msg.args, 1), int_at(&msg.args, 2));
                if buffer != 0 && !matches!(self.objects.get(&buffer), Some(Obj::Buffer(_))) {
                    self.kill(
                        display_error::INVALID_OBJECT,
                        object_id,
                        "attach to unknown buffer",
                    );
                    return Err(self.fatal.clone().unwrap());
                }
                let Some(Obj::Surface(s)) = self.objects.get_mut(&object_id) else {
                    unreachable!()
                };
                s.pending.buffer = buffer;
                s.pending.offset = (dx, dy);
            }
            Mn::SurfaceDamage | Mn::SurfaceDamageBuffer => {
                let (x, y, w, h) = int4(&msg.args);
                let Some(Obj::Surface(s)) = self.objects.get_mut(&object_id) else {
                    unreachable!()
                };
                s.pending
                    .damage
                    .push(Rect::new(x, y, w.unsigned_abs(), h.unsigned_abs()));
            }
            Mn::SurfaceFrame => {
                let Value::NewId(cb) = msg.args[0] else {
                    unreachable!()
                };
                self.create(cb, Obj::Callback)?;
                let Some(Obj::Surface(s)) = self.objects.get_mut(&object_id) else {
                    unreachable!()
                };
                s.pending.frame_callbacks.push(cb);
            }
            Mn::SurfaceSetOpaqueRegion | Mn::SurfaceSetInputRegion => {
                let Value::Object(region) = msg.args[0] else {
                    unreachable!()
                };
                let rects = if region == 0 {
                    Vec::new()
                } else {
                    let Some(Obj::Region(r)) = self.objects.get(&region) else {
                        self.kill(display_error::INVALID_OBJECT, object_id, "unknown region");
                        return Err(self.fatal.clone().unwrap());
                    };
                    r.clone()
                };
                let Some(Obj::Surface(s)) = self.objects.get_mut(&object_id) else {
                    unreachable!()
                };
                let slot = if opcode == 4 {
                    &mut s.pending.opaque_region
                } else {
                    &mut s.pending.input_region
                };
                *slot = Some(rects);
            }
            Mn::SurfaceCommit => self.surface_commit(object_id)?,
            Mn::SeatGetPointer => {
                let Value::NewId(id) = msg.args[0] else {
                    unreachable!()
                };
                self.create(id, Obj::Pointer { focus: 0 })?;
            }
            Mn::SeatGetKeyboard => {
                let Value::NewId(id) = msg.args[0] else {
                    unreachable!()
                };
                self.create(id, Obj::Keyboard { focus: 0 })?;
            }
            Mn::SeatGetTouch => {
                let Value::NewId(id) = msg.args[0] else {
                    unreachable!()
                };
                self.create(id, Obj::Touch)?;
            }
            Mn::PointerSetCursor => {
                let Value::Object(surface) = msg.args[1] else {
                    unreachable!()
                };
                if surface != 0 && !matches!(self.objects.get(&surface), Some(Obj::Surface(_))) {
                    self.kill(display_error::INVALID_OBJECT, object_id, "cursor surface");
                    return Err(self.fatal.clone().unwrap());
                }
                // The subset records the cursor; LDP cursor objects
                // are the driver's business.
            }
            Mn::WmBaseCreatePositioner => {
                let Value::NewId(id) = msg.args[0] else {
                    unreachable!()
                };
                self.create(id, Obj::Positioner(Positioner::default()))?;
            }
            Mn::WmBaseGetXdgSurface => {
                let Value::NewId(id) = msg.args[0] else {
                    unreachable!()
                };
                let Value::Object(surface) = msg.args[1] else {
                    unreachable!()
                };
                if !matches!(self.objects.get(&surface), Some(Obj::Surface(_))) {
                    self.kill(
                        display_error::INVALID_OBJECT,
                        object_id,
                        "xdg surface over unknown wl_surface",
                    );
                    return Err(self.fatal.clone().unwrap());
                }
                if matches!(
                    surface_role(&self.objects, surface),
                    Some(Role::Toplevel(_) | Role::Popup(_))
                ) {
                    self.kill(
                        display_error::INVALID_ARGUMENT,
                        object_id,
                        "surface already has a role",
                    );
                    return Err(self.fatal.clone().unwrap());
                }
                self.create(id, Obj::XdgSurface(XdgSurface::new(surface)))?;
            }
            Mn::WmBasePong => {
                let Value::Uint(serial) = msg.args[0] else {
                    unreachable!()
                };
                if self.ping != Some(serial) {
                    self.kill(
                        display_error::INVALID_ARGUMENT,
                        object_id,
                        "pong serial mismatch",
                    );
                    return Err(self.fatal.clone().unwrap());
                }
                self.ping = None;
            }
            Mn::PositionerSetSize => {
                let (w, h) = (int_at(&msg.args, 0), int_at(&msg.args, 1));
                let Some(Obj::Positioner(p)) = self.objects.get_mut(&object_id) else {
                    unreachable!()
                };
                if w <= 0 || h <= 0 {
                    self.kill(
                        display_error::INVALID_ARGUMENT,
                        object_id,
                        "positioner size",
                    );
                    return Err(self.fatal.clone().unwrap());
                }
                p.size = Some((w, h));
            }
            Mn::PositionerSetAnchorRect => {
                let (x, y, w, h) = int4(&msg.args);
                let Some(Obj::Positioner(p)) = self.objects.get_mut(&object_id) else {
                    unreachable!()
                };
                if w < 0 || h < 0 {
                    self.kill(display_error::INVALID_ARGUMENT, object_id, "anchor rect");
                    return Err(self.fatal.clone().unwrap());
                }
                p.anchor_rect = Rect::new(x, y, w.unsigned_abs(), h.unsigned_abs());
            }
            Mn::PositionerSetAnchor | Mn::PositionerSetGravity => {
                let Value::Uint(v) = msg.args[0] else {
                    unreachable!()
                };
                let a = Anchor::from_wire(v);
                if a == Anchor::Unset {
                    self.kill(display_error::INVALID_ARGUMENT, object_id, "anchor value");
                    return Err(self.fatal.clone().unwrap());
                }
                let Some(Obj::Positioner(p)) = self.objects.get_mut(&object_id) else {
                    unreachable!()
                };
                if opcode == 3 {
                    if a == Anchor::None {
                        self.kill(display_error::INVALID_ARGUMENT, object_id, "anchor none");
                        return Err(self.fatal.clone().unwrap());
                    }
                    p.anchor = a;
                } else {
                    p.gravity = a;
                }
            }
            Mn::PositionerSetConstraintAdjustment => {
                let Value::Uint(v) = msg.args[0] else {
                    unreachable!()
                };
                if v & !crate::state::constraints::MASK != 0 {
                    self.kill(
                        display_error::INVALID_ARGUMENT,
                        object_id,
                        "constraint bits",
                    );
                    return Err(self.fatal.clone().unwrap());
                }
                let Some(Obj::Positioner(p)) = self.objects.get_mut(&object_id) else {
                    unreachable!()
                };
                p.constraint_adjustment = v;
            }
            Mn::PositionerSetOffset => {
                let (dx, dy) = (int_at(&msg.args, 0), int_at(&msg.args, 1));
                let Some(Obj::Positioner(p)) = self.objects.get_mut(&object_id) else {
                    unreachable!()
                };
                p.offset = (dx, dy);
            }
            Mn::XdgSurfaceDestroy => {
                // Tearing the xdg_surface destroys the whole role
                // stack (toplevel/popup) and frees the surface.
                if let Some(Obj::XdgSurface(x)) = self.objects.get(&object_id).cloned() {
                    self.teardown_role(object_id, x.surface);
                }
            }
            Mn::XdgSurfaceGetToplevel => {
                let Value::NewId(id) = msg.args[0] else {
                    unreachable!()
                };
                let phase_ok = matches!(
                    self.objects.get(&object_id),
                    Some(Obj::XdgSurface(x)) if x.phase == XdgPhase::NoRole
                );
                if !phase_ok {
                    self.kill(
                        display_error::INVALID_ARGUMENT,
                        object_id,
                        "role already set",
                    );
                    return Err(self.fatal.clone().unwrap());
                }
                self.create(
                    id,
                    Obj::Toplevel(Toplevel {
                        xdg: object_id,
                        ..Toplevel::default()
                    }),
                )?;
                let surface = xdg_surface_of(&self.objects, object_id);
                if let Some(Obj::Surface(s)) = self.objects.get_mut(&surface) {
                    s.role = Role::Toplevel(object_id);
                }
                if let Some(Obj::XdgSurface(x)) = self.objects.get_mut(&object_id) {
                    x.phase = XdgPhase::RoleAssigned;
                }
                // The initial configure: size 0,0 (client chooses) and
                // the surface-level configure carrying the serial.
                let serial = self.next_serial();
                self.emit(
                    id,
                    0,
                    vec![
                        Value::Int(0),
                        Value::Int(0),
                        Value::Array(Vec::new().into_boxed_slice()),
                    ],
                );
                self.emit(object_id, 0, vec![Value::Uint(serial)]);
                if let Some(Obj::XdgSurface(x)) = self.objects.get_mut(&object_id) {
                    x.phase = XdgPhase::ConfigureSent(serial);
                    x.serial = serial;
                }
            }
            Mn::XdgSurfaceGetPopup => {
                let Value::NewId(id) = msg.args[0] else {
                    unreachable!()
                };
                let Value::Object(parent) = msg.args[1] else {
                    unreachable!()
                };
                let Value::Object(positioner) = msg.args[2] else {
                    unreachable!()
                };
                let phase_ok = matches!(
                    self.objects.get(&object_id),
                    Some(Obj::XdgSurface(x)) if x.phase == XdgPhase::NoRole
                );
                if !phase_ok {
                    self.kill(
                        display_error::INVALID_ARGUMENT,
                        object_id,
                        "role already set",
                    );
                    return Err(self.fatal.clone().unwrap());
                }
                if !matches!(self.objects.get(&parent), Some(Obj::XdgSurface(_))) {
                    self.kill(display_error::INVALID_OBJECT, object_id, "popup parent");
                    return Err(self.fatal.clone().unwrap());
                }
                let Some(Obj::Positioner(p)) = self.objects.get(&positioner) else {
                    self.kill(display_error::INVALID_OBJECT, object_id, "popup positioner");
                    return Err(self.fatal.clone().unwrap());
                };
                let pos = p.clone();
                if pos.size.is_none() {
                    self.kill(
                        display_error::INVALID_ARGUMENT,
                        object_id,
                        "positioner without size",
                    );
                    return Err(self.fatal.clone().unwrap());
                }
                self.create(
                    id,
                    Obj::Popup(Popup {
                        xdg: object_id,
                        parent,
                        positioner: pos.clone(),
                        grab_serial: 0,
                    }),
                )?;
                let surface = xdg_surface_of(&self.objects, object_id);
                if let Some(Obj::Surface(s)) = self.objects.get_mut(&surface) {
                    s.role = Role::Popup(object_id);
                }
                // Solve the initial position: the anchor point plus
                // the offset, honoring gravity (no constraint
                // solving — the driver re-solves through ldp-shell's
                // placement authority and re-configures on changes).
                let (pw, ph) = pos.size.unwrap_or((1, 1));
                let (x, y) = solve_position(&pos);
                let serial = self.next_serial();
                self.emit(
                    id,
                    0,
                    vec![Value::Int(x), Value::Int(y), Value::Int(pw), Value::Int(ph)],
                );
                self.emit(object_id, 0, vec![Value::Uint(serial)]);
                if let Some(Obj::XdgSurface(x)) = self.objects.get_mut(&object_id) {
                    x.phase = XdgPhase::ConfigureSent(serial);
                    x.serial = serial;
                }
            }
            Mn::XdgSurfaceSetWindowGeometry => {
                let (x, y, w, h) = int4(&msg.args);
                if w < 0 || h < 0 {
                    self.kill(display_error::INVALID_ARGUMENT, object_id, "geometry");
                    return Err(self.fatal.clone().unwrap());
                }
                let Some(Obj::XdgSurface(xdg)) = self.objects.get_mut(&object_id) else {
                    unreachable!()
                };
                xdg.window_geometry = Some(Rect::new(x, y, w.unsigned_abs(), h.unsigned_abs()));
            }
            Mn::XdgSurfaceAckConfigure => {
                let Value::Uint(serial) = msg.args[0] else {
                    unreachable!()
                };
                let phase = match self.objects.get(&object_id) {
                    Some(Obj::XdgSurface(x)) => (x.phase, x.serial),
                    _ => unreachable!(),
                };
                match phase.0 {
                    XdgPhase::ConfigureSent(s) if s == serial => {
                        if let Some(Obj::XdgSurface(x)) = self.objects.get_mut(&object_id) {
                            x.phase = XdgPhase::Acked(serial);
                        }
                    }
                    XdgPhase::Acked(_) | XdgPhase::Mapped => {
                        // Duplicate acks are tolerated (the serial must
                        // still match the last one sent).
                        if serial != phase.1 {
                            self.kill(
                                display_error::INVALID_ARGUMENT,
                                object_id,
                                "ack serial mismatch",
                            );
                            return Err(self.fatal.clone().unwrap());
                        }
                    }
                    _ => {
                        self.kill(
                            display_error::INVALID_ARGUMENT,
                            object_id,
                            "ack without configure",
                        );
                        return Err(self.fatal.clone().unwrap());
                    }
                }
            }
            Mn::ToplevelSetTitle => {
                let Value::String(title) = msg.args[0].clone() else {
                    unreachable!()
                };
                if let Some(Obj::Toplevel(t)) = self.objects.get_mut(&object_id) {
                    t.title = Some(title);
                }
            }
            Mn::ToplevelSetAppId => {
                let Value::String(app) = msg.args[0].clone() else {
                    unreachable!()
                };
                if let Some(Obj::Toplevel(t)) = self.objects.get_mut(&object_id) {
                    t.app_id = Some(app);
                }
            }
            // The interactive belt (parent, window menu, move,
            // resize, state toggles): the subset records nothing —
            // LDP routes those through its own shell gestures.
            Mn::ToplevelSetParent
            | Mn::ToplevelShowWindowMenu
            | Mn::ToplevelMove
            | Mn::ToplevelResize
            | Mn::ToplevelStateRequests => {}
            Mn::ToplevelSetMaxSize | Mn::ToplevelSetMinSize => {
                let (w, h) = (int_at(&msg.args, 0), int_at(&msg.args, 1));
                if let Some(Obj::Toplevel(t)) = self.objects.get_mut(&object_id) {
                    if opcode == 7 {
                        t.max_size = (w, h);
                    } else {
                        t.min_size = (w, h);
                    }
                }
            }
            Mn::ToplevelDestroy => {
                if let Some(Obj::Toplevel(_)) = self.objects.get(&object_id) {
                    let xdg = toplevel_owner(&self.objects, object_id).unwrap_or(0);
                    if xdg != 0 {
                        if let Some(Obj::XdgSurface(x)) = self.objects.get(&xdg).cloned() {
                            self.teardown_role(xdg, x.surface);
                            return Ok(());
                        }
                    }
                    self.destroy(object_id);
                }
            }
            Mn::PopupGrab => {
                let Value::Object(_seat) = msg.args[0] else {
                    unreachable!()
                };
                let Value::Uint(serial) = msg.args[1] else {
                    unreachable!()
                };
                let Some(Obj::Popup(p)) = self.objects.get_mut(&object_id) else {
                    unreachable!()
                };
                p.grab_serial = serial;
            }
            Mn::PopupReposition => {
                let Value::Object(positioner) = msg.args[0] else {
                    unreachable!()
                };
                let Value::Uint(token) = msg.args[1] else {
                    unreachable!()
                };
                let Some(Obj::Positioner(p)) = self.objects.get(&positioner) else {
                    self.kill(display_error::INVALID_OBJECT, object_id, "reposition");
                    return Err(self.fatal.clone().unwrap());
                };
                let pos = p.clone();
                let (x, y) = solve_position(&pos);
                let Some(Obj::Popup(p)) = self.objects.get_mut(&object_id) else {
                    unreachable!()
                };
                p.positioner = pos;
                self.emit(object_id, 2, vec![Value::Uint(token)]);
                self.emit(
                    object_id,
                    0,
                    vec![Value::Int(x), Value::Int(y), Value::Int(1), Value::Int(1)],
                );
            }
            Mn::PopupDestroy => {
                if let Some(Obj::Popup(_)) = self.objects.get(&object_id) {
                    let xdg = popup_owner(&self.objects, object_id).unwrap_or(0);
                    if xdg != 0 {
                        if let Some(Obj::XdgSurface(x)) = self.objects.get(&xdg).cloned() {
                            self.teardown_role(xdg, x.surface);
                            return Ok(());
                        }
                    }
                    self.destroy(object_id);
                }
            }
        }
        Ok(())
    }

    /// The surface commit: apply the pending state under the xdg
    /// handshake rules.
    fn surface_commit(&mut self, surface_id: u32) -> Result<(), Fatal> {
        let role = surface_role(&self.objects, surface_id);
        let xdg = match role {
            Some(Role::Toplevel(x) | Role::Popup(x)) => Some(x),
            _ => None,
        };
        let buffer = match self.objects.get(&surface_id) {
            Some(Obj::Surface(s)) => s.pending.buffer,
            _ => unreachable!(),
        };
        if let Some(x) = xdg {
            let phase = match self.objects.get(&x) {
                Some(Obj::XdgSurface(s)) => s.phase,
                _ => unreachable!(),
            };
            if buffer != 0 && !matches!(phase, XdgPhase::Acked(_) | XdgPhase::Mapped) {
                self.kill(
                    display_error::INVALID_ARGUMENT,
                    x,
                    "unconfigured buffer: commit before the initial ack",
                );
                return Err(self.fatal.clone().unwrap());
            }
        }
        // Apply.
        let fire = match self.objects.get_mut(&surface_id) {
            Some(Obj::Surface(s)) => s.commit(),
            _ => unreachable!(),
        };
        if let Some(x) = xdg {
            if let Some(Obj::XdgSurface(xs)) = self.objects.get_mut(&x) {
                if buffer != 0 && matches!(xs.phase, XdgPhase::Acked(_)) {
                    xs.phase = XdgPhase::Mapped;
                }
            }
        }
        // Sync callbacks fire on commit (the presentation barrier).
        for cb in fire {
            let serial = self.next_serial();
            self.emit(cb, 0, vec![Value::Uint(serial)]);
            self.server_destroy(cb);
            self.emit_raw(&protocol::WL_DISPLAY, 1, 1, vec![Value::Uint(cb)]);
            self.graveyard.retain(|&g| g != cb);
        }
        Ok(())
    }

    /// Tear down an xdg role stack: the role object, the xdg_surface,
    /// and the surface's role binding.
    fn teardown_role(&mut self, xdg_id: u32, surface_id: u32) {
        // Role object first (toplevel or popup).
        for (id, obj) in self.objects.clone() {
            let is_role = match obj {
                Obj::Toplevel(_) => toplevel_owner(&self.objects, id) == Some(xdg_id),
                Obj::Popup(_) => popup_owner(&self.objects, id) == Some(xdg_id),
                _ => false,
            };
            if is_role {
                self.destroy(id);
            }
        }
        if let Some(Obj::Surface(s)) = self.objects.get_mut(&surface_id) {
            s.role = Role::None;
            s.mapped = false;
            s.attached = 0;
        }
        self.destroy(xdg_id);
    }

    // ---- driver seams: input injection ----

    /// The driver's clock word (rides every injected event).
    pub fn set_time(&mut self, time: u32) {
        self.time = time;
    }

    /// Pointer focus + motion: enter/leave/motion with fresh serials.
    /// Coordinates are surface-space (fixed 24.8 on the wire).
    pub fn pointer_focus(&mut self, pointer: u32, surface: u32, x: i32, y: i32) {
        self.pointer_pos = (x, y);
        let old = match self.objects.get(&pointer) {
            Some(Obj::Pointer { focus }) => *focus,
            _ => return,
        };
        if old != surface && old != 0 {
            let serial = self.next_serial();
            self.emit(pointer, 1, vec![Value::Uint(serial), Value::Object(old)]);
        }
        if surface != 0 && surface != old {
            let serial = self.next_serial();
            self.emit(
                pointer,
                0,
                vec![
                    Value::Uint(serial),
                    Value::Object(surface),
                    Value::Fixed(x << 8),
                    Value::Fixed(y << 8),
                ],
            );
            self.emit(pointer, 5, vec![]);
        }
        if let Some(Obj::Pointer { focus }) = self.objects.get_mut(&pointer) {
            *focus = surface;
        }
    }

    /// Pointer motion within the focused surface.
    pub fn pointer_motion(&mut self, pointer: u32, x: i32, y: i32) {
        self.pointer_pos = (x, y);
        self.emit(
            pointer,
            2,
            vec![
                Value::Uint(self.time),
                Value::Fixed(x << 8),
                Value::Fixed(y << 8),
            ],
        );
        self.emit(pointer, 5, vec![]);
    }

    /// A pointer button (Wayland button codes: 1..3 for the mouse).
    pub fn pointer_button(&mut self, pointer: u32, button: u32, pressed: bool) {
        let serial = self.next_serial();
        self.emit(
            pointer,
            3,
            vec![
                Value::Uint(serial),
                Value::Uint(self.time),
                Value::Uint(button),
                Value::Uint(u32::from(pressed)),
            ],
        );
        self.emit(pointer, 5, vec![]);
    }

    /// A pointer axis step (0 = vertical wheel, 1 = horizontal).
    pub fn pointer_axis(&mut self, pointer: u32, axis: u32, value: i32) {
        self.emit(
            pointer,
            4,
            vec![
                Value::Uint(self.time),
                Value::Uint(axis),
                Value::Fixed(value << 8),
            ],
        );
        self.emit(pointer, 5, vec![]);
    }

    /// Keyboard focus: enter with the pressed-key set, or leave.
    pub fn keyboard_focus(&mut self, keyboard: u32, surface: u32, keys: &[u32]) {
        let old = match self.objects.get(&keyboard) {
            Some(Obj::Keyboard { focus }) => *focus,
            _ => return,
        };
        if old != 0 && old != surface {
            let serial = self.next_serial();
            self.emit(keyboard, 2, vec![Value::Uint(serial), Value::Object(old)]);
        }
        if surface != 0 && surface != old {
            let serial = self.next_serial();
            // The pressed-key set rides as an array of uint32 words.
            let mut key_bytes = Vec::with_capacity(keys.len() * 4);
            for &k in keys {
                key_bytes.extend_from_slice(&k.to_le_bytes());
            }
            self.emit(
                keyboard,
                1,
                vec![
                    Value::Uint(serial),
                    Value::Object(surface),
                    Value::Array(key_bytes.into()),
                ],
            );
        }
        if let Some(Obj::Keyboard { focus }) = self.objects.get_mut(&keyboard) {
            *focus = surface;
        }
    }

    /// Deliver the keymap on one keyboard object (send-once).
    ///
    /// The blob is the process binary's xkb v1 text (the same keymap
    /// LDP's own clients get); the fd index and byte size ride the
    /// event per the wire schema.
    pub fn send_keymap(&mut self, keyboard: u32) {
        if !matches!(self.objects.get(&keyboard), Some(Obj::Keyboard { .. })) {
            return;
        }
        if self.keymap_state.contains(&keyboard) {
            return;
        }
        let fd = self.host.keymap_fd();
        let blob = self.host.keymap();
        // Format 1 = xkb v1 text; size counts the NUL terminator.
        let size = blob.len() as u32 + 1;
        self.keymap_state.insert(keyboard);
        self.emit(
            keyboard,
            0,
            vec![Value::Fd(fd), Value::Uint(1), Value::Uint(size)],
        );
    }

    /// A key event (evdev keycode).
    pub fn key(&mut self, keyboard: u32, keycode: u32, pressed: bool) {
        let serial = self.next_serial();
        self.emit(
            keyboard,
            3,
            vec![
                Value::Uint(serial),
                Value::Uint(self.time),
                Value::Uint(keycode),
                Value::Uint(u32::from(pressed)),
            ],
        );
    }

    /// The modifier group state.
    pub fn modifiers(
        &mut self,
        keyboard: u32,
        depressed: u32,
        latched: u32,
        locked: u32,
        group: u32,
    ) {
        let serial = self.next_serial();
        self.emit(
            keyboard,
            4,
            vec![
                Value::Uint(serial),
                Value::Uint(depressed),
                Value::Uint(latched),
                Value::Uint(locked),
                Value::Uint(group),
            ],
        );
    }

    // ---- driver seams: shell injection ----

    /// A toplevel configure from LDP (size 0 = client chooses). Sends
    /// `xdg_toplevel.configure` then `xdg_surface.configure(serial)`.
    pub fn toplevel_configure(&mut self, toplevel: u32, width: i32, height: i32) -> u32 {
        let serial = self.next_serial();
        self.emit(
            toplevel,
            0,
            vec![
                Value::Int(width),
                Value::Int(height),
                Value::Array(Vec::new().into_boxed_slice()),
            ],
        );
        let xdg = toplevel_owner(&self.objects, toplevel).unwrap_or(0);
        if xdg != 0 {
            self.emit(xdg, 0, vec![Value::Uint(serial)]);
            if let Some(Obj::XdgSurface(x)) = self.objects.get_mut(&xdg) {
                x.serial = serial;
                if matches!(x.phase, XdgPhase::Mapped | XdgPhase::Acked(_)) {
                    // A reconfigure after mapping: the ack gate
                    // re-arms for the next commit.
                    x.phase = XdgPhase::ConfigureSent(serial);
                }
            }
        }
        serial
    }

    /// The user asked to close a toplevel.
    pub fn toplevel_close(&mut self, toplevel: u32) {
        self.emit(toplevel, 1, vec![]);
    }

    /// Dismiss a popup (focus loss).
    pub fn popup_done(&mut self, popup: u32) {
        self.emit(popup, 1, vec![]);
    }

    // ---- driver seams: state reads ----

    /// Read one surface's committed state.
    #[must_use]
    pub fn surface(&self, id: u32) -> Option<&Surface> {
        match self.objects.get(&id) {
            Some(Obj::Surface(s)) => Some(s),
            _ => None,
        }
    }

    /// Read one buffer's geometry.
    #[must_use]
    pub fn buffer(&self, id: u32) -> Option<&WlBuffer> {
        match self.objects.get(&id) {
            Some(Obj::Buffer(b)) => Some(b),
            _ => None,
        }
    }

    /// Read one toplevel's shell state.
    #[must_use]
    pub fn toplevel(&self, id: u32) -> Option<&Toplevel> {
        match self.objects.get(&id) {
            Some(Obj::Toplevel(t)) => Some(t),
            _ => None,
        }
    }

    /// Read one popup's shell state.
    #[must_use]
    pub fn popup(&self, id: u32) -> Option<&Popup> {
        match self.objects.get(&id) {
            Some(Obj::Popup(p)) => Some(p),
            _ => None,
        }
    }

    /// The wl_surface an xdg_surface wraps.
    #[must_use]
    pub fn xdg_surface_wl(&self, xdg: u32) -> Option<u32> {
        match self.objects.get(&xdg) {
            Some(Obj::XdgSurface(x)) => Some(x.surface),
            _ => None,
        }
    }

    /// Every mapped surface (the driver's export set).
    #[must_use]
    pub fn mapped_surfaces(&self) -> Vec<(u32, &Surface)> {
        self.objects
            .iter()
            .filter_map(|(&id, o)| match o {
                Obj::Surface(s) if s.mapped => Some((id, s)),
                _ => None,
            })
            .collect()
    }

    /// Attach one buffer and commit (the test scripting path — one
    /// batched feed).
    ///
    /// # Errors
    /// The surface-commit handshake errors (unconfigured buffer).
    ///
    /// # Panics
    /// Never: the schemas are compile-time constants of the subset.
    pub fn commit_with_buffer(&mut self, surface: u32, buffer: u32) -> Result<(), Fatal> {
        let attach = Message {
            object_id: surface,
            opcode: 1,
            args: vec![Value::Object(buffer), Value::Int(0), Value::Int(0)],
        };
        let (attach_bytes, _) = wire::encode(
            &attach,
            protocol::request(&protocol::WL_SURFACE, 1)
                .expect("attach schema")
                .args,
        );
        let commit = Message {
            object_id: surface,
            opcode: 6,
            args: vec![],
        };
        let (commit_bytes, _) = wire::encode(
            &commit,
            protocol::request(&protocol::WL_SURFACE, 6)
                .expect("commit schema")
                .args,
        );
        let mut batch = attach_bytes;
        batch.extend_from_slice(&commit_bytes);
        self.feed(&batch)
    }

    /// Read a buffer's pixels through the host seam (the implicit-sync
    /// read barrier — the commit's request stream is the fence).
    #[must_use]
    pub fn read_buffer(&mut self, buffer: u32) -> Option<Vec<u8>> {
        let (pool, offset, len) = match self.objects.get(&buffer) {
            Some(Obj::Buffer(b)) => (b.pool, b.offset as usize, b.byte_len()),
            _ => return None,
        };
        self.host.read_pool(pool, offset, len)
    }

    /// The current serial counter.
    #[must_use]
    pub fn serial(&self) -> u32 {
        self.serial
    }

    /// Every pointer device object the client created.
    #[must_use]
    pub fn pointer_objects(&self) -> Vec<u32> {
        self.objects
            .iter()
            .filter(|(_, o)| matches!(o, Obj::Pointer { .. }))
            .map(|(&id, _)| id)
            .collect()
    }

    /// Every keyboard device object the client created.
    #[must_use]
    pub fn keyboard_objects(&self) -> Vec<u32> {
        self.objects
            .iter()
            .filter(|(_, o)| matches!(o, Obj::Keyboard { .. }))
            .map(|(&id, _)| id)
            .collect()
    }

    /// The xdg_toplevel that a surface's role names (0 when the role
    /// is not a toplevel or the object is gone).
    #[must_use]
    pub fn toplevel_of_surface(&self, surface: u32) -> Option<u32> {
        let role = self.surface(surface)?.role;
        match role {
            Role::Toplevel(xdg) => {
                // The toplevel object is the one bound to this xdg_surface.
                for (&id, o) in &self.objects {
                    if let Obj::Toplevel(_) = o {
                        if toplevel_owner(&self.objects, id) == Some(xdg) {
                            return Some(id);
                        }
                    }
                }
                None
            }
            _ => None,
        }
    }

    /// The window geometry an xdg_surface set (None = the buffer).
    #[must_use]
    pub fn xdg_window_geometry(&self, xdg: u32) -> Option<Rect> {
        match self.objects.get(&xdg) {
            Some(Obj::XdgSurface(x)) => x.window_geometry,
            _ => None,
        }
    }

    /// Whether a keyboard object got its keymap (test visibility).
    #[must_use]
    pub fn keymap_delivered(&self, keyboard: u32) -> bool {
        self.keymap_state.contains(&keyboard)
    }
}

/// Solve the base popup position from a positioner (anchor edge +
/// gravity + offset, no constraints — the driver re-solves).
#[must_use]
fn solve_position(p: &Positioner) -> (i32, i32) {
    let r = &p.anchor_rect;
    // The anchor point on the anchor edge.
    let (ax, ay) = match p.anchor {
        Anchor::TopLeft | Anchor::Unset | Anchor::None => (r.x, r.y),
        Anchor::Top => (r.x + r.w as i32 / 2, r.y),
        Anchor::TopRight => (r.x + r.w as i32, r.y),
        Anchor::Right => (r.x + r.w as i32, r.y + r.h as i32 / 2),
        Anchor::BottomRight => (r.x + r.w as i32, r.y + r.h as i32),
        Anchor::Bottom => (r.x + r.w as i32 / 2, r.y + r.h as i32),
        Anchor::BottomLeft => (r.x, r.y + r.h as i32),
        Anchor::Left => (r.x, r.y + r.h as i32 / 2),
    };
    // Gravity pulls the popup's matching corner onto the anchor: the
    // horizontal share of w and the vertical share of h each corner
    // claims (0 = none, 1 = half, 2 = all).
    let (w, h) = p.size.unwrap_or((0, 0));
    let (wx, hy) = match p.gravity {
        Anchor::TopLeft | Anchor::TopRight => (2, 2),
        Anchor::Top => (1, 2),
        Anchor::Right => (2, 1),
        Anchor::Bottom => (1, 0),
        Anchor::BottomLeft => (0, 2),
        Anchor::Left => (0, 1),
        // BottomRight and the unset/none defaults: no pull.
        Anchor::BottomRight | Anchor::Unset | Anchor::None => (0, 0),
    };
    let x = ax - w * wx / 2 + p.offset.0;
    let y = ay - h * hy / 2 + p.offset.1;
    (x, y)
}

/// The request a (object, opcode) pair routes to, by interface name.
enum Mn {
    DisplaySync,
    DisplayGetRegistry,
    RegistryBind,
    CompositorCreateSurface,
    CompositorCreateRegion,
    RegionDestroy,
    RegionAdd,
    RegionSubtract,
    ShmCreatePool,
    PoolDestroy,
    PoolCreateBuffer,
    PoolResize,
    BufferDestroy,
    SurfaceDestroy,
    SurfaceAttach,
    SurfaceDamage,
    SurfaceDamageBuffer,
    SurfaceFrame,
    SurfaceSetOpaqueRegion,
    SurfaceSetInputRegion,
    SurfaceCommit,
    SeatGetPointer,
    SeatGetKeyboard,
    SeatGetTouch,
    SeatRelease,
    PointerSetCursor,
    PointerRelease,
    KeyboardRelease,
    TouchRelease,
    WmBaseDestroy,
    WmBaseCreatePositioner,
    WmBaseGetXdgSurface,
    WmBasePong,
    PositionerDestroy,
    PositionerSetSize,
    PositionerSetAnchorRect,
    PositionerSetAnchor,
    PositionerSetGravity,
    PositionerSetConstraintAdjustment,
    PositionerSetOffset,
    XdgSurfaceDestroy,
    XdgSurfaceGetToplevel,
    XdgSurfaceGetPopup,
    XdgSurfaceSetWindowGeometry,
    XdgSurfaceAckConfigure,
    ToplevelDestroy,
    ToplevelSetParent,
    ToplevelSetTitle,
    ToplevelSetAppId,
    ToplevelShowWindowMenu,
    ToplevelMove,
    ToplevelResize,
    ToplevelSetMaxSize,
    ToplevelSetMinSize,
    ToplevelStateRequests,
    PopupDestroy,
    PopupGrab,
    PopupReposition,
}

#[allow(clippy::too_many_lines)] // one flat routing table over the subset
fn opcode_mnemonic(object_id: u32, opcode: u32, objects: &BTreeMap<u32, Obj>) -> Mn {
    let name = objects.get(&object_id).map_or("", |o| o.iface().name);
    match (name, opcode) {
        ("wl_display", 0) => Mn::DisplaySync,
        ("wl_display", 1) => Mn::DisplayGetRegistry,
        ("wl_registry", 0) => Mn::RegistryBind,
        ("wl_compositor", 0) => Mn::CompositorCreateSurface,
        ("wl_compositor", 1) => Mn::CompositorCreateRegion,
        ("wl_region", 0) => Mn::RegionDestroy,
        ("wl_region", 1) => Mn::RegionAdd,
        ("wl_region", 2) => Mn::RegionSubtract,
        ("wl_shm", 0) => Mn::ShmCreatePool,
        ("wl_shm_pool", 0) => Mn::PoolDestroy,
        ("wl_shm_pool", 1) => Mn::PoolCreateBuffer,
        ("wl_shm_pool", 2) => Mn::PoolResize,
        ("wl_buffer", 0) => Mn::BufferDestroy,
        ("wl_surface", 0) => Mn::SurfaceDestroy,
        ("wl_surface", 1) => Mn::SurfaceAttach,
        ("wl_surface", 2) => Mn::SurfaceDamage,
        ("wl_surface", 3) => Mn::SurfaceFrame,
        ("wl_surface", 4) => Mn::SurfaceSetOpaqueRegion,
        ("wl_surface", 5) => Mn::SurfaceSetInputRegion,
        ("wl_surface", 6) => Mn::SurfaceCommit,
        ("wl_surface", 7) => Mn::SurfaceDamageBuffer,
        ("wl_seat", 0) => Mn::SeatGetPointer,
        ("wl_seat", 1) => Mn::SeatGetKeyboard,
        ("wl_seat", 2) => Mn::SeatGetTouch,
        ("wl_seat", 3) => Mn::SeatRelease,
        ("wl_pointer", 0) => Mn::PointerSetCursor,
        ("wl_pointer", 1) => Mn::PointerRelease,
        ("wl_keyboard", 0) => Mn::KeyboardRelease,
        ("wl_touch", 0) => Mn::TouchRelease,
        ("xdg_wm_base", 0) => Mn::WmBaseDestroy,
        ("xdg_wm_base", 1) => Mn::WmBaseCreatePositioner,
        ("xdg_wm_base", 2) => Mn::WmBaseGetXdgSurface,
        ("xdg_wm_base", 3) => Mn::WmBasePong,
        ("xdg_positioner", 0) => Mn::PositionerDestroy,
        ("xdg_positioner", 1) => Mn::PositionerSetSize,
        ("xdg_positioner", 2) => Mn::PositionerSetAnchorRect,
        ("xdg_positioner", 3) => Mn::PositionerSetAnchor,
        ("xdg_positioner", 4) => Mn::PositionerSetGravity,
        ("xdg_positioner", 5) => Mn::PositionerSetConstraintAdjustment,
        ("xdg_positioner", 6) => Mn::PositionerSetOffset,
        ("xdg_surface", 0) => Mn::XdgSurfaceDestroy,
        ("xdg_surface", 1) => Mn::XdgSurfaceGetToplevel,
        ("xdg_surface", 2) => Mn::XdgSurfaceGetPopup,
        ("xdg_surface", 3) => Mn::XdgSurfaceSetWindowGeometry,
        ("xdg_surface", 4) => Mn::XdgSurfaceAckConfigure,
        ("xdg_toplevel", 0) => Mn::ToplevelDestroy,
        ("xdg_toplevel", 1) => Mn::ToplevelSetParent,
        ("xdg_toplevel", 2) => Mn::ToplevelSetTitle,
        ("xdg_toplevel", 3) => Mn::ToplevelSetAppId,
        ("xdg_toplevel", 4) => Mn::ToplevelShowWindowMenu,
        ("xdg_toplevel", 5) => Mn::ToplevelMove,
        ("xdg_toplevel", 6) => Mn::ToplevelResize,
        ("xdg_toplevel", 7) => Mn::ToplevelSetMaxSize,
        ("xdg_toplevel", 8) => Mn::ToplevelSetMinSize,
        ("xdg_toplevel", 9..=13) => Mn::ToplevelStateRequests,
        ("xdg_popup", 0) => Mn::PopupDestroy,
        ("xdg_popup", 1) => Mn::PopupGrab,
        ("xdg_popup", 2) => Mn::PopupReposition,
        _ => unreachable!("schema lookup validated the opcode"),
    }
}

/// Pull four int arguments.
fn int4(args: &[Value]) -> (i32, i32, i32, i32) {
    (
        int_at(args, 0),
        int_at(args, 1),
        int_at(args, 2),
        int_at(args, 3),
    )
}

fn int_at(args: &[Value], i: usize) -> i32 {
    args.get(i).map_or(0, |v| match v {
        Value::Int(x) | Value::Fixed(x) => *x,
        Value::Uint(x) => *x as i32,
        _ => 0,
    })
}

/// The surface's role, if any.
fn surface_role(objects: &BTreeMap<u32, Obj>, surface: u32) -> Option<Role> {
    match objects.get(&surface) {
        Some(Obj::Surface(s)) => Some(s.role),
        _ => None,
    }
}

/// The wl_surface an xdg_surface wraps.
fn xdg_surface_of(objects: &BTreeMap<u32, Obj>, xdg: u32) -> u32 {
    match objects.get(&xdg) {
        Some(Obj::XdgSurface(x)) => x.surface,
        _ => 0,
    }
}

/// The xdg_surface that owns a toplevel (the role object records the
/// back-link).
fn toplevel_owner(objects: &BTreeMap<u32, Obj>, toplevel: u32) -> Option<u32> {
    match objects.get(&toplevel) {
        Some(Obj::Toplevel(t)) if t.xdg != 0 => Some(t.xdg),
        _ => None,
    }
}

/// The xdg_surface that owns a popup (the role object records the
/// back-link).
fn popup_owner(objects: &BTreeMap<u32, Obj>, popup: u32) -> Option<u32> {
    match objects.get(&popup) {
        Some(Obj::Popup(p)) if p.xdg != 0 => Some(p.xdg),
        _ => None,
    }
}
