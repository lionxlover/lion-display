//! The multitouch slot machine — kernel protocol B.
//!
//! Protocol B devices carry a fixed set of *slots*; `ABS_MT_SLOT`
//! selects the slot subsequent `ABS_MT_*` events apply to, and
//! `ABS_MT_TRACKING_ID` assigns or releases (value `-1`) the contact
//! identity living in that slot. Between two `SYN_REPORT`s the kernel
//! emits only the *changed* axes of the *changed* slot — the machine
//! keeps full per-slot state and publishes a complete [`MtFrame`]
//! diff (active points, began/ended identities) per packet.
//!
//! Protocol A (slotless, `SYN_MT_REPORT`-separated) is refused with
//! `None`: the classifier already routes such devices to
//! [`crate::device::DeviceClass::Other`], and silently mis-parsing
//! them would corrupt gesture state.
//!
//! `SYN_DROPPED` resynchronizes by forgetting every contact — the
//! kernel's contract is that all state is void after a drop, and
//! clients see `cancel`-equivalent `ended` entries for every active
//! identity.

#![forbid(unsafe_code)]

use ldp_core::time::Mono;

use crate::codes::{abs, ev, mt_tool};
use crate::evdev::{DeviceFrame, RawEvent};

/// One active contact, in raw device coordinates.
///
/// Normalization to surface coordinates is the normalizer's job; the
/// slot machine is byte-faithful to the kernel's slot state.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TouchPoint {
    /// Tracking ID (stable for the contact's lifetime).
    pub id: i32,
    /// Raw X (device units).
    pub x: i32,
    /// Raw Y (device units).
    pub y: i32,
    /// Pressure axis, when the device has one.
    pub pressure: Option<i32>,
    /// Touch ellipse major axis, when reported.
    pub touch_major: Option<i32>,
    /// Touch ellipse minor axis, when reported.
    pub touch_minor: Option<i32>,
    /// Touch ellipse orientation, when reported.
    pub orientation: Option<i32>,
    /// Tool type (`MT_TOOL_*`), when reported.
    pub tool: Option<i32>,
}

impl TouchPoint {
    /// Whether the tool is a palm (rejection candidate).
    #[must_use]
    pub const fn is_palm(&self) -> bool {
        matches!(self.tool, Some(t) if t == mt_tool::PALM)
    }
}

/// One frame's slot-state diff.
#[derive(Clone, Debug)]
pub struct MtFrame {
    /// Frame time.
    pub time: Mono,
    /// All active points, ordered by slot index.
    pub points: Vec<TouchPoint>,
    /// Identities that became active this frame.
    pub began: Vec<i32>,
    /// Identities released this frame (including drop-resets).
    pub ended: Vec<i32>,
    /// The kernel dropped events before this frame.
    pub dropped: bool,
}

impl MtFrame {
    /// The number of active contacts.
    #[must_use]
    pub fn count(&self) -> usize {
        self.points.len()
    }

    /// Whether a tracking id is currently active.
    #[must_use]
    pub fn contains(&self, id: i32) -> bool {
        self.points.iter().any(|p| p.id == id)
    }
}

#[derive(Clone, Debug)]
struct Slot {
    tracking_id: i32,
    x: i32,
    y: i32,
    pressure: Option<i32>,
    touch_major: Option<i32>,
    touch_minor: Option<i32>,
    orientation: Option<i32>,
    tool: Option<i32>,
}

impl Slot {
    fn unused() -> Slot {
        Slot {
            tracking_id: -1,
            x: 0,
            y: 0,
            pressure: None,
            touch_major: None,
            touch_minor: None,
            orientation: None,
            tool: None,
        }
    }

    fn point(&self) -> TouchPoint {
        TouchPoint {
            id: self.tracking_id,
            x: self.x,
            y: self.y,
            pressure: self.pressure,
            touch_major: self.touch_major,
            touch_minor: self.touch_minor,
            orientation: self.orientation,
            tool: self.tool,
        }
    }
}

/// Protocol-B slot state for one device.
#[derive(Clone, Debug)]
pub struct MtSlots {
    slots: Vec<Slot>,
    current: usize,
}

impl MtSlots {
    /// A machine with `n_slots` slots, all unused.
    ///
    /// The count is the device's `ABS_MT_SLOT` maximum plus one (the
    /// slot axis is 0-based), read from the probe.
    #[must_use]
    pub fn new(n_slots: usize) -> MtSlots {
        MtSlots {
            slots: vec![Slot::unused(); n_slots],
            current: 0,
        }
    }

    /// The number of slots.
    #[must_use]
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// Whether the machine has no slots.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Feed one device frame; `None` for protocol-A packets or slotless
    /// touch (single-touch `ABS_X/Y` + `BTN_TOUCH` devices need no
    /// machine — the normalizer reads them directly).
    pub fn feed(&mut self, frame: &DeviceFrame) -> Option<MtFrame> {
        if frame.protocol_a {
            return None;
        }
        let mut began = Vec::new();
        let mut ended = Vec::new();
        if frame.dropped {
            // Resynchronization: every active identity ends.
            for slot in &mut self.slots {
                if slot.tracking_id >= 0 {
                    ended.push(slot.tracking_id);
                    *slot = Slot::unused();
                }
            }
            self.current = 0;
            return Some(MtFrame {
                time: frame.time,
                points: Vec::new(),
                began,
                ended,
                dropped: true,
            });
        }
        for e in &frame.events {
            self.apply(e, &mut began, &mut ended);
        }
        let points = self
            .slots
            .iter()
            .filter(|s| s.tracking_id >= 0)
            .map(Slot::point)
            .collect();
        Some(MtFrame {
            time: frame.time,
            points,
            began,
            ended,
            dropped: false,
        })
    }

    fn apply(&mut self, e: &RawEvent, began: &mut Vec<i32>, ended: &mut Vec<i32>) {
        if e.ev_type != ev::ABS {
            return;
        }
        if e.code == abs::MT_SLOT {
            let slot = e.value.max(0) as usize;
            if slot < self.slots.len() {
                self.current = slot;
            }
            return;
        }
        let Some(slot) = self.slots.get_mut(self.current) else {
            return;
        };
        match e.code {
            abs::MT_TRACKING_ID => {
                if e.value < 0 {
                    if slot.tracking_id >= 0 {
                        ended.push(slot.tracking_id);
                    }
                    slot.tracking_id = -1;
                } else if slot.tracking_id != e.value {
                    if slot.tracking_id >= 0 {
                        // Reassignment without release: the kernel does
                        // not do this, but a misbehaving driver must not
                        // leak the old identity.
                        ended.push(slot.tracking_id);
                    }
                    began.push(e.value);
                    slot.tracking_id = e.value;
                }
            }
            abs::MT_POSITION_X => slot.x = e.value,
            abs::MT_POSITION_Y => slot.y = e.value,
            abs::MT_PRESSURE => slot.pressure = Some(e.value),
            abs::MT_TOUCH_MAJOR => slot.touch_major = Some(e.value),
            abs::MT_TOUCH_MINOR => slot.touch_minor = Some(e.value),
            abs::MT_ORIENTATION => slot.orientation = Some(e.value),
            abs::MT_TOOL_TYPE => slot.tool = Some(e.value),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codes::{ev, syn};
    use crate::evdev::{Framer, RawEvent};

    const US: u64 = 1_000_000;

    fn run(mt: &mut MtSlots, events: &[RawEvent]) -> MtFrame {
        let mut framer = Framer::new();
        let packets = framer.feed_all(events);
        assert_eq!(packets.len(), 1);
        mt.feed(&packets[0]).expect("protocol B frame")
    }

    fn two_finger_down() -> Vec<RawEvent> {
        vec![
            RawEvent::new(US, ev::ABS, abs::MT_SLOT, 0),
            RawEvent::new(US, ev::ABS, abs::MT_TRACKING_ID, 11),
            RawEvent::new(US, ev::ABS, abs::MT_POSITION_X, 1000),
            RawEvent::new(US, ev::ABS, abs::MT_POSITION_Y, 1500),
            RawEvent::new(US, ev::ABS, abs::MT_PRESSURE, 42),
            RawEvent::new(US, ev::ABS, abs::MT_SLOT, 1),
            RawEvent::new(US, ev::ABS, abs::MT_TRACKING_ID, 12),
            RawEvent::new(US, ev::ABS, abs::MT_POSITION_X, 1200),
            RawEvent::new(US, ev::ABS, abs::MT_POSITION_Y, 1500),
            RawEvent::new(US, ev::SYN, syn::REPORT, 0),
        ]
    }

    #[test]
    fn two_fingers_then_move_then_lift() {
        let mut mt = MtSlots::new(2);
        let down = run(&mut mt, &two_finger_down());
        assert_eq!(down.count(), 2);
        assert_eq!(down.began, vec![11, 12]);
        assert!(down.ended.is_empty());
        assert_eq!(down.points[0].x, 1000);
        assert_eq!(down.points[1].id, 12);
        assert_eq!(down.points[0].pressure, Some(42));

        // Move slot 1 only (partial updates are the protocol's norm).
        let moved = run(
            &mut mt,
            &[
                RawEvent::new(US + 8000, ev::ABS, abs::MT_SLOT, 1),
                RawEvent::new(US + 8000, ev::ABS, abs::MT_POSITION_X, 1250),
                RawEvent::new(US + 8000, ev::SYN, syn::REPORT, 0),
            ],
        );
        assert!(moved.began.is_empty());
        assert!(moved.ended.is_empty());
        // Full state stays published, not just the delta.
        assert_eq!(moved.points[0].x, 1000);
        assert_eq!(moved.points[1].x, 1250);

        // Lift slot 0.
        let lifted = run(
            &mut mt,
            &[
                RawEvent::new(US + 16_000, ev::ABS, abs::MT_SLOT, 0),
                RawEvent::new(US + 16_000, ev::ABS, abs::MT_TRACKING_ID, -1),
                RawEvent::new(US + 16_000, ev::SYN, syn::REPORT, 0),
            ],
        );
        assert_eq!(lifted.count(), 1);
        assert_eq!(lifted.ended, vec![11]);
        assert!(lifted.contains(12));
    }

    #[test]
    fn tool_type_and_palm() {
        let mut mt = MtSlots::new(1);
        let frame = run(
            &mut mt,
            &[
                RawEvent::new(US, ev::ABS, abs::MT_TRACKING_ID, 5),
                RawEvent::new(US, ev::ABS, abs::MT_POSITION_X, 10),
                RawEvent::new(US, ev::ABS, abs::MT_POSITION_Y, 10),
                RawEvent::new(US, ev::ABS, abs::MT_TOOL_TYPE, mt_tool::PALM),
                RawEvent::new(US, ev::SYN, syn::REPORT, 0),
            ],
        );
        assert!(frame.points[0].is_palm());
    }

    #[test]
    fn syn_drop_resets_all_contacts() {
        let mut mt = MtSlots::new(2);
        run(&mut mt, &two_finger_down());
        let reset = run(
            &mut mt,
            &[
                RawEvent::new(US + 5000, ev::SYN, syn::DROPPED, 0),
                RawEvent::new(US + 5001, ev::SYN, syn::REPORT, 0),
            ],
        );
        assert!(reset.dropped);
        assert_eq!(reset.count(), 0);
        assert_eq!(reset.ended, vec![11, 12]);
    }

    #[test]
    fn tracking_id_reassignment_ends_old_identity() {
        let mut mt = MtSlots::new(1);
        let frame = run(
            &mut mt,
            &[
                RawEvent::new(US, ev::ABS, abs::MT_TRACKING_ID, 7),
                RawEvent::new(US, ev::ABS, abs::MT_POSITION_X, 5),
                RawEvent::new(US, ev::SYN, syn::REPORT, 0),
            ],
        );
        assert_eq!(frame.began, vec![7]);
        let reassigned = run(
            &mut mt,
            &[
                RawEvent::new(US + 1000, ev::ABS, abs::MT_TRACKING_ID, 8),
                RawEvent::new(US + 1000, ev::SYN, syn::REPORT, 0),
            ],
        );
        assert_eq!(reassigned.ended, vec![7]);
        assert_eq!(reassigned.began, vec![8]);
        assert_eq!(reassigned.points[0].id, 8);
    }

    #[test]
    fn out_of_range_slot_is_ignored() {
        let mut mt = MtSlots::new(2);
        let frame = run(
            &mut mt,
            &[
                RawEvent::new(US, ev::ABS, abs::MT_SLOT, 9),
                RawEvent::new(US, ev::ABS, abs::MT_POSITION_X, 100),
                RawEvent::new(US, ev::SYN, syn::REPORT, 0),
            ],
        );
        assert_eq!(frame.count(), 0);
    }

    #[test]
    fn protocol_a_refused() {
        let mut mt = MtSlots::new(2);
        let mut framer = Framer::new();
        let packets = framer.feed_all(&[
            RawEvent::new(US, ev::ABS, abs::X, 5),
            RawEvent::new(US, ev::SYN, syn::MT_REPORT, 0),
            RawEvent::new(US, ev::SYN, syn::REPORT, 0),
        ]);
        assert!(mt.feed(&packets[0]).is_none());
    }
}
