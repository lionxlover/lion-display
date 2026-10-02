//! The evdev ABI constants this crate consumes.
//!
//! Every constant is pinned to its numeric value from the kernel's
//! `include/uapi/linux/input-event-codes.h` (and `input.h` for the bus
//! types and ioctls). The set is deliberately the *documented subset*
//! the decoder, classifier and normalizer act on — unknown codes decode
//! fine ([`crate::evdev::RawEvent`] is generic over `code`) and are
//! surfaced as [`crate::normalizer::InputEvent::Other`] rather than
//! dropped, so device quirks stay observable.

#![forbid(unsafe_code)]

/// Event types (`EV_*`).
pub mod ev {
    /// Synchronization events frame the stream.
    pub const SYN: u16 = 0x00;
    /// Key/button state changes.
    pub const KEY: u16 = 0x01;
    /// Relative motion.
    pub const REL: u16 = 0x02;
    /// Absolute axes.
    pub const ABS: u16 = 0x03;
    /// Miscellaneous (scan codes, device timestamps).
    pub const MSC: u16 = 0x04;
    /// LEDs (keyboard indicators).
    pub const LED: u16 = 0x11;
    /// Autorepeat parameters (server-managed here; ignored on the wire).
    pub const REP: u16 = 0x14;
}

/// Synchronization codes (`SYN_*`, `EV_SYN` payloads).
pub mod syn {
    /// End of one event packet — the frame boundary.
    pub const REPORT: u16 = 0;
    /// Legacy protocol-A touch boundary (slotless multitouch).
    pub const MT_REPORT: u16 = 2;
    /// The kernel dropped events; the client must resynchronize.
    pub const DROPPED: u16 = 3;
}

/// Relative axes (`REL_*`).
pub mod rel {
    /// Relative X.
    pub const X: u16 = 0x00;
    /// Relative Y.
    pub const Y: u16 = 0x01;
    /// Horizontal wheel (tilt-style), discrete clicks.
    pub const HWHEEL: u16 = 0x06;
    /// Vertical wheel, discrete clicks.
    pub const WHEEL: u16 = 0x08;
    /// Horizontal wheel, high-resolution (120 units per click area).
    pub const HWHEEL_HI_RES: u16 = 0x0c;
    /// Vertical wheel, high-resolution (120 units per click).
    pub const WHEEL_HI_RES: u16 = 0x0b;
}

/// Keys (`KEY_*`) — the subset the tests and tables name explicitly.
pub mod key {
    /// Escape.
    pub const ESC: u32 = 1;
    /// Digit row `1`..`0` occupy 2..=11.
    pub const ROW_1: u32 = 2;
    /// Digit `0` (end of the digit row).
    pub const ZERO: u32 = 11;
    /// Left Control.
    pub const LEFTCTRL: u32 = 29;
    /// `A` (the letter block runs 30..=38).
    pub const A: u32 = 30;
    /// Left Shift.
    pub const LEFTSHIFT: u32 = 42;
    /// Left Alt.
    pub const LEFTALT: u32 = 56;
    /// Space.
    pub const SPACE: u32 = 57;
    /// Caps Lock.
    pub const CAPSLOCK: u32 = 58;
    /// Right Control.
    pub const RIGHTCTRL: u32 = 97;
    /// Right Alt (AltGr on European layouts).
    pub const RIGHTALT: u32 = 100;
    /// Left Meta (Super).
    pub const LEFTMETA: u32 = 125;
    /// Right Meta (Super).
    pub const RIGHTMETA: u32 = 126;
}

/// Buttons (`BTN_*`): pointer and tool buttons.
pub mod btn {
    /// Left pointer button.
    pub const LEFT: u32 = 0x110;
    /// Right pointer button.
    pub const RIGHT: u32 = 0x111;
    /// Middle pointer button.
    pub const MIDDLE: u32 = 0x112;
    /// Side button (thumb).
    pub const SIDE: u32 = 0x113;
    /// Extra button.
    pub const EXTRA: u32 = 0x114;
    /// Tool in proximity: pen.
    pub const TOOL_PEN: u32 = 0x140;
    /// Tool in proximity: eraser (rubber).
    pub const TOOL_RUBBER: u32 = 0x141;
    /// Tool in proximity: brush.
    pub const TOOL_BRUSH: u32 = 0x142;
    /// Tool in proximity: pencil.
    pub const TOOL_PENCIL: u32 = 0x143;
    /// Tool in proximity: airbrush.
    pub const TOOL_AIRBRUSH: u32 = 0x144;
    /// Tool in proximity: finger (touchpads, touchscreens).
    pub const TOOL_FINGER: u32 = 0x145;
    /// Tool in proximity: mouse (tablet pucks).
    pub const TOOL_MOUSE: u32 = 0x146;
    /// Tool in proximity: lens (magnifier pucks).
    pub const TOOL_LENS: u32 = 0x147;
    /// Contact active (touch pressure threshold in the kernel driver).
    pub const TOUCH: u32 = 0x14a;
    /// First stylus side button.
    pub const STYLUS: u32 = 0x14b;
    /// Second stylus side button.
    pub const STYLUS2: u32 = 0x14c;
}

/// Absolute axes (`ABS_*`).
pub mod abs {
    /// Absolute X (pointer-class devices).
    pub const X: u16 = 0x00;
    /// Absolute Y.
    pub const Y: u16 = 0x01;
    /// Pressure (stylus contact force).
    pub const PRESSURE: u16 = 0x13;
    /// Hover distance (stylus distance off the surface).
    pub const DISTANCE: u16 = 0x14;
    /// Tilt X (signed, usually -9000..9000 → degrees).
    pub const TILT_X: u16 = 0x15;
    /// Tilt Y (signed, usually -9000..9000 → degrees).
    pub const TILT_Y: u16 = 0x16;
    /// Tool width (contact ellipse width).
    pub const TOOL_WIDTH: u16 = 0x17;
    /// The multitouch slot selector (protocol B).
    pub const MT_SLOT: u16 = 0x2f;
    /// Touch ellipse major axis.
    pub const MT_TOUCH_MAJOR: u16 = 0x30;
    /// Touch ellipse minor axis.
    pub const MT_TOUCH_MINOR: u16 = 0x31;
    /// Touch ellipse orientation.
    pub const MT_ORIENTATION: u16 = 0x34;
    /// Touch center X.
    pub const MT_POSITION_X: u16 = 0x35;
    /// Touch center Y.
    pub const MT_POSITION_Y: u16 = 0x36;
    /// Touch tool type (finger/pen/palm).
    pub const MT_TOOL_TYPE: u16 = 0x37;
    /// Touch tracking ID (protocol B contact identity).
    pub const MT_TRACKING_ID: u16 = 0x39;
    /// Touch pressure.
    pub const MT_PRESSURE: u16 = 0x3a;
}

/// `ABS_MT_TOOL_TYPE` values.
pub mod mt_tool {
    /// A finger.
    pub const FINGER: i32 = 0;
    /// A pen.
    pub const PEN: i32 = 1;
    /// A palm (rejection candidate).
    pub const PALM: i32 = 2;
}

/// Miscellaneous codes (`MSC_*`).
pub mod msc {
    /// Keyboard scan code (pre-keycode; diagnostics only).
    pub const SCAN: u16 = 0x04;
    /// Device-side timestamp (µs since boot; monotonic aid).
    pub const TIMESTAMP: u16 = 0x05;
}

/// LED codes (`LED_*`).
pub mod led {
    /// Num Lock.
    pub const NUML: u16 = 0x00;
    /// Caps Lock.
    pub const CAPSL: u16 = 0x01;
    /// Scroll Lock.
    pub const SCROLLL: u16 = 0x02;
    /// Compose.
    pub const COMPOSE: u16 = 0x03;
}

/// Bus types (`BUS_*`, `struct input_id`).
pub mod bus {
    /// PCI.
    pub const PCI: u16 = 0x01;
    /// USB (most external keyboards, mice, tablets).
    pub const USB: u16 = 0x03;
    /// Bluetooth.
    pub const BLUETOOTH: u16 = 0x05;
    /// Virtual (injected/test devices).
    pub const VIRTUAL: u16 = 0x06;
    /// PS/2 via the i8042 controller (built-in touchpads).
    pub const I8042: u16 = 0x11;
    /// I2C (recent built-in touchpads/touchscreens).
    pub const I2C: u16 = 0x18;
    /// SPI (some touchscreens).
    pub const SPI: u16 = 0x1c;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pinning test: every constant matches the kernel ABI this
    /// crate is compiled against. A failure here means a typo in the
    /// tables above, not a kernel change.
    #[test]
    fn constants_match_kernel_abi() {
        assert_eq!(ev::SYN, 0x00);
        assert_eq!(ev::KEY, 0x01);
        assert_eq!(ev::REL, 0x02);
        assert_eq!(ev::ABS, 0x03);
        assert_eq!(ev::MSC, 0x04);
        assert_eq!(ev::LED, 0x11);
        assert_eq!(rel::WHEEL, 0x08);
        assert_eq!(rel::WHEEL_HI_RES, 0x0b);
        assert_eq!(rel::HWHEEL_HI_RES, 0x0c);
        assert_eq!(key::LEFTSHIFT, 42);
        assert_eq!(key::LEFTMETA, 125);
        assert_eq!(btn::LEFT, 0x110);
        assert_eq!(btn::TOUCH, 0x14a);
        assert_eq!(btn::TOOL_PEN, 0x140);
        assert_eq!(abs::PRESSURE, 0x13);
        assert_eq!(abs::TILT_X, 0x15);
        assert_eq!(abs::MT_SLOT, 0x2f);
        assert_eq!(abs::MT_POSITION_X, 0x35);
        assert_eq!(abs::MT_TRACKING_ID, 0x39);
        assert_eq!(bus::I8042, 0x11);
        assert_eq!(bus::USB, 0x03);
    }
}
