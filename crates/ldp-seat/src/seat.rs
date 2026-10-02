//! Seats: coherent device groups with their own focus, keymap, and
//! capabilities.
//!
//! A [`Seat`] owns its devices (assignment *moves* the spec in — a
//! device lives in exactly one seat, enforced by ownership, checked
//! by tests). Devices arrive tagged with their udev `ID_SEAT`
//! property (default `seat0`); [`SeatManager::assign`] routes by tag,
//! creating seats on first sight.
//!
//! Capability bits (`seat_caps`): pointer from mouse/touchpad,
//! keyboard from keyboards, touch from touchscreens, tablet from
//! tablets. The gesture bit follows the touchpad (gestures are
//! recognized from touchpad contacts). [`SeatManager::assign`] and
//! [`SeatManager::remove_device`] recompute the mask and report the
//! change, because the wire re-emits `seat.capabilities` whenever
//! devices come and go.
//!
//! Multi-seat isolation (architecture §14) is structural here and in
//! the router: every seat carries its own focus router, grab model,
//! acceleration filter, gesture machine, and keymap state — two seats
//! never share focus, clipboard, or spaces because they never share
//! *anything*.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use ldp_input::device::{DeviceClass, DeviceSpec};

/// `seat_caps` bit positions.
pub mod seat_caps {
    /// The seat has a pointer.
    pub const POINTER: u32 = 1 << 0;
    /// The seat has a keyboard.
    pub const KEYBOARD: u32 = 1 << 1;
    /// The seat has touch.
    pub const TOUCH: u32 = 1 << 2;
    /// The seat has a tablet.
    pub const TABLET: u32 = 1 << 3;
    /// The seat has gesture recognition (touchpad).
    pub const GESTURE: u32 = 1 << 4;
}

/// Seat-manager failures.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SeatError {
    /// Two seats may not share a name.
    DuplicateSeat(String),
    /// No seat with that name.
    UnknownSeat(String),
    /// A device with that name is already assigned somewhere.
    DuplicateDevice(String),
}

impl std::fmt::Display for SeatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SeatError::DuplicateSeat(n) => write!(f, "seat {n} already exists"),
            SeatError::UnknownSeat(n) => write!(f, "no seat named {n}"),
            SeatError::DuplicateDevice(n) => write!(f, "device {n} already assigned"),
        }
    }
}

impl std::error::Error for SeatError {}

/// One seat: devices and capabilities.
#[derive(Clone, Debug)]
pub struct Seat {
    /// The seat's name (its registry-global identity).
    pub name: String,
    /// The assigned devices, in assignment order.
    pub devices: Vec<DeviceSpec>,
    /// The current capability mask (`seat_caps` bits).
    pub caps: u32,
}

impl Seat {
    /// The capability mask implied by the device set.
    #[must_use]
    pub fn compute_caps(devices: &[DeviceSpec]) -> u32 {
        let mut caps = 0u32;
        for d in devices {
            match d.classify() {
                DeviceClass::Mouse | DeviceClass::Touchpad => caps |= seat_caps::POINTER,
                DeviceClass::Keyboard => caps |= seat_caps::KEYBOARD,
                DeviceClass::Touchscreen => caps |= seat_caps::TOUCH,
                DeviceClass::Tablet => caps |= seat_caps::TABLET | seat_caps::POINTER,
                DeviceClass::Other => {}
            }
            // The gesture bit rides the touchpad specifically.
            if d.hint.touchpad || matches!(d.classify(), DeviceClass::Touchpad) {
                caps |= seat_caps::GESTURE;
            }
        }
        caps
    }
}

/// The outcome of an assignment or removal: the seat's current
/// capability mask (the wire re-emits `seat.capabilities` whenever
/// this differs from the last emitted value — callers compare).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CapsChange {
    /// The seat whose capabilities this is.
    pub seat: String,
    /// The new capability mask.
    pub caps: u32,
}

/// All seats, keyed by name, creation-ordered.
#[derive(Clone, Debug, Default)]
pub struct SeatManager {
    order: Vec<String>,
    seats: BTreeMap<String, Seat>,
}

impl SeatManager {
    /// No seats.
    #[must_use]
    pub fn new() -> SeatManager {
        SeatManager::default()
    }

    /// The seats in creation order.
    #[must_use]
    pub fn seats(&self) -> Vec<&Seat> {
        self.order
            .iter()
            .filter_map(|n| self.seats.get(n))
            .collect()
    }

    /// One seat by name.
    #[must_use]
    pub fn seat(&self, name: &str) -> Option<&Seat> {
        self.seats.get(name)
    }

    /// One seat by name, mutable.
    #[must_use]
    pub fn seat_mut(&mut self, name: &str) -> Option<&mut Seat> {
        self.seats.get_mut(name)
    }

    /// Create a seat explicitly.
    ///
    /// # Errors
    /// [`SeatError::DuplicateSeat`] when the name is taken.
    pub fn create_seat(&mut self, name: &str) -> Result<(), SeatError> {
        if self.seats.contains_key(name) {
            return Err(SeatError::DuplicateSeat(name.to_owned()));
        }
        self.order.push(name.to_owned());
        self.seats.insert(
            name.to_owned(),
            Seat {
                name: name.to_owned(),
                devices: Vec::new(),
                caps: 0,
            },
        );
        Ok(())
    }

    /// Assign a device by its seat tag (auto-creating the seat).
    /// Returns the seat name and the new capability mask.
    ///
    /// # Errors
    /// [`SeatError::DuplicateDevice`] when a device with the same
    /// name is assigned to any seat.
    ///
    /// # Panics
    /// Never: the two `expect`s guard paths proven unreachable above
    /// (the duplicate-name and just-created-seat checks).
    pub fn assign(&mut self, spec: DeviceSpec) -> Result<CapsChange, SeatError> {
        let device_name = spec.name.clone();
        if self
            .seats
            .values()
            .any(|s| s.devices.iter().any(|d| d.name == device_name))
        {
            return Err(SeatError::DuplicateDevice(device_name));
        }
        let seat_name = if spec.seat_tag.is_empty() {
            "seat0".to_owned()
        } else {
            spec.seat_tag.clone()
        };
        if !self.seats.contains_key(&seat_name) {
            self.create_seat(&seat_name)
                .expect("checked above: not a duplicate");
        }
        let seat = self.seats.get_mut(&seat_name).expect("just created");
        seat.devices.push(spec);
        seat.caps = Seat::compute_caps(&seat.devices);
        Ok(CapsChange {
            seat: seat_name,
            caps: seat.caps,
        })
    }

    /// Remove a device (unplug); returns the capability change.
    ///
    /// # Errors
    /// [`SeatError::UnknownSeat`] or [`SeatError::DuplicateDevice`]
    /// (the device-not-found spelling) when absent.
    pub fn remove_device(
        &mut self,
        seat_name: &str,
        device_name: &str,
    ) -> Result<CapsChange, SeatError> {
        let Some(seat) = self.seats.get_mut(seat_name) else {
            return Err(SeatError::UnknownSeat(seat_name.to_owned()));
        };
        let Some(idx) = seat.devices.iter().position(|d| d.name == device_name) else {
            return Err(SeatError::DuplicateDevice(device_name.to_owned()));
        };
        seat.devices.remove(idx);
        seat.caps = Seat::compute_caps(&seat.devices);
        Ok(CapsChange {
            seat: seat_name.to_owned(),
            caps: seat.caps,
        })
    }

    /// Which seat owns a device name.
    #[must_use]
    pub fn seat_of_device(&self, device_name: &str) -> Option<&str> {
        self.seats
            .values()
            .find(|s| s.devices.iter().any(|d| d.name == device_name))
            .map(|s| s.name.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ldp_input::codes::{btn, ev, rel};
    use ldp_input::device::{AbsInfo, ClassHint, DeviceId};

    fn mouse() -> DeviceSpec {
        DeviceSpec::new("M1", DeviceId::default())
            .with_bit(ev::REL, rel::X)
            .with_bit(ev::REL, rel::Y)
            .with_bit(ev::KEY, btn::LEFT as u16)
            .with_bit(ev::KEY, btn::RIGHT as u16)
            .with_hint(ClassHint {
                mouse: true,
                ..ClassHint::default()
            })
    }

    fn keyboard() -> DeviceSpec {
        DeviceSpec::new("K1", DeviceId::default())
            .with_bit(ev::KEY, ldp_input::codes::key::A as u16)
    }

    fn touchpad() -> DeviceSpec {
        DeviceSpec::new("T1", DeviceId::default())
            .with_abs(ldp_input::codes::abs::MT_POSITION_X, AbsInfo::range(3200))
            .with_abs(ldp_input::codes::abs::MT_POSITION_Y, AbsInfo::range(2100))
            .with_abs(ldp_input::codes::abs::MT_SLOT, AbsInfo::range(2))
            .with_abs(ldp_input::codes::abs::MT_TRACKING_ID, AbsInfo::range(65535))
            .with_bit(ev::KEY, btn::TOUCH as u16)
            .with_bit(ev::KEY, btn::TOOL_FINGER as u16)
            .with_hint(ClassHint {
                touchpad: true,
                ..ClassHint::default()
            })
    }

    fn with_tag(mut spec: DeviceSpec, tag: &str) -> DeviceSpec {
        spec.seat_tag = tag.to_owned();
        spec
    }

    #[test]
    fn assignment_by_tag_and_auto_creation() {
        let mut m = SeatManager::new();
        let c = m.assign(with_tag(mouse(), "seat0")).expect("assign");
        assert_eq!(c.seat, "seat0");
        assert_eq!(c.caps, seat_caps::POINTER);

        // A second seat springs into existence on first sight.
        let c = m.assign(with_tag(keyboard(), "seat-usb")).expect("assign");
        assert_eq!(c.seat, "seat-usb");
        assert_eq!(c.caps, seat_caps::KEYBOARD);
        assert_eq!(m.seats().len(), 2);

        // The touchpad brings pointer AND gesture bits.
        let c = m.assign(with_tag(touchpad(), "seat0")).expect("assign");
        assert_eq!(c.caps, seat_caps::POINTER | seat_caps::GESTURE);
    }

    #[test]
    fn duplicate_devices_are_refused() {
        let mut m = SeatManager::new();
        m.assign(mouse()).expect("first");
        let err = m.assign(mouse()).expect_err("duplicate");
        assert_eq!(err, SeatError::DuplicateDevice("M1".into()));
        // Even into another seat: one device, one seat, ever.
        let err = m
            .assign(with_tag(mouse(), "seat1"))
            .expect_err("cross-seat duplicate");
        assert!(matches!(err, SeatError::DuplicateDevice(_)));
    }

    #[test]
    fn removal_recomputes_caps() {
        let mut m = SeatManager::new();
        m.assign(mouse()).expect("assign");
        m.assign(keyboard()).expect("assign");
        let c = m.remove_device("seat0", "M1").expect("remove");
        assert_eq!(c.caps, seat_caps::KEYBOARD);
        assert_eq!(m.seat("seat0").expect("seat").devices.len(), 1);
        let err = m.remove_device("seat0", "M1").expect_err("gone");
        assert!(matches!(err, SeatError::DuplicateDevice(_)));
        let err = m.remove_device("nope", "K1").expect_err("no seat");
        assert_eq!(err, SeatError::UnknownSeat("nope".into()));
    }

    #[test]
    fn seat_lookup_and_ownership() {
        let mut m = SeatManager::new();
        m.assign(mouse()).expect("assign");
        let mut second = DeviceSpec::new("M2", DeviceId::default())
            .with_bit(ev::REL, rel::X)
            .with_bit(ev::REL, rel::Y)
            .with_bit(ev::KEY, btn::LEFT as u16)
            .with_hint(ClassHint {
                mouse: true,
                ..ClassHint::default()
            });
        second.seat_tag = "seat-usb".to_owned();
        m.assign(second).expect("assign");
        // Devices are distinct (different names from the callers), so
        // ownership is well-defined:
        assert_eq!(m.seat_of_device("M1"), Some("seat0"));
        assert_eq!(m.seat_of_device("nothing"), None);
        assert!(m.create_seat("seat0").is_err());
    }

    #[test]
    fn empty_tag_defaults_to_seat0() {
        let mut m = SeatManager::new();
        let mut spec = mouse();
        spec.seat_tag = String::new();
        let c = m.assign(spec).expect("assign");
        assert_eq!(c.seat, "seat0");
    }
}
