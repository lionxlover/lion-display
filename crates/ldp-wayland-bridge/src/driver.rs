//! The LDP side of the Wayland bridge: the foreign-surface proxy.
//!
//! The driver owns the Wayland [`Client`] state machine and plays one
//! LDP client. Every **committed** foreign surface maps to one LDP
//! surface + toplevel (or popup); every commit reads the foreign
//! buffer through the pool seam and re-exports it through the
//! driver's own LDP shm pool with exact damage. Input flows the other
//! way: LDP pointer/keyboard events (which name the LDP surface)
//! route back onto the foreign client's device objects with fresh
//! serials. LDP toplevel configures become `xdg_toplevel.configure`
//! plus `ack_configure` handshakes; LDP close becomes
//! `xdg_toplevel.close`.
//!
//! **Implicit-to-explicit sync translation.** Foreign `wl_shm`
//! buffers carry no fences — the client's request stream *is* the
//! synchronization. The bridge reads the pool bytes at
//! commit-processing time (the request barrier), mirrors them into
//! its own pool, and emits the LDP `commit` — an explicit readiness
//! statement by construction. The LDP wire never sees an implicit
//! dependency; foreign DMA-BUF (the future path) attaches through the
//! same seam with fences minted there.
//!
//! **Popup translation.** An `xdg_positioner` becomes the
//! `ldp.shell.shell.get_popup` argument set through the anchor /
//! gravity / constraint tables in [`crate::state`] — the LDP
//! vocabulary is the superset, so the mapping is total and pinned by
//! the popup suite.
//!
//! **Token gate.** As with the X bridge: no LDP message leaves before
//! a bridge-scope token passes the [`TokenCheck`] seam.

#![forbid(unsafe_code)]

use ldp_core::geometry::Rect;
use ldp_core::wire::Value;
use ldp_protocol::generated::{core, shell};
use ldp_protocol::Message;

use crate::dispatch::{Client, Host};
use crate::state::{Anchor, Role};

/// Bridge-scope token validation seam (wired to the broker in
/// production, the Phase 16 grant table in tests).
pub trait TokenCheck {
    /// Validate one token submission for `app_id`.
    fn check(&mut self, app_id: &str, token: [u32; 8]) -> bool;
}

/// Process-binary seam for the bridge's own pool export.
///
/// Pool identity is explicit (the LDP pool object id the driver
/// minted): a multi-window foreign client owns one pool per export,
/// and a later recommit of an early export must mirror into THAT
/// pool — "the most recently allocated one" would silently cross
/// streams. The X11 face has a single root pool, so the identity is
/// a constant there.
pub trait DriverHost {
    /// Allocate (or grow) the backing for pool `pool` at `size`
    /// bytes; returns the ancillary fd index for
    /// `shm.create_pool` (the descriptor itself rides the message).
    fn pool_fd(&mut self, pool: u32, size: u64) -> u32;
    /// Mirror `bytes` into pool `pool`'s backing (offset 0).
    fn sync_pool(&mut self, pool: u32, bytes: &[u8]);
}

/// A recording host for tests.
#[derive(Default)]
pub struct RecordingHost {
    /// Every pool allocation (id, size), in order.
    pub pools: Vec<(u32, u64)>,
    /// Every mirror-sync (pool, byte length), in order.
    pub syncs: Vec<(u32, usize)>,
}

impl DriverHost for RecordingHost {
    fn pool_fd(&mut self, pool: u32, size: u64) -> u32 {
        self.pools.push((pool, size));
        0
    }
    fn sync_pool(&mut self, pool: u32, bytes: &[u8]) {
        self.syncs.push((pool, bytes.len()));
    }
}

/// Why the driver refused to start.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum AuthError {
    /// The token was rejected by the check seam.
    Rejected,
    /// The driver already authenticated.
    AlreadyRunning,
}

/// The driver's lifecycle phase.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    /// No token accepted yet.
    Unauthenticated,
    /// Bootstrapped and proxying.
    Running,
}

/// One foreign surface's LDP export.
#[derive(Clone, Debug)]
struct Export {
    /// The wl_surface id.
    wl_surface: u32,
    /// The LDP surface object id.
    ldp_surface: u32,
    /// The LDP shm pool id.
    ldp_pool: u32,
    /// The LDP buffer id.
    ldp_buffer: u32,
    /// The LDP toplevel (0 for popups — they export through the
    /// parent's surface tree at the protocol level).
    ldp_toplevel: u32,
    /// The xdg_toplevel id (foreign side) when the role is a toplevel.
    xdg_toplevel: u32,
    /// The committed geometry (buffer size or window geometry).
    geometry: Rect,
}

/// The LDP-side object id floor for per-surface exports.
const FIRST_EXPORT_ID: u32 = 10;

/// The Wayland-to-LDP proxy driver.
pub struct WlBridgeDriver {
    client: Client,
    phase: Phase,
    out: Vec<Message>,
    host: Box<dyn DriverHost>,
    exports: Vec<Export>,
    next_id: u32,
    /// The commit-cookie counter (each commit carries one; the
    /// compositor's presented verdict rides it back).
    cookie: u32,
}

impl WlBridgeDriver {
    /// A driver over a fresh Wayland client state (the host seam
    /// supplies pool memory and the keymap).
    #[must_use]
    pub fn new(wayland_host: Box<dyn Host>) -> WlBridgeDriver {
        WlBridgeDriver {
            client: Client::new(wayland_host),
            phase: Phase::Unauthenticated,
            out: Vec::new(),
            host: Box::new(RecordingHost::default()),
            exports: Vec::new(),
            next_id: FIRST_EXPORT_ID,
            cookie: 0,
        }
    }

    /// Attach a custom driver host (the pool export seam).
    #[must_use]
    pub fn with_driver_host(mut self, host: Box<dyn DriverHost>) -> WlBridgeDriver {
        self.host = host;
        self
    }

    /// The bridge-scope token gate.
    ///
    /// # Errors
    /// [`AuthError::Rejected`] when the seam rejects the token;
    /// [`AuthError::AlreadyRunning`] when called twice.
    pub fn authenticate(
        &mut self,
        app_id: &str,
        token: [u32; 8],
        check: &mut dyn TokenCheck,
    ) -> Result<(), AuthError> {
        if self.phase != Phase::Unauthenticated {
            return Err(AuthError::AlreadyRunning);
        }
        if !check.check(app_id, token) {
            return Err(AuthError::Rejected);
        }
        self.phase = Phase::Running;
        self.bootstrap();
        Ok(())
    }

    /// The LDP bootstrap burst: hello, registry, four binds.
    fn bootstrap(&mut self) {
        let hello = Message::new(1, core::connection::request::HELLO)
            .arg(Value::Uint32(1))
            .arg(Value::Bitset(ldp_core::bitset::Bitset128::EMPTY));
        self.out.push(hello);
        let registry = Message::new(1, core::connection::request::GET_REGISTRY)
            .arg(Value::Uint32(1))
            .arg(Value::NewId(object_id(2)));
        self.out.push(registry);
        for (interface, version, id) in [
            (core::compositor::NAME, 1, 3),
            (core::shm::NAME, 1, 4),
            ("ldp.input.seat", 1, 5),
            (shell::shell::NAME, 1, 6),
        ] {
            let bind = Message::new(2, 1)
                .arg(Value::String(interface.into()))
                .arg(Value::Uint32(version))
                .arg(Value::NewId(object_id(id)));
            self.out.push(bind);
        }
        self.next_id = FIRST_EXPORT_ID;
    }

    // ---- Wayland side ----

    /// Feed bytes from the Wayland socket.
    ///
    /// # Errors
    /// The client's fatal reason (protocol violation): the process
    /// must close the connection.
    pub fn wl_feed(&mut self, bytes: &[u8]) -> Result<(), crate::dispatch::Fatal> {
        self.client.feed(bytes)
    }

    /// Take the pending Wayland event stream (bytes + fd count).
    pub fn wl_output(&mut self) -> (Vec<u8>, u32) {
        self.client.take_output()
    }

    /// The Wayland client state (multi-client processes drive more
    /// through their own bookkeeping; the Phase 17 shape is one).
    #[must_use]
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// Mutable access (multi-client processes drive their own
    /// bookkeeping through it).
    pub fn client_mut(&mut self) -> &mut Client {
        &mut self.client
    }

    // ---- LDP side ----

    /// Take the pending LDP messages.
    pub fn take_ldp_messages(&mut self) -> Vec<Message> {
        std::mem::take(&mut self.out)
    }

    /// Poll the foreign client for newly committed surfaces and
    /// re-commit changed ones. Called on the driver's frame tick.
    pub fn on_frame_tick(&mut self) {
        if self.phase != Phase::Running {
            return;
        }
        let committed: Vec<(u32, Rect)> = self
            .client
            .mapped_surfaces()
            .into_iter()
            .map(|(id, s)| (id, geometry_of(&self.client, id, s.attached)))
            .collect();
        for (wl, geometry) in committed {
            let existing = self.exports.iter().position(|e| e.wl_surface == wl);
            match existing {
                None => self.export_surface(wl, geometry),
                Some(idx) => {
                    let changed = self.exports[idx].geometry != geometry;
                    if changed {
                        self.recommit_with_geometry(idx, geometry);
                    } else {
                        self.recommit(idx);
                    }
                }
            }
        }
    }

    /// Export one foreign surface: LDP surface + pool + buffer +
    /// toplevel, then the first attach/commit.
    fn export_surface(&mut self, wl_surface: u32, geometry: Rect) {
        let surface_id = self.alloc_id();
        let pool_id = self.alloc_id();
        let buffer_id = self.alloc_id();
        let toplevel_id = self.alloc_id();
        // The pool carries the buffer bytes.
        let size = u64::from(geometry.w) * u64::from(geometry.h) * 4;
        let fd = self.host.pool_fd(pool_id, size);
        let create_surface = Message::new(3, core::compositor::request::CREATE_SURFACE)
            .arg(Value::NewId(object_id(surface_id)));
        self.out.push(create_surface);
        let create_pool = Message::new(4, core::shm::request::CREATE_POOL)
            .arg(Value::Fd(fd))
            .arg(Value::Int64(size as i64))
            .arg(Value::NewId(object_id(pool_id)));
        self.out.push(create_pool);
        let create_buffer = Message::new(pool_id, core::shm_pool::request::CREATE_BUFFER)
            .arg(Value::Int32(0))
            .arg(Value::Int32(geometry.w as i32))
            .arg(Value::Int32(geometry.h as i32))
            .arg(Value::Int32((geometry.w * 4) as i32))
            .arg(Value::Uint32(ldp_core::buffer::FourCC::XRGB8888.code()))
            .arg(Value::NewId(object_id(buffer_id)));
        self.out.push(create_buffer);
        // The role: toplevel (client decorations — the foreign client
        // draws its own).
        let xdg = role_object(&self.client, wl_surface);
        let get_toplevel = Message::new(6, shell::shell::request::GET_TOPLEVEL)
            .arg(Value::Object(Some(object_id(surface_id))))
            .arg(Value::Enum(2))
            .arg(Value::NewId(object_id(toplevel_id)));
        self.out.push(get_toplevel);
        let export = Export {
            wl_surface,
            ldp_surface: surface_id,
            ldp_pool: pool_id,
            ldp_buffer: buffer_id,
            ldp_toplevel: toplevel_id,
            xdg_toplevel: xdg,
            geometry,
        };
        // The foreign title rides the LDP toplevel.
        if export.xdg_toplevel != 0 {
            if let Some(t) = self.client.toplevel(export.xdg_toplevel) {
                if let Some(title) = &t.title {
                    let set_title = Message::new(toplevel_id, shell::toplevel::request::SET_TITLE)
                        .arg(Value::String(title.clone()));
                    self.out.push(set_title);
                }
            }
        }
        self.exports.push(export);
        self.commit_last();
    }

    /// Re-commit a changed export (new damage only).
    fn recommit(&mut self, idx: usize) {
        self.commit_at(idx);
    }

    /// Commit the most recent export (attach + damage + commit).
    fn commit_last(&mut self) {
        let idx = self.exports.len() - 1;
        self.commit_at(idx);
    }

    fn commit_at(&mut self, idx: usize) {
        let Some(export) = self.exports.get(idx).cloned() else {
            return;
        };
        let Some(surface) = self.client.surface(export.wl_surface).cloned() else {
            return;
        };
        // The implicit→explicit barrier: read the foreign pool now.
        let Some(pixels) = self.client.read_buffer(surface.attached) else {
            return;
        };
        let pool = export.ldp_pool;
        self.host.sync_pool(pool, &pixels);
        let attach = Message::new(export.ldp_surface, core::surface::request::ATTACH)
            .arg(Value::Object(Some(object_id(export.ldp_buffer))));
        self.out.push(attach);
        let damage: Vec<ldp_core::wire::Primitive> = surface
            .damage
            .iter()
            .map(|r| ldp_core::wire::Primitive::Rect(*r))
            .collect();
        if let Ok(array) = Value::array(ldp_core::wire::ArgType::Rect, damage) {
            let damage_msg =
                Message::new(export.ldp_surface, core::surface::request::DAMAGE).arg(array);
            self.out.push(damage_msg);
        }
        let commit = Message::new(export.ldp_surface, core::surface::request::COMMIT)
            .arg(Value::Uint32(self.cookie.wrapping_add(1)));
        self.cookie = self.cookie.wrapping_add(1);
        self.out.push(commit);
    }

    /// Grow the export's pool for a new geometry (the resize path).
    fn recommit_with_geometry(&mut self, idx: usize, geometry: Rect) {
        let size = u64::from(geometry.w) * u64::from(geometry.h) * 4;
        let (pool, surface_obj) = {
            let Some(e) = self.exports.get(idx) else {
                return;
            };
            (e.ldp_pool, e.ldp_surface)
        };
        let resize =
            Message::new(pool, core::shm_pool::request::RESIZE).arg(Value::Int64(size as i64));
        self.out.push(resize);
        let _ = self.host.pool_fd(pool, size);
        let buffer_id = self.alloc_id();
        let create_buffer = Message::new(pool, core::shm_pool::request::CREATE_BUFFER)
            .arg(Value::Int32(0))
            .arg(Value::Int32(geometry.w as i32))
            .arg(Value::Int32(geometry.h as i32))
            .arg(Value::Int32((geometry.w * 4) as i32))
            .arg(Value::Uint32(ldp_core::buffer::FourCC::XRGB8888.code()))
            .arg(Value::NewId(object_id(buffer_id)));
        self.out.push(create_buffer);
        if let Some(e) = self.exports.get_mut(idx) {
            e.ldp_buffer = buffer_id;
            e.geometry = geometry;
        }
        let _ = surface_obj;
        self.commit_at(idx);
    }

    /// Allocate one LDP client object id.
    fn alloc_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    // ---- input translation ----

    /// The foreign client's pointer objects (the driver routes LDP
    /// pointer events into them).
    #[must_use]
    pub fn pointer_objects(&self) -> Vec<u32> {
        self.client.pointer_objects()
    }

    /// The foreign client's keyboard objects.
    #[must_use]
    pub fn keyboard_objects(&self) -> Vec<u32> {
        self.client.keyboard_objects()
    }

    /// An LDP pointer enter/motion landed on an exported surface:
    /// focus + motion on every foreign pointer, in surface coords.
    pub fn ldp_pointer(&mut self, ldp_surface: u32, x: i32, y: i32) {
        let wl = match self.exports.iter().find(|e| e.ldp_surface == ldp_surface) {
            Some(e) => e.wl_surface,
            None => return,
        };
        for pointer in self.client.pointer_objects() {
            self.client.pointer_focus(pointer, wl, x, y);
        }
    }

    /// An LDP pointer button.
    pub fn ldp_pointer_button(&mut self, ldp_surface: u32, button: u32, pressed: bool) {
        let _ = ldp_surface;
        for pointer in self.client.pointer_objects() {
            self.client.pointer_button(pointer, button, pressed);
        }
    }

    /// An LDP keyboard enter: focus + keymap on every foreign
    /// keyboard, with the pressed-key set.
    pub fn ldp_keyboard_enter(&mut self, ldp_surface: u32, keys: &[u32]) {
        let wl = match self.exports.iter().find(|e| e.ldp_surface == ldp_surface) {
            Some(e) => e.wl_surface,
            None => return,
        };
        for keyboard in self.client.keyboard_objects() {
            self.client.send_keymap(keyboard);
            self.client.keyboard_focus(keyboard, wl, keys);
        }
    }

    /// An LDP key event (evdev keycode — Wayland uses the same).
    pub fn ldp_key(&mut self, keycode: u32, pressed: bool) {
        for keyboard in self.client.keyboard_objects() {
            self.client.key(keyboard, keycode, pressed);
        }
    }

    /// An LDP keyboard leave.
    pub fn ldp_keyboard_leave(&mut self, _ldp_surface: u32) {
        for keyboard in self.client.keyboard_objects() {
            self.client.keyboard_focus(keyboard, 0, &[]);
        }
    }

    // ---- shell translation ----

    /// An LDP toplevel configure: the foreign client gets
    /// `xdg_toplevel.configure` + the ack handshake.
    pub fn on_ldp_configure(&mut self, ldp_toplevel: u32, width: i32, height: i32) {
        let Some(xdg) = self
            .exports
            .iter()
            .find(|e| e.ldp_toplevel == ldp_toplevel)
            .map(|e| e.xdg_toplevel)
            .filter(|x| *x != 0)
        else {
            return;
        };
        self.client.toplevel_configure(xdg, width, height);
    }

    /// An LDP toplevel close.
    pub fn on_ldp_close(&mut self, ldp_toplevel: u32) {
        let Some(xdg) = self
            .exports
            .iter()
            .find(|e| e.ldp_toplevel == ldp_toplevel)
            .map(|e| e.xdg_toplevel)
            .filter(|x| *x != 0)
        else {
            return;
        };
        self.client.toplevel_close(xdg);
    }

    /// The popup translation table: an xdg positioner's anchor /
    /// gravity / constraints as `ldp.protocol.shell` popup creation
    /// arguments (the LDP vocabulary is the superset — the mapping
    /// is total). Returns `(anchor, gravity, constraints)`.
    #[must_use]
    pub fn popup_args(positioner: &crate::state::Positioner) -> (u32, u32, u32) {
        let anchor = positioner
            .anchor
            .to_ldp_anchor()
            .unwrap_or(shell::ANCHOR_VALUES[0].1);
        let gravity = positioner
            .gravity
            .to_ldp_anchor()
            .unwrap_or(shell::GRAVITY_VALUES[0].1);
        let mut constraints = 0u32;
        let adj = positioner.constraint_adjustment;
        if adj & crate::state::constraints::SLIDE_X != 0 {
            constraints |= shell::popup_constraints::SLIDE_X as u32;
        }
        if adj & crate::state::constraints::SLIDE_Y != 0 {
            constraints |= shell::popup_constraints::SLIDE_Y as u32;
        }
        if adj & crate::state::constraints::FLIP_X != 0 {
            constraints |= shell::popup_constraints::FLIP_X as u32;
        }
        if adj & crate::state::constraints::FLIP_Y != 0 {
            constraints |= shell::popup_constraints::FLIP_Y as u32;
        }
        if adj & crate::state::constraints::RESIZE_X != 0 {
            constraints |= shell::popup_constraints::RESIZE_X as u32;
        }
        if adj & crate::state::constraints::RESIZE_Y != 0 {
            constraints |= shell::popup_constraints::RESIZE_Y as u32;
        }
        (anchor, gravity, constraints)
    }

    /// Every export's LDP surface id (test visibility).
    #[must_use]
    pub fn export_surfaces(&self) -> Vec<(u32, u32)> {
        self.exports
            .iter()
            .map(|e| (e.wl_surface, e.ldp_surface))
            .collect()
    }
}

/// Allocate one client object id (panics never: constants are valid).
fn object_id(raw: u32) -> ldp_core::ids::ObjectId {
    ldp_core::ids::ObjectId::client(raw).expect("static client id in range")
}

/// The foreign geometry for one committed surface: the window
/// geometry when the role set one, else the buffer size.
fn geometry_of(client: &Client, surface: u32, buffer: u32) -> Rect {
    if let Some(role) = client.surface(surface).map(|s| s.role) {
        if let Some(r) = role_xdg(client, role).and_then(|x| client.xdg_window_geometry(x)) {
            return r;
        }
    }
    client.buffer(buffer).map_or(Rect::new(0, 0, 0, 0), |b| {
        Rect::new(0, 0, b.width.unsigned_abs(), b.height.unsigned_abs())
    })
}

fn role_xdg(_client: &Client, role: Role) -> Option<u32> {
    match role {
        Role::Toplevel(x) | Role::Popup(x) => Some(x),
        Role::None => None,
    }
}

/// The role object (xdg_toplevel id) of a surface, when it is one.
fn role_object(client: &Client, surface: u32) -> u32 {
    match client.surface(surface).map(|s| s.role) {
        Some(Role::Toplevel(_)) => client.toplevel_of_surface(surface).unwrap_or(0),
        _ => 0,
    }
}

/// An anchor's LDP wire value with the subset's default.
#[must_use]
pub fn ldp_anchor_of(anchor: Anchor) -> u32 {
    anchor.to_ldp_anchor().unwrap_or(1)
}
