//! LDP font layer: the compositor's own typeface and rasterizer.
//!
//! [`LION_SANS`] is a utilitarian geometric sans authored directly in
//! em units — the working face a title bar wears. [`Face::layout`]
//! shapes a left-to-right run with pixel-true truncation (the
//! ellipsis glyph), and [`Face::bitmap`] rasterizes one glyph to
//! 8-bit analytic coverage (the raster module's exact area
//! integration).
//!
//! The layer's doctrine (Phase 54, the title glyph):
//!
//! * **Deterministic**: no hinting, no environment, no caches — the
//!   same text, pixel size, and width budget always yield
//!   byte-identical ink. The compositor caches; the face never does.
//! * **Honest coverage**: ASCII plus the ellipsis plus nbsp; every
//!   other codepoint draws the notdef box (visible truth, never a
//!   crash, never silent garbage). Control characters draw blanks —
//!   their advance is real, their ink is none.
//! * **Zero dependencies**: a display server that carries its own
//!   face never rides a system font lottery — no fontconfig
//!   discovery, no fallback waterfall, no per-user font breaking the
//!   title bar.
//! * **Pure placement**: glyph positions round independently from
//!   exact em-space pen positions — no accumulated drift, ever.

#![forbid(unsafe_code)]

mod outline;
mod raster;

use outline::Glyph;

/// One layout resolution: a real glyph (carrying its original char)
/// or a control character's blank.
enum Resolved {
    /// A control character: real advance, no ink.
    Blank,
    /// A covered or notdef glyph with its source char.
    Glyph(char, Glyph),
}

/// The Lion Sans typeface: 96 glyphs + notdef, em 1000, cap 720,
/// x-height 500, ascender 800, descender −200.
pub static LION_SANS: Face = Face;

/// One typeface. A zero-sized handle over the static outline table —
/// the identity of every answer derives from the table alone.
#[derive(Debug, Clone, Copy)]
pub struct Face;

/// The face's metrics at one pixel size (all values in device px).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FaceMetrics {
    /// The cap height above the baseline.
    pub cap: i32,
    /// The x-height above the baseline.
    pub x_height: i32,
    /// The ascender (the line box's top).
    pub ascent: i32,
    /// The descender (the line box's bottom, negative or zero).
    pub descent: i32,
}

/// One placed glyph in a run: the codepoint and its pen position in
/// device px (the position the glyph's *origin* sits at; the ink
/// extends per [`Face::bitmap`]'s bearing contract).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    /// The character drawn (the covered set's codepoint; `'\u{0}'`
    /// marks a blank — real advance, no ink).
    pub c: char,
    /// The pen x in device px from the run's origin.
    pub x: i32,
}

/// The ink box of a run, in device px: the tight rectangle the ink
/// can occupy (from the outline bounds, the exact upper bound of the
/// rasterized ink).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InkBox {
    /// The left edge from the run's origin.
    pub min_x: i32,
    /// The right edge from the run's origin.
    pub max_x: i32,
    /// The top edge above the baseline (positive).
    pub top: i32,
    /// The bottom edge below the baseline (negative or zero).
    pub bottom: i32,
}

impl InkBox {
    /// The width in px.
    #[must_use]
    pub fn width(self) -> u32 {
        (self.max_x - self.min_x).max(0) as u32
    }

    /// The height in px.
    #[must_use]
    pub fn height(self) -> u32 {
        (self.top - self.bottom).max(0) as u32
    }
}

/// One shaped run: the placed glyphs, the total advance, the ink box,
/// and whether the width budget forced the ellipsis truncation.
#[derive(Clone, Debug)]
pub struct Run {
    /// The placed glyphs, in drawing order.
    pub placed: Vec<Placement>,
    /// The total advance width (px).
    pub width: u32,
    /// The ink box (px); `None` when the run draws no ink.
    pub ink: Option<InkBox>,
    /// Whether the ellipsis truncation applied.
    pub truncated: bool,
}

/// One rasterized glyph: the coverage bitmap and its placement
/// contract — the bitmap's top-left pixel lands at
/// `(pen_x + x_off, baseline_row − y_top)`.
#[derive(Clone, Debug)]
pub struct GlyphBitmap {
    /// The bitmap's width.
    pub w: u32,
    /// The bitmap's height.
    pub h: u32,
    /// The left bearing from the pen.
    pub x_off: i32,
    /// The rows the bitmap's top sits above the baseline.
    pub y_top: i32,
    /// The coverage, row-major `w · h` bytes.
    pub alpha: Vec<u8>,
}

impl Face {
    /// The face's metrics at one pixel size.
    #[must_use]
    pub fn metrics(&self, px: u32) -> FaceMetrics {
        let s = |v: i32| (f64::from(v) * f64::from(px) / 1000.0).round() as i32;
        FaceMetrics {
            cap: s(outline::CAP),
            x_height: s(outline::X_HEIGHT),
            ascent: s(outline::ASCENT),
            descent: s(outline::DESCENT),
        }
    }

    /// Shape one left-to-right run at `px` device pixels under a
    /// `max_w` width budget: the full run when it fits, the longest
    /// prefix plus the ellipsis glyph when it does not (pixel-true —
    /// the decision rides the scaled advances, never a character
    /// count).
    ///
    /// Control characters (C0) draw as blanks: real advances, no ink.
    /// Uncovered codepoints draw the notdef box. An empty run — or
    /// one of blanks alone — carries `ink: None`.
    ///
    /// # Panics
    ///
    /// Never in-crate: the truncation's ellipsis comes from the
    /// covered table (the static outline owns it — a build without it
    /// is a build error, not a runtime path).
    #[must_use]
    pub fn layout(&self, text: &str, px: u32, max_w: u32) -> Run {
        // Resolve every char: the glyph (or the blank/notdef stand-in)
        // and its em advance. The placement carries the *original*
        // char — `bitmap(c)` round-trips (the notdef for the
        // uncovered, the blank for controls).
        let blank_advance = lookup_advance(' ').unwrap_or(260);
        let chars: Vec<char> = text.chars().collect();
        let resolved: Vec<(Resolved, i32)> = chars
            .iter()
            .map(|&c| {
                if (c as u32) < 0x20 {
                    (Resolved::Blank, blank_advance)
                } else {
                    match outline::lookup(c) {
                        Some(g) => {
                            let adv = g.advance;
                            (Resolved::Glyph(c, g), adv)
                        }
                        None => (
                            Resolved::Glyph(c, outline::notdef()),
                            outline::notdef().advance,
                        ),
                    }
                }
            })
            .collect();
        // The em-space pen (exact integers — no drift) and the scaled
        // width of one em prefix.
        let scale = |em: i32| -> i32 { ((i64::from(em) * i64::from(px) + 500) / 1000) as i32 };
        let total_em: i32 = resolved.iter().map(|(_, a)| a).sum();
        if resolved.is_empty() {
            return Run {
                placed: Vec::new(),
                width: 0,
                ink: None,
                truncated: false,
            };
        }
        if u32::try_from(scale(total_em)).unwrap_or(u32::MAX) <= max_w {
            return Self::run_of(&resolved, px, false);
        }
        // Truncation: the longest prefix whose width plus the
        // ellipsis's fits, the ellipsis appended after it.
        let ell = outline::lookup('\u{2026}').expect("the ellipsis is covered");
        let ell_w = scale(ell.advance);
        let mut kept_em: i32 = 0;
        let mut kept = 0usize;
        for (i, (_, adv)) in resolved.iter().enumerate() {
            let next = kept_em + adv;
            if u32::try_from(scale(next))
                .unwrap_or(u32::MAX)
                .saturating_add(ell_w.max(0) as u32)
                > max_w
            {
                break;
            }
            kept_em = next;
            kept = i + 1;
        }
        let mut trunc: Vec<(Resolved, i32)> = resolved.into_iter().take(kept).collect();
        let ell_adv = ell.advance;
        trunc.push((Resolved::Glyph('\u{2026}', ell), ell_adv));
        Self::run_of(&trunc, px, true)
    }

    /// Rasterize one character at `px`: the covered glyph, the notdef
    /// box for the uncovered, blank (empty) for control characters.
    #[must_use]
    pub fn bitmap(&self, c: char, px: u32) -> GlyphBitmap {
        let g = if (c as u32) < 0x20 {
            return GlyphBitmap {
                w: 0,
                h: 0,
                x_off: 0,
                y_top: 0,
                alpha: Vec::new(),
            };
        } else {
            outline::lookup(c).unwrap_or_else(outline::notdef)
        };
        let b = raster::rasterize(&g.contours, f64::from(px) as f32 / 1000.0);
        GlyphBitmap {
            w: b.w,
            h: b.h,
            x_off: b.x_off,
            y_top: b.y_top,
            alpha: b.alpha,
        }
    }

    /// Build the placed run from resolved glyphs: the pen walks em
    /// space exactly, each glyph's x rounds independently.
    fn run_of(resolved: &[(Resolved, i32)], px: u32, truncated: bool) -> Run {
        let scale = |em: i32| -> i32 { ((i64::from(em) * i64::from(px) + 500) / 1000) as i32 };
        let mut placed: Vec<Placement> = Vec::new();
        let mut ink: Option<InkBox> = None;
        let mut pen_em: i32 = 0;
        for (res, adv) in resolved {
            let x_px = scale(pen_em);
            let c = match res {
                Resolved::Blank => '\u{0}',
                Resolved::Glyph(c, g) => {
                    // The ink box: this glyph's outline bounds at the
                    // placed position.
                    let b = raster::rasterize(&g.contours, f64::from(px) as f32 / 1000.0);
                    if b.w > 0 && b.h > 0 {
                        let left = x_px + b.x_off;
                        let right = left + b.w as i32;
                        let top = b.y_top;
                        let bottom = b.y_top - b.h as i32;
                        ink = Some(match ink {
                            None => InkBox {
                                min_x: left,
                                max_x: right,
                                top,
                                bottom,
                            },
                            Some(mut b2) => {
                                b2.min_x = b2.min_x.min(left);
                                b2.max_x = b2.max_x.max(right);
                                b2.top = b2.top.max(top);
                                b2.bottom = b2.bottom.min(bottom);
                                b2
                            }
                        });
                    }
                    *c
                }
            };
            placed.push(Placement { c, x: x_px });
            pen_em += adv;
        }
        Run {
            placed,
            width: scale(pen_em).max(0) as u32,
            ink,
            truncated,
        }
    }
}

/// One glyph's advance by its char (the layout's blank fallback).
fn lookup_advance(c: char) -> Option<i32> {
    outline::lookup(c).map(|g| g.advance)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_metrics_scale_with_px() {
        let m = LION_SANS.metrics(16);
        assert_eq!(m.cap, 12); // 720 · 16 / 1000 = 11.52 → 12
        assert_eq!(m.x_height, 8);
        assert_eq!(m.descent, -3); // −200 · 16 / 1000 = −3.2 → −3
        let m = LION_SANS.metrics(32);
        assert_eq!(m.cap, 23); // 23.04 → 23
    }

    #[test]
    fn the_run_width_is_the_scaled_advance_sum() {
        // "ii" = 280 + 280 em at 16 px = 8.96 px.
        let r = LION_SANS.layout("ii", 16, 1000);
        assert_eq!(r.width, 9);
        assert!(!r.truncated);
        assert_eq!(r.placed.len(), 2);
        assert_eq!(r.placed[0].x, 0);
        assert_eq!(r.placed[1].x, 4); // 280 em · 16 / 1000 = 4.48 → 4
    }

    #[test]
    fn the_empty_run_has_no_ink() {
        let r = LION_SANS.layout("", 16, 100);
        assert_eq!(r.width, 0);
        assert!(r.ink.is_none());
        let r = LION_SANS.layout("   ", 16, 100);
        assert_eq!(r.placed.len(), 3);
        assert!(r.ink.is_none(), "blanks carry no ink box");
    }

    #[test]
    fn the_truncation_is_pixel_true() {
        // "Image Viewer" at 16 px: the full width vs a budget one px
        // under — the ellipsis rides, the width stays within budget.
        let full = LION_SANS.layout("Image Viewer", 16, 10_000);
        let w = full.width;
        assert!(!full.truncated);
        let cut = LION_SANS.layout("Image Viewer", 16, w.saturating_sub(1).max(1));
        assert!(cut.truncated, "one px under the full width truncates");
        assert!(cut.width <= w.saturating_sub(1));
        // The truncated run ends with the ellipsis glyph and keeps
        // the longest fitting prefix.
        assert!(cut.placed.len() >= 2);
        // Exactly at the width: no truncation.
        let exact = LION_SANS.layout("Image Viewer", 16, w);
        assert!(!exact.truncated);
    }

    #[test]
    fn the_very_tight_budget_keeps_the_ellipsis_alone() {
        // A budget too small even for a prefix + ellipsis: the
        // ellipsis alone (the run may exceed the budget by the
        // ellipsis's own overhang — the honest minimum, never an
        // empty drawing).
        let cut = LION_SANS.layout("Constantinople", 16, 4);
        assert!(cut.truncated);
        assert_eq!(cut.placed.len(), 1, "the ellipsis alone");
        assert!(cut.width <= 16);
    }

    #[test]
    fn the_notdef_and_blank_doctrines() {
        // Uncovered: the notdef box inks.
        let b = LION_SANS.bitmap('é', 16);
        assert!(b.w > 0 && b.h > 0);
        assert!(b.alpha.iter().any(|&a| a > 0));
        // Control: blank.
        let b = LION_SANS.bitmap('\n', 16);
        assert_eq!(b.w, 0);
        // The layout maps them likewise.
        let r = LION_SANS.layout("a\tb", 16, 10_000);
        assert_eq!(r.placed.len(), 3);
        assert_eq!(r.placed[1].c, '\u{0}', "the control char's blank label");
        let r2 = LION_SANS.layout("aéb", 16, 10_000);
        assert_eq!(r2.placed.len(), 3);
        assert!(r2.placed[1].c != 'b');
    }

    #[test]
    fn determinism_is_byte_exact() {
        let a = LION_SANS.layout("The Quick Brown Fox", 16, 500);
        let b = LION_SANS.layout("The Quick Brown Fox", 16, 500);
        assert_eq!(a.width, b.width);
        assert_eq!(a.truncated, b.truncated);
        assert_eq!(a.placed, b.placed);
        let g1 = LION_SANS.bitmap('R', 16);
        let g2 = LION_SANS.bitmap('R', 16);
        assert_eq!(g1.alpha, g2.alpha);
    }

    #[test]
    fn the_scale_sweep_stays_sane() {
        for px in [8u32, 12, 16, 24, 32, 48, 64] {
            for text in ["O", "m", "8", "The Title"] {
                let r = LION_SANS.layout(text, px, 100_000);
                assert!(r.width > 0);
                if let Some(ink) = r.ink {
                    assert!(ink.width() > 0);
                    // The ink box's outward quantization may poke one
                    // pixel past the rounded metrics (the i's dot at
                    // 780 em against an ascent rounded down).
                    assert!(ink.top <= LION_SANS.metrics(px).ascent + 1);
                    assert!(ink.bottom >= LION_SANS.metrics(px).descent - 1);
                }
            }
        }
    }

    #[test]
    fn the_ink_box_bounds_the_rasterized_ink() {
        // Every placed glyph's bitmap lands inside the run's ink box
        // (the box is the union of the bitmap extents).
        let r = LION_SANS.layout("Water", 16, 10_000);
        let ink = r.ink.expect("ink");
        for p in &r.placed {
            let b = LION_SANS.bitmap(p.c, 16);
            if b.w == 0 {
                continue;
            }
            let left = p.x + b.x_off;
            let right = left + b.w as i32;
            let top = b.y_top;
            let bottom = b.y_top - b.h as i32;
            assert!(left >= ink.min_x && right <= ink.max_x);
            assert!(top <= ink.top && bottom >= ink.bottom);
        }
    }
}
