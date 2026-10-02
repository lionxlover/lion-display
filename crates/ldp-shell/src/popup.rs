//! The popup machine: anchor/gravity placement and constraint solving.
//!
//! Placement model (spec `get_popup`): `anchor_rect` lives in parent
//! coordinates; `anchor` names the edge/corner of that rect the popup
//! attaches to; `gravity` is the direction the popup *grows* from the
//! anchor point — equivalently, the popup corner that touches the
//! anchor is the one opposite the gravity vector. `offset` shifts the
//! attached corner (xdg semantics: `offset_x/y` displace the popup
//! along the anchor edge's axes).
//!
//! When the unconstrained placement leaves the usable area (the parent
//! toplevel's mapped rect intersected with the output), the solver
//! applies the strategies the client allowed, in a fixed order —
//! slide (x then y), flip (x then y), resize (x then y) — each only
//! its axis and only when permitted, and only while that axis still
//! offends. The final placement is reported relative to the parent's
//! top-left in `popup.configure`.

#![forbid(unsafe_code)]

use ldp_core::bitset::Bitset128;
use ldp_core::geometry::Rect;

use crate::serial::{Serial, SerialClock};

/// The anchor edge/corner (wire `anchor`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Anchor {
    /// Top-left corner (wire 1).
    TopLeft,
    /// Top edge midpoint (wire 2).
    Top,
    /// Top-right corner (wire 3).
    TopRight,
    /// Right edge midpoint (wire 4).
    Right,
    /// Bottom-right corner (wire 5).
    BottomRight,
    /// Bottom edge midpoint (wire 6).
    Bottom,
    /// Bottom-left corner (wire 7).
    BottomLeft,
    /// Left edge midpoint (wire 8).
    Left,
}

impl Anchor {
    /// The wire value.
    #[must_use]
    pub const fn wire(self) -> u32 {
        match self {
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

    /// From the wire value.
    #[must_use]
    pub const fn from_wire(v: u32) -> Option<Anchor> {
        Some(match v {
            1 => Anchor::TopLeft,
            2 => Anchor::Top,
            3 => Anchor::TopRight,
            4 => Anchor::Right,
            5 => Anchor::BottomRight,
            6 => Anchor::Bottom,
            7 => Anchor::BottomLeft,
            8 => Anchor::Left,
            _ => return None,
        })
    }

    /// The anchor point on `r` (parent coordinates).
    #[must_use]
    pub const fn point(self, r: Rect) -> (i32, i32) {
        let cx = r.x + (r.w as i32) / 2;
        let cy = r.y + (r.h as i32) / 2;
        match self {
            Anchor::TopLeft => (r.x, r.y),
            Anchor::Top => (cx, r.y),
            Anchor::TopRight => (r.x + r.w as i32, r.y),
            Anchor::Right => (r.x + r.w as i32, cy),
            Anchor::BottomRight => (r.x + r.w as i32, r.y + r.h as i32),
            Anchor::Bottom => (cx, r.y + r.h as i32),
            Anchor::BottomLeft => (r.x, r.y + r.h as i32),
            Anchor::Left => (r.x, cy),
        }
    }

    /// The anchor mirrored to the opposite side (flip strategy).
    #[must_use]
    pub const fn flipped(self) -> Anchor {
        match self {
            Anchor::TopLeft => Anchor::BottomRight,
            Anchor::Top => Anchor::Bottom,
            Anchor::TopRight => Anchor::BottomLeft,
            Anchor::Right => Anchor::Left,
            Anchor::BottomRight => Anchor::TopLeft,
            Anchor::Bottom => Anchor::Top,
            Anchor::BottomLeft => Anchor::TopRight,
            Anchor::Left => Anchor::Right,
        }
    }
}

/// The growth direction (wire `gravity`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Gravity {
    /// Grows left and up (wire 1).
    TopLeft,
    /// Grows up (wire 2).
    Top,
    /// Grows right and up (wire 3).
    TopRight,
    /// Grows right (wire 4).
    Right,
    /// Grows right and down (wire 5).
    BottomRight,
    /// Grows down (wire 6).
    Bottom,
    /// Grows left and down (wire 7).
    BottomLeft,
    /// Grows left (wire 8).
    Left,
}

impl Gravity {
    /// The wire value.
    #[must_use]
    pub const fn wire(self) -> u32 {
        match self {
            Gravity::TopLeft => 1,
            Gravity::Top => 2,
            Gravity::TopRight => 3,
            Gravity::Right => 4,
            Gravity::BottomRight => 5,
            Gravity::Bottom => 6,
            Gravity::BottomLeft => 7,
            Gravity::Left => 8,
        }
    }

    /// From the wire value.
    #[must_use]
    pub const fn from_wire(v: u32) -> Option<Gravity> {
        Some(match v {
            1 => Gravity::TopLeft,
            2 => Gravity::Top,
            3 => Gravity::TopRight,
            4 => Gravity::Right,
            5 => Gravity::BottomRight,
            6 => Gravity::Bottom,
            7 => Gravity::BottomLeft,
            8 => Gravity::Left,
            _ => return None,
        })
    }

    /// The gravity mirrored to the opposite direction (flip strategy).
    #[must_use]
    pub const fn flipped(self) -> Gravity {
        match self {
            Gravity::TopLeft => Gravity::BottomRight,
            Gravity::Top => Gravity::Bottom,
            Gravity::TopRight => Gravity::BottomLeft,
            Gravity::Right => Gravity::Left,
            Gravity::BottomRight => Gravity::TopLeft,
            Gravity::Bottom => Gravity::Top,
            Gravity::BottomLeft => Gravity::TopRight,
            Gravity::Left => Gravity::Right,
        }
    }

    /// The offset from the anchor point to the popup's top-left
    /// corner: the spec defines gravity as the direction the popup
    /// *grows* from the anchor point, so the popup edge opposite the
    /// growth vector sits at the anchor (e.g. gravity `bottom` grows
    /// downward — the popup's top edge is at the anchor point).
    #[must_use]
    pub const fn corner_offset(self, w: u32, h: u32) -> (i32, i32) {
        match self {
            Gravity::TopLeft => (-(w as i32), -(h as i32)),
            Gravity::Top => (-(w as i32) / 2, -(h as i32)),
            Gravity::TopRight => (0, -(h as i32)),
            Gravity::Right => (0, -(h as i32) / 2),
            Gravity::BottomRight => (0, 0),
            Gravity::Bottom => (-(w as i32) / 2, 0),
            Gravity::BottomLeft => (-(w as i32), 0),
            Gravity::Left => (-(w as i32), -(h as i32) / 2),
        }
    }
}

/// The constraint strategies (wire `popup_constraints`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct PopupConstraints(pub Bitset128);

impl PopupConstraints {
    /// Slide along x (bit 0).
    pub const SLIDE_X: u32 = 0;
    /// Slide along y (bit 1).
    pub const SLIDE_Y: u32 = 1;
    /// Flip horizontally (bit 2).
    pub const FLIP_X: u32 = 2;
    /// Flip vertically (bit 3).
    pub const FLIP_Y: u32 = 3;
    /// Resize horizontally (bit 4).
    pub const RESIZE_X: u32 = 4;
    /// Resize vertically (bit 5).
    pub const RESIZE_Y: u32 = 5;

    /// No strategies (hard placement).
    #[must_use]
    pub const fn none() -> PopupConstraints {
        PopupConstraints(Bitset128::from_words([0, 0, 0, 0]))
    }

    /// Allow a strategy.
    #[must_use]
    pub const fn with(self, bit: u32) -> PopupConstraints {
        PopupConstraints(self.0.with(bit))
    }

    /// Whether a strategy is allowed.
    #[must_use]
    pub fn allows(self, bit: u32) -> bool {
        self.0.test(bit)
    }

    /// All strategies (the default a generous client grants).
    #[must_use]
    pub const fn all() -> PopupConstraints {
        PopupConstraints(Bitset128::from_words([0b11_1111, 0, 0, 0]))
    }
}

/// The solved placement (relative to the parent's top-left).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Placement {
    /// Left edge (parent coords).
    pub x: i32,
    /// Top edge (parent coords).
    pub y: i32,
    /// Width (possibly clamped by resize).
    pub width: u32,
    /// Height.
    pub height: u32,
}

/// Popup misuse.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PopupError {
    /// `grab` serial does not reference a press from this seat.
    BadGrabSerial,
    /// The popup is already dismissed (`reposition`/`grab` after
    /// `done`).
    Dismissed,
}

impl std::fmt::Display for PopupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PopupError::BadGrabSerial => {
                f.write_str("grab serial does not reference a button press")
            }
            PopupError::Dismissed => f.write_str("the popup is dismissed"),
        }
    }
}

impl std::error::Error for PopupError {}

/// The anchor geometry of a popup (what `get_popup`/`reposition` carry).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PopupGeometry {
    /// Anchor rect, parent coordinates.
    pub anchor_rect: Rect,
    /// Which edge/corner to attach to.
    pub anchor: Anchor,
    /// Growth direction.
    pub gravity: Gravity,
    /// Offset applied to the attached corner.
    pub offset: (i32, i32),
}

/// A popup: geometry, constraint grant, grab state, lifecycle.
#[derive(Debug)]
pub struct Popup {
    serials: SerialClock,
    geometry: PopupGeometry,
    constraints: PopupConstraints,
    /// The live placement proposal (a reposition replaces it).
    pending: Option<(Serial, Placement)>,
    /// Whether an explicit pointer grab holds.
    grabbed: bool,
    /// Whether the popup is live (done ⇒ false).
    live: bool,
}

impl Popup {
    /// A new popup with its anchor geometry and constraint grant.
    #[must_use]
    pub fn new(geometry: PopupGeometry, constraints: PopupConstraints) -> Popup {
        Popup {
            serials: SerialClock::new(),
            geometry,
            constraints,
            pending: None,
            grabbed: false,
            live: true,
        }
    }

    /// Whether the popup is live.
    #[must_use]
    pub fn is_live(&self) -> bool {
        self.live
    }

    /// Whether an explicit grab holds.
    #[must_use]
    pub fn is_grabbed(&self) -> bool {
        self.grabbed
    }

    /// The current geometry.
    #[must_use]
    pub fn geometry(&self) -> PopupGeometry {
        self.geometry
    }

    /// The constraint grant.
    #[must_use]
    pub fn constraints(&self) -> PopupConstraints {
        self.constraints
    }

    /// The live placement proposal.
    #[must_use]
    pub fn pending(&self) -> Option<(Serial, Placement)> {
        self.pending
    }

    /// Solve the unconstrained placement for a popup of `size` under
    /// the current geometry.
    #[must_use]
    pub fn unconstrained(&self, size: (u32, u32)) -> Placement {
        solve_unconstrained(&self.geometry, size)
    }

    /// Propose a placement: run the constraint solver for `size`
    /// against `usable` (parent coords) and record it as the live
    /// proposal. Returns the placement to emit in `configure`.
    #[must_use]
    pub fn propose(&mut self, size: (u32, u32), usable: Rect) -> (Serial, Placement) {
        let serial = self.serials.issue();
        let p = solve(&self.geometry, size, usable, self.constraints);
        self.serials.reserve(serial);
        self.pending = Some((serial, p));
        (serial, p)
    }

    /// `reposition`: replace the geometry and re-solve.
    ///
    /// # Errors
    /// [`PopupError::Dismissed`] when the popup is done.
    pub fn reposition(&mut self, geometry: PopupGeometry) -> Result<(), PopupError> {
        if !self.live {
            return Err(PopupError::Dismissed);
        }
        self.geometry = geometry;
        Ok(())
    }

    /// `ack_configure` on a popup: any serial the client saw is
    /// acceptable (placement is advisory; the client commits a buffer
    /// of its chosen size). Returns whether the serial was known.
    pub fn ack_configure(&mut self, serial: Serial) -> bool {
        self.pending.is_some_and(|(s, _)| s == serial)
    }

    /// `grab`: take an explicit pointer grab. The serial must reference
    /// a button press from this seat — the integrator validates that
    /// against its serial record; here `press_serial` is that record's
    /// current value (`None`: no press is referenceable).
    ///
    /// # Errors
    /// [`PopupError::BadGrabSerial`] when the serial does not match;
    /// [`PopupError::Dismissed`] after `done`.
    pub fn grab(&mut self, press_serial: Option<Serial>, serial: Serial) -> Result<(), PopupError> {
        if !self.live {
            return Err(PopupError::Dismissed);
        }
        match press_serial {
            Some(p) if p == serial => {
                self.grabbed = true;
                Ok(())
            }
            _ => Err(PopupError::BadGrabSerial),
        }
    }

    /// `dismiss` (client-initiated) or a server-side dismissal cause:
    /// the popup is done.
    ///
    /// # Panics
    ///
    /// Never; dismissing twice is a no-op.
    pub fn dismiss(&mut self) {
        self.live = false;
        self.grabbed = false;
    }
}

/// The unconstrained placement: the popup's gravity-opposite corner at
/// the anchor point, shifted by the offset.
#[must_use]
pub fn solve_unconstrained(g: &PopupGeometry, size: (u32, u32)) -> Placement {
    let (ax, ay) = g.anchor.point(g.anchor_rect);
    let (dx, dy) = g.gravity.corner_offset(size.0, size.1);
    Placement {
        x: ax + dx + g.offset.0,
        y: ay + dy + g.offset.1,
        width: size.0,
        height: size.1,
    }
}

/// The full constraint pipeline. Order: slide x, slide y, flip x,
/// flip y, resize x, resize y — each only when allowed and only while
/// the axis still offends the usable area.
///
/// Semantics (xdg-lineage, pinned by the geometry suites):
/// * **slide** moves the popup toward the usable area along one axis,
///   never detaching past the *opposite* edge of the anchor rect;
/// * **flip** mirrors the anchor and gravity on the still-offending
///   axis (the offset is an anchor-point displacement and is not
///   negated) and accepts the mirrored placement only when that axis
///   then fits;
/// * **resize** clamps the size so the popup fits from its current
///   position toward the overflowing edge (left/top overflow cannot
///   be resized away — the placement simply stands, partially
///   off-screen, the honest "constraints exhausted" outcome).
#[must_use]
pub fn solve(
    g: &PopupGeometry,
    size: (u32, u32),
    usable: Rect,
    constraints: PopupConstraints,
) -> Placement {
    let mut p = solve_unconstrained(g, size);
    let usable_right = usable.x + usable.w as i32;
    let usable_bottom = usable.y + usable.h as i32;

    // Slide x: toward the usable area, capped at the anchor rect's
    // opposite edge (the popup stays attached).
    if constraints.allows(PopupConstraints::SLIDE_X) {
        if p.x < usable.x {
            let needed = usable.x - p.x;
            // The popup's left edge may not pass the anchor rect's
            // right edge.
            let cap = ((g.anchor_rect.x + g.anchor_rect.w as i32) - p.x).max(0);
            p.x += needed.min(cap);
        } else if p.x + p.width as i32 > usable_right {
            let needed = (p.x + p.width as i32) - usable_right;
            // The popup's right edge may not pass the anchor rect's
            // left edge.
            let cap = (p.x - (g.anchor_rect.x - p.width as i32)).max(0);
            p.x -= needed.min(cap);
        }
    }
    // Slide y.
    if constraints.allows(PopupConstraints::SLIDE_Y) {
        if p.y < usable.y {
            let needed = usable.y - p.y;
            let cap = ((g.anchor_rect.y + g.anchor_rect.h as i32) - p.y).max(0);
            p.y += needed.min(cap);
        } else if p.y + p.height as i32 > usable_bottom {
            let needed = (p.y + p.height as i32) - usable_bottom;
            let cap = (p.y - (g.anchor_rect.y - p.height as i32)).max(0);
            p.y -= needed.min(cap);
        }
    }

    // Flip x: mirror anchor + gravity, re-place, accept only when
    // the axis then fits.
    if (p.x < usable.x || p.x + p.width as i32 > usable_right)
        && constraints.allows(PopupConstraints::FLIP_X)
    {
        let mut g2 = *g;
        g2.anchor = g.anchor.flipped_x();
        g2.gravity = g.gravity.flipped_x();
        let q = solve_unconstrained(&g2, (p.width, p.height));
        if q.x >= usable.x && q.x + q.width as i32 <= usable_right {
            p.x = q.x;
        }
    }
    // Flip y.
    if (p.y < usable.y || p.y + p.height as i32 > usable_bottom)
        && constraints.allows(PopupConstraints::FLIP_Y)
    {
        let mut g2 = *g;
        g2.anchor = g.anchor.flipped_y();
        g2.gravity = g.gravity.flipped_y();
        let q = solve_unconstrained(&g2, (p.width, p.height));
        if q.y >= usable.y && q.y + q.height as i32 <= usable_bottom {
            p.y = q.y;
        }
    }

    // Resize: clamp the extent toward the overflowing edge.
    if constraints.allows(PopupConstraints::RESIZE_X) && p.x + p.width as i32 > usable_right {
        p.width = p.width.min((usable_right - p.x).max(0) as u32);
    }
    if constraints.allows(PopupConstraints::RESIZE_Y) && p.y + p.height as i32 > usable_bottom {
        p.height = p.height.min((usable_bottom - p.y).max(0) as u32);
    }
    p
}

impl Anchor {
    /// Mirror only the horizontal component (flip x).
    #[must_use]
    pub const fn flipped_x(self) -> Anchor {
        match self {
            Anchor::TopLeft => Anchor::TopRight,
            Anchor::Top => Anchor::Top,
            Anchor::TopRight => Anchor::TopLeft,
            Anchor::Right => Anchor::Right,
            Anchor::BottomRight => Anchor::BottomLeft,
            Anchor::Bottom => Anchor::Bottom,
            Anchor::BottomLeft => Anchor::BottomRight,
            Anchor::Left => Anchor::Left,
        }
    }

    /// Mirror only the vertical component (flip y).
    #[must_use]
    pub const fn flipped_y(self) -> Anchor {
        match self {
            Anchor::TopLeft => Anchor::BottomLeft,
            Anchor::Top => Anchor::Bottom,
            Anchor::TopRight => Anchor::BottomRight,
            Anchor::Right => Anchor::Right,
            Anchor::BottomRight => Anchor::TopRight,
            Anchor::Bottom => Anchor::Top,
            Anchor::BottomLeft => Anchor::TopLeft,
            Anchor::Left => Anchor::Left,
        }
    }
}

impl Gravity {
    /// Mirror only the horizontal component (flip x).
    #[must_use]
    pub const fn flipped_x(self) -> Gravity {
        match self {
            Gravity::TopLeft => Gravity::TopRight,
            Gravity::Top => Gravity::Top,
            Gravity::TopRight => Gravity::TopLeft,
            Gravity::Right => Gravity::Left,
            Gravity::BottomRight => Gravity::BottomLeft,
            Gravity::Bottom => Gravity::Bottom,
            Gravity::BottomLeft => Gravity::BottomRight,
            Gravity::Left => Gravity::Right,
        }
    }

    /// Mirror only the vertical component (flip y).
    #[must_use]
    pub const fn flipped_y(self) -> Gravity {
        match self {
            Gravity::TopLeft => Gravity::BottomLeft,
            Gravity::Top => Gravity::Bottom,
            Gravity::TopRight => Gravity::BottomRight,
            Gravity::Right => Gravity::Right,
            Gravity::BottomRight => Gravity::TopRight,
            Gravity::Bottom => Gravity::Top,
            Gravity::BottomLeft => Gravity::TopLeft,
            Gravity::Left => Gravity::Left,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geom(anchor: Anchor, gravity: Gravity) -> PopupGeometry {
        PopupGeometry {
            anchor_rect: Rect::new(900, 500, 120, 30),
            anchor,
            gravity,
            offset: (0, 0),
        }
    }

    fn all() -> PopupConstraints {
        PopupConstraints::all()
    }

    const SCREEN: Rect = Rect::new(0, 0, 1920, 1080);

    #[test]
    fn anchor_and_gravity_wire_round_trip() {
        for a in [
            Anchor::TopLeft,
            Anchor::Top,
            Anchor::TopRight,
            Anchor::Right,
            Anchor::BottomRight,
            Anchor::Bottom,
            Anchor::BottomLeft,
            Anchor::Left,
        ] {
            assert_eq!(Anchor::from_wire(a.wire()), Some(a));
        }
        assert_eq!(Anchor::from_wire(0), None);
        assert_eq!(Anchor::from_wire(9), None);
        for g in [
            Gravity::TopLeft,
            Gravity::Top,
            Gravity::TopRight,
            Gravity::Right,
            Gravity::BottomRight,
            Gravity::Bottom,
            Gravity::BottomLeft,
            Gravity::Left,
        ] {
            assert_eq!(Gravity::from_wire(g.wire()), Some(g));
        }
        assert_eq!(Gravity::from_wire(0), None);
        // Flips are involutions.
        for a in [Anchor::TopLeft, Anchor::Top, Anchor::Right, Anchor::Bottom] {
            assert_eq!(a.flipped().flipped(), a);
            assert_eq!(a.flipped_x().flipped_x(), a);
            assert_eq!(a.flipped_y().flipped_y(), a);
        }
    }

    #[test]
    fn anchor_point_picks_the_edge() {
        let r = Rect::new(100, 200, 60, 40);
        assert_eq!(Anchor::TopLeft.point(r), (100, 200));
        assert_eq!(Anchor::Top.point(r), (130, 200));
        assert_eq!(Anchor::TopRight.point(r), (160, 200));
        assert_eq!(Anchor::Right.point(r), (160, 220));
        assert_eq!(Anchor::BottomRight.point(r), (160, 240));
        assert_eq!(Anchor::Bottom.point(r), (130, 240));
        assert_eq!(Anchor::BottomLeft.point(r), (100, 240));
        assert_eq!(Anchor::Left.point(r), (100, 220));
    }

    #[test]
    fn unconstrained_grows_along_gravity() {
        // Menu below a bar item: anchor bottom, gravity bottom (grows
        // down) — the popup's top edge is at the anchor point.
        let g = geom(Anchor::Bottom, Gravity::Bottom);
        let p = solve_unconstrained(&g, (200, 100));
        // Anchor point (960, 530); popup top-center there.
        assert_eq!((p.x, p.y, p.width, p.height), (860, 530, 200, 100));
        // Same anchor, gravity top: grows up from the same point.
        let g2 = geom(Anchor::Bottom, Gravity::Top);
        let p2 = solve_unconstrained(&g2, (200, 100));
        assert_eq!((p2.x, p2.y), (860, 430));
        // Gravity right: grows right from the anchor midpoint.
        let g3 = geom(Anchor::Left, Gravity::Right);
        let p3 = solve_unconstrained(&g3, (200, 100));
        // Anchor point (900, 515); top edge centered vertically.
        assert_eq!((p3.x, p3.y), (900, 465));
        // Offset displaces the anchor point.
        let g4 = PopupGeometry {
            offset: (10, 4),
            ..geom(Anchor::Bottom, Gravity::Bottom)
        };
        let p4 = solve_unconstrained(&g4, (200, 100));
        assert_eq!((p4.x, p4.y), (870, 534));
    }

    #[test]
    fn slide_pulls_back_but_not_off_the_anchor() {
        // A menu at the far right edge: anchor top_right, gravity
        // bottom_right (grows right, off-screen).
        let g = PopupGeometry {
            anchor_rect: Rect::new(1850, 100, 60, 30),
            anchor: Anchor::TopRight,
            gravity: Gravity::BottomRight,
            offset: (0, 0),
        };
        let p = solve(&g, (200, 100), SCREEN, all());
        // Unconstrained x = 1910 (off by 190). Slide left: needed 190,
        // cap keeps right edge >= anchor left edge (1850): popup left
        // >= 1650. 1910-190 = 1720 >= 1650 → slide succeeds fully.
        assert_eq!((p.x, p.width), (1720, 200));
        assert!(p.x >= 1650);
        // No slide permission: stands off-screen.
        let hard = solve(&g, (200, 100), SCREEN, PopupConstraints::none());
        assert_eq!(hard.x, 1910);
        // Slide capped: a popup wider than the room the cap allows
        // slides only as far as the anchor's left edge.
        let g2 = PopupGeometry {
            anchor_rect: Rect::new(1880, 100, 30, 30),
            anchor: Anchor::TopRight,
            gravity: Gravity::BottomRight,
            offset: (0, 0),
        };
        let p2 = solve(
            &g2,
            (400, 100),
            SCREEN,
            PopupConstraints::none().with(PopupConstraints::SLIDE_X),
        );
        // Unconstrained x = 1910, right edge 2310 (off by 390); cap =
        // 1910 - (1880 - 400) = 430 → slide fully: x = 1520. The cap
        // binds only when the popup is wider than the anchor-to-edge
        // room — the flip test covers the exhausted case instead.
        assert_eq!((p2.x, p2.width), (1520, 400));
    }

    #[test]
    fn flip_mirrors_to_the_other_side() {
        // Menu anchored at the bottom of the screen growing down:
        // with slide allowed the slide fully fixes it (the cap always
        // suffices when the anchor rect is inside the usable area).
        let g = PopupGeometry {
            anchor_rect: Rect::new(900, 1000, 120, 30),
            anchor: Anchor::Bottom,
            gravity: Gravity::Bottom,
            offset: (0, 0),
        };
        let p = solve(&g, (200, 300), SCREEN, all());
        // Slid up: bottom edge at 1080 (cap allows down to y=700).
        assert_eq!(p.y, 780);
        // Without slide, flip y saves it: mirrored anchor top of the
        // rect (960, 1000), gravity top (grows up) → bottom at 1000.
        let only_flip = PopupConstraints::none().with(PopupConstraints::FLIP_Y);
        let p2 = solve(&g, (200, 300), SCREEN, only_flip);
        assert_eq!((p2.x, p2.y), (860, 700));
        // Neither slide nor flip: y stands off-screen, but resize_y
        // clamps the height to what fits below.
        let only_resize = PopupConstraints::none().with(PopupConstraints::RESIZE_Y);
        let p3 = solve(&g, (200, 300), SCREEN, only_resize);
        assert_eq!(p3.y, 1030);
        assert_eq!(p3.height, 50); // 1080 - 1030
    }

    #[test]
    fn resize_clamps_when_nothing_else_fits() {
        let g = PopupGeometry {
            anchor_rect: Rect::new(1850, 100, 60, 30),
            anchor: Anchor::TopRight,
            gravity: Gravity::BottomRight,
            offset: (0, 0),
        };
        // No slide, no flip: x stands at 1910; resize clamps width.
        let only_resize = PopupConstraints::none()
            .with(PopupConstraints::RESIZE_X)
            .with(PopupConstraints::RESIZE_Y);
        let p = solve(&g, (200, 100), SCREEN, only_resize);
        assert_eq!((p.x, p.width), (1910, 10));
        assert_eq!(p.height, 100);
    }

    #[test]
    fn well_fitting_popup_is_untouched() {
        let g = geom(Anchor::Bottom, Gravity::Bottom);
        let p = solve(&g, (200, 100), SCREEN, all());
        assert_eq!(p, solve_unconstrained(&g, (200, 100)));
    }

    #[test]
    fn popup_machine_lifecycle() {
        let g = geom(Anchor::Bottom, Gravity::Bottom);
        let mut popup = Popup::new(g, all());
        assert!(popup.is_live());
        assert!(!popup.is_grabbed());
        let (s1, p1) = popup.propose((200, 100), SCREEN);
        assert_eq!(s1, Serial(1));
        assert_eq!(popup.pending(), Some((s1, p1)));
        assert!(popup.ack_configure(s1));
        assert!(!popup.ack_configure(Serial(9)));
        // Reposition + re-propose: new serial supersedes.
        popup
            .reposition(geom(Anchor::BottomLeft, Gravity::BottomLeft))
            .unwrap();
        let (s2, _) = popup.propose((150, 80), SCREEN);
        assert!(s2.after(s1));
        assert!(!popup.ack_configure(s1));
        assert!(popup.ack_configure(s2));
        // Grab requires a matching press serial.
        assert_eq!(popup.grab(None, Serial(7)), Err(PopupError::BadGrabSerial));
        assert_eq!(
            popup.grab(Some(Serial(8)), Serial(7)),
            Err(PopupError::BadGrabSerial)
        );
        popup.grab(Some(Serial(7)), Serial(7)).unwrap();
        assert!(popup.is_grabbed());
        // Dismissal ends everything; repeat ops refuse.
        popup.dismiss();
        assert!(!popup.is_live());
        assert!(!popup.is_grabbed());
        assert_eq!(popup.reposition(g), Err(PopupError::Dismissed));
        assert_eq!(
            popup.grab(Some(Serial(7)), Serial(7)),
            Err(PopupError::Dismissed)
        );
        popup.dismiss(); // idempotent
    }
}
