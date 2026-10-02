//! The device model: identity, capability bits, ABS axis ranges, class
//! classification, and LED indicator state.
//!
//! A [`DeviceSpec`] is the *probe result* — everything the kernel
//! exposes through `EVIOCGID`/`EVIOCGBIT`/`EVIOCGABS`, plus the udev
//! seat tag and input-class hints the hotplug layer reads on real
//! systems. Headless tests construct specs directly, which is the same
//! information path minus the ioctls.
//!
//! Classification ([`DeviceClass`]) is a pure function of the spec:
//! explicit hints win (udev's `ID_INPUT_*` properties are authoritative
//! on real hardware), and the bit-pattern fallback documents exactly
//! which combinations map to which class. The multitouch slot machine
//! itself lives in [`crate::mt`]; this module is the static model.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use crate::codes::{abs, btn, ev};

/// Mirror of `struct input_absinfo`: one ABS axis's range metadata.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct AbsInfo {
    /// Last reported value (from the probe; informational).
    pub value: i32,
    /// Axis minimum.
    pub min: i32,
    /// Axis maximum.
    pub max: i32,
    /// Noise tolerance: deltas within `fuzz` are ignored.
    pub fuzz: i32,
    /// Flat region: values within `flat` of the center are center.
    pub flat: i32,
    /// Resolution in units/mm (0 = unknown).
    pub resolution: u32,
}

impl AbsInfo {
    /// A clean 0..`max` axis (the common synthesized case).
    #[must_use]
    pub fn range(max: i32) -> AbsInfo {
        AbsInfo {
            min: 0,
            max,
            ..AbsInfo::default()
        }
    }

    /// A signed symmetric axis, e.g. tilt (-9000..9000).
    #[must_use]
    pub fn symmetric(max: i32) -> AbsInfo {
        AbsInfo {
            min: -max,
            max,
            ..AbsInfo::default()
        }
    }

    /// Normalize a raw value into 0..1 against the range, clamped.
    ///
    /// Fuzz is *not* applied here — it is a dead-zone concern of the
    /// normalizer, which needs the previous value to decide whether a
    /// change is real.
    #[must_use]
    pub fn normalize(&self, value: i32) -> f32 {
        if self.max <= self.min {
            return 0.0;
        }
        let span = (self.max - self.min) as f32;
        let v = (value - self.min) as f32 / span;
        v.clamp(0.0, 1.0)
    }

    /// Millimeters per unit (resolution inverted), if known.
    #[must_use]
    pub fn mm_per_unit(&self) -> Option<f32> {
        if self.resolution == 0 {
            None
        } else {
            Some(1.0 / self.resolution as f32)
        }
    }
}

/// Mirror of `struct input_id`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct DeviceId {
    /// Bus (`BUS_*`).
    pub bustype: u16,
    /// Vendor ID.
    pub vendor: u16,
    /// Product ID.
    pub product: u16,
    /// Version.
    pub version: u16,
}

/// The udev input-class hints (`ID_INPUT_*`), authoritative when
/// present; injected by tests, read from properties on real systems.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct ClassHint {
    /// `ID_INPUT_MOUSE`.
    pub mouse: bool,
    /// `ID_INPUT_TOUCHPAD`.
    pub touchpad: bool,
    /// `ID_INPUT_TOUCHSCREEN`.
    pub touchscreen: bool,
    /// `ID_INPUT_TABLET`.
    pub tablet: bool,
    /// `ID_INPUT_KEYBOARD`.
    pub keyboard: bool,
}

/// What kind of device this is, as the input pipeline treats it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DeviceClass {
    /// Relative pointer (mouse, trackball).
    Mouse,
    /// Absolute multitouch touchpad (gestures, finger-scroll).
    Touchpad,
    /// Absolute multitouch screen (direct touch).
    Touchscreen,
    /// Stylus tablet (pressure, tilt, tools).
    Tablet,
    /// Key-emitting keyboard.
    Keyboard,
    /// Emits nothing the pipeline consumes (observed, not hidden).
    Other,
}

impl DeviceClass {
    /// The seat capability bit each class contributes.
    #[must_use]
    pub const fn seat_cap_bit(self) -> Option<u32> {
        match self {
            DeviceClass::Mouse | DeviceClass::Touchpad => Some(0),
            DeviceClass::Keyboard => Some(1),
            DeviceClass::Touchscreen => Some(2),
            DeviceClass::Tablet => Some(3),
            DeviceClass::Other => None,
        }
    }
}

/// A fully probed input device: identity, bits, and ABS ranges.
#[derive(Clone, Debug, Default)]
pub struct DeviceSpec {
    /// Kernel device name (`EVIOCGNAME`).
    pub name: String,
    /// Bus/vendor/product/version.
    pub id: DeviceId,
    /// Event codes the device can emit: `(type, code)` pairs.
    pub bits: std::collections::BTreeSet<(u16, u16)>,
    /// ABS axis metadata per code.
    pub abs: BTreeMap<u16, AbsInfo>,
    /// The udev `ID_SEAT` tag (defaults to `seat0` when absent).
    pub seat_tag: String,
    /// The udev `ID_INPUT_*` hints.
    pub hint: ClassHint,
}

impl DeviceSpec {
    /// An empty spec with the default seat tag.
    #[must_use]
    pub fn new(name: &str, id: DeviceId) -> DeviceSpec {
        DeviceSpec {
            name: name.to_owned(),
            id,
            seat_tag: "seat0".to_owned(),
            ..DeviceSpec::default()
        }
    }

    /// Declare an emitted code.
    #[must_use]
    pub fn with_bit(mut self, ev_type: u16, code: u16) -> DeviceSpec {
        self.bits.insert((ev_type, code));
        self
    }

    /// Declare an ABS axis with its range.
    #[must_use]
    pub fn with_abs(mut self, code: u16, info: AbsInfo) -> DeviceSpec {
        self.bits.insert((ev::ABS, code));
        self.abs.insert(code, info);
        self
    }

    /// Declare the hints.
    #[must_use]
    pub fn with_hint(mut self, hint: ClassHint) -> DeviceSpec {
        self.hint = hint;
        self
    }

    /// Whether the device emits a code.
    #[must_use]
    pub fn has(&self, ev_type: u16, code: u16) -> bool {
        self.bits.contains(&(ev_type, code))
    }

    /// Whether the device emits any code of a type.
    #[must_use]
    pub fn has_type(&self, ev_type: u16) -> bool {
        self.bits
            .range((ev_type, 0)..(ev_type, u16::MAX))
            .next()
            .is_some()
    }

    /// Whether a key/button code (as `u32`, evdev `KEY_*`/`BTN_*`) is
    /// declared.
    #[must_use]
    pub fn has_key(&self, code: u32) -> bool {
        match u16::try_from(code) {
            Ok(c) => self.has(ev::KEY, c),
            Err(_) => false,
        }
    }

    /// Whether the multitouch position axes exist.
    #[must_use]
    pub fn is_multitouch(&self) -> bool {
        self.has(ev::ABS, abs::MT_POSITION_X) && self.has(ev::ABS, abs::MT_POSITION_Y)
    }

    /// The classification (hint-first, bit-pattern fallback).
    ///
    /// Fallback rules, in order: stylus tool buttons plus absolute X/Y
    /// and pressure make a tablet; multitouch position plus `BTN_TOUCH`
    /// makes a touchscreen (real touchpads always carry the udev hint,
    /// which wins here); relative X/Y plus pointer buttons make a
    /// mouse; any plain key below the button range makes a keyboard.
    #[must_use]
    pub fn classify(&self) -> DeviceClass {
        let stylus = self.has_key(btn::TOOL_PEN)
            || self.has_key(btn::TOOL_RUBBER)
            || self.has_key(btn::TOOL_BRUSH)
            || self.has_key(btn::TOOL_PENCIL)
            || self.has_key(btn::TOOL_AIRBRUSH);
        let pen_axes = self.has(ev::ABS, abs::X)
            && self.has(ev::ABS, abs::Y)
            && self.has(ev::ABS, abs::PRESSURE);
        let rel_pointer = self.has(ev::REL, crate::codes::rel::X)
            && self.has(ev::REL, crate::codes::rel::Y)
            && self.has_key(btn::LEFT);
        let has_plain_keys = self
            .bits
            .range((ev::KEY, 0)..(ev::KEY, btn::LEFT as u16))
            .next()
            .is_some();

        if self.hint.tablet && stylus {
            return DeviceClass::Tablet;
        }
        if self.hint.touchpad && self.is_multitouch() {
            return DeviceClass::Touchpad;
        }
        if self.hint.touchscreen && self.is_multitouch() {
            return DeviceClass::Touchscreen;
        }
        if self.hint.mouse && rel_pointer {
            return DeviceClass::Mouse;
        }
        if self.hint.keyboard && has_plain_keys {
            return DeviceClass::Keyboard;
        }
        if stylus && pen_axes {
            return DeviceClass::Tablet;
        }
        if self.is_multitouch() && self.has_key(btn::TOUCH) {
            return DeviceClass::Touchscreen;
        }
        if rel_pointer {
            return DeviceClass::Mouse;
        }
        if has_plain_keys {
            return DeviceClass::Keyboard;
        }
        DeviceClass::Other
    }
}

/// Keyboard indicator state, tracked for LED writeback.
///
/// The compositor owns the truth (xkb locked modifiers); the device is
/// an output for it. [`LedState`] is the bit image to write back.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct LedState {
    bits: u8,
}

const LED_NUML: u8 = 1 << 0;
const LED_CAPSL: u8 = 1 << 1;
const LED_SCROLLL: u8 = 1 << 2;
const LED_COMPOSE: u8 = 1 << 3;

impl LedState {
    /// All indicators off.
    #[must_use]
    pub const fn new() -> LedState {
        LedState { bits: 0 }
    }

    /// Set one LED code (`LED_*`) from an event value.
    pub fn apply(&mut self, code: u16, value: i32) {
        let bit = match code {
            crate::codes::led::NUML => LED_NUML,
            crate::codes::led::CAPSL => LED_CAPSL,
            crate::codes::led::SCROLLL => LED_SCROLLL,
            crate::codes::led::COMPOSE => LED_COMPOSE,
            _ => return,
        };
        if value != 0 {
            self.bits |= bit;
        } else {
            self.bits &= !bit;
        }
    }

    /// Num Lock on.
    #[must_use]
    pub const fn num_lock(self) -> bool {
        self.bits & LED_NUML != 0
    }

    /// Caps Lock on.
    #[must_use]
    pub const fn caps_lock(self) -> bool {
        self.bits & LED_CAPSL != 0
    }

    /// Scroll Lock on.
    #[must_use]
    pub const fn scroll_lock(self) -> bool {
        self.bits & LED_SCROLLL != 0
    }

    /// Compose on.
    #[must_use]
    pub const fn compose(self) -> bool {
        self.bits & LED_COMPOSE != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codes::key;

    fn mouse() -> DeviceSpec {
        DeviceSpec::new(
            "Test Mouse",
            DeviceId {
                bustype: crate::codes::bus::USB,
                ..DeviceId::default()
            },
        )
        .with_bit(ev::REL, crate::codes::rel::X)
        .with_bit(ev::REL, crate::codes::rel::Y)
        .with_bit(ev::KEY, btn::LEFT as u16)
        .with_bit(ev::KEY, btn::RIGHT as u16)
        .with_bit(ev::KEY, btn::MIDDLE as u16)
    }

    fn touchpad() -> DeviceSpec {
        DeviceSpec::new(
            "Test Touchpad",
            DeviceId {
                bustype: crate::codes::bus::I8042,
                ..DeviceId::default()
            },
        )
        .with_abs(abs::MT_POSITION_X, AbsInfo::range(3200))
        .with_abs(abs::MT_POSITION_Y, AbsInfo::range(2100))
        .with_abs(abs::MT_SLOT, AbsInfo::range(2))
        .with_abs(abs::MT_TRACKING_ID, AbsInfo::range(65535))
        .with_bit(ev::KEY, btn::TOUCH as u16)
        .with_bit(ev::KEY, btn::TOOL_FINGER as u16)
        .with_bit(ev::KEY, btn::LEFT as u16)
        .with_hint(ClassHint {
            touchpad: true,
            ..ClassHint::default()
        })
    }

    fn tablet() -> DeviceSpec {
        DeviceSpec::new(
            "Test Tablet",
            DeviceId {
                bustype: crate::codes::bus::USB,
                ..DeviceId::default()
            },
        )
        .with_abs(abs::X, AbsInfo::range(31_680))
        .with_abs(abs::Y, AbsInfo::range(23_625))
        .with_abs(abs::PRESSURE, AbsInfo::range(4095))
        .with_abs(abs::TILT_X, AbsInfo::symmetric(9000))
        .with_abs(abs::TILT_Y, AbsInfo::symmetric(9000))
        .with_bit(ev::KEY, btn::TOOL_PEN as u16)
        .with_bit(ev::KEY, btn::TOOL_RUBBER as u16)
        .with_bit(ev::KEY, btn::STYLUS as u16)
        .with_bit(ev::KEY, btn::TOUCH as u16)
    }

    fn touchscreen() -> DeviceSpec {
        DeviceSpec::new(
            "Test Touchscreen",
            DeviceId {
                bustype: crate::codes::bus::USB,
                ..DeviceId::default()
            },
        )
        .with_abs(abs::MT_POSITION_X, AbsInfo::range(1920))
        .with_abs(abs::MT_POSITION_Y, AbsInfo::range(1080))
        .with_bit(ev::KEY, btn::TOUCH as u16)
    }

    fn keyboard() -> DeviceSpec {
        DeviceSpec::new(
            "Test Keyboard",
            DeviceId {
                bustype: crate::codes::bus::USB,
                ..DeviceId::default()
            },
        )
        .with_bit(ev::KEY, key::A as u16)
        .with_bit(ev::KEY, key::LEFTSHIFT as u16)
        .with_bit(ev::LED, crate::codes::led::CAPSL)
    }

    #[test]
    fn classification_by_hint_and_bits() {
        assert_eq!(mouse().classify(), DeviceClass::Mouse);
        assert_eq!(touchpad().classify(), DeviceClass::Touchpad);
        assert_eq!(tablet().classify(), DeviceClass::Tablet);
        assert_eq!(touchscreen().classify(), DeviceClass::Touchscreen);
        assert_eq!(keyboard().classify(), DeviceClass::Keyboard);
    }

    #[test]
    fn touchpad_without_hint_falls_back_to_touchscreen() {
        // Documented fallback: no udev hint + MT + BTN_TOUCH.
        let mut spec = touchpad();
        spec.hint = ClassHint::default();
        assert_eq!(spec.classify(), DeviceClass::Touchscreen);
    }

    #[test]
    fn seat_capability_bits() {
        assert_eq!(DeviceClass::Mouse.seat_cap_bit(), Some(0));
        assert_eq!(DeviceClass::Touchpad.seat_cap_bit(), Some(0));
        assert_eq!(DeviceClass::Keyboard.seat_cap_bit(), Some(1));
        assert_eq!(DeviceClass::Touchscreen.seat_cap_bit(), Some(2));
        assert_eq!(DeviceClass::Tablet.seat_cap_bit(), Some(3));
        assert_eq!(DeviceClass::Other.seat_cap_bit(), None);
    }

    #[test]
    fn absinfo_normalization() {
        let a = AbsInfo {
            min: 100,
            max: 300,
            ..AbsInfo::default()
        };
        assert_eq!(a.normalize(100), 0.0);
        assert_eq!(a.normalize(200), 0.5);
        assert_eq!(a.normalize(300), 1.0);
        assert_eq!(a.normalize(-1000), 0.0);
        assert_eq!(a.normalize(5000), 1.0);
        // Degenerate range normalizes to zero.
        assert_eq!(AbsInfo::default().normalize(50), 0.0);
        // Symmetric tilt: center is 0.5.
        let t = AbsInfo::symmetric(9000);
        assert_eq!(t.normalize(0), 0.5);
        assert_eq!(t.normalize(-9000), 0.0);
        assert_eq!(t.normalize(9000), 1.0);
    }

    #[test]
    fn led_state_transitions() {
        let mut led = LedState::new();
        led.apply(crate::codes::led::CAPSL, 1);
        assert!(led.caps_lock());
        led.apply(crate::codes::led::NUML, 1);
        assert!(led.num_lock());
        led.apply(crate::codes::led::CAPSL, 0);
        assert!(!led.caps_lock());
        assert!(led.num_lock());
        // Unknown LED codes are ignored.
        led.apply(0x7f, 1);
        assert!(!led.scroll_lock());
        assert!(!led.compose());
    }

    #[test]
    fn mm_per_unit_resolution() {
        let mut a = AbsInfo::range(3200);
        assert_eq!(a.mm_per_unit(), None);
        a.resolution = 40; // 40 units per mm
        assert!((a.mm_per_unit().unwrap() - 0.025).abs() < 1e-6);
    }

    #[test]
    fn multitouch_detection() {
        assert!(touchpad().is_multitouch());
        assert!(touchscreen().is_multitouch());
        assert!(!mouse().is_multitouch());
        assert!(!keyboard().is_multitouch());
    }
}
