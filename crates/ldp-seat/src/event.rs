//! The typed seat-event vocabulary and its wire encoding.
//!
//! [`SeatEvent`] mirrors `spec/input.toml` event-for-event: one Rust
//! variant per wire event, the same argument order, the same value
//! domains. Encoding goes through the compiled schema registry
//! ([`ldp_protocol::REGISTRY`]): the variant names its interface and
//! event, the registry supplies the opcode, and [`SeatEvent::args`]
//! builds the wire values — so a typo in either place fails loudly
//! at encode time instead of drifting silently.
//!
//! The [`wire`](self) sub-enums carry the protocol's shared value
//! domains (button/key state, axes, axis sources, tablet tools) with
//! their exact wire numbers, pinned by tests against the spec tables.

#![forbid(unsafe_code)]

use ldp_core::ids::ObjectId;
use ldp_core::wire::Value;
use ldp_protocol::{Direction, Message, REGISTRY};

/// Physical button/key state (`button_state`, `key_state`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PressState {
    /// Released (wire 1).
    Released,
    /// Pressed (wire 2).
    Pressed,
}

impl PressState {
    /// The wire value.
    #[must_use]
    pub const fn wire(self) -> u32 {
        match self {
            PressState::Released => 1,
            PressState::Pressed => 2,
        }
    }

    /// From a boolean press state.
    #[must_use]
    pub const fn from_press(pressed: bool) -> PressState {
        if pressed {
            PressState::Pressed
        } else {
            PressState::Released
        }
    }
}

/// Scroll axis (`axis`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Axis {
    /// Vertical (wire 1).
    Vertical,
    /// Horizontal (wire 2).
    Horizontal,
}

impl Axis {
    /// The wire value.
    #[must_use]
    pub const fn wire(self) -> u32 {
        match self {
            Axis::Vertical => 1,
            Axis::Horizontal => 2,
        }
    }
}

/// What generated a scroll sequence (`axis_source`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AxisSource {
    /// A wheel (wire 1).
    Wheel,
    /// A finger on a touchpad (wire 2).
    Finger,
    /// Continuous scrolling (wire 3).
    Continuous,
    /// A tilting wheel (wire 4).
    WheelTilt,
}

impl AxisSource {
    /// The wire value.
    #[must_use]
    pub const fn wire(self) -> u32 {
        match self {
            AxisSource::Wheel => 1,
            AxisSource::Finger => 2,
            AxisSource::Continuous => 3,
            AxisSource::WheelTilt => 4,
        }
    }
}

/// Keymap serialization format (`keymap_format`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KeymapFormat {
    /// xkbcommon v1 text (wire 1).
    XkbV1,
}

impl KeymapFormat {
    /// The wire value.
    #[must_use]
    pub const fn wire(self) -> u32 {
        match self {
            KeymapFormat::XkbV1 => 1,
        }
    }
}

/// The proximity tool of a tablet (`tablet_tool_type`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TabletToolType {
    /// Pen (wire 1).
    Pen,
    /// Eraser (wire 2).
    Eraser,
    /// Brush (wire 3).
    Brush,
    /// Pencil (wire 4).
    Pencil,
    /// Airbrush (wire 5).
    Airbrush,
    /// Mouse puck (wire 6).
    Mouse,
    /// Lens puck (wire 7).
    Lens,
    /// Finger (wire 8).
    Finger,
}

impl TabletToolType {
    /// The wire value.
    #[must_use]
    pub const fn wire(self) -> u32 {
        match self {
            TabletToolType::Pen => 1,
            TabletToolType::Eraser => 2,
            TabletToolType::Brush => 3,
            TabletToolType::Pencil => 4,
            TabletToolType::Airbrush => 5,
            TabletToolType::Mouse => 6,
            TabletToolType::Lens => 7,
            TabletToolType::Finger => 8,
        }
    }
}

/// One typed seat event, routing-ready.
#[derive(Clone, PartialEq, Debug)]
pub enum SeatEvent {
    /// `seat.capabilities`: the seat's current capability set.
    SeatCapabilities {
        /// Capability bitset (see `seat_caps` bits).
        caps: u32,
    },
    /// `seat.name`.
    SeatName {
        /// The seat's human-readable name.
        name: Box<str>,
    },
    /// `pointer.enter`.
    PointerEnter {
        /// The entered surface.
        surface: ObjectId,
        /// X in surface coordinates.
        x: f32,
        /// Y in surface coordinates.
        y: f32,
    },
    /// `pointer.leave`.
    PointerLeave {
        /// The left surface.
        surface: ObjectId,
    },
    /// `pointer.motion`.
    PointerMotion {
        /// X in surface coordinates.
        x: f32,
        /// Y in surface coordinates.
        y: f32,
    },
    /// `pointer.relative_motion`.
    PointerRelativeMotion {
        /// Raw X delta.
        dx_unaccel: f32,
        /// Raw Y delta.
        dy_unaccel: f32,
        /// Accelerated X delta.
        dx_accel: f32,
        /// Accelerated Y delta.
        dy_accel: f32,
    },
    /// `pointer.button`.
    PointerButton {
        /// Button code (Linux input code).
        button: u32,
        /// New state.
        state: PressState,
    },
    /// `pointer.axis`.
    PointerAxis {
        /// Which axis.
        axis: Axis,
        /// Scroll amount in the source's unit.
        value: f32,
    },
    /// `pointer.axis_discrete`.
    PointerAxisDiscrete {
        /// Which axis.
        axis: Axis,
        /// Discrete steps.
        discrete: i32,
    },
    /// `pointer.axis_stop`.
    PointerAxisStop {
        /// Which axis.
        axis: Axis,
    },
    /// `pointer.axis_source`.
    PointerAxisSource {
        /// The sequence's source.
        source: AxisSource,
    },
    /// `pointer.frame`.
    PointerFrame,
    /// `keyboard.keymap` (descriptor rides the message).
    KeyboardKeymap {
        /// The serialization format.
        format: KeymapFormat,
    },
    /// `keyboard.enter`.
    KeyboardEnter {
        /// The focused surface.
        surface: ObjectId,
        /// Currently pressed keycodes.
        keys: Vec<u32>,
    },
    /// `keyboard.leave`.
    KeyboardLeave {
        /// The left surface.
        surface: ObjectId,
    },
    /// `keyboard.key`.
    KeyboardKey {
        /// Linux evdev keycode.
        keycode: u32,
        /// New state.
        state: PressState,
    },
    /// `keyboard.modifiers`.
    KeyboardModifiers {
        /// Depressed modifier mask.
        depressed: u32,
        /// Latched modifier mask.
        latched: u32,
        /// Locked modifier mask.
        locked: u32,
        /// Effective layout group.
        group: u32,
    },
    /// `keyboard.repeat_info`.
    KeyboardRepeatInfo {
        /// Repeats per second.
        rate: i32,
        /// Hold delay, milliseconds.
        delay: i32,
    },
    /// `touch.down`.
    TouchDown {
        /// Touch point id.
        id: u32,
        /// The surface that owns the point.
        surface: ObjectId,
        /// X in surface coordinates.
        x: f32,
        /// Y in surface coordinates.
        y: f32,
    },
    /// `touch.up`.
    TouchUp {
        /// Touch point id.
        id: u32,
    },
    /// `touch.motion`.
    TouchMotion {
        /// Touch point id.
        id: u32,
        /// X in surface coordinates.
        x: f32,
        /// Y in surface coordinates.
        y: f32,
    },
    /// `touch.frame`.
    TouchFrame,
    /// `touch.cancel`.
    TouchCancel,
    /// `touch.shape`.
    TouchShape {
        /// Touch point id.
        id: u32,
        /// Contact ellipse major axis (mm).
        major: f32,
        /// Contact ellipse minor axis (mm).
        minor: f32,
    },
    /// `touch.orientation`.
    TouchOrientation {
        /// Touch point id.
        id: u32,
        /// Contact ellipse orientation, radians.
        orientation: f32,
    },
    /// `tablet.tool` (proximity).
    TabletTool {
        /// Tool instance id.
        id: u64,
        /// The tool type.
        tool_type: TabletToolType,
    },
    /// `tablet.tool_done`.
    TabletToolDone {
        /// Tool instance id.
        id: u64,
    },
    /// `tablet.down`.
    TabletDown {
        /// Tool instance id.
        id: u64,
        /// The contact surface.
        surface: ObjectId,
        /// X in surface coordinates.
        x: f32,
        /// Y in surface coordinates.
        y: f32,
    },
    /// `tablet.up`.
    TabletUp {
        /// Tool instance id.
        id: u64,
    },
    /// `tablet.motion`.
    TabletMotion {
        /// Tool instance id.
        id: u64,
        /// X in surface coordinates.
        x: f32,
        /// Y in surface coordinates.
        y: f32,
    },
    /// `tablet.pressure`.
    TabletPressure {
        /// Tool instance id.
        id: u64,
        /// Normalized pressure.
        value: f32,
    },
    /// `tablet.tilt`.
    TabletTilt {
        /// Tool instance id.
        id: u64,
        /// Tilt X, radians.
        x: f32,
        /// Tilt Y, radians.
        y: f32,
    },
    /// `tablet.rotation`.
    TabletRotation {
        /// Tool instance id.
        id: u64,
        /// Rotation, radians.
        value: f32,
    },
    /// `tablet.slider`.
    TabletSlider {
        /// Tool instance id.
        id: u64,
        /// Normalized slider position.
        value: f32,
    },
    /// `tablet.wheel`.
    TabletWheel {
        /// Tool instance id.
        id: u64,
        /// Wheel distance, discrete steps.
        distance: f32,
    },
    /// `tablet.button`.
    TabletButton {
        /// Tool instance id.
        id: u64,
        /// Button code.
        button: u32,
        /// New state.
        state: PressState,
    },
    /// `tablet.frame`.
    TabletFrame,
    /// `gestures.swipe_begin`.
    GestureSwipeBegin {
        /// Gesture instance id.
        id: u32,
        /// Finger count.
        fingers: u32,
    },
    /// `gestures.swipe_update`.
    GestureSwipeUpdate {
        /// Gesture instance id.
        id: u32,
        /// Centroid X delta.
        dx: f32,
        /// Centroid Y delta.
        dy: f32,
    },
    /// `gestures.swipe_end`.
    GestureSwipeEnd {
        /// Gesture instance id.
        id: u32,
        /// Interrupted, not completed.
        cancelled: bool,
    },
    /// `gestures.pinch_begin`.
    GesturePinchBegin {
        /// Gesture instance id.
        id: u32,
        /// Finger count.
        fingers: u32,
    },
    /// `gestures.pinch_update`.
    GesturePinchUpdate {
        /// Gesture instance id.
        id: u32,
        /// Incremental scale factor.
        scale: f32,
        /// Centroid X delta.
        dx: f32,
        /// Centroid Y delta.
        dy: f32,
    },
    /// `gestures.pinch_end`.
    GesturePinchEnd {
        /// Gesture instance id.
        id: u32,
        /// Interrupted, not completed.
        cancelled: bool,
    },
    /// `gestures.hold_begin`.
    GestureHoldBegin {
        /// Gesture instance id.
        id: u32,
        /// Finger count.
        fingers: u32,
    },
    /// `gestures.hold_end`.
    GestureHoldEnd {
        /// Gesture instance id.
        id: u32,
        /// Interrupted, not completed.
        cancelled: bool,
    },
}

impl SeatEvent {
    /// The fully qualified interface this event belongs to.
    #[must_use]
    #[allow(clippy::match_same_arms)] // one interface per family: same arms are the point
    pub const fn interface(&self) -> &'static str {
        match self {
            SeatEvent::SeatCapabilities { .. } | SeatEvent::SeatName { .. } => "ldp.input.seat",
            SeatEvent::PointerEnter { .. }
            | SeatEvent::PointerLeave { .. }
            | SeatEvent::PointerMotion { .. }
            | SeatEvent::PointerRelativeMotion { .. }
            | SeatEvent::PointerButton { .. }
            | SeatEvent::PointerAxis { .. }
            | SeatEvent::PointerAxisDiscrete { .. }
            | SeatEvent::PointerAxisStop { .. }
            | SeatEvent::PointerAxisSource { .. }
            | SeatEvent::PointerFrame => "ldp.input.pointer",
            SeatEvent::KeyboardKeymap { .. }
            | SeatEvent::KeyboardEnter { .. }
            | SeatEvent::KeyboardLeave { .. }
            | SeatEvent::KeyboardKey { .. }
            | SeatEvent::KeyboardModifiers { .. }
            | SeatEvent::KeyboardRepeatInfo { .. } => "ldp.input.keyboard",
            SeatEvent::TouchDown { .. }
            | SeatEvent::TouchUp { .. }
            | SeatEvent::TouchMotion { .. }
            | SeatEvent::TouchFrame
            | SeatEvent::TouchCancel
            | SeatEvent::TouchShape { .. }
            | SeatEvent::TouchOrientation { .. } => "ldp.input.touch",
            SeatEvent::TabletTool { .. }
            | SeatEvent::TabletToolDone { .. }
            | SeatEvent::TabletDown { .. }
            | SeatEvent::TabletUp { .. }
            | SeatEvent::TabletMotion { .. }
            | SeatEvent::TabletPressure { .. }
            | SeatEvent::TabletTilt { .. }
            | SeatEvent::TabletRotation { .. }
            | SeatEvent::TabletSlider { .. }
            | SeatEvent::TabletWheel { .. }
            | SeatEvent::TabletButton { .. }
            | SeatEvent::TabletFrame => "ldp.input.tablet",
            SeatEvent::GestureSwipeBegin { .. }
            | SeatEvent::GestureSwipeUpdate { .. }
            | SeatEvent::GestureSwipeEnd { .. }
            | SeatEvent::GesturePinchBegin { .. }
            | SeatEvent::GesturePinchUpdate { .. }
            | SeatEvent::GesturePinchEnd { .. }
            | SeatEvent::GestureHoldBegin { .. }
            | SeatEvent::GestureHoldEnd { .. } => "ldp.input.gestures",
        }
    }

    /// The event's name within its interface (the spec's spelling).
    #[must_use]
    #[allow(clippy::match_same_arms)] // shared names across families (enter, frame) are real
    pub const fn name(&self) -> &'static str {
        match self {
            SeatEvent::SeatCapabilities { .. } => "capabilities",
            SeatEvent::SeatName { .. } => "name",
            SeatEvent::PointerEnter { .. } => "enter",
            SeatEvent::PointerLeave { .. } => "leave",
            SeatEvent::PointerMotion { .. } => "motion",
            SeatEvent::PointerRelativeMotion { .. } => "relative_motion",
            SeatEvent::PointerButton { .. } => "button",
            SeatEvent::PointerAxis { .. } => "axis",
            SeatEvent::PointerAxisDiscrete { .. } => "axis_discrete",
            SeatEvent::PointerAxisStop { .. } => "axis_stop",
            SeatEvent::PointerAxisSource { .. } => "axis_source",
            SeatEvent::PointerFrame => "frame",
            SeatEvent::KeyboardKeymap { .. } => "keymap",
            SeatEvent::KeyboardEnter { .. } => "enter",
            SeatEvent::KeyboardLeave { .. } => "leave",
            SeatEvent::KeyboardKey { .. } => "key",
            SeatEvent::KeyboardModifiers { .. } => "modifiers",
            SeatEvent::KeyboardRepeatInfo { .. } => "repeat_info",
            SeatEvent::TouchDown { .. } => "down",
            SeatEvent::TouchUp { .. } => "up",
            SeatEvent::TouchMotion { .. } => "motion",
            SeatEvent::TouchFrame => "frame",
            SeatEvent::TouchCancel => "cancel",
            SeatEvent::TouchShape { .. } => "shape",
            SeatEvent::TouchOrientation { .. } => "orientation",
            SeatEvent::TabletTool { .. } => "tool",
            SeatEvent::TabletToolDone { .. } => "tool_done",
            SeatEvent::TabletDown { .. } => "down",
            SeatEvent::TabletUp { .. } => "up",
            SeatEvent::TabletMotion { .. } => "motion",
            SeatEvent::TabletPressure { .. } => "pressure",
            SeatEvent::TabletTilt { .. } => "tilt",
            SeatEvent::TabletRotation { .. } => "rotation",
            SeatEvent::TabletSlider { .. } => "slider",
            SeatEvent::TabletWheel { .. } => "wheel",
            SeatEvent::TabletButton { .. } => "button",
            SeatEvent::TabletFrame => "frame",
            SeatEvent::GestureSwipeBegin { .. } => "swipe_begin",
            SeatEvent::GestureSwipeUpdate { .. } => "swipe_update",
            SeatEvent::GestureSwipeEnd { .. } => "swipe_end",
            SeatEvent::GesturePinchBegin { .. } => "pinch_begin",
            SeatEvent::GesturePinchUpdate { .. } => "pinch_update",
            SeatEvent::GesturePinchEnd { .. } => "pinch_end",
            SeatEvent::GestureHoldBegin { .. } => "hold_begin",
            SeatEvent::GestureHoldEnd { .. } => "hold_end",
        }
    }

    /// The wire arguments, in declaration order.
    ///
    /// # Panics
    /// Never in practice: the one `expect` guards a u32-array
    /// construction that cannot mismatch.
    #[must_use]
    #[allow(clippy::match_same_arms)] // zero-arg events across families are distinct events
    pub fn args(&self) -> Vec<Value> {
        use SeatEvent as E;
        let f = Value::Float32;
        let u = Value::Uint32;
        let i = Value::Int32;
        let surface = |o: ObjectId| Value::Object(Some(o));
        match self {
            E::SeatCapabilities { caps } => vec![u(*caps)],
            E::SeatName { name } => vec![Value::String(name.clone())],
            E::PointerEnter { surface: s, x, y } => vec![surface(*s), f(*x), f(*y)],
            E::PointerLeave { surface: s } => vec![surface(*s)],
            E::PointerMotion { x, y } => vec![f(*x), f(*y)],
            E::PointerRelativeMotion {
                dx_unaccel,
                dy_unaccel,
                dx_accel,
                dy_accel,
            } => vec![f(*dx_unaccel), f(*dy_unaccel), f(*dx_accel), f(*dy_accel)],
            E::PointerButton { button, state } => vec![u(*button), Value::Enum(state.wire())],
            E::PointerAxis { axis, value } => vec![Value::Enum(axis.wire()), f(*value)],
            E::PointerAxisDiscrete { axis, discrete } => {
                vec![Value::Enum(axis.wire()), i(*discrete)]
            }
            E::PointerAxisStop { axis } => vec![Value::Enum(axis.wire())],
            E::PointerAxisSource { source } => vec![Value::Enum(source.wire())],
            E::PointerFrame => vec![],
            E::KeyboardKeymap { format } => {
                // The descriptor index is 0 of this message's table;
                // the FD itself rides the message.
                vec![Value::Fd(0), Value::Enum(format.wire())]
            }
            E::KeyboardEnter { surface: s, keys } => vec![
                surface(*s),
                Value::array(
                    ldp_core::wire::ArgType::Uint32,
                    keys.iter()
                        .map(|k| ldp_core::wire::Primitive::Uint32(*k))
                        .collect::<Vec<ldp_core::wire::Primitive>>(),
                )
                .expect("primitive array"),
            ],
            E::KeyboardLeave { surface: s } => vec![surface(*s)],
            E::KeyboardKey { keycode, state } => vec![u(*keycode), Value::Enum(state.wire())],
            E::KeyboardModifiers {
                depressed,
                latched,
                locked,
                group,
            } => vec![u(*depressed), u(*latched), u(*locked), u(*group)],
            E::KeyboardRepeatInfo { rate, delay } => vec![i(*rate), i(*delay)],
            E::TouchDown {
                id,
                surface: s,
                x,
                y,
            } => {
                vec![u(*id), surface(*s), f(*x), f(*y)]
            }
            E::TouchUp { id } => vec![u(*id)],
            E::TouchMotion { id, x, y } => vec![u(*id), f(*x), f(*y)],
            E::TouchFrame | E::TouchCancel => vec![],
            E::TouchShape { id, major, minor } => vec![u(*id), f(*major), f(*minor)],
            E::TouchOrientation { id, orientation } => vec![u(*id), f(*orientation)],
            E::TabletTool { id, tool_type } => {
                vec![Value::Uint64(*id), Value::Enum(tool_type.wire())]
            }
            E::TabletToolDone { id } => vec![Value::Uint64(*id)],
            E::TabletDown {
                id,
                surface: s,
                x,
                y,
            } => {
                vec![Value::Uint64(*id), surface(*s), f(*x), f(*y)]
            }
            E::TabletUp { id } => vec![Value::Uint64(*id)],
            E::TabletMotion { id, x, y } => vec![Value::Uint64(*id), f(*x), f(*y)],
            E::TabletPressure { id, value } => vec![Value::Uint64(*id), f(*value)],
            E::TabletTilt { id, x, y } => vec![Value::Uint64(*id), f(*x), f(*y)],
            E::TabletRotation { id, value } => vec![Value::Uint64(*id), f(*value)],
            E::TabletSlider { id, value } => vec![Value::Uint64(*id), f(*value)],
            E::TabletWheel { id, distance } => vec![Value::Uint64(*id), f(*distance)],
            E::TabletButton { id, button, state } => {
                vec![Value::Uint64(*id), u(*button), Value::Enum(state.wire())]
            }
            E::TabletFrame => vec![],
            E::GestureSwipeBegin { id, fingers } => vec![u(*id), u(*fingers)],
            E::GestureSwipeUpdate { id, dx, dy } => vec![u(*id), f(*dx), f(*dy)],
            E::GestureSwipeEnd { id, cancelled } => vec![u(*id), Value::Bool(*cancelled)],
            E::GesturePinchBegin { id, fingers } => vec![u(*id), u(*fingers)],
            E::GesturePinchUpdate { id, scale, dx, dy } => {
                vec![u(*id), f(*scale), f(*dx), f(*dy)]
            }
            E::GesturePinchEnd { id, cancelled } => vec![u(*id), Value::Bool(*cancelled)],
            E::GestureHoldBegin { id, fingers } => vec![u(*id), u(*fingers)],
            E::GestureHoldEnd { id, cancelled } => vec![u(*id), Value::Bool(*cancelled)],
        }
    }

    /// Encode against the compiled registry: a [`Message`] addressed
    /// to `object`, opcode resolved from the schema.
    ///
    /// # Panics
    /// When the event's `(interface, name)` pair does not exist in the
    /// compiled spec — a build-time consistency break, not a runtime
    /// condition.
    #[must_use]
    #[allow(clippy::too_many_lines)] // a flat dispatch over the event vocabulary
    pub fn to_message(&self, object: ObjectId) -> Message {
        let iface = REGISTRY
            .interface(self.interface())
            .unwrap_or_else(|| panic!("interface {} missing from schema", self.interface()));
        let op = iface
            .events
            .iter()
            .find(|e| e.name == self.name())
            .unwrap_or_else(|| {
                panic!(
                    "event {}.{} missing from schema",
                    self.interface(),
                    self.name()
                )
            });
        let mut m = Message::new(object.as_u32(), op.opcode);
        for v in self.args() {
            m = m.arg(v);
        }
        m
    }
}

/// The direction token re-exported for doc cross-references.
#[allow(unused_imports)]
use Direction as _DirectionRef;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_values_pin_the_spec() {
        assert_eq!(PressState::Released.wire(), 1);
        assert_eq!(PressState::Pressed.wire(), 2);
        assert_eq!(Axis::Vertical.wire(), 1);
        assert_eq!(Axis::Horizontal.wire(), 2);
        assert_eq!(AxisSource::Wheel.wire(), 1);
        assert_eq!(AxisSource::Finger.wire(), 2);
        assert_eq!(AxisSource::Continuous.wire(), 3);
        assert_eq!(AxisSource::WheelTilt.wire(), 4);
        assert_eq!(KeymapFormat::XkbV1.wire(), 1);
        assert_eq!(TabletToolType::Pen.wire(), 1);
        assert_eq!(TabletToolType::Finger.wire(), 8);
        assert_eq!(TabletToolType::Lens.wire(), 7);
    }

    #[test]
    #[allow(clippy::too_many_lines)] // the exhaustive per-family table
    fn every_event_resolves_against_the_schema() {
        // One event per interface family: name + args shape.
        let cases: Vec<(SeatEvent, usize)> = vec![
            (SeatEvent::SeatCapabilities { caps: 0b1_0011 }, 1),
            (
                SeatEvent::SeatName {
                    name: "seat0".into(),
                },
                1,
            ),
            (
                SeatEvent::PointerEnter {
                    surface: ObjectId::from_wire(7),
                    x: 1.5,
                    y: 2.5,
                },
                3,
            ),
            (
                SeatEvent::PointerRelativeMotion {
                    dx_unaccel: 1.0,
                    dy_unaccel: 0.0,
                    dx_accel: 2.0,
                    dy_accel: 0.0,
                },
                4,
            ),
            (
                SeatEvent::PointerAxisSource {
                    source: AxisSource::Finger,
                },
                1,
            ),
            (SeatEvent::PointerFrame, 0),
            (
                SeatEvent::KeyboardModifiers {
                    depressed: 1,
                    latched: 0,
                    locked: 2,
                    group: 0,
                },
                4,
            ),
            (
                SeatEvent::KeyboardEnter {
                    surface: ObjectId::from_wire(9),
                    keys: vec![42, 30],
                },
                2,
            ),
            (
                SeatEvent::TouchDown {
                    id: 3,
                    surface: ObjectId::from_wire(9),
                    x: 0.5,
                    y: 0.5,
                },
                4,
            ),
            (SeatEvent::TouchCancel, 0),
            (
                SeatEvent::TabletTool {
                    id: 77,
                    tool_type: TabletToolType::Pen,
                },
                2,
            ),
            (
                SeatEvent::TabletTilt {
                    id: 77,
                    x: 0.1,
                    y: -0.2,
                },
                3,
            ),
            (
                SeatEvent::GesturePinchUpdate {
                    id: 1,
                    scale: 1.25,
                    dx: 0.5,
                    dy: -0.5,
                },
                4,
            ),
        ];
        for (ev, argc) in cases {
            let iface = REGISTRY
                .interface(ev.interface())
                .expect("interface in schema");
            let op = iface
                .events
                .iter()
                .find(|e| e.name == ev.name())
                .expect("event in schema");
            assert_eq!(op.args.len(), argc, "{}.{}", ev.interface(), ev.name());
            assert_eq!(ev.args().len(), argc);
            // And the message builds with the resolved opcode.
            let m = ev.to_message(ObjectId::from_wire(0x8000_0100));
            assert_eq!(m.opcode, op.opcode);
            assert_eq!(m.object_id, 0x8000_0100);
        }
    }

    #[test]
    fn keymap_event_carries_an_fd_index() {
        let ev = SeatEvent::KeyboardKeymap {
            format: KeymapFormat::XkbV1,
        };
        let m = ev.to_message(ObjectId::from_wire(0x8000_0002));
        assert_eq!(m.required_fd_count(), 1);
        assert!(matches!(m.args[0], Value::Fd(0)));
    }
}
