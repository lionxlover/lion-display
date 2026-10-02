//! The evdev input path (Phase 33): real devices through the seat
//! router onto the wire.
//!
//! The pure stack was already built and unit-pinned — `ldp-input`'s
//! evdev codec and `/dev/input` backend, `ldp-seat`'s router with
//! its focus/grab/accel/xkb/gesture machinery. This module is the
//! *serving* half: the compositor opens the machine's devices,
//! decodes and normalizes their frames, builds the routing view of
//! the scene, feeds the router, and delivers the routed protocol
//! events to clients through the outbox — the same cross-client
//! vehicle the presentation and data families use.
//!
//! The wake contract (honest, documented): routed events park in the
//! per-client outbox queues and stream at each client's next wake
//! point — its next inbound message. Interactive clients keep the
//! ping-pong alive (a frame request outstanding, the animation
//! cycle); a push-side session wake (an eventfd per session) is the
//! self-scheduled-render-thread roadmap line. Until then the tests
//! drive delivery exactly the way real interactive clients do: by
//! roundtripping.
//!
//! The keyboard focus policy is the shell's (the routing doctrine:
//! the scene supplies it, the router never guesses): click-to-focus,
//! initially the topmost mapped root. The pointer domain needs no
//! policy — it follows the input-region hit test. Roots are the v1
//! routing targets (one binding per client, addressed at the root's
//! device objects); per-subsurface bindings arrive with the shell
//! protocol's growth.
//!
//! The scripted source: tests (and only tests) queue normalized
//! event batches through [`World::queue_input`] and run the same
//! pump the real devices take — the fd read is the only difference,
//! which is what keeps the end-to-end suite honest.
//!
//! Phase 50 — the operator's hand: the pump is also the drag engine's
//! heartbeat. A live interactive drag (a `toplevel.start_move`/
//! `start_resize` grip) advances on every motion batch — the move's
//! geometry applies here (server truth), the resize's proposals mint
//! here (the machine's two-phase commit) — and the button's release
//! retires it. The `advance_drag`/`end_drag` methods below and the
//! shell's `DragHost` carry the mechanism's own story.

#![forbid(unsafe_code)]

use std::collections::{HashMap, VecDeque};
use std::io::Read as _;
use std::os::fd::{AsRawFd, RawFd};

use ldp_compositor::surface::{Surface, SurfaceId};
use ldp_core::ids::{ClientId, ObjectId};
use ldp_core::time::Mono;
use ldp_input::device::DeviceClass;
use ldp_input::evdev::{Framer, StreamDecoder};
use ldp_input::normalizer::{InputEvent, Normalizer};
use ldp_seat::event::SeatEvent;
use ldp_seat::focus::{
    ClientBinding, InputRegion, RectF, Scene as RouteScene, SurfaceKey, SurfaceRef,
};
use ldp_seat::route::{Router, RouterConfig};

use crate::outbox::OutboxEntry;
use crate::scene::World;

/// The input mode (CLI `--input`, `CompositorConfig.input`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum InputMode {
    /// On with real devices when the DRM mode serves, off headless
    /// (the CI byte-exactness doctrine: the mock path's pixels never
    /// change because input never arrives).
    #[default]
    Auto,
    /// Always on (the operator's explicit ask; a machine with no
    /// `/dev/input` honestly reports zero devices).
    On,
    /// Off.
    Off,
}

impl InputMode {
    /// Parse the CLI value.
    ///
    /// # Errors
    /// [`crate::sys::SysError`] never — a plain `Err(String)` keeps
    /// the CLI's error type simple; the message names the flag.
    pub fn parse(v: &str) -> Result<InputMode, String> {
        match v {
            "auto" => Ok(InputMode::Auto),
            "on" => Ok(InputMode::On),
            "off" => Ok(InputMode::Off),
            other => Err(format!("--input needs auto|on|off (got {other})")),
        }
    }

    /// The effective on/off for a given serve mode.
    #[must_use]
    pub const fn effective(self, drm: bool) -> bool {
        match self {
            InputMode::On => true,
            InputMode::Off => false,
            InputMode::Auto => drm,
        }
    }
}

/// One real evdev device: the owned descriptor, its decoder and
/// framer, and its normalizer (the per-device state the normalized
/// units depend on).
struct InputDevice {
    class: DeviceClass,
    file: std::fs::File,
    decoder: StreamDecoder,
    framer: Framer,
    normalizer: Normalizer,
}

impl InputDevice {
    /// Read the fd dry and normalize: one batch of input events per
    /// call site (would-block ends the drain honestly).
    fn drain(&mut self) -> Option<Vec<InputEvent>> {
        let mut buf = [0u8; 4096];
        let Ok(got) = self.file.read(&mut buf) else {
            return None; // would-block: the fd is dry
        };
        if got == 0 {
            return None; // no device emits EOF; defensive
        }
        let raw = self.decoder.feed(&buf[..got]);
        let frames = self.framer.feed_all(&raw);
        let mut out = Vec::new();
        for frame in frames {
            out.extend(self.normalizer.normalize_frame(&frame));
        }
        Some(out)
    }
}

/// The per-client device-object mints (what the router addresses).
#[derive(Clone, Debug, Default)]
pub struct DeviceObjects {
    /// The client's `ldp.input.pointer`.
    pub pointer: Option<ObjectId>,
    /// The client's `ldp.input.keyboard`.
    pub keyboard: Option<ObjectId>,
    /// The client's `ldp.input.touch`.
    pub touch: Option<ObjectId>,
    /// The client's `ldp.input.tablet`.
    pub tablet: Option<ObjectId>,
    /// The client's `ldp.input.gestures`.
    pub gestures: Option<ObjectId>,
}

/// The keymap bundle: the xkb v1 text (cloned into a read-only memfd
/// per receiving client) — the compiled real keymap when
/// libxkbcommon is present, the honest empty fallback otherwise.
struct KeymapBundle {
    blob: Vec<u8>,
}

impl KeymapBundle {
    /// The read-only memfd carrying the keymap text.
    fn memfd(&self) -> Option<std::os::fd::OwnedFd> {
        use std::io::Write as _;
        let fd = crate::sys::memfd("ldp-keymap").ok()?;
        let mut file = std::fs::File::from(fd);
        file.write_all(&self.blob).ok()?;
        file.set_len(self.blob.len() as u64).ok()?;
        Some(into_fd(file))
    }
}

/// The served input state: the router, the bindings, the focus, the
/// devices.
pub struct InputState {
    /// The one seat's router (seat0; multi-seat is broker-era work).
    router: Router,
    /// Per-client device-object mints.
    bindings: HashMap<ClientId, DeviceObjects>,
    /// The shell's keyboard focus (click-to-focus).
    keyboard_focus: Option<SurfaceId>,
    /// The keymap (None only when even the memfd failed).
    keymap: Option<KeymapBundle>,
    /// The real devices (empty when off, headless, or no /dev/input).
    devices: Vec<InputDevice>,
    /// The scripted feed (the test path).
    scripted: VecDeque<(DeviceClass, Vec<InputEvent>)>,
    /// Whether the input path serves at all.
    enabled: bool,
}

impl InputState {
    /// Bring the input state up: the keymap (real or fallback), the
    /// router with its xkb state, and — when on — the machine's
    /// devices.
    #[must_use]
    pub fn bring_up(mode: InputMode, drm: bool) -> InputState {
        let enabled = mode.effective(drm);
        // The keymap: libxkbcommon's compiled default when the
        // library and its keymap files exist, the honest fallback
        // otherwise (a well-formed empty v1 keymap — raw keycodes
        // still route, modifiers report none).
        let (blob, state, fd) = match real_keymap() {
            Some((text, state, fd)) => (text, Some(state), Some(fd)),
            None => (b"xkb_keymap {\n};\n\0".to_vec(), None, None),
        };
        let mut router = Router::new(RouterConfig::default());
        if let (Some(state), Some(fd)) = (state, fd) {
            // Install with no clients yet: the state drives the
            // modifier decomposition; the per-client keymap events
            // ride each keyboard's mint instead (below). The returned
            // broadcast is empty by construction (no bindings yet).
            let _ = router.set_keymap(state, fd, &[]);
        }
        let mut input = InputState {
            router,
            bindings: HashMap::new(),
            keyboard_focus: None,
            keymap: Some(KeymapBundle { blob }),
            devices: Vec::new(),
            scripted: VecDeque::new(),
            enabled,
        };
        if enabled {
            input.open_devices();
        }
        input
    }

    /// Open the machine's devices: enumerate, probe, open. A machine
    /// with no `/dev/input` (a container, a VM without input) yields
    /// zero devices — a normal state, reported honestly on the
    /// startup line.
    fn open_devices(&mut self) {
        // No /dev/input at all — honestly empty.
        let nodes = ldp_input::backend::enumerate().unwrap_or_default();
        for path in nodes {
            // Permission or the node raced away.
            let Ok(file) = ldp_input::backend::open_device(&path) else {
                continue;
            };
            // Not an evdev node after all.
            let Ok(spec) = ldp_input::backend::probe(&file) else {
                continue;
            };
            // Only the classes the router consumes; the rest (sensors,
            // LEDs, switches) are observed, not hidden — they simply
            // carry nothing the seat routes.
            let class = spec.classify();
            if !matches!(
                class,
                DeviceClass::Mouse
                    | DeviceClass::Touchpad
                    | DeviceClass::Touchscreen
                    | DeviceClass::Tablet
                    | DeviceClass::Keyboard
            ) {
                continue;
            }
            self.devices.push(InputDevice {
                class,
                file,
                decoder: StreamDecoder::new(),
                framer: Framer::new(),
                normalizer: Normalizer::new(spec),
            });
        }
    }

    /// The poll rows for the serve loop (the real devices' fds).
    pub fn interests(&self) -> Vec<RawFd> {
        self.devices.iter().map(|d| d.file.as_raw_fd()).collect()
    }

    /// The startup report line (honest: what serves, or why not).
    #[must_use]
    pub fn report(&self) -> String {
        if self.enabled {
            let keymap = match self.keymap.as_ref() {
                Some(k) if k.blob.len() > 16 => "compiled",
                _ => "fallback",
            };
            format!(
                "input: {} device(s) over evdev (seat0, keymap {keymap})",
                self.devices.len()
            )
        } else {
            "input: off".to_owned()
        }
    }

    /// Queue one normalized event batch (the scripted/test source —
    /// the same pump the real devices take).
    pub fn queue(&mut self, class: DeviceClass, events: Vec<InputEvent>) {
        self.scripted.push_back((class, events));
    }

    /// Note a device-object mint (the dispatch side of
    /// `seat.get_*`). Returns the keymap descriptor when the mint is
    /// a keyboard (the client's `keyboard.keymap` rides the same
    /// dispatch).
    pub fn note_device(
        &mut self,
        client: ClientId,
        kind: &str,
        object: ObjectId,
    ) -> Option<std::os::fd::OwnedFd> {
        let entry = self.bindings.entry(client).or_default();
        match kind {
            "get_pointer" => {
                entry.pointer = Some(object);
                None
            }
            "get_keyboard" => {
                entry.keyboard = Some(object);
                self.keymap.as_ref().and_then(KeymapBundle::memfd)
            }
            "get_touch" => {
                entry.touch = Some(object);
                None
            }
            "get_tablet" => {
                entry.tablet = Some(object);
                None
            }
            _ => {
                entry.gestures = Some(object);
                None
            }
        }
    }

    /// Forget a client (its outbox queue drops with the session's
    /// own teardown; the router's grabs dismiss through the
    /// surface-gone reflex).
    pub fn client_gone(&mut self, client: ClientId) {
        self.bindings.remove(&client);
    }

    /// The current pointer position (output coordinates) — the
    /// operator's introspection and the tests' oracle.
    #[must_use]
    pub fn pointer_position(&self) -> (f32, f32) {
        let p = self.router.pointer_position();
        (p.x, p.y)
    }

    /// The current pointer focus (the routed surface).
    #[must_use]
    pub fn pointer_focus(&self) -> Option<SurfaceId> {
        self.router
            .pointer_focus()
            .map(|k| SurfaceId::from_raw(k.as_u64()))
    }

    /// The current keyboard focus (the shell's policy target).
    #[must_use]
    pub fn keyboard_focus(&self) -> Option<SurfaceId> {
        self.keyboard_focus
    }
}

impl World {
    /// The scripted input feed (tests): queue one normalized batch.
    pub fn queue_input(&mut self, class: DeviceClass, events: Vec<InputEvent>) {
        self.input.queue(class, events);
    }

    /// The input pump: every ready device (and the whole scripted
    /// queue) through the router, the routed events into the
    /// outboxes. Returns the number of routed protocol events.
    ///
    /// This is the real input arrival (Phase 31's rig becomes the
    /// true path): the activity mark, the render trigger, and the
    /// input→photon arm ride every batch.
    pub fn pump_input(&mut self) -> usize {
        let mut total = 0usize;
        // The scripted source first (tests drive the same path).
        while let Some((class, events)) = self.input.scripted.pop_front() {
            total += self.route_one(class, &events);
        }
        // The real devices: each fd drained dry, batch by batch.
        let count = self.input.devices.len();
        for i in 0..count {
            while let Some(events) = self.input.devices[i].drain() {
                let class = self.input.devices[i].class;
                if !events.is_empty() {
                    total += self.route_one(class, &events);
                }
            }
        }
        total
    }

    /// Begin an interactive drag (Phase 50 — the operator's hand, the
    /// dispatcher's `start_move`/`start_resize` arm's recording step):
    /// one hand — a fresh grip retires any live one, the retired
    /// drag's `resizing` bit clearing with it (the new regime's
    /// proposals carry their own truth).
    pub fn begin_drag(&mut self, drag: crate::shell::LiveDrag) {
        if let Some(old) = self.drags.begin(drag) {
            self.quiet_drag(&old);
        }
    }

    /// End the drag without a final proposal (the window's death, the
    /// role object's death, the geometry verb, the superseding grip):
    /// the machine's `resizing` bit clears — the drag is over,
    /// whatever killed it — nothing is emitted (the killer's own
    /// narrative is the emission, if any).
    pub fn end_drag_silent(&mut self) {
        if let Some(drag) = self.drags.end() {
            self.quiet_drag(&drag);
        }
    }

    /// Clear the `resizing` bit of a retired drag's machine (the
    /// quiet end's one piece of hygiene).
    fn quiet_drag(&mut self, drag: &crate::shell::LiveDrag) {
        if let Some(entry) = self.toplevels.entry_mut(drag.client, drag.object) {
            entry.machine.set_resizing(false);
        }
    }

    /// Advance the live drag to the pointer's current position (the
    /// input pump's per-batch companion, Phase 50). A *move*
    /// re-anchors the window's geometry now — server truth, applied
    /// through `set_position_now` (the damage engine's R2 rule
    /// repaints both ends), the keep band holding the title grip
    /// reachable inside the work area. A *resize* computes the
    /// edge-algebra target, clamps it against the client's own
    /// min/max grammar (logical), and — when the clamped target
    /// changed — proposes it through the two-phase commit (the
    /// outbox entry carries the machine's proposal; the grace window
    /// and the outbox's own freshest-wins coalescing make the
    /// pointer-paced cadence honest for a frame-cadence client).
    /// Returns the proposal's outbox entry, if one was minted.
    #[allow(clippy::too_many_lines)] // one narrative: the move's geometry, the resize's proposal
    fn advance_drag(&mut self) -> Option<OutboxEntry> {
        use crate::shell::{clamp_drag_position, resize_target, DragMode};
        let pointer = self.input.pointer_position();
        let (client, object, surface, mode, delta, window_start, last_size) = {
            let drag = self.drags.live()?;
            (
                drag.client,
                drag.object,
                drag.surface,
                drag.mode,
                drag.delta(pointer),
                drag.window_start,
                drag.last_size,
            )
        };
        // The belt-and-braces eligibility: the sweeps end the drag on
        // every death; a drag that slipped through ends itself here.
        let still_live = self
            .scene
            .tree
            .get(surface)
            .is_some_and(|s| s.state().is_mapped())
            && !self.scene.hidden.contains(&surface);
        if !still_live {
            self.end_drag_silent();
            return None;
        }
        let policy = self.toplevel_policy(surface, None);
        let usable = self.shell.usable_physical();
        let keep = crate::shell::scale_axis(crate::shell::DRAG_KEEP_LOGICAL, policy.scale);
        match mode {
            DragMode::Move => {
                let rect = self
                    .scene
                    .tree
                    .get(surface)
                    .map(Surface::last_bounds)
                    .expect("the eligibility check just proved the surface");
                let raw_x = window_start.x + delta.0;
                let raw_y = window_start.y + delta.1;
                let (x, y) =
                    clamp_drag_position(raw_x, raw_y, rect.w as i32, rect.h as i32, usable, keep);
                if (x, y) != (rect.x, rect.y) {
                    self.scene.tree.set_position_now(surface, x, y).ok();
                    self.scene.dirty = true;
                }
                None
            }
            DragMode::Resize(edges) => {
                // The unclamped physical target (the engaged edges
                // follow the drag's delta, the opposite corner
                // anchors), then the logical content size it names.
                let raw = resize_target(window_start, edges, delta.0, delta.1);
                let logical_w = crate::shell::unscale_axis(raw.w as i32, policy.scale).max(1);
                let logical_h = crate::shell::unscale_axis(raw.h as i32, policy.scale).max(1);
                let ask = (
                    u32::try_from(logical_w).unwrap_or(1),
                    u32::try_from(logical_h).unwrap_or(1),
                );
                let (tw, th) = self
                    .toplevels
                    .entry(client, object)
                    .map_or(ask, |entry| entry.machine.clamp_size(ask.0, ask.1));
                if (tw, th) == last_size {
                    return None; // a still-target motion proposes nothing
                }
                // The clamped physical extent and the position it
                // implies (the engaged left/top edges' target — the
                // opposite borders' anchors hold).
                let phys_w = crate::shell::scale_axis(tw as i32, policy.scale);
                let phys_h = crate::shell::scale_axis(th as i32, policy.scale);
                let right = window_start.x + window_start.w as i32;
                let bottom = window_start.y + window_start.h as i32;
                let x = if edges.grabs_left() {
                    right - phys_w
                } else {
                    window_start.x
                };
                let y = if edges.grabs_top() {
                    bottom - phys_h
                } else {
                    window_start.y
                };
                // The bookkeeping first (the drag's cadence), then the
                // machine's proposal — the position the realize takes
                // rides the entry, exactly the size's own anchor.
                if let Some(drag) = self.drags.live_mut() {
                    drag.last_size = (tw, th);
                    drag.proposed = true;
                }
                if let Some(entry) = self.toplevels.entry_mut(client, object) {
                    entry.drag_pos = Some((x, y));
                    let proposal = entry.machine.propose_drag(&policy, (tw, th));
                    let client_id = ClientId::new(client)?;
                    return Some(OutboxEntry::position_state(
                        client_id,
                        ObjectId::from_wire(object),
                        "configure",
                        crate::shell::toplevel_configure_values(&proposal),
                        crate::outbox::POSITION_DRAG_CONFIGURE,
                    ));
                }
                self.end_drag_silent();
                None
            }
        }
    }

    /// End the live drag at the button's release (Phase 50): a
    /// *resize* mints the final proposal — the `resizing` state
    /// clears, the last target size stands (the client acks, its next
    /// commit realizes; the two-phase contract holds through the
    /// drag's whole life) — and a *move* ends silently (the geometry
    /// already stands). Returns the final proposal's outbox entry, if
    /// one was minted (a drag that never proposed — no motion, or a
    /// move — has nothing to say).
    fn end_drag(&mut self) -> Option<OutboxEntry> {
        let drag = self.drags.end()?;
        if !drag.proposed {
            self.quiet_drag(&drag);
            return None;
        }
        let policy = self.toplevel_policy(drag.surface, None);
        let entry = self.toplevels.entry_mut(drag.client, drag.object)?;
        entry.machine.set_resizing(false);
        let proposal = entry.machine.propose_sized(&policy, drag.last_size);
        let client = ClientId::new(drag.client)?;
        Some(OutboxEntry::event(
            client,
            ObjectId::from_wire(drag.object),
            "configure",
            crate::shell::toplevel_configure_values(&proposal),
        ))
    }

    /// Route one normalized batch: the scene view, the router, the
    /// click-to-focus policy, the delivery.
    fn route_one(&mut self, class: DeviceClass, events: &[InputEvent]) -> usize {
        let now: Mono = self.device.now();
        // The real input arrival: activity, the render trigger, the
        // latency arm (Phase 31's rig, now the true path).
        self.inject_input();
        // Click-to-focus: a button press re-targets the keyboard
        // focus at the pointer's hit (the shell's policy — the
        // router only reads what the scene supplies).
        if events
            .iter()
            .any(|e| matches!(e, InputEvent::Button { pressed: true, .. }))
        {
            let hit = self.pointer_hit();
            if hit != self.input.keyboard_focus {
                self.retarget_keyboard_focus(hit);
            }
        }
        // The routing view of the scene.
        let (surfaces, bindings, focus, bounds) = self.route_scene();
        let scene = RouteScene {
            surfaces: &surfaces,
            clients: &bindings,
            keyboard_focus: focus,
            bounds,
        };
        let routed = self.input.router.feed_frame(class, events, now, &scene);
        for r in &routed {
            if let Some(entry) = routed_entry(r) {
                self.outboxes.push(entry);
            }
        }
        // Phase 50 — the operator's hand: a live drag rides the
        // pointer's freshest sample (the same position state the
        // routed motion serves — the router's position is the truth
        // both consumers read). A move re-anchors the window's
        // geometry now (server truth, the keep band holding the grip
        // reachable); a resize computes the edge-algebra target and
        // proposes when the clamped size changed (the client's
        // ack+commit realizes it — the two-phase contract, never
        // tearing).
        if events
            .iter()
            .any(|e| matches!(e, InputEvent::PointerMotion { .. }))
        {
            if let Some(entry) = self.advance_drag() {
                self.outboxes.push(entry);
            }
        }
        // The drag ends at the button's release (the grip lifts): the
        // resize's final proposal clears the `resizing` state and
        // carries the last target; a move ends silently (the geometry
        // already stands).
        if events
            .iter()
            .any(|e| matches!(e, InputEvent::Button { pressed: false, .. }))
        {
            if let Some(entry) = self.end_drag() {
                self.outboxes.push(entry);
            }
        }
        routed.len()
    }

    /// The pointer's current hit (the topmost mapped root under the
    /// router's position).
    fn pointer_hit(&mut self) -> Option<SurfaceId> {
        let (surfaces, bindings, focus, bounds) = self.route_scene();
        let scene = RouteScene {
            surfaces: &surfaces,
            clients: &bindings,
            keyboard_focus: focus,
            bounds,
        };
        let pos = self.input.router.pointer_position();
        let point = ldp_core::geometry::PointF::new(pos.x, pos.y);
        ldp_seat::focus::hit_test(scene.surfaces, point).map(|k| SurfaceId::from_raw(k.as_u64()))
    }

    /// Move the keyboard focus: `keyboard.leave` to the old owner,
    /// `keyboard.enter` (with the held keys) to the new one (the
    /// dialog mapping's focus handoff — Phase 48 — and the
    /// click-to-focus policy's own arm). Phase 51 — the focus truth:
    /// the transition also drives the `activated` state bit (the
    /// frozen flag riding real proposals for the first time) — the
    /// old toplevel clears it, the new one sets it; a non-toplevel
    /// surface (a dialog, a popup) holds no bit itself, but the old
    /// toplevel still clears when one takes the keys.
    pub(crate) fn retarget_keyboard_focus(&mut self, to: Option<SurfaceId>) {
        let old = std::mem::replace(&mut self.input.keyboard_focus, to);
        let keys: Vec<u32> = self.input.router.keys_held().to_vec();
        if let Some(old_id) = old {
            if Some(old_id) != to {
                if let Some(entry) = self.keyboard_transition(old_id, false, &keys) {
                    self.outboxes.push(entry);
                }
                if let Some(entry) = self.drive_activation(old_id, false) {
                    self.outboxes.push(entry);
                }
            }
        }
        if let Some(new_id) = to {
            if Some(new_id) != old {
                if let Some(entry) = self.keyboard_transition(new_id, true, &keys) {
                    self.outboxes.push(entry);
                }
                if let Some(entry) = self.drive_activation(new_id, true) {
                    self.outboxes.push(entry);
                }
            }
        }
    }

    /// Phase 51 — the focus truth: drive the `activated` state bit on
    /// one focus transition and propose it through the grace-parking
    /// state proposal (a focus move the client never asked for never
    /// punishes a client draining a drag's serials). Toplevel-role
    /// surfaces only — the bit is toplevel vocabulary. `None` when
    /// the surface holds no toplevel or the bit already matches (a
    /// re-press on the focused window proposes nothing; a minimize
    /// verb's own proposal already cleared it).
    fn drive_activation(&mut self, surface: SurfaceId, activated: bool) -> Option<OutboxEntry> {
        // The policy inputs first (the immutable reads: the usable
        // area, the primary's scale, the decoration, the home space),
        // then the machine's own transition.
        let policy = self.toplevel_policy(surface, None);
        let entry = self.toplevels.by_surface_mut(surface)?;
        if entry.machine.wanted().activated() == activated {
            return None;
        }
        entry.machine.set_activated(activated);
        let proposal = entry.machine.propose_state(&policy);
        let client = ClientId::new(entry.client)?;
        Some(OutboxEntry::event(
            client,
            ObjectId::from_wire(entry.object),
            "configure",
            crate::shell::toplevel_configure_values(&proposal),
        ))
    }

    /// Phase 51 — the focus follows the desktop's truth: when the
    /// focus holder leaves (hidden by a state or the view switch,
    /// unmapped, destroyed), the keys land on the frontmost window
    /// that remains — the topmost toplevel or dialog in the routing
    /// order — never on `None` while a window could take them (the
    /// macOS/DWM key-window doctrine; popups hold no keys — they are
    /// pointer-grab surfaces). No-op when the frontmost already
    /// holds the focus (the common case — nothing transitions).
    pub(crate) fn promote_focus(&mut self) {
        self.promote_focus_where(None);
    }

    /// The view switch's own arm (Phase 51): the keys land on the
    /// frontmost window *homed on the newly-viewed space* (the
    /// sticky windows show everywhere, but the switch's keys belong
    /// to the space's own windows); an empty space falls back to the
    /// frontmost visible window (a sticky window's own truth).
    pub(crate) fn promote_focus_on_space(&mut self, space: u32) {
        self.promote_focus_where(Some(space));
    }

    /// The shared promotion: the routing order's toplevels and
    /// dialogs, top-first, filtered by the caller's space preference
    /// when one rides the view.
    fn promote_focus_where(&mut self, space: Option<u32>) {
        let candidates: Vec<SurfaceId> = {
            let (surfaces, _bindings, _focus, _bounds) = self.route_scene();
            surfaces
                .into_iter()
                .map(|s| SurfaceId::from_raw(s.key.as_u64()))
                .filter(|id| {
                    self.toplevels.by_surface(*id).is_some() || self.dialogs.is_dialog_surface(*id)
                })
                .collect()
        };
        let target = match space {
            Some(sp) => candidates
                .iter()
                .copied()
                .find(|id| self.home_space_of(*id) == Some(sp))
                .or_else(|| candidates.first().copied()),
            None => candidates.first().copied(),
        };
        if target != self.input.keyboard_focus {
            self.retarget_keyboard_focus(target);
        }
    }

    /// The space a focus candidate is *homed* on (Phase 51): a
    /// toplevel's own assignment, a dialog's parent's (the sheet
    /// moves with its window). `None` for anything else.
    fn home_space_of(&self, id: SurfaceId) -> Option<u32> {
        if self.toplevels.by_surface(id).is_some() {
            self.spaces
                .space_of(ldp_shell::WindowKey::new(id.raw()))
                .ok()
        } else {
            self.dialogs.by_surface_ids(id).and_then(|(_, _, parent)| {
                self.spaces
                    .space_of(ldp_shell::WindowKey::new(parent.raw()))
                    .ok()
            })
        }
    }

    /// One keyboard enter/leave transition as an outbox entry.
    fn keyboard_transition(
        &self,
        surface: SurfaceId,
        enter: bool,
        keys: &[u32],
    ) -> Option<OutboxEntry> {
        let route = self.scene.routes.get(&surface)?;
        let objects = self.input.bindings.get(&route.client)?;
        let keyboard = objects.keyboard?;
        let object = route.surface_obj;
        let event = if enter {
            SeatEvent::KeyboardEnter {
                surface: object,
                keys: keys.to_vec(),
            }
        } else {
            SeatEvent::KeyboardLeave { surface: object }
        };
        Some(OutboxEntry::event(
            route.client,
            keyboard,
            event.name(),
            event.args(),
        ))
    }

    /// The routing view: mapped *roots* top-first (subsurfaces route
    /// through their root — the v1 contract), the client bindings,
    /// the keyboard focus, the desktop bounds.
    fn route_scene(
        &mut self,
    ) -> (
        Vec<SurfaceRef>,
        Vec<ClientBinding>,
        Option<SurfaceKey>,
        (f32, f32),
    ) {
        // The snapshot's render order is back-to-front; the routing
        // order is its reverse (topmost first).
        let snap = self.scene.snapshot();
        // Phase 48 — the modal gate: a live *mapped* modal dialog
        // gates its parent's whole tree (the spec's clause). The
        // router routes roots only, so the gate is the parent root
        // plus the popups rooted under it — membership the scene
        // supplies, the router never guesses (its own doctrine).
        let gated: Vec<SurfaceId> = {
            let mut gated = Vec::new();
            for (parent, dialog) in self.dialogs.gating_pairs() {
                let dialog_mapped = snap.node(dialog).is_some_and(|n| n.mapped);
                if dialog_mapped {
                    gated.push(parent);
                    for (pc, po) in self.popups.children_of_ids(parent) {
                        if let Some(entry) = self.popups.entry(pc, po) {
                            gated.push(entry.surface);
                        }
                    }
                }
            }
            gated
        };
        let mut surfaces = Vec::new();
        for id in snap.render_order().iter().rev() {
            // Roots only: the input footprint of a tree is its root.
            if self.scene.tree.parent_of(*id).is_some() {
                continue;
            }
            let Some(node) = snap.node(*id) else { continue };
            if !node.mapped {
                continue;
            }
            // Phase 49: the states arm's hidden set — a minimized or
            // off-space window takes no input (the hit-test never
            // finds it, the focus never holds it).
            if self.scene.hidden.contains(id) {
                continue;
            }
            // The modal gate: a gated surface takes no input (the
            // hit-test never finds it, the focus never holds it).
            if gated.contains(id) {
                continue;
            }
            let Some(route) = self.scene.routes.get(id) else {
                continue;
            };
            let region = InputRegion {
                rects: node
                    .input_region
                    .iter()
                    .filter(|r| r.w > 0 && r.h > 0)
                    .map(|r| RectF::new(r.x as f32, r.y as f32, r.w as f32, r.h as f32))
                    .collect(),
            };
            surfaces.push(SurfaceRef {
                key: SurfaceKey::new(id.raw()),
                surface: route.surface_obj,
                origin: ldp_core::geometry::PointF::new(node.bounds.x as f32, node.bounds.y as f32),
                size: (node.bounds.w as f32, node.bounds.h as f32),
                region,
            });
        }
        // The bindings: one per client that owns a routed root.
        let mut bindings = Vec::new();
        let mut seen: Vec<u64> = Vec::new();
        for s in &surfaces {
            if seen.contains(&s.key.as_u64()) {
                continue;
            }
            seen.push(s.key.as_u64());
            let id = SurfaceId::from_raw(s.key.as_u64());
            let Some(route) = self.scene.routes.get(&id) else {
                continue;
            };
            let Some(objects) = self.input.bindings.get(&route.client) else {
                continue;
            };
            bindings.push(ClientBinding {
                client: route.client.as_u32(),
                seat: ObjectId::CONNECTION, // routing uses only the device objects
                surface: s.key,
                pointer: objects.pointer,
                keyboard: objects.keyboard,
                touch: objects.touch,
                tablet: objects.tablet,
                gestures: objects.gestures,
            });
        }
        // The keyboard focus as a routing key — a gated focus is no
        // focus at all (the modal dialog took the desktop: the parent
        // keeps rendering, its keys stop routing); a hidden focus is
        // equally nothing (Phase 49: a minimized or off-space window
        // never holds the keyboard).
        let focus = self
            .input
            .keyboard_focus
            .filter(|id| !gated.contains(id))
            .filter(|id| !self.scene.hidden.contains(id))
            .map(|id| SurfaceKey::new(id.raw()));
        // The desktop bounds: the union of the outputs' layouts (the
        // pointer clamps inside it); a dark world routes nothing.
        let mut bounds = (0.0f32, 0.0f32);
        for slot in &self.outputs {
            let mode = slot.output.mode();
            let (w, h) = (u32::from(mode.hdisplay), u32::from(mode.vdisplay));
            let (x, y) = slot.output.layout;
            bounds.0 = bounds.0.max((x + w as i32) as f32);
            bounds.1 = bounds.1.max((y + h as i32) as f32);
        }
        (surfaces, bindings, focus, bounds)
    }
}

/// One routed event to one outbox entry (the descriptor rides the
/// keymap events). The entry carries its §10.4 emission doctrine
/// (Phase 39): the pointer's absolute sample and the batch
/// terminators are position state — the freshest value replaces the
/// still-pending one in its slot, so a client draining at frame
/// cadence reads one motion per frame whatever the device's rate; the
/// delta streams deliver in full; every unclassified seat event is
/// discrete input — never dropped, never reordered, and each seals
/// the object's pending position state at push time (the barrier: a
/// click's context is the sample it rode with).
///
/// Phase 43 extends the doctrine to the contact-carrying streams
/// (see [`contact_stream_entry`]): touch and tablet position state
/// coalesces **per contact**, the shared terminators collapse once
/// per object, and the contact's discrete events seal that contact's
/// stream — never another contact's.
fn routed_entry(r: &ldp_seat::route::RoutedEvent) -> Option<OutboxEntry> {
    use ldp_seat::event::SeatEvent;
    let client = ClientId::new(r.client)?;
    let event = &r.event;
    let mut entry = match event {
        SeatEvent::PointerMotion { .. } => OutboxEntry::position_state(
            client,
            r.object,
            event.name(),
            event.args(),
            crate::outbox::POSITION_MOTION,
        ),
        // The batch terminators, shared per object: one frame ends the
        // batch for every contact — the pointer's own doctrine, one
        // slot, the pending one replaced.
        SeatEvent::PointerFrame | SeatEvent::TouchFrame | SeatEvent::TabletFrame => {
            OutboxEntry::position_state(
                client,
                r.object,
                event.name(),
                event.args(),
                crate::outbox::POSITION_FRAME,
            )
        }
        // The delta streams: every delta delivers, full fidelity (a
        // wheel distance is not a position to replace, it is an
        // increment to keep).
        SeatEvent::PointerRelativeMotion { .. } | SeatEvent::TabletWheel { .. } => {
            OutboxEntry::delta_input(client, r.object, event.name(), event.args())
        }
        // The contact-carrying family (touch, tablet) — or the
        // conservative discrete default for everything else.
        _ => contact_stream_entry(client, r.object, event).unwrap_or_else(|| {
            OutboxEntry::discrete_input(client, r.object, event.name(), event.args())
        }),
    };
    if let Some(fd) = r.fd.as_ref() {
        entry = entry.carrying_fd(fd.try_clone().ok()?);
    }
    Some(entry)
}

/// The contact-carrying streams (Phase 43, the roadmap's "per-contact
/// keys" line): touch and tablet position state coalesces per contact
/// — each finger of a two-finger scroll collapses to its own freshest
/// sample, never one finger's sample riding another's slot; the
/// tablet's per-tool axes are position state of their own kind (a
/// 240 Hz pen stream parking samples between display frames delivers
/// one motion, one pressure, one tilt: the freshest of each, in the
/// slots the first sample of each axis took). The contact's discrete
/// events — `touch.down`/`up`, the tablet's tool lifecycle and
/// buttons — seal *that contact's* stream so a down's context is the
/// sample it rode with; `touch.cancel` seals nothing (a new sequence
/// must begin with a `touch.down`, which carries its own seal — a
/// cancel barrier would swallow the next sequence's first sample into
/// a pre-cancel slot).
///
/// Phase 45, the axis-metadata extension (the roadmap's own named
/// line): the touch contact's geometry axes — `touch.shape` (the
/// major/minor ellipse) and `touch.orientation` (its angle) — are
/// position state of their own kinds, the pressure/tilt doctrine
/// verbatim: a 120 Hz panel flooding shape samples per contact
/// delivers the contact's freshest ellipse per display frame, never a
/// queue of stale ones. Their kind tags are disjoint from motion's,
/// so a flood of both delivers one motion *and* one shape — each its
/// own freshest.
///
/// `None` for events outside the family (the caller's conservative
/// default takes them).
fn contact_stream_entry(
    client: ClientId,
    object: ObjectId,
    event: &SeatEvent,
) -> Option<OutboxEntry> {
    use ldp_seat::event::SeatEvent;
    let entry = match event {
        // The touch stream, per contact.
        SeatEvent::TouchMotion { id, .. } => OutboxEntry::contact_position_state(
            client,
            object,
            event.name(),
            event.args(),
            crate::outbox::POSITION_MOTION,
            u64::from(*id),
        ),
        // The touch contact's geometry axes: per-contact position
        // state of their own kinds (Phase 45).
        SeatEvent::TouchShape { id, .. } => OutboxEntry::contact_position_state(
            client,
            object,
            event.name(),
            event.args(),
            crate::outbox::POSITION_SHAPE,
            u64::from(*id),
        ),
        SeatEvent::TouchOrientation { id, .. } => OutboxEntry::contact_position_state(
            client,
            object,
            event.name(),
            event.args(),
            crate::outbox::POSITION_ORIENTATION,
            u64::from(*id),
        ),
        SeatEvent::TouchDown { id, .. } | SeatEvent::TouchUp { id } => {
            OutboxEntry::contact_discrete_input(
                client,
                object,
                event.name(),
                event.args(),
                Some(u64::from(*id)),
            )
        }
        // The void: `touch.cancel` invalidates the whole current
        // sequence and seals nothing.
        SeatEvent::TouchCancel => {
            OutboxEntry::contact_discrete_input(client, object, event.name(), event.args(), None)
        }
        // The tablet stream, per tool: motion and each axis carry
        // their own kind, so each collapses to its own freshest.
        SeatEvent::TabletMotion { id, .. } => OutboxEntry::contact_position_state(
            client,
            object,
            event.name(),
            event.args(),
            crate::outbox::POSITION_MOTION,
            *id,
        ),
        SeatEvent::TabletPressure { id, .. } => OutboxEntry::contact_position_state(
            client,
            object,
            event.name(),
            event.args(),
            crate::outbox::POSITION_PRESSURE,
            *id,
        ),
        SeatEvent::TabletTilt { id, .. } => OutboxEntry::contact_position_state(
            client,
            object,
            event.name(),
            event.args(),
            crate::outbox::POSITION_TILT,
            *id,
        ),
        SeatEvent::TabletRotation { id, .. } => OutboxEntry::contact_position_state(
            client,
            object,
            event.name(),
            event.args(),
            crate::outbox::POSITION_ROTATION,
            *id,
        ),
        SeatEvent::TabletSlider { id, .. } => OutboxEntry::contact_position_state(
            client,
            object,
            event.name(),
            event.args(),
            crate::outbox::POSITION_SLIDER,
            *id,
        ),
        // Tool lifecycle and buttons: discrete, sealing the tool's
        // stream (proximity in/out and press are context barriers for
        // the samples they rode with).
        SeatEvent::TabletTool { id, .. }
        | SeatEvent::TabletToolDone { id }
        | SeatEvent::TabletDown { id, .. }
        | SeatEvent::TabletUp { id }
        | SeatEvent::TabletButton { id, .. } => OutboxEntry::contact_discrete_input(
            client,
            object,
            event.name(),
            event.args(),
            Some(*id),
        ),
        // The frames and the wheel live in the caller's arms; the
        // pointer and everything else are not this family's.
        _ => return None,
    };
    Some(entry)
}

/// The compiled real keymap (libxkbcommon present and a default
/// selection compiles): the v1 text, the router's xkb state, and the
/// state's memfd. `None` takes the fallback path.
fn real_keymap() -> Option<(Vec<u8>, ldp_input::xkb::State, std::os::fd::OwnedFd)> {
    use std::io::Write as _;
    let lib = ldp_input::xkb::sys::LibXkb::open().ok()?;
    let ctx = ldp_input::xkb::Context::new(std::sync::Arc::new(lib)).ok()?;
    let keymap = ctx
        .compile(&ldp_input::xkb::Rmlvo::default_selection())
        .ok()?;
    let state = keymap.state().ok()?;
    let text = keymap.to_v1_string();
    if text.is_empty() {
        return None; // a compiled-but-empty keymap is the fallback's job
    }
    let blob = format!("{text}\0").into_bytes();
    let fd = crate::sys::memfd("ldp-keymap").ok()?;
    let mut file = std::fs::File::from(fd);
    file.write_all(&blob).ok()?;
    file.set_len(blob.len() as u64).ok()?;
    Some((blob, state, into_fd(file)))
}

/// A `File` back to its owned descriptor.
fn into_fd(file: std::fs::File) -> std::os::fd::OwnedFd {
    std::os::fd::OwnedFd::from(file)
}
