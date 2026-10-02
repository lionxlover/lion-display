//! The LDP side of the X bridge: a rootful proxy client.
//!
//! The driver owns the [`Server`] (the X protocol state machine) and
//! plays one LDP client: it authenticates with a **bridge scope
//! token**, bootstraps the LDP connection, exports the composited root
//! store through a shared-memory pool, and re-commits it frame by
//! frame with exact damage. Input flows the other way: the process
//! feeds decoded LDP input events into the driver, which routes them
//! through the server's input seams and hands back X event bytes.
//!
//! **Implicit-to-explicit sync translation.** X11 requests carry no
//! fences: the client's byte stream *is* the synchronization. The
//! bridge processes every request to completion (pixels are in the
//! backing stores) before it emits an LDP `commit`, so the commit is
//! an explicit readiness statement by construction — an LDP acquire
//! fence would add nothing, and the LDP wire never sees implicit
//! dependencies. Foreign DMA-BUF surfaces (the future X main-loop
//! path) attach through the same boundary with fences minted at the
//! seam; the pure request path here needs none, and that doctrine is
//! pinned by tests.
//!
//! **Token gate.** The bridge runs with ambient authority (`Scope::Bridge`
//! in ldp-core's capability model) — granted by the Phase 16 broker,
//! never manifest-baselined. The crate keeps `ldp-security` out of its
//! runtime dependency set: the driver checks tokens through the
//! [`TokenCheck`] seam, which the process binary wires to its broker
//! connection (tests wire it to the real grant table).
//!
//! **Driver host seam.** Shared memory and fds are process property:
//! [`DriverHost`] allocates the pool backing (returning the ancillary
//! fd index for `shm.create_pool`) and mirrors the root store bytes
//! into it on every commit.

#![forbid(unsafe_code)]

use ldp_core::geometry::Rect;
use ldp_core::wire::Value;
use ldp_protocol::generated::{core, shell};
use ldp_protocol::Message;

use crate::dispatch::{ClientId, ClosePolicy, Focus, Server};
use crate::setup::ScreenParams;
use crate::shm::{NoShm, ShmHost};

/// The bridge's own LDP object ids (client-allocated). Public for
/// the process binary's event router (it watches the root surface and
/// toplevel for presented/configure routing).
pub mod ids {
    /// The pre-bound bootstrap connection object.
    pub const CONNECTION: u32 = 1;
    /// The registry.
    pub const REGISTRY: u32 = 2;
    /// The bound compositor global.
    pub const COMPOSITOR: u32 = 3;
    /// The bound shm global.
    pub const SHM: u32 = 4;
    /// The bound seat global.
    pub const SEAT: u32 = 5;
    /// The bound shell global.
    pub const SHELL: u32 = 6;
    /// The root surface.
    pub const SURFACE: u32 = 7;
    /// The shm pool.
    pub const POOL: u32 = 8;
    /// The single re-attached buffer.
    pub const BUFFER: u32 = 9;
    /// The rootful toplevel.
    pub const TOPLEVEL: u32 = 10;
}

/// Bridge-scope token validation seam (wired to the broker in
/// production, to the Phase 16 grant table in tests).
pub trait TokenCheck {
    /// Validate one token submission for `app_id`; the bridge runs
    /// only when this returns true (the `bridge` scope is granted).
    fn check(&mut self, app_id: &str, token: [u32; 8]) -> bool;
}

/// Process-binary seam for pool allocation and mirroring (the X11
/// face exports one root pool, so the identity is the fixed id the
/// driver mints — the trait matches the Wayland face's multi-pool
/// contract, so the process binary hosts both with one shape).
pub trait DriverHost {
    /// Allocate (or grow) the backing for pool `pool` at `size`
    /// bytes; returns the ancillary fd index the `shm.create_pool`
    /// message references.
    fn pool_fd(&mut self, pool: u32, size: u64) -> u32;
    /// Mirror `bytes` (the root store) into pool `pool`.
    fn sync_pool(&mut self, pool: u32, bytes: &[u8]);
}

/// A recording host for tests and the crate's own examples.
#[derive(Default)]
pub struct RecordingHost {
    /// Every pool allocation (id, size), in order.
    pub pools: Vec<(u32, u64)>,
    /// Every mirror sync (pool, byte length), in order.
    pub syncs: Vec<(u32, usize)>,
}

impl DriverHost for RecordingHost {
    fn pool_fd(&mut self, pool: u32, size: u64) -> u32 {
        self.pools.push((pool, size));
        0 // the first ancillary fd slot
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

/// The driver's lifecycle phase (shared by the rootful and
/// rootless drivers).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    /// No token accepted yet.
    Unauthenticated,
    /// Bootstrapped and committing frames.
    Running,
}

/// The rootful X-to-LDP proxy.
pub struct XBridgeDriver {
    server: Server,
    x_client: ClientId,
    phase: Phase,
    out: Vec<Message>,
    host: Box<dyn DriverHost>,
    /// The buffer geometry currently advertised to LDP.
    committed: (u32, u32),
    /// The commit-cookie counter (each commit carries one; the
    /// compositor's presented verdict rides it back).
    cookie: u32,
}

impl XBridgeDriver {
    /// A driver over a fresh server (the given screen) with no X
    /// clients attached yet.
    pub fn new(screen: ScreenParams) -> XBridgeDriver {
        let mut server = Server::new(screen, Box::new(NoShm));
        let x_client = server.add_client();
        XBridgeDriver {
            server,
            x_client,
            phase: Phase::Unauthenticated,
            out: Vec::new(),
            host: Box::new(RecordingHost::default()),
            committed: (0, 0),
            cookie: 0,
        }
    }

    /// Attach a custom SHM host (the process binary's real one).
    #[must_use]
    pub fn with_shm_host(mut self, host: Box<dyn ShmHost>) -> XBridgeDriver {
        self.server.set_shm_host(host);
        self
    }

    /// Attach a custom driver host (pool allocation seam).
    #[must_use]
    pub fn with_driver_host(mut self, host: Box<dyn DriverHost>) -> XBridgeDriver {
        self.host = host;
        self
    }

    /// The bridge-scope token gate: the driver refuses to emit any LDP
    /// message until a token passes the check seam.
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

    /// The bootstrap burst: hello, registry, the four binds, surface,
    /// pool, buffer, toplevel.
    fn bootstrap(&mut self) {
        let hello = Message::new(ids::CONNECTION, core::connection::request::HELLO)
            .arg(Value::Uint32(1))
            .arg(Value::Bitset(ldp_core::bitset::Bitset128::EMPTY));
        self.out.push(hello);
        let registry = Message::new(ids::CONNECTION, core::connection::request::GET_REGISTRY)
            .arg(Value::Uint32(1))
            .arg(Value::NewId(
                ldp_core::ids::ObjectId::client(ids::REGISTRY).expect("registry id"),
            ));
        self.out.push(registry);
        for (object, interface, version, id) in [
            (ids::REGISTRY, core::compositor::NAME, 1, ids::COMPOSITOR),
            (ids::REGISTRY, core::shm::NAME, 1, ids::SHM),
            (ids::REGISTRY, "ldp.input.seat", 1, ids::SEAT),
            (ids::REGISTRY, shell::shell::NAME, 1, ids::SHELL),
        ] {
            let bind = Message::new(object, 1)
                .arg(Value::String(interface.into()))
                .arg(Value::Uint32(version))
                .arg(Value::NewId(
                    ldp_core::ids::ObjectId::client(id).expect("bind id"),
                ));
            self.out.push(bind);
        }
        let surface = Message::new(ids::COMPOSITOR, core::compositor::request::CREATE_SURFACE).arg(
            Value::NewId(ldp_core::ids::ObjectId::client(ids::SURFACE).expect("surface id")),
        );
        self.out.push(surface);
        // Pool + buffer sized to the root store.
        let (w, h) = self.root_size();
        let stride = u64::from(w) * 4;
        let size = stride * u64::from(h);
        let fd = self.host.pool_fd(ids::POOL, size);
        let pool = Message::new(ids::SHM, core::shm::request::CREATE_POOL)
            .arg(Value::Fd(fd))
            .arg(Value::Int64(size as i64))
            .arg(Value::NewId(
                ldp_core::ids::ObjectId::client(ids::POOL).expect("pool id"),
            ));
        self.out.push(pool);
        self.emit_buffer(w, h, stride);
        // Client decorations: the bridge composites the whole X screen.
        let toplevel = Message::new(ids::SHELL, shell::shell::request::GET_TOPLEVEL)
            .arg(Value::Object(Some(
                ldp_core::ids::ObjectId::client(ids::SURFACE).expect("surface id"),
            )))
            .arg(Value::Enum(2)) // client decorations
            .arg(Value::NewId(
                ldp_core::ids::ObjectId::client(ids::TOPLEVEL).expect("toplevel id"),
            ));
        self.out.push(toplevel);
        if let Some(title) = self.window_title_best() {
            let set_title = Message::new(ids::TOPLEVEL, shell::toplevel::request::SET_TITLE)
                .arg(Value::String(title));
            self.out.push(set_title);
        }
        self.committed = (w, h);
    }

    /// The best available ICCCM title across the top-level X windows.
    fn window_title_best(&self) -> Option<Box<str>> {
        for &w in self.server.top_level_windows().iter().rev() {
            if let Some(t) = self.server.window_title(w) {
                return Some(t);
            }
        }
        None
    }

    /// Emit `shm_pool.create_buffer` for the given geometry.
    fn emit_buffer(&mut self, w: u32, h: u32, stride: u64) {
        let buffer = Message::new(ids::POOL, core::shm_pool::request::CREATE_BUFFER)
            .arg(Value::Int32(0))
            .arg(Value::Int32(w as i32))
            .arg(Value::Int32(h as i32))
            .arg(Value::Int32(stride as i32))
            .arg(Value::Uint32(ldp_core::buffer::FourCC::XRGB8888.code()))
            .arg(Value::NewId(
                ldp_core::ids::ObjectId::client(ids::BUFFER).expect("buffer id"),
            ));
        self.out.push(buffer);
    }

    /// The root store's current size (or the screen's, before the
    /// first frame creates the store).
    fn root_size(&self) -> (u32, u32) {
        if let Some(s) = self.server.root_store() {
            return (s.width, s.height);
        }
        let sc = self.server.screen();
        (u32::from(sc.width), u32::from(sc.height))
    }

    // ---- X side ----

    /// Feed bytes from the X socket into the server (one connection —
    /// the rootful Phase 17 shape; more attach through the server
    /// directly).
    ///
    /// # Errors
    /// The server's fatal reasons (handshake or wire violations): the
    /// process must close the X connection.
    pub fn x_feed(&mut self, bytes: &[u8]) -> Result<(), crate::dispatch::FatalReason> {
        self.server.feed(self.x_client, bytes)
    }

    /// Take the pending X-bound output (replies, events, errors).
    pub fn x_output(&mut self) -> Vec<u8> {
        self.server.take_output(self.x_client)
    }

    /// Direct access to the X server state (multi-client processes
    /// attach further connections here).
    pub fn server_mut(&mut self) -> &mut Server {
        &mut self.server
    }

    /// The bridge's own X connection handle.
    #[must_use]
    pub fn x_client(&self) -> ClientId {
        self.x_client
    }

    // ---- LDP side ----

    /// Take the pending LDP messages (the process writes them to the
    /// display server socket).
    pub fn take_ldp_messages(&mut self) -> Vec<Message> {
        std::mem::take(&mut self.out)
    }

    /// A frame tick: mirror the root store into the pool and commit
    /// with the accumulated damage. Emits nothing when there is no
    /// damage or no root store yet (the bridge never commits blank
    /// frames — LDP commits are presentation events, not keepalives).
    ///
    /// # Panics
    /// Never in practice: the client object ids are compile-time
    /// constants inside the valid client range.
    pub fn on_frame_tick(&mut self) {
        if self.phase != Phase::Running {
            return;
        }
        let Some(damage) = self.server.take_damage() else {
            return;
        };
        let Some(store) = self.server.root_store() else {
            return;
        };
        let (w, h) = (store.width, store.height);
        let store_bytes = store.data.clone();
        // Resize: grow the pool, mint a fresh buffer for the new
        // geometry (the old buffer is superseded by the attach).
        if (w, h) != self.committed {
            let stride = u64::from(w) * 4;
            let size = stride * u64::from(h);
            let resize = Message::new(ids::POOL, core::shm_pool::request::RESIZE)
                .arg(Value::Int64(size as i64));
            self.out.push(resize);
            let _ = self.host.pool_fd(ids::POOL, size); // the host learns the new size
            self.emit_buffer(w, h, stride);
            self.committed = (w, h);
        }
        self.host.sync_pool(ids::POOL, &store_bytes);
        let attach = Message::new(ids::SURFACE, core::surface::request::ATTACH).arg(Value::Object(
            Some(ldp_core::ids::ObjectId::client(ids::BUFFER).expect("buffer id")),
        ));
        self.out.push(attach);
        let damage_msg = Message::new(ids::SURFACE, core::surface::request::DAMAGE).arg(
            Value::array(
                ldp_core::wire::ArgType::Rect,
                vec![ldp_core::wire::Primitive::Rect(damage)],
            )
            .expect("damage rect array"),
        );
        self.out.push(damage_msg);
        // Implicit-to-explicit sync: the requests are fully applied —
        // no acquire fence rides the commit (readiness is stated by
        // the commit itself, the explicit form). The cookie rides for
        // the presented correlation.
        let commit = Message::new(ids::SURFACE, core::surface::request::COMMIT)
            .arg(Value::Uint32(self.cookie.wrapping_add(1)));
        self.cookie = self.cookie.wrapping_add(1);
        self.out.push(commit);
    }

    /// The LDP toplevel was presented (the release side of the sync
    /// translation; a pure CPU bridge has no fence to signal).
    pub fn on_presented(&mut self) {
        // Nothing to retire: the single buffer re-attaches per frame.
    }

    /// An LDP toplevel configure: the rootful surface resizes; every
    /// mapped X window is re-exposed (they repaint through Expose).
    /// A 0x0 proposal is the shell's *initial* configure — "the
    /// client picks its own size" (the xdg doctrine) — and the
    /// rootful bridge's answer is the operator's `--screen WxH`: it
    /// is not a resize to nothing.
    pub fn on_ldp_configure(&mut self, width: u32, height: u32) {
        if self.phase != Phase::Running {
            return;
        }
        if width == 0 || height == 0 {
            return;
        }
        self.server.resize_root(width, height);
    }

    /// An LDP toplevel close: the ICCCM dance on the topmost mapped
    /// top-level X window (`WM_DELETE_WINDOW` when listed, destroy
    /// otherwise).
    pub fn on_ldp_close(&mut self, time: u32) -> ClosePolicy {
        let tops = self.server.top_level_windows();
        let target = tops.last().copied().unwrap_or(0);
        if target == 0 {
            return ClosePolicy::NoSuchWindow;
        }
        self.server.request_close(target, time)
    }

    // ---- input translation (LDP events in, X events out) ----

    /// An LDP pointer motion (surface coordinates = root coordinates
    /// in the rootful shape).
    pub fn ldp_pointer_motion(&mut self, x: i32, y: i32, time: u32) {
        self.server.pointer_move(x, y, time);
    }

    /// An LDP pointer button.
    pub fn ldp_pointer_button(&mut self, button: u8, pressed: bool, time: u32) {
        self.server.pointer_button(button, pressed, time);
    }

    /// An LDP key event. LDP keycodes are evdev; X keycodes are
    /// evdev + 8.
    pub fn ldp_key(&mut self, evdev_keycode: u32, pressed: bool, time: u32) {
        let x_keycode = u8::try_from(evdev_keycode.saturating_add(8)).unwrap_or(0);
        if x_keycode == 0 {
            return;
        }
        self.server.key(x_keycode, pressed, time);
    }

    /// An LDP keyboard enter/leave: focus follows into the rootful
    /// surface (the X focus moves to the pointer window).
    pub fn ldp_keyboard_focus(&mut self, focused: bool, time: u32) {
        let target = if focused {
            Focus::PointerRoot
        } else {
            Focus::None
        };
        self.server.set_focus(target, time);
    }

    /// Root damage accumulated right now (test visibility).
    #[must_use]
    pub fn pending_damage(&self) -> Option<Rect> {
        self.server.damage_now()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::mask;

    fn screen() -> ScreenParams {
        ScreenParams::default()
    }

    /// A token check that accepts exactly one token.
    struct OneToken(Option<[u32; 8]>);
    impl TokenCheck for OneToken {
        fn check(&mut self, _app: &str, token: [u32; 8]) -> bool {
            self.0 == Some(token)
        }
    }

    fn handshake() -> Vec<u8> {
        let mut b = vec![0x6c, 0];
        b.extend_from_slice(&11u16.to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes());
        b.extend_from_slice(&[0; 6]);
        b
    }

    fn request(opcode: u8, payload: &[u8]) -> Vec<u8> {
        let mut r = vec![opcode, 0];
        // Length counts 4-byte units including this 4-byte header.
        let units = 1 + payload.len().div_ceil(4);
        r.extend_from_slice(&(units as u16).to_le_bytes());
        r.extend_from_slice(payload);
        while r.len() % 4 != 0 {
            r.push(0);
        }
        r
    }

    #[test]
    fn unauthenticated_driver_is_silent() {
        let mut d = XBridgeDriver::new(screen());
        d.on_frame_tick();
        assert!(d.take_ldp_messages().is_empty());
        let mut check = OneToken(None);
        assert_eq!(
            d.authenticate("bridge", [1; 8], &mut check),
            Err(AuthError::Rejected)
        );
        assert!(d.take_ldp_messages().is_empty());
    }

    #[test]
    fn authentication_gates_the_bootstrap() {
        let mut d = XBridgeDriver::new(screen());
        let mut check = OneToken(Some([9; 8]));
        d.authenticate("lion-x-bridge", [9; 8], &mut check).unwrap();
        let msgs = d.take_ldp_messages();
        // hello, get_registry, 4 binds, create_surface, create_pool,
        // create_buffer, get_toplevel — in order.
        assert_eq!(msgs.len(), 10);
        assert_eq!(msgs[0].opcode, core::connection::request::HELLO);
        assert_eq!(msgs[1].opcode, core::connection::request::GET_REGISTRY);
        assert_eq!(msgs[2].args[0], Value::String("ldp.core.compositor".into()));
        assert_eq!(msgs[6].opcode, core::compositor::request::CREATE_SURFACE);
        assert_eq!(msgs[7].opcode, core::shm::request::CREATE_POOL);
        assert_eq!(msgs[8].opcode, core::shm_pool::request::CREATE_BUFFER);
        assert_eq!(msgs[9].opcode, shell::shell::request::GET_TOPLEVEL);
        // The pool is screen-sized: 1024*768*4.
        assert_eq!(msgs[7].args[1], Value::Int64((1024 * 768 * 4) as i64));
    }

    #[test]
    fn double_authentication_rejects() {
        let mut d = XBridgeDriver::new(screen());
        let mut check = OneToken(Some([1; 8]));
        d.authenticate("app", [1; 8], &mut check).unwrap();
        assert_eq!(
            d.authenticate("app", [1; 8], &mut check),
            Err(AuthError::AlreadyRunning)
        );
    }

    #[test]
    fn the_initial_zero_configure_is_not_a_root_resize() {
        // The shell's initial configure carries 0x0 — "the client
        // picks its own size" (the xdg doctrine). The rootful bridge's
        // answer is the operator's screen: a 0x0 proposal must not
        // collapse the root (the store would empty, the pool would
        // try to shrink, the session would die).
        let mut d = XBridgeDriver::new(screen());
        let mut check = OneToken(Some([1; 8]));
        d.authenticate("app", [1; 8], &mut check).unwrap();
        let before = d.root_size();
        assert!(before.0 > 0 && before.1 > 0, "the screen is real");
        d.on_ldp_configure(0, 0);
        assert_eq!(d.root_size(), before, "a 0x0 configure keeps the screen");
        // A real proposal (a window manager resizing the toplevel)
        // still resizes the root.
        d.on_ldp_configure(800, 600);
        assert_eq!(d.root_size(), (800, 600), "a real configure resizes");
    }

    #[test]
    fn frame_tick_commits_damage_after_drawing() {
        let mut d = XBridgeDriver::new(screen());
        let mut check = OneToken(Some([1; 8]));
        d.authenticate("app", [1; 8], &mut check).unwrap();
        d.take_ldp_messages();
        // An X client appears: window + map + one filled rectangle.
        let mut cx = handshake();
        let mut attrs = Vec::new();
        attrs.extend_from_slice(&0x41u32.to_le_bytes()); // wid
        attrs.extend_from_slice(&0x40u32.to_le_bytes()); // parent = root
        attrs.extend_from_slice(&10i16.to_le_bytes());
        attrs.extend_from_slice(&10i16.to_le_bytes());
        attrs.extend_from_slice(&64u16.to_le_bytes());
        attrs.extend_from_slice(&64u16.to_le_bytes());
        attrs.extend_from_slice(&0u16.to_le_bytes()); // border
        attrs.extend_from_slice(&1u8.to_le_bytes()); // InputOutput
        attrs.extend(&[0, 0]); // pad + visual low bytes
                               // (visual is a full u32; rebuild properly below)
        attrs.clear();
        attrs.extend_from_slice(&0x41u32.to_le_bytes());
        attrs.extend_from_slice(&0x40u32.to_le_bytes());
        attrs.extend_from_slice(&10i16.to_le_bytes());
        attrs.extend_from_slice(&10i16.to_le_bytes());
        attrs.extend_from_slice(&64u16.to_le_bytes());
        attrs.extend_from_slice(&64u16.to_le_bytes());
        attrs.extend_from_slice(&0u16.to_le_bytes());
        attrs.extend(&[1u8, 0]);
        attrs.extend_from_slice(&0x21u32.to_le_bytes()); // visual
        let mask: u32 = (1 << 1) | (1 << 8); // back-pixel + event-mask
        attrs.extend_from_slice(&mask.to_le_bytes());
        attrs.extend_from_slice(&0x00ff_ff00u32.to_le_bytes()); // back pixel
        attrs.extend_from_slice(&mask::EXPOSURE.to_le_bytes());
        cx.extend_from_slice(&request(1, &attrs));
        // MapWindow.
        let mut map = Vec::new();
        map.extend_from_slice(&0x41u32.to_le_bytes());
        cx.extend_from_slice(&request(8, &map));
        // CreateGC.
        let mut gc = Vec::new();
        gc.extend_from_slice(&0x50u32.to_le_bytes()); // cid
        gc.extend_from_slice(&0x41u32.to_le_bytes()); // drawable
        let gmask: u32 = 1 << 2; // foreground
        gc.extend_from_slice(&gmask.to_le_bytes());
        gc.extend_from_slice(&0x00ff_0000u32.to_le_bytes());
        cx.extend_from_slice(&request(55, &gc));
        // PolyFillRectangle: 8x8 at (4, 4).
        let mut fr = Vec::new();
        fr.extend_from_slice(&0x41u32.to_le_bytes());
        fr.extend_from_slice(&0x50u32.to_le_bytes());
        fr.extend_from_slice(&4i16.to_le_bytes());
        fr.extend_from_slice(&4i16.to_le_bytes());
        fr.extend_from_slice(&8u16.to_le_bytes());
        fr.extend_from_slice(&8u16.to_le_bytes());
        cx.extend_from_slice(&request(70, &fr));
        d.x_feed(&cx).unwrap();
        let _ = d.x_output();
        // A frame tick: attach + damage + commit.
        d.on_frame_tick();
        let msgs = d.take_ldp_messages();
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0].opcode, core::surface::request::ATTACH);
        assert_eq!(msgs[1].opcode, core::surface::request::DAMAGE);
        assert_eq!(msgs[2].opcode, core::surface::request::COMMIT);
        // The damage is the drawn + exposed area, not the whole screen.
        match &msgs[1].args[0] {
            Value::Array { items, .. } => {
                assert_eq!(items.len(), 1);
                match &items[0] {
                    ldp_core::wire::Primitive::Rect(r) => {
                        assert_eq!((r.x, r.y, r.w, r.h), (10, 10, 64, 64));
                    }
                    other => panic!("unexpected damage element {other:?}"),
                }
            }
            other => panic!("unexpected damage arg {other:?}"),
        }
        // A second tick with no drawing commits nothing.
        d.on_frame_tick();
        assert!(d.take_ldp_messages().is_empty());
    }

    #[test]
    fn close_routes_to_the_wm_protocol() {
        let mut d = XBridgeDriver::new(screen());
        let mut check = OneToken(Some([1; 8]));
        d.authenticate("app", [1; 8], &mut check).unwrap();
        // Intern WM_PROTOCOLS/WM_DELETE_WINDOW and set the property,
        // then map the window.
        let mut cx = handshake();
        // InternAtom "WM_PROTOCOLS": name-len(2), unused(2), name.
        let mut ia = Vec::new();
        ia.extend_from_slice(&12u16.to_le_bytes());
        ia.extend_from_slice(&[0; 2]);
        ia.extend_from_slice(b"WM_PROTOCOLS");
        cx.extend_from_slice(&request(16, &ia));
        d.x_feed(&cx).unwrap();
        let out = d.x_output();
        // The atom reply follows the 140-byte setup reply: dynamic
        // atom 69 for WM_PROTOCOLS.
        let reply = &out[out.len() - 32..];
        assert_eq!(reply[0], 1);
        assert_eq!(
            u32::from_le_bytes([reply[8], reply[9], reply[10], reply[11]]),
            69
        );
        // WM_DELETE_WINDOW -> atom 70.
        let mut cx2 = Vec::new();
        let mut ia2 = Vec::new();
        ia2.extend_from_slice(&16u16.to_le_bytes());
        ia2.extend_from_slice(&[0; 2]);
        ia2.extend_from_slice(b"WM_DELETE_WINDOW");
        cx2.extend_from_slice(&request(16, &ia2));
        d.x_feed(&cx2).unwrap();
        let out2 = d.x_output();
        assert_eq!(
            u32::from_le_bytes([out2[8], out2[9], out2[10], out2[11]]),
            70
        );
        // Window with the protocols property.
        let mut cx3 = Vec::new();
        let mut attrs = Vec::new();
        attrs.extend_from_slice(&0x41u32.to_le_bytes());
        attrs.extend_from_slice(&0x40u32.to_le_bytes());
        attrs.extend_from_slice(&0i16.to_le_bytes());
        attrs.extend_from_slice(&0i16.to_le_bytes());
        attrs.extend_from_slice(&32u16.to_le_bytes());
        attrs.extend_from_slice(&32u16.to_le_bytes());
        attrs.extend_from_slice(&0u16.to_le_bytes());
        attrs.extend(&[1u8, 0]);
        attrs.extend_from_slice(&0x21u32.to_le_bytes());
        attrs.extend_from_slice(&0u32.to_le_bytes()); // no attrs
        cx3.extend_from_slice(&request(1, &attrs));
        let mut map = Vec::new();
        map.extend_from_slice(&0x41u32.to_le_bytes());
        cx3.extend_from_slice(&request(8, &map));
        // ChangeProperty: WM_PROTOCOLS = [70].
        let mut cp = vec![0u8, 0, 0, 0];
        cp.extend_from_slice(&0x41u32.to_le_bytes());
        cp.extend_from_slice(&69u32.to_le_bytes());
        cp.extend_from_slice(&4u32.to_le_bytes()); // ATOM
        cp.extend(&[32u8, 0, 0, 0]);
        cp.extend_from_slice(&1u32.to_le_bytes());
        cp.extend_from_slice(&70u32.to_le_bytes());
        cx3.extend_from_slice(&request(18, &cp[4..]));
        d.x_feed(&cx3).unwrap();
        let _ = d.x_output();
        // Close: the ClientMessage arrives (code 32, format 32).
        assert_eq!(d.on_ldp_close(1234), ClosePolicy::ProtocolMessage);
        let ev = d.x_output();
        assert_eq!(ev[0], 32);
        assert_eq!(ev[1], 32); // format
        assert_eq!(u32::from_le_bytes([ev[4], ev[5], ev[6], ev[7]]), 0x41);
        assert_eq!(u32::from_le_bytes([ev[8], ev[9], ev[10], ev[11]]), 69);
        assert_eq!(u32::from_le_bytes([ev[12], ev[13], ev[14], ev[15]]), 70);
        assert_eq!(u32::from_le_bytes([ev[16], ev[17], ev[18], ev[19]]), 1234);
    }

    #[test]
    fn ldp_key_translates_evdev_to_x_keycodes() {
        let mut d = XBridgeDriver::new(screen());
        let mut check = OneToken(Some([1; 8]));
        d.authenticate("app", [1; 8], &mut check).unwrap();
        d.ldp_keyboard_focus(true, 100);
        // A window selecting key events, under the pointer.
        let mut cx = handshake();
        let mut attrs = Vec::new();
        attrs.extend_from_slice(&0x41u32.to_le_bytes());
        attrs.extend_from_slice(&0x40u32.to_le_bytes());
        attrs.extend_from_slice(&0i16.to_le_bytes());
        attrs.extend_from_slice(&0i16.to_le_bytes());
        attrs.extend_from_slice(&100u16.to_le_bytes());
        attrs.extend_from_slice(&100u16.to_le_bytes());
        attrs.extend_from_slice(&0u16.to_le_bytes());
        attrs.extend(&[1u8, 0]);
        attrs.extend_from_slice(&0x21u32.to_le_bytes());
        let mask: u32 = 1 << 8; // event-mask
        attrs.extend_from_slice(&mask.to_le_bytes());
        attrs.extend_from_slice(&(mask::KEY_PRESS | mask::KEY_RELEASE).to_le_bytes());
        cx.extend_from_slice(&request(1, &attrs));
        let mut map = Vec::new();
        map.extend_from_slice(&0x41u32.to_le_bytes());
        cx.extend_from_slice(&request(8, &map));
        d.x_feed(&cx).unwrap();
        let _ = d.x_output();
        // evdev KEY_Q (16) -> X keycode 24.
        d.ldp_key(16, true, 200);
        let ev = d.x_output();
        assert_eq!(ev[0], 2); // KeyPress
        assert_eq!(ev[1], 24); // keycode
                               // evdev KEY_LEFTSHIFT (42) -> X 50: state carries Shift after.
        d.ldp_key(42, true, 300);
        d.ldp_key(30, true, 400); // KEY_A -> X 38
        let ev = d.x_output();
        // Two events: shift press, then 'a' press with state 0x1.
        let shift = &ev[..32];
        let a = &ev[32..];
        assert_eq!(shift[1], 50);
        assert_eq!(a[1], 38);
        let state = u16::from_le_bytes([a[30], a[31]]);
        assert_eq!(state, 0x0001);
    }
}
