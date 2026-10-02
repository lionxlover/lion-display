//! The router: normalized input events in, seat protocol events out.
//!
//! One [`Router`] per seat owns that seat's entire input state —
//! pointer position, button mask, acceleration filter, gesture
//! machine, keymap state, repeat model, touch and tablet grabs.
//! Feeding it a device frame ([`Router::feed_frame`]) produces the
//! wire-honest event batch: focus follows the input-region hit test,
//! grabs override it, keys decompose through xkb, touchpads feed
//! gestures, touchscreens route contacts, and every pointer/touch/
//! tablet batch terminates with the protocol's `frame` event.
//!
//! Units: pointer motion crosses the acceleration filter (raw +
//! accelerated pairs in `relative_motion`); wheel values are detents
//! (hi-res counts ÷ 120); finger-scroll millimeters become radians
//! through a nominal wheel radius (25 mm — the physical wheel a
//! touchpad emulates); touch and tablet normalized axes map across
//! the output bounds before hit testing.
//!
//! Timestamps arrive with each frame (the device clock, converted);
//! the router reads none itself.

#![forbid(unsafe_code)]

use std::os::fd::OwnedFd;

use ldp_core::geometry::PointF;
use ldp_core::ids::ObjectId;
use ldp_core::time::Mono;
use ldp_input::accel::{AccelProfile, PointerAccel};
use ldp_input::device::DeviceClass;
use ldp_input::gesture::{GestureConfig, GestureEvent, GestureMachine};
use ldp_input::normalizer::{Axis, InputEvent, TabletTool, TabletUpdate, TouchUpdate, Wheel};
use ldp_input::repeat::RepeatModel;
use ldp_input::xkb;

use crate::event::{
    Axis as WireAxis, AxisSource, KeymapFormat, PressState, SeatEvent, TabletToolType,
};
use crate::focus::{ClientBinding, FocusRouter, Scene, SurfaceKey};
use crate::grab::{GrabKind, GrabModel};

/// The nominal wheel radius a touchpad's finger scroll emulates, in
/// millimeters (arc = radius × angle).
pub const FINGER_WHEEL_RADIUS_MM: f32 = 25.0;

/// Router tuning (per seat).
#[derive(Clone, Debug)]
pub struct RouterConfig {
    /// The pointer acceleration curve.
    pub accel: AccelProfile,
    /// The gesture recognition thresholds.
    pub gestures: GestureConfig,
    /// The key repeat parameters.
    pub repeat: RepeatModel,
}

impl Default for RouterConfig {
    fn default() -> Self {
        RouterConfig {
            accel: AccelProfile::default_profile(),
            gestures: GestureConfig::default(),
            repeat: RepeatModel::default(),
        }
    }
}

/// One routed protocol event, ready for the outbox.
#[derive(Debug)]
pub struct RoutedEvent {
    /// The owning client.
    pub client: u32,
    /// The target interface object.
    pub object: ObjectId,
    /// The event.
    pub event: SeatEvent,
    /// A descriptor riding the message (the keymap).
    pub fd: Option<OwnedFd>,
}

/// Which client-side object a routed event addresses.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Obj {
    /// The client's `pointer` object.
    Pointer,
    /// The client's `keyboard` object.
    Keyboard,
    /// The client's `touch` object.
    Touch,
    /// The client's `tablet` object.
    Tablet,
    /// The client's `gestures` object.
    Gestures,
}

impl Obj {
    fn of(self, binding: &ClientBinding) -> Option<ObjectId> {
        match self {
            Obj::Pointer => binding.pointer,
            Obj::Keyboard => binding.keyboard,
            Obj::Touch => binding.touch,
            Obj::Tablet => binding.tablet,
            Obj::Gestures => binding.gestures,
        }
    }
}

/// The per-seat input router.
#[derive(Debug)]
pub struct Router {
    cfg: RouterConfig,
    accel: PointerAccel,
    gestures: GestureMachine,
    focus: FocusRouter,
    grabs: GrabModel,
    pos: PointF,
    keys_held: Vec<u32>,
    xkb: Option<xkb::State>,
    keymap_fd: Option<OwnedFd>,
    /// The id handed to the in-proximity tool.
    tool_id: u64,
    /// The in-proximity tool (None = out of proximity).
    tool: Option<TabletTool>,
    /// The surface that owns the current tablet contact.
    tablet_contact: Option<SurfaceKey>,
    /// The last surface a tablet event went to (proximity-out needs
    /// a target after the contact ends).
    tablet_last: Option<SurfaceKey>,
    /// The last single-finger touchpad contact (id + mm position):
    /// single-finger strokes move the pointer, millimeter deltas
    /// through the acceleration filter.
    tp_finger: Option<(i32, f32, f32)>,
}

impl Router {
    /// A router with the given configuration and no state.
    #[must_use]
    pub fn new(cfg: RouterConfig) -> Router {
        let gestures = GestureMachine::new(cfg.gestures);
        Router {
            accel: PointerAccel::new(cfg.accel),
            gestures,
            cfg,
            focus: FocusRouter::new(),
            grabs: GrabModel::new(),
            pos: PointF::new(0.0, 0.0),
            keys_held: Vec::new(),
            xkb: None,
            keymap_fd: None,
            tool_id: 0,
            tool: None,
            tablet_contact: None,
            tablet_last: None,
            tp_finger: None,
        }
    }

    /// The current pointer position (output coordinates).
    #[must_use]
    pub fn pointer_position(&self) -> PointF {
        self.pos
    }

    /// The current pointer focus.
    #[must_use]
    pub fn pointer_focus(&self) -> Option<SurfaceKey> {
        self.focus.pointer()
    }

    /// The active pointer grab.
    #[must_use]
    pub fn pointer_grab(&self) -> Option<(SurfaceKey, GrabKind)> {
        self.grabs.pointer().map(|g| (g.surface, g.kind))
    }

    /// The keys currently held (for `keyboard.enter`).
    #[must_use]
    pub fn keys_held(&self) -> &[u32] {
        &self.keys_held
    }

    /// The modifier snapshot, when a keymap is installed.
    #[must_use]
    pub fn modifiers(&self) -> Option<xkb::ModsSnapshot> {
        self.xkb.as_ref().map(xkb::State::mods)
    }

    /// Install the keymap state; emits `keymap` + `repeat_info` to
    /// every client with a keyboard object. The descriptor is cloned
    /// per client (read-only sealed memfd, shareable).
    #[must_use]
    pub fn set_keymap(
        &mut self,
        state: xkb::State,
        fd: OwnedFd,
        clients: &[ClientBinding],
    ) -> Vec<RoutedEvent> {
        self.xkb = Some(state);
        self.keymap_fd = Some(fd);
        let mut out = Vec::new();
        for c in clients {
            let Some(object) = c.keyboard else { continue };
            let fd = self.keymap_fd.as_ref().and_then(|f| f.try_clone().ok());
            out.push(RoutedEvent {
                client: c.client,
                object,
                event: SeatEvent::KeyboardKeymap {
                    format: KeymapFormat::XkbV1,
                },
                fd,
            });
            out.push(RoutedEvent {
                client: c.client,
                object,
                event: SeatEvent::KeyboardRepeatInfo {
                    rate: self.cfg.repeat.rate_hz,
                    delay: self.cfg.repeat.delay_ms,
                },
                fd: None,
            });
        }
        out
    }

    /// Feed one normalized device frame; returns the routed events.
    #[must_use]
    pub fn feed_frame(
        &mut self,
        source: DeviceClass,
        events: &[InputEvent],
        time: Mono,
        scene: &Scene<'_>,
    ) -> Vec<RoutedEvent> {
        let mut out = Vec::new();
        let mut pointer_batch = false;

        for ev in events {
            match ev {
                InputEvent::PointerMotion { dx, dy } => {
                    self.route_motion(*dx, *dy, time, scene, &mut out);
                    pointer_batch = true;
                }
                InputEvent::Button { button, pressed } => {
                    self.route_button(*button, *pressed, scene, &mut out);
                    pointer_batch = true;
                }
                InputEvent::Wheel(w) => {
                    self.route_wheel(w, scene, &mut out);
                    pointer_batch = true;
                }
                InputEvent::Key { keycode, pressed } => {
                    self.route_key(*keycode, *pressed, scene, &mut out);
                }
                InputEvent::Touch(t) => {
                    if source == DeviceClass::Touchpad {
                        self.route_gestures(t, scene, &mut out, &mut pointer_batch);
                    } else {
                        self.route_touch(t, scene, &mut out);
                    }
                }
                InputEvent::Tablet(t) => {
                    self.route_tablet(t, scene, &mut out);
                }
                InputEvent::Other { .. } => {}
            }
        }

        if pointer_batch {
            let target = self
                .grabs
                .pointer_target(crate::focus::hit_test(scene.surfaces, self.pos));
            if let Some(key) = target {
                Self::emit(Obj::Pointer, key, scene, SeatEvent::PointerFrame, &mut out);
            }
        }
        out
    }

    /// A surface went away: dismiss focus, grabs, and touch owners.
    pub fn surface_gone(&mut self, key: SurfaceKey) {
        self.focus.surface_gone(key);
        self.grabs.surface_gone(key);
        if self.tablet_contact == Some(key) {
            self.tablet_contact = None;
        }
        self.gestures.reset();
        self.accel.reset();
    }

    // -- pointer ----------------------------------------------------------

    fn route_motion(
        &mut self,
        dx: i32,
        dy: i32,
        time: Mono,
        scene: &Scene<'_>,
        out: &mut Vec<RoutedEvent>,
    ) {
        self.route_motion_f32(dx as f32, dy as f32, time, scene, out);
    }

    fn route_button(
        &mut self,
        button: u32,
        pressed: bool,
        scene: &Scene<'_>,
        out: &mut Vec<RoutedEvent>,
    ) {
        let hit = crate::focus::hit_test(scene.surfaces, self.pos);
        if pressed {
            let target = self.grabs.pointer_target(hit).or(hit);
            if let Some(t) = target {
                self.grabs.pointer_press(button, t);
            }
        } else {
            self.grabs.pointer_release(button);
        }
        let target = self.grabs.pointer_target(hit);
        if let Some(t) = target {
            Self::emit(
                Obj::Pointer,
                t,
                scene,
                SeatEvent::PointerButton {
                    button,
                    state: PressState::from_press(pressed),
                },
                out,
            );
        }
    }

    fn route_wheel(&mut self, w: &Wheel, scene: &Scene<'_>, out: &mut Vec<RoutedEvent>) {
        let target = self
            .grabs
            .pointer_target(crate::focus::hit_test(scene.surfaces, self.pos));
        let Some(t) = target else { return };
        let axis = match w.axis {
            Axis::Vertical => WireAxis::Vertical,
            Axis::Horizontal => WireAxis::Horizontal,
        };
        Self::emit(
            Obj::Pointer,
            t,
            scene,
            SeatEvent::PointerAxisSource {
                source: AxisSource::Wheel,
            },
            out,
        );
        Self::emit(
            Obj::Pointer,
            t,
            scene,
            SeatEvent::PointerAxis {
                axis,
                value: w.hi_res / 120.0,
            },
            out,
        );
        if w.discrete != 0 {
            Self::emit(
                Obj::Pointer,
                t,
                scene,
                SeatEvent::PointerAxisDiscrete {
                    axis,
                    discrete: w.discrete,
                },
                out,
            );
        }
    }

    // -- keyboard ---------------------------------------------------------

    fn route_key(
        &mut self,
        keycode: u32,
        pressed: bool,
        scene: &Scene<'_>,
        out: &mut Vec<RoutedEvent>,
    ) {
        let target = self.grabs.keyboard_target(scene.keyboard_focus);
        let Some(t) = target else { return };
        let mods_changed = match self.xkb.as_mut() {
            Some(st) => st.update_key(keycode, pressed) != 0,
            None => false,
        };
        if pressed {
            self.keys_held.retain(|k| *k != keycode);
            self.keys_held.push(keycode);
        } else {
            self.keys_held.retain(|k| *k != keycode);
        }
        Self::emit(
            Obj::Keyboard,
            t,
            scene,
            SeatEvent::KeyboardKey {
                keycode,
                state: PressState::from_press(pressed),
            },
            out,
        );
        if mods_changed {
            let mods = self.xkb.as_ref().map(xkb::State::mods).unwrap_or_default();
            Self::emit(
                Obj::Keyboard,
                t,
                scene,
                SeatEvent::KeyboardModifiers {
                    depressed: mods.depressed,
                    latched: mods.latched,
                    locked: mods.locked,
                    group: mods.group,
                },
                out,
            );
        }
    }

    // -- touch (touchscreens) ---------------------------------------------

    fn route_touch(&mut self, t: &TouchUpdate, scene: &Scene<'_>, out: &mut Vec<RoutedEvent>) {
        // Every owner that received an event this frame gets the
        // terminating `frame` event — captured before lifts remove
        // their ownership.
        let mut touched: Vec<SurfaceKey> = Vec::new();
        let note = |key: SurfaceKey, touched: &mut Vec<SurfaceKey>| {
            if !touched.contains(&key) {
                touched.push(key);
            }
        };
        if t.dropped {
            for owner in self.focus.touch_cancel_all() {
                Self::emit(Obj::Touch, owner, scene, SeatEvent::TouchCancel, out);
                note(owner, &mut touched);
            }
            for key in touched {
                Self::emit(Obj::Touch, key, scene, SeatEvent::TouchFrame, out);
            }
            return;
        }
        for p in &t.points {
            let pos = PointF::new(p.x * scene.bounds.0, p.y * scene.bounds.1);
            if let Some(owner) = self.focus.touch(p.id) {
                let local = Self::local_of(owner, pos, scene);
                Self::emit(
                    Obj::Touch,
                    owner,
                    scene,
                    SeatEvent::TouchMotion {
                        id: p.id as u32,
                        x: local.x,
                        y: local.y,
                    },
                    out,
                );
                if let (Some(major), Some(minor)) = (p.touch_major, p.touch_minor) {
                    Self::emit(
                        Obj::Touch,
                        owner,
                        scene,
                        SeatEvent::TouchShape {
                            id: p.id as u32,
                            major,
                            minor,
                        },
                        out,
                    );
                }
                if let Some(o) = p.orientation {
                    Self::emit(
                        Obj::Touch,
                        owner,
                        scene,
                        SeatEvent::TouchOrientation {
                            id: p.id as u32,
                            orientation: o,
                        },
                        out,
                    );
                }
                note(owner, &mut touched);
            } else if let Some(key) = crate::focus::hit_test(scene.surfaces, pos) {
                self.focus.touch_down(p.id, key);
                let local = Self::local_of(key, pos, scene);
                Self::emit(
                    Obj::Touch,
                    key,
                    scene,
                    SeatEvent::TouchDown {
                        id: p.id as u32,
                        surface: Self::object_of(key, scene),
                        x: local.x,
                        y: local.y,
                    },
                    out,
                );
                note(key, &mut touched);
            }
        }
        for id in &t.ended {
            if let Some(owner) = self.focus.touch_up(*id) {
                Self::emit(
                    Obj::Touch,
                    owner,
                    scene,
                    SeatEvent::TouchUp { id: *id as u32 },
                    out,
                );
                note(owner, &mut touched);
            }
        }
        for key in touched {
            Self::emit(Obj::Touch, key, scene, SeatEvent::TouchFrame, out);
        }
    }

    // -- gestures (touchpads) ----------------------------------------------

    fn route_gestures(
        &mut self,
        t: &TouchUpdate,
        scene: &Scene<'_>,
        out: &mut Vec<RoutedEvent>,
        pointer_batch: &mut bool,
    ) {
        self.route_touchpad_pointer(t, scene, out, pointer_batch);
        for g in self.gestures.feed(t) {
            match g {
                GestureEvent::SwipeBegin { id, fingers } => {
                    self.emit_gesture(scene, SeatEvent::GestureSwipeBegin { id, fingers }, out);
                }
                GestureEvent::SwipeUpdate { id, dx, dy } => {
                    self.emit_gesture(scene, SeatEvent::GestureSwipeUpdate { id, dx, dy }, out);
                }
                GestureEvent::SwipeEnd { id, cancelled } => {
                    self.emit_gesture(scene, SeatEvent::GestureSwipeEnd { id, cancelled }, out);
                }
                GestureEvent::PinchBegin { id, fingers } => {
                    self.emit_gesture(scene, SeatEvent::GesturePinchBegin { id, fingers }, out);
                }
                GestureEvent::PinchUpdate { id, scale, dx, dy } => {
                    self.emit_gesture(
                        scene,
                        SeatEvent::GesturePinchUpdate { id, scale, dx, dy },
                        out,
                    );
                }
                GestureEvent::PinchEnd { id, cancelled } => {
                    self.emit_gesture(scene, SeatEvent::GesturePinchEnd { id, cancelled }, out);
                }
                GestureEvent::HoldBegin { id, fingers } => {
                    self.emit_gesture(scene, SeatEvent::GestureHoldBegin { id, fingers }, out);
                }
                GestureEvent::HoldEnd { id, cancelled } => {
                    self.emit_gesture(scene, SeatEvent::GestureHoldEnd { id, cancelled }, out);
                }
                GestureEvent::ScrollBegin { .. } => {
                    self.emit_to_focus(
                        Obj::Pointer,
                        scene,
                        SeatEvent::PointerAxisSource {
                            source: AxisSource::Finger,
                        },
                        out,
                    );
                    *pointer_batch = true;
                }
                GestureEvent::ScrollUpdate { id: _, dx, dy } => {
                    // Millimeters → radians through the nominal wheel.
                    if dx != 0.0 {
                        self.emit_to_focus(
                            Obj::Pointer,
                            scene,
                            SeatEvent::PointerAxis {
                                axis: WireAxis::Horizontal,
                                value: -dx / FINGER_WHEEL_RADIUS_MM,
                            },
                            out,
                        );
                    }
                    if dy != 0.0 {
                        self.emit_to_focus(
                            Obj::Pointer,
                            scene,
                            SeatEvent::PointerAxis {
                                axis: WireAxis::Vertical,
                                value: -dy / FINGER_WHEEL_RADIUS_MM,
                            },
                            out,
                        );
                    }
                    *pointer_batch = true;
                }
                GestureEvent::ScrollEnd { .. } => {
                    self.emit_to_focus(
                        Obj::Pointer,
                        scene,
                        SeatEvent::PointerAxisStop {
                            axis: WireAxis::Vertical,
                        },
                        out,
                    );
                    self.emit_to_focus(
                        Obj::Pointer,
                        scene,
                        SeatEvent::PointerAxisStop {
                            axis: WireAxis::Horizontal,
                        },
                        out,
                    );
                    *pointer_batch = true;
                }
            }
        }
    }

    // -- tablet -------------------------------------------------------------

    /// Proximity transitions in and out; returns the frame target.
    fn route_tool_transition(
        &mut self,
        t: &TabletUpdate,
        rest_hit: Option<SurfaceKey>,
        scene: &Scene<'_>,
        out: &mut Vec<RoutedEvent>,
    ) -> Option<SurfaceKey> {
        let mut frame_target: Option<SurfaceKey> = None;
        if self.tool.is_some() {
            // Proximity out: the tool-done pair goes to the last
            // target (the contact may already have ended).
            let id = self.tool_id;
            self.tool = None;
            self.tablet_contact = None;
            if let Some(key) = self.tablet_last {
                Self::emit(
                    Obj::Tablet,
                    key,
                    scene,
                    SeatEvent::TabletToolDone { id },
                    out,
                );
                Self::emit(Obj::Tablet, key, scene, SeatEvent::TabletFrame, out);
            }
        }
        if let Some(tool) = t.tool {
            // Proximity in: a fresh tool id, announced at the resting
            // position's surface.
            self.tool_id += 1;
            self.tool = Some(tool);
            if let Some(key) = self.tablet_contact.or(rest_hit) {
                Self::emit(
                    Obj::Tablet,
                    key,
                    scene,
                    SeatEvent::TabletTool {
                        id: self.tool_id,
                        tool_type: wire_tool(tool),
                    },
                    out,
                );
                frame_target = Some(key);
                self.tablet_last = Some(key);
            }
        }
        frame_target
    }

    /// Axis frames (position, contact, pressure, tilt); returns the
    /// frame target.
    fn route_tablet_axes(
        &mut self,
        t: &TabletUpdate,
        hit: Option<SurfaceKey>,
        pos: PointF,
        scene: &Scene<'_>,
        out: &mut Vec<RoutedEvent>,
    ) -> Option<SurfaceKey> {
        let id = self.tool_id;
        let mut frame_target: Option<SurfaceKey> = None;
        if t.contact && self.tablet_contact.is_none() {
            self.tablet_contact = hit;
            if let Some(key) = hit {
                let local = Self::local_of(key, pos, scene);
                Self::emit(
                    Obj::Tablet,
                    key,
                    scene,
                    SeatEvent::TabletDown {
                        id,
                        surface: Self::object_of(key, scene),
                        x: local.x,
                        y: local.y,
                    },
                    out,
                );
            }
        } else if !t.contact {
            if let Some(key) = self.tablet_contact.take() {
                Self::emit(Obj::Tablet, key, scene, SeatEvent::TabletUp { id }, out);
            }
        }
        if let Some(key) = self.tablet_contact.or(hit) {
            let local = Self::local_of(key, pos, scene);
            Self::emit(
                Obj::Tablet,
                key,
                scene,
                SeatEvent::TabletMotion {
                    id,
                    x: local.x,
                    y: local.y,
                },
                out,
            );
            if let Some(p) = t.pressure {
                Self::emit(
                    Obj::Tablet,
                    key,
                    scene,
                    SeatEvent::TabletPressure { id, value: p },
                    out,
                );
            }
            if let (Some(tx), Some(ty)) = (t.tilt_x, t.tilt_y) {
                Self::emit(
                    Obj::Tablet,
                    key,
                    scene,
                    SeatEvent::TabletTilt { id, x: tx, y: ty },
                    out,
                );
            }
            frame_target = Some(key);
        }
        frame_target
    }

    fn route_tablet(&mut self, t: &TabletUpdate, scene: &Scene<'_>, out: &mut Vec<RoutedEvent>) {
        let rest_pos = PointF::new(t.x * scene.bounds.0, t.y * scene.bounds.1);
        let rest_hit = crate::focus::hit_test(scene.surfaces, rest_pos);
        // The frame terminator follows the last target of this batch.
        let mut frame_target: Option<SurfaceKey> = None;
        if t.tool_changed {
            frame_target = self.route_tool_transition(t, rest_hit, scene, out);
        }
        if t.tool.is_none() || !t.axes_present {
            // Out of proximity, or a pure proximity transition: no
            // axis events to report — but the tool transition above
            // still needs its terminator.
            if let Some(key) = frame_target {
                Self::emit(Obj::Tablet, key, scene, SeatEvent::TabletFrame, out);
            }
            return;
        }
        let pos = PointF::new(t.x * scene.bounds.0, t.y * scene.bounds.1);
        let hit = crate::focus::hit_test(scene.surfaces, pos);
        frame_target = self.route_tablet_axes(t, hit, pos, scene, out);
        if let Some(key) = frame_target {
            self.tablet_last = Some(key);
            Self::emit(Obj::Tablet, key, scene, SeatEvent::TabletFrame, out);
        }
    }

    /// Single-finger touchpad strokes move the pointer: millimeter
    /// deltas through the acceleration filter (the 1:1 mm-to-logical
    /// mapping is the default touchpad speed; calibration is shell
    /// policy). Tap-to-click is deliberately absent — physical
    /// touchpad buttons arrive as `Button` events, and the tap policy
    /// belongs to the shell phase.
    fn route_touchpad_pointer(
        &mut self,
        t: &TouchUpdate,
        scene: &Scene<'_>,
        out: &mut Vec<RoutedEvent>,
        pointer_batch: &mut bool,
    ) {
        if t.points.len() == 1 {
            let p = &t.points[0];
            match self.tp_finger {
                Some((id, x, y)) if id == p.id => {
                    let (dx, dy) = (p.x_mm - x, p.y_mm - y);
                    if dx != 0.0 || dy != 0.0 {
                        self.route_motion_f32(dx, dy, t.time, scene, out);
                        *pointer_batch = true;
                    }
                    self.tp_finger = Some((p.id, p.x_mm, p.y_mm));
                }
                _ => {
                    // A new stroke begins; no delta on the first frame.
                    self.tp_finger = Some((p.id, p.x_mm, p.y_mm));
                }
            }
        } else {
            // Stroke ended (lift or multi-finger).
            self.tp_finger = None;
        }
    }

    /// The float-delta motion path (touchpad millimeters).
    fn route_motion_f32(
        &mut self,
        dx: f32,
        dy: f32,
        time: Mono,
        scene: &Scene<'_>,
        out: &mut Vec<RoutedEvent>,
    ) {
        let m = self.accel.filter_f32(dx, dy, time);
        self.pos = PointF::new(
            (self.pos.x + m.dx_accel).clamp(0.0, scene.bounds.0),
            (self.pos.y + m.dy_accel).clamp(0.0, scene.bounds.1),
        );
        let hit = crate::focus::hit_test(scene.surfaces, self.pos);
        let target = self.grabs.pointer_target(hit);
        if target != self.focus.pointer() {
            if let Some(old) = self.focus.set_pointer(target) {
                Self::emit(
                    Obj::Pointer,
                    old,
                    scene,
                    SeatEvent::PointerLeave {
                        surface: Self::object_of(old, scene),
                    },
                    out,
                );
            }
            if let Some(new) = target {
                let local = Self::local_of(new, self.pos, scene);
                Self::emit(
                    Obj::Pointer,
                    new,
                    scene,
                    SeatEvent::PointerEnter {
                        surface: Self::object_of(new, scene),
                        x: local.x,
                        y: local.y,
                    },
                    out,
                );
            }
        } else if let Some(t) = target {
            let local = Self::local_of(t, self.pos, scene);
            Self::emit(
                Obj::Pointer,
                t,
                scene,
                SeatEvent::PointerMotion {
                    x: local.x,
                    y: local.y,
                },
                out,
            );
        }
        if let Some(t) = self.focus.pointer() {
            Self::emit(
                Obj::Pointer,
                t,
                scene,
                SeatEvent::PointerRelativeMotion {
                    dx_unaccel: m.dx_unaccel,
                    dy_unaccel: m.dy_unaccel,
                    dx_accel: m.dx_accel,
                    dy_accel: m.dy_accel,
                },
                out,
            );
        }
    }

    // -- emit helpers --------------------------------------------------------

    fn object_of(key: SurfaceKey, scene: &Scene<'_>) -> ObjectId {
        scene
            .surface(key)
            .map_or(ObjectId::from_wire(0), |s| s.surface)
    }

    fn local_of(key: SurfaceKey, pos: PointF, scene: &Scene<'_>) -> PointF {
        scene.surface(key).map_or(pos, |s| s.local(pos))
    }

    fn emit(
        obj: Obj,
        key: SurfaceKey,
        scene: &Scene<'_>,
        event: SeatEvent,
        out: &mut Vec<RoutedEvent>,
    ) {
        let Some(binding) = scene.binding_for(key) else {
            return;
        };
        if let Some(object) = obj.of(binding) {
            out.push(RoutedEvent {
                client: binding.client,
                object,
                event,
                fd: None,
            });
        }
    }

    fn emit_to_focus(
        &self,
        obj: Obj,
        scene: &Scene<'_>,
        event: SeatEvent,
        out: &mut Vec<RoutedEvent>,
    ) {
        if let Some(key) = self.focus.pointer() {
            Self::emit(obj, key, scene, event, out);
        }
    }

    fn emit_gesture(&self, scene: &Scene<'_>, event: SeatEvent, out: &mut Vec<RoutedEvent>) {
        if let Some(key) = self.focus.pointer() {
            Self::emit(Obj::Gestures, key, scene, event, out);
        }
    }
}

/// The wire tablet tool type for a normalized tool.
fn wire_tool(tool: TabletTool) -> TabletToolType {
    match tool {
        TabletTool::Pen => TabletToolType::Pen,
        TabletTool::Eraser => TabletToolType::Eraser,
        TabletTool::Brush => TabletToolType::Brush,
        TabletTool::Pencil => TabletToolType::Pencil,
        TabletTool::Airbrush => TabletToolType::Airbrush,
        TabletTool::Mouse => TabletToolType::Mouse,
        TabletTool::Lens => TabletToolType::Lens,
        TabletTool::Finger => TabletToolType::Finger,
    }
}
