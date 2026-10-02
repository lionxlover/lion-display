//! The normalizer: device frames to typed, unit-honest input events.
//!
//! This is where device units die. Everything downstream — the seat
//! router, the gesture machines, the acceleration filter — works in
//! the vocabulary of [`InputEvent`]:
//!
//! * relative pointer deltas stay integer counts (the accelerator
//!   turns them into floats),
//! * wheel events carry both the discrete detent count and the
//!   high-resolution fraction (units of 1/120 detent, the kernel's
//!   `*_HI_RES` convention),
//! * touch positions are normalized 0..1 against the axis ranges and
//!   carry millimeter coordinates derived from the axis resolution
//!   (raw units when the device reports none — the gesture thresholds
//!   document this),
//! * tablet pressure is 0..1 with the kernel's *fuzz* dead-zone
//!   applied (changes within `fuzz` of the last value are noise),
//! * tilt is radians (the kernel's ±9000 hundredths-of-degree scale
//!   converted),
//! * kernel autorepeat (`EV_KEY` value 2) is dropped — repeat is
//!   modeled server-side ([`crate::repeat`]).
//!
//! Unknown events surface as [`InputEvent::Other`] instead of being
//! silently discarded; the seat layer logs them, never routes them.
//! Stylus tablets and single-touch screens both speak plain `ABS_X/Y`
//! plus `BTN_TOUCH`; the disambiguator is the stylus tool button
//! (`BTN_TOOL_PEN` et al.), which tablets always carry and screens
//! never do.

#![forbid(unsafe_code)]

use ldp_core::time::Mono;

use crate::codes::{abs, btn, ev, rel};
use crate::device::{AbsInfo, DeviceSpec};
use crate::evdev::{DeviceFrame, RawEvent};
use crate::mt::{MtFrame, MtSlots, TouchPoint as MtPoint};

/// A scroll axis.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Axis {
    /// Vertical (the classic wheel).
    Vertical,
    /// Horizontal (tilt wheels).
    Horizontal,
}

/// A wheel update: discrete detents plus the hi-res fraction.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Wheel {
    /// Which axis.
    pub axis: Axis,
    /// Discrete detents this frame (signed).
    pub discrete: i32,
    /// High-resolution fraction in 1/120 detent units (signed).
    pub hi_res: f32,
}

/// One normalized touch contact.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct TouchContact {
    /// Tracking ID (stable for the contact's lifetime).
    pub id: i32,
    /// Normalized X, 0..1 across the sensor.
    pub x: f32,
    /// Normalized Y, 0..1 across the sensor.
    pub y: f32,
    /// Millimeter X (raw units when no resolution is reported).
    pub x_mm: f32,
    /// Millimeter Y (raw units when no resolution is reported).
    pub y_mm: f32,
    /// Normalized pressure 0..1, when reported.
    pub pressure: Option<f32>,
    /// Contact ellipse major axis, millimeters, when reported.
    pub touch_major: Option<f32>,
    /// Contact ellipse minor axis, millimeters, when reported.
    pub touch_minor: Option<f32>,
    /// Ellipse orientation, radians, when reported.
    pub orientation: Option<f32>,
    /// Tool type, when reported (kernel `MT_TOOL_*` values).
    pub tool: Option<i32>,
}

/// One frame of touch state.
#[derive(Clone, PartialEq, Debug)]
pub struct TouchUpdate {
    /// Frame time.
    pub time: Mono,
    /// Active contacts.
    pub points: Vec<TouchContact>,
    /// Identities that began this frame.
    pub began: Vec<i32>,
    /// Identities that ended this frame.
    pub ended: Vec<i32>,
    /// Kernel dropped events: treat the sequence as cancelled.
    pub dropped: bool,
}

/// The in-proximity tool of a tablet.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TabletTool {
    /// Pen.
    Pen,
    /// Eraser.
    Eraser,
    /// Brush.
    Brush,
    /// Pencil.
    Pencil,
    /// Airbrush.
    Airbrush,
    /// Mouse puck.
    Mouse,
    /// Lens puck.
    Lens,
    /// Finger (touch-capable tablets).
    Finger,
}

impl TabletTool {
    /// From a `BTN_TOOL_*` code, if it is one.
    #[must_use]
    pub fn from_btn(code: u32) -> Option<TabletTool> {
        match code {
            btn::TOOL_PEN => Some(TabletTool::Pen),
            btn::TOOL_RUBBER => Some(TabletTool::Eraser),
            btn::TOOL_BRUSH => Some(TabletTool::Brush),
            btn::TOOL_PENCIL => Some(TabletTool::Pencil),
            btn::TOOL_AIRBRUSH => Some(TabletTool::Airbrush),
            btn::TOOL_MOUSE => Some(TabletTool::Mouse),
            btn::TOOL_LENS => Some(TabletTool::Lens),
            btn::TOOL_FINGER => Some(TabletTool::Finger),
            _ => None,
        }
    }
}

/// One frame of tablet state.
#[derive(Clone, PartialEq, Debug)]
pub struct TabletUpdate {
    /// Frame time.
    pub time: Mono,
    /// The in-proximity tool (`None` = no tool in proximity).
    pub tool: Option<TabletTool>,
    /// Whether the tool changed this frame (proximity transitions).
    pub tool_changed: bool,
    /// Contact is active (`BTN_TOUCH` or pressure above zero).
    pub contact: bool,
    /// Normalized X 0..1 across the sensor.
    pub x: f32,
    /// Normalized Y 0..1 across the sensor.
    pub y: f32,
    /// Normalized pressure 0..1 with fuzz applied, when reported.
    pub pressure: Option<f32>,
    /// Hover distance normalized 0..1, when reported.
    pub distance: Option<f32>,
    /// Tilt X, radians.
    pub tilt_x: Option<f32>,
    /// Tilt Y, radians.
    pub tilt_y: Option<f32>,
    /// Whether this frame carried axis content (as opposed to a
    /// pure proximity transition): axis events without it would be
    /// resting-value spam.
    pub axes_present: bool,
}

/// The typed event vocabulary: one device frame's normalized contents.
#[derive(Clone, PartialEq, Debug)]
pub enum InputEvent {
    /// Relative pointer motion accumulated over the frame.
    PointerMotion {
        /// Summed X delta, device counts.
        dx: i32,
        /// Summed Y delta, device counts.
        dy: i32,
    },
    /// A wheel update.
    Wheel(Wheel),
    /// A pointer or tool button transition.
    Button {
        /// Button code (`BTN_*`).
        button: u32,
        /// New state.
        pressed: bool,
    },
    /// A key transition (kernel autorepeat is filtered).
    Key {
        /// Keycode (`KEY_*`).
        keycode: u32,
        /// New state.
        pressed: bool,
    },
    /// A touch frame (multitouch or single-touch devices).
    Touch(TouchUpdate),
    /// A tablet frame.
    Tablet(TabletUpdate),
    /// An observed event outside the consumed vocabulary.
    Other {
        /// The raw event type.
        ev_type: u16,
        /// The raw code.
        code: u16,
        /// The raw value.
        value: i32,
    },
}

#[derive(Clone, Debug, Default)]
struct TabletState {
    tool: Option<TabletTool>,
    x: i32,
    y: i32,
    pressure: i32,
    distance: i32,
    tilt_x: i32,
    tilt_y: i32,
    contact: bool,
}

#[derive(Clone, Debug, Default)]
struct SingleTouchState {
    active: bool,
    x: i32,
    y: i32,
    pressure: i32,
}

/// Per-device normalization state.
#[derive(Debug)]
pub struct Normalizer {
    spec: DeviceSpec,
    mt: Option<MtSlots>,
    last_pressure: Option<i32>,
    tablet: TabletState,
    single_touch: SingleTouchState,
}

impl Normalizer {
    /// Build the normalizer for a probed device.
    #[must_use]
    pub fn new(spec: DeviceSpec) -> Normalizer {
        let mt = if spec.is_multitouch() {
            let n = spec
                .abs
                .get(&abs::MT_SLOT)
                .map_or(1, |a| a.max.max(0) as usize + 1);
            Some(MtSlots::new(n))
        } else {
            None
        };
        Normalizer {
            spec,
            mt,
            last_pressure: None,
            tablet: TabletState::default(),
            single_touch: SingleTouchState::default(),
        }
    }

    /// The device this normalizer belongs to.
    #[must_use]
    pub fn spec(&self) -> &DeviceSpec {
        &self.spec
    }

    /// Whether the device carries any stylus tool button.
    #[must_use]
    fn has_stylus(&self) -> bool {
        TabletTool::from_btn(btn::TOOL_PEN).is_some()
            && (self.spec.has_key(btn::TOOL_PEN)
                || self.spec.has_key(btn::TOOL_RUBBER)
                || self.spec.has_key(btn::TOOL_BRUSH)
                || self.spec.has_key(btn::TOOL_PENCIL)
                || self.spec.has_key(btn::TOOL_AIRBRUSH)
                || self.spec.has_key(btn::TOOL_MOUSE)
                || self.spec.has_key(btn::TOOL_LENS))
    }

    /// Normalize one device frame.
    #[must_use]
    pub fn normalize_frame(&mut self, frame: &DeviceFrame) -> Vec<InputEvent> {
        let mut out = Vec::new();
        let mut dx = 0i32;
        let mut dy = 0i32;
        let mut wheel_v = (0i32, 0f32);
        let mut wheel_h = (0i32, 0f32);
        let mut touch_input = false;
        let mut saw_plain_abs = false;

        for e in &frame.events {
            match e.ev_type {
                ev::REL => match e.code {
                    rel::X => dx += e.value,
                    rel::Y => dy += e.value,
                    rel::WHEEL => wheel_v.0 += e.value,
                    rel::WHEEL_HI_RES => wheel_v.1 += e.value as f32,
                    rel::HWHEEL => wheel_h.0 += e.value,
                    rel::HWHEEL_HI_RES => wheel_h.1 += e.value as f32,
                    _ => out.push(other(e)),
                },
                ev::KEY => {
                    // value 2 is kernel autorepeat; repeat is ours.
                    if e.value != 0 && e.value != 1 {
                        continue;
                    }
                    let pressed = e.value == 1;
                    let code = u32::from(e.code);
                    if let Some(tool) = TabletTool::from_btn(code) {
                        self.tablet.tool = pressed.then_some(tool);
                        out.push(InputEvent::Tablet(self.tablet_update(
                            frame.time,
                            true,
                            saw_plain_abs,
                        )));
                    } else if u32::from(e.code) == btn::TOUCH {
                        if self.spec.is_multitouch() || self.has_stylus() {
                            // Protocol B reports contact via slots; on
                            // tablets BTN_TOUCH is the contact flag the
                            // tablet state consumes.
                            self.tablet.contact = pressed;
                        } else {
                            self.single_touch.active = pressed;
                            touch_input = true;
                        }
                    } else if code < btn::LEFT {
                        // The key range: everything below the button
                        // block (`KEY_*`, not `BTN_*`).
                        out.push(InputEvent::Key {
                            keycode: code,
                            pressed,
                        });
                    } else {
                        out.push(InputEvent::Button {
                            button: code,
                            pressed,
                        });
                    }
                }
                ev::ABS => {
                    if e.code >= abs::MT_SLOT {
                        // Consumed by the slot machine at frame end.
                    } else {
                        self.apply_plain_abs(e.code, e.value);
                        saw_plain_abs = true;
                        if e.code == abs::X || e.code == abs::Y {
                            touch_input = true;
                        }
                    }
                }
                _ => out.push(other(e)),
            }
        }

        if dx != 0 || dy != 0 {
            out.push(InputEvent::PointerMotion { dx, dy });
        }
        for (axis, (discrete, hi_res)) in [(Axis::Vertical, wheel_v), (Axis::Horizontal, wheel_h)] {
            if discrete != 0 || hi_res != 0.0 {
                // Without a HI_RES axis, one detent = 120 hi-res units.
                let hi = if hi_res == 0.0 {
                    discrete as f32 * 120.0
                } else {
                    hi_res
                };
                out.push(InputEvent::Wheel(Wheel {
                    axis,
                    discrete,
                    hi_res: hi,
                }));
            }
        }

        if let Some(mt) = self.mt.as_mut() {
            if let Some(f) = mt.feed(frame) {
                out.push(InputEvent::Touch(self.touch_from_mt(&f)));
            }
        } else if touch_input && self.spec.has_key(btn::TOUCH) && !self.has_stylus() {
            out.push(InputEvent::Touch(self.touch_from_single(frame.time)));
        }

        // Tablet frames: a tool in proximity and actual content in the
        // frame (an empty SYN-only packet emits nothing, matching the
        // kernel's spurious-packet behavior).
        let has_content = !frame.events.is_empty();
        if has_content && self.tablet.tool.is_some() && self.has_stylus() && self.mt.is_none() {
            out.push(InputEvent::Tablet(self.tablet_update(
                frame.time,
                false,
                saw_plain_abs,
            )));
        }
        out
    }

    fn apply_plain_abs(&mut self, code: u16, value: i32) {
        let t = &mut self.tablet;
        match code {
            abs::X => {
                t.x = value;
                self.single_touch.x = value;
            }
            abs::Y => {
                t.y = value;
                self.single_touch.y = value;
            }
            abs::PRESSURE => {
                t.pressure = value;
                self.single_touch.pressure = value;
            }
            abs::DISTANCE => t.distance = value,
            abs::TILT_X => t.tilt_x = value,
            abs::TILT_Y => t.tilt_y = value,
            _ => {}
        }
    }

    #[must_use]
    fn tablet_update(
        &mut self,
        time: Mono,
        tool_changed: bool,
        axes_present: bool,
    ) -> TabletUpdate {
        let contact = self.tablet.contact || self.tablet.pressure > 0;
        TabletUpdate {
            time,
            tool: self.tablet.tool,
            tool_changed,
            contact,
            x: self.normalize_abs(abs::X, self.tablet.x),
            y: self.normalize_abs(abs::Y, self.tablet.y),
            pressure: self.normalize_fuzzed_pressure(self.tablet.pressure),
            distance: self.normalize_opt(abs::DISTANCE, self.tablet.distance),
            tilt_x: self.tilt(abs::TILT_X, self.tablet.tilt_x),
            tilt_y: self.tilt(abs::TILT_Y, self.tablet.tilt_y),
            axes_present,
        }
    }

    #[must_use]
    fn normalize_abs(&self, code: u16, value: i32) -> f32 {
        self.spec.abs.get(&code).map_or(0.0, |a| a.normalize(value))
    }

    #[must_use]
    fn normalize_opt(&self, code: u16, value: i32) -> Option<f32> {
        if !self.spec.has(ev::ABS, code) {
            return None;
        }
        self.spec.abs.get(&code).map(|a| a.normalize(value))
    }

    #[must_use]
    fn normalize_fuzzed_pressure(&mut self, value: i32) -> Option<f32> {
        if !self.spec.has(ev::ABS, abs::PRESSURE) {
            return None;
        }
        let info = self
            .spec
            .abs
            .get(&abs::PRESSURE)
            .copied()
            .unwrap_or(AbsInfo::default());
        let effective = match self.last_pressure {
            Some(last) if (value - last).abs() <= info.fuzz => last,
            _ => value,
        };
        self.last_pressure = Some(effective);
        Some(info.normalize(effective))
    }

    #[must_use]
    fn tilt(&self, code: u16, value: i32) -> Option<f32> {
        if !self.spec.has(ev::ABS, code) {
            return None;
        }
        // Kernel scale: hundredths of a degree.
        Some(value as f32 / 100.0 * core::f32::consts::PI / 180.0)
    }

    #[must_use]
    fn touch_from_mt(&mut self, f: &MtFrame) -> TouchUpdate {
        let points = f.points.iter().map(|p| self.contact_from_mt(p)).collect();
        TouchUpdate {
            time: f.time,
            points,
            began: f.began.clone(),
            ended: f.ended.clone(),
            dropped: f.dropped,
        }
    }

    #[must_use]
    fn contact_from_mt(&self, p: &MtPoint) -> TouchContact {
        TouchContact {
            id: p.id,
            x: self.axis_norm(abs::MT_POSITION_X, p.x),
            y: self.axis_norm(abs::MT_POSITION_Y, p.y),
            x_mm: self.axis_mm(abs::MT_POSITION_X, p.x),
            y_mm: self.axis_mm(abs::MT_POSITION_Y, p.y),
            pressure: p.pressure.map(|v| self.axis_norm(abs::MT_PRESSURE, v)),
            touch_major: p.touch_major.map(|v| self.axis_mm(abs::MT_TOUCH_MAJOR, v)),
            touch_minor: p.touch_minor.map(|v| self.axis_mm(abs::MT_TOUCH_MINOR, v)),
            // Kernel orientation: degrees.
            orientation: p
                .orientation
                .map(|v| v as f32 * core::f32::consts::PI / 180.0),
            tool: p.tool,
        }
    }

    #[must_use]
    fn touch_from_single(&mut self, time: Mono) -> TouchUpdate {
        let s = &self.single_touch;
        let x = self.axis_norm(abs::X, s.x);
        let y = self.axis_norm(abs::Y, s.y);
        let pressure = if self.spec.has(ev::ABS, abs::PRESSURE) {
            Some(self.axis_norm(abs::PRESSURE, s.pressure))
        } else {
            None
        };
        let (points, began, ended) = if s.active {
            (
                vec![TouchContact {
                    id: 0,
                    x,
                    y,
                    x_mm: self.axis_mm(abs::X, s.x),
                    y_mm: self.axis_mm(abs::Y, s.y),
                    pressure,
                    touch_major: None,
                    touch_minor: None,
                    orientation: None,
                    tool: None,
                }],
                vec![0],
                Vec::new(),
            )
        } else {
            (Vec::new(), Vec::new(), vec![0])
        };
        TouchUpdate {
            time,
            points,
            began,
            ended,
            dropped: false,
        }
    }

    #[must_use]
    fn axis_norm(&self, code: u16, value: i32) -> f32 {
        self.spec.abs.get(&code).map_or(0.0, |a| a.normalize(value))
    }

    #[must_use]
    fn axis_mm(&self, code: u16, value: i32) -> f32 {
        match self.spec.abs.get(&code).map(AbsInfo::mm_per_unit) {
            Some(Some(mm)) => value as f32 * mm,
            _ => value as f32,
        }
    }
}

#[must_use]
fn other(e: &RawEvent) -> InputEvent {
    InputEvent::Other {
        ev_type: e.ev_type,
        code: e.code,
        value: e.value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codes::syn;
    use crate::evdev::Framer;

    const US: u64 = 1_000_000;

    fn mouse_norm() -> Normalizer {
        Normalizer::new(
            DeviceSpec::new("m", crate::device::DeviceId::default())
                .with_bit(ev::REL, rel::X)
                .with_bit(ev::REL, rel::Y)
                .with_bit(ev::REL, rel::WHEEL)
                .with_bit(ev::KEY, btn::LEFT as u16),
        )
    }

    fn frame(events: &[RawEvent]) -> DeviceFrame {
        let mut f = Framer::new();
        f.feed_all(events).pop().expect("one frame")
    }

    #[test]
    fn mouse_motion_and_wheel() {
        let mut n = mouse_norm();
        let out = n.normalize_frame(&frame(&[
            RawEvent::new(US, ev::REL, rel::X, 3),
            RawEvent::new(US, ev::REL, rel::Y, -4),
            RawEvent::new(US, ev::REL, rel::WHEEL, -1),
            RawEvent::new(US, ev::SYN, syn::REPORT, 0),
        ]));
        assert_eq!(out[0], InputEvent::PointerMotion { dx: 3, dy: -4 });
        match &out[1] {
            InputEvent::Wheel(w) => {
                assert_eq!(w.axis, Axis::Vertical);
                assert_eq!(w.discrete, -1);
                assert!((w.hi_res + 120.0).abs() < f32::EPSILON);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn buttons_and_autorepeat_filtering() {
        let mut n = mouse_norm();
        let out = n.normalize_frame(&frame(&[
            RawEvent::new(US, ev::KEY, btn::LEFT as u16, 1),
            RawEvent::new(US, ev::SYN, syn::REPORT, 0),
        ]));
        assert_eq!(
            out[0],
            InputEvent::Button {
                button: btn::LEFT,
                pressed: true
            }
        );
        // Kernel autorepeat (value 2) is dropped.
        let out = n.normalize_frame(&frame(&[
            RawEvent::new(US, ev::KEY, btn::LEFT as u16, 2),
            RawEvent::new(US, ev::SYN, syn::REPORT, 0),
        ]));
        assert!(out.is_empty());
    }

    #[test]
    fn touchpad_positions_in_mm() {
        let mut spec = DeviceSpec::new("tp", crate::device::DeviceId::default())
            .with_abs(abs::MT_POSITION_X, AbsInfo::range(3200))
            .with_abs(abs::MT_POSITION_Y, AbsInfo::range(2100))
            .with_abs(abs::MT_SLOT, AbsInfo::range(2))
            .with_abs(abs::MT_TRACKING_ID, AbsInfo::range(65535));
        let mut x = *spec.abs.get(&abs::MT_POSITION_X).unwrap();
        x.resolution = 40; // 40 units/mm
        spec.abs.insert(abs::MT_POSITION_X, x);
        let mut n = Normalizer::new(spec);
        let out = n.normalize_frame(&frame(&[
            RawEvent::new(US, ev::ABS, abs::MT_SLOT, 0),
            RawEvent::new(US, ev::ABS, abs::MT_TRACKING_ID, 11),
            RawEvent::new(US, ev::ABS, abs::MT_POSITION_X, 1600),
            RawEvent::new(US, ev::ABS, abs::MT_POSITION_Y, 1050),
            RawEvent::new(US, ev::SYN, syn::REPORT, 0),
        ]));
        match &out[0] {
            InputEvent::Touch(t) => {
                assert_eq!(t.began, vec![11]);
                let p = &t.points[0];
                assert!((p.x - 0.5).abs() < 1e-6);
                assert!((p.y - 0.5).abs() < 1e-6);
                assert!((p.x_mm - 40.0).abs() < 1e-3); // 1600/40
                assert!((p.y_mm - 1050.0).abs() < 1e-3); // no resolution
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn tablet_pressure_fuzz_and_tilt() {
        let spec = DeviceSpec::new("tab", crate::device::DeviceId::default())
            .with_abs(abs::X, AbsInfo::range(1000))
            .with_abs(abs::Y, AbsInfo::range(1000))
            .with_abs(
                abs::PRESSURE,
                AbsInfo {
                    fuzz: 10,
                    ..AbsInfo::range(1000)
                },
            )
            .with_abs(abs::TILT_X, AbsInfo::symmetric(9000))
            .with_bit(ev::KEY, btn::TOOL_PEN as u16)
            .with_bit(ev::KEY, btn::STYLUS as u16);
        let mut n = Normalizer::new(spec);
        // Tool enters proximity.
        let out = n.normalize_frame(&frame(&[
            RawEvent::new(US, ev::KEY, btn::TOOL_PEN as u16, 1),
            RawEvent::new(US, ev::SYN, syn::REPORT, 0),
        ]));
        match &out[0] {
            InputEvent::Tablet(t) => {
                assert_eq!(t.tool, Some(TabletTool::Pen));
                assert!(t.tool_changed);
                assert!(!t.contact);
            }
            other => panic!("unexpected {other:?}"),
        }
        // Pressure 500 → normalized 0.5; contact becomes active.
        let out = n.normalize_frame(&frame(&[
            RawEvent::new(US, ev::ABS, abs::PRESSURE, 500),
            RawEvent::new(US, ev::ABS, abs::X, 250),
            RawEvent::new(US, ev::ABS, abs::Y, 750),
            RawEvent::new(US, ev::ABS, abs::TILT_X, 9000),
            RawEvent::new(US, ev::SYN, syn::REPORT, 0),
        ]));
        match &out[0] {
            InputEvent::Tablet(t) => {
                assert!(t.contact);
                assert!((t.x - 0.25).abs() < 1e-6);
                assert!((t.y - 0.75).abs() < 1e-6);
                assert!((t.pressure.unwrap() - 0.5).abs() < 1e-6);
                assert!((t.tilt_x.unwrap() - core::f32::consts::FRAC_PI_2).abs() < 1e-4);
            }
            other => panic!("unexpected {other:?}"),
        }
        // A wiggle within fuzz (10) must not move the value.
        let out = n.normalize_frame(&frame(&[
            RawEvent::new(US, ev::ABS, abs::PRESSURE, 505),
            RawEvent::new(US, ev::SYN, syn::REPORT, 0),
        ]));
        match &out[0] {
            InputEvent::Tablet(t) => assert!((t.pressure.unwrap() - 0.5).abs() < 1e-6),
            other => panic!("unexpected {other:?}"),
        }
        // A real change beyond fuzz moves it.
        let out = n.normalize_frame(&frame(&[
            RawEvent::new(US, ev::ABS, abs::PRESSURE, 520),
            RawEvent::new(US, ev::SYN, syn::REPORT, 0),
        ]));
        match &out[0] {
            InputEvent::Tablet(t) => assert!((t.pressure.unwrap() - 0.52).abs() < 1e-6),
            other => panic!("unexpected {other:?}"),
        }
        // Tool leaves proximity.
        let out = n.normalize_frame(&frame(&[
            RawEvent::new(US, ev::KEY, btn::TOOL_PEN as u16, 0),
            RawEvent::new(US, ev::SYN, syn::REPORT, 0),
        ]));
        match &out[0] {
            InputEvent::Tablet(t) => {
                assert_eq!(t.tool, None);
                assert!(t.tool_changed);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn single_touch_screen() {
        let spec = DeviceSpec::new("ts", crate::device::DeviceId::default())
            .with_abs(abs::X, AbsInfo::range(1919))
            .with_abs(abs::Y, AbsInfo::range(1079))
            .with_bit(ev::KEY, btn::TOUCH as u16);
        let mut n = Normalizer::new(spec);
        // Touch down at (960, 540) → normalized ~ (0.5, 0.5).
        let out = n.normalize_frame(&frame(&[
            RawEvent::new(US, ev::KEY, btn::TOUCH as u16, 1),
            RawEvent::new(US, ev::ABS, abs::X, 960),
            RawEvent::new(US, ev::ABS, abs::Y, 540),
            RawEvent::new(US, ev::SYN, syn::REPORT, 0),
        ]));
        match &out[0] {
            InputEvent::Touch(t) => {
                assert_eq!(t.began, vec![0]);
                let p = &t.points[0];
                assert!((p.x - 960.0 / 1919.0).abs() < 1e-4);
                assert!((p.y - 540.0 / 1079.0).abs() < 1e-4);
            }
            other => panic!("unexpected {other:?}"),
        }
        // Lift with no motion still reports the end.
        let out = n.normalize_frame(&frame(&[
            RawEvent::new(US, ev::KEY, btn::TOUCH as u16, 0),
            RawEvent::new(US, ev::SYN, syn::REPORT, 0),
        ]));
        match &out[0] {
            InputEvent::Touch(t) => {
                assert!(t.points.is_empty());
                assert_eq!(t.ended, vec![0]);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn unknown_events_surface_as_other() {
        let mut n = mouse_norm();
        let out = n.normalize_frame(&frame(&[
            RawEvent::new(US, ev::MSC, crate::codes::msc::SCAN, 0x1f),
            RawEvent::new(US, ev::SYN, syn::REPORT, 0),
        ]));
        assert_eq!(
            out[0],
            InputEvent::Other {
                ev_type: ev::MSC,
                code: crate::codes::msc::SCAN,
                value: 0x1f
            }
        );
    }
}
