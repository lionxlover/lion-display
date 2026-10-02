//! The pinned protocol subset: interface and message schemas.
//!
//! This is the bridge's hand-written Wayland subset — the interfaces a
//! weston-terminal-class client needs, frozen as data: `wl_display`,
//! `wl_registry`, `wl_callback`, `wl_shm` (+ pool/buffer), `wl_surface`,
//! `wl_region`, `wl_seat` (+ pointer/keyboard/touch), and the whole
//! `xdg-shell` family (`xdg_wm_base`, `xdg_positioner`, `xdg_surface`,
//! `xdg_toplevel`, `xdg_popup`).
//!
//! Opcodes are contiguous from 0 in declaration order (the Wayland
//! rule); every table here is byte-pinned by the wire golden tests.
//! Versions: the subset advertises and speaks version 1 of each
//! interface except `wl_shm`/`wl_surface`/`wl_seat`... which also pin
//! at the features version 1 carries — the bridge speaks one dialect,
//! deliberately: no version negotiation surprises, `version` fields
//! are validated as exactly the advertised number.

#![forbid(unsafe_code)]

use crate::wire::{Arg, MessageSchema};

/// One interface's schema: name, server-advertised version, requests,
/// events.
#[derive(Clone, Debug)]
pub struct Interface {
    /// The interface name (wire-exact).
    pub name: &'static str,
    /// The version the bridge advertises and speaks.
    pub version: u32,
    /// Request schemas, opcode order.
    pub requests: &'static [MessageSchema],
    /// Event schemas, opcode order.
    pub events: &'static [MessageSchema],
}

/// Look up a request schema.
#[must_use]
pub fn request(iface: &Interface, opcode: u32) -> Option<&'static MessageSchema> {
    iface.requests.iter().find(|m| m.opcode == opcode)
}

/// Look up an event schema.
#[must_use]
pub fn event(iface: &Interface, opcode: u32) -> Option<&'static MessageSchema> {
    iface.events.iter().find(|m| m.opcode == opcode)
}

const fn msg(opcode: u32, name: &'static str, args: &'static [Arg]) -> MessageSchema {
    MessageSchema { opcode, name, args }
}

use Arg::{Array, Fd, Fixed, Int, NewId, Object, String as Str, Uint};

/// `wl_display` — the bootstrap object, always id 1.
pub static WL_DISPLAY: Interface = Interface {
    name: "wl_display",
    version: 1,
    requests: &[msg(0, "sync", &[NewId]), msg(1, "get_registry", &[NewId])],
    events: &[
        msg(0, "error", &[Object, Uint, Str]),
        msg(1, "delete_id", &[Uint]),
    ],
};

/// `wl_registry` — the global table.
pub static WL_REGISTRY: Interface = Interface {
    name: "wl_registry",
    version: 1,
    requests: &[msg(0, "bind", &[Uint, Str, Uint, NewId])],
    events: &[
        msg(0, "global", &[Uint, Str, Uint]),
        msg(1, "global_remove", &[Uint]),
    ],
};

/// `wl_callback` — one-shot frame/sync completion.
pub static WL_CALLBACK: Interface = Interface {
    name: "wl_callback",
    version: 1,
    requests: &[],
    events: &[msg(0, "done", &[Uint])],
};

/// `wl_compositor` — the surface factory.
pub static WL_COMPOSITOR: Interface = Interface {
    name: "wl_compositor",
    version: 1,
    requests: &[
        msg(0, "create_surface", &[NewId]),
        msg(1, "create_region", &[NewId]),
    ],
    events: &[],
};

/// `wl_shm` — the shared-memory factory.
pub static WL_SHM: Interface = Interface {
    name: "wl_shm",
    version: 1,
    requests: &[msg(0, "create_pool", &[Fd, Int, NewId])],
    events: &[msg(0, "format", &[Uint])],
};

/// `wl_shm_pool` — one mapped segment.
pub static WL_SHM_POOL: Interface = Interface {
    name: "wl_shm_pool",
    version: 1,
    requests: &[
        msg(0, "destroy", &[]),
        msg(1, "create_buffer", &[NewId, Int, Int, Int, Int, Uint]),
        msg(2, "resize", &[Int]),
    ],
    events: &[],
};

/// `wl_buffer` — one buffer carved from a pool.
pub static WL_BUFFER: Interface = Interface {
    name: "wl_buffer",
    version: 1,
    requests: &[msg(0, "destroy", &[])],
    events: &[msg(0, "release", &[])],
};

/// `wl_surface` — the double-buffered drawing target.
pub static WL_SURFACE: Interface = Interface {
    name: "wl_surface",
    version: 1,
    requests: &[
        msg(0, "destroy", &[]),
        msg(1, "attach", &[Object, Int, Int]),
        msg(2, "damage", &[Int, Int, Int, Int]),
        msg(3, "frame", &[NewId]),
        msg(4, "set_opaque_region", &[Object]),
        msg(5, "set_input_region", &[Object]),
        msg(6, "commit", &[]),
        msg(7, "damage_buffer", &[Int, Int, Int, Int]),
    ],
    events: &[msg(0, "enter", &[Object]), msg(1, "leave", &[Object])],
};

/// `wl_region` — a rectangle union.
pub static WL_REGION: Interface = Interface {
    name: "wl_region",
    version: 1,
    requests: &[
        msg(0, "destroy", &[]),
        msg(1, "add", &[Int, Int, Int, Int]),
        msg(2, "subtract", &[Int, Int, Int, Int]),
    ],
    events: &[],
};

/// `wl_seat` — the input device group (the frame-grouping dialect:
/// version 5 seat/pointer/keyboard).
pub static WL_SEAT: Interface = Interface {
    name: "wl_seat",
    version: 5,
    requests: &[
        msg(0, "get_pointer", &[NewId]),
        msg(1, "get_keyboard", &[NewId]),
        msg(2, "get_touch", &[NewId]),
        msg(3, "release", &[]),
    ],
    events: &[msg(0, "capabilities", &[Uint]), msg(1, "name", &[Str])],
};

/// `wl_pointer` — the pointer device.
pub static WL_POINTER: Interface = Interface {
    name: "wl_pointer",
    version: 5,
    requests: &[
        msg(0, "set_cursor", &[Uint, Object, Int, Int]),
        msg(1, "release", &[]),
    ],
    events: &[
        msg(0, "enter", &[Uint, Object, Fixed, Fixed]),
        msg(1, "leave", &[Uint, Object]),
        msg(2, "motion", &[Uint, Fixed, Fixed]),
        msg(3, "button", &[Uint, Uint, Uint, Uint]),
        msg(4, "axis", &[Uint, Uint, Fixed]),
        msg(5, "frame", &[]),
    ],
};

/// `wl_keyboard` — the keyboard device.
pub static WL_KEYBOARD: Interface = Interface {
    name: "wl_keyboard",
    version: 5,
    requests: &[msg(0, "release", &[])],
    events: &[
        msg(0, "keymap", &[Fd, Uint, Uint]),
        msg(1, "enter", &[Uint, Object, Array]),
        msg(2, "leave", &[Uint, Object]),
        msg(3, "key", &[Uint, Uint, Uint, Uint]),
        msg(4, "modifiers", &[Uint, Uint, Uint, Uint, Uint, Uint]),
        msg(5, "repeat_info", &[Int, Int]),
    ],
};

/// `wl_touch` — the touch device (the frame skeleton).
pub static WL_TOUCH: Interface = Interface {
    name: "wl_touch",
    version: 5,
    requests: &[msg(0, "release", &[])],
    events: &[
        msg(0, "down", &[Uint, Uint, Object, Int, Fixed, Fixed]),
        msg(1, "up", &[Uint, Uint, Int]),
        msg(2, "motion", &[Uint, Int, Fixed, Fixed]),
        msg(3, "frame", &[]),
        msg(4, "cancel", &[]),
    ],
};

/// `xdg_wm_base` — the shell factory.
pub static XDG_WM_BASE: Interface = Interface {
    name: "xdg_wm_base",
    version: 1,
    requests: &[
        msg(0, "destroy", &[]),
        msg(1, "create_positioner", &[NewId]),
        msg(2, "get_xdg_surface", &[NewId, Object]),
        msg(3, "pong", &[Uint]),
    ],
    events: &[msg(0, "ping", &[Uint])],
};

/// `xdg_positioner` — the popup placement rules.
pub static XDG_POSITIONER: Interface = Interface {
    name: "xdg_positioner",
    version: 1,
    requests: &[
        msg(0, "destroy", &[]),
        msg(1, "set_size", &[Int, Int]),
        msg(2, "set_anchor_rect", &[Int, Int, Int, Int]),
        msg(3, "set_anchor", &[Uint]),
        msg(4, "set_gravity", &[Uint]),
        msg(5, "set_constraint_adjustment", &[Uint]),
        msg(6, "set_offset", &[Int, Int]),
    ],
    events: &[],
};

/// `xdg_surface` — a surface's shell role glue.
pub static XDG_SURFACE: Interface = Interface {
    name: "xdg_surface",
    version: 1,
    requests: &[
        msg(0, "destroy", &[]),
        msg(1, "get_toplevel", &[NewId]),
        msg(2, "get_popup", &[NewId, Object, Object]),
        msg(3, "set_window_geometry", &[Int, Int, Int, Int]),
        msg(4, "ack_configure", &[Uint]),
    ],
    events: &[msg(0, "configure", &[Uint])],
};

/// `xdg_toplevel` — a top-level window.
pub static XDG_TOPLEVEL: Interface = Interface {
    name: "xdg_toplevel",
    version: 1,
    requests: &[
        msg(0, "destroy", &[]),
        msg(1, "set_parent", &[Object]),
        msg(2, "set_title", &[Str]),
        msg(3, "set_app_id", &[Str]),
        msg(4, "show_window_menu", &[Uint, Int, Int]),
        msg(5, "move", &[Uint, Int]),
        msg(6, "resize", &[Uint, Uint, Int]),
        msg(7, "set_max_size", &[Int, Int]),
        msg(8, "set_min_size", &[Int, Int]),
        msg(9, "set_maximized", &[]),
        msg(10, "unset_maximized", &[]),
        msg(11, "set_fullscreen", &[Object]),
        msg(12, "unset_fullscreen", &[]),
        msg(13, "set_minimized", &[]),
    ],
    events: &[
        msg(0, "configure", &[Int, Int, Array]),
        msg(1, "close", &[]),
        msg(2, "configure_bounds", &[Int, Int]),
    ],
};

/// `xdg_popup` — a grab-scoped child surface.
pub static XDG_POPUP: Interface = Interface {
    name: "xdg_popup",
    version: 1,
    requests: &[
        msg(0, "destroy", &[]),
        msg(1, "grab", &[Object, Uint]),
        msg(2, "reposition", &[Object, Uint]),
    ],
    events: &[
        msg(0, "configure", &[Int, Int, Int, Int]),
        msg(1, "popup_done", &[]),
        msg(2, "repositioned", &[Uint]),
    ],
};

/// The globals the bridge advertises, in registry-replay order. The
/// registry names run 1..=4 (replayed in this order and pinned
/// byte-exact by the wire tests).
pub static ALL_GLOBALS: &[&Interface] = &[&WL_COMPOSITOR, &WL_SHM, &WL_SEAT, &XDG_WM_BASE];

/// Every interface in the subset (event-schema lookup for tooling).
pub static ALL_INTERFACES: &[&Interface] = &[
    &WL_DISPLAY,
    &WL_REGISTRY,
    &WL_CALLBACK,
    &WL_COMPOSITOR,
    &WL_SHM,
    &WL_SHM_POOL,
    &WL_BUFFER,
    &WL_SURFACE,
    &WL_REGION,
    &WL_SEAT,
    &WL_POINTER,
    &WL_KEYBOARD,
    &WL_TOUCH,
    &XDG_WM_BASE,
    &XDG_POSITIONER,
    &XDG_SURFACE,
    &XDG_TOPLEVEL,
    &XDG_POPUP,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shm_pool_schema_lookup() {
        assert_eq!(request(&WL_SHM_POOL, 0).unwrap().name, "destroy");
        assert_eq!(request(&WL_SHM_POOL, 1).unwrap().name, "create_buffer");
        assert_eq!(request(&WL_SHM_POOL, 1).unwrap().args.len(), 6);
        assert_eq!(request(&WL_SHM_POOL, 2).unwrap().name, "resize");
    }
}
