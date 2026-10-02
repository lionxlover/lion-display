//! Magnifier control — the `ldp.a11y.accessibility.set_magnifier`
//! state machine and lens geometry.
//!
//! The lens is a fixed-size viewport rectangle in output pixels; what
//! it shows is the content rectangle — the lens dimensions divided by
//! the Q8 scale — centered on the tracked point. Follow modes select
//! the tracking input (focus, caret, pointer, or a fixed center);
//! non-matching inputs are ignored so an active magnifier never
//! twitches when an unfollowed source moves.
//!
//! Invariants (property-tested in tests/magnifier.rs):
//!
//! * The lens always stays inside the output bounds.
//! * The content rectangle is inside the output bounds too (it is
//!   smaller than the lens for every scale above 1x, and the lens is
//!   in bounds).
//! * Scale changes keep the tracked point centered.
//! * `scale_q8` 0 keeps the current scale (the wire's "keep" sentinel).

use ldp_core::geometry::Rect;
use ldp_core::scale::ScaleFactor;

/// (width, height) of a rect as i32, saturating (Rect dimensions are
/// u32; the magnifier works on real output sizes).
fn dims_i32(r: Rect) -> (i32, i32) {
    (
        i32::try_from(r.w).unwrap_or(i32::MAX / 4),
        i32::try_from(r.h).unwrap_or(i32::MAX / 4),
    )
}

/// Whether `inner` lies fully inside `outer` (edge-inclusive) — the
/// lens-bounds invariant checker, shared with the integration suites.
#[must_use]
pub fn rect_contains(outer: Rect, inner: Rect) -> bool {
    inner.x >= outer.x
        && inner.y >= outer.y
        && inner.right() <= outer.right()
        && inner.bottom() <= outer.bottom()
}

/// What the magnifier lens tracks (wire: `ldp.a11y.magnifier_follow`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum MagnifierFollow {
    /// The focused surface's focus point.
    Focus,
    /// The text caret.
    Caret,
    /// The pointer.
    Pointer,
    /// A fixed lens center.
    Center,
}

impl MagnifierFollow {
    /// Wire value (`spec/a11y.toml`).
    #[must_use]
    pub const fn to_wire(self) -> u32 {
        match self {
            Self::Focus => 1,
            Self::Caret => 2,
            Self::Pointer => 3,
            Self::Center => 4,
        }
    }

    /// Parse a wire value.
    #[must_use]
    pub const fn from_wire(v: u32) -> Option<Self> {
        match v {
            1 => Some(Self::Focus),
            2 => Some(Self::Caret),
            3 => Some(Self::Pointer),
            4 => Some(Self::Center),
            _ => None,
        }
    }
}

/// Magnifier control failures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum MagnifierError {
    /// Scale outside 256..=4096 (1.0x..16.0x), excluding the 0 sentinel.
    ScaleRange,
}

impl std::fmt::Display for MagnifierError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ScaleRange => f.write_str("magnifier scale out of range"),
        }
    }
}

impl std::error::Error for MagnifierError {}

/// The tracked-point kinds (which input source a point update came
/// from — non-matching sources are ignored per the follow mode).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackSource {
    /// A focus-point update.
    Focus,
    /// A caret-point update.
    Caret,
    /// A pointer update.
    Pointer,
}

/// Magnifier state.
#[derive(Clone, Debug)]
pub struct Magnifier {
    enabled: bool,
    follow: MagnifierFollow,
    scale_q8: u32,
    /// Lens center in output pixel coordinates.
    center: (i32, i32),
    /// Lens viewport size (output px).
    lens: (u32, u32),
    /// Output bounds (output px).
    output: Rect,
}

impl Magnifier {
    /// A magnifier over `output` with the given lens size, disabled by
    /// default, 1x scale, centered.
    #[must_use]
    pub fn new(output: Rect, lens_w: u32, lens_h: u32) -> Self {
        let (ow, oh) = dims_i32(output);
        let cx = output.x + ow / 2;
        let cy = output.y + oh / 2;
        Magnifier {
            enabled: false,
            follow: MagnifierFollow::Focus,
            scale_q8: 256,
            center: (cx, cy),
            lens: (lens_w.max(1), lens_h.max(1)),
            output,
        }
    }

    /// The `set_magnifier` request: `scale_q8` 0 keeps the current
    /// scale (the wire sentinel).
    ///
    /// # Errors
    ///
    /// [`MagnifierError::ScaleRange`] for a nonzero scale outside
    /// 256..=4096 (the request fails; state is unchanged).
    pub fn set(
        &mut self,
        enabled: bool,
        follow: MagnifierFollow,
        scale_q8: u32,
    ) -> Result<(), MagnifierError> {
        if scale_q8 != 0 && !(256..=4096).contains(&scale_q8) {
            return Err(MagnifierError::ScaleRange);
        }
        self.enabled = enabled;
        self.follow = follow;
        if scale_q8 != 0 {
            self.scale_q8 = scale_q8;
        }
        self.clamp_center();
        Ok(())
    }

    /// Whether the magnifier is on.
    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    /// The follow mode.
    #[must_use]
    pub const fn follow(&self) -> MagnifierFollow {
        self.follow
    }

    /// The current scale (Q8).
    #[must_use]
    pub const fn scale_q8(&self) -> u32 {
        self.scale_q8
    }

    /// The tracked lens center (output px).
    #[must_use]
    pub const fn center(&self) -> (i32, i32) {
        self.center
    }

    /// One zoom step up (multiply by 4/3, clamp to 16x). Returns the
    /// new Q8 scale.
    pub fn zoom_in(&mut self) -> u32 {
        let next = (u64::from(self.scale_q8) * 4).div_ceil(3);
        self.scale_q8 = u32::try_from(next.min(4096)).unwrap_or(4096);
        self.clamp_center();
        self.scale_q8
    }

    /// One zoom step down (multiply by 3/4, clamp to 1x). Returns the
    /// new Q8 scale.
    pub fn zoom_out(&mut self) -> u32 {
        let next = (u64::from(self.scale_q8) * 3).div_ceil(4);
        self.scale_q8 = u32::try_from(next.max(256)).unwrap_or(256);
        self.clamp_center();
        self.scale_q8
    }

    /// A tracked-source point update: applied only when the follow mode
    /// matches (Center ignores all sources). Returns whether the lens
    /// moved.
    pub fn track(&mut self, source: TrackSource, x: i32, y: i32) -> bool {
        let matches = matches!(
            (self.follow, source),
            (MagnifierFollow::Focus, TrackSource::Focus)
                | (MagnifierFollow::Caret, TrackSource::Caret)
                | (MagnifierFollow::Pointer, TrackSource::Pointer)
        );
        if !matches {
            return false;
        }
        let moved = self.center != (x, y);
        self.center = (x, y);
        self.clamp_center();
        moved || self.center != (x, y)
    }

    /// The lens viewport rectangle (output px), clamped inside the
    /// output.
    #[must_use]
    pub fn lens_rect(&self) -> Rect {
        let (w, h) = self.lens_dims_i32();
        let x = self.center.0.saturating_sub(w / 2);
        let y = self.center.1.saturating_sub(h / 2);
        Rect::new(x, y, w.max(1) as u32, h.max(1) as u32)
    }

    /// The content rectangle the lens shows (output px): lens
    /// dimensions divided by the scale, centered on the tracked point.
    #[must_use]
    pub fn content_rect(&self) -> Rect {
        let scale = ScaleFactor::from_q8(self.scale_q8).unwrap_or(ScaleFactor::IDENTITY);
        let (lw, lh) = self.lens_dims_i32();
        let cw = scale.unscale_px(lw.max(0) as u32).max(1) as i32;
        let ch = scale.unscale_px(lh.max(0) as u32).max(1) as i32;
        let x = self.center.0.saturating_sub(cw / 2);
        let y = self.center.1.saturating_sub(ch / 2);
        Rect::new(x, y, cw as u32, ch as u32)
    }

    fn lens_dims_i32(&self) -> (i32, i32) {
        let (ow, oh) = dims_i32(self.output);
        (
            i32::try_from(self.lens.0)
                .unwrap_or(i32::MAX / 4)
                .min(ow.max(1)),
            i32::try_from(self.lens.1)
                .unwrap_or(i32::MAX / 4)
                .min(oh.max(1)),
        )
    }

    /// Keep the lens (and content) inside the output by moving the
    /// center, never the lens size.
    fn clamp_center(&mut self) {
        let (w, h) = self.lens_dims_i32();
        let half_w = w / 2;
        let half_h = h / 2;
        let (ow, oh) = dims_i32(self.output);
        let min_x = self.output.x.saturating_add(half_w);
        let max_x = self.output.x + ow - half_w;
        let min_y = self.output.y.saturating_add(half_h);
        let max_y = self.output.y + oh - half_h;
        self.center.0 = self.center.0.clamp(min_x, max_x.max(min_x));
        self.center.1 = self.center.1.clamp(min_y, max_y.max(min_y));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output() -> Rect {
        Rect::new(0, 0, 3840, 2160)
    }

    #[test]
    fn wire_values() {
        assert_eq!(MagnifierFollow::Focus.to_wire(), 1);
        assert_eq!(MagnifierFollow::Center.to_wire(), 4);
        assert_eq!(
            MagnifierFollow::from_wire(3),
            Some(MagnifierFollow::Pointer)
        );
        assert_eq!(MagnifierFollow::from_wire(5), None);
    }

    #[test]
    fn scale_zero_sentinel_keeps_current() {
        let mut m = Magnifier::new(output(), 800, 600);
        m.set(true, MagnifierFollow::Pointer, 512).unwrap();
        assert_eq!(m.scale_q8(), 512);
        m.set(true, MagnifierFollow::Center, 0).unwrap();
        assert_eq!(m.scale_q8(), 512);
        assert_eq!(m.follow(), MagnifierFollow::Center);
        // Out-of-range scales reject and change nothing.
        assert_eq!(
            m.set(true, MagnifierFollow::Center, 255).unwrap_err(),
            MagnifierError::ScaleRange
        );
        assert_eq!(
            m.set(true, MagnifierFollow::Center, 4097).unwrap_err(),
            MagnifierError::ScaleRange
        );
        assert_eq!(m.scale_q8(), 512);
    }

    #[test]
    fn follow_selectivity() {
        let mut m = Magnifier::new(output(), 800, 600);
        m.set(true, MagnifierFollow::Focus, 512).unwrap();
        assert!(!m.track(TrackSource::Pointer, 100, 100));
        assert!(!m.track(TrackSource::Caret, 100, 100));
        assert!(m.track(TrackSource::Focus, 1234, 567));
        assert_eq!(m.center(), (1234, 567));
        // Center mode ignores every source.
        m.set(true, MagnifierFollow::Center, 0).unwrap();
        assert!(!m.track(TrackSource::Focus, 10, 10));
        assert_eq!(m.center(), (1234, 567));
    }

    #[test]
    fn lens_and_content_stay_in_bounds() {
        let mut m = Magnifier::new(output(), 800, 600);
        m.set(true, MagnifierFollow::Pointer, 512).unwrap();
        // Track far outside: clamped back in.
        m.track(TrackSource::Pointer, -5000, -5000);
        let lens = m.lens_rect();
        let content = m.content_rect();
        assert!(rect_contains(output(), lens));
        assert!(rect_contains(output(), content));
        m.track(TrackSource::Pointer, 9000, 9000);
        assert!(rect_contains(output(), m.lens_rect()));
    }

    #[test]
    fn zoom_steps_clamp() {
        let mut m = Magnifier::new(output(), 800, 600);
        assert_eq!(m.scale_q8(), 256);
        for _ in 0..20 {
            m.zoom_in();
        }
        assert_eq!(m.scale_q8(), 4096);
        for _ in 0..30 {
            m.zoom_out();
        }
        assert_eq!(m.scale_q8(), 256);
    }
}
