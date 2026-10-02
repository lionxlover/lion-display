//! Server-side decoration geometry and its per-output scaling.
//!
//! LionOS draws the chrome by default (SSD): every `configure` carries
//! the four insets the client must reserve around its content area.
//! Clients may opt out at creation (CSD); the shell then reports the
//! *system chrome metrics* (shadow margins, hit zones) as the insets so
//! CSD apps still land on the system grid (spec `decoration_mode`).
//!
//! The mixed-DPI contract: metrics are authored in logical pixels and
//! scaled per output with ceil semantics ([`ScaleFactor::scale_px_up`])
//! — physical chrome never under-covers the logical reservation, and
//! the error against the exact rational product is strictly less than
//! one physical pixel. Content rectangles stay integral in both spaces:
//! frame = content + insets exactly.

#![forbid(unsafe_code)]

use ldp_core::scale::ScaleFactor;
use ldp_core::{geometry::Rect, limits::Limits};

/// Who draws the window chrome (wire `decoration_mode`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DecorationMode {
    /// LionOS chrome; content insets reported in configure (wire 1).
    Server,
    /// Client-decorated; system metrics (shadow, hit zones) reported
    /// as the insets to reserve (wire 2).
    Client,
}

impl DecorationMode {
    /// The wire value.
    #[must_use]
    pub const fn wire(self) -> u32 {
        match self {
            DecorationMode::Server => 1,
            DecorationMode::Client => 2,
        }
    }

    /// From the wire value (`None` on unknown codes).
    #[must_use]
    pub const fn from_wire(v: u32) -> Option<DecorationMode> {
        match v {
            1 => Some(DecorationMode::Server),
            2 => Some(DecorationMode::Client),
            _ => None,
        }
    }
}

/// Decoration metrics in logical pixels.
///
/// Server-side: the frame is a border ring plus a title bar on top;
/// a shadow margin surrounds the frame (drawn by the compositor's
/// decoration pass, reserved in the *frame* geometry so it never
/// overlaps neighboring windows' content). Client-side: the client
/// draws its own chrome, but the shadow margin and the resize hit
/// zone are still system metrics the client must reserve.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SsdMetrics {
    /// Title bar height (SSD only; 0 under CSD).
    pub title_bar: u32,
    /// Border width on all four sides (SSD only).
    pub border: u32,
    /// Shadow margin outside the frame (both modes: the compositor
    /// still draws the system shadow).
    pub shadow: u32,
    /// Resize hit-zone width outside the frame (both modes).
    pub hit_zone: u32,
}

impl SsdMetrics {
    /// The default LionOS chrome metrics (logical px, 1× baseline).
    pub const LION: SsdMetrics = SsdMetrics {
        title_bar: 28,
        border: 1,
        shadow: 8,
        hit_zone: 5,
    };

    /// Zero metrics (no chrome at all; the fullscreen case).
    pub const NONE: SsdMetrics = SsdMetrics {
        title_bar: 0,
        border: 0,
        shadow: 0,
        hit_zone: 0,
    };

    /// The insets to report in `configure` under this mode, scaled to
    /// `scale` (left, top, right, bottom).
    ///
    /// SSD: border + (title bar on top). CSD: the client draws its own
    /// chrome but reserves the system hit zone (and the shadow lives
    /// outside the frame, not in the insets).
    #[must_use]
    pub fn insets(self, mode: DecorationMode, scale: ScaleFactor) -> Insets {
        let (l, t, r, b) = match mode {
            DecorationMode::Server => {
                let side = scale.scale_px_up(self.border);
                let top = scale.scale_px_up(self.border + self.title_bar);
                (side, top, side, side)
            }
            DecorationMode::Client => {
                let hz = scale.scale_px_up(self.hit_zone);
                (hz, hz, hz, hz)
            }
        };
        Insets {
            left: l,
            top: t,
            right: r,
            bottom: b,
        }
    }

    /// The frame margin outside the *window frame* (shadow + hit
    /// zone), scaled — the space the compositor reserves around the
    /// frame so the system shadow never overlaps neighbors.
    #[must_use]
    pub fn frame_margin(self, scale: ScaleFactor) -> Insets {
        let m = scale.scale_px_up(self.shadow.max(self.hit_zone));
        Insets {
            left: m,
            top: m,
            right: m,
            bottom: m,
        }
    }
}

/// The four content insets reported by `configure` (physical device
/// pixels around the content area; positive or zero).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Insets {
    /// Left inset.
    pub left: u32,
    /// Top inset.
    pub top: u32,
    /// Right inset.
    pub right: u32,
    /// Bottom inset.
    pub bottom: u32,
}

impl Insets {
    /// Zero insets (fullscreen content covers everything).
    pub const ZERO: Insets = Insets {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };

    /// Uniform inset.
    #[must_use]
    pub const fn uniform(v: u32) -> Insets {
        Insets {
            left: v,
            top: v,
            right: v,
            bottom: v,
        }
    }

    /// Total horizontal inset (left + right).
    #[must_use]
    pub const fn width(self) -> u32 {
        self.left + self.right
    }

    /// Total vertical inset (top + bottom).
    #[must_use]
    pub const fn height(self) -> u32 {
        self.top + self.bottom
    }

    /// The frame rectangle around a content rectangle: the content
    /// area grown by the insets on each side.
    ///
    /// # Panics
    ///
    /// On debug builds when the insets overflow the content origin;
    /// the compositor clamps window positions to the output first, so
    /// this is a logic bug, not a client condition.
    #[must_use]
    pub fn frame_around(self, content: Rect) -> Rect {
        let x = content.x - self.left as i32;
        let y = content.y - self.top as i32;
        Rect::new(x, y, content.w + self.width(), content.h + self.height())
    }

    /// The content rectangle inside a frame rectangle (the inverse of
    /// [`Insets::frame_around`]).
    #[must_use]
    pub fn content_inside(self, frame: Rect) -> Rect {
        Rect::new(
            frame.x + self.left as i32,
            frame.y + self.top as i32,
            frame.w.saturating_sub(self.width()),
            frame.h.saturating_sub(self.height()),
        )
    }
}

/// Author a window title/app-id: bounded by the protocol string limit
/// (the wire codec enforces it later; the shell refuses earlier with
/// the same error class).
///
/// # Errors
/// [`ldp_core::error::ErrorCode::LimitExceeded`] when the string
/// exceeds the default [`Limits`] string budget.
pub fn bounded_string(s: &str) -> Result<Box<str>, ldp_core::error::ErrorCode> {
    let limits = Limits::default();
    if s.len() as u64 > u64::from(limits.string_bytes) {
        Err(ldp_core::error::ErrorCode::LimitExceeded)
    } else if s.contains('\0') {
        Err(ldp_core::error::ErrorCode::InvalidString)
    } else {
        Ok(s.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ldp_core::error::ErrorCode;

    fn sf(v: f32) -> ScaleFactor {
        ScaleFactor::from_f32_lossy(v).unwrap()
    }

    #[test]
    fn mode_wire_round_trip() {
        for m in [DecorationMode::Server, DecorationMode::Client] {
            assert_eq!(DecorationMode::from_wire(m.wire()), Some(m));
        }
        assert_eq!(DecorationMode::from_wire(0), None);
        assert_eq!(DecorationMode::from_wire(3), None);
    }

    #[test]
    fn ssd_insets_scale_with_ceil() {
        let m = SsdMetrics::LION;
        // 1×: exact.
        let i1 = m.insets(DecorationMode::Server, sf(1.0));
        assert_eq!((i1.left, i1.top, i1.right, i1.bottom), (1, 29, 1, 1));
        // 2×: exact doubling.
        let i2 = m.insets(DecorationMode::Server, sf(2.0));
        assert_eq!((i2.left, i2.top, i2.right, i2.bottom), (2, 58, 2, 2));
        // 1.25×: border 1.25 → 2 (ceil), top 36.25 → 37.
        let i125 = m.insets(DecorationMode::Server, sf(1.25));
        assert_eq!((i125.left, i125.right, i125.bottom), (2, 2, 2));
        assert_eq!(i125.top, 37);
    }

    #[test]
    fn insets_never_undercover_the_logical_reservation() {
        let m = SsdMetrics::LION;
        for &v in &[1.0f32, 1.25, 1.5, 1.75, 2.0, 2.5] {
            let scale = sf(v);
            let i = m.insets(DecorationMode::Server, scale);
            let logical_top = (m.border + m.title_bar) as f32 * v;
            assert!(
                i.top as f32 >= logical_top,
                "top {v}: {} < {logical_top}",
                i.top
            );
            assert!((i.top as f32) < logical_top + 1.0);
            let logical_side = m.border as f32 * v;
            assert!(i.left as f32 >= logical_side);
            assert!((i.left as f32) < logical_side + 1.0);
        }
    }

    #[test]
    fn csd_reports_hit_zone_not_chrome() {
        let m = SsdMetrics::LION;
        let i = m.insets(DecorationMode::Client, sf(1.0));
        assert_eq!((i.left, i.top, i.right, i.bottom), (5, 5, 5, 5));
        let i2 = m.insets(DecorationMode::Client, sf(2.0));
        assert_eq!((i2.left, i2.top, i2.right, i2.bottom), (10, 10, 10, 10));
    }

    #[test]
    fn frame_content_round_trip_is_exact() {
        let i = Insets {
            left: 2,
            top: 58,
            right: 2,
            bottom: 2,
        };
        let content = Rect::new(100, 100, 800, 600);
        let frame = i.frame_around(content);
        assert_eq!(frame, Rect::new(98, 42, 804, 660));
        assert_eq!(i.content_inside(frame), content);
        // Degenerate: frame smaller than insets saturates at zero.
        let tiny = Rect::new(0, 0, 2, 2);
        assert_eq!(i.content_inside(tiny), Rect::new(2, 58, 0, 0));
    }

    #[test]
    fn frame_margin_is_the_larger_of_shadow_and_hit_zone() {
        let m = SsdMetrics {
            title_bar: 28,
            border: 1,
            shadow: 8,
            hit_zone: 5,
        };
        let fm = m.frame_margin(sf(1.0));
        assert_eq!((fm.left, fm.top, fm.right, fm.bottom), (8, 8, 8, 8));
        // Hit zone dominating.
        let m2 = SsdMetrics { hit_zone: 12, ..m };
        assert_eq!(m2.frame_margin(sf(1.0)).top, 12);
    }

    #[test]
    fn bounded_string_enforces_the_protocol_budget() {
        assert_eq!(&*bounded_string("Editor").unwrap(), "Editor");
        let too_long = "x".repeat(4097);
        assert_eq!(bounded_string(&too_long), Err(ErrorCode::LimitExceeded));
        assert_eq!(bounded_string("a\0b"), Err(ErrorCode::InvalidString));
        let exact = "x".repeat(4096);
        assert!(bounded_string(&exact).is_ok());
    }
}
