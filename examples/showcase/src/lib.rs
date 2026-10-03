//! # showcase — a colorful client-authored scene
//!
//! The hello-ldp choreography with the pixel half turned up: a
//! 960×540 wallpaper surface (a sunset sky gradient, a rising sun, a
//! fading star field, drifting clouds, block lettering, a format
//! palette) plus — since Phase 27 — a translucent **glass panel**: a
//! full-width ARGB bar the compositor dresses with the Liquid
//! material (rounded corners, the frosted backdrop, the soft shadow)
//! while a critically-damped spring slides its app pills in — the
//! macOS feel, client-driven through the presentation loop exactly
//! the way an iOS panel animates. Every frame is written into the shm
//! pools, committed through the real pipeline, and confirmed by the
//! compositor's presentation verdicts.
//!
//! When the last line reads `showcase: 4 frame(s) presented`, the
//! full stack — client library, wire codec, transport with FD
//! passing, server dispatch, scene graph, scheduler, renderer,
//! scanout — has carried four client-authored animations end to end.
//!
//! ```text
//! showcase [--socket NAME] [--hold SECS]
//! ```

#![forbid(unsafe_code)]

use std::io::Write;

use ldp_compositor::spring::Spring;
use ldp_core::geometry::Rect;
use ldp_tools::error::{Result, ToolError};
use ldp_tools::session::{FrameCycle, ToolSession, FORMAT_ARGB8888};
use ldp_transport::UnixAddr;

/// Scene width (px).
pub const W: u32 = 960;
/// Scene height (px).
pub const H: u32 = 540;
/// Frames in the animation (the presentation count to reach).
pub const FRAMES: u32 = 4;

/// The glass panel: full width, 84 px tall (the phone's material bar).
pub const PANEL_H: u32 = 84;
/// The app pills' resting slots.
const PILL_TARGETS: [f32; 5] = [48.0, 148.0, 248.0, 348.0, 448.0];
/// Where the pills spring from (off the left edge).
const PILL_FROM: f32 = -72.0;
/// The frame cadence the presentation verdicts report (60 Hz).
const FRAME_MS: u64 = 1000 / 60;
/// The horizon line: sky above, water below (px).
const HORIZON: u32 = 430;
/// The sun's radius (px).
const SUN_R: i32 = 70;
/// The sun's horizontal center (px).
const SUN_X: i32 = 480;

/// An RGB color (one byte per channel).
type Rgb = [u8; 3];

/// A 5×7 bitmap glyph: one byte per cell, nonzero means ink.
type Glyph = [[u8; 5]; 7];

/// Integer lerp: `a` toward `b` by `t/d`.
fn lerp(a: u8, b: u8, t: u32, d: u32) -> u8 {
    let d = d.max(1);
    let a32 = u32::from(a);
    let b32 = u32::from(b);
    u8::try_from((a32 * (d - t.min(d)) + b32 * t) / d).unwrap_or(u8::MAX)
}

/// Per-channel integer mix of two colors by `t/d`.
fn mix(c0: Rgb, c1: Rgb, t: u32, d: u32) -> Rgb {
    [
        lerp(c0[0], c1[0], t, d),
        lerp(c0[1], c1[1], t, d),
        lerp(c0[2], c1[2], t, d),
    ]
}

/// Multiply a color toward black by `num/den`.
fn darken(c: Rgb, num: u32, den: u32) -> Rgb {
    let den = den.max(1);
    [
        u8::try_from(u32::from(c[0]) * num / den).unwrap_or(0),
        u8::try_from(u32::from(c[1]) * num / den).unwrap_or(0),
        u8::try_from(u32::from(c[2]) * num / den).unwrap_or(0),
    ]
}

/// Write one pixel (bounds-checked; out-of-range is a no-op).
fn put(buf: &mut [u8], x: i64, y: i64, c: Rgb) {
    if x < 0 || y < 0 || x >= i64::from(W) || y >= i64::from(H) {
        return;
    }
    let at = (y * i64::from(W) + x) * 3;
    buf[at as usize..at as usize + 3].copy_from_slice(&c);
}

/// Read one pixel (bounds-checked; out-of-range reads black).
fn get(buf: &[u8], x: i64, y: i64) -> Rgb {
    if x < 0 || y < 0 || x >= i64::from(W) || y >= i64::from(H) {
        return [0, 0, 0];
    }
    let at = (y * i64::from(W) + x) * 3;
    [buf[at as usize], buf[at as usize + 1], buf[at as usize + 2]]
}

/// A filled circle with a radial core→edge gradient (clipped above
/// `y_limit` when given — the sun never draws under the water line).
fn fill_circle(
    buf: &mut [u8],
    cx: i64,
    cy: i64,
    r: i32,
    core: Rgb,
    edge: Rgb,
    y_limit: Option<i64>,
) {
    let rr = i64::from(r) * i64::from(r);
    for dy in -i64::from(r)..=i64::from(r) {
        let y = cy + dy;
        if let Some(limit) = y_limit {
            if y >= limit {
                continue;
            }
        }
        for dx in -i64::from(r)..=i64::from(r) {
            let d2 = dx * dx + dy * dy;
            if d2 > rr {
                continue;
            }
            let c = mix(core, edge, d2 as u32, rr as u32);
            put(buf, cx + dx, y, c);
        }
    }
}

/// The 5×7 block font: uppercase L, D, P, V, digits 0 and 1, the
/// lowercase v, and the period.
fn glyph(ch: char) -> Option<Glyph> {
    let g = match ch {
        'L' => [
            [1, 0, 0, 0, 0],
            [1, 0, 0, 0, 0],
            [1, 0, 0, 0, 0],
            [1, 0, 0, 0, 0],
            [1, 0, 0, 0, 0],
            [1, 0, 0, 0, 0],
            [1, 1, 1, 1, 1],
        ],
        'D' => [
            [1, 1, 1, 0, 0],
            [1, 0, 0, 1, 0],
            [1, 0, 0, 0, 1],
            [1, 0, 0, 0, 1],
            [1, 0, 0, 0, 1],
            [1, 0, 0, 1, 0],
            [1, 1, 1, 0, 0],
        ],
        'P' => [
            [1, 1, 1, 1, 0],
            [1, 0, 0, 0, 1],
            [1, 0, 0, 0, 1],
            [1, 1, 1, 1, 0],
            [1, 0, 0, 0, 0],
            [1, 0, 0, 0, 0],
            [1, 0, 0, 0, 0],
        ],
        'V' => [
            [1, 0, 0, 0, 1],
            [1, 0, 0, 0, 1],
            [1, 0, 0, 0, 1],
            [1, 0, 0, 0, 1],
            [1, 0, 0, 0, 1],
            [0, 1, 0, 1, 0],
            [0, 0, 1, 0, 0],
        ],
        'v' => [
            [0, 0, 0, 0, 0],
            [0, 0, 0, 0, 0],
            [1, 0, 0, 0, 1],
            [1, 0, 0, 0, 1],
            [1, 0, 0, 0, 1],
            [0, 1, 0, 1, 0],
            [0, 0, 1, 0, 0],
        ],
        '0' => [
            [0, 1, 1, 1, 0],
            [1, 0, 0, 0, 1],
            [1, 0, 0, 1, 1],
            [1, 0, 1, 0, 1],
            [1, 1, 0, 0, 1],
            [1, 0, 0, 0, 1],
            [0, 1, 1, 1, 0],
        ],
        '1' => [
            [0, 0, 1, 0, 0],
            [0, 1, 1, 0, 0],
            [0, 0, 1, 0, 0],
            [0, 0, 1, 0, 0],
            [0, 0, 1, 0, 0],
            [0, 0, 1, 0, 0],
            [0, 1, 1, 1, 0],
        ],
        '.' => [
            [0, 0, 0, 0, 0],
            [0, 0, 0, 0, 0],
            [0, 0, 0, 0, 0],
            [0, 0, 0, 0, 0],
            [0, 0, 0, 0, 0],
            [0, 0, 1, 1, 0],
            [0, 0, 1, 1, 0],
        ],
        _ => return None,
    };
    Some(g)
}

/// Draw `text` at `(x, y)` in `scale`-sized block glyphs with a drop
/// shadow; unsupported characters advance one cell.
fn draw_text(buf: &mut [u8], x: i64, y: i64, scale: i64, text: &str, color: Rgb) {
    let shadow: Rgb = [14, 10, 36];
    let offset = 1 + scale / 4;
    let mut cx = x;
    for ch in text.chars() {
        if let Some(g) = glyph(ch) {
            for (gy, row) in g.iter().enumerate() {
                for (gx, cell) in row.iter().enumerate() {
                    if *cell == 0 {
                        continue;
                    }
                    let px = cx + i64::from(gx as u32) * scale;
                    let py = y + i64::from(gy as u32) * scale;
                    for sy in 0..scale {
                        for sx in 0..scale {
                            put(buf, px + sx + offset, py + sy + offset, shadow);
                        }
                    }
                }
            }
            for (gy, row) in g.iter().enumerate() {
                for (gx, cell) in row.iter().enumerate() {
                    if *cell == 0 {
                        continue;
                    }
                    let px = cx + i64::from(gx as u32) * scale;
                    let py = y + i64::from(gy as u32) * scale;
                    for sy in 0..scale {
                        for sx in 0..scale {
                            put(buf, px + sx, py + sy, color);
                        }
                    }
                }
            }
        }
        cx += 6 * scale;
    }
}

/// One step of the deterministic star-field generator (SplitMix64).
fn star_next(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A horizontal capsule (rounded bar) — a cloud body.
fn capsule(buf: &mut [u8], x0: i64, y0: i64, w: i64, h: i64, c: Rgb) {
    let r = h / 2;
    if r < 1 {
        return;
    }
    for dy in 0..h {
        for dx in 0..w {
            let ex = if dx < r {
                r - dx
            } else if dx > w - 1 - r {
                dx - (w - 1 - r)
            } else {
                0
            };
            let ey = if dy < r {
                r - dy
            } else if dy > h - 1 - r {
                dy - (h - 1 - r)
            } else {
                0
            };
            if ex > 0 && ey > 0 && ex * ex + ey * ey > r * r {
                continue;
            }
            put(buf, x0 + dx, y0 + dy, c);
        }
    }
}

/// The sky gradient per frame: pre-dawn navy through sunrise orange.
fn sky_palette(frame: u32) -> (Rgb, Rgb) {
    match frame {
        0 => ([6, 8, 26], [22, 18, 54]),
        1 => ([26, 12, 62], [110, 36, 86]),
        2 => ([52, 18, 100], [214, 98, 58]),
        _ => ([84, 36, 132], [255, 158, 72]),
    }
}

/// The sun's vertical center per frame: it rises through the horizon.
fn sun_y(frame: u32) -> i64 {
    match frame {
        0 => 566,
        1 => 512,
        2 => 478,
        _ => i64::from(HORIZON) - 6,
    }
}

/// The star brightness per frame: the stars fade as day arrives.
fn star_level(frame: u32) -> u32 {
    match frame {
        0 => 250,
        1 => 190,
        2 => 90,
        _ => 0,
    }
}

/// The block-letter baseline per frame: the title rises with the sun.
fn title_y(frame: u32) -> i64 {
    match frame {
        0 => 292,
        1 => 240,
        2 => 204,
        _ => 184,
    }
}

/// The animated color palette shown on the final frame.
const PALETTE: [Rgb; 12] = [
    [255, 68, 68],
    [255, 140, 46],
    [255, 196, 0],
    [186, 219, 0],
    [104, 220, 56],
    [24, 208, 116],
    [20, 200, 200],
    [36, 156, 255],
    [84, 96, 255],
    [150, 72, 255],
    [230, 60, 200],
    [255, 62, 132],
];

/// Paint the sky gradient and the fading star field (above the
/// horizon only).
fn render_sky(buf: &mut [u8], frame: u32, top: Rgb, bottom: Rgb) {
    // The sky: a smooth vertical gradient to the horizon.
    for y in 0..HORIZON {
        let c = mix(top, bottom, y, HORIZON);
        for x in 0..W {
            put(buf, i64::from(x), i64::from(y), c);
        }
    }

    // The star field: 90 deterministic stars, fading with the dawn.
    let level = star_level(frame);
    if level == 0 {
        return;
    }
    let mut state: u64 = 0x1234_5678_9ABC_DEF0;
    for _ in 0..90 {
        state = star_next(&mut state);
        let x = i64::try_from(state % u64::from(W)).unwrap_or(0);
        let y = i64::try_from(state % 360).unwrap_or(0);
        put(
            buf,
            x,
            y,
            [
                level as u8,
                level as u8,
                u8::try_from(level + 24).unwrap_or(u8::MAX),
            ],
        );
    }
}

/// Paint the sun's halo and disc, both clipped at the water line.
fn render_sun(buf: &mut [u8], frame: u32) {
    let sy = sun_y(frame);
    let glow = 3 * SUN_R;
    for dy in -i64::from(glow)..=i64::from(glow) {
        let y = sy + dy;
        if y < 0 || y >= i64::from(HORIZON) {
            continue;
        }
        for dx in -i64::from(glow)..=i64::from(glow) {
            let d2 = dx * dx + dy * dy;
            let g2 = i64::from(glow) * i64::from(glow);
            if d2 >= g2 {
                continue;
            }
            let fade = (g2 - d2) as u32 * 140 / g2 as u32;
            let here = get(buf, i64::from(SUN_X) + dx, y);
            let warmed = mix(here, [255, 150, 70], fade, 100);
            put(buf, i64::from(SUN_X) + dx, y, warmed);
        }
    }
    fill_circle(
        buf,
        i64::from(SUN_X),
        sy,
        SUN_R,
        [255, 244, 176],
        [255, 120, 40],
        Some(i64::from(HORIZON)),
    );
}

/// Paint the water: a dark mirror of the sky with the sun's
/// shimmering reflection column.
fn render_water(buf: &mut [u8], bottom: Rgb) {
    for y in HORIZON..H {
        let depth = y - HORIZON;
        let base = darken(bottom, 12 - depth.min(8), 20);
        for x in 0..W {
            put(buf, i64::from(x), i64::from(y), base);
        }
        // The shimmering reflection column under the sun.
        let shimmer = if (y / 3) % 2 == 0 { 120 } else { 70 };
        for dx in -110i64..110 {
            let x = i64::from(SUN_X) + dx;
            let ax = dx.unsigned_abs() as i64;
            if ax >= 110 {
                continue;
            }
            let strength = (110 - ax) as u32 * shimmer / 110;
            let here = get(buf, x, i64::from(y));
            let lit = mix(here, [255, 170, 90], strength, 100);
            put(buf, x, i64::from(y), lit);
        }
    }
}

/// Paint the two clouds drifting east as the frames advance.
fn render_clouds(buf: &mut [u8], frame: u32, top: Rgb, bottom: Rgb) {
    for (cx, cy, wide) in [(170, 96, 130), (640, 150, 170)] {
        let drift = i64::from(frame) * 26;
        let sky_here = mix(top, bottom, u32::try_from(cy).unwrap_or(0), HORIZON);
        let cloud = mix([236, 240, 250], sky_here, 42, 100);
        let x0 = cx + drift;
        let half = wide / 2;
        capsule(buf, x0 - half, cy - 12, wide, 24, cloud);
        fill_circle(buf, x0 - half / 3, cy - 4, 14, cloud, cloud, None);
        fill_circle(buf, x0 + half / 4, cy - 2, 11, cloud, cloud, None);
    }
}

/// Paint the overlays: the rising title, the subtitle on frame three
/// onward, the palette strip on the final frame, and the border.
fn render_overlays(buf: &mut [u8], frame: u32) {
    // The title rises with the sun; the subtitle lands on the third.
    draw_text(buf, 372, title_y(frame), 12, "LDP", [252, 252, 255]);
    if frame >= 2 {
        draw_text(
            buf,
            402,
            title_y(frame) + 108,
            5,
            "v0.25.0",
            [230, 236, 255],
        );
    }

    // The palette strip lands on the final frame.
    if frame + 1 == FRAMES {
        for (i, c) in PALETTE.iter().enumerate() {
            let x0 = 36 + i as i64 * 76;
            for dy in 0..16 {
                for dx in 0..72 {
                    put(buf, x0 + dx, 506 + dy, *c);
                }
            }
        }
    }

    // A one-pixel inset border frames every scene.
    let border: Rgb = [70, 66, 110];
    for x in 0..i64::from(W) {
        put(buf, x, 0, border);
        put(buf, x, i64::from(H) - 1, border);
    }
    for y in 0..i64::from(H) {
        put(buf, 0, y, border);
        put(buf, i64::from(W) - 1, y, border);
    }
}

/// Render frame `frame` (0-based, [`FRAMES`] total) as packed RGB24.
///
/// The scene is deterministic: the same frame number always renders
/// the same pixels (the scenario test asserts scanout equality).
#[must_use]
/// The panel's ARGB8888 premultiplied words for one frame: a light
/// haze the compositor frosts, plus the spring-driven app pills.
///
/// The spring is the Phase 27 engine — critically damped, stepped at
/// the presentation cadence — so the pills glide in with the macOS
/// feel and settle by the final frame.
pub fn render_panel(frame: u32) -> Vec<u32> {
    // One shared spring shape per pill, phase-shifted by the slot.
    let mut pills: Vec<Spring> = PILL_TARGETS
        .iter()
        .map(|&t| {
            let mut s = Spring::critically_damped(t, 9000.0);
            s.displace(PILL_FROM);
            s
        })
        .collect();
    for _ in 0..frame {
        for s in &mut pills {
            s.step(FRAME_MS);
        }
    }
    let mut buf = vec![0x3C3C_3C3Cu32; (W as usize) * (PANEL_H as usize)];
    for s in &pills {
        let x = s.pos.round() as i64;
        let y = 18i64;
        // The pill: a bright capsule over the haze.
        capsule_words(&mut buf, x, y, 64, 48, [225, 228, 235], 235);
    }
    buf
}

/// A rounded capsule over the panel's premultiplied word buffer.
fn capsule_words(buf: &mut [u32], x0: i64, y0: i64, w: i64, h: i64, rgb: [u8; 3], alpha: u8) {
    let prem = [
        mul8(rgb[0], alpha),
        mul8(rgb[1], alpha),
        mul8(rgb[2], alpha),
        alpha,
    ];
    let word = u32::from(prem[3]) << 24
        | u32::from(prem[0]) << 16
        | u32::from(prem[1]) << 8
        | u32::from(prem[2]);
    let r = h / 2;
    let in_cap =
        |x: i64, y: i64, cx: i64, cy: i64| (x - cx) * (x - cx) + (y - cy) * (y - cy) <= r * r;
    for y in y0..y0 + h {
        for x in x0..x0 + w {
            let inside = if y >= y0 + r && y < y0 + h - r {
                true // The body rows: the full width.
            } else if y < y0 + r {
                in_cap(x, y, x0 + r, y0 + r) || in_cap(x, y, x0 + w - r, y0 + r)
            } else {
                in_cap(x, y, x0 + r, y0 + h - r) || in_cap(x, y, x0 + w - r, y0 + h - r)
            };
            if !inside {
                continue;
            }
            if x >= 0 && x < W as i64 {
                let px = y * W as i64 + x;
                if px >= 0 && px < buf.len() as i64 {
                    buf[px as usize] = word;
                }
            }
        }
    }
}

/// `v * a / 255`, round half up (the showcase's own tiny mul255).
fn mul8(v: u8, a: u8) -> u8 {
    ((u32::from(v) * u32::from(a) + 127) / 255) as u8
}

/// Expand the panel's words into a whole-pool ARGB8888 byte image.
fn panel_pool_bytes(words: &[u32], stride: u32, buffers: u64) -> Vec<u8> {
    let one = stride as usize * PANEL_H as usize;
    let mut out = vec![0u8; one * buffers as usize];
    for y in 0..PANEL_H as usize {
        for x in 0..W as usize {
            let word = words[y * W as usize + x];
            let dst = y * stride as usize + x * 4;
            // ARGB8888 in shm: one little-endian u32 word per pixel
            // (0xAArrggbb) — the byte order is B, G, R, A.
            let bytes = word.to_le_bytes();
            for region in 0..buffers as usize {
                out[region * one + dst..region * one + dst + 4].copy_from_slice(&bytes);
            }
        }
    }
    out
}

/// The wallpaper scene for one frame: RGB24 bytes, `W`×`H`.
pub fn render_frame(frame: u32) -> Vec<u8> {
    let mut buf = vec![0u8; (W as usize) * (H as usize) * 3];
    let (top, bottom) = sky_palette(frame);
    render_sky(&mut buf, frame, top, bottom);
    render_sun(&mut buf, frame);
    render_water(&mut buf, bottom);
    render_clouds(&mut buf, frame, top, bottom);
    render_overlays(&mut buf, frame);
    buf
}

/// Expand one RGB24 scene into a whole-pool XRGB8888 byte image
/// covering both ping-pong buffers.
fn pool_bytes(rgb: &[u8], stride: u32, buffers: u64) -> Vec<u8> {
    let one = stride as usize * H as usize;
    let mut out = vec![0u8; one * buffers as usize];
    for y in 0..H as usize {
        for x in 0..W as usize {
            let src = (y * W as usize + x) * 3;
            let dst = y * stride as usize + x * 4;
            let r = rgb[src];
            let g = rgb[src + 1];
            let b = rgb[src + 2];
            // XRGB8888 in shm: one little-endian u32 word per pixel
            // (0xFFrrggbb), so the byte order is B, G, R, X.
            for region in 0..buffers as usize {
                out[region * one + dst..region * one + dst + 4].copy_from_slice(&[b, g, r, 0xFF]);
            }
        }
    }
    out
}

/// Parsed command line.
#[derive(Clone, Debug, Default)]
pub struct ShowcaseArgs {
    /// `--socket NAME` / `LDP_SOCKET`.
    pub socket: Option<String>,
    /// `--hold SECS`: keep the session (and the final frame) on screen
    /// after the frame loop — the live-capture demo window (default 0).
    pub hold: Option<f64>,
}

/// The usage text.
#[must_use]
pub fn usage() -> String {
    "showcase — a colorful client-authored scene\n\
     \n\
     USAGE:\n\
     \x20 showcase [--socket NAME] [--hold SECS]\n\
     \n\
     OPTIONS:\n\
     \x20 --socket NAME   abstract socket name (or LDP_SOCKET)\n\
     \x20 --hold SECS      keep the final frame on screen after the loop\n\
     \x20                  (the live-capture demo window; default 0)\n\
     \x20 --help | --version  this text"
        .to_owned()
}

/// Run the example against `addr` with no hold; returns the exit code.
///
/// # Errors
/// As [`run_with`].
pub fn run(addr: &UnixAddr, out: &mut dyn Write) -> Result<i32, ToolError> {
    run_with(addr, None, out)
}

/// Run the example against `addr`, optionally holding the final frame
/// (`--hold SECS`, the live-capture demo window); returns the exit code.
///
/// # Errors
/// [`ToolError`] on connection, bootstrap, or frame-cycle failures —
/// the binary prints it and exits 1.
pub fn run_with(addr: &UnixAddr, hold: Option<f64>, out: &mut dyn Write) -> Result<i32, ToolError> {
    let mut session = ToolSession::connect(addr)?;
    session.bootstrap()?;
    let _ = writeln!(
        out,
        "showcase: connected to {} — {} global(s) advertised",
        addr.display_string(),
        session.globals().len()
    );

    let mut cycle = session.setup_surface(W, H, 2, 0x00)?;
    let _ = writeln!(
        out,
        "showcase: {W}x{H} surface {} ({} buffer(s))",
        cycle.surface.id().as_u32(),
        cycle.buffers.len()
    );
    // The glass panel: a translucent ARGB bar the compositor dresses
    // with the Liquid material (Phase 27) — the system owns the
    // frost, the corners and the shadow; the client owns the content.
    let mut panel = session.setup_surface_format(W, PANEL_H, 2, 0x00, FORMAT_ARGB8888)?;
    let _ = writeln!(
        out,
        "showcase: {W}x{PANEL_H} glass panel {} (translucent, frosted by the system)",
        panel.surface.id().as_u32()
    );

    let stride = FrameCycle::stride(W);
    let buffers = cycle.buffers.len() as u64;
    let panel_stride = FrameCycle::stride(W);
    let panel_buffers = panel.buffers.len() as u64;
    for frame in 0..FRAMES {
        let id = u64::from(frame) + 1;
        let rgb = render_frame(frame);
        let bytes = pool_bytes(&rgb, stride, buffers);
        cycle.pool.write_at(0, &bytes)?;
        session.commit_frame(&mut cycle, id, &[Rect::new(0, 0, W, H)])?;
        // The panel's pills glide in on the spring; the compositor
        // frosts whatever the wallpaper shows behind them.
        let panel_words = render_panel(frame);
        let panel_bytes = panel_pool_bytes(&panel_words, panel_stride, panel_buffers);
        panel.pool.write_at(0, &panel_bytes)?;
        session.commit_frame(&mut panel, id, &[Rect::new(0, 0, W, PANEL_H)])?;
        let _ = writeln!(out, "showcase: frame {id} committed and live");
        match session.wait_presented(&cycle.surface, id, ldp_tools::session::WAIT) {
            Ok(Some(sample)) => {
                let _ = writeln!(
                    out,
                    "showcase: frame {id} presented at {} ns (refresh {} ns)",
                    sample.ts_ns, sample.refresh_ns
                );
            }
            Ok(None) => {
                let _ = writeln!(
                    out,
                    "showcase: frame {id} was dropped (no presentation verdict)"
                );
            }
            Err(e) => return Err(e),
        }
    }
    let _ = writeln!(out, "showcase: {FRAMES} frame(s) presented — goodbye");
    if let Some(secs) = hold {
        if secs > 0.0 {
            let _ = writeln!(out, "showcase: holding the final frame for {secs}s");
            std::thread::sleep(std::time::Duration::from_secs_f64(secs));
        }
    }
    Ok(0)
}

/// Resolve the socket the same way the tools do.
///
/// # Errors
/// A usage-shaped [`ToolError`] when neither source names a socket.
pub fn resolve_socket(args: &ShowcaseArgs) -> Result<UnixAddr, ToolError> {
    let env = std::env::var("LDP_SOCKET").ok();
    let name = args.socket.as_deref().or(env.as_deref()).map(str::to_owned);
    let Some(name) = name else {
        return Err(ToolError::Logic(
            "no socket name: pass --socket NAME or set LDP_SOCKET".to_owned(),
        ));
    };
    let name = name.strip_prefix('@').unwrap_or(&name).to_owned();
    if name.is_empty() {
        return Err(ToolError::Logic("socket name is empty".to_owned()));
    }
    UnixAddr::abstract_name(name.as_bytes()).map_err(|e| ToolError::Logic(format!("{e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_render_deterministically() {
        let a = render_frame(2);
        let b = render_frame(2);
        assert_eq!(a, b);
        assert_eq!(a.len(), W as usize * H as usize * 3);
    }

    #[test]
    fn panels_render_deterministically() {
        let a = render_panel(2);
        let b = render_panel(2);
        assert_eq!(a, b);
        assert_eq!(a.len(), W as usize * PANEL_H as usize);
    }

    #[test]
    fn the_pills_spring_in_and_settle() {
        let first = render_panel(0);
        let last = render_panel(FRAMES - 1);
        // Frame 0: the pills sit off the left edge — the panel is the
        // bare haze (every word carries the haze's exact alpha).
        assert!(
            first.iter().all(|&w| w == 0x3C3C_3C3C),
            "frame 0 carries no pill ink"
        );
        // By the final frame the bright pills are on-screen.
        assert!(
            last.iter().any(|&w| w >> 24 == 235),
            "the settled panel carries pill ink"
        );
        // The spring is monotone: pill ink only grows frame over frame.
        let ink = |words: &[u32]| words.iter().filter(|&&w| w >> 24 == 235).count();
        let mut last_ink = 0usize;
        for frame in 0..FRAMES {
            let now = ink(&render_panel(frame));
            assert!(now >= last_ink, "ink shrank at frame {frame}");
            last_ink = now;
        }
    }

    #[test]
    fn lerp_and_mix_stay_in_range() {
        assert_eq!(lerp(0, 255, 1, 2), 127);
        assert_eq!(lerp(10, 20, 0, 5), 10);
        assert_eq!(lerp(10, 20, 5, 5), 20);
        assert_eq!(mix([0, 0, 0], [100, 200, 10], 1, 2), [50, 100, 5]);
    }

    #[test]
    fn the_sun_rises_and_the_stars_fade() {
        assert!(sun_y(0) > sun_y(3));
        assert!(star_level(0) > star_level(3));
        assert!(title_y(0) > title_y(3));
    }
}
