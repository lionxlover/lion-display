//! Focus routing: subpixel hit testing and per-seat focus targets.
//!
//! Input regions are the protocol's honest answer to "which surface
//! does this point belong to": a surface may declare an arbitrary set
//! of rectangles (surface coordinates, subpixel); the empty region
//! means the whole surface. Hit testing walks the scene top-first and
//! returns the first surface whose region contains the point.
//!
//! Three focus domains, three policies (architecture §14):
//!
//! * **pointer focus** follows the input-region hit test on motion,
//! * **keyboard focus** is shell-driven — the scene supplies it per
//!   frame, the router never guesses,
//! * **touch focus** is grab-driven — the surface a contact landed on
//!   owns that contact until it lifts.
//!
//! [`FocusRouter`] tracks the pointer domain (the other two arrive
//! with the scene or live in the router's touch-grab map); it is pure
//! data, so multi-seat isolation is structural: one instance per
//! seat, no shared state anywhere.

#![forbid(unsafe_code)]

use std::collections::HashMap;

use ldp_core::geometry::PointF;
use ldp_core::ids::ObjectId;

/// Opaque surface identity for routing (the compositor maps these to
/// its scene keys).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct SurfaceKey(u64);

impl SurfaceKey {
    /// Construct.
    pub const fn new(id: u64) -> SurfaceKey {
        SurfaceKey(id)
    }

    /// The raw key.
    #[must_use]
    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

/// A subpixel rectangle (surface coordinates for regions, output
/// coordinates for positions).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct RectF {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width.
    pub w: f32,
    /// Height.
    pub h: f32,
}

impl RectF {
    /// Construct.
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> RectF {
        RectF { x, y, w, h }
    }

    /// Whether `p` (in the same coordinate space) is inside.
    #[must_use]
    pub fn contains(&self, p: PointF) -> bool {
        p.x >= self.x && p.x < self.x + self.w && p.y >= self.y && p.y < self.y + self.h
    }
}

/// An input region: rects in surface coordinates; empty = the whole
/// surface.
#[derive(Clone, Debug, Default)]
pub struct InputRegion {
    /// The rects (disjunction).
    pub rects: Vec<RectF>,
}

impl InputRegion {
    /// The whole-surface region (no rects).
    #[must_use]
    pub fn everywhere() -> InputRegion {
        InputRegion::default()
    }

    /// Whether the region is the whole surface.
    #[must_use]
    pub fn is_everywhere(&self) -> bool {
        self.rects.is_empty()
    }

    /// Whether a surface-local point is in the region.
    #[must_use]
    pub fn contains(&self, local: PointF) -> bool {
        self.is_everywhere() || self.rects.iter().any(|r| r.contains(local))
    }
}

/// One routable surface: identity, wire object, placement, input
/// region.
#[derive(Clone, Debug)]
pub struct SurfaceRef {
    /// Routing key.
    pub key: SurfaceKey,
    /// The surface's wire object id (for enter/leave/down args).
    pub surface: ObjectId,
    /// The surface's origin in output coordinates.
    pub origin: PointF,
    /// The surface's size (for whole-surface hit testing).
    pub size: (f32, f32),
    /// The input region (surface coordinates; empty = whole surface).
    pub region: InputRegion,
}

impl SurfaceRef {
    /// Whether an output-space point is over this surface.
    #[must_use]
    pub fn hit(&self, p: PointF) -> bool {
        let local = PointF::new(p.x - self.origin.x, p.y - self.origin.y);
        let in_bbox =
            local.x >= 0.0 && local.x < self.size.0 && local.y >= 0.0 && local.y < self.size.1;
        in_bbox && self.region.contains(local)
    }

    /// The surface-local coordinates of an output-space point.
    #[must_use]
    pub fn local(&self, p: PointF) -> PointF {
        PointF::new(p.x - self.origin.x, p.y - self.origin.y)
    }
}

/// Hit test a scene, top-first.
#[must_use]
pub fn hit_test(surfaces: &[SurfaceRef], p: PointF) -> Option<SurfaceKey> {
    surfaces.iter().find(|s| s.hit(p)).map(|s| s.key)
}

/// One client's seat-object bindings (what the router addresses
/// events to).
#[derive(Clone, Debug)]
pub struct ClientBinding {
    /// The client's session id.
    pub client: u32,
    /// The seat object this client bound.
    pub seat: ObjectId,
    /// The surface this client owns (one per binding keeps the golden
    /// corpus honest; the compositor generalizes).
    pub surface: SurfaceKey,
    /// The client's pointer object, once created.
    pub pointer: Option<ObjectId>,
    /// The client's keyboard object, once created.
    pub keyboard: Option<ObjectId>,
    /// The client's touch object, once created.
    pub touch: Option<ObjectId>,
    /// The client's tablet object, once created.
    pub tablet: Option<ObjectId>,
    /// The client's gestures object, once created.
    pub gestures: Option<ObjectId>,
}

/// The routing view of one frame: surfaces (top-first), the client
/// bindings, the shell's keyboard focus, and the output bounds.
pub struct Scene<'a> {
    /// Surfaces, topmost first.
    pub surfaces: &'a [SurfaceRef],
    /// Client bindings.
    pub clients: &'a [ClientBinding],
    /// The shell-driven keyboard focus.
    pub keyboard_focus: Option<SurfaceKey>,
    /// Output bounds (pointer clamping, touch/tablet normalization).
    pub bounds: (f32, f32),
}

impl Scene<'_> {
    /// The binding that owns a surface.
    #[must_use]
    pub fn binding_for(&self, key: SurfaceKey) -> Option<&ClientBinding> {
        self.clients.iter().find(|c| c.surface == key)
    }

    /// The surface ref for a key.
    #[must_use]
    pub fn surface(&self, key: SurfaceKey) -> Option<&SurfaceRef> {
        self.surfaces.iter().find(|s| s.key == key)
    }
}

/// Pointer-focus state (the router's only self-managed focus domain).
#[derive(Clone, Debug, Default)]
pub struct FocusRouter {
    pointer: Option<SurfaceKey>,
    /// Touch-point owners: grab semantics, one per active contact.
    touches: HashMap<i32, SurfaceKey>,
}

impl FocusRouter {
    /// Nothing focused.
    #[must_use]
    pub fn new() -> FocusRouter {
        FocusRouter::default()
    }

    /// The current pointer focus.
    #[must_use]
    pub fn pointer(&self) -> Option<SurfaceKey> {
        self.pointer
    }

    /// Set the pointer focus (returns the previous one).
    pub fn set_pointer(&mut self, key: Option<SurfaceKey>) -> Option<SurfaceKey> {
        core::mem::replace(&mut self.pointer, key)
    }

    /// The owner of a touch point, if active.
    #[must_use]
    pub fn touch(&self, id: i32) -> Option<SurfaceKey> {
        self.touches.get(&id).copied()
    }

    /// A touch point landed on `key`.
    pub fn touch_down(&mut self, id: i32, key: SurfaceKey) {
        self.touches.insert(id, key);
    }

    /// A touch point lifted; returns its owner.
    pub fn touch_up(&mut self, id: i32) -> Option<SurfaceKey> {
        self.touches.remove(&id)
    }

    /// Cancel every touch point (returns the owners, for cancel
    /// routing).
    pub fn touch_cancel_all(&mut self) -> Vec<SurfaceKey> {
        self.touches.drain().map(|(_, k)| k).collect()
    }

    /// Forget a destroyed surface from every domain (returns whether
    /// it held any focus).
    pub fn surface_gone(&mut self, key: SurfaceKey) -> bool {
        let mut held = false;
        if self.pointer == Some(key) {
            self.pointer = None;
            held = true;
        }
        let before = self.touches.len();
        self.touches.retain(|_, k| *k != key);
        held |= self.touches.len() != before;
        held
    }

    /// Every key currently owning at least one touch point.
    #[must_use]
    pub fn touch_owners(&self) -> Vec<SurfaceKey> {
        let mut v: Vec<SurfaceKey> = self.touches.values().copied().collect();
        v.sort_unstable();
        v.dedup();
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn surf(key: u64, id: u32, x: f32, y: f32, w: f32, h: f32) -> SurfaceRef {
        SurfaceRef {
            key: SurfaceKey::new(key),
            surface: ObjectId::from_wire(id),
            origin: PointF::new(x, y),
            size: (w, h),
            region: InputRegion::everywhere(),
        }
    }

    #[test]
    fn hit_test_topmost_wins() {
        let surfaces = vec![
            surf(1, 0x10, 0.0, 0.0, 100.0, 100.0),
            surf(2, 0x11, 50.0, 50.0, 100.0, 100.0),
        ];
        // Over the overlap: surface 1 (topmost).
        assert_eq!(
            hit_test(&surfaces, PointF::new(75.0, 75.0)),
            Some(SurfaceKey::new(1))
        );
        // Only over surface 2.
        assert_eq!(
            hit_test(&surfaces, PointF::new(140.0, 140.0)),
            Some(SurfaceKey::new(2))
        );
        // Over neither.
        assert_eq!(hit_test(&surfaces, PointF::new(250.0, 250.0)), None);
        // Subpixel edge: [x, x+w) semantics — 99.9 is surface 1's,
        // 100.0 is not (and y=25 misses surface 2's [50,150) band).
        assert_eq!(
            hit_test(&surfaces, PointF::new(99.9, 25.0)),
            Some(SurfaceKey::new(1))
        );
        assert_eq!(hit_test(&surfaces, PointF::new(100.0, 25.0)), None);
    }

    #[test]
    fn input_region_restricts_hits() {
        let mut s = surf(1, 0x10, 10.0, 10.0, 100.0, 100.0);
        // Only a small rect near the surface's top-left accepts input.
        s.region = InputRegion {
            rects: vec![RectF::new(0.0, 0.0, 10.0, 10.0)],
        };
        assert!(s.hit(PointF::new(15.0, 15.0)));
        assert!(!s.hit(PointF::new(25.0, 25.0)));
        // Region rect in surface coords, point in output coords.
        assert!(!s.hit(PointF::new(5.0, 5.0)));
    }

    #[test]
    fn focus_router_pointer_and_touch() {
        let mut f = FocusRouter::new();
        assert_eq!(f.pointer(), None);
        assert_eq!(f.set_pointer(Some(SurfaceKey::new(3))), None);
        assert_eq!(f.pointer(), Some(SurfaceKey::new(3)));
        assert_eq!(f.set_pointer(None), Some(SurfaceKey::new(3)));

        f.touch_down(11, SurfaceKey::new(4));
        f.touch_down(12, SurfaceKey::new(4));
        f.touch_down(13, SurfaceKey::new(5));
        assert_eq!(f.touch(11), Some(SurfaceKey::new(4)));
        assert_eq!(
            f.touch_owners(),
            vec![SurfaceKey::new(4), SurfaceKey::new(5)]
        );
        assert_eq!(f.touch_up(11), Some(SurfaceKey::new(4)));
        assert_eq!(f.touch(11), None);
        let cancelled = f.touch_cancel_all();
        assert_eq!(cancelled.len(), 2);
        assert!(f.touch_owners().is_empty());
    }

    #[test]
    fn surface_gone_clears_all_domains() {
        let mut f = FocusRouter::new();
        f.set_pointer(Some(SurfaceKey::new(7)));
        f.touch_down(1, SurfaceKey::new(7));
        f.touch_down(2, SurfaceKey::new(8));
        assert!(f.surface_gone(SurfaceKey::new(7)));
        assert_eq!(f.pointer(), None);
        assert_eq!(f.touch(1), None);
        assert_eq!(f.touch(2), Some(SurfaceKey::new(8)));
        assert!(!f.surface_gone(SurfaceKey::new(9)));
    }
}
