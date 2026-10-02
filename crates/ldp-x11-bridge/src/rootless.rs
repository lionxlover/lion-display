//! The rootless X-to-LDP proxy: every top-level X window rides its
//! own LDP surface.
//!
//! The rootful driver ([`crate::driver`]) exports the whole X screen
//! as one window — one toplevel, one buffer, the X server's composite
//! shipped whole. The rootless driver splits at the natural seam:
//!
//! * **The export set** is the X tree's root children — every mapped
//!   InputOutput window whose parent is the root (override-redirect
//!   or not). Each becomes one LDP surface: a toplevel for the
//!   managed windows, a *popup* for the override-redirect ones
//!   (menus and tooltips the X protocol exempts from management —
//!   the anchor/gravity role's exact client). Windows deeper in a
//!   subtree stay where the X protocol puts them: painted into
//!   their top-level's export by [`Server::window_frame`] (the
//!   subtree composite), exactly as they paint into the rootful
//!   root store.
//! * **One pool, per-window buffers**: a single `shm` pool backs
//!   every export, each window's pixels at its own offset — one
//!   descriptor, N `create_buffer` views, the offsets re-laid when
//!   the window set or any geometry changes.
//! * **Per-window damage**: the server's root damage distributed
//!   across the export set ([`Server::take_top_damage`]), each
//!   window committing only its own dirty rect — the same union the
//!   rootful driver commits whole.
//! * **Two coordinate systems, one truth each.** The X protocol's
//!   geometry is the X clients' truth (ConfigureNotify answers from
//!   the X tree; the bridge never moves an X window — it is not a
//!   window manager). The LDP display's placement is the screen's
//!   truth (the positioning shell places each toplevel; the popup
//!   solver anchors OR windows relative to the screen). Input
//!   translation bridges them the honest way: LDP pointer events
//!   are *surface-local*, surface-local is export-local, and
//!   export-local + the X window's own absolute origin = the X root
//!   coordinates the X protocol reports. A click hits the pixel the
//!   user sees; the X client sees the click at its own geometry.
//!   The root window itself is not exported (the XQuartz-rootless
//!   doctrine: no root-window content; the LDP desktop's own
//!   background shows).
//!
//! Everything else is the rootful driver's doctrine carried over:
//! the bridge-scope token gate, the implicit-to-explicit sync
//! translation (requests complete before the commit is emitted), the
//! `DriverHost` pool seam, no `unsafe`, no sockets, no clocks.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use ldp_core::bitset::Bitset128;
use ldp_core::geometry::Rect;
use ldp_core::wire::Value;
use ldp_protocol::generated::{core, shell};
use ldp_protocol::Message;

use crate::dispatch::{ClientId, ClosePolicy, Server};
use crate::driver::{AuthError, DriverHost, Phase, RecordingHost, TokenCheck};
use crate::setup::ScreenParams;
use crate::shm::{NoShm, ShmHost};

/// The bridge's own fixed LDP object ids (client-allocated). The
/// rootless shape: the pool is the one shared object; every window's
/// surface/buffer/role mints from [`ids::FIRST_FREE`] upward.
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
    /// The one shared pool.
    pub const POOL: u32 = 8;
    /// The first client-allocated id above the fixed set.
    pub const FIRST_FREE: u32 = 11;
}

/// One exported X window: its LDP surface, buffer view, role object,
/// and pool slot.
#[derive(Debug)]
struct Export {
    /// The `ldp.core.surface` object.
    surface: u32,
    /// The current `ldp.core.buffer` object (re-minted on geometry
    /// or pool-layout change).
    buffer: u32,
    /// The role: `Toplevel { id }` or `Popup { id }`.
    role: Role,
    /// The window's byte offset in the pool.
    offset: u64,
    /// The geometry the buffer was minted for.
    committed: (u32, u32),
}

/// The role an export carries.
#[derive(Debug)]
enum Role {
    /// A managed window: `get_toplevel`, client decorations.
    Toplevel {
        /// The `ldp.shell.toplevel` object.
        id: u32,
    },
    /// An override-redirect window: `get_popup` anchored at its own
    /// screen rect (the parent-null doctrine — the anchor lives on
    /// the output).
    Popup {
        /// The `ldp.shell.popup` object.
        id: u32,
    },
}

/// Which role an export's watch entry names (the link's routing
/// tables are role-specific: toplevels and popups carry different
/// event vocabularies).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WatchKind {
    /// A managed window's `ldp.shell.toplevel`.
    Toplevel,
    /// An override-redirect window's `ldp.shell.popup`.
    Popup,
}

/// The rootless X-to-LDP proxy.
pub struct RootlessDriver {
    server: Server,
    phase: Phase,
    out: Vec<Message>,
    host: Box<dyn DriverHost>,
    /// The commit-cookie counter.
    cookie: u32,
    /// The next client-allocated object id.
    next_id: u32,
    /// X window → export.
    windows: BTreeMap<u32, Export>,
    /// The pool's committed size (`0`: not yet created).
    pool_size: u64,
    /// The pool mirror (reused; the whole pool's bytes).
    mirror: Vec<u8>,
    /// Per-window pending damage (window-local), accumulated between
    /// ticks and committed on the next.
    pending: BTreeMap<u32, Rect>,
}

impl RootlessDriver {
    /// A driver over a fresh server (the given screen) with no X
    /// clients attached yet.
    pub fn new(screen: ScreenParams) -> RootlessDriver {
        let mut server = Server::new(screen, Box::new(NoShm));
        // The rootless driver never feeds its own X connection; the
        // process's clients attach through the server directly.
        let _ = server.add_client();
        RootlessDriver {
            server,
            phase: Phase::Unauthenticated,
            out: Vec::new(),
            host: Box::new(RecordingHost::default()),
            cookie: 0,
            next_id: ids::FIRST_FREE,
            windows: BTreeMap::new(),
            pool_size: 0,
            mirror: Vec::new(),
            pending: BTreeMap::new(),
        }
    }

    /// Attach a custom SHM host (the process binary's real one).
    #[must_use]
    pub fn with_shm_host(mut self, host: Box<dyn ShmHost>) -> RootlessDriver {
        self.server.set_shm_host(host);
        self
    }

    /// Attach the driver host (the pool seam).
    #[must_use]
    pub fn with_driver_host(mut self, host: Box<dyn DriverHost>) -> RootlessDriver {
        self.host = host;
        self
    }

    /// The bridge-scope token gate (the rootful driver's doctrine).
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

    /// The bootstrap burst: hello, registry, the four binds. The pool
    /// mints with the first exported window (a windowless X desktop
    /// commits nothing — the honest dark state).
    fn bootstrap(&mut self) {
        let hello = Message::new(ids::CONNECTION, core::connection::request::HELLO)
            .arg(Value::Uint32(1))
            .arg(Value::Bitset(Bitset128::EMPTY));
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
    }

    // ---- X side ----------------------------------------------------

    /// Feed bytes from one X client's socket into the server.
    ///
    /// # Errors
    /// The server's fatal reasons (handshake or wire violations): the
    /// process must close that X connection.
    pub fn x_feed(
        &mut self,
        client: ClientId,
        bytes: &[u8],
    ) -> Result<(), crate::dispatch::FatalReason> {
        self.server.feed(client, bytes)
    }

    /// Take one client's pending X-bound output.
    pub fn x_output(&mut self, client: ClientId) -> Vec<u8> {
        self.server.take_output(client)
    }

    /// Direct access to the X server state (the process attaches and
    /// removes connections here).
    pub fn server_mut(&mut self) -> &mut Server {
        &mut self.server
    }

    // ---- LDP side --------------------------------------------------

    /// Take the pending LDP messages (the process writes them to the
    /// display server socket).
    pub fn take_ldp_messages(&mut self) -> Vec<Message> {
        std::mem::take(&mut self.out)
    }

    /// Every export's LDP object ids — the process registers them
    /// with the link for event routing (the rootless id plan is
    /// dynamic; this is the link's watch feed). `(kind, role id,
    /// surface id)` per export.
    #[must_use]
    pub fn watch_ids(&self) -> Vec<(WatchKind, u32, u32)> {
        self.windows
            .values()
            .map(|e| match e.role {
                Role::Toplevel { id } => (WatchKind::Toplevel, id, e.surface),
                Role::Popup { id } => (WatchKind::Popup, id, e.surface),
            })
            .collect()
    }

    /// The frame tick: distribute the server's damage across the
    /// export set, sync the window set (minting and tearing down
    /// surfaces as X windows come and go), re-lay the pool if the
    /// set or any geometry changed, and commit every window with its
    /// own damage. Emits nothing when the desktop is still (the
    /// never-commit-blank doctrine).
    ///
    /// # Panics
    /// Never in practice: every id is allocated from the legal
    /// client range and every geometry is validated by the pool's
    /// own buffer checks.
    pub fn on_frame_tick(&mut self) {
        if self.phase != Phase::Running {
            return;
        }
        // 1. Distribute the server's root damage window-locally.
        for (window, local) in self.server.take_top_damage() {
            let entry = self.pending.entry(window).or_insert(Rect::new(0, 0, 0, 0));
            *entry = entry.union(local);
        }
        // 2. Sync the export set with the X tree's root children.
        self.sync_windows();
        // 3. Re-lay the pool when the set or geometry changed.
        self.relayout_pool();
        // 4. Commit every window with pending damage.
        let mut commits: Vec<(u32, Rect)> = Vec::new();
        for (&window, damage) in &self.pending {
            let Some(export) = self.windows.get(&window) else {
                continue;
            };
            let (w, h) = export.committed;
            if w == 0 || h == 0 {
                continue;
            }
            if let Some(d) = damage.intersect(Rect::new(0, 0, w, h)) {
                commits.push((window, d));
            }
        }
        let mut mirrored = false;
        for (window, damage) in commits {
            if self.commit_window(window, damage) {
                mirrored = true;
            }
        }
        self.pending.clear();
        // The pool mirror rides the host seam once per tick when any
        // window committed (the host's positioned write of the whole
        // assembled pool).
        if mirrored {
            self.host.sync_pool(ids::POOL, &self.mirror);
        }
    }

    /// Sync the export set with the server's root children: mint
    /// surfaces for new windows, tear down the gone ones.
    fn sync_windows(&mut self) {
        let live: Vec<u32> = self.server.top_level_windows();
        // Tear down: windows that unmapped or died.
        let gone: Vec<u32> = self
            .windows
            .keys()
            .filter(|w| !live.contains(w))
            .copied()
            .collect();
        for window in gone {
            self.teardown_window(window);
        }
        // Mint: newly mapped root children, in stacking order (the
        // LDP surface order follows the X map order — the stacking
        // doctrine v1: creation order; explicit X restacks are the
        // window-management roadmap line).
        for window in live {
            if self.windows.contains_key(&window) {
                continue;
            }
            self.mint_window(window);
        }
    }

    /// Mint one window's export: surface, role (toplevel for managed
    /// windows, popup anchored at its own screen rect for
    /// override-redirect ones), and the ICCCM title when one exists.
    fn mint_window(&mut self, window: u32) {
        let (or, root_rect, title) = {
            let Some(w) = self.server.window_ref(window) else {
                return;
            };
            (
                w.override_redirect,
                self.server.window_root_rect(window),
                self.server.window_title(window),
            )
        };
        let surface = self.alloc_id();
        let create = Message::new(ids::COMPOSITOR, core::compositor::request::CREATE_SURFACE).arg(
            Value::NewId(ldp_core::ids::ObjectId::client(surface).expect("surface id")),
        );
        self.out.push(create);
        let buffer = self.alloc_id();
        if or {
            // The popup role: anchored at the OR window's own screen
            // rect (parent null — the anchor lives on the output),
            // growing down-right, every constraint strategy granted.
            // The solver's unconstrained answer *is* the X geometry;
            // the constraints only catch the off-screen tail.
            let popup = self.alloc_id();
            let get_popup = Message::new(ids::SHELL, shell::shell::request::GET_POPUP)
                .arg(Value::Object(Some(
                    ldp_core::ids::ObjectId::client(surface).expect("surface id"),
                )))
                .arg(Value::Object(None))
                .arg(Value::Rect(root_rect))
                .arg(Value::Enum(1)) // anchor: top_left
                .arg(Value::Enum(5)) // gravity: bottom_right
                .arg(Value::Int32(0))
                .arg(Value::Int32(0))
                .arg(Value::Bitset(
                    Bitset128::single(0).with(1).with(2).with(3).with(4).with(5),
                ))
                .arg(Value::NewId(
                    ldp_core::ids::ObjectId::client(popup).expect("popup id"),
                ));
            self.out.push(get_popup);
            self.windows.insert(
                window,
                Export {
                    surface,
                    buffer,
                    role: Role::Popup { id: popup },
                    offset: 0,
                    committed: (0, 0),
                },
            );
        } else {
            let toplevel = self.alloc_id();
            let get_toplevel = Message::new(ids::SHELL, shell::shell::request::GET_TOPLEVEL)
                .arg(Value::Object(Some(
                    ldp_core::ids::ObjectId::client(surface).expect("surface id"),
                )))
                .arg(Value::Enum(2)) // client decorations
                .arg(Value::NewId(
                    ldp_core::ids::ObjectId::client(toplevel).expect("toplevel id"),
                ));
            self.out.push(get_toplevel);
            if let Some(title) = title {
                let set_title = Message::new(toplevel, shell::toplevel::request::SET_TITLE)
                    .arg(Value::String(title));
                self.out.push(set_title);
            }
            self.windows.insert(
                window,
                Export {
                    surface,
                    buffer,
                    role: Role::Toplevel { id: toplevel },
                    offset: 0,
                    committed: (0, 0),
                },
            );
        }
    }

    /// Tear down one window's export: destroy the role, the buffer,
    /// and the surface (the generic `connection.destroy` path — the
    /// objects were client-minted, the client retires them).
    fn teardown_window(&mut self, window: u32) {
        let Some(export) = self.windows.remove(&window) else {
            return;
        };
        self.pending.remove(&window);
        let role = match export.role {
            Role::Toplevel { id } | Role::Popup { id } => id,
        };
        for object in [role, export.buffer, export.surface] {
            self.emit_destroy(object);
        }
    }

    /// Emit `connection.destroy(object, cookie)`.
    fn emit_destroy(&mut self, object: u32) {
        self.cookie = self.cookie.wrapping_add(1);
        let destroy = Message::new(ids::CONNECTION, core::connection::request::DESTROY)
            .arg(Value::Uint32(object))
            .arg(Value::Uint32(self.cookie));
        self.out.push(destroy);
    }

    /// Re-lay the pool over the current export set: offsets packed in
    /// stacking order, the pool grown (or created) to fit, the
    /// buffers whose offset or geometry changed re-minted.
    fn relayout_pool(&mut self) {
        // The desired layout: stacking order (the server's root
        // children order), 4-byte-aligned offsets.
        let live: Vec<u32> = self.server.top_level_windows();
        let mut offset: u64 = 0;
        let mut changed = false;
        let mut layout: Vec<(u32, u64, u32, u32)> = Vec::new();
        for window in &live {
            if !self.windows.contains_key(window) {
                continue;
            }
            let Some(w) = self.server.window_ref(*window) else {
                continue;
            };
            let (width, height) = (w.width, w.height);
            let stride = u64::from(width) * 4;
            let size = stride.saturating_mul(u64::from(height));
            layout.push((*window, offset, width, height));
            offset = offset.saturating_add(size);
            offset = (offset + 3) & !3; // 4-byte alignment
        }
        // What changed: geometry or offset moves.
        for (window, off, width, height) in &layout {
            if let Some(export) = self.windows.get_mut(window) {
                if export.committed != (*width, *height) || export.offset != *off {
                    changed = true;
                }
            } else {
                changed = true;
            }
        }
        if self.windows.len() != layout.len() {
            changed = true;
        }
        if !changed {
            return;
        }
        // The pool's new total: the last window's end.
        let new_size = layout.last().map_or(0, |(_, off, w, h)| {
            let stride = u64::from(*w) * 4;
            off.saturating_add(stride.saturating_mul(u64::from(*h)))
        });
        // The re-mint plan: which windows need a fresh buffer view
        // (geometry or offset moved). Computed read-only first — the
        // mint walk then mutates one field at a time.
        let mut plan: Vec<(u32, u64, u32, u32)> = Vec::new();
        for (window, off, width, height) in &layout {
            let fresh = match self.windows.get(window) {
                Some(e) => e.committed != (*width, *height) || e.offset != *off,
                None => true,
            };
            if fresh {
                plan.push((*window, *off, *width, *height));
            } else if let Some(e) = self.windows.get_mut(window) {
                e.offset = *off;
            }
        }
        // The pool first (a buffer view cannot precede its pool on
        // the wire): created at the first window, grown after — and
        // **never shrunk** (the server's pools are high-water by
        // design: a shrink is a protocol error; the offsets re-pack
        // into the standing allocation, the tail simply goes unused).
        let target = new_size.max(self.pool_size);
        if self.pool_size == 0 && target > 0 {
            let fd = self.host.pool_fd(ids::POOL, target);
            let create = Message::new(ids::SHM, core::shm::request::CREATE_POOL)
                .arg(Value::Fd(fd))
                .arg(Value::Int64(target as i64))
                .arg(Value::NewId(
                    ldp_core::ids::ObjectId::client(ids::POOL).expect("pool id"),
                ));
            self.out.push(create);
        } else if target > self.pool_size {
            let _ = self.host.pool_fd(ids::POOL, target);
            let resize = Message::new(ids::POOL, core::shm_pool::request::RESIZE)
                .arg(Value::Int64(target as i64));
            self.out.push(resize);
        }
        self.pool_size = target;
        // The mirror grows with the pool and **never re-zeros**: the
        // standing windows' bytes must survive an append — only the
        // freshly minted windows re-commit (their own regions), so a
        // zeroed mirror would blank every window that did not move
        // (the growth race the session suite caught: the newest
        // window renders, its elders go black).
        if target as usize > self.mirror.len() {
            self.mirror.resize(target as usize, 0);
        }
        // Then the buffer views over it.
        for (window, off, width, height) in plan {
            let old = self.windows.get(&window).map(|e| (e.buffer, e.committed));
            let buffer = self.alloc_id();
            let stride = u64::from(width) * 4;
            let create = Message::new(ids::POOL, core::shm_pool::request::CREATE_BUFFER)
                .arg(Value::Int32(i32::try_from(off).unwrap_or(i32::MAX)))
                .arg(Value::Int32(width as i32))
                .arg(Value::Int32(height as i32))
                .arg(Value::Int32(stride as i32))
                .arg(Value::Uint32(ldp_core::buffer::FourCC::XRGB8888.code()))
                .arg(Value::NewId(
                    ldp_core::ids::ObjectId::client(buffer).expect("buffer id"),
                ));
            self.out.push(create);
            if let Some((old_buffer, committed)) = old {
                if committed != (0, 0) {
                    self.emit_destroy(old_buffer);
                }
            }
            if let Some(e) = self.windows.get_mut(&window) {
                e.buffer = buffer;
                e.offset = off;
                e.committed = (width, height);
            }
            // A re-minted buffer must be re-attached with the
            // window's full damage (the next commit re-establishes
            // the whole window).
            let entry = self.pending.entry(window).or_insert(Rect::new(0, 0, 0, 0));
            *entry = entry.union(Rect::new(0, 0, width, height));
        }
    }

    /// Commit one window's frame: composite the subtree, mirror it
    /// into the pool at the window's offset, attach, damage, commit.
    /// Returns whether a commit was emitted (the mirror follows).
    fn commit_window(&mut self, window: u32, damage: Rect) -> bool {
        let Some(export) = self.windows.get(&window) else {
            return false;
        };
        let (width, height) = export.committed;
        if width == 0 || height == 0 {
            return false;
        }
        let Some(frame) = self.server.window_frame(window, damage) else {
            return false;
        };
        let offset = export.offset as usize;
        let end = offset.saturating_add(frame.data.len());
        if end > self.mirror.len() {
            return false; // the layout outran the mirror (a racing resize)
        }
        self.mirror[offset..end].copy_from_slice(&frame.data);
        let surface = export.surface;
        let buffer = export.buffer;
        let attach = Message::new(surface, core::surface::request::ATTACH).arg(Value::Object(
            Some(ldp_core::ids::ObjectId::client(buffer).expect("buffer id")),
        ));
        self.out.push(attach);
        let damage_msg = Message::new(surface, core::surface::request::DAMAGE).arg(
            Value::array(
                ldp_core::wire::ArgType::Rect,
                vec![ldp_core::wire::Primitive::Rect(damage)],
            )
            .expect("damage rect array"),
        );
        self.out.push(damage_msg);
        // The implicit-to-explicit sync translation: the requests are
        // fully applied — readiness is stated by the commit itself.
        self.cookie = self.cookie.wrapping_add(1);
        let commit =
            Message::new(surface, core::surface::request::COMMIT).arg(Value::Uint32(self.cookie));
        self.out.push(commit);
        true
    }

    /// The LDP presented verdict (a window's commit landed; the
    /// steady-state tick follows — the same cadence as the rootful
    /// driver's `on_presented` → `on_frame_tick` cycle).
    pub fn on_presented(&mut self) {
        // Nothing to retire: each window's buffer re-attaches on its
        // next commit (the single-view doctrine).
    }

    /// An LDP toplevel configure: the rootless export never resizes
    /// on a proposal — the X client owns its window's geometry, and
    /// the compositor's only proposal today is the initial 0x0 (the
    /// client's own first buffer answers it, which the X geometry
    /// already fixed). A nonzero proposal would be window management
    /// (the shell's roadmap line); it is consumed, never applied.
    pub fn on_ldp_configure(&mut self, _toplevel: u32, _width: u32, _height: u32) {}

    /// The popup solver's placement proposal: acknowledged. The X
    /// geometry stays the X truth (the bridge never moves an X
    /// window); the display follows the solver's answer wherever it
    /// lands relative to the anchor.
    pub fn on_ldp_popup_configure(&mut self, popup: u32, serial: u32) {
        let ack =
            Message::new(popup, shell::popup::request::ACK_CONFIGURE).arg(Value::Uint32(serial));
        self.out.push(ack);
    }

    /// The popup was dismissed server-side (the parent surface died —
    /// which the driver itself drove when the X parent died, so this
    /// normally arrives after the teardown; a live export here means
    /// the dismissal raced ahead): detach the surface honestly.
    pub fn on_ldp_popup_done(&mut self, popup: u32) {
        let Some(window) = self
            .windows
            .iter()
            .find(|(_, e)| matches!(e.role, Role::Popup { id } if id == popup))
            .map(|(w, _)| *w)
        else {
            return;
        };
        self.teardown_window(window);
    }

    /// An LDP toplevel close routed at one export: the ICCCM dance on
    /// that X window (`WM_DELETE_WINDOW` when listed, destroy
    /// otherwise). Returns the policy for the process's X output.
    pub fn on_ldp_close(&mut self, toplevel: u32, time: u32) -> ClosePolicy {
        let Some(window) = self
            .windows
            .iter()
            .find(|(_, e)| matches!(e.role, Role::Toplevel { id } if id == toplevel))
            .map(|(w, _)| *w)
        else {
            return ClosePolicy::NoSuchWindow;
        };
        self.server.request_close(window, time)
    }

    // ---- input translation (LDP events in, X events out) ---------

    /// An LDP pointer motion over one export's surface: surface-local
    /// is window-local; the X root coordinates are the X window's own
    /// absolute origin plus the local offset (the coordinate-fiction
    /// bridge: the click hits the pixel the user sees; the X client
    /// sees the click at its own geometry).
    pub fn ldp_pointer_motion(&mut self, surface: u32, x: i32, y: i32, time: u32) {
        let Some(window) = self.window_of_surface(surface) else {
            return;
        };
        let (ox, oy) = self.server.window_origin(window);
        self.server.pointer_move(ox + x, oy + y, time);
    }

    /// An LDP pointer button (the X server routes by the current
    /// pointer position — the motion above).
    pub fn ldp_pointer_button(&mut self, button: u8, pressed: bool, time: u32) {
        self.server.pointer_button(button, pressed, time);
    }

    /// An LDP key event. LDP keycodes are evdev; X keycodes are
    /// evdev + 8 (the rootful driver's translation).
    pub fn ldp_key(&mut self, evdev_keycode: u32, pressed: bool, time: u32) {
        let x_keycode = u8::try_from(evdev_keycode.saturating_add(8)).unwrap_or(0);
        if x_keycode == 0 {
            return;
        }
        self.server.key(x_keycode, pressed, time);
    }

    /// An LDP keyboard enter/leave: the X focus follows (PointerRoot
    /// while any export holds it — the rootful driver's translation).
    pub fn ldp_keyboard_focus(&mut self, focused: bool, time: u32) {
        let target = if focused {
            crate::dispatch::Focus::PointerRoot
        } else {
            crate::dispatch::Focus::None
        };
        self.server.set_focus(target, time);
    }

    /// The X window whose export owns `surface`.
    fn window_of_surface(&self, surface: u32) -> Option<u32> {
        self.windows
            .iter()
            .find(|(_, e)| e.surface == surface)
            .map(|(w, _)| *w)
    }

    /// Allocate the next client object id.
    fn alloc_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        id
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
        let units = 1 + payload.len().div_ceil(4);
        r.extend_from_slice(&(units as u16).to_le_bytes());
        r.extend_from_slice(payload);
        while r.len() % 4 != 0 {
            r.push(0);
        }
        r
    }

    /// One CreateWindow request: the rootful test's byte shape, with
    /// the override-redirect flag optional (the subset's CW bit 10).
    fn create_window(wid: u32, x: i16, y: i16, w: u16, h: u16, or: bool) -> Vec<u8> {
        let mut attrs = Vec::new();
        attrs.extend_from_slice(&wid.to_le_bytes());
        attrs.extend_from_slice(&0x40u32.to_le_bytes()); // parent = root
        attrs.extend_from_slice(&x.to_le_bytes());
        attrs.extend_from_slice(&y.to_le_bytes());
        attrs.extend_from_slice(&w.to_le_bytes());
        attrs.extend_from_slice(&h.to_le_bytes());
        attrs.extend_from_slice(&0u16.to_le_bytes()); // border
        attrs.extend(&[1u8, 0]); // class InputOutput + pad
        attrs.extend_from_slice(&0x21u32.to_le_bytes()); // visual
        let mut mask: u32 = 1 << 1; // back-pixel
        if or {
            mask |= 1 << 10; // override-redirect (the subset's bit)
        }
        attrs.extend_from_slice(&mask.to_le_bytes());
        attrs.extend_from_slice(&0x00ff_ff00u32.to_le_bytes()); // back pixel
        if or {
            attrs.push(1); // override-redirect true
            attrs.extend_from_slice(&[0, 0, 0]); // pad to u32
        }
        request(1, &attrs)
    }

    fn map_window(wid: u32) -> Vec<u8> {
        request(8, &wid.to_le_bytes())
    }

    fn unmap_window(wid: u32) -> Vec<u8> {
        request(10, &wid.to_le_bytes())
    }

    fn driver() -> RootlessDriver {
        let mut d = RootlessDriver::new(screen());
        let mut check = OneToken(Some([1; 8]));
        d.authenticate("lion-x-bridge", [1; 8], &mut check).unwrap();
        d
    }

    #[test]
    fn unauthenticated_rootless_is_silent() {
        let mut d = RootlessDriver::new(screen());
        d.on_frame_tick();
        assert!(d.take_ldp_messages().is_empty());
        let mut check = OneToken(None);
        assert_eq!(
            d.authenticate("bridge", [1; 8], &mut check),
            Err(AuthError::Rejected)
        );
    }

    #[test]
    fn the_bootstrap_is_the_bare_handshake() {
        // No windows yet: hello, get_registry, four binds — and no
        // pool (a windowless X desktop commits nothing).
        let mut d = driver();
        let msgs = d.take_ldp_messages();
        assert_eq!(msgs.len(), 6);
        assert_eq!(msgs[0].opcode, core::connection::request::HELLO);
        assert_eq!(msgs[1].opcode, core::connection::request::GET_REGISTRY);
        assert_eq!(msgs[2].args[0], Value::String("ldp.core.compositor".into()));
        d.on_frame_tick();
        assert!(d.take_ldp_messages().is_empty());
    }

    #[test]
    fn windows_mint_surfaces_and_roles() {
        let mut d = driver();
        d.take_ldp_messages();
        // Two X windows: one managed, one override-redirect.
        let mut cx = handshake();
        cx.extend_from_slice(&create_window(0x41, 10, 10, 64, 64, false));
        cx.extend_from_slice(&map_window(0x41));
        cx.extend_from_slice(&create_window(0x42, 100, 20, 32, 16, true));
        cx.extend_from_slice(&map_window(0x42));
        let client = d.server_mut().add_client();
        d.x_feed(client, &cx).unwrap();
        let _ = d.x_output(client);
        d.on_frame_tick();
        let msgs = d.take_ldp_messages();
        // Per window: create_surface + get_toplevel / get_popup; the
        // pool once; one buffer per window. (Filters are
        // object-aware: opcode numbers are per-interface, so 1 means
        // attach AND create_buffer AND get_toplevel alike.)
        let ops: Vec<(u32, u32)> = msgs.iter().map(|m| (m.object_id, m.opcode)).collect();
        assert!(
            ops.contains(&(ids::SHELL, shell::shell::request::GET_TOPLEVEL)),
            "the managed window takes the toplevel role"
        );
        assert!(
            ops.contains(&(ids::SHELL, shell::shell::request::GET_POPUP)),
            "the OR window takes the popup role"
        );
        assert!(
            ops.contains(&(ids::SHM, core::shm::request::CREATE_POOL)),
            "the pool mints with the first window"
        );
        assert_eq!(
            msgs.iter()
                .filter(|m| {
                    m.object_id == ids::POOL && m.opcode == core::shm_pool::request::CREATE_BUFFER
                })
                .count(),
            2,
            "one buffer view per window"
        );
        // The second window's buffer rides a nonzero offset.
        let offsets: Vec<i64> = msgs
            .iter()
            .filter(|m| {
                m.object_id == ids::POOL && m.opcode == core::shm_pool::request::CREATE_BUFFER
            })
            .map(|m| match m.args[0] {
                Value::Int32(v) => i64::from(v),
                _ => -1,
            })
            .collect();
        assert_eq!(offsets[0], 0);
        assert_eq!(offsets[1], (64 * 64 * 4 + 3) & !3, "packed past window one");
        // Both commit (the first appearance is full-window damage).
        // COMMIT is surface opcode 12 — unique among this tick's
        // emissions, but the object check keeps it honest.
        assert_eq!(
            msgs.iter()
                .filter(|m| {
                    m.object_id != ids::POOL
                        && m.object_id != ids::SHM
                        && m.object_id != ids::SHELL
                        && m.object_id != ids::COMPOSITOR
                        && m.opcode == core::surface::request::COMMIT
                })
                .count(),
            2
        );
        // The watch feed names both pairs.
        assert_eq!(d.watch_ids().len(), 2);
    }

    #[test]
    fn damage_isolates_to_the_drawing_window() {
        let mut d = driver();
        d.take_ldp_messages();
        let mut cx = handshake();
        cx.extend_from_slice(&create_window(0x41, 0, 0, 32, 32, false));
        cx.extend_from_slice(&map_window(0x41));
        cx.extend_from_slice(&create_window(0x42, 64, 0, 32, 32, false));
        cx.extend_from_slice(&map_window(0x42));
        let client = d.server_mut().add_client();
        d.x_feed(client, &cx).unwrap();
        let _ = d.x_output(client);
        d.on_frame_tick();
        let msgs = d.take_ldp_messages();
        // Both windows committed their first frame.
        assert_eq!(
            msgs.iter()
                .filter(|m| {
                    m.object_id != ids::POOL
                        && m.object_id != ids::SHM
                        && m.object_id != ids::SHELL
                        && m.object_id != ids::COMPOSITOR
                        && m.opcode == core::surface::request::COMMIT
                })
                .count(),
            2
        );
        // Now draw ONLY into window two: a 4x4 fill at (2, 2).
        let mut gc = Vec::new();
        gc.extend_from_slice(&0x50u32.to_le_bytes()); // cid
        gc.extend_from_slice(&0x42u32.to_le_bytes()); // drawable
        let gmask: u32 = 1 << 2; // foreground
        gc.extend_from_slice(&gmask.to_le_bytes());
        gc.extend_from_slice(&0x00ff_0000u32.to_le_bytes());
        let mut feed = request(55, &gc);
        let mut fr = Vec::new();
        fr.extend_from_slice(&0x42u32.to_le_bytes());
        fr.extend_from_slice(&0x50u32.to_le_bytes());
        fr.extend_from_slice(&2i16.to_le_bytes());
        fr.extend_from_slice(&2i16.to_le_bytes());
        fr.extend_from_slice(&4u16.to_le_bytes());
        fr.extend_from_slice(&4u16.to_le_bytes());
        feed.extend_from_slice(&request(70, &fr));
        d.x_feed(client, &feed).unwrap();
        let _ = d.x_output(client);
        d.on_frame_tick();
        let msgs = d.take_ldp_messages();
        // Exactly one window committed: window two's surface.
        let commits: Vec<&Message> = msgs
            .iter()
            .filter(|m| m.opcode == core::surface::request::COMMIT)
            .collect();
        assert_eq!(commits.len(), 1, "damage isolates to one window");
        // Its damage is window-LOCAL (2, 2), not root coords.
        let damage = msgs
            .iter()
            .find(|m| m.opcode == core::surface::request::DAMAGE)
            .expect("the damage rides the commit");
        match &damage.args[0] {
            Value::Array { items, .. } => match &items[0] {
                ldp_core::wire::Primitive::Rect(r) => {
                    assert_eq!((r.x, r.y, r.w, r.h), (2, 2, 4, 4));
                }
                other => panic!("unexpected damage element {other:?}"),
            },
            other => panic!("unexpected damage arg {other:?}"),
        }
        // A still tick commits nothing.
        d.on_frame_tick();
        assert!(d.take_ldp_messages().is_empty());
    }

    #[test]
    fn unmap_tears_the_export_down() {
        let mut d = driver();
        d.take_ldp_messages();
        let mut cx = handshake();
        cx.extend_from_slice(&create_window(0x41, 0, 0, 32, 32, false));
        cx.extend_from_slice(&map_window(0x41));
        let client = d.server_mut().add_client();
        d.x_feed(client, &cx).unwrap();
        let _ = d.x_output(client);
        d.on_frame_tick();
        assert!(!d.take_ldp_messages().is_empty());
        assert_eq!(d.watch_ids().len(), 1);
        // Unmap: the export tears down (role, buffer, surface — the
        // generic connection.destroy path).
        d.x_feed(client, &unmap_window(0x41)).unwrap();
        let _ = d.x_output(client);
        d.on_frame_tick();
        let msgs = d.take_ldp_messages();
        let destroys: Vec<&Message> = msgs
            .iter()
            .filter(|m| {
                m.object_id == ids::CONNECTION && m.opcode == core::connection::request::DESTROY
            })
            .collect();
        assert_eq!(destroys.len(), 3, "role, buffer, and surface retire");
        assert!(d.watch_ids().is_empty());
    }

    #[test]
    fn input_translates_through_the_window_origin() {
        let mut d = driver();
        d.take_ldp_messages();
        let mut cx = handshake();
        cx.extend_from_slice(&create_window(0x41, 40, 30, 64, 64, false));
        cx.extend_from_slice(&map_window(0x41));
        let client = d.server_mut().add_client();
        d.x_feed(client, &cx).unwrap();
        let _ = d.x_output(client);
        d.on_frame_tick();
        let msgs = d.take_ldp_messages();
        // The minted surface id (from the create_surface message).
        let surface = msgs
            .iter()
            .find(|m| {
                m.object_id == ids::COMPOSITOR
                    && m.opcode == core::compositor::request::CREATE_SURFACE
            })
            .map(|m| match m.args[0] {
                Value::NewId(id) => id.as_u32(),
                _ => 0,
            })
            .expect("the surface minted");
        // A pointer motion at surface-local (5, 7): the X root
        // coordinates are the X window's origin (40, 30) + local.
        d.ldp_pointer_motion(surface, 5, 7, 100);
        assert_eq!(d.server_mut().pointer_position(), (45, 37));
    }

    #[test]
    fn close_routes_to_the_owning_window() {
        let mut d = driver();
        d.take_ldp_messages();
        let mut cx = handshake();
        cx.extend_from_slice(&create_window(0x41, 0, 0, 32, 32, false));
        cx.extend_from_slice(&map_window(0x41));
        let client = d.server_mut().add_client();
        d.x_feed(client, &cx).unwrap();
        let _ = d.x_output(client);
        d.on_frame_tick();
        let msgs = d.take_ldp_messages();
        let toplevel = msgs
            .iter()
            .find(|m| m.object_id == ids::SHELL && m.opcode == shell::shell::request::GET_TOPLEVEL)
            .map(|m| match m.args[2] {
                Value::NewId(id) => id.as_u32(),
                _ => 0,
            })
            .expect("the toplevel minted");
        // No WM_PROTOCOLS property: the close destroys the window.
        assert_eq!(d.on_ldp_close(toplevel, 1000), ClosePolicy::Destroyed);
        // An unknown toplevel closes nothing.
        assert_eq!(
            d.on_ldp_close(toplevel + 100, 1000),
            ClosePolicy::NoSuchWindow
        );
    }

    #[test]
    fn popup_configure_is_acknowledged() {
        let mut d = driver();
        d.take_ldp_messages();
        // The solver's proposal for any popup: the driver acks it.
        d.on_ldp_popup_configure(0x42, 7);
        let msgs = d.take_ldp_messages();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].opcode, shell::popup::request::ACK_CONFIGURE);
        assert_eq!(msgs[0].args[0], Value::Uint32(7));
    }

    /// A host that records into shared state (the test reads what the
    /// driver drove).
    struct SharedHost(std::sync::Arc<std::sync::Mutex<Vec<String>>>);
    impl DriverHost for SharedHost {
        fn pool_fd(&mut self, _pool: u32, size: u64) -> u32 {
            self.0.lock().expect("host").push(format!("fd {size}"));
            0
        }
        fn sync_pool(&mut self, _pool: u32, bytes: &[u8]) {
            self.0
                .lock()
                .expect("host")
                .push(format!("sync {}", bytes.len()));
        }
    }

    #[test]
    fn the_host_sees_the_pool_lifecycle() {
        // The host seam: the pool allocation at the first window and
        // the mirror sync at every committing tick (the process
        // binary's real host answers the same calls).
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut d = RootlessDriver::new(screen())
            .with_driver_host(Box::new(SharedHost(std::sync::Arc::clone(&log))));
        let mut check = OneToken(Some([1; 8]));
        d.authenticate("app", [1; 8], &mut check).unwrap();
        d.take_ldp_messages();
        assert!(
            log.lock().expect("host").is_empty(),
            "no pool before windows"
        );
        let mut cx = handshake();
        cx.extend_from_slice(&create_window(0x41, 0, 0, 16, 16, false));
        cx.extend_from_slice(&map_window(0x41));
        let client = d.server_mut().add_client();
        d.x_feed(client, &cx).unwrap();
        let _ = d.x_output(client);
        d.on_frame_tick();
        let _ = d.take_ldp_messages();
        let recorded = log.lock().expect("host").clone();
        // The pool allocated at the window's size, then the mirror
        // synced (16*16*4 = 1024 bytes).
        assert!(
            recorded[0].starts_with("fd 1024"),
            "the pool sized to the window: {recorded:?}"
        );
        assert!(
            recorded.contains(&"sync 1024".to_owned()),
            "the mirror flowed: {recorded:?}"
        );
        // A still tick syncs nothing more.
        let before = recorded.len();
        d.on_frame_tick();
        assert_eq!(
            log.lock().expect("host").len(),
            before,
            "a still desktop syncs nothing"
        );
    }
}
