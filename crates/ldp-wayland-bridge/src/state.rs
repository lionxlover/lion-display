//! Object state for the Wayland subset: surface double-buffering,
//! xdg-shell role machines, positioner vocabulary, and buffers.
//!
//! The `wl_surface` model is the spec's: attach/damage/regions
//! accumulate in a pending state and apply atomically at `commit`.
//! The xdg-shell role machine enforces the configure/ack handshake —
//! `get_toplevel`/`get_popup` before the first buffer commit, one
//! `configure` sent when the role attaches, `ack_configure` echoing
//! the serial before that first commit, and geometry changes only
//! through the two-phase dance afterwards.
//!
//! The positioner is translated into `ldp-shell`'s own placement
//! vocabulary at the driver boundary (one placement authority): the
//! anchor/gravity/constraint bit tables below map one-to-one onto
//! `ldp.protocol.shell`'s `Anchor`, `Gravity`, and
//! `popup_constraints` values — pinned by the popup suite.

#![forbid(unsafe_code)]

use ldp_core::geometry::Rect;

/// xdg positioner anchor values (also the gravity values — the same
/// enum in both roles).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Anchor {
    /// The default (invalid until set; the subset treats unset as
    /// `TopLeft` at solve time but records the unset state).
    #[default]
    Unset,
    /// `none` (0) — only legal for gravity.
    None,
    /// `top_left` (1).
    TopLeft,
    /// `top` (2).
    Top,
    /// `top_right` (3).
    TopRight,
    /// `right` (4).
    Right,
    /// `bottom_right` (5).
    BottomRight,
    /// `bottom` (6).
    Bottom,
    /// `bottom_left` (7).
    BottomLeft,
    /// `left` (8).
    Left,
}

impl Anchor {
    /// Parse a wire value.
    #[must_use]
    pub const fn from_wire(v: u32) -> Anchor {
        match v {
            0 => Anchor::None,
            1 => Anchor::TopLeft,
            2 => Anchor::Top,
            3 => Anchor::TopRight,
            4 => Anchor::Right,
            5 => Anchor::BottomRight,
            6 => Anchor::Bottom,
            7 => Anchor::BottomLeft,
            8 => Anchor::Left,
            _ => Anchor::Unset,
        }
    }

    /// The wire value.
    #[must_use]
    pub const fn to_wire(self) -> u32 {
        match self {
            // Unset and none share the 0 wire word (gravity only).
            Anchor::Unset | Anchor::None => 0,
            Anchor::TopLeft => 1,
            Anchor::Top => 2,
            Anchor::TopRight => 3,
            Anchor::Right => 4,
            Anchor::BottomRight => 5,
            Anchor::Bottom => 6,
            Anchor::BottomLeft => 7,
            Anchor::Left => 8,
        }
    }

    /// The `ldp.protocol.shell` anchor wire value (`ldp-shell`'s own
    /// numbering: top_left 1 … left 8 — identical table).
    #[must_use]
    pub const fn to_ldp_anchor(self) -> Option<u32> {
        match self {
            Anchor::TopLeft => Some(1),
            Anchor::Top => Some(2),
            Anchor::TopRight => Some(3),
            Anchor::Right => Some(4),
            Anchor::BottomRight => Some(5),
            Anchor::Bottom => Some(6),
            Anchor::BottomLeft => Some(7),
            Anchor::Left => Some(8),
            Anchor::None | Anchor::Unset => None,
        }
    }
}

/// xdg constraint-adjustment bits.
pub mod constraints {
    /// Slide the popup along the axis (xdg bit 0).
    pub const SLIDE_X: u32 = 1;
    /// Slide vertically (bit 1).
    pub const SLIDE_Y: u32 = 2;
    /// Flip to the opposite side (bit 2).
    pub const FLIP_X: u32 = 4;
    /// Flip vertically (bit 3).
    pub const FLIP_Y: u32 = 8;
    /// Resize instead of moving (bit 4).
    pub const RESIZE_X: u32 = 16;
    /// Resize vertically (bit 5).
    pub const RESIZE_Y: u32 = 32;
    /// Every bit the subset knows.
    pub const MASK: u32 = 63;
}

/// One xdg positioner (validated state).
#[derive(Clone, Debug)]
pub struct Positioner {
    /// The popup size (must be set before use).
    pub size: Option<(i32, i32)>,
    /// The anchor rectangle within the parent surface.
    pub anchor_rect: Rect,
    /// The anchor edge.
    pub anchor: Anchor,
    /// The popup gravity.
    pub gravity: Anchor,
    /// The constraint adjustment bitmask.
    pub constraint_adjustment: u32,
    /// The surface-space offset.
    pub offset: (i32, i32),
}

impl Default for Positioner {
    fn default() -> Self {
        Positioner {
            size: None,
            anchor_rect: Rect::new(0, 0, 0, 0),
            anchor: Anchor::Unset,
            gravity: Anchor::Unset,
            constraint_adjustment: 0,
            offset: (0, 0),
        }
    }
}

/// The pending half of a surface's double-buffered state.
#[derive(Clone, Debug, Default)]
pub struct Pending {
    /// The wl_buffer id to attach (0 = detach).
    pub buffer: u32,
    /// The attach offset.
    pub offset: (i32, i32),
    /// Accumulated surface-coordinate damage.
    pub damage: Vec<Rect>,
    /// Callback ids registered since the last commit.
    pub frame_callbacks: Vec<u32>,
    /// An opaque region set since the last commit (None = unchanged).
    pub opaque_region: Option<Vec<Rect>>,
    /// An input region set since the last commit (None = unchanged).
    pub input_region: Option<Vec<Rect>>,
}

/// A surface's role.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    /// A plain surface (no shell role yet).
    None,
    /// xdg_surface with a toplevel role (the xdg_surface id).
    Toplevel(u32),
    /// xdg_surface with a popup role (the xdg_surface id).
    Popup(u32),
}

/// One wl_surface.
#[derive(Clone, Debug)]
pub struct Surface {
    /// The pending state (applied at commit).
    pub pending: Pending,
    /// The current state.
    pub attached: u32,
    /// The committed damage accumulated for the driver.
    pub damage: Vec<Rect>,
    /// The applied opaque region.
    pub opaque_region: Vec<Rect>,
    /// The applied input region.
    pub input_region: Vec<Rect>,
    /// The role.
    pub role: Role,
    /// Frame callbacks awaiting the next commit's presentation.
    pub frame_callbacks: Vec<u32>,
    /// Whether the surface has ever committed a buffer.
    pub mapped: bool,
}

impl Default for Surface {
    fn default() -> Self {
        Surface {
            pending: Pending::default(),
            attached: 0,
            damage: Vec::new(),
            opaque_region: Vec::new(),
            input_region: Vec::new(),
            role: Role::None,
            frame_callbacks: Vec::new(),
            mapped: false,
        }
    }
}

impl Surface {
    /// Apply the pending state at commit; returns the frame
    /// callbacks that fire (the presentation this commit causes).
    #[must_use]
    pub fn commit(&mut self) -> Vec<u32> {
        let Pending {
            buffer,
            offset: _,
            damage,
            frame_callbacks,
            opaque_region,
            input_region,
        } = self.pending.clone();
        self.pending = Pending::default();
        // A zero buffer is a detach: the surface unmaps its content
        // but stays "mapped" (the role survives).
        self.attached = buffer;
        if buffer != 0 {
            self.mapped = true;
        }
        self.damage = damage;
        if let Some(o) = opaque_region {
            self.opaque_region = o;
        }
        if let Some(i) = input_region {
            self.input_region = i;
        }
        let fire = frame_callbacks;
        self.frame_callbacks.extend(fire.iter().copied());
        fire
    }
}

/// The xdg_surface handshake state.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum XdgPhase {
    /// No role assigned yet.
    NoRole,
    /// A role was assigned; the initial configure is on its way.
    RoleAssigned,
    /// The initial configure was sent, awaiting the ack.
    ConfigureSent(u32),
    /// The client acked; the first commit may map the surface.
    Acked(u32),
    /// Mapped (the first buffer commit landed).
    Mapped,
}

/// One xdg_surface (toplevel or popup role glue).
#[derive(Clone, Debug)]
pub struct XdgSurface {
    /// The wl_surface this xdg_surface wraps.
    pub surface: u32,
    /// The handshake phase.
    pub phase: XdgPhase,
    /// The last configure serial sent.
    pub serial: u32,
    /// The window geometry the client set (None = the buffer).
    pub window_geometry: Option<Rect>,
}

impl XdgSurface {
    /// A fresh xdg_surface wrapping `surface`.
    #[must_use]
    pub fn new(surface: u32) -> XdgSurface {
        XdgSurface {
            surface,
            phase: XdgPhase::NoRole,
            serial: 0,
            window_geometry: None,
        }
    }
}

/// One toplevel's shell state.
#[derive(Clone, Debug, Default)]
pub struct Toplevel {
    /// The owning xdg_surface (the role back-link).
    pub xdg: u32,
    /// The window title.
    pub title: Option<Box<str>>,
    /// The app id.
    pub app_id: Option<Box<str>>,
    /// The min size hints (0 = unset).
    pub min_size: (i32, i32),
    /// The max size hints (0 = unset).
    pub max_size: (i32, i32),
}

/// One popup's shell state.
#[derive(Clone, Debug)]
pub struct Popup {
    /// The owning xdg_surface (the role back-link).
    pub xdg: u32,
    /// The parent xdg_surface.
    pub parent: u32,
    /// The positioner state at creation.
    pub positioner: Positioner,
    /// The grab serial (0 = ungrabbed).
    pub grab_serial: u32,
}

/// One wl_shm_pool.
#[derive(Clone, Debug)]
pub struct ShmPool {
    /// The pool size in bytes.
    pub size: i32,
    /// Whether the client still owns it.
    pub alive: bool,
}

/// One wl_buffer carved from a pool.
#[derive(Clone, Debug)]
pub struct WlBuffer {
    /// The owning pool id.
    pub pool: u32,
    /// The byte offset in the pool.
    pub offset: i32,
    /// Width in pixels.
    pub width: i32,
    /// Height in pixels.
    pub height: i32,
    /// Row stride in bytes.
    pub stride: i32,
    /// The format (0 argb8888, 1 xrgb8888).
    pub format: u32,
}

impl WlBuffer {
    /// The byte length this buffer occupies.
    #[must_use]
    pub fn byte_len(&self) -> usize {
        self.stride.unsigned_abs() as usize * self.height.unsigned_abs() as usize
    }
}

/// The shm formats the subset serves.
pub mod formats {
    /// `argb8888` — premultiplied ARGB.
    pub const ARGB8888: u32 = 0;
    /// `xrgb8888` — opaque XRGB.
    pub const XRGB8888: u32 = 1;
    /// Every format code the subset accepts.
    pub const ACCEPTED: &[u32] = &[ARGB8888, XRGB8888];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchor_round_trips() {
        for v in 0..=8u32 {
            assert_eq!(Anchor::from_wire(v).to_wire(), v);
        }
        assert_eq!(Anchor::from_wire(7), Anchor::BottomLeft);
        assert_eq!(Anchor::from_wire(7).to_ldp_anchor(), Some(7));
        assert_eq!(Anchor::from_wire(0).to_ldp_anchor(), None);
        assert_eq!(Anchor::from_wire(9), Anchor::Unset);
    }

    #[test]
    fn surface_commit_applies_pending_atomically() {
        let mut s = Surface::default();
        s.pending.buffer = 9;
        s.pending.damage.push(Rect::new(0, 0, 10, 10));
        s.pending.frame_callbacks.push(4);
        let fire = s.commit();
        assert_eq!(fire, vec![4]);
        assert_eq!(s.attached, 9);
        assert!(s.mapped);
        assert_eq!(s.damage, vec![Rect::new(0, 0, 10, 10)]);
        // The pending state resets.
        assert_eq!(s.pending.buffer, 0);
        // A second commit without changes fires nothing new.
        assert!(s.commit().is_empty());
    }

    #[test]
    fn detach_keeps_mapped_but_nulls_buffer() {
        let mut s = Surface::default();
        s.pending.buffer = 5;
        let _ = s.commit();
        s.pending.buffer = 0; // detach
        let _ = s.commit();
        assert_eq!(s.attached, 0);
        assert!(s.mapped, "mapped is sticky");
    }

    #[test]
    fn positioner_defaults_are_unset() {
        let p = Positioner::default();
        assert!(p.size.is_none());
        assert_eq!(p.anchor, Anchor::Unset);
        assert_eq!(p.constraint_adjustment, 0);
    }

    #[test]
    fn buffer_byte_len_is_stride_times_height() {
        let b = WlBuffer {
            pool: 1,
            offset: 0,
            width: 8,
            height: 4,
            stride: 32,
            format: formats::XRGB8888,
        };
        assert_eq!(b.byte_len(), 128);
    }
}
