//! The positioning shell: where the system puts windows (Phase 28).
//!
//! This module is the *placement half* of the shell — the pure
//! geometry policy that decides, for one output, (a) which region of
//! the screen the system offers to windows (the *usable area*, carved
//! by the dock's reservation), (b) where the system's own dock
//! renders (the screen edge it clings to), and (c) where each new
//! toplevel anchors under the output's device class.
//!
//! The two doctrines:
//!
//! * **Phone** (a portrait output): one app at a time, anchored at
//!   the usable origin — the app stack sits *above* the dock, and a
//!   window taller than the usable area runs *under* it (the dock is
//!   chrome rendered above client content; overflow is occluded, not
//!   destroyed — the iOS keyboard doctrine).
//! * **Desktop** (landscape): the classic cascade — each new window
//!   steps diagonally until the usable edge catches it, then the
//!   stack accumulates at the corner (windows never leave the
//!   screen).
//!
//! Everything here is arithmetic on [`Rect`]: no clocks, no scene
//! types, no effects. The compositor feeds the output geometry and
//! the dock configuration in, reads placements out, and executes them
//! on the surface tree; the shell never learns what a surface is.
//!
//! The dock's *pixels* (the frosted bar, the app pills) are the
//! compositor's system chrome — Phase 27's material language applied
//! by the system itself, exactly the "the system owns the materials"
//! doctrine. This module only reserves the geometry.

#![forbid(unsafe_code)]

use ldp_core::geometry::Rect;

/// A screen edge.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Edge {
    /// The top edge (a status-bar dock).
    Top,
    /// The bottom edge (the phone home dock, the macOS dock).
    Bottom,
    /// The left edge (a vertical dock).
    Left,
    /// The right edge.
    Right,
}

/// The dock's configuration: which edge it clings to and how thick it
/// is. The dock always *spans* its edge (full width for a horizontal
/// edge) and *reserves* its thickness out of the usable area —
/// windows are placed above it, never under it by placement (what
/// they do with their own overflow is their business).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DockConfig {
    /// The edge the dock clings to.
    pub edge: Edge,
    /// The dock's thickness across the edge (px, logical).
    pub thickness: u32,
}

impl DockConfig {
    /// The phone's home dock: the bottom edge, 84 px — the pill bar
    /// the showcase's glass panel measured in Phase 27.
    pub const PHONE: DockConfig = DockConfig {
        edge: Edge::Bottom,
        thickness: 84,
    };

    /// Build a dock configuration.
    #[must_use]
    pub const fn new(edge: Edge, thickness: u32) -> DockConfig {
        DockConfig { edge, thickness }
    }
}

/// The device class an output's shape implies — the layout doctrine.
///
/// A portrait output (height > width) is a phone: one app stack, the
/// dock owns the bottom edge. Everything else is a desktop: the
/// cascade. The rule is deliberately *shallow* — shape, not a device
/// tree probe — so it is deterministic, testable, and honest about
/// what it knows (an external portrait monitor gets the phone
/// doctrine too, which is the correct behavior for a
/// portrait-primarily device like the phone this serves).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DeviceClass {
    /// Portrait: apps anchor at the usable origin (the phone).
    Phone,
    /// Landscape: windows cascade diagonally (the desktop).
    Desktop,
}

impl DeviceClass {
    /// Classify an output by its shape: portrait → phone.
    #[must_use]
    pub const fn classify(width: u32, height: u32) -> DeviceClass {
        if height > width {
            DeviceClass::Phone
        } else {
            DeviceClass::Desktop
        }
    }

    /// The default placement policy for this class.
    #[must_use]
    pub const fn default_policy(self) -> PlacementPolicy {
        match self {
            DeviceClass::Phone => PlacementPolicy::Fill,
            DeviceClass::Desktop => PlacementPolicy::Cascade,
        }
    }

    /// The one-word report line ("phone" / "desktop").
    #[must_use]
    pub const fn report(self) -> &'static str {
        match self {
            DeviceClass::Phone => "phone",
            DeviceClass::Desktop => "desktop",
        }
    }
}

/// How a new toplevel anchors in the usable area.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PlacementPolicy {
    /// Anchor at the usable origin — the phone's app stack. A window
    /// larger than the usable area overflows *under* the dock (the
    /// dock renders above; the overflow is occluded, never cropped).
    Fill,
    /// The classic desktop cascade: window *n* steps diagonally by
    /// [`CASCADE_STEP`] until the usable edge catches it, after which
    /// the stack accumulates at the corner.
    Cascade,
    /// Center in the usable area — dialogs. A dialog larger than the
    /// usable area centers with symmetric overflow.
    Center,
}

/// The cascade's diagonal step (px per placed window).
pub const CASCADE_STEP: i32 = 24;

/// The resolved layout for one output: the device class, the usable
/// area, and the dock's rectangle.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Layout {
    /// The output's device class.
    pub class: DeviceClass,
    /// The region offered to windows (the output minus the dock's
    /// reservation; the full output when no dock is configured).
    pub usable: Rect,
    /// Where the dock renders, when one is configured.
    pub dock: Option<Rect>,
    /// The dock configuration that produced this layout.
    pub dock_config: Option<DockConfig>,
}

impl Layout {
    /// Resolve the layout for one output: classify, dock, carve.
    ///
    /// The dock's thickness is clamped against half the output's
    /// along-edge extent (a dock may never eat more than half its
    /// edge — a 200 px dock on a 300 px output is a misconfiguration,
    /// not a brick: it renders at 150 px).
    #[must_use]
    pub fn resolve(output: (u32, u32), dock: Option<DockConfig>) -> Layout {
        let class = DeviceClass::classify(output.0, output.1);
        let dock_config = dock.map(|mut d| {
            let along = match d.edge {
                Edge::Top | Edge::Bottom => output.1,
                Edge::Left | Edge::Right => output.0,
            };
            d.thickness = d.thickness.min(along / 2);
            d
        });
        let dock_rect = dock_config.map(|d| dock_rect(output, d));
        Layout {
            class,
            usable: usable_area(output, dock_rect),
            dock: dock_rect,
            dock_config,
        }
    }

    /// Place a toplevel of `size` under this layout's default policy;
    /// `nth` is the placement counter (the cascade's step).
    #[must_use]
    pub fn place(&self, size: (u32, u32), nth: u32) -> (i32, i32) {
        place(self.class.default_policy(), self.usable, size, nth)
    }

    /// The honest one-line report ("phone layout, dock 84 px at the
    /// bottom" / "desktop layout, no dock").
    #[must_use]
    pub fn report(&self) -> String {
        match self.dock_config {
            Some(d) => format!(
                "{} layout, dock {} px at the {}",
                self.class.report(),
                d.thickness,
                edge_name(d.edge)
            ),
            None => format!("{} layout, no dock", self.class.report()),
        }
    }
}

/// The dock's rectangle on a full-span edge.
///
/// (Internal invariant: the caller clamped the thickness against half
/// the along-edge extent; a degenerate 0-width output yields a 0-size
/// rect, which simply reserves nothing.)
#[must_use]
pub fn dock_rect(output: (u32, u32), dock: DockConfig) -> Rect {
    let (w, h) = output;
    let t = dock.thickness.min(match dock.edge {
        Edge::Top | Edge::Bottom => h / 2,
        Edge::Left | Edge::Right => w / 2,
    });
    match dock.edge {
        Edge::Top => Rect::new(0, 0, w, t),
        Edge::Bottom => Rect::new(0, (h - t) as i32, w, t),
        Edge::Left => Rect::new(0, 0, t, h),
        Edge::Right => Rect::new((w - t) as i32, 0, t, h),
    }
}

/// The usable area: the output minus the dock's reservation. With no
/// dock (or a zero-thickness one) the whole output is usable.
///
/// The carve reads the dock's *span*: a dock spanning the output's
/// full width is horizontal (a top dock when it hugs `y = 0`, a
/// bottom dock otherwise); a dock spanning the output's full height
/// is vertical (left when it hugs `x = 0`, right otherwise). Anything
/// else is not an edge-spanning dock and reserves nothing.
#[must_use]
pub fn usable_area(output: (u32, u32), dock: Option<Rect>) -> Rect {
    let (w, h) = output;
    let full = Rect::new(0, 0, w, h);
    let Some(d) = dock else {
        return full;
    };
    if d.w == w && w > 0 {
        // Horizontal: carve the thickness off the hugging edge.
        if d.y == 0 {
            Rect::new(0, d.h as i32, w, h.saturating_sub(d.h))
        } else {
            Rect::new(0, 0, w, (d.y.max(0)) as u32)
        }
    } else if d.h == h && h > 0 {
        // Vertical: carve the thickness off the hugging edge.
        if d.x == 0 {
            Rect::new(d.w as i32, 0, w.saturating_sub(d.w), h)
        } else {
            Rect::new(0, 0, (d.x.max(0)) as u32, h)
        }
    } else {
        full
    }
}

/// Place a toplevel of `size` under `policy` in `usable`; `nth` is
/// the placement counter (the cascade's step index).
#[must_use]
pub fn place(policy: PlacementPolicy, usable: Rect, size: (u32, u32), nth: u32) -> (i32, i32) {
    match policy {
        PlacementPolicy::Fill => (usable.x, usable.y),
        PlacementPolicy::Cascade => {
            // A window that cannot fit anchors at the origin (the
            // corner stack's terminal state).
            if size.0 >= usable.w || size.1 >= usable.h {
                return (usable.x, usable.y);
            }
            let step = CASCADE_STEP.saturating_mul(nth as i32);
            let max_x = usable.right() - size.0 as i32;
            let max_y = usable.bottom() - size.1 as i32;
            ((usable.x + step).min(max_x), (usable.y + step).min(max_y))
        }
        PlacementPolicy::Center => {
            let x = usable.x + (usable.w.saturating_sub(size.0) / 2) as i32;
            let y = usable.y + (usable.h.saturating_sub(size.1) / 2) as i32;
            (x, y)
        }
    }
}

/// Place a toplevel of content `size` that wears server-drawn chrome
/// grown by `insets` (Phase 56 — the chrome-aware placement): the
/// *frame* — the content grown by the insets, the band above, the
/// border ring around — takes the policy slot, and the answer is the
/// content position inside it. A window parked at the usable origin
/// wears its top band *on* the screen, never above it; the cascade
/// steps captions, the way every desktop a user has ever used does
/// (DWM's work area answers in frame space; so does this one).
///
/// With [`crate::ssd::Insets::ZERO`] the answer is byte-identical
/// to [`place`]`(...)` for every policy — the no-chrome path never
/// moves, by construction.
#[must_use]
pub fn place_chrome(
    policy: PlacementPolicy,
    usable: Rect,
    size: (u32, u32),
    insets: crate::ssd::Insets,
    nth: u32,
) -> (i32, i32) {
    // The frame's footprint: the content grown by the chrome.
    let outer_w = size.0.saturating_add(insets.width());
    let outer_h = size.1.saturating_add(insets.height());
    let inside = |fx: i32, fy: i32| (fx + insets.left as i32, fy + insets.top as i32);
    match policy {
        // The frame anchors at the usable origin: a phone app's band
        // is the first thing on the screen, its content below, its
        // overflow still running under the dock (the Fill doctrine
        // unchanged — the band rides above it, never occluded).
        PlacementPolicy::Fill => inside(usable.x, usable.y),
        PlacementPolicy::Cascade => {
            // A frame that cannot fit anchors its corner at the
            // origin (the terminal corner stack, band visible).
            if outer_w >= usable.w || outer_h >= usable.h {
                return inside(usable.x, usable.y);
            }
            let step = CASCADE_STEP.saturating_mul(nth as i32);
            let max_x = usable.right() - outer_w as i32;
            let max_y = usable.bottom() - outer_h as i32;
            inside((usable.x + step).min(max_x), (usable.y + step).min(max_y))
        }
        PlacementPolicy::Center => {
            let x = usable.x + (usable.w.saturating_sub(outer_w) / 2) as i32;
            let y = usable.y + (usable.h.saturating_sub(outer_h) / 2) as i32;
            inside(x, y)
        }
    }
}

/// Clamp a chrome-wearing window fully inside `usable` (the migration
/// arm's answer, Phase 56): the *frame* — the content grown by
/// `insets` — is the rectangle that must fit, and the answer is the
/// content position inside the clamped frame. A frame larger than the
/// usable area anchors its corner at the origin (the terminal
/// doctrine, the band never left off-screen). With
/// [`crate::ssd::Insets::ZERO`] the frame *is* the content and the
/// answer matches [`clamp_into`] for the same rect.
#[must_use]
pub fn clamp_chrome(content: Rect, insets: crate::ssd::Insets, usable: Rect) -> (i32, i32) {
    let fx = content.x - insets.left as i32;
    let fy = content.y - insets.top as i32;
    let outer_w = content.w.saturating_add(insets.width());
    let outer_h = content.h.saturating_add(insets.height());
    if outer_w > usable.w || outer_h > usable.h {
        return (usable.x + insets.left as i32, usable.y + insets.top as i32);
    }
    let x = fx.clamp(usable.x, usable.right() - outer_w as i32);
    let y = fy.clamp(usable.y, usable.bottom() - outer_h as i32);
    (x + insets.left as i32, y + insets.top as i32)
}

/// Clamp a rectangle fully inside `usable`: pushed right/down (or
/// left/up) so no edge overhangs. A rectangle larger than the usable
/// area anchors at the usable origin (overflow is the caller's
/// doctrine — Fill occludes under the dock, Cascade never builds
/// one).
#[must_use]
pub fn clamp_into(rect: Rect, usable: Rect) -> Rect {
    if rect.w > usable.w || rect.h > usable.h {
        return Rect::new(usable.x, usable.y, rect.w, rect.h);
    }
    let x = rect.x.clamp(usable.x, usable.right() - rect.w as i32);
    let y = rect.y.clamp(usable.y, usable.bottom() - rect.h as i32);
    Rect::new(x, y, rect.w, rect.h)
}

/// The edge's report name.
const fn edge_name(edge: Edge) -> &'static str {
    match edge {
        Edge::Top => "top",
        Edge::Bottom => "bottom",
        Edge::Left => "left",
        Edge::Right => "right",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUT: (u32, u32) = (960, 540);
    const PHONE: (u32, u32) = (540, 960);

    #[test]
    fn classification_by_shape() {
        assert_eq!(DeviceClass::classify(540, 960), DeviceClass::Phone);
        assert_eq!(DeviceClass::classify(960, 540), DeviceClass::Desktop);
        // Square is a desktop (a window only becomes a phone by being
        // strictly taller).
        assert_eq!(DeviceClass::classify(1080, 1080), DeviceClass::Desktop);
        assert_eq!(DeviceClass::classify(1, 2), DeviceClass::Phone);
    }

    #[test]
    fn default_policies_follow_the_class() {
        assert_eq!(DeviceClass::Phone.default_policy(), PlacementPolicy::Fill);
        assert_eq!(
            DeviceClass::Desktop.default_policy(),
            PlacementPolicy::Cascade
        );
    }

    #[test]
    fn dock_rect_on_every_edge() {
        assert_eq!(
            dock_rect(OUT, DockConfig::new(Edge::Top, 40)),
            Rect::new(0, 0, 960, 40)
        );
        assert_eq!(
            dock_rect(OUT, DockConfig::new(Edge::Bottom, 84)),
            Rect::new(0, 456, 960, 84)
        );
        assert_eq!(
            dock_rect(OUT, DockConfig::new(Edge::Left, 30)),
            Rect::new(0, 0, 30, 540)
        );
        assert_eq!(
            dock_rect(OUT, DockConfig::new(Edge::Right, 30)),
            Rect::new(930, 0, 30, 540)
        );
    }

    #[test]
    fn dock_thickness_clamps_to_half_the_edge() {
        // A 400 px dock on a 300 px-tall output renders at 150 px.
        let r = dock_rect((640, 300), DockConfig::new(Edge::Bottom, 400));
        assert_eq!(r, Rect::new(0, 150, 640, 150));
        // Horizontal edges clamp against the width.
        let r = dock_rect((300, 640), DockConfig::new(Edge::Left, 400));
        assert_eq!(r, Rect::new(0, 0, 150, 640));
    }

    #[test]
    fn usable_area_carves_every_edge() {
        let out = (960, 540);
        let dock = |e: Edge| Some(dock_rect(out, DockConfig::new(e, 40)));
        assert_eq!(usable_area(out, None), Rect::new(0, 0, 960, 540));
        assert_eq!(
            usable_area(out, dock(Edge::Top)),
            Rect::new(0, 40, 960, 500)
        );
        assert_eq!(
            usable_area(out, dock(Edge::Bottom)),
            Rect::new(0, 0, 960, 500)
        );
        assert_eq!(
            usable_area(out, dock(Edge::Left)),
            Rect::new(40, 0, 920, 540)
        );
        assert_eq!(
            usable_area(out, dock(Edge::Right)),
            Rect::new(0, 0, 920, 540)
        );
        // A zero-thickness dock reserves nothing.
        let zero = Some(dock_rect(out, DockConfig::new(Edge::Bottom, 0)));
        assert_eq!(usable_area(out, zero), Rect::new(0, 0, 960, 540));
    }

    #[test]
    fn non_edge_dock_leaves_the_output_whole() {
        // A free-floating rect is not an edge-spanning dock: no carve.
        let floating = Some(Rect::new(100, 100, 50, 50));
        assert_eq!(usable_area(OUT, floating), Rect::new(0, 0, 960, 540));
    }

    #[test]
    fn resolve_phone_layout() {
        let l = Layout::resolve(PHONE, Some(DockConfig::PHONE));
        assert_eq!(l.class, DeviceClass::Phone);
        assert_eq!(l.usable, Rect::new(0, 0, 540, 876));
        assert_eq!(l.dock, Some(Rect::new(0, 876, 540, 84)));
    }

    #[test]
    fn resolve_desktop_layout() {
        let l = Layout::resolve(OUT, Some(DockConfig::PHONE));
        assert_eq!(l.class, DeviceClass::Desktop);
        assert_eq!(l.usable, Rect::new(0, 0, 960, 456));
        assert_eq!(l.dock, Some(Rect::new(0, 456, 960, 84)));
        // No dock: the full output.
        let plain = Layout::resolve(OUT, None);
        assert_eq!(plain.usable, Rect::new(0, 0, 960, 540));
        assert!(plain.dock.is_none());
    }

    #[test]
    fn fill_anchors_at_the_usable_origin() {
        let l = Layout::resolve(PHONE, Some(DockConfig::PHONE));
        // A full-usable app.
        assert_eq!(l.place((540, 876), 0), (0, 0));
        // A small app, letterboxed top-left.
        assert_eq!(l.place((300, 400), 7), (0, 0));
        // An oversized app: anchored, overflow is the caller's
        // doctrine (the dock occludes it).
        assert_eq!(l.place((540, 960), 0), (0, 0));
    }

    #[test]
    fn cascade_steps_and_catches_at_the_edge() {
        let usable = Rect::new(0, 0, 960, 456);
        // Window 0 at the origin.
        assert_eq!(
            place(PlacementPolicy::Cascade, usable, (400, 300), 0),
            (0, 0)
        );
        // Window 1 one step in.
        assert_eq!(
            place(PlacementPolicy::Cascade, usable, (400, 300), 1),
            (24, 24)
        );
        // Window 5 five steps in.
        assert_eq!(
            place(PlacementPolicy::Cascade, usable, (400, 300), 5),
            (120, 120)
        );
        // A window that would overhang the right edge (960) catches:
        // max_x = 960 - 400 = 560; 24 * 40 = 960 → clamped. Same on
        // the vertical: max_y = 456 - 300 = 156.
        assert_eq!(
            place(PlacementPolicy::Cascade, usable, (400, 300), 40),
            (560, 156)
        );
        // A window that cannot fit anchors at the origin.
        assert_eq!(
            place(PlacementPolicy::Cascade, usable, (1000, 300), 3),
            (0, 0)
        );
        assert_eq!(
            place(PlacementPolicy::Cascade, usable, (400, 500), 3),
            (0, 0)
        );
    }

    #[test]
    fn cascade_respects_a_offset_usable_origin() {
        // The usable area of a top-dock output: origin (0, 40).
        let usable = Rect::new(0, 40, 960, 500);
        assert_eq!(
            place(PlacementPolicy::Cascade, usable, (400, 300), 2),
            (48, 88)
        );
        assert_eq!(place(PlacementPolicy::Fill, usable, (400, 300), 9), (0, 40));
    }

    #[test]
    fn center_centers_in_the_usable_area() {
        let usable = Rect::new(0, 0, 960, 456);
        assert_eq!(
            place(PlacementPolicy::Center, usable, (400, 300), 0),
            (280, 78)
        );
        // Odd sizes floor.
        assert_eq!(
            place(PlacementPolicy::Center, usable, (401, 301), 0),
            (279, 77)
        );
        // A dialog larger than the usable area centers with symmetric
        // overflow (saturating subtraction → 0 offset).
        assert_eq!(
            place(PlacementPolicy::Center, usable, (1200, 300), 0),
            (0, 78)
        );
        // nth is irrelevant to centering.
        assert_eq!(
            place(PlacementPolicy::Center, usable, (400, 300), 99),
            (280, 78)
        );
    }

    #[test]
    fn clamp_into_pushes_every_edge() {
        let usable = Rect::new(0, 84, 540, 876);
        // Overhangs right and bottom.
        let r = clamp_into(Rect::new(400, 800, 200, 200), usable);
        assert_eq!(r, Rect::new(340, 760, 200, 200));
        // Overhangs left and top (negative origins).
        let r = clamp_into(Rect::new(-30, -30, 200, 200), usable);
        assert_eq!(r, Rect::new(0, 84, 200, 200));
        // Already inside: untouched.
        let r = clamp_into(Rect::new(10, 100, 200, 200), usable);
        assert_eq!(r, Rect::new(10, 100, 200, 200));
        // Larger than usable: anchors at the origin.
        let r = clamp_into(Rect::new(30, 30, 900, 900), usable);
        assert_eq!(r, Rect::new(0, 84, 900, 900));
    }

    /// The Lion chrome at 1x (the SSD pass's own band + ring).
    const BAND: crate::ssd::Insets = crate::ssd::Insets {
        left: 1,
        top: 29,
        right: 1,
        bottom: 1,
    };

    #[test]
    fn chrome_placement_parks_the_frame_at_the_origin() {
        // Fill: the frame anchors at the usable origin — the content
        // rides inside, the band on-screen above it.
        let usable = Rect::new(0, 0, 540, 876);
        assert_eq!(
            place_chrome(PlacementPolicy::Fill, usable, (540, 876), BAND, 0),
            (1, 29)
        );
        // The terminal anchor (a frame that cannot fit) still keeps
        // the band visible.
        assert_eq!(
            place_chrome(PlacementPolicy::Fill, usable, (540, 960), BAND, 0),
            (1, 29)
        );
    }

    #[test]
    fn chrome_placement_steps_the_cascade_by_frames() {
        let usable = Rect::new(0, 0, 960, 540);
        // The frame takes the slot: content = slot + insets.
        assert_eq!(
            place_chrome(PlacementPolicy::Cascade, usable, (400, 300), BAND, 0),
            (1, 29)
        );
        assert_eq!(
            place_chrome(PlacementPolicy::Cascade, usable, (400, 300), BAND, 1),
            (25, 53)
        );
        // The catch: the frame's edge, not the content's.
        // max_y = 540 - (300 + 30) = 210; 24 * 8 = 192 < 210 keeps
        // the step; 24 * 9 = 216 clamps.
        assert_eq!(
            place_chrome(PlacementPolicy::Cascade, usable, (400, 300), BAND, 9),
            (217, 210 + 29)
        );
        // A frame that cannot fit: the corner stack, band visible.
        assert_eq!(
            place_chrome(PlacementPolicy::Cascade, usable, (960, 300), BAND, 3),
            (1, 29)
        );
    }

    #[test]
    fn chrome_placement_centers_frames() {
        let usable = Rect::new(0, 0, 960, 540);
        // 960 - 402 = 558 / 2 = 279; 540 - 330 = 210 / 2 = 105.
        assert_eq!(
            place_chrome(PlacementPolicy::Center, usable, (400, 300), BAND, 0),
            (280, 134)
        );
        // Wider than usable: the saturating center is the origin,
        // the content rides at the inset (the overflow doctrine's
        // honest look — the band still on-screen).
        assert_eq!(
            place_chrome(PlacementPolicy::Center, usable, (1200, 300), BAND, 0),
            (1, 134)
        );
    }

    #[test]
    fn chrome_placement_is_the_placement_when_there_is_no_chrome() {
        // THE zero-drift proof: Insets::ZERO answers place() exactly,
        // every policy, every slot, sizes that fit and sizes that
        // do not.
        let usable = Rect::new(0, 40, 960, 500);
        for policy in [
            PlacementPolicy::Fill,
            PlacementPolicy::Cascade,
            PlacementPolicy::Center,
        ] {
            for nth in [0u32, 1, 5, 40] {
                for size in [(400u32, 300u32), (1000, 300), (960, 540)] {
                    assert_eq!(
                        place_chrome(policy, usable, size, crate::ssd::Insets::ZERO, nth),
                        place(policy, usable, size, nth),
                        "zero insets must never move {policy:?} nth={nth} size={size:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn chrome_clamp_keeps_the_frame_inside() {
        let usable = Rect::new(0, 0, 960, 540);
        // A window whose band rides above the display (content at
        // y=24, band top at -5): the frame is pushed down so the
        // band's top edge sits at y=0; the content rides at 29.
        assert_eq!(
            clamp_chrome(Rect::new(24, 24, 400, 300), BAND, usable),
            (24, 29)
        );
        // A frame overhanging the bottom (fy = 260 - 29 = 231 >
        // 540 - 330): pushed up to 210, the content at 239.
        assert_eq!(
            clamp_chrome(Rect::new(100, 260, 400, 300), BAND, usable),
            (100, 239)
        );
        // Already inside: the content stays where it is.
        assert_eq!(
            clamp_chrome(Rect::new(100, 100, 400, 300), BAND, usable),
            (100, 100)
        );
        // A frame larger than usable: the corner anchor, band
        // visible.
        assert_eq!(
            clamp_chrome(Rect::new(30, 30, 960, 540), BAND, usable),
            (1, 29)
        );
        // THE zero-drift proof: no chrome, the clamp is clamp_into.
        let plain = clamp_into(Rect::new(-30, -30, 200, 200), usable);
        assert_eq!(
            clamp_chrome(
                Rect::new(-30, -30, 200, 200),
                crate::ssd::Insets::ZERO,
                usable
            ),
            (plain.x, plain.y)
        );
    }

    #[test]
    fn layout_report_lines() {
        let phone = Layout::resolve(PHONE, Some(DockConfig::PHONE));
        assert_eq!(phone.report(), "phone layout, dock 84 px at the bottom");
        let plain = Layout::resolve(OUT, None);
        assert_eq!(plain.report(), "desktop layout, no dock");
        let top = Layout::resolve(OUT, Some(DockConfig::new(Edge::Top, 30)));
        assert_eq!(top.report(), "desktop layout, dock 30 px at the top");
    }

    #[test]
    fn degenerate_output_reserves_nothing() {
        // A 0-height output: the dock is empty, the usable is empty.
        let l = Layout::resolve((640, 0), Some(DockConfig::new(Edge::Bottom, 84)));
        assert_eq!(l.dock, Some(Rect::new(0, 0, 640, 0)));
        assert_eq!(l.usable, Rect::new(0, 0, 640, 0));
        // A 0-width output on a vertical dock.
        let l = Layout::resolve((0, 480), Some(DockConfig::new(Edge::Left, 84)));
        assert_eq!(l.dock, Some(Rect::new(0, 0, 0, 480)));
        assert_eq!(l.usable, Rect::new(0, 0, 0, 480));
    }

    #[test]
    fn usable_carve_survives_a_half_thickness_clamp() {
        // A dock thicker than half the output: clamped to half, so
        // the usable area is exactly the other half.
        let l = Layout::resolve((480, 640), Some(DockConfig::new(Edge::Bottom, 400)));
        assert_eq!(l.dock, Some(Rect::new(0, 320, 480, 320)));
        assert_eq!(l.usable, Rect::new(0, 0, 480, 320));
    }
}
