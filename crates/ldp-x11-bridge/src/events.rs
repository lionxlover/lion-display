//! Core event envelopes (server to client), in wire order.
//!
//! Every event is exactly 32 bytes. The core form carries the low 16
//! bits of the request sequence number in its header
//! ([`event_prefix`]); the trailing bytes are unused padding
//! ([`event_tail`]). KeymapNotify is the one exception — no sequence,
//! its 31 key bytes run to the end of the envelope.
//!
//! Only the events the Phase 17 subset generates are modeled:
//! input (key/button/motion/enter/leave/focus), exposure, structure
//! notify, property and selection lifecycles, ClientMessage and
//! NoExpose/GraphicsExpose. GravityNotify and ResizeRequest never
//! occur (the bridge is the window manager: it configures, never
//! redirects), and MappingNotify never occurs (the keymap is frozen at
//! the Phase 17 subset table).

#![forbid(unsafe_code)]

use crate::wire::{event_prefix, event_tail, Endian};

/// Core event codes (the first byte of the envelope).
#[allow(missing_docs)] // the names are the protocol's
pub mod code {
    pub const KEY_PRESS: u8 = 2;
    pub const KEY_RELEASE: u8 = 3;
    pub const BUTTON_PRESS: u8 = 4;
    pub const BUTTON_RELEASE: u8 = 5;
    pub const MOTION_NOTIFY: u8 = 6;
    pub const ENTER_NOTIFY: u8 = 7;
    pub const LEAVE_NOTIFY: u8 = 8;
    pub const FOCUS_IN: u8 = 9;
    pub const FOCUS_OUT: u8 = 10;
    pub const KEYMAP_NOTIFY: u8 = 11;
    pub const EXPOSE: u8 = 12;
    pub const GRAPHICS_EXPOSE: u8 = 13;
    pub const NO_EXPOSE: u8 = 14;
    pub const VISIBILITY_NOTIFY: u8 = 15;
    pub const CREATE_NOTIFY: u8 = 16;
    pub const DESTROY_NOTIFY: u8 = 17;
    pub const UNMAP_NOTIFY: u8 = 18;
    pub const MAP_NOTIFY: u8 = 19;
    pub const REPARENT_NOTIFY: u8 = 20;
    pub const CONFIGURE_NOTIFY: u8 = 22;
    pub const CIRCULATE_NOTIFY: u8 = 25;
    pub const PROPERTY_NOTIFY: u8 = 27;
    pub const SELECTION_CLEAR: u8 = 28;
    pub const SELECTION_REQUEST: u8 = 29;
    pub const SELECTION_NOTIFY: u8 = 30;
    pub const CLIENT_MESSAGE: u8 = 32;
}

/// Event-mask bits (the `event-mask` attribute and SelectInput).
#[allow(missing_docs)] // the names are the protocol's
pub mod mask {
    pub const KEY_PRESS: u32 = 1 << 0;
    pub const KEY_RELEASE: u32 = 1 << 1;
    pub const BUTTON_PRESS: u32 = 1 << 2;
    pub const BUTTON_RELEASE: u32 = 1 << 3;
    pub const ENTER_WINDOW: u32 = 1 << 4;
    pub const LEAVE_WINDOW: u32 = 1 << 5;
    pub const POINTER_MOTION: u32 = 1 << 6;
    pub const POINTER_MOTION_HINT: u32 = 1 << 7;
    pub const BUTTON_MOTION: u32 = 1 << 13;
    pub const KEYMAP_STATE: u32 = 1 << 14;
    pub const EXPOSURE: u32 = 1 << 15;
    pub const VISIBILITY_CHANGE: u32 = 1 << 16;
    pub const STRUCTURE_NOTIFY: u32 = 1 << 17;
    pub const SUBSTRUCTURE_NOTIFY: u32 = 1 << 19;
    pub const FOCUS_CHANGE: u32 = 1 << 21;
    pub const PROPERTY_CHANGE: u32 = 1 << 22;
}

/// Error codes (the error envelope's second byte).
#[allow(missing_docs)] // the names are the protocol's
pub mod err {
    pub const REQUEST: u8 = 1;
    pub const VALUE: u8 = 2;
    pub const WINDOW: u8 = 3;
    pub const PIXMAP: u8 = 4;
    pub const ATOM: u8 = 5;
    pub const CURSOR: u8 = 6;
    pub const FONT: u8 = 7;
    pub const MATCH: u8 = 8;
    pub const DRAWABLE: u8 = 9;
    pub const ACCESS: u8 = 10;
    pub const ALLOC: u8 = 11;
    pub const COLOR: u8 = 12;
    pub const GC: u8 = 13;
    pub const ID_CHOICE: u8 = 14;
    pub const NAME: u8 = 15;
    pub const LENGTH: u8 = 16;
    pub const IMPLEMENTATION: u8 = 17;
}

/// One core event, in field order, ready to encode.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(missing_docs)] // field names restate the wire; doc comments
                       // would duplicate the protocol table.
pub enum XEvent {
    KeyPress {
        keycode: u8,
        time: u32,
        root: u32,
        event: u32,
        root_x: i16,
        root_y: i16,
        event_x: i16,
        event_y: i16,
        state: u16,
    },
    KeyRelease {
        keycode: u8,
        time: u32,
        root: u32,
        event: u32,
        root_x: i16,
        root_y: i16,
        event_x: i16,
        event_y: i16,
        state: u16,
    },
    ButtonPress {
        button: u8,
        time: u32,
        root: u32,
        event: u32,
        root_x: i16,
        root_y: i16,
        event_x: i16,
        event_y: i16,
        state: u16,
    },
    ButtonRelease {
        button: u8,
        time: u32,
        root: u32,
        event: u32,
        root_x: i16,
        root_y: i16,
        event_x: i16,
        event_y: i16,
        state: u16,
    },
    MotionNotify {
        detail: u8,
        time: u32,
        root: u32,
        event: u32,
        root_x: i16,
        root_y: i16,
        event_x: i16,
        event_y: i16,
        state: u16,
    },
    EnterNotify {
        detail: u8,
        time: u32,
        root: u32,
        event: u32,
        child: u32,
        root_x: i16,
        root_y: i16,
        event_x: i16,
        event_y: i16,
        state: u16,
        mode: u8,
    },
    LeaveNotify {
        detail: u8,
        time: u32,
        root: u32,
        event: u32,
        child: u32,
        root_x: i16,
        root_y: i16,
        event_x: i16,
        event_y: i16,
        state: u16,
        mode: u8,
    },
    FocusIn {
        event: u32,
        mode: u8,
    },
    FocusOut {
        event: u32,
        mode: u8,
    },
    KeymapNotify {
        keys: [u8; 31],
    },
    Expose {
        window: u32,
        x: u16,
        y: u16,
        width: u16,
        height: u16,
        count: u16,
    },
    GraphicsExpose {
        drawable: u32,
        x: u16,
        y: u16,
        width: u16,
        height: u16,
        minor: u16,
        count: u16,
        major: u8,
    },
    NoExpose {
        drawable: u32,
        minor: u16,
        major: u8,
    },
    VisibilityNotify {
        window: u32,
        state: u8,
    },
    CreateNotify {
        parent: u32,
        window: u32,
        x: i16,
        y: i16,
        width: u16,
        height: u16,
        border_width: u16,
        override_redirect: bool,
    },
    DestroyNotify {
        event: u32,
        window: u32,
    },
    UnmapNotify {
        event: u32,
        window: u32,
        from_configure: bool,
    },
    MapNotify {
        event: u32,
        window: u32,
        override_redirect: bool,
    },
    ConfigureNotify {
        event: u32,
        window: u32,
        above_sibling: u32,
        x: i16,
        y: i16,
        width: u16,
        height: u16,
        border_width: u16,
        override_redirect: bool,
    },
    CirculateNotify {
        event: u32,
        window: u32,
        place: u8,
    },
    PropertyNotify {
        window: u32,
        atom: u32,
        time: u32,
        deleted: bool,
    },
    SelectionClear {
        time: u32,
        owner: u32,
        selection: u32,
    },
    SelectionRequest {
        time: u32,
        owner: u32,
        requestor: u32,
        selection: u32,
        target: u32,
        property: u32,
    },
    SelectionNotify {
        time: u32,
        requestor: u32,
        selection: u32,
        target: u32,
        property: u32,
    },
    ClientMessage {
        format: u8,
        window: u32,
        type_atom: u32,
        data: [u8; 20],
    },
}

impl XEvent {
    /// The core event code.
    #[must_use]
    pub const fn event_code(&self) -> u8 {
        match self {
            XEvent::KeyPress { .. } => code::KEY_PRESS,
            XEvent::KeyRelease { .. } => code::KEY_RELEASE,
            XEvent::ButtonPress { .. } => code::BUTTON_PRESS,
            XEvent::ButtonRelease { .. } => code::BUTTON_RELEASE,
            XEvent::MotionNotify { .. } => code::MOTION_NOTIFY,
            XEvent::EnterNotify { .. } => code::ENTER_NOTIFY,
            XEvent::LeaveNotify { .. } => code::LEAVE_NOTIFY,
            XEvent::FocusIn { .. } => code::FOCUS_IN,
            XEvent::FocusOut { .. } => code::FOCUS_OUT,
            XEvent::KeymapNotify { .. } => code::KEYMAP_NOTIFY,
            XEvent::Expose { .. } => code::EXPOSE,
            XEvent::GraphicsExpose { .. } => code::GRAPHICS_EXPOSE,
            XEvent::NoExpose { .. } => code::NO_EXPOSE,
            XEvent::VisibilityNotify { .. } => code::VISIBILITY_NOTIFY,
            XEvent::CreateNotify { .. } => code::CREATE_NOTIFY,
            XEvent::DestroyNotify { .. } => code::DESTROY_NOTIFY,
            XEvent::UnmapNotify { .. } => code::UNMAP_NOTIFY,
            XEvent::MapNotify { .. } => code::MAP_NOTIFY,
            XEvent::ConfigureNotify { .. } => code::CONFIGURE_NOTIFY,
            XEvent::CirculateNotify { .. } => code::CIRCULATE_NOTIFY,
            XEvent::PropertyNotify { .. } => code::PROPERTY_NOTIFY,
            XEvent::SelectionClear { .. } => code::SELECTION_CLEAR,
            XEvent::SelectionRequest { .. } => code::SELECTION_REQUEST,
            XEvent::SelectionNotify { .. } => code::SELECTION_NOTIFY,
            XEvent::ClientMessage { .. } => code::CLIENT_MESSAGE,
        }
    }

    /// Encode the 32-byte envelope in the connection's byte order.
    ///
    /// One flat wire encoder over the whole core event vocabulary —
    /// the length is the price of exhaustiveness.
    #[allow(clippy::too_many_lines)]
    #[must_use]
    pub fn encode(&self, endian: Endian, seq: u32) -> Vec<u8> {
        let mut out = Vec::with_capacity(32);
        match *self {
            XEvent::KeyPress {
                keycode,
                time,
                root,
                event,
                root_x,
                root_y,
                event_x,
                event_y,
                state,
            } => {
                event_prefix(&mut out, endian, code::KEY_PRESS, keycode, seq);
                push_input_body(
                    &mut out, endian, time, root, event, 0, root_x, root_y, event_x, event_y,
                    state, 1,
                );
            }
            XEvent::KeyRelease {
                keycode,
                time,
                root,
                event,
                root_x,
                root_y,
                event_x,
                event_y,
                state,
            } => {
                event_prefix(&mut out, endian, code::KEY_RELEASE, keycode, seq);
                push_input_body(
                    &mut out, endian, time, root, event, 0, root_x, root_y, event_x, event_y,
                    state, 1,
                );
            }
            XEvent::ButtonPress {
                button,
                time,
                root,
                event,
                root_x,
                root_y,
                event_x,
                event_y,
                state,
            } => {
                event_prefix(&mut out, endian, code::BUTTON_PRESS, button, seq);
                push_input_body(
                    &mut out, endian, time, root, event, 0, root_x, root_y, event_x, event_y,
                    state, 1,
                );
            }
            XEvent::ButtonRelease {
                button,
                time,
                root,
                event,
                root_x,
                root_y,
                event_x,
                event_y,
                state,
            } => {
                event_prefix(&mut out, endian, code::BUTTON_RELEASE, button, seq);
                push_input_body(
                    &mut out, endian, time, root, event, 0, root_x, root_y, event_x, event_y,
                    state, 1,
                );
            }
            XEvent::MotionNotify {
                detail,
                time,
                root,
                event,
                root_x,
                root_y,
                event_x,
                event_y,
                state,
            } => {
                event_prefix(&mut out, endian, code::MOTION_NOTIFY, detail, seq);
                push_input_body(
                    &mut out, endian, time, root, event, 0, root_x, root_y, event_x, event_y,
                    state, 1,
                );
            }
            XEvent::EnterNotify {
                detail,
                time,
                root,
                event,
                child,
                root_x,
                root_y,
                event_x,
                event_y,
                state,
                mode,
            } => {
                event_prefix(&mut out, endian, code::ENTER_NOTIFY, detail, seq);
                push_input_body(
                    &mut out, endian, time, root, event, child, root_x, root_y, event_x, event_y,
                    state, mode,
                );
            }
            XEvent::LeaveNotify {
                detail,
                time,
                root,
                event,
                child,
                root_x,
                root_y,
                event_x,
                event_y,
                state,
                mode,
            } => {
                event_prefix(&mut out, endian, code::LEAVE_NOTIFY, detail, seq);
                push_input_body(
                    &mut out, endian, time, root, event, child, root_x, root_y, event_x, event_y,
                    state, mode,
                );
            }
            XEvent::FocusIn { event, mode } => {
                event_prefix(&mut out, endian, code::FOCUS_IN, 0, seq);
                out.extend_from_slice(&endian.put_u32(event));
                out.push(mode);
                event_tail(&mut out);
            }
            XEvent::FocusOut { event, mode } => {
                event_prefix(&mut out, endian, code::FOCUS_OUT, 0, seq);
                out.extend_from_slice(&endian.put_u32(event));
                out.push(mode);
                event_tail(&mut out);
            }
            XEvent::KeymapNotify { keys } => {
                // No sequence: byte 0 is the code, then 31 key bytes.
                out.push(code::KEYMAP_NOTIFY);
                out.extend_from_slice(&keys);
            }
            XEvent::Expose {
                window,
                x,
                y,
                width,
                height,
                count,
            } => {
                event_prefix(&mut out, endian, code::EXPOSE, 0, seq);
                out.extend_from_slice(&endian.put_u32(window));
                out.extend_from_slice(&endian.put_u16(x));
                out.extend_from_slice(&endian.put_u16(y));
                out.extend_from_slice(&endian.put_u16(width));
                out.extend_from_slice(&endian.put_u16(height));
                out.extend_from_slice(&endian.put_u16(count));
                event_tail(&mut out);
            }
            XEvent::GraphicsExpose {
                drawable,
                x,
                y,
                width,
                height,
                minor,
                count,
                major,
            } => {
                event_prefix(&mut out, endian, code::GRAPHICS_EXPOSE, 0, seq);
                out.extend_from_slice(&endian.put_u32(drawable));
                out.extend_from_slice(&endian.put_u16(x));
                out.extend_from_slice(&endian.put_u16(y));
                out.extend_from_slice(&endian.put_u16(width));
                out.extend_from_slice(&endian.put_u16(height));
                out.extend_from_slice(&endian.put_u16(minor));
                out.extend_from_slice(&endian.put_u16(count));
                out.push(major);
                event_tail(&mut out);
            }
            XEvent::NoExpose {
                drawable,
                minor,
                major,
            } => {
                event_prefix(&mut out, endian, code::NO_EXPOSE, 0, seq);
                out.extend_from_slice(&endian.put_u32(drawable));
                out.extend_from_slice(&endian.put_u16(minor));
                out.push(major);
                event_tail(&mut out);
            }
            XEvent::VisibilityNotify { window, state } => {
                event_prefix(&mut out, endian, code::VISIBILITY_NOTIFY, 0, seq);
                out.extend_from_slice(&endian.put_u32(window));
                out.push(state);
                event_tail(&mut out);
            }
            XEvent::CreateNotify {
                parent,
                window,
                x,
                y,
                width,
                height,
                border_width,
                override_redirect,
            } => {
                event_prefix(&mut out, endian, code::CREATE_NOTIFY, 0, seq);
                out.extend_from_slice(&endian.put_u32(parent));
                out.extend_from_slice(&endian.put_u32(window));
                out.extend_from_slice(&endian.put_u16(x as u16));
                out.extend_from_slice(&endian.put_u16(y as u16));
                out.extend_from_slice(&endian.put_u16(width));
                out.extend_from_slice(&endian.put_u16(height));
                out.extend_from_slice(&endian.put_u16(border_width));
                out.push(u8::from(override_redirect));
                event_tail(&mut out);
            }
            XEvent::DestroyNotify { event, window } => {
                event_prefix(&mut out, endian, code::DESTROY_NOTIFY, 0, seq);
                out.extend_from_slice(&endian.put_u32(event));
                out.extend_from_slice(&endian.put_u32(window));
                event_tail(&mut out);
            }
            XEvent::UnmapNotify {
                event,
                window,
                from_configure,
            } => {
                event_prefix(&mut out, endian, code::UNMAP_NOTIFY, 0, seq);
                out.extend_from_slice(&endian.put_u32(event));
                out.extend_from_slice(&endian.put_u32(window));
                out.push(u8::from(from_configure));
                event_tail(&mut out);
            }
            XEvent::MapNotify {
                event,
                window,
                override_redirect,
            } => {
                event_prefix(&mut out, endian, code::MAP_NOTIFY, 0, seq);
                out.extend_from_slice(&endian.put_u32(event));
                out.extend_from_slice(&endian.put_u32(window));
                out.push(u8::from(override_redirect));
                event_tail(&mut out);
            }
            XEvent::ConfigureNotify {
                event,
                window,
                above_sibling,
                x,
                y,
                width,
                height,
                border_width,
                override_redirect,
            } => {
                event_prefix(&mut out, endian, code::CONFIGURE_NOTIFY, 0, seq);
                out.extend_from_slice(&endian.put_u32(event));
                out.extend_from_slice(&endian.put_u32(window));
                out.extend_from_slice(&endian.put_u32(above_sibling));
                out.extend_from_slice(&endian.put_u16(x as u16));
                out.extend_from_slice(&endian.put_u16(y as u16));
                out.extend_from_slice(&endian.put_u16(width));
                out.extend_from_slice(&endian.put_u16(height));
                out.extend_from_slice(&endian.put_u16(border_width));
                out.push(u8::from(override_redirect));
                event_tail(&mut out);
            }
            XEvent::CirculateNotify {
                event,
                window,
                place,
            } => {
                event_prefix(&mut out, endian, code::CIRCULATE_NOTIFY, 0, seq);
                out.extend_from_slice(&endian.put_u32(event));
                out.extend_from_slice(&endian.put_u32(window));
                out.push(place);
                event_tail(&mut out);
            }
            XEvent::PropertyNotify {
                window,
                atom,
                time,
                deleted,
            } => {
                event_prefix(
                    &mut out,
                    endian,
                    code::PROPERTY_NOTIFY,
                    u8::from(deleted),
                    seq,
                );
                out.extend_from_slice(&endian.put_u32(window));
                out.extend_from_slice(&endian.put_u32(atom));
                out.extend_from_slice(&endian.put_u32(time));
                event_tail(&mut out);
            }
            XEvent::SelectionClear {
                time,
                owner,
                selection,
            } => {
                event_prefix(&mut out, endian, code::SELECTION_CLEAR, 0, seq);
                out.extend_from_slice(&endian.put_u32(time));
                out.extend_from_slice(&endian.put_u32(owner));
                out.extend_from_slice(&endian.put_u32(selection));
                event_tail(&mut out);
            }
            XEvent::SelectionRequest {
                time,
                owner,
                requestor,
                selection,
                target,
                property,
            } => {
                event_prefix(&mut out, endian, code::SELECTION_REQUEST, 0, seq);
                out.extend_from_slice(&endian.put_u32(time));
                out.extend_from_slice(&endian.put_u32(owner));
                out.extend_from_slice(&endian.put_u32(requestor));
                out.extend_from_slice(&endian.put_u32(selection));
                out.extend_from_slice(&endian.put_u32(target));
                out.extend_from_slice(&endian.put_u32(property));
                event_tail(&mut out);
            }
            XEvent::SelectionNotify {
                time,
                requestor,
                selection,
                target,
                property,
            } => {
                event_prefix(&mut out, endian, code::SELECTION_NOTIFY, 0, seq);
                out.extend_from_slice(&endian.put_u32(time));
                out.extend_from_slice(&endian.put_u32(requestor));
                out.extend_from_slice(&endian.put_u32(selection));
                out.extend_from_slice(&endian.put_u32(target));
                out.extend_from_slice(&endian.put_u32(property));
                event_tail(&mut out);
            }
            XEvent::ClientMessage {
                format,
                window,
                type_atom,
                data,
            } => {
                event_prefix(&mut out, endian, code::CLIENT_MESSAGE, format, seq);
                out.extend_from_slice(&endian.put_u32(window));
                out.extend_from_slice(&endian.put_u32(type_atom));
                out.extend_from_slice(&data);
            }
        }
        debug_assert_eq!(out.len(), 32, "event envelope must be 32 bytes");
        out
    }
}

/// The shared input-event body (key/button/motion/enter/leave share
/// one layout: time, root, event, child, 4×i16 coords, state, mode).
#[allow(clippy::too_many_arguments)] // the protocol's flat input-event record
fn push_input_body(
    out: &mut Vec<u8>,
    endian: Endian,
    time: u32,
    root: u32,
    event: u32,
    child: u32,
    root_x: i16,
    root_y: i16,
    event_x: i16,
    event_y: i16,
    state: u16,
    mode: u8,
) {
    out.extend_from_slice(&endian.put_u32(time));
    out.extend_from_slice(&endian.put_u32(root));
    out.extend_from_slice(&endian.put_u32(event));
    out.extend_from_slice(&endian.put_u32(child));
    out.extend_from_slice(&endian.put_u16(root_x as u16));
    out.extend_from_slice(&endian.put_u16(root_y as u16));
    out.extend_from_slice(&endian.put_u16(event_x as u16));
    out.extend_from_slice(&endian.put_u16(event_y as u16));
    out.extend_from_slice(&endian.put_u16(state));
    out.push(mode);
    out.push(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)] // the exhaustive sample corpus
    fn every_event_encodes_to_32_bytes() {
        let samples: Vec<XEvent> = vec![
            XEvent::KeyPress {
                keycode: 24,
                time: 1000,
                root: 0x40,
                event: 0x41,
                root_x: 5,
                root_y: 6,
                event_x: 1,
                event_y: 2,
                state: 0,
            },
            XEvent::ButtonPress {
                button: 1,
                time: 1001,
                root: 0x40,
                event: 0x41,
                root_x: 5,
                root_y: 6,
                event_x: 1,
                event_y: 2,
                state: 0x100,
            },
            XEvent::MotionNotify {
                detail: 0,
                time: 1002,
                root: 0x40,
                event: 0x41,
                root_x: 5,
                root_y: 6,
                event_x: 1,
                event_y: 2,
                state: 0,
            },
            XEvent::EnterNotify {
                detail: 0,
                time: 1003,
                root: 0x40,
                event: 0x41,
                child: 0,
                root_x: 5,
                root_y: 6,
                event_x: 1,
                event_y: 2,
                state: 0,
                mode: 0,
            },
            XEvent::LeaveNotify {
                detail: 2,
                time: 1004,
                root: 0x40,
                event: 0x41,
                child: 0,
                root_x: 5,
                root_y: 6,
                event_x: 1,
                event_y: 2,
                state: 0,
                mode: 0,
            },
            XEvent::FocusIn {
                event: 0x41,
                mode: 0,
            },
            XEvent::FocusOut {
                event: 0x41,
                mode: 0,
            },
            XEvent::KeymapNotify { keys: [0; 31] },
            XEvent::Expose {
                window: 0x41,
                x: 0,
                y: 0,
                width: 320,
                height: 200,
                count: 0,
            },
            XEvent::GraphicsExpose {
                drawable: 0x41,
                x: 0,
                y: 0,
                width: 10,
                height: 10,
                minor: 0,
                count: 0,
                major: 62,
            },
            XEvent::NoExpose {
                drawable: 0x41,
                minor: 0,
                major: 62,
            },
            XEvent::VisibilityNotify {
                window: 0x41,
                state: 0,
            },
            XEvent::CreateNotify {
                parent: 0x40,
                window: 0x41,
                x: 0,
                y: 0,
                width: 100,
                height: 50,
                border_width: 0,
                override_redirect: false,
            },
            XEvent::DestroyNotify {
                event: 0x40,
                window: 0x41,
            },
            XEvent::UnmapNotify {
                event: 0x40,
                window: 0x41,
                from_configure: false,
            },
            XEvent::MapNotify {
                event: 0x40,
                window: 0x41,
                override_redirect: false,
            },
            XEvent::ConfigureNotify {
                event: 0x41,
                window: 0x41,
                above_sibling: 0,
                x: 0,
                y: 0,
                width: 100,
                height: 50,
                border_width: 0,
                override_redirect: false,
            },
            XEvent::CirculateNotify {
                event: 0x40,
                window: 0x41,
                place: 0,
            },
            XEvent::PropertyNotify {
                window: 0x41,
                atom: 39,
                time: 2000,
                deleted: false,
            },
            XEvent::SelectionClear {
                time: 3000,
                owner: 0x41,
                selection: 1,
            },
            XEvent::SelectionRequest {
                time: 3001,
                owner: 0x41,
                requestor: 0x42,
                selection: 1,
                target: 27,
                property: 345,
            },
            XEvent::SelectionNotify {
                time: 3002,
                requestor: 0x42,
                selection: 1,
                target: 27,
                property: 345,
            },
            XEvent::ClientMessage {
                format: 32,
                window: 0x41,
                type_atom: 210,
                data: [0u8; 20],
            },
        ];
        for ev in &samples {
            let lsb = ev.encode(Endian::Lsb, 7);
            let msb = ev.encode(Endian::Msb, 7);
            assert_eq!(lsb.len(), 32, "{ev:?}");
            assert_eq!(msb.len(), 32, "{ev:?}");
            assert_eq!(lsb[0], ev.event_code());
            // The 16-bit sequence rides the header; the tail pads.
            if !matches!(ev, XEvent::KeymapNotify { .. }) {
                assert_eq!(&lsb[2..4], &7u16.to_le_bytes());
                assert_eq!(&msb[2..4], &7u16.to_be_bytes());
            }
        }
    }

    #[test]
    fn expose_layout_is_field_exact() {
        let ev = XEvent::Expose {
            window: 0x4142,
            x: 3,
            y: 4,
            width: 320,
            height: 200,
            count: 0,
        };
        let b = ev.encode(Endian::Lsb, 9);
        assert_eq!(b[0], 12);
        assert_eq!(&b[4..8], &0x4142u32.to_le_bytes());
        assert_eq!(&b[8..10], &3u16.to_le_bytes());
        assert_eq!(&b[10..12], &4u16.to_le_bytes());
        assert_eq!(&b[12..14], &320u16.to_le_bytes());
        assert_eq!(&b[14..16], &200u16.to_le_bytes());
        assert_eq!(&b[16..18], &0u16.to_le_bytes());
    }

    #[test]
    fn keymap_notify_has_no_sequence() {
        let ev = XEvent::KeymapNotify { keys: [7; 31] };
        let b = ev.encode(Endian::Lsb, 99);
        assert_eq!(b[0], 11);
        assert_eq!(&b[1..], &[7u8; 31]);
    }

    #[test]
    fn client_message_carries_the_format_in_detail() {
        let mut data = [0u8; 20];
        data[0..4].copy_from_slice(&0xdead_beefu32.to_le_bytes());
        let ev = XEvent::ClientMessage {
            format: 32,
            window: 5,
            type_atom: 210,
            data,
        };
        let b = ev.encode(Endian::Lsb, 1);
        assert_eq!(b[1], 32);
        assert_eq!(&b[4..8], &5u32.to_le_bytes());
        assert_eq!(&b[8..12], &210u32.to_le_bytes());
        assert_eq!(&b[12..16], &0xdead_beefu32.to_le_bytes());
    }
}
