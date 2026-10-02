//! The X11 server-subset connection state machine.
//!
//! A [`Server`] owns the screen-wide state — the window tree, atom
//! table, GC/pixmap/colormap registries, SHM segments, selection and
//! focus, the pointer, and the composited root store — plus one
//! per-client protocol state. The process binary owns the Unix socket:
//! it accepts connections, shuttles bytes through [`Server::feed`] /
//! [`Server::take_output`], and supplies the MIT-SHM segment backing
//! through the [`ShmHost`] seam.
//!
//! Simplifications pinned here (documented, exercised by the EC
//! suites):
//!
//! * **One event-mask owner per window.** The window's event mask is
//!   set by (and events flow to) the client that created it. Real X
//!   allows per-client selection on shared windows; the bridge plays
//!   the window manager itself, so no second client ever selects.
//! * **The bridge is the WM.** `ReparentWindow` answers
//!   `BadImplementation`; stacking comes from `ConfigureWindow` only.
//! * **Fonts and text are out.** Font requests are inert or empty;
//!   `QueryFont` / `QueryTextExtents` answer `BadImplementation`.
//! * **Unimplemented drawing state** (tiled/stippled fill, bitmap
//!   clip-masks) answers `BadImplementation` at draw time rather than
//!   misrendering.
//! * **Timing is injected.** The crate reads no clock: the input seams
//!   take `time_ms`, and that time rides the events.
//!
//! The per-opcode request handlers live in [`crate::ops`]; this module
//! owns the connection state, event routing, compositing, and the
//! driver-facing seams.

#![forbid(unsafe_code)]
// Geometry and wire plumbing restate the protocol's single-character
// field vocabulary (x, y, w, h); the module is flat protocol state.
#![allow(clippy::many_single_char_names)]

use std::collections::{BTreeMap, BTreeSet};

use ldp_core::geometry::Rect;

use crate::atoms::AtomTable;
use crate::events::XEvent;
use crate::gc::{Gc, GcStore};
use crate::property::PropError;
use crate::render::{Store, Target};
use crate::setup::{encode_setup_reply, parse_handshake, ClientSetup, Handshake, ScreenParams};
use crate::shm::{ShmHost, ShmState};
use crate::window::{StructuralEffect, WinClass, WindowTree};
use crate::wire::{error_envelope, frame_request, reply_header, Endian, Seq, WireError};

/// Extension major opcodes the subset serves (fixed, like real
/// servers' first-extension allocation).
pub mod ext {
    /// MIT-SHM.
    pub const SHM: u8 = 130;
    /// BIG-REQUESTS.
    pub const BIGREQ: u8 = 131;
}

/// Why a connection was torn down (the process closes the socket).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum FatalReason {
    /// The setup prefix was malformed (bad order byte or version).
    Handshake,
    /// A fatal wire violation.
    Wire(WireError),
}

/// Result of feeding bytes to one client.
pub type FeedOutcome = Result<(), FatalReason>;

/// Keyboard focus target.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Focus {
    /// No window.
    None,
    /// Follow the pointer (treated as the pointer window).
    PointerRoot,
    /// A window XID.
    Window(u32),
}

/// What [`Server::request_close`] did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ClosePolicy {
    /// `WM_DELETE_WINDOW` ClientMessage delivered.
    ProtocolMessage,
    /// No protocol listed: the window was destroyed.
    Destroyed,
    /// No such window.
    NoSuchWindow,
}

/// What a drawable resolved to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DrawableKind {
    /// A drawable window.
    Window,
    /// An InputOnly window (drawing is `BadMatch`).
    InputOnlyWindow,
    /// A pixmap.
    Pixmap,
}

/// Handle to one client connection.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ClientId(usize);

/// One connected client's protocol state.
#[derive(Clone, Debug)]
struct Conn {
    ready: bool,
    endian: Endian,
    bigreq: bool,
    seq: Seq,
    inbuf: Vec<u8>,
    out: Vec<u8>,
}

impl Conn {
    fn new() -> Conn {
        Conn {
            ready: false,
            endian: Endian::Lsb,
            bigreq: false,
            seq: Seq::default(),
            inbuf: Vec::new(),
            out: Vec::new(),
        }
    }
}

/// The whole X-side server state.
pub struct Server {
    screen: ScreenParams,
    tree: WindowTree,
    atoms: AtomTable,
    gcs: GcStore,
    pixmaps: BTreeMap<u32, Store>,
    cursors: BTreeSet<u32>,
    fonts: BTreeSet<u32>,
    colormaps: BTreeSet<u32>,
    owners: BTreeMap<u32, ClientId>,
    shm: ShmState,
    shm_host: Box<dyn ShmHost>,
    /// Selection atom → (owner window, time).
    selection: BTreeMap<u32, (u32, u32)>,
    focus: Focus,
    pointer: (i32, i32),
    buttons: u16,
    pressed_keys: BTreeSet<u8>,
    conns: Vec<Option<Conn>>,
    root_store: Option<Store>,
    damage: Option<Rect>,
}

impl Server {
    /// A server for one screen with the given SHM host.
    pub fn new(screen: ScreenParams, shm_host: Box<dyn ShmHost>) -> Server {
        let tree = WindowTree::new(
            screen.root,
            u32::from(screen.width),
            u32::from(screen.height),
            screen.black_pixel,
        );
        Server {
            screen,
            tree,
            atoms: AtomTable::new(),
            gcs: GcStore::default(),
            pixmaps: BTreeMap::new(),
            cursors: BTreeSet::new(),
            fonts: BTreeSet::new(),
            colormaps: BTreeSet::new(),
            owners: BTreeMap::new(),
            shm: ShmState::default(),
            shm_host,
            selection: BTreeMap::new(),
            focus: Focus::None,
            pointer: (0, 0),
            buttons: 0,
            pressed_keys: BTreeSet::new(),
            conns: Vec::new(),
            root_store: None,
            damage: None,
        }
    }

    /// The screen parameters.
    #[must_use]
    pub fn screen(&self) -> ScreenParams {
        self.screen
    }

    /// Attach a new client connection.
    pub fn add_client(&mut self) -> ClientId {
        let id = ClientId(self.conns.len());
        self.conns.push(Some(Conn::new()));
        id
    }

    /// Detach a client: destroys its windows (full structural
    /// effects) and drops its pending output.
    pub fn remove_client(&mut self, client: ClientId) {
        if !self.conns.get(client.0).is_some_and(Option::is_some) {
            return;
        }
        let owned: Vec<u32> = self
            .owners
            .iter()
            .filter(|(_, &c)| c == client)
            .map(|(&w, _)| w)
            .collect();
        for w in owned {
            if self.tree.get(w).is_some() {
                self.destroy_window_internal(w);
            }
        }
        self.conns[client.0] = None;
    }

    /// Whether a client slot is live.
    #[must_use]
    pub fn client_alive(&self, client: ClientId) -> bool {
        self.conns.get(client.0).is_some_and(Option::is_some)
    }

    /// Feed raw socket bytes from one client.
    ///
    /// # Errors
    /// [`FatalReason::Handshake`] for a malformed setup prefix (after
    /// the failure reply was queued), [`FatalReason::Wire`] for a
    /// fatal framing violation — both mean the process must close the
    /// connection.
    pub fn feed(&mut self, client: ClientId, bytes: &[u8]) -> FeedOutcome {
        let setup: Option<ClientSetup> = {
            let Some(Some(conn)) = self.conns.get_mut(client.0) else {
                return Ok(());
            };
            conn.inbuf.extend_from_slice(bytes);
            if conn.ready {
                None
            } else {
                match parse_handshake(&conn.inbuf) {
                    Handshake::Incomplete(_) => return Ok(()),
                    Handshake::Ready(setup, used) => {
                        conn.inbuf.drain(..used);
                        conn.ready = true;
                        conn.endian = setup.endian;
                        Some(*setup)
                    }
                }
            }
        };
        if let Some(setup) = setup {
            self.handshake_reply(client, &setup)?;
        }
        self.pump(client)
    }

    /// Take the pending output bytes for one client.
    pub fn take_output(&mut self, client: ClientId) -> Vec<u8> {
        let mut out = Vec::new();
        if let Some(Some(conn)) = self.conns.get_mut(client.0) {
            std::mem::swap(&mut out, &mut conn.out);
        }
        out
    }

    // ---- driver seams ----

    /// The composited root store (the LDP buffer contents).
    #[must_use]
    pub fn root_store(&self) -> Option<&Store> {
        self.root_store.as_ref()
    }

    /// Take root damage accumulated since the last call (the LDP
    /// `surface.damage` argument).
    pub fn take_damage(&mut self) -> Option<Rect> {
        self.damage.take()
    }

    /// Take the accumulated root damage, **distributed across the
    /// mapped top-level windows** as window-local rects — the rootless
    /// driver's per-window commit feed. Consumed exactly like
    /// [`Server::take_damage`] (one call per frame tick; the union
    /// both drivers see is the same pixels).
    pub fn take_top_damage(&mut self) -> Vec<(u32, Rect)> {
        let Some(d) = self.damage.take() else {
            return Vec::new();
        };
        let tops = self.top_level_windows();
        let mut out = Vec::new();
        for w in tops {
            let rect = self.tree.root_rect(w);
            if let Some(i) = d.intersect(rect) {
                // Window-local: the export's own frame.
                out.push((w, Rect::new(i.x - rect.x, i.y - rect.y, i.w, i.h)));
            }
        }
        out
    }

    /// Composite one top-level window's **subtree** over `dirty` (the
    /// window-local dirty rect): the window's own store as the base,
    /// every mapped InputOutput descendant painted over in
    /// bottom-to-top stacking order — foreign subtrees never occlude
    /// (the display-side compositor owns cross-window overlap; the
    /// rootless doctrine). The returned store is the export frame.
    ///
    /// This is the rootless counterpart of the root composite the
    /// rootful driver ships whole: same blit discipline, same
    /// visibility arithmetic ([`WindowTree::region_within`]), a
    /// different frame of reference per window.
    pub fn window_frame(&mut self, window: u32, dirty: Rect) -> Option<Store> {
        let w = self.tree.get(window)?;
        let (width, height) = (w.width, w.height);
        let d = dirty.intersect(Rect::new(0, 0, width, height))?;
        // The base: the window's own pixels, or its background pixel
        // (the rootful recomposite's own rule — never a guess).
        let mut comp = match w.store.as_ref() {
            Some(s) => s.clone(),
            None => Store::filled(width, height, w.background_pixel.unwrap_or(0)),
        };
        // The subtree's stacking order, bottom-to-top, the window
        // itself excluded (it is the base).
        let order = self.subtree_order(window);
        for id in order {
            let Some(w) = self.tree.get(id) else {
                continue;
            };
            if !w.mapped || w.class != WinClass::InputOutput {
                continue;
            }
            let region = self.tree.region_within(id, window);
            for r in &region {
                let Some(i) = r.intersect(d) else {
                    continue;
                };
                if let Some(src) = w.store.as_ref() {
                    // The descendant's origin within the export frame.
                    let (ax, ay) = self.tree.absolute_origin(id);
                    let (tx, ty) = self.tree.absolute_origin(window);
                    blit_to_root(&mut comp, src, ax - tx, ay - ty, &i);
                } else {
                    let bg = w.background_pixel.unwrap_or(0);
                    fill_root(&mut comp, &i, bg);
                }
            }
        }
        Some(comp)
    }

    /// The subtree's stacking order (bottom-to-top), `window` itself
    /// excluded — the paint order of the subtree composite.
    fn subtree_order(&self, window: u32) -> Vec<u32> {
        let mut out = Vec::new();
        let mut stack = Vec::new();
        if let Some(w) = self.tree.get(window) {
            for &c in w.children.iter().rev() {
                stack.push(c);
            }
        }
        while let Some(cur) = stack.pop() {
            out.push(cur);
            if let Some(w) = self.tree.get(cur) {
                for &c in w.children.iter().rev() {
                    stack.push(c);
                }
            }
        }
        out
    }

    /// Root damage accumulated right now, without consuming it.
    #[must_use]
    pub fn damage_now(&self) -> Option<Rect> {
        self.damage
    }

    /// Swap the SHM host (the process binary installs its real one).
    pub fn set_shm_host(&mut self, host: Box<dyn ShmHost>) {
        self.shm_host = host;
    }

    /// Resize the root window (rootful bridge following an LDP
    /// toplevel configure). Re-exposes every mapped window.
    pub fn resize_root(&mut self, width: u32, height: u32) {
        let old = (
            self.tree.get(self.tree.root).map_or(0, |w| w.width),
            self.tree.get(self.tree.root).map_or(0, |w| w.height),
        );
        if (width, height) == old {
            return;
        }
        if let Some(w) = self.tree.get_mut(self.tree.root) {
            w.width = width;
            w.height = height;
        }
        self.root_store = None;
        let exposures = self.reexpose_all();
        self.mark_dirty(Rect::new(0, 0, width, height));
        self.recomposite();
        self.emit_exposures(&exposures);
    }

    /// The pointer's root coordinates.
    #[must_use]
    pub fn pointer_position(&self) -> (i32, i32) {
        self.pointer
    }

    /// The current focus.
    #[must_use]
    pub fn focus(&self) -> Focus {
        self.focus
    }

    /// Mapped top-level windows (children of the root), top of stack
    /// last — the rootless stretch seam.
    #[must_use]
    pub fn top_level_windows(&self) -> Vec<u32> {
        self.tree
            .get(self.tree.root)
            .map(|w| w.children.clone())
            .unwrap_or_default()
            .into_iter()
            .filter(|&c| {
                self.tree
                    .get(c)
                    .is_some_and(|cw| cw.mapped && cw.class == WinClass::InputOutput)
            })
            .collect()
    }

    /// One window's root-coordinate rect (the rootless driver's
    /// anchor geometry and damage split).
    #[must_use]
    pub fn window_root_rect(&self, window: u32) -> Rect {
        self.tree.root_rect(window)
    }

    /// One window's absolute (root-frame) origin — the rootless
    /// input translation's X-side truth.
    #[must_use]
    pub fn window_origin(&self, window: u32) -> (i32, i32) {
        self.tree.absolute_origin(window)
    }

    /// Read one window's state (geometry, flags) — the rootless
    /// driver's mint walk.
    #[must_use]
    pub fn window_ref(&self, window: u32) -> Option<&crate::window::Window> {
        self.tree.get(window)
    }

    /// A window's ICCCM title (`_NET_WM_NAME` UTF-8 preferred, then
    /// `WM_NAME` STRING8).
    #[must_use]
    pub fn window_title(&self, window: u32) -> Option<Box<str>> {
        let w = self.tree.get(window)?;
        for (name, type_name) in [("_NET_WM_NAME", "UTF8_STRING"), ("WM_NAME", "STRING")] {
            let Some(prop_atom) = self.atoms.lookup(name) else {
                continue;
            };
            let Some(ty) = self.atoms.lookup(type_name) else {
                continue;
            };
            if let Some(v) = w.props.get(prop_atom) {
                if v.type_atom == ty && v.format == 8 {
                    return Some(
                        String::from_utf8_lossy(&v.data)
                            .into_owned()
                            .into_boxed_str(),
                    );
                }
            }
        }
        None
    }

    /// Ask a window to close (ICCCM): the `WM_DELETE_WINDOW` protocol
    /// if the window listed it, otherwise destroy.
    pub fn request_close(&mut self, window: u32, time: u32) -> ClosePolicy {
        if !self.tree.windows.contains_key(&window) {
            return ClosePolicy::NoSuchWindow;
        }
        let proto = self
            .atoms
            .lookup(crate::atoms::WM_PROTOCOLS_NAME)
            .unwrap_or(210);
        let delete = self
            .atoms
            .lookup(crate::atoms::WM_DELETE_WINDOW_NAME)
            .unwrap_or(211);
        let listed = self
            .tree
            .get(window)
            .and_then(|w| w.props.get(proto))
            .is_some_and(|v| {
                v.format == 32
                    && v.data.len() % 4 == 0
                    && v.data.chunks_exact(4).any(|c| {
                        u32::from(c[0])
                            | (u32::from(c[1]) << 8)
                            | (u32::from(c[2]) << 16)
                            | (u32::from(c[3]) << 24)
                            == delete
                    })
            });
        if listed {
            let endian = self.client_endian_of_window(window);
            let mut data = [0u8; 20];
            data[0..4].copy_from_slice(&endian.put_u32(delete));
            data[4..8].copy_from_slice(&endian.put_u32(time));
            let ev = XEvent::ClientMessage {
                format: 32,
                window,
                type_atom: proto,
                data,
            };
            self.deliver_to_window_owner(window, &ev);
            ClosePolicy::ProtocolMessage
        } else {
            self.destroy_window_internal(window);
            ClosePolicy::Destroyed
        }
    }

    // ---- input seams (the driver injects user input) ----

    /// Move the pointer; generates Leave/Enter/Motion as routed.
    pub fn pointer_move(&mut self, x: i32, y: i32, time: u32) {
        let old_w = self.tree.window_at(self.pointer.0, self.pointer.1);
        self.pointer = (x, y);
        let new_w = self.tree.window_at(x, y);
        if new_w != old_w {
            self.enter_leave(old_w, new_w, time);
        }
        if self.mask_of(new_w) & crate::events::mask::POINTER_MOTION == 0 {
            return;
        }
        let (ox, oy) = self.tree.absolute_origin(new_w);
        let root = self.tree.root;
        let ev = XEvent::MotionNotify {
            detail: 0,
            time,
            root,
            event: new_w,
            root_x: x as i16,
            root_y: y as i16,
            event_x: (x - ox) as i16,
            event_y: (y - oy) as i16,
            state: self.input_state(),
        };
        self.send_input_event(new_w, &ev);
    }

    /// Press or release a pointer button.
    pub fn pointer_button(&mut self, button: u8, pressed: bool, time: u32) {
        if button == 0 || button > 5 {
            return;
        }
        if pressed {
            self.buttons |= 1 << (u32::from(button) - 1);
        } else {
            self.buttons &= !(1 << (u32::from(button) - 1));
        }
        let target = self.tree.window_at(self.pointer.0, self.pointer.1);
        let want = if pressed {
            crate::events::mask::BUTTON_PRESS
        } else {
            crate::events::mask::BUTTON_RELEASE
        };
        if self.mask_of(target) & want == 0 {
            return;
        }
        let (ox, oy) = self.tree.absolute_origin(target);
        let root = self.tree.root;
        let (x, y) = self.pointer;
        let state = self.input_state();
        let ev = if pressed {
            XEvent::ButtonPress {
                button,
                time,
                root,
                event: target,
                root_x: x as i16,
                root_y: y as i16,
                event_x: (x - ox) as i16,
                event_y: (y - oy) as i16,
                state,
            }
        } else {
            XEvent::ButtonRelease {
                button,
                time,
                root,
                event: target,
                root_x: x as i16,
                root_y: y as i16,
                event_x: (x - ox) as i16,
                event_y: (y - oy) as i16,
                state,
            }
        };
        self.send_input_event(target, &ev);
    }

    /// Press or release a key (X keycode = evdev + 8).
    pub fn key(&mut self, keycode: u8, pressed: bool, time: u32) {
        if pressed {
            self.pressed_keys.insert(keycode);
        } else {
            self.pressed_keys.remove(&keycode);
        }
        let target = match self.focus {
            Focus::Window(w) => w,
            Focus::PointerRoot => self.tree.window_at(self.pointer.0, self.pointer.1),
            Focus::None => return,
        };
        let want = if pressed {
            crate::events::mask::KEY_PRESS
        } else {
            crate::events::mask::KEY_RELEASE
        };
        if self.mask_of(target) & want == 0 {
            return;
        }
        let (ox, oy) = self.tree.absolute_origin(target);
        let root = self.tree.root;
        let (x, y) = self.pointer;
        let state = self.input_state();
        let ev = if pressed {
            XEvent::KeyPress {
                keycode,
                time,
                root,
                event: target,
                root_x: x as i16,
                root_y: y as i16,
                event_x: (x - ox) as i16,
                event_y: (y - oy) as i16,
                state,
            }
        } else {
            XEvent::KeyRelease {
                keycode,
                time,
                root,
                event: target,
                root_x: x as i16,
                root_y: y as i16,
                event_x: (x - ox) as i16,
                event_y: (y - oy) as i16,
                state,
            }
        };
        self.send_input_event(target, &ev);
    }

    /// Move the keyboard focus (FocusOut → old, FocusIn + optional
    /// KeymapNotify → new).
    pub fn set_focus(&mut self, target: Focus, time: u32) {
        let _ = time;
        let old = self.focus;
        if let Focus::Window(w) = old {
            if w != 0 && self.mask_of(w) & crate::events::mask::FOCUS_CHANGE != 0 {
                let ev = XEvent::FocusOut { event: w, mode: 0 };
                self.send_input_event(w, &ev);
            }
        }
        self.focus = target;
        if let Focus::Window(w) = target {
            if w != 0 && self.mask_of(w) & crate::events::mask::FOCUS_CHANGE != 0 {
                let ev = XEvent::FocusIn { event: w, mode: 0 };
                self.send_input_event(w, &ev);
                if self.mask_of(w) & crate::events::mask::KEYMAP_STATE != 0 {
                    let keys = self.keymap_bits();
                    let ev = XEvent::KeymapNotify { keys };
                    self.send_input_event(w, &ev);
                }
            }
        }
    }

    /// The 32-byte keymap bit vector of pressed keycodes.
    pub(crate) fn keymap_bits(&self) -> [u8; 31] {
        let mut keys = [0u8; 31];
        for &kc in &self.pressed_keys {
            let byte = usize::from(kc / 8);
            if byte < 31 {
                keys[byte] |= 1 << (kc % 8);
            }
        }
        keys
    }

    /// Buttons | modifiers as the event `state` word.
    pub(crate) fn input_state(&self) -> u16 {
        let mut s = self.buttons;
        for &kc in &self.pressed_keys {
            s |= match kc {
                50 | 62 => 0x0001,   // Shift
                37 | 105 => 0x0004,  // Control
                64 | 108 => 0x0008,  // Mod1 (Alt)
                133 | 134 => 0x0040, // Mod4 (Super)
                _ => 0,
            };
        }
        s
    }

    /// Generate LeaveNotify on the old pointer window and EnterNotify
    /// on the new one (detail per the crossing relationship).
    fn enter_leave(&mut self, old_w: u32, new_w: u32, time: u32) {
        let root = self.tree.root;
        let (x, y) = self.pointer;
        let state = self.input_state();
        if self.mask_of(old_w) & crate::events::mask::LEAVE_WINDOW != 0 {
            let (ox, oy) = self.tree.absolute_origin(old_w);
            let detail = if self.tree.is_ancestor_or_self(old_w, new_w) {
                2 // Inferior: the pointer descended into a child.
            } else if self.tree.is_ancestor_or_self(new_w, old_w) {
                0 // Ancestor: the pointer rose out of the window.
            } else {
                3 // Nonlinear.
            };
            let ev = XEvent::LeaveNotify {
                detail,
                time,
                root,
                event: old_w,
                child: 0,
                root_x: x as i16,
                root_y: y as i16,
                event_x: (x - ox) as i16,
                event_y: (y - oy) as i16,
                state,
                mode: 0,
            };
            self.send_input_event(old_w, &ev);
        }
        if self.mask_of(new_w) & crate::events::mask::ENTER_WINDOW != 0 {
            let (ox, oy) = self.tree.absolute_origin(new_w);
            let detail = if self.tree.is_ancestor_or_self(new_w, old_w) {
                2 // Inferior: arrived back from a descendant.
            } else if self.tree.is_ancestor_or_self(old_w, new_w) {
                0 // Ancestor: descended from an ancestor.
            } else {
                3 // Nonlinear.
            };
            let ev = XEvent::EnterNotify {
                detail,
                time,
                root,
                event: new_w,
                child: 0,
                root_x: x as i16,
                root_y: y as i16,
                event_x: (x - ox) as i16,
                event_y: (y - oy) as i16,
                state,
                mode: 0,
            };
            self.send_input_event(new_w, &ev);
        }
    }

    // ---- internals ----

    fn client_endian_of_window(&self, window: u32) -> Endian {
        match self
            .owners
            .get(&window)
            .and_then(|&c| self.conns.get(c.0))
            .and_then(|c| c.as_ref())
        {
            Some(conn) if conn.ready => conn.endian,
            _ => Endian::Lsb,
        }
    }

    /// Deliver one event to the client that owns `window`.
    pub(crate) fn deliver_to_window_owner(&mut self, window: u32, ev: &XEvent) {
        let Some(&owner) = self.owners.get(&window) else {
            return;
        };
        let Some(Some(conn)) = self.conns.get(owner.0) else {
            return;
        };
        let bytes = ev.encode(conn.endian, conn.seq.current());
        if let Some(Some(conn)) = self.conns.get_mut(owner.0) {
            conn.out.extend_from_slice(&bytes);
        }
    }

    /// Deliver one event to a specific client (no window routing).
    fn send_input_event(&mut self, window: u32, ev: &XEvent) {
        self.deliver_to_window_owner(window, ev);
    }

    /// Deliver a pre-encoded 32-byte event to a window's owner
    /// (the SendEvent path — the bytes already carry the send-event
    /// bit).
    pub(crate) fn send_raw_to_window_owner(&mut self, window: u32, event: &[u8]) {
        let Some(&owner) = self.owners.get(&window) else {
            return;
        };
        self.out_bytes(owner, event);
    }

    /// The window's event mask (0 when unknown).
    fn mask_of(&self, window: u32) -> u32 {
        self.tree.get(window).map_or(0, |w| w.event_mask)
    }

    /// Emit Expose events for an effect's exposed gains (root
    /// coordinates → window-local, bounds-clipped, count-pinned).
    pub(crate) fn emit_exposures(&mut self, effect: &StructuralEffect) {
        for (window, rects) in &effect.exposed {
            if self.mask_of(*window) & crate::events::mask::EXPOSURE == 0 {
                continue;
            }
            let (ox, oy) = self.tree.absolute_origin(*window);
            let (ww, wh) = (
                self.tree.get(*window).map_or(0, |w| w.width),
                self.tree.get(*window).map_or(0, |w| w.height),
            );
            let mut kept: Vec<Rect> = Vec::new();
            for r in rects {
                let local = r.translate(-ox, -oy);
                if let Some(c) = local.intersect(Rect::new(0, 0, ww, wh)) {
                    kept.push(c);
                }
            }
            let total = kept.len();
            for (i, r) in kept.into_iter().enumerate() {
                let remaining = (total - i - 1) as u16;
                let ev = XEvent::Expose {
                    window: *window,
                    x: r.x.unsigned_abs() as u16,
                    y: r.y.unsigned_abs() as u16,
                    width: r.w as u16,
                    height: r.h as u16,
                    count: remaining,
                };
                self.deliver_to_window_owner(*window, &ev);
            }
        }
    }

    /// Destroy a window (and subtree) from server code: DestroyNotify
    /// to the window's client, recomposite, expose what it revealed.
    pub(crate) fn destroy_window_internal(&mut self, window: u32) {
        let parent = self.tree.get(window).and_then(|w| w.parent);
        let Ok((order, effect)) = self.tree.destroy(window) else {
            return;
        };
        // Deliver DestroyNotify while the ownership map still routes
        // it; only then drop the owners.
        for &did in &order {
            let ev = XEvent::DestroyNotify {
                event: did,
                window: did,
            };
            self.deliver_to_window_owner(did, &ev);
        }
        for did in &order {
            self.owners.remove(did);
        }
        if let Some(p) = parent {
            if self.mask_of(p) & crate::events::mask::SUBSTRUCTURE_NOTIFY != 0 {
                let ev = XEvent::DestroyNotify { event: p, window };
                self.deliver_to_window_owner(p, &ev);
            }
        }
        self.recomposite_effect(&effect);
        self.emit_exposures(&effect);
    }

    /// Recomposite the effect's dirty region into the root store.
    pub(crate) fn recomposite_effect(&mut self, effect: &StructuralEffect) {
        if let Some(dirty) = effect.dirty {
            self.mark_dirty(dirty);
            self.recomposite();
        }
    }

    /// Ensure the root store exists.
    fn ensure_root_store(&mut self) {
        if self.root_store.is_none() {
            let w = self.tree.get(self.tree.root).map_or(0, |r| r.width);
            let h = self.tree.get(self.tree.root).map_or(0, |r| r.height);
            self.root_store = Some(Store::filled(w, h, self.tree.root_background));
        }
    }

    /// Record root damage (union).
    fn mark_dirty(&mut self, rect: Rect) {
        if rect.w == 0 || rect.h == 0 {
            return;
        }
        self.damage = Some(match self.damage {
            Some(d) => d.union(rect),
            None => rect,
        });
    }

    /// Recomposite the dirty region of the root store from the window
    /// tree (bottom-to-top, exact visibility regions).
    fn recomposite(&mut self) {
        let Some(dirty) = self.damage else {
            return;
        };
        self.ensure_root_store();
        let (rw, rh) = match self.root_store.as_ref() {
            Some(s) => (s.width, s.height),
            None => return,
        };
        let Some(d) = dirty.intersect(Rect::new(0, 0, rw, rh)) else {
            return;
        };
        let root = self.tree.root;
        let order = self.stacking_order();
        for id in order {
            if id == root {
                continue;
            }
            let Some(w) = self.tree.get(id) else {
                continue;
            };
            if !w.mapped || w.class != WinClass::InputOutput {
                continue;
            }
            let (ox, oy) = self.tree.absolute_origin(id);
            let screen = self.tree.screen_region(id);
            if let Some(src) = w.store.as_ref() {
                for r in &screen {
                    if let Some(i) = r.intersect(d) {
                        if let Some(root_store) = self.root_store.as_mut() {
                            blit_to_root(root_store, src, ox, oy, &i);
                        }
                    }
                }
            } else {
                let bg = w.background_pixel.unwrap_or(0);
                for r in &screen {
                    if let Some(i) = r.intersect(d) {
                        if let Some(root_store) = self.root_store.as_mut() {
                            fill_root(root_store, &i, bg);
                        }
                    }
                }
            }
        }
    }

    /// Every window in bottom-to-top stacking order (root first).
    fn stacking_order(&self) -> Vec<u32> {
        let mut out = Vec::new();
        let mut stack = vec![self.tree.root];
        while let Some(cur) = stack.pop() {
            out.push(cur);
            if let Some(w) = self.tree.get(cur) {
                for &c in w.children.iter().rev() {
                    stack.push(c);
                }
            }
        }
        out
    }

    /// Full-surface exposure effect after a root resize.
    fn reexpose_all(&mut self) -> StructuralEffect {
        let mut effect = StructuralEffect::default();
        let width = self.tree.get(self.tree.root).map_or(0, |w| w.width);
        let height = self.tree.get(self.tree.root).map_or(0, |w| w.height);
        effect.dirty = Some(Rect::new(0, 0, width, height));
        for (id, w) in &self.tree.windows {
            if w.mapped && w.class == WinClass::InputOutput {
                let (ox, oy) = self.tree.absolute_origin(*id);
                let r = Rect::new(ox, oy, w.width, w.height);
                if let Some(i) = r.intersect(Rect::new(0, 0, width, height)) {
                    effect.exposed.push((*id, vec![i]));
                }
            }
        }
        effect
    }

    // ---- handshake ----

    fn handshake_reply(&mut self, client: ClientId, setup: &ClientSetup) -> FeedOutcome {
        let endian = setup.endian;
        if setup.major != 11 {
            let bytes =
                crate::setup::encode_setup_failed(endian, b"protocol version not supported");
            self.out_bytes(client, &bytes);
            return Err(FatalReason::Handshake);
        }
        let bytes = encode_setup_reply(
            endian,
            &self.screen,
            11_017_000,
            0x0040_0000,
            0x001f_ffff,
            65535,
            b"LionOS X Bridge",
            8,
            255,
        );
        self.out_bytes(client, &bytes);
        Ok(())
    }

    // ---- request pump ----

    fn pump(&mut self, client: ClientId) -> FeedOutcome {
        loop {
            let frame = {
                let Some(Some(conn)) = self.conns.get(client.0) else {
                    return Ok(());
                };
                if conn.inbuf.len() < 4 {
                    return Ok(());
                }
                match frame_request(&conn.inbuf, conn.endian, conn.bigreq) {
                    Ok((f, used)) => Some((f.opcode, f.data, f.payload.to_vec(), used)),
                    Err(WireError::ShortHeader) => None,
                    Err(e) => return Err(FatalReason::Wire(e)),
                }
            };
            let Some((opcode, data, payload, used)) = frame else {
                return Ok(());
            };
            {
                let Some(Some(conn)) = self.conns.get_mut(client.0) else {
                    return Ok(());
                };
                let _ = conn.seq.next();
                conn.inbuf.drain(..used);
            }
            crate::ops::dispatch_request(self, client, opcode, data, &payload)?;
        }
    }

    // ---- helpers used by ops ----

    /// Append raw bytes to a client's output.
    pub(crate) fn out_bytes(&mut self, client: ClientId, bytes: &[u8]) {
        if let Some(Some(conn)) = self.conns.get_mut(client.0) {
            conn.out.extend_from_slice(bytes);
        }
    }

    /// Enable/disable BIG-REQUESTS for one client.
    pub(crate) fn set_bigreq(&mut self, client: ClientId, on: bool) {
        if let Some(Some(conn)) = self.conns.get_mut(client.0) {
            conn.bigreq = on;
        }
    }

    /// Emit one error envelope.
    pub(crate) fn error(&mut self, client: ClientId, code: u8, value: u32, minor: u8, major: u8) {
        let (endian, seq) = match self.conns.get(client.0) {
            Some(Some(c)) => (c.endian, c.seq.current()),
            _ => return,
        };
        let mut env = Vec::with_capacity(32);
        error_envelope(&mut env, endian, seq, code, value, minor, major);
        self.out_bytes(client, &env);
    }

    /// Start a reply (the 8-byte prefix with the length word zeroed);
    /// the caller appends payload and finishes with
    /// [`Server::finish_reply`], which fixes the length word.
    pub(crate) fn reply_prefix(&mut self, client: ClientId) -> Vec<u8> {
        let (endian, seq) = match self.conns.get(client.0) {
            Some(Some(c)) => (c.endian, c.seq.current()),
            _ => (Endian::Lsb, 0),
        };
        let mut out = Vec::with_capacity(32);
        reply_header(&mut out, endian, seq, 0);
        out
    }

    /// Pad a reply to 4-byte alignment, fix the length word (4-byte
    /// units beyond the fixed 32), pad to the 32-byte minimum, emit.
    pub(crate) fn finish_reply(&mut self, client: ClientId, mut out: Vec<u8>) {
        while out.len() % 4 != 0 {
            out.push(0);
        }
        let extra = if out.len() > 32 {
            (out.len() - 32) / 4
        } else {
            0
        };
        if out.len() >= 8 {
            let endian = self.endian_of(client);
            out[4..8].copy_from_slice(&endian.put_u32(extra as u32));
        }
        while out.len() < 32 {
            out.push(0);
        }
        self.out_bytes(client, &out);
    }

    /// The client's negotiated endian (LSB before the handshake).
    pub(crate) fn endian_of(&self, client: ClientId) -> Endian {
        match self.conns.get(client.0) {
            Some(Some(c)) => c.endian,
            _ => Endian::Lsb,
        }
    }

    /// Atom table access.
    pub(crate) fn atoms(&self) -> &AtomTable {
        &self.atoms
    }

    pub(crate) fn atoms_mut(&mut self) -> &mut AtomTable {
        &mut self.atoms
    }

    /// Tree access.
    pub(crate) fn tree(&self) -> &WindowTree {
        &self.tree
    }

    pub(crate) fn tree_mut(&mut self) -> &mut WindowTree {
        &mut self.tree
    }

    /// Owner registry mutation access.
    pub(crate) fn owners_mut(&mut self) -> &mut BTreeMap<u32, ClientId> {
        &mut self.owners
    }

    /// GC store access.
    pub(crate) fn gcs_mut(&mut self) -> &mut GcStore {
        &mut self.gcs
    }

    pub(crate) fn gcs(&self) -> &GcStore {
        &self.gcs
    }

    /// Pixmap store access.
    pub(crate) fn pixmaps_mut(&mut self) -> &mut BTreeMap<u32, Store> {
        &mut self.pixmaps
    }

    pub(crate) fn pixmaps(&self) -> &BTreeMap<u32, Store> {
        &self.pixmaps
    }

    /// The tree root XID.
    pub(crate) fn root_id(&self) -> u32 {
        self.tree.root
    }

    /// Build the drawing clip for a window drawable: window bounds,
    /// minus mapped InputOutput children unless the GC is
    /// IncludeInferiors.
    pub(crate) fn window_draw_clip(&self, window: u32, gc: &Gc) -> Vec<Rect> {
        let Some(w) = self.tree.get(window) else {
            return Vec::new();
        };
        let bounds = Rect::new(0, 0, w.width, w.height);
        let mut region = ldp_core::geometry::Region::from_rect(bounds);
        if gc.subwindow_mode == crate::gc::SubwindowMode::ClipByChildren {
            for &c in &w.children {
                if let Some(cw) = self.tree.get(c) {
                    if cw.mapped && cw.class == WinClass::InputOutput {
                        let cutter = Rect::new(cw.x, cw.y, cw.width, cw.height);
                        let mut next = ldp_core::geometry::Region::new();
                        for r in &region {
                            for piece in &r.subtract(cutter) {
                                next.add(*piece);
                            }
                        }
                        region = next;
                    }
                }
            }
        }
        region.iter().copied().collect()
    }

    /// Resolve a drawable to its kind.
    pub(crate) fn drawable_kind(&self, id: u32) -> Option<DrawableKind> {
        if let Some(w) = self.tree.get(id) {
            if w.class == WinClass::InputOnly {
                return Some(DrawableKind::InputOnlyWindow);
            }
            return Some(DrawableKind::Window);
        }
        if self.pixmaps.contains_key(&id) {
            return Some(DrawableKind::Pixmap);
        }
        None
    }

    /// Ensure a window's backing store exists (lazily filled with its
    /// background).
    pub(crate) fn ensure_window_store(&mut self, window: u32) {
        let Some(w) = self.tree.get(window) else {
            return;
        };
        if w.store.is_none() {
            let bg = w.background_pixel.unwrap_or(0);
            let (width, height) = (w.width, w.height);
            if let Some(w) = self.tree.get_mut(window) {
                w.store = Some(Store::filled(width, height, bg));
            }
        }
    }

    /// Run a drawing closure against a window's store with GC
    /// clipping, then recomposite the visible part of the damage.
    pub(crate) fn draw_on_window(&mut self, window: u32, gc: &Gc, f: impl FnOnce(&mut Target<'_>)) {
        self.ensure_window_store(window);
        let mut clip = self.window_draw_clip(window, gc);
        if !gc.clip_rects.is_empty() {
            let shifted: Vec<Rect> = gc
                .clip_rects
                .iter()
                .map(|r| r.translate(gc.clip_origin.0, gc.clip_origin.1))
                .collect();
            clip = intersect_rect_lists(&clip, &shifted);
        }
        let origin = self.tree.absolute_origin(window);
        let Some(w) = self.tree.get_mut(window) else {
            return;
        };
        let Some(store) = w.store.as_mut() else {
            return;
        };
        let mut target = Target::full(store, gc.function, gc.plane_mask).with_clip(clip);
        f(&mut target);
        let damage = target.take_damage();
        if let Some(d) = damage {
            let root_d = d.translate(origin.0, origin.1);
            let screen = self.tree.screen_region(window);
            for r in &screen {
                if let Some(i) = r.intersect(root_d) {
                    self.mark_dirty(i);
                }
            }
            self.recomposite();
        }
    }

    /// Run a drawing closure against a pixmap's store.
    pub(crate) fn draw_on_pixmap(&mut self, pixmap: u32, gc: &Gc, f: impl FnOnce(&mut Target<'_>)) {
        let Some(store) = self.pixmaps.get_mut(&pixmap) else {
            return;
        };
        let bounds = Rect::new(0, 0, store.width, store.height);
        let clip = gc.effective_clip(bounds);
        let mut target = Target::full(store, gc.function, gc.plane_mask).with_clip(clip);
        f(&mut target);
    }

    /// Emit a PropertyNotify.
    pub(crate) fn property_notify(&mut self, window: u32, atom: u32, time: u32, deleted: bool) {
        if self.mask_of(window) & crate::events::mask::PROPERTY_CHANGE == 0 {
            return;
        }
        let ev = XEvent::PropertyNotify {
            window,
            atom,
            time,
            deleted,
        };
        self.deliver_to_window_owner(window, &ev);
    }

    /// Convert a property-store error to an X error and emit it.
    pub(crate) fn prop_error(&mut self, client: ClientId, e: PropError, value: u32, major: u8) {
        match e {
            PropError::Mismatch => self.error(client, crate::events::err::MATCH, value, 0, major),
            PropError::Limit => self.error(client, crate::events::err::ALLOC, value, 0, major),
            PropError::BadFormat(f) => {
                self.error(client, crate::events::err::VALUE, u32::from(f), 0, major);
            }
        }
    }

    /// SHM state access.
    pub(crate) fn shm_state_mut(&mut self) -> &mut ShmState {
        &mut self.shm
    }

    pub(crate) fn shm_state(&self) -> &ShmState {
        &self.shm
    }

    pub(crate) fn shm_host_mut(&mut self) -> &mut dyn ShmHost {
        self.shm_host.as_mut()
    }

    /// Record a ConfigureNotify (StructureNotify on the window,
    /// SubstructureNotify on the parent).
    pub(crate) fn configure_notify(&mut self, window: u32) {
        let Some(w) = self.tree.get(window) else {
            return;
        };
        let (x, y, width, height, bw) = (w.x, w.y, w.width, w.height, w.border_width);
        let parent = w.parent;
        let or = w.override_redirect;
        if self.mask_of(window) & crate::events::mask::STRUCTURE_NOTIFY != 0 {
            let ev = XEvent::ConfigureNotify {
                event: window,
                window,
                above_sibling: 0,
                x: x as i16,
                y: y as i16,
                width: width as u16,
                height: height as u16,
                border_width: bw as u16,
                override_redirect: or,
            };
            self.deliver_to_window_owner(window, &ev);
        }
        if let Some(p) = parent {
            if self.mask_of(p) & crate::events::mask::SUBSTRUCTURE_NOTIFY != 0 {
                let ev = XEvent::ConfigureNotify {
                    event: p,
                    window,
                    above_sibling: 0,
                    x: x as i16,
                    y: y as i16,
                    width: width as u16,
                    height: height as u16,
                    border_width: bw as u16,
                    override_redirect: or,
                };
                self.deliver_to_window_owner(p, &ev);
            }
        }
    }

    /// Map/Unmap notify pair (window + parent).
    pub(crate) fn map_notify(&mut self, window: u32, mapped: bool) {
        let Some(w) = self.tree.get(window) else {
            return;
        };
        let parent = w.parent;
        let or = w.override_redirect;
        if self.mask_of(window) & crate::events::mask::STRUCTURE_NOTIFY != 0 {
            let ev = if mapped {
                XEvent::MapNotify {
                    event: window,
                    window,
                    override_redirect: or,
                }
            } else {
                XEvent::UnmapNotify {
                    event: window,
                    window,
                    from_configure: false,
                }
            };
            self.deliver_to_window_owner(window, &ev);
        }
        if let Some(p) = parent {
            if self.mask_of(p) & crate::events::mask::SUBSTRUCTURE_NOTIFY != 0 {
                let ev = if mapped {
                    XEvent::MapNotify {
                        event: p,
                        window,
                        override_redirect: or,
                    }
                } else {
                    XEvent::UnmapNotify {
                        event: p,
                        window,
                        from_configure: false,
                    }
                };
                self.deliver_to_window_owner(p, &ev);
            }
        }
    }

    /// CreateNotify to the parent.
    pub(crate) fn create_notify(&mut self, window: u32) {
        let Some(w) = self.tree.get(window) else {
            return;
        };
        let Some(parent) = w.parent else {
            return;
        };
        if self.mask_of(parent) & crate::events::mask::SUBSTRUCTURE_NOTIFY == 0 {
            return;
        }
        let ev = XEvent::CreateNotify {
            parent,
            window,
            x: w.x as i16,
            y: w.y as i16,
            width: w.width as u16,
            height: w.height as u16,
            border_width: w.border_width as u16,
            override_redirect: w.override_redirect,
        };
        self.deliver_to_window_owner(parent, &ev);
    }

    /// ClearArea: fill or expose, per background semantics.
    pub(crate) fn clear_area(
        &mut self,
        window: u32,
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        exposures: bool,
    ) {
        let Some(win) = self.tree.get(window) else {
            return;
        };
        let (ww, wh) = (win.width, win.height);
        let has_bg = win.background_pixel.is_some();
        // Zero width/height means "to the far edge".
        let w = if w == 0 {
            (i64::from(ww) - i64::from(x)).max(0) as u32
        } else {
            w
        };
        let h = if h == 0 {
            (i64::from(wh) - i64::from(y)).max(0) as u32
        } else {
            h
        };
        let Some(rect) = Rect::new(x, y, w, h).intersect(Rect::new(0, 0, ww, wh)) else {
            return;
        };
        if has_bg {
            let bg = win.background_pixel.unwrap_or(0);
            self.ensure_window_store(window);
            if let Some(win) = self.tree.get_mut(window) {
                if let Some(store) = win.store.as_mut() {
                    let mut t = Target::full(store, crate::render::Gx::Copy, 0x00ff_ffff);
                    t.fill_rect(rect.x, rect.y, rect.w, rect.h, bg);
                }
            }
            let (ox, oy) = self.tree.absolute_origin(window);
            let root_d = rect.translate(ox, oy);
            let screen = self.tree.screen_region(window);
            for r in &screen {
                if let Some(i) = r.intersect(root_d) {
                    self.mark_dirty(i);
                }
            }
            self.recomposite();
        } else if exposures {
            let (ox, oy) = self.tree.absolute_origin(window);
            let root_d = rect.translate(ox, oy);
            let effect = StructuralEffect {
                exposed: vec![(window, vec![root_d])],
                dirty: Some(root_d),
            };
            self.emit_exposures(&effect);
        }
    }

    /// Selection registry access.
    pub(crate) fn selection_mut(&mut self) -> &mut BTreeMap<u32, (u32, u32)> {
        &mut self.selection
    }

    /// Cursor registry access.
    pub(crate) fn cursors_mut(&mut self) -> &mut BTreeSet<u32> {
        &mut self.cursors
    }

    pub(crate) fn cursors(&self) -> &BTreeSet<u32> {
        &self.cursors
    }

    /// Font registry access.
    pub(crate) fn fonts_mut(&mut self) -> &mut BTreeSet<u32> {
        &mut self.fonts
    }

    pub(crate) fn fonts(&self) -> &BTreeSet<u32> {
        &self.fonts
    }

    /// Colormap registry access.
    pub(crate) fn colormaps_mut(&mut self) -> &mut BTreeSet<u32> {
        &mut self.colormaps
    }

    pub(crate) fn colormaps(&self) -> &BTreeSet<u32> {
        &self.colormaps
    }

    /// The default colormap XID (from the screen).
    pub(crate) fn default_colormap(&self) -> u32 {
        self.screen.colormap
    }

    /// The screen's visual XID.
    pub(crate) fn visual(&self) -> u32 {
        self.screen.visual
    }
}

/// Fill a root-store rectangle with one pixel.
fn fill_root(root: &mut Store, rect: &Rect, pixel: u32) {
    let mut t = Target::full(root, crate::render::Gx::Copy, 0x00ff_ffff);
    t.fill_rect(rect.x, rect.y, rect.w, rect.h, pixel);
}

/// Pairwise intersection of two rectangle lists.
pub(crate) fn intersect_rect_lists(a: &[Rect], b: &[Rect]) -> Vec<Rect> {
    let mut out = Vec::new();
    for ra in a {
        for rb in b {
            if let Some(i) = ra.intersect(*rb) {
                out.push(i);
            }
        }
    }
    out
}

/// Blit one rectangle from a window store to the root store.
fn blit_to_root(root: &mut Store, src: &Store, ox: i32, oy: i32, rect: &Rect) {
    let mut t = Target::full(root, crate::render::Gx::Copy, 0x00ff_ffff);
    for y in rect.y..rect.y + rect.h as i32 {
        for x in rect.x..rect.x + rect.w as i32 {
            if let Some(p) = src.pixel(x - ox, y - oy) {
                t.put(x, y, p);
            }
        }
    }
}
