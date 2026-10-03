//! The positioning shell (Phase 28): where the system puts windows,
//! and the dock it keeps for itself.
//!
//! Two halves, one doctrine — *the system owns the screen*:
//!
//! * **Placement** (pure policy from `ldp-shell::layout`): a portrait
//!   output is a phone — every app anchors at the usable origin,
//!   above the dock; a landscape output is a desktop — windows
//!   cascade diagonally until the usable edge catches them. Placement
//!   happens at a root's *first attach* (the buffer's size is the
//!   input), riding the pending queue so the position applies with
//!   the very commit that maps the surface — a window never lands at
//!   the origin and jumps. Migrations re-place every mapped root
//!   immediately (`SurfaceTree::set_position_now`) so the desktop
//!   re-arranges onto the new output in the migration frame itself.
//! * **The dock** (system chrome): a frosted bar clinging to a screen
//!   edge — the phone's home dock. It is *not* a client surface: the
//!   compositor owns its pixels (a haze plus a row of app pills),
//!   renders it above every client layer, and frosts it with Phase
//!   27's material language (the system owns the materials). The dock
//!   *reserves* its thickness out of the usable area — windows are
//!   placed above it; a window taller than the usable area runs
//!   *under* it (the dock is chrome above client content; overflow is
//!   occluded, never destroyed — the iOS keyboard doctrine).
//!
//! The dock's intro is a spring ([`ldp_compositor::spring`]): the bar
//! rises from below the output edge, critically damped, settling
//! within a handful of frames. The rise integrates at render cadence
//! from the driver's timestamps — bit-reproducible, the Phase 27
//! motion doctrine — and a quiescent compositor simply holds its last
//! integrated offset (nothing moves when nothing renders).
//!
//! Library default: **the dock is off** and placement still runs —
//! every programmatic construction keeps the plain Phase 26/27
//! pixels byte-identical (the equivalence corpora's oracles), while
//! the CLI defaults to `--dock auto` so an operator's compositor
//! serves the home dock. Same doctrine as the effects tier.

use ldp_compositor::spring::Spring;
use ldp_core::buffer::{BufferGeometry, FourCC, Modifier};
use ldp_core::color::ColorDescription;
use ldp_core::geometry::{Rect, Region, Transform};
use ldp_renderer::{BufferView, LayerStyle, SurfaceLayer};
use ldp_shell::layout::{self, DockConfig, Layout, PlacementPolicy};
use ldp_shell::popup::{Placement, Popup, PopupConstraints, PopupGeometry};
use ldp_shell::{clamp_into, place};

/// The dock's choice (CLI `--dock`): on (the operator default) or
/// off (the library default — byte-exact legacy pixels).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum DockMode {
    /// Serve the system dock (the CLI default).
    #[default]
    Auto,
    /// No dock: the whole output is usable, plain chrome.
    Off,
}

impl DockMode {
    /// Parse `--dock` values.
    ///
    /// # Errors
    /// `None` for an unknown value (the caller prints usage).
    #[must_use]
    pub fn parse(value: &str) -> Option<DockMode> {
        match value {
            "auto" => Some(DockMode::Auto),
            "off" => Some(DockMode::Off),
            _ => None,
        }
    }
}

/// The positioning shell's configuration (`CompositorConfig` member).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ShellConfig {
    /// Whether the system dock serves.
    pub dock: DockMode,
    /// The dock's thickness (px); the phone home bar is 84.
    pub dock_thickness: u32,
}

impl Default for ShellConfig {
    fn default() -> ShellConfig {
        ShellConfig {
            dock: DockMode::Off,
            dock_thickness: DockConfig::PHONE.thickness,
        }
    }
}

/// The dock's pill row geometry (the ink painter's parameters).
const PILLS: u32 = 4;
/// The pill row's horizontal margins (px).
const PILL_MARGIN: u32 = 24;
/// The pill row's inter-pill gap (px).
const PILL_GAP: u32 = 16;
/// The pill row's vertical inset (px, top and bottom).
const PILL_INSET: u32 = 12;
/// The pill capsule's ink: a light, mostly-opaque capsule.
const PILL_RGB: [u8; 3] = [225, 228, 235];
/// The pill capsule's alpha.
const PILL_ALPHA: u8 = 235;
/// The dock's haze base (premultiplied word: ~24% dark veil).
const HAZE_WORD: u32 = 0x3C3C_3C3C;
/// The dock's rise spring stiffness — settled within a handful of
/// frames at presentation cadence (the Phase 27 panel's value).
const RISE_STIFFNESS: f32 = 9000.0;

/// The positioning shell: the resolved layout, the placement counter,
/// and the system dock.
#[derive(Debug)]
pub struct Shell {
    /// The layout resolved for the live output (class, usable area,
    /// dock reservation) — in *logical* pixels.
    pub layout: Layout,
    /// The system dock, when one serves (its rectangle is physical —
    /// the render space).
    pub dock: Option<SystemDock>,
    /// Roots placed so far (the cascade's step index).
    placed: u32,
    /// The shell's configuration (dock on/off survives re-layouts).
    config: ShellConfig,
    /// The output's scale factor (Phase 31): the layout class, the
    /// usable area, and the placement cascade resolve in *logical*
    /// pixels (`physical / scale`); the dock's rectangle and every
    /// placement result return in *physical* pixels (the tree and the
    /// renderer's space). Identity keeps the Phase 28 geometry
    /// byte-identical.
    scale: ldp_core::scale::ScaleFactor,
}

impl Shell {
    /// A shell with no output yet (bring-up resolves the layout).
    #[must_use]
    pub fn new(config: ShellConfig) -> Shell {
        Shell {
            layout: Layout::resolve((1, 1), None),
            dock: None,
            placed: 0,
            config,
            scale: ldp_core::scale::ScaleFactor::IDENTITY,
        }
    }

    /// The shell's configuration.
    #[must_use]
    pub const fn config(&self) -> ShellConfig {
        self.config
    }

    /// Whether the positioning shell serves (placement runs, the
    /// dock renders). `DockMode::Off` — the library default — is the
    /// legacy doctrine: roots keep their creation positions and no
    /// system chrome draws, so every Phase 26/27 pixel oracle stays
    /// byte-exact. The CLI's `--dock auto` opts the operator in.
    #[must_use]
    pub const fn is_active(&self) -> bool {
        matches!(self.config.dock, DockMode::Auto)
    }

    /// Resolve the layout for a (possibly new) output: classify,
    /// dock, carve. A fresh output replays the dock's intro rise and
    /// resets the placement counter (a new screen starts a new
    /// cascade); a re-resolution at the *same* geometry keeps the
    /// rise where it is (bring-up calls this once per pipeline).
    pub fn relayout(
        &mut self,
        output: (u32, u32),
        scale: ldp_core::scale::ScaleFactor,
        reset: bool,
    ) {
        self.scale = scale;
        // The layout doctrine resolves on the *logical* canvas: a
        // 1920x1080 panel at 2x serves a 960x540 logical screen — the
        // phone class, the fill policy, the usable carve, all of it
        // (the operator asked for a phone-density desktop and gets
        // one). The dock's own rectangle returns physical.
        let logical = (scale.unscale_px(output.0), scale.unscale_px(output.1));
        let dock_config = match self.config.dock {
            DockMode::Off => None,
            DockMode::Auto => Some(DockConfig::new(
                layout::Edge::Bottom,
                self.config.dock_thickness,
            )),
        };
        self.layout = Layout::resolve(logical, dock_config);
        self.dock = self.layout.dock.map(|rect| {
            let physical = scale_rect(rect, scale);
            // A fresh dock rises; a re-resolution of the same output
            // keeps the settled one (nothing moved).
            if reset {
                SystemDock::new(physical)
            } else {
                SystemDock::settled(physical)
            }
        });
        if reset {
            self.placed = 0;
        }
    }

    /// Place a newly-attaching root of `size` under the class's
    /// default policy; increments the placement counter.
    #[must_use]
    pub fn place_root(&mut self, size: (u32, u32)) -> (i32, i32) {
        let nth = self.placed;
        self.placed = self.placed.saturating_add(1);
        // The buffer's size is physical (the tree's space); the
        // cascade resolves logically; the placement lands physically.
        let logical = (self.scale.unscale_px(size.0), self.scale.unscale_px(size.1));
        let (x, y) = place(
            self.layout.class.default_policy(),
            self.layout.usable,
            logical,
            nth,
        );
        (scale_axis(x, self.scale), scale_axis(y, self.scale))
    }

    /// Re-place a mapped root for the *current* layout (the
    /// migration arm): a phone re-anchors at the usable origin; a
    /// desktop clamps the window fully inside the usable area.
    /// Returns the new position.
    #[must_use]
    pub fn replace_root(&self, bounds: Rect) -> (i32, i32) {
        // Bounds arrive physical (the tree's space); the re-placement
        // resolves logically; the answer lands physically.
        let logical = unscale_rect(bounds, self.scale);
        if self.layout.class.default_policy() == PlacementPolicy::Fill {
            (
                scale_axis(self.layout.usable.x, self.scale),
                scale_axis(self.layout.usable.y, self.scale),
            )
        } else {
            let clamped = clamp_into(logical, self.layout.usable);
            (
                scale_axis(clamped.x, self.scale),
                scale_axis(clamped.y, self.scale),
            )
        }
    }

    /// The dock's resting rectangle (its permanent reservation), when
    /// one serves — the direct-scanout subtraction and the usable
    /// carve's anchor.
    #[must_use]
    pub fn dock_rect(&self) -> Option<Rect> {
        // Physical pixels (the render space): the layout's logical
        // reservation scaled by the output's factor.
        self.layout.dock.map(|rect| scale_rect(rect, self.scale))
    }

    /// The usable area in *physical* pixels (the tree's and the
    /// renderer's space) — the placement's carve translated from the
    /// logical layout. The popup solver's screen-side truth.
    #[must_use]
    pub fn usable_physical(&self) -> Rect {
        scale_rect(self.layout.usable, self.scale)
    }

    /// The honest one-line report ("phone layout, dock 84 px at the
    /// bottom") — the resolved layout doctrine, the startup's third
    /// line. The legacy default reports itself honestly too.
    #[must_use]
    pub fn report(&self) -> String {
        if self.is_active() {
            self.layout.report()
        } else {
            "legacy (origin placement, no dock)".to_owned()
        }
    }
}

/// The system dock: the ink (haze + pill row), the resting rectangle,
/// and the intro rise.
#[derive(Debug)]
pub struct SystemDock {
    /// The resting rectangle (output coordinates).
    pub rect: Rect,
    /// The intro rise: the y offset from the resting place, springing
    /// from `thickness` (fully below the edge) to 0.
    pub rise: Spring,
    /// The ink: ARGB8888 premultiplied, tightly packed, dock-sized.
    ink: Vec<u8>,
    /// The ink's validated geometry (the renderer's view contract).
    geometry: BufferGeometry,
    /// The driver timestamp the rise last integrated at (ms).
    integrated_at: Option<u64>,
    /// The last offset a frame rendered (the vacate rule's "old"
    /// side); `None` before the first frame and after settling.
    last_offset: Option<f32>,
    /// The buffer identity (the GL texture-cache key; the system dock
    /// is buffer 0 — clients mint from 1).
    key: u64,
}

impl SystemDock {
    /// A fresh dock that will rise from below the edge.
    #[must_use]
    pub fn new(rect: Rect) -> SystemDock {
        let mut rise = Spring::critically_damped(0.0, RISE_STIFFNESS);
        rise.displace(rect.h as f32);
        SystemDock {
            rect,
            ink: paint_dock(rect.w, rect.h),
            geometry: dock_geometry(rect.w, rect.h),
            rise,
            integrated_at: None,
            last_offset: None,
            key: 0,
        }
    }

    /// A dock already at rest (a re-resolution of the same output).
    #[must_use]
    pub fn settled(rect: Rect) -> SystemDock {
        SystemDock {
            rect,
            ink: paint_dock(rect.w, rect.h),
            geometry: dock_geometry(rect.w, rect.h),
            rise: Spring::resting(0.0),
            integrated_at: None,
            last_offset: None,
            key: 0,
        }
    }

    /// Integrate the rise to `now_ms` and report the frame's dock
    /// placement: the destination rectangle. While animating, the
    /// union of the previous and current placements joins `repaint`
    /// (the vacate rule: the wallpaper the dock covered is revealed,
    /// the area it reaches is new ink); a settled dock repaints
    /// nothing here.
    ///
    /// `now_ms` is the driver's clock in whole milliseconds — the
    /// same timestamps the frame's flips land on, so the rise is
    /// bit-reproducible from the render sequence.
    pub fn advance(&mut self, now_ms: u64, repaint: &mut Region) -> Rect {
        if let Some(last) = self.integrated_at {
            let dt = now_ms.saturating_sub(last);
            if dt > 0 {
                self.rise.step(dt);
            }
        }
        self.integrated_at = Some(now_ms);
        let offset = self.rise.pos.max(0.0);
        let dest = Rect::new(
            self.rect.x,
            self.rect.y + offset.round() as i32,
            self.rect.w,
            self.rect.h,
        );
        let animating = !self.rise.settled() || self.last_offset.is_some();
        if animating {
            // The vacate rule: while the dock moves, both the old and
            // the new placements repaint.
            if let Some(prev) = self.last_offset {
                let old = Rect::new(
                    self.rect.x,
                    self.rect.y + prev.max(0.0).round() as i32,
                    self.rect.w,
                    self.rect.h,
                );
                repaint.add(old);
            }
            repaint.add(dest);
            if self.rise.settled() {
                // Latched: the next quiet frame needs no vacate.
                self.last_offset = None;
            } else {
                self.last_offset = Some(offset);
            }
        }
        dest
    }

    /// The dock's Liquid style at `tier`: the **chrome material**
    /// (Phase 40) — the translucent frost (the system's glass), the
    /// shadow cleared (the bar hugs the edge — the Phase 28 doctrine),
    /// and the chrome hairline tracing the bar's top edge (the light
    /// catch that separates glass from wallpaper). `Minimal` keeps the
    /// plain legacy bytes.
    #[must_use]
    pub fn style(&self, tier: ldp_renderer::EffectTier) -> LayerStyle {
        ldp_renderer::Material::Chrome.style(tier)
    }

    /// Build the render layer for a placement (the frame loop's last
    /// layer, above every client surface).
    ///
    /// # Errors
    /// Propagates [`ldp_renderer::RendererError`] only if the ink's
    /// geometry and storage disagree (a construction-time invariant —
    /// never in practice).
    pub fn layer(
        &self,
        dest: Rect,
        style: LayerStyle,
    ) -> Result<SurfaceLayer<'_>, ldp_renderer::RendererError> {
        let view = BufferView::new(self.key, &self.ink, self.geometry.clone())?;
        let mut layer = SurfaceLayer::new(
            view,
            dest,
            Transform::Normal,
            ColorDescription::srgb_sdr(),
            1.0,
            Region::new(),
        );
        layer.style = style.sanitized(dest.w, dest.h);
        Ok(layer)
    }

    /// The dock's current placement — the resting rectangle offset by
    /// the rise's present position (the render truth after `advance`
    /// integrated the spring at the driver's clock; the resting
    /// rectangle once settled).
    #[must_use]
    pub fn current_rect(&self) -> Rect {
        let offset = self.rise.pos.max(0.0);
        Rect::new(
            self.rect.x,
            self.rect.y + offset.round() as i32,
            self.rect.w,
            self.rect.h,
        )
    }

    /// The dock's resting rectangle.
    #[must_use]
    pub const fn rect(&self) -> Rect {
        self.rect
    }
}

/// Paint the dock's ink: a haze base plus the pill row, ARGB8888
/// premultiplied, tightly packed (stride = `w` * 4). Degenerate docks
/// (too small to hold a row) keep the haze alone.
fn paint_dock(w: u32, h: u32) -> Vec<u8> {
    let mut words = vec![HAZE_WORD; (w as usize) * (h as usize)];
    let pill_h = h.saturating_sub(2 * PILL_INSET);
    let span = w
        .saturating_sub(2 * PILL_MARGIN)
        .saturating_sub((PILLS - 1) * PILL_GAP);
    let pill_w = span / PILLS;
    if pill_w >= 2 && pill_h >= 2 {
        for i in 0..PILLS {
            let x0 = (PILL_MARGIN + i * (pill_w + PILL_GAP)) as i64;
            let y0 = PILL_INSET as i64;
            capsule_words(&mut words, w, x0, y0, pill_w as i64, pill_h as i64);
        }
    }
    words_to_bytes(&words)
}

/// Paint one rounded capsule (a horizontal pill) into a premultiplied
/// word buffer of width `w`: the body columns run the full height,
/// the two ends are semicircles of radius `ph / 2` (the Phase 27
/// showcase painter, corrected — the body is *columns*, not rows: a
/// wide pill's middle is full-height ink, only the ends round).
fn capsule_words(buf: &mut [u32], w: u32, x0: i64, y0: i64, pw: i64, ph: i64) {
    let prem = [
        mul255(PILL_RGB[0], PILL_ALPHA),
        mul255(PILL_RGB[1], PILL_ALPHA),
        mul255(PILL_RGB[2], PILL_ALPHA),
        PILL_ALPHA,
    ];
    let word = u32::from(prem[3]) << 24
        | u32::from(prem[0]) << 16
        | u32::from(prem[1]) << 8
        | u32::from(prem[2]);
    capsule_word(buf, w, x0, y0, pw, ph, word);
}

/// The capsule painter's parameterized core (Phase 52: the close
/// affordance paints its own capsule — the dock's pill row and the
/// chrome's button share the geometry, not the ink).
fn capsule_word(buf: &mut [u32], w: u32, x0: i64, y0: i64, pw: i64, ph: i64, word: u32) {
    let r = ph / 2;
    let in_cap =
        |x: i64, y: i64, cx: i64, cy: i64| (x - cx) * (x - cx) + (y - cy) * (y - cy) <= r * r;
    for y in y0..y0 + ph {
        for x in x0..x0 + pw {
            let inside = if x >= x0 + r && x < x0 + pw - r {
                true // The body columns: the full height.
            } else if x < x0 + r {
                in_cap(x, y, x0 + r, y0 + r)
            } else {
                in_cap(x, y, x0 + pw - r, y0 + r)
            };
            if !inside {
                continue;
            }
            if x >= 0 && x < w as i64 {
                let px = y * w as i64 + x;
                if px >= 0 && (px as usize) < buf.len() {
                    buf[px as usize] = word;
                }
            }
        }
    }
}

/// `v * a / 255`, round half up (the showcase's mul8).
fn mul255(v: u8, a: u8) -> u8 {
    ((u32::from(v) * u32::from(a) + 127) / 255) as u8
}

/// Expand premultiplied words into ARGB8888 bytes (little-endian u32
/// per pixel — the shm byte order).
fn words_to_bytes(words: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(words.len() * 4);
    for w in words {
        out.extend_from_slice(&w.to_le_bytes());
    }
    out
}

/// The dock ink's validated geometry.
fn dock_geometry(w: u32, h: u32) -> BufferGeometry {
    let planes = BufferGeometry::simple_layout(w, h, FourCC::ARGB8888);
    let storage = u64::from(w) * u64::from(h) * 4;
    BufferGeometry::new(w, h, FourCC::ARGB8888, Modifier::LINEAR, &planes, storage)
        .expect("dock-sized geometry is always valid")
}

// ---------------------------------------------------------------------------
// Phase 52 — the SSD chrome pass (the drawn band and its close button)
// ---------------------------------------------------------------------------

/// The title band's ink: a light, near-opaque bar (the drawn chrome's
/// base). `pub` for the pixel oracle's honesty (the tests assert the
/// exact words the painter writes).
pub const BAND_RGB: [u8; 3] = [238, 241, 246];
/// The title band's alpha.
pub const BAND_ALPHA: u8 = 250;
/// The border ring's ink: a crisp, fully-opaque hairline.
pub const RING_RGB: [u8; 3] = [176, 180, 192];
/// The border ring's alpha.
pub const RING_ALPHA: u8 = 255;
/// The close affordance's ink: a warm capsule (the farewell grip).
pub const CLOSE_RGB: [u8; 3] = [226, 88, 76];
/// The close affordance's alpha.
pub const CLOSE_ALPHA: u8 = 255;
/// The close glyph's ink (the × the capsule carries).
pub const GLYPH_RGB: [u8; 3] = [255, 255, 255];
/// The close glyph's alpha.
pub const GLYPH_ALPHA: u8 = 255;
/// The close glyph's stroke half-width (logical px, scaled per
/// output).
const GLYPH_STROKE: u32 = 2;

/// The chrome raster's shape key: the frame's size, the band split
/// (the applied insets), and the close affordance's scaled geometry —
/// the scale's whole effect on the ink (two outputs whose insets
/// coincide carry distinct keys whenever the button's metrics do).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ChromeShape {
    /// The frame's width.
    pub w: u32,
    /// The frame's height.
    pub h: u32,
    /// The title band's height (the top inset).
    pub top: u32,
    /// The left border's width.
    pub left: u32,
    /// The right border's width.
    pub right: u32,
    /// The bottom border's width.
    pub bottom: u32,
    /// The close affordance's scaled size.
    pub close: u32,
    /// The close affordance's scaled margin.
    pub margin: u32,
}

/// One cached chrome raster: the ink, its validated geometry, and
/// the buffer-view identity (unique per raster — never a client
/// buffer's, the dock's 0, or a ghost's small counter).
#[derive(Debug)]
pub struct ChromeRaster {
    /// The raster's identity (the renderer's cache key).
    pub key: u64,
    /// The ink: ARGB8888 premultiplied, tightly packed (`w * 4`
    /// bytes per row), frame-sized with a transparent content hole
    /// (the client's own layer composites above the middle).
    pub ink: Vec<u8>,
    /// The ink's validated geometry (the renderer's view contract).
    pub geometry: BufferGeometry,
}

/// The SSD chrome pass (Phase 52): the drawn band around every
/// server-decorated window — the title bar, the border ring, and the
/// close affordance whose release asks the client to close
/// (`toplevel.close`, the frozen event's first sender). CPU ink on
/// the dock's own model (no framebuffer, the honest `NoFb` demotion;
/// a visible band pins the frame to the composite arm), cached per
/// frame *shape* (the ink is stateless — geometry is its only input,
/// so same-shaped windows share one raster; a resize re-paints).
#[derive(Debug, Default)]
pub struct ChromePass {
    /// The raster cache, one per distinct shape.
    rasters: std::collections::HashMap<ChromeShape, ChromeRaster>,
    /// The raster counter (identities mint `KEY_BASE + n`).
    next: u64,
}

/// The chrome identities' base — far above the dock's 0, the ghost
/// counter, and every client buffer's identity (the shm family mints
/// from a small atomic; a serving desktop never reaches a trillion
/// rasters).
const CHROME_KEY_BASE: u64 = 1 << 48;

impl ChromePass {
    /// The pass's prepare (the mutable half): paint every raster the
    /// next grade walk will ask for — one per distinct chrome shape
    /// among the *serving* windows (mapped, routed with a buffer, not
    /// hidden, and [`chrome_geometry`]'s role truth). The grade walk
    /// then reads immutably; the two never diverge because both ask
    /// the same helper under the same scale (the primary's — the
    /// policy's own doctrine: every proposal's insets, and therefore
    /// every band, derive from it).
    pub fn prepare(
        &mut self,
        snapshot: &ldp_compositor::snapshot::FrameSnapshot,
        scene: &crate::scene::Scene,
        toplevels: &ToplevelHost,
        scale: ldp_core::scale::ScaleFactor,
    ) {
        let mut wanted: Vec<ChromeShape> = Vec::new();
        for id in snapshot.render_order() {
            let Some(node) = snapshot.node(*id) else {
                continue;
            };
            if !node.mapped
                || scene.hidden.contains(id)
                || !scene.routes.get(id).is_some_and(|r| r.buffer.is_some())
            {
                continue;
            }
            if let Some((frame, insets)) = chrome_geometry(toplevels, *id, node.bounds) {
                let shape = ChromeShape::of(frame, insets, scale);
                if !wanted.contains(&shape) {
                    wanted.push(shape);
                }
            }
        }
        for shape in wanted {
            if !self.rasters.contains_key(&shape) {
                let ink = paint_chrome(
                    shape.w,
                    shape.h,
                    ldp_shell::ssd::Insets {
                        left: shape.left,
                        top: shape.top,
                        right: shape.right,
                        bottom: shape.bottom,
                    },
                    shape,
                );
                let geometry = dock_geometry(shape.w, shape.h);
                let key = CHROME_KEY_BASE + self.next;
                self.next += 1;
                self.rasters
                    .insert(shape, ChromeRaster { key, ink, geometry });
            }
        }
    }

    /// The prepared raster for one shape (the grade walk's read
    /// half). `None` only if the prepare and the walk disagree —
    /// impossible by construction (both ask [`chrome_geometry`]),
    /// kept honest by skipping the chrome that frame.
    #[must_use]
    pub fn raster(&self, shape: ChromeShape) -> Option<&ChromeRaster> {
        self.rasters.get(&shape)
    }
}

impl ChromeShape {
    /// The shape of one serving chrome: its frame, the applied band
    /// split, and the scaled close-affordance metrics.
    #[must_use]
    pub fn of(
        frame: Rect,
        insets: ldp_shell::ssd::Insets,
        scale: ldp_core::scale::ScaleFactor,
    ) -> ChromeShape {
        ChromeShape {
            w: frame.w,
            h: frame.h,
            top: insets.top,
            left: insets.left,
            right: insets.right,
            bottom: insets.bottom,
            close: scale.scale_px_up(ldp_shell::ssd::CLOSE_SIZE),
            margin: scale.scale_px_up(ldp_shell::ssd::CLOSE_MARGIN),
        }
    }
}

/// The one geometry truth of the drawn chrome (Phase 52): the frame
/// rect and the applied insets of a serving window's band — `None`
/// when the window draws no chrome: not a toplevel (a plain surface,
/// a popup, a dialog), client-decorated (CSD draws its own chrome —
/// the system grid only reserves the hit zone *inside* the client's
/// buffer), never-yet-applied (the two-phase commit has not accepted
/// the insets — the window has not reserved the band), or applied
/// zero insets (fullscreen covers everything, the `NONE` metrics).
/// The render pass (the layer), the damage ledger (the claims), and
/// the input pump (the ring hit test) all read this one answer.
pub fn chrome_geometry(
    toplevels: &ToplevelHost,
    surface: ldp_compositor::surface::SurfaceId,
    content: Rect,
) -> Option<(Rect, ldp_shell::ssd::Insets)> {
    let entry = toplevels.by_surface(surface)?;
    if entry.decoration != ldp_shell::ssd::DecorationMode::Server {
        return None;
    }
    let applied = entry.machine.applied()?;
    Some((applied.insets.frame_around(content), applied.insets))
}

/// Paint one window's chrome ink (Phase 52 — the SSD pass): the
/// title band across the frame's top, the border ring down the sides
/// and under the bottom, and the close affordance (a warm capsule
/// carrying a white × glyph) at the title band's right end —
/// ARGB8888 premultiplied, tightly packed, frame-sized, the content
/// hole transparent. Degenerate frames (thinner than their own band)
/// saturate per side; the painter never writes outside its bounds.
fn paint_chrome(
    frame_w: u32,
    frame_h: u32,
    band: ldp_shell::ssd::Insets,
    shape: ChromeShape,
) -> Vec<u8> {
    let band_word = prem_word(BAND_RGB, BAND_ALPHA);
    let ring_word = prem_word(RING_RGB, RING_ALPHA);
    let close_word = prem_word(CLOSE_RGB, CLOSE_ALPHA);
    let glyph_word = prem_word(GLYPH_RGB, GLYPH_ALPHA);
    let mut words = vec![0u32; frame_w as usize * frame_h as usize];
    let title_rows = band.top.min(frame_h);
    let left_cols = band.left.min(frame_w);
    let right_cols = band.right.min(frame_w);
    let bottom_rows = band.bottom.min(frame_h);
    // The title band: full width across the top (the ring's top edge
    // is the band's own ink).
    for y in 0..title_rows as usize {
        for x in 0..frame_w as usize {
            words[y * frame_w as usize + x] = band_word;
        }
    }
    // The ring: the left/right/bottom borders.
    for y in title_rows as usize..frame_h as usize {
        for x in 0..left_cols as usize {
            words[y * frame_w as usize + x] = ring_word;
        }
        for x in (frame_w - right_cols) as usize..frame_w as usize {
            words[y * frame_w as usize + x] = ring_word;
        }
    }
    for y in (frame_h - bottom_rows) as usize..frame_h as usize {
        for x in 0..frame_w as usize {
            words[y * frame_w as usize + x] = ring_word;
        }
    }
    // The close affordance: a capsule at the title band's right end
    // (frame-relative geometry rebuilt from the shape's own scaled
    // metrics — the same numbers the hit test reads, never a second
    // scale source).
    let size = shape.close;
    let margin = shape.margin;
    let bx = frame_w.saturating_sub(margin.saturating_add(size));
    let by = margin;
    if size >= 2 && bx + size <= frame_w && by + size <= frame_h {
        capsule_word(
            &mut words,
            frame_w,
            i64::from(bx),
            i64::from(by),
            i64::from(size),
            i64::from(size),
            close_word,
        );
        // The × glyph: two diagonal strokes through the capsule's
        // center, the stroke half-width scaling with the button.
        let cx = i64::from(bx) + i64::from(size) / 2;
        let cy = i64::from(by) + i64::from(size) / 2;
        let stroke = (u64::from(GLYPH_STROKE) * u64::from(size)
            / u64::from(ldp_shell::ssd::CLOSE_SIZE))
        .max(1) as i64;
        for y in by..by + size {
            for x in bx..bx + size {
                let d1 = i64::from(x) - cx - (i64::from(y) - cy);
                let d2 = i64::from(x) - cx + i64::from(y) - cy;
                if d1.abs() <= stroke || d2.abs() <= stroke {
                    words[y as usize * frame_w as usize + x as usize] = glyph_word;
                }
            }
        }
    }
    words_to_bytes(&words)
}

/// One premultiplied ARGB word from an RGB triple and alpha.
fn prem_word(rgb: [u8; 3], a: u8) -> u32 {
    let p = [mul255(rgb[0], a), mul255(rgb[1], a), mul255(rgb[2], a)];
    u32::from(a) << 24 | u32::from(p[0]) << 16 | u32::from(p[1]) << 8 | u32::from(p[2])
}

/// Scale a logical axis coordinate into physical pixels
/// (round-to-nearest; Q8.8 arithmetic in i64 — negatives place left of
/// the origin).
pub(crate) fn scale_axis(x: i32, scale: ldp_core::scale::ScaleFactor) -> i32 {
    let q8 = u64::from(scale.to_q8());
    let scaled = (i64::from(x) * q8 as i64 + 128) / 256;
    i32::try_from(scaled).unwrap_or(i32::MAX)
}

/// Inverse-scale a physical axis coordinate into logical pixels.
pub(crate) fn unscale_axis(x: i32, scale: ldp_core::scale::ScaleFactor) -> i32 {
    let q8 = u64::from(scale.to_q8());
    let unscaled = (i64::from(x) * 256) / q8 as i64;
    i32::try_from(unscaled).unwrap_or(i32::MAX)
}

/// Scale a logical rectangle into physical pixels.
fn scale_rect(rect: Rect, scale: ldp_core::scale::ScaleFactor) -> Rect {
    Rect::new(
        scale_axis(rect.x, scale),
        scale_axis(rect.y, scale),
        scale.scale_px(rect.w),
        scale.scale_px(rect.h),
    )
}

/// Inverse-scale a physical rectangle into logical pixels.
fn unscale_rect(rect: Rect, scale: ldp_core::scale::ScaleFactor) -> Rect {
    Rect::new(
        unscale_axis(rect.x, scale),
        unscale_axis(rect.y, scale),
        scale.unscale_px(rect.w),
        scale.unscale_px(rect.h),
    )
}

/// One served popup: the wire identity (client + popup object), the
/// popup's own surface, the owning parent surface, and `ldp-shell`'s
/// placement machine.
///
/// The host keeps the *policy*; the tree keeps the *pixels*. A
/// popup's surface is a root of the scene like any toplevel — the
/// tree's position is the solved placement translated into output
/// coordinates (parent's position + the placement's parent-relative
/// offset), applied through the pending queue so it lands with the
/// mapping commit, never origin-then-jump.
#[derive(Debug)]
pub struct ServedPopup {
    /// The owning client (u32 — the wire identity the dispatcher
    /// addresses).
    pub client: u32,
    /// The `ldp.shell.popup` object id.
    pub object: u32,
    /// The popup's own surface (the one `get_popup` gave the role).
    pub surface: ldp_compositor::surface::SurfaceId,
    /// The parent surface the anchor coordinates live in (`None`:
    /// the anchor lives on the output itself — the bar-menu
    /// doctrine, the spec's null-parent clause).
    pub parent: Option<ldp_compositor::surface::SurfaceId>,
    /// The placement machine (anchor/gravity/constraints/serials).
    pub machine: Popup,
    /// The size the last proposal solved for (the re-proposal
    /// trigger: a mapping commit of a different size re-solves).
    solved: (u32, u32),
}

impl ServedPopup {
    /// The size the standing proposal solved for (the re-solve's
    /// size feed; `(0, 0)` before the first attach).
    #[must_use]
    pub const fn solved_size(&self) -> (u32, u32) {
        self.solved
    }
}

/// The popup-role host: every live popup the compositor serves.
///
/// Phase 36's shell service surface — the role `get_popup` mints and
/// the dispatcher drives:
///
/// * **the usable area** for constraint solving is the shell's usable
///   area translated into *parent coordinates* (a menu must stay on
///   screen and near its parent — the macOS doctrine), the frame the
///   solver speaks natively;
/// * **the placement proposal** rides `popup.configure` (serial, x,
///   y relative to the parent's top-left, width, height) at mint and
///   whenever the mapping commit's buffer size differs from the last
///   solved size — a menu whose content grew re-solves against the
///   world;
/// * **dismissal** (`popup.done`) fires when the client dismisses or
///   the parent goes away — the grab-break of the spec's dismissal
///   clause. The host only *records* the state; the dispatcher owns
///   the wire emission.
///
/// The host is policy-only: no sockets, no clocks (the serials draw
/// from `Popup`'s own `SerialClock`, advanced by calls, not time).
#[derive(Debug, Default)]
pub struct PopupHost {
    /// Every popup entry, insertion order (dismissed entries stay
    /// until the object is destroyed — a `done`-but-not-destroyed
    /// popup must keep answering `ack_configure` with its last
    /// proposal without re-proposing).
    popups: Vec<ServedPopup>,
}

impl PopupHost {
    /// An empty host.
    #[must_use]
    pub const fn new() -> PopupHost {
        PopupHost { popups: Vec::new() }
    }

    /// Register a freshly minted popup object (the `get_popup`
    /// dispatch arm): the machine starts live with the client's
    /// anchor geometry and constraint grant.
    pub fn mint(
        &mut self,
        client: u32,
        object: u32,
        surface: ldp_compositor::surface::SurfaceId,
        parent: Option<ldp_compositor::surface::SurfaceId>,
        geometry: PopupGeometry,
        constraints: PopupConstraints,
    ) {
        self.popups.push(ServedPopup {
            client,
            object,
            surface,
            parent,
            machine: Popup::new(geometry, constraints),
            solved: (0, 0),
        });
    }

    /// The popup entry a wire object addresses.
    #[must_use]
    pub fn entry(&self, client: u32, object: u32) -> Option<&ServedPopup> {
        self.popups
            .iter()
            .find(|p| p.client == client && p.object == object)
    }

    /// The popup entry a wire object addresses, mutably (the
    /// dispatcher's request arms).
    pub fn entry_mut(&mut self, client: u32, object: u32) -> Option<&mut ServedPopup> {
        self.popups
            .iter_mut()
            .find(|p| p.client == client && p.object == object)
    }

    /// The live popup a *surface* belongs to (the re-proposal
    /// sweep's lookup: a mapping commit re-solves). Returns the
    /// entry, mutably.
    pub fn by_surface_mut(
        &mut self,
        surface: ldp_compositor::surface::SurfaceId,
    ) -> Option<&mut ServedPopup> {
        self.popups
            .iter_mut()
            .find(|p| p.surface == surface && p.machine.is_live())
    }

    /// The wire identity of the live popup a *surface* belongs to:
    /// `(client, popup object, parent surface)` — the attach path's
    /// lookup (borrow-free so the caller can consult the scene
    /// between the two host calls).
    #[must_use]
    pub fn by_surface_ids(
        &self,
        surface: ldp_compositor::surface::SurfaceId,
    ) -> Option<(u32, u32, Option<ldp_compositor::surface::SurfaceId>)> {
        self.popups
            .iter()
            .find(|p| p.surface == surface && p.machine.is_live())
            .map(|p| (p.client, p.object, p.parent))
    }

    /// Whether `surface` is some live popup's surface (the dismissal
    /// rule's membership test).
    #[must_use]
    pub fn is_popup_surface(&self, surface: ldp_compositor::surface::SurfaceId) -> bool {
        self.popups
            .iter()
            .any(|p| p.surface == surface && p.machine.is_live())
    }

    /// Every popup whose parent is `parent` — the dismissal sweep a
    /// parent unmap/destroy runs (a menu cannot outlive its window).
    /// Returns `(client, popup object)` pairs (the dispatcher owns
    /// the wire emission).
    #[must_use]
    pub fn children_of_ids(&self, parent: ldp_compositor::surface::SurfaceId) -> Vec<(u32, u32)> {
        self.popups
            .iter()
            .filter(|p| p.parent == Some(parent))
            .map(|p| (p.client, p.object))
            .collect()
    }

    /// Drop the entry a destroyed object addressed (the dispatcher's
    /// `on_destroy` arm). Unknown pairs are a no-op — a double
    /// destroy is the client's right, never a crash.
    pub fn drop_entry(&mut self, client: u32, object: u32) {
        self.popups
            .retain(|p| !(p.client == client && p.object == object));
    }

    /// Solve a placement for `entry` at buffer size `size` against
    /// `usable_parent` (the usable area in *parent coordinates*) when
    /// the size changed since the last solve — the re-proposal
    /// doctrine. Returns the proposal to emit in `popup.configure`
    /// (`None`: no change, no emission).
    pub fn repropose(
        &mut self,
        client: u32,
        object: u32,
        size: (u32, u32),
        usable_parent: Rect,
    ) -> Option<(u32, Placement)> {
        let entry = self.entry_mut(client, object)?;
        if !entry.machine.is_live() {
            return None;
        }
        if entry.solved == size && entry.machine.pending().is_some() {
            return None; // same size, proposal already stands
        }
        entry.solved = size;
        let (serial, placement) = entry.machine.propose(size, usable_parent);
        Some((serial.0, placement))
    }

    /// The forced re-solve (the `reposition` arm): the geometry
    /// changed, so the solver runs again even at the standing size.
    /// Returns the proposal to emit (`None` when the entry is gone
    /// or dismissed, or before any size is known).
    pub fn repropose_force(
        &mut self,
        client: u32,
        object: u32,
        size: (u32, u32),
        usable_parent: Rect,
    ) -> Option<(u32, Placement)> {
        let entry = self.entry_mut(client, object)?;
        if !entry.machine.is_live() || entry.solved == (0, 0) {
            return None;
        }
        let (serial, placement) = entry.machine.propose(size, usable_parent);
        Some((serial.0, placement))
    }

    /// The solved placement for a popup surface, translated into
    /// *output* coordinates: the parent's absolute position plus the
    /// parent-relative placement (a null parent's anchor already
    /// lives on the output — the origin applies).
    #[must_use]
    pub fn resolved_position(
        &self,
        client: u32,
        object: u32,
        parent_origin: Option<(i32, i32)>,
    ) -> Option<(i32, i32)> {
        let entry = self.entry(client, object)?;
        let (_, placement) = entry.machine.pending()?;
        let (ox, oy) = parent_origin.unwrap_or((0, 0));
        Some((ox + placement.x, oy + placement.y))
    }
}

/// One served dialog (Phase 48): the wire identity, the parent the
/// dialog belongs to, and the policy machine
/// ([`ldp_shell::dialog::Dialog`] — the crate's own tested core: the
/// two-phase size proposal, the title bounds, the modality's gate).
///
/// The host keeps the *policy*; the tree keeps the *pixels* — exactly
/// the [`PopupHost`] doctrine. The dialog's surface is a root of the
/// scene like any toplevel: the tree's position is the centered
/// placement (the parent's content center, clamped into the usable
/// area — [`ldp_shell::dialog::Dialog::center_over`]), applied
/// through the pending queue so it lands with the mapping commit.
#[derive(Debug)]
pub struct ServedDialog {
    /// The owning client (u32 — the wire identity the dispatcher
    /// addresses).
    pub client: u32,
    /// The `ldp.shell.dialog` object id.
    pub object: u32,
    /// The dialog's own surface (the one `get_dialog` gave the role).
    pub surface: ldp_compositor::surface::SurfaceId,
    /// The parent toplevel's *surface* (the dialog's gating target
    /// and centering anchor — the toplevel object may be re-minted
    /// over the same surface; the window is what the dialog belongs
    /// to).
    pub parent: ldp_compositor::surface::SurfaceId,
    /// The policy machine (modality, serials, proposals).
    pub machine: ldp_shell::dialog::Dialog,
}

impl ServedDialog {
    /// Whether this entry still gates input (a live modal machine).
    #[must_use]
    pub fn gates(&self) -> bool {
        self.machine.modality() == ldp_shell::dialog::DialogModality::Modal
            && !self.machine.is_closed()
    }
}

/// The dialog-role host: every live dialog the compositor serves
/// (Phase 48 — the frozen `get_dialog` surface, finally driven).
///
/// The host is policy-only, exactly the popup pattern: no sockets, no
/// clocks (the serials draw from the machine's own `SerialClock`,
/// advanced by calls, not time). The dispatcher owns every wire
/// emission; this host answers three questions:
///
/// * **identity** — which surface is a dialog (`is_dialog_surface`),
///   so role exclusivity, the ghost fade, and the transitions
///   eligibility all name the same truth;
/// * **ownership** — which dialogs belong to a parent
///   (`children_of_ids`), so a parent's death closes its dialogs (a
///   dialog cannot outlive its window — the sheet-dies-with-window
///   doctrine);
/// * **gating** — which surface a live modal dialog gates
///   (`gating_parents`), the spec's "modal dialogs gate input
///   delivery to their parent's tree" clause. The *tree* is the
///   parent root plus the popups rooted under it; the router routes
///   roots only, so the gate is the membership the input path
///   filters on.
#[derive(Debug, Default)]
pub struct DialogHost {
    /// Every dialog entry, insertion order (closed entries stay
    /// until the object is destroyed — a close-but-not-destroyed
    /// dialog must keep answering `ack_configure` with its last
    /// proposal, exactly the popup's done-but-alive doctrine).
    dialogs: Vec<ServedDialog>,
}

impl DialogHost {
    /// An empty host.
    #[must_use]
    pub const fn new() -> DialogHost {
        DialogHost {
            dialogs: Vec::new(),
        }
    }

    /// Register a freshly minted dialog object (the `get_dialog`
    /// dispatch arm): the machine starts with the client's modality
    /// claim and the parent pairing the spec's arguments name.
    pub fn mint(
        &mut self,
        client: u32,
        object: u32,
        surface: ldp_compositor::surface::SurfaceId,
        parent: ldp_compositor::surface::SurfaceId,
        modality: ldp_shell::dialog::DialogModality,
    ) {
        let machine = ldp_shell::dialog::Dialog::new(
            ldp_shell::WindowKey::new(surface.raw()),
            ldp_shell::WindowKey::new(parent.raw()),
            modality,
        );
        self.dialogs.push(ServedDialog {
            client,
            object,
            surface,
            parent,
            machine,
        });
    }

    /// The dialog entry a wire object addresses.
    #[must_use]
    pub fn entry(&self, client: u32, object: u32) -> Option<&ServedDialog> {
        self.dialogs
            .iter()
            .find(|d| d.client == client && d.object == object)
    }

    /// The dialog entry a wire object addresses, mutably (the
    /// dispatcher's request arms).
    pub fn entry_mut(&mut self, client: u32, object: u32) -> Option<&mut ServedDialog> {
        self.dialogs
            .iter_mut()
            .find(|d| d.client == client && d.object == object)
    }

    /// The live dialog a *surface* belongs to, mutably (the surface's
    /// death closes the machine — the gating lifts).
    pub fn by_surface_mut(
        &mut self,
        surface: ldp_compositor::surface::SurfaceId,
    ) -> Option<&mut ServedDialog> {
        self.dialogs
            .iter_mut()
            .find(|d| d.surface == surface && !d.machine.is_closed())
    }

    /// The wire identity of the live dialog a *surface* belongs to:
    /// `(client, dialog object, parent surface)` — the attach path's
    /// lookup (borrow-free so the caller can consult the scene
    /// between the host calls).
    #[must_use]
    pub fn by_surface_ids(
        &self,
        surface: ldp_compositor::surface::SurfaceId,
    ) -> Option<(u32, u32, ldp_compositor::surface::SurfaceId)> {
        self.dialogs
            .iter()
            .find(|d| d.surface == surface && !d.machine.is_closed())
            .map(|d| (d.client, d.object, d.parent))
    }

    /// Whether `surface` is some live dialog's surface (the role
    /// exclusivity and transitions eligibility membership test).
    #[must_use]
    pub fn is_dialog_surface(&self, surface: ldp_compositor::surface::SurfaceId) -> bool {
        self.dialogs
            .iter()
            .any(|d| d.surface == surface && !d.machine.is_closed())
    }

    /// Every dialog whose parent is `parent` — the death sweep a
    /// parent destroy runs (a dialog cannot outlive its window).
    /// Returns `(client, dialog object)` pairs (the dispatcher owns
    /// the wire emission).
    #[must_use]
    pub fn children_of_ids(&self, parent: ldp_compositor::surface::SurfaceId) -> Vec<(u32, u32)> {
        self.dialogs
            .iter()
            .filter(|d| d.parent == parent && !d.machine.is_closed())
            .map(|d| (d.client, d.object))
            .collect()
    }

    /// The parent roots a live modal dialog gates — the input path's
    /// filter set. Only *mapped* dialogs gate (a dialog that has not
    /// committed content is not on the screen; the caller consults
    /// the tree, the host never guesses — the router's own doctrine:
    /// the scene supplies the truth).
    ///
    /// Returns `(parent surface, dialog surface)` pairs: the dialog's
    /// own surface is the gate's *authority* (the pair the mapped
    /// check reads).
    #[must_use]
    pub fn gating_pairs(
        &self,
    ) -> Vec<(
        ldp_compositor::surface::SurfaceId,
        ldp_compositor::surface::SurfaceId,
    )> {
        self.dialogs
            .iter()
            .filter(|d| d.gates())
            .map(|d| (d.parent, d.surface))
            .collect()
    }

    /// Drop the entry a destroyed object addressed (the dispatcher's
    /// `on_destroy` arm). Unknown pairs are a no-op — a double
    /// destroy is the client's right, never a crash.
    pub fn drop_entry(&mut self, client: u32, object: u32) {
        self.dialogs
            .retain(|d| !(d.client == client && d.object == object));
    }

    /// Close every live dialog whose *surface* is `surface` (the
    /// surface's death): the machines record the close (the gate
    /// lifts), the caller owns the `close` emissions.
    ///
    /// Returns the `(client, object)` pairs whose machines this call
    /// closed — only the freshly closed emit (a close is idempotent,
    /// never a double event).
    pub fn close_surface(
        &mut self,
        surface: ldp_compositor::surface::SurfaceId,
    ) -> Vec<(u32, u32)> {
        let mut closed = Vec::new();
        for d in &mut self.dialogs {
            if d.surface == surface && !d.machine.is_closed() {
                d.machine.close();
                closed.push((d.client, d.object));
            }
        }
        closed
    }

    /// Close every live dialog whose *parent* is `parent` (the
    /// parent's death): the sheet-dies-with-window doctrine. Returns
    /// the freshly closed `(client, object)` pairs.
    pub fn close_children(
        &mut self,
        parent: ldp_compositor::surface::SurfaceId,
    ) -> Vec<(u32, u32)> {
        let mut closed = Vec::new();
        for d in &mut self.dialogs {
            if d.parent == parent && !d.machine.is_closed() {
                d.machine.close();
                closed.push((d.client, d.object));
            }
        }
        closed
    }

    /// Every live dialog's surface (Phase 51, the view switch's
    /// visibility sweep — a dialog's space is its parent's, so the
    /// sweep visits the role the spaces machine never tracked).
    #[must_use]
    pub fn surface_ids(&self) -> Vec<ldp_compositor::surface::SurfaceId> {
        self.dialogs.iter().map(|d| d.surface).collect()
    }
}

/// A served toplevel (Phase 49, the states arm): the wire object, its
/// surface, the policy machine
/// ([`ldp_shell::toplevel::Toplevel`] — the crate's own tested core:
/// the intent flags, the two-phase proposals, the bounded strings),
/// and the two serving truths the machine does not own.
///
/// * `decoration` — the client's `get_toplevel` choice (SSD or CSD):
///   every proposal's insets derive from it, so the host records it
///   once at mint (the wire argument was previously discarded).
/// * `restore` — the position the window held when it first engaged
///   a server-geometry state (`maximize`/`fullscreen`): the un-verbs
///   return there. A chain of geometry verbs restores to the one
///   point where server geometry began, never to a position in
///   between.
///
/// The host keeps the *policy*; the tree keeps the *pixels* — the
/// popup and dialog doctrine, unchanged. The machine's `commit()`
/// (fired at the surface's commit after an ack) is the realization
/// seam; the dispatcher owns every wire emission.
#[derive(Debug)]
pub struct ServedToplevel {
    /// The owning client (u32 — the wire identity the dispatcher
    /// addresses).
    pub client: u32,
    /// The `ldp.shell.toplevel` object id.
    pub object: u32,
    /// The window's surface (the one `get_toplevel` gave the role).
    pub surface: ldp_compositor::surface::SurfaceId,
    /// The policy machine (states, serials, proposals, hints).
    pub machine: ldp_shell::toplevel::Toplevel,
    /// The decoration mode chosen at creation (the insets' input).
    pub decoration: ldp_shell::ssd::DecorationMode,
    /// The pre-geometry position (`None` while the window has never
    /// been server-geometried).
    pub restore: Option<(i32, i32)>,
    /// The floating size at the moment server geometry first engaged
    /// (Phase 50 — the demotion's restore answer, captured beside the
    /// restore point; the operator's drag on a geometry-stated window
    /// consumes it, a fresh engagement re-captures it).
    pub restore_size: Option<(u32, u32)>,
    /// The position the live drag holds for the next realize (Phase 50
    /// — a resize's anchor: the engaged left/top edges' target,
    /// realized together with the client's committed size; a geometry
    /// verb clears it — the verbs supersede the hand).
    pub drag_pos: Option<(i32, i32)>,
}

/// The toplevel-role host (Phase 49): every live `ldp.shell.toplevel`
/// object — the states arm's serving surface. Policy-only, exactly the
/// popup/dialog pattern: no sockets, no clocks of its own (the first
/// proposal's serial comes from the seat's interaction clock through
/// `propose_at`; later ones from the machine's own clock). The
/// dispatcher mints and drives it; the tree carries the positions.
#[derive(Debug, Default)]
pub struct ToplevelHost {
    /// Every entry, insertion order (a destroyed object drops its
    /// entry; a re-mint over the same surface is a fresh one).
    toplevels: Vec<ServedToplevel>,
}

impl ToplevelHost {
    /// An empty host.
    #[must_use]
    pub const fn new() -> ToplevelHost {
        ToplevelHost {
            toplevels: Vec::new(),
        }
    }

    /// Register a freshly minted toplevel object (the `get_toplevel`
    /// dispatch arm): the machine starts clean (no states, no
    /// proposals — the mint arm makes the first one), and the host
    /// records the client's decoration choice.
    pub fn mint(
        &mut self,
        client: u32,
        object: u32,
        surface: ldp_compositor::surface::SurfaceId,
        decoration: ldp_shell::ssd::DecorationMode,
    ) {
        let machine = ldp_shell::toplevel::Toplevel::new(ldp_shell::WindowKey::new(surface.raw()));
        self.toplevels.push(ServedToplevel {
            client,
            object,
            surface,
            machine,
            decoration,
            restore: None,
            restore_size: None,
            drag_pos: None,
        });
    }

    /// The entry a wire object addresses.
    #[must_use]
    pub fn entry(&self, client: u32, object: u32) -> Option<&ServedToplevel> {
        self.toplevels
            .iter()
            .find(|t| t.client == client && t.object == object)
    }

    /// The entry a wire object addresses, mutably (the dispatcher's
    /// request arms).
    pub fn entry_mut(&mut self, client: u32, object: u32) -> Option<&mut ServedToplevel> {
        self.toplevels
            .iter_mut()
            .find(|t| t.client == client && t.object == object)
    }

    /// The live toplevel a *surface* belongs to, mutably (the commit
    /// hook's realize path and the visibility sync).
    pub fn by_surface_mut(
        &mut self,
        surface: ldp_compositor::surface::SurfaceId,
    ) -> Option<&mut ServedToplevel> {
        self.toplevels.iter_mut().find(|t| t.surface == surface)
    }

    /// The live toplevel a *surface* belongs to (the policy reads).
    #[must_use]
    pub fn by_surface(
        &self,
        surface: ldp_compositor::surface::SurfaceId,
    ) -> Option<&ServedToplevel> {
        self.toplevels.iter().find(|t| t.surface == surface)
    }

    /// The surface a wire object addresses (the routing lookups).
    #[must_use]
    pub fn surface_of(
        &self,
        client: u32,
        object: u32,
    ) -> Option<ldp_compositor::surface::SurfaceId> {
        self.toplevels
            .iter()
            .find(|t| t.client == client && t.object == object)
            .map(|t| t.surface)
    }

    /// Drop the entry a destroyed *object* addressed (the
    /// dispatcher's `on_destroy` arm). Unknown pairs are a no-op — a
    /// double destroy is the client's right, never a crash.
    pub fn drop_entry(&mut self, client: u32, object: u32) {
        self.toplevels
            .retain(|t| !(t.client == client && t.object == object));
    }

    /// Every live entry's surface (Phase 51, the view switch's
    /// visibility sweep — the sweep visits the spaces residents).
    #[must_use]
    pub fn surface_ids(&self) -> Vec<ldp_compositor::surface::SurfaceId> {
        self.toplevels.iter().map(|t| t.surface).collect()
    }

    /// Drop every entry whose *surface* is `surface` (the surface's
    /// death — the window is gone; its states die with it). Returns
    /// the dropped `(client, object)` pairs (the dispatcher's
    /// bookkeeping; nothing is emitted — the client's own surface
    /// death already told it).
    pub fn drop_surface(&mut self, surface: ldp_compositor::surface::SurfaceId) -> Vec<(u32, u32)> {
        let mut dropped = Vec::new();
        self.toplevels.retain(|t| {
            if t.surface == surface {
                dropped.push((t.client, t.object));
                false
            } else {
                true
            }
        });
        dropped
    }
}

// ---- Phase 50 — the operator's hand ----------------------------------------

/// The operator's keep band: how much of a dragged window stays
/// reachable inside the work area (logical pixels, scaled at the
/// clamp site) — the title grip the operator can always find again,
/// the DWM/macOS "you cannot lose your window off-screen" doctrine.
pub const DRAG_KEEP_LOGICAL: i32 = 48;

/// The drag mode one pointer grip drives: the title-bar move or the
/// edge-grip resize.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragMode {
    /// The whole window follows the pointer (server truth, immediate).
    Move,
    /// The engaged edges follow the pointer; the opposite corner
    /// anchors (proposals at the pointer's cadence, the client's
    /// ack+commit realizes).
    Resize(ldp_shell::toplevel::ResizeEdge),
}

/// One live interactive drag (Phase 50 — the operator's hand): the
/// grip a `toplevel.start_move`/`start_resize` minted, the geometry
/// truths the pointer's deltas derive from, and the proposal state
/// the resize's cadence tracks. The host is a *serving* structure
/// exactly like the popup/dialog/toplevel hosts — policy only, no
/// sockets, no clocks; the input pump advances it, the dispatcher
/// mints and retires it.
#[derive(Debug)]
pub struct LiveDrag {
    /// The owning client (u32 — the wire identity whose toplevel
    /// object the drag drives).
    pub client: u32,
    /// The `ldp.shell.toplevel` object id.
    pub object: u32,
    /// The dragged window's surface.
    pub surface: ldp_compositor::surface::SurfaceId,
    /// The mode (move or edge-grip resize).
    pub mode: DragMode,
    /// The pointer position (output coordinates) at the grip.
    pub pointer_start: (f32, f32),
    /// The window's rect (physical tree space) at the grip — the
    /// deltas' anchor. For the demotion drag this is the *anchored*
    /// rect (the position the restore size maps the pointer's grip
    /// onto, the size the client has yet to commit), by the same
    /// doctrine.
    pub window_start: Rect,
    /// The last proposed content size (logical) — the resize's
    /// change detection (a still-target motion proposes nothing).
    pub last_size: (u32, u32),
    /// Whether any resize proposal went out (the release's final
    /// proposal only emits when the drag ever spoke).
    pub proposed: bool,
}

impl LiveDrag {
    /// The drag's deltas from its start: the pointer's current
    /// position against the grip's, rounded to physical integers
    /// (the tree's space).
    pub fn delta(&self, pointer: (f32, f32)) -> (i32, i32) {
        let dx = (pointer.0 - self.pointer_start.0).round();
        let dy = (pointer.1 - self.pointer_start.1).round();
        (dx as i32, dy as i32)
    }
}

/// The interactive-drag host (Phase 50): the one live drag the one
/// pointer grip drives — a singleton by construction (one hand), the
/// supersede doctrine `begin` returns the retired drag for.
#[derive(Debug, Default)]
pub struct DragHost {
    live: Option<LiveDrag>,
}

impl DragHost {
    /// An empty host.
    #[must_use]
    pub const fn new() -> DragHost {
        DragHost { live: None }
    }

    /// Begin a drag, retiring any live one (one hand: a fresh grip
    /// supersedes the old — the caller ends the returned drag
    /// silently, the new regime's proposals supersede its own).
    pub fn begin(&mut self, drag: LiveDrag) -> Option<LiveDrag> {
        self.live.replace(drag)
    }

    /// The live drag, if any.
    #[must_use]
    pub fn live(&self) -> Option<&LiveDrag> {
        self.live.as_ref()
    }

    /// The live drag, mutably (the pump's advance).
    pub fn live_mut(&mut self) -> Option<&mut LiveDrag> {
        self.live.as_mut()
    }

    /// The dragged surface (the death sweeps' check).
    #[must_use]
    pub fn surface(&self) -> Option<ldp_compositor::surface::SurfaceId> {
        self.live.as_ref().map(|d| d.surface)
    }

    /// End the drag (the release, the death, the supersede): the
    /// live drag out, `None` when none was live (idempotent).
    pub fn end(&mut self) -> Option<LiveDrag> {
        self.live.take()
    }
}

/// Clamp a dragged position so the window's overlap with the usable
/// area never drops below `keep` physical pixels on either axis —
/// the operator can always find the title grip again. A window
/// smaller than twice the band stays fully inside (nothing to keep).
/// Pure arithmetic, pin-tested.
#[must_use]
#[allow(clippy::many_single_char_names)] // a rect clamp: the axis names are the math's own
pub fn clamp_drag_position(x: i32, y: i32, w: i32, h: i32, usable: Rect, keep: i32) -> (i32, i32) {
    let (l, r) = (usable.x, usable.x + usable.w as i32);
    let (t, b) = (usable.y, usable.y + usable.h as i32);
    let clamp_axis = |pos: i32, size: i32, lo: i32, hi: i32| -> i32 {
        if size <= 0 {
            return pos;
        }
        if size < 2 * keep {
            // Too small to band: fully inside.
            pos.clamp(lo, (hi - size).max(lo))
        } else {
            // At least `keep` px of overlap, whichever side escapes.
            pos.clamp(lo - size + keep, hi - keep)
        }
    };
    (clamp_axis(x, w, l, r), clamp_axis(y, h, t, b))
}

/// The interactive resize's target algebra (Phase 50): the engaged
/// edges follow the pointer's delta, the opposite borders anchor.
/// The result is the *unclamped* physical target (the clamps — the
/// client's own min/max grammar, the work area — apply in the pump,
/// where the machine's hints live). Pure arithmetic, pin-tested.
#[must_use]
pub fn resize_target(
    start: Rect,
    edges: ldp_shell::toplevel::ResizeEdge,
    dx: i32,
    dy: i32,
) -> Rect {
    let mut x = start.x;
    let mut y = start.y;
    let mut w = start.w as i32;
    let mut h = start.h as i32;
    if edges.horizontal() {
        if edges.grabs_left() {
            w -= dx;
            x += dx;
        } else {
            w += dx;
        }
    }
    if edges.vertical() {
        if edges.grabs_top() {
            h -= dy;
            y += dy;
        } else {
            h += dy;
        }
    }
    Rect::new(x, y, w.max(1) as u32, h.max(1) as u32)
}

/// The toplevel `configure`'s ten-argument wire shape (the schema's
/// declaration, the machine's proposal — one shared constructor for
/// the dispatcher's emissions and the drag pump's outbox entries; a
/// partial emission is a wire violation the client's validator
/// rejects).
#[must_use]
pub fn toplevel_configure_values(c: &ldp_shell::toplevel::Configure) -> Vec<ldp_core::wire::Value> {
    use ldp_core::wire::Value;
    vec![
        Value::Uint32(c.serial.0),
        Value::Bitset(c.states.0),
        Value::Uint32(c.width),
        Value::Uint32(c.height),
        Value::Int32(c.insets.left as i32),
        Value::Int32(c.insets.top as i32),
        Value::Int32(c.insets.right as i32),
        Value::Int32(c.insets.bottom as i32),
        Value::Uint32(c.workspace),
        Value::Object(c.output),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use ldp_shell::popup::{Anchor, Gravity};

    // ---- the popup host (Phase 36) ----------------------------------

    fn surface_id(n: u64) -> ldp_compositor::surface::SurfaceId {
        ldp_compositor::surface::SurfaceId::from_raw(n)
    }

    /// The all-strategies constraint grant (the X11 OR window's).
    fn all_constraints() -> PopupConstraints {
        use ldp_core::bitset::Bitset128;
        PopupConstraints(Bitset128::single(0).with(1).with(2).with(3).with(4).with(5))
    }

    /// A bottom-right-gravity geometry anchored at a rect's bottom.
    fn menu_geometry(anchor_rect: Rect) -> PopupGeometry {
        PopupGeometry {
            anchor_rect,
            anchor: Anchor::BottomLeft,
            gravity: Gravity::BottomRight,
            offset: (0, 0),
        }
    }

    #[test]
    fn popup_host_mints_and_solves() {
        let mut host = PopupHost::new();
        let parent = surface_id(1);
        host.mint(
            7,
            40,
            surface_id(2),
            Some(parent),
            menu_geometry(Rect::new(10, 20, 8, 4)),
            all_constraints(),
        );
        // The first attach (size 60x14) proposes: the popup's
        // top-left at the anchor rect's bottom-left point, growing
        // down-right (the menu doctrine).
        let proposal = host
            .repropose(7, 40, (60, 14), Rect::new(0, 0, 800, 600))
            .expect("the first attach proposes");
        assert_eq!(proposal.1.x, 10);
        assert_eq!(proposal.1.y, 24);
        assert_eq!((proposal.1.width, proposal.1.height), (60, 14));
        // The same size again: no re-proposal (the proposal stands).
        assert!(host
            .repropose(7, 40, (60, 14), Rect::new(0, 0, 800, 600))
            .is_none());
        // A different size re-solves.
        let bigger = host
            .repropose(7, 40, (60, 30), Rect::new(0, 0, 800, 600))
            .expect("a grown menu re-solves");
        assert_eq!(bigger.1.y, 24);
    }

    #[test]
    fn popup_host_constrains_to_the_usable_area() {
        let mut host = PopupHost::new();
        host.mint(
            7,
            40,
            surface_id(2),
            None,
            menu_geometry(Rect::new(780, 580, 8, 4)),
            all_constraints(),
        );
        // The anchor sits at the usable area's bottom-right corner:
        // the solver slides the popup inside (slide_y first, then
        // flip/resize as allowed).
        let proposal = host
            .repropose(7, 40, (60, 14), Rect::new(0, 0, 800, 600))
            .expect("the offending anchor still proposes");
        // The slide keeps it attached to the anchor rect's right edge.
        assert!(
            proposal.1.x + proposal.1.width as i32 > 780,
            "still attached"
        );
        assert!(
            proposal.1.y + proposal.1.height as i32 <= 600,
            "never below the usable bottom"
        );
    }

    #[test]
    fn popup_host_dismissal_and_children() {
        let mut host = PopupHost::new();
        let parent = surface_id(1);
        host.mint(
            7,
            40,
            surface_id(2),
            Some(parent),
            menu_geometry(Rect::new(0, 0, 8, 4)),
            all_constraints(),
        );
        host.mint(
            7,
            41,
            surface_id(3),
            Some(parent),
            menu_geometry(Rect::new(4, 4, 8, 4)),
            all_constraints(),
        );
        assert_eq!(host.children_of_ids(parent), vec![(7, 40), (7, 41)]);
        assert!(host.is_popup_surface(surface_id(2)));
        // Dismiss one: no longer a live popup surface, but its entry
        // answers lookups until the object dies.
        if let Some(entry) = host.entry_mut(7, 40) {
            entry.machine.dismiss();
        }
        assert!(!host.is_popup_surface(surface_id(2)));
        assert!(host.entry(7, 40).is_some());
        // A dismissed popup never re-proposes.
        assert!(host
            .repropose(7, 40, (60, 14), Rect::new(0, 0, 800, 600))
            .is_none());
        // The parent dies: both entries dismissed, the sweep sees both.
        let children = host.children_of_ids(parent);
        assert_eq!(children.len(), 2);
        host.drop_entry(7, 40);
        host.drop_entry(7, 41);
        assert_eq!(host.children_of_ids(parent), Vec::new());
        // A double drop is a no-op.
        host.drop_entry(7, 40);
    }

    #[test]
    fn popup_host_resolved_position_translates_by_parent() {
        let mut host = PopupHost::new();
        let parent = surface_id(1);
        host.mint(
            7,
            40,
            surface_id(2),
            Some(parent),
            menu_geometry(Rect::new(100, 200, 8, 4)),
            all_constraints(),
        );
        let _ = host.repropose(7, 40, (60, 14), Rect::new(0, 0, 800, 600));
        // The parent sits at (50, 60): the popup lands at the anchor
        // point translated into output coordinates (top-left there,
        // growing down-right).
        let (x, y) = host
            .resolved_position(7, 40, Some((50, 60)))
            .expect("a standing proposal resolves");
        assert_eq!(x, 50 + 100);
        assert_eq!(y, 60 + 204);
        // A null parent's anchor lives on the output itself.
        host.mint(
            7,
            42,
            surface_id(4),
            None,
            menu_geometry(Rect::new(5, 6, 8, 4)),
            all_constraints(),
        );
        let _ = host.repropose(7, 42, (60, 14), Rect::new(0, 0, 800, 600));
        let (x, y) = host
            .resolved_position(7, 42, None)
            .expect("the null parent resolves");
        assert_eq!(x, 5);
        assert_eq!(y, 6 + 4);
    }

    #[test]
    fn dock_mode_parses() {
        assert_eq!(DockMode::parse("auto"), Some(DockMode::Auto));
        assert_eq!(DockMode::parse("off"), Some(DockMode::Off));
        assert_eq!(DockMode::parse("side"), None);
    }

    #[test]
    fn shell_default_is_off() {
        assert_eq!(ShellConfig::default().dock, DockMode::Off);
        let s = Shell::new(ShellConfig::default());
        assert!(s.dock.is_none());
    }

    #[test]
    fn relayout_phone_with_dock() {
        let mut s = Shell::new(ShellConfig {
            dock: DockMode::Auto,
            dock_thickness: 84,
        });
        s.relayout((540, 960), ldp_core::scale::ScaleFactor::IDENTITY, true);
        assert_eq!(s.layout.class, ldp_shell::DeviceClass::Phone);
        assert_eq!(s.layout.usable, Rect::new(0, 0, 540, 876));
        let dock = s.dock.as_ref().unwrap();
        assert_eq!(dock.rect, Rect::new(0, 876, 540, 84));
        // The rise starts fully below the edge.
        assert_eq!(dock.rise.pos, 84.0);
        assert!(!dock.rise.settled());
        assert_eq!(s.report(), "phone layout, dock 84 px at the bottom");
    }

    #[test]
    fn place_root_cascades_on_desktop() {
        let mut s = Shell::new(ShellConfig::default());
        s.relayout((960, 540), ldp_core::scale::ScaleFactor::IDENTITY, true);
        assert_eq!(s.place_root((400, 300)), (0, 0));
        assert_eq!(s.place_root((400, 300)), (24, 24));
        assert_eq!(s.place_root((400, 300)), (48, 48));
    }

    #[test]
    fn place_root_fills_on_phone() {
        let mut s = Shell::new(ShellConfig {
            dock: DockMode::Auto,
            dock_thickness: 84,
        });
        s.relayout((540, 960), ldp_core::scale::ScaleFactor::IDENTITY, true);
        // Above the dock: the usable origin.
        assert_eq!(s.place_root((540, 876)), (0, 0));
        assert_eq!(s.place_root((300, 400)), (0, 0));
        // An oversized app anchors too (overflow is occluded by the
        // dock, never cropped).
        assert_eq!(s.place_root((540, 960)), (0, 0));
    }

    #[test]
    fn replace_root_reanchors_and_clamps() {
        let mut phone = Shell::new(ShellConfig {
            dock: DockMode::Auto,
            dock_thickness: 84,
        });
        phone.relayout((540, 960), ldp_core::scale::ScaleFactor::IDENTITY, true);
        assert_eq!(phone.replace_root(Rect::new(30, 30, 540, 876)), (0, 0));
        let mut desk = Shell::new(ShellConfig::default());
        desk.relayout((960, 540), ldp_core::scale::ScaleFactor::IDENTITY, true);
        // A window overhanging the right/bottom edges is pushed in.
        assert_eq!(desk.replace_root(Rect::new(800, 400, 400, 300)), (560, 240));
    }

    #[test]
    fn dock_ink_is_dock_sized_with_pills() {
        let bytes = paint_dock(540, 84);
        assert_eq!(bytes.len(), 540 * 84 * 4);
        // The haze base (little-endian words).
        assert_eq!(&bytes[0..4], &HAZE_WORD.to_le_bytes());
        // The first pill's body center is pill ink (not the haze).
        let pill_w = (540 - 2 * 24 - 3 * 16) / 4;
        assert_eq!(pill_w, 111);
        let cx = 24 + pill_w / 2;
        let cy = 42;
        let px = (cy * 540 + cx) * 4;
        let word = u32::from_le_bytes(bytes[px..px + 4].try_into().unwrap());
        assert_eq!(word >> 24, PILL_ALPHA as u32);
        // A gap pixel (between pills 0 and 1) stays haze.
        let gap_x = 24 + pill_w + 8;
        let gp = (cy * 540 + gap_x) * 4;
        assert_eq!(&bytes[gp..gp + 4], &HAZE_WORD.to_le_bytes());
    }

    #[test]
    fn degenerate_dock_keeps_the_haze() {
        // Too thin for a pill row: haze only, no panic.
        let bytes = paint_dock(200, 20);
        assert_eq!(bytes.len(), 200 * 20 * 4);
        for chunk in bytes.chunks_exact(4) {
            assert_eq!(chunk, &HAZE_WORD.to_le_bytes()[..]);
        }
        // Too narrow for the row.
        let narrow = paint_dock(40, 84);
        assert_eq!(narrow.len(), 40 * 84 * 4);
        assert_eq!(&narrow[0..4], &HAZE_WORD.to_le_bytes());
    }

    #[test]
    fn dock_geometry_matches_storage() {
        let g = dock_geometry(540, 84);
        assert_eq!(g.width(), 540);
        assert_eq!(g.height(), 84);
        assert_eq!(g.format(), FourCC::ARGB8888);
        assert_eq!(g.spanned_bytes(), 540 * 84 * 4);
    }

    #[test]
    fn dock_layer_builds() {
        let dock = SystemDock::settled(Rect::new(0, 456, 960, 84));
        let layer = dock
            .layer(dock.rect, dock.style(ldp_renderer::EffectTier::High))
            .unwrap();
        assert_eq!(layer.dest, Rect::new(0, 456, 960, 84));
        assert!(layer.style.backdrop.is_some());
        assert!(layer.style.shadow.is_none());
    }

    #[test]
    fn rise_advances_and_settles() {
        let mut dock = SystemDock::new(Rect::new(0, 876, 540, 84));
        assert_eq!(dock.rise.pos, 84.0);
        let mut repaint = Region::new();
        // 24 frames at 16 ms: the critically damped rise (ω ≈ 95/s)
        // clears the position threshold within ~8 frames and the
        // strict velocity threshold within ~16 — a quarter-second
        // intro, the iOS cadence. The approach is monotone (critical
        // damping: no overshoot below the resting place).
        let mut last = Rect::EMPTY;
        let mut prev_y = 960;
        for i in 0..24u64 {
            last = dock.advance(i * 16, &mut repaint);
            assert!(last.y >= dock.rect.y, "never above the resting place");
            assert!(last.y <= prev_y, "monotone rise");
            prev_y = last.y;
        }
        assert!(dock.rise.settled());
        assert_eq!(last, dock.rect);
        // The repaint covered the whole rise span.
        let b = repaint.bounds();
        assert!(b.h >= 84, "the rise spanned at least the dock height");
        // A settled dock no longer repaints on advance.
        let before = repaint.len();
        dock.advance(1000, &mut repaint);
        assert_eq!(repaint.len(), before);
    }

    #[test]
    fn settled_dock_renders_at_rest() {
        let mut dock = SystemDock::settled(Rect::new(0, 456, 960, 84));
        let mut repaint = Region::new();
        let dest = dock.advance(0, &mut repaint);
        assert_eq!(dest, dock.rect);
        assert!(repaint.is_empty(), "a settled dock claims no repaint");
        let dest = dock.advance(16, &mut repaint);
        assert_eq!(dest, dock.rect);
        assert!(repaint.is_empty());
    }

    // ---- the dialog host (Phase 48) ----------------------------------

    mod dialog_host {
        use super::super::*;

        fn sid(n: u64) -> ldp_compositor::surface::SurfaceId {
            ldp_compositor::surface::SurfaceId::from_raw(n)
        }

        fn modal_host() -> DialogHost {
            let mut host = DialogHost::new();
            host.mint(
                7,
                21,
                sid(2),
                sid(1),
                ldp_shell::dialog::DialogModality::Modal,
            );
            host
        }

        #[test]
        fn the_host_memberships_name_the_same_truth() {
            let host = modal_host();
            assert!(host.is_dialog_surface(sid(2)));
            assert!(!host.is_dialog_surface(sid(1)));
            assert_eq!(host.entry(7, 21).map(|e| e.parent), Some(sid(1)));
            assert_eq!(host.by_surface_ids(sid(2)), Some((7, 21, sid(1))));
            assert_eq!(host.by_surface_ids(sid(9)), None);
        }

        #[test]
        fn a_modal_dialog_gates_its_parent_only() {
            let host = modal_host();
            assert_eq!(host.gating_pairs(), vec![(sid(1), sid(2))]);
            // Modeless: no gate.
            let mut host = DialogHost::new();
            host.mint(
                7,
                21,
                sid(2),
                sid(1),
                ldp_shell::dialog::DialogModality::Modeless,
            );
            assert!(host.gating_pairs().is_empty());
        }

        #[test]
        fn the_surface_death_closes_and_only_the_fresh_do() {
            let mut host = modal_host();
            host.mint(
                7,
                22,
                sid(3),
                sid(1),
                ldp_shell::dialog::DialogModality::Modal,
            );
            // The dialog's own surface dies: exactly it closes.
            assert_eq!(host.close_surface(sid(2)), vec![(7, 21)]);
            // Idempotent: no double close.
            assert!(host.close_surface(sid(2)).is_empty());
            // The gate lifted with the machine.
            assert_eq!(host.gating_pairs(), vec![(sid(1), sid(3))]);
        }

        #[test]
        fn the_parent_death_sweeps_its_children() {
            let mut host = modal_host();
            host.mint(
                8,
                30,
                sid(4),
                sid(1),
                ldp_shell::dialog::DialogModality::Modeless,
            );
            host.mint(
                8,
                31,
                sid(5),
                sid(9),
                ldp_shell::dialog::DialogModality::Modal,
            );
            // The parent dies: its dialogs close (both modalities —
            // ownership, not gating); the stranger's does not.
            let mut closed = host.close_children(sid(1));
            closed.sort_unstable();
            assert_eq!(closed, vec![(7, 21), (8, 30)]);
            assert!(host.gating_pairs().iter().all(|(p, _)| *p == sid(9)));
        }

        #[test]
        fn the_object_destroy_drops_the_entry() {
            let mut host = modal_host();
            host.drop_entry(7, 21);
            assert!(!host.is_dialog_surface(sid(2)));
            assert!(host.gating_pairs().is_empty());
            // Unknown pairs are a no-op, never a crash.
            host.drop_entry(7, 99);
            assert_eq!(host.dialogs.len(), 0);
        }

        #[test]
        fn a_closed_dialog_keeps_answering_until_the_object_dies() {
            let mut host = modal_host();
            // The popup host's done-but-alive doctrine: the entry
            // stays (ack_configure still answers), the gate lifts.
            host.close_children(sid(1));
            assert!(!host.is_dialog_surface(sid(2)));
            assert!(host.entry(7, 21).is_some());
        }
    }

    // ---- the toplevel host (Phase 49, the states arm) --------------

    use ldp_shell::ssd::DecorationMode;
    use ldp_shell::toplevel::PolicyInputs;

    fn sid(n: u64) -> ldp_compositor::surface::SurfaceId {
        ldp_compositor::surface::SurfaceId::from_raw(n)
    }

    /// The desktop policy the served arms derive from (the testbench
    /// geometry: 1920x1080 usable, identity scale, CSD).
    fn states_inputs() -> PolicyInputs {
        PolicyInputs {
            workspace_area: Rect::new(0, 0, 1920, 1080),
            output_size: (1920, 1080),
            metrics: ldp_shell::ssd::SsdMetrics::LION,
            decoration: DecorationMode::Client,
            scale: ldp_core::scale::ScaleFactor::IDENTITY,
            workspace: 0,
            output: None,
        }
    }

    /// Two minted toplevels over two surfaces.
    fn toplevel_host() -> ToplevelHost {
        let mut host = ToplevelHost::new();
        host.mint(7, 41, sid(1), DecorationMode::Client);
        host.mint(8, 52, sid(2), DecorationMode::Server);
        host
    }

    #[test]
    fn toplevel_host_mints_and_resolves() {
        let host = toplevel_host();
        assert_eq!(host.surface_of(7, 41), Some(sid(1)));
        assert_eq!(host.by_surface(sid(2)).map(|t| t.object), Some(52));
        // The decoration choice rides the entry (the insets' input).
        assert_eq!(
            host.by_surface(sid(2)).map(|t| t.decoration),
            Some(DecorationMode::Server)
        );
        // A stranger's object resolves to nothing.
        assert_eq!(host.surface_of(9, 41), None);
    }

    #[test]
    fn the_object_destroy_drops_the_toplevel_entry() {
        let mut host = toplevel_host();
        host.drop_entry(7, 41);
        assert_eq!(host.surface_of(7, 41), None);
        assert!(host.by_surface(sid(1)).is_none());
        // The other entry survives; unknown pairs are a no-op.
        host.drop_entry(7, 99);
        assert!(host.by_surface(sid(2)).is_some());
    }

    #[test]
    fn the_surface_death_drops_its_toplevels() {
        let mut host = toplevel_host();
        // A second object over the same surface (a re-mint): both
        // die with the surface.
        host.mint(7, 63, sid(1), DecorationMode::Client);
        let mut dropped = host.drop_surface(sid(1));
        dropped.sort_unstable();
        assert_eq!(dropped, vec![(7, 41), (7, 63)]);
        assert!(host.by_surface(sid(1)).is_none());
        assert!(host.by_surface(sid(2)).is_some());
    }

    #[test]
    fn the_mint_proposal_adopts_the_seat_serial() {
        let mut host = ToplevelHost::new();
        host.mint(7, 41, sid(1), DecorationMode::Client);
        let entry = host.entry_mut(7, 41).expect("the entry");
        // The mint arm's handshake: the seat's interaction clock
        // issued serial 3; the first proposal carries exactly it, and
        // the machine's clock continues after it.
        let c1 = entry
            .machine
            .propose_at(&states_inputs(), ldp_shell::serial::Serial(3));
        assert_eq!(c1.serial, ldp_shell::serial::Serial(3));
        assert_eq!((c1.width, c1.height), (0, 0));
        assert!(!c1.states.maximized() && !c1.states.fullscreen());
        entry
            .machine
            .ack_configure(ldp_shell::serial::Serial(3))
            .unwrap();
        assert_eq!(entry.machine.commit(), Some(c1));
        // The machine continues the domain: the next proposal is 4.
        entry.machine.maximize();
        let c2 = entry.machine.propose(&states_inputs());
        assert_eq!(c2.serial, ldp_shell::serial::Serial(4));
        assert!(c2.states.maximized());
    }

    #[test]
    fn the_restore_point_survives_the_host_lookup_pattern() {
        // The restore bookkeeping the realize path drives: capture,
        // hold, clear — the host is the carrier, the dispatcher is
        // the mover.
        let mut host = ToplevelHost::new();
        host.mint(7, 41, sid(1), DecorationMode::Client);
        let entry = host.entry_mut(7, 41).expect("the entry");
        assert_eq!(entry.restore, None);
        entry.restore = Some((120, 80));
        // A fresh lookup sees the same truth (the entry is stored,
        // not rebuilt).
        assert_eq!(host.entry(7, 41).and_then(|t| t.restore), Some((120, 80)));
    }

    // ---- the drag host (Phase 50) -----------------------------------

    fn live_drag(mode: DragMode) -> LiveDrag {
        LiveDrag {
            client: 7,
            object: 41,
            surface: sid(1),
            mode,
            pointer_start: (100.0, 100.0),
            window_start: Rect::new(60, 40, 320, 240),
            last_size: (320, 240),
            proposed: false,
        }
    }

    #[test]
    fn the_drag_host_begins_ends_and_supersedes() {
        let mut host = DragHost::new();
        assert!(host.live().is_none() && host.surface().is_none());
        // One hand: a fresh grip retires the live one (the returned
        // drag is the superseded grip the caller ends silently).
        assert!(host.begin(live_drag(DragMode::Move)).is_none());
        assert_eq!(host.surface(), Some(sid(1)));
        let retired = host
            .begin(live_drag(DragMode::Resize(
                ldp_shell::toplevel::ResizeEdge::BottomRight,
            )))
            .expect("the move was retired");
        assert_eq!(retired.mode, DragMode::Move);
        assert!(matches!(
            host.live().expect("the resize").mode,
            DragMode::Resize(ldp_shell::toplevel::ResizeEdge::BottomRight)
        ));
        // The end is idempotent.
        assert_eq!(host.end().expect("the resize lives").object, 41);
        assert!(host.end().is_none());
    }

    #[test]
    fn the_drag_delta_rounds_from_the_grip() {
        let d = live_drag(DragMode::Move);
        assert_eq!(d.delta((100.0, 100.0)), (0, 0));
        assert_eq!(d.delta((160.4, 90.6)), (60, -9));
        assert_eq!(d.delta((-40.0, 140.0)), (-140, 40));
    }

    #[test]
    fn the_keep_band_holds_the_grip_reachable() {
        let usable = Rect::new(0, 0, 1920, 1040);
        // Deep inside: untouched.
        assert_eq!(
            clamp_drag_position(500, 400, 320, 240, usable, 48),
            (500, 400)
        );
        // A huge leftward drag: only 48 px of the window's right edge
        // stay reachable (the operator can always find the title
        // grip again).
        assert_eq!(
            clamp_drag_position(-5000, 400, 320, 240, usable, 48),
            (-272, 400)
        );
        // Symmetric on the bottom: 48 px of the title band stay
        // inside the area's bottom.
        assert_eq!(
            clamp_drag_position(500, 5000, 320, 240, usable, 48),
            (500, 992)
        );
        // A window smaller than twice the band stays fully inside.
        assert_eq!(clamp_drag_position(-500, -500, 60, 60, usable, 48), (0, 0));
        assert_eq!(
            clamp_drag_position(1900, 1000, 60, 60, usable, 48),
            (1860, 980)
        );
    }

    #[test]
    fn the_resize_target_algebra_per_edge() {
        use ldp_shell::toplevel::ResizeEdge as E;
        let start = Rect::new(100, 100, 300, 200);
        // The bottom-right corner follows; the top-left anchors.
        let r = resize_target(start, E::BottomRight, 60, 40);
        assert_eq!((r.x, r.y, r.w as i32, r.h as i32), (100, 100, 360, 240));
        // The top-left corner follows; the bottom-right anchors.
        let r = resize_target(start, E::TopLeft, 60, 40);
        assert_eq!((r.x, r.y, r.w as i32, r.h as i32), (160, 140, 240, 160));
        // A plain right edge: width only.
        let r = resize_target(start, E::Right, 60, 40);
        assert_eq!((r.x, r.y, r.w as i32, r.h as i32), (100, 100, 360, 200));
        // A plain top edge: height and origin only.
        let r = resize_target(start, E::Top, 60, 40);
        assert_eq!((r.x, r.y, r.w as i32, r.h as i32), (100, 140, 300, 160));
        // The bottom-left corner: x follows, y anchors, width and
        // height both engage.
        let r = resize_target(start, E::BottomLeft, 60, 40);
        assert_eq!((r.x, r.y, r.w as i32, r.h as i32), (160, 100, 240, 240));
        // Never below 1 px on an engaged axis (the drag's own floor;
        // the hints clamp above this).
        let r = resize_target(start, E::TopLeft, 5000, 5000);
        assert_eq!((r.w, r.h), (1, 1));
        assert_eq!((r.x, r.y), (5100, 5100));
    }

    #[test]
    fn the_configure_values_shape_is_the_schema_ten() {
        let mut m = ldp_shell::toplevel::Toplevel::new(ldp_shell::WindowKey::new(1));
        m.set_resizing(true);
        let c = m.propose_drag(&states_inputs(), (640, 480));
        let v = toplevel_configure_values(&c);
        assert_eq!(v.len(), 10);
        assert_eq!(v[0], ldp_core::wire::Value::Uint32(c.serial.0));
        assert_eq!(v[2], ldp_core::wire::Value::Uint32(640));
        assert_eq!(v[3], ldp_core::wire::Value::Uint32(480));
    }

    // -- Phase 52: the SSD chrome pass ---------------------------------

    /// One ink word at (x, y).
    fn word_at(ink: &[u8], w: u32, x: u32, y: u32) -> u32 {
        let off = (y * w + x) as usize * 4;
        u32::from_le_bytes([ink[off], ink[off + 1], ink[off + 2], ink[off + 3]])
    }

    /// The 1× chrome shape of a 100×50 frame with the Lion band.
    fn lion_shape(w: u32, h: u32) -> ChromeShape {
        ChromeShape {
            w,
            h,
            top: 29,
            left: 1,
            right: 1,
            bottom: 1,
            close: 20,
            margin: 5,
        }
    }

    #[test]
    fn the_chrome_ink_paints_band_ring_button_and_hole() {
        let band = ldp_shell::ssd::Insets {
            left: 1,
            top: 29,
            right: 1,
            bottom: 1,
        };
        let ink = paint_chrome(100, 50, band, lion_shape(100, 50));
        let band_word = prem_word(BAND_RGB, BAND_ALPHA);
        let ring_word = prem_word(RING_RGB, RING_ALPHA);
        let close_word = prem_word(CLOSE_RGB, CLOSE_ALPHA);
        let glyph_word = prem_word(GLYPH_RGB, GLYPH_ALPHA);
        // The title band: full width across the top.
        assert_eq!(word_at(&ink, 100, 50, 5), band_word);
        assert_eq!(word_at(&ink, 100, 0, 28), band_word);
        // The border ring: the sides and the bottom.
        assert_eq!(word_at(&ink, 100, 0, 40), ring_word);
        assert_eq!(word_at(&ink, 100, 99, 40), ring_word);
        assert_eq!(word_at(&ink, 100, 50, 49), ring_word);
        // The content hole: transparent (the client's layer above).
        assert_eq!(word_at(&ink, 100, 50, 40), 0);
        assert_eq!(word_at(&ink, 100, 30, 30), 0);
        // The close capsule at the band's right end: a point inside
        // the circle but off both glyph strokes is capsule ink, the
        // button's center carries the × glyph.
        // (100 - 5 - 20 = 75 .. 95, y 5 .. 25; circle r=10 at (85,15).)
        assert_eq!(word_at(&ink, 100, 78, 15), close_word);
        assert_eq!(word_at(&ink, 100, 85, 9), close_word);
        assert_eq!(word_at(&ink, 100, 85, 15), glyph_word);
        // The ink is tightly packed and frame-sized.
        assert_eq!(ink.len(), 100 * 50 * 4);
    }

    #[test]
    fn the_close_glyph_is_a_cross_not_a_blob() {
        let band = ldp_shell::ssd::Insets {
            left: 1,
            top: 29,
            right: 1,
            bottom: 1,
        };
        let ink = paint_chrome(100, 50, band, lion_shape(100, 50));
        let glyph_word = prem_word(GLYPH_RGB, GLYPH_ALPHA);
        let close_word = prem_word(CLOSE_RGB, CLOSE_ALPHA);
        // The glyph's arms reach the capsule's diagonals, its
        // off-diagonal corners stay capsule ink: (77, 7) sits on the
        // anti-diagonal's far corner — capsule, not glyph.
        let cx: i64 = 85;
        let cy: i64 = 15;
        for (x, y) in [(80, 10), (90, 20), (90, 10), (80, 20)] {
            let d1 = i64::from(x) - cx - (i64::from(y) - cy);
            let d2 = i64::from(x) - cx + i64::from(y) - cy;
            let w = word_at(&ink, 100, x, y);
            if d1.abs() <= 2 || d2.abs() <= 2 {
                assert_eq!(w, glyph_word, "glyph arm at ({x},{y})");
            } else {
                assert_eq!(w, close_word, "capsule body at ({x},{y})");
            }
        }
    }

    #[test]
    fn the_chrome_ink_scales_its_affordance() {
        let band = ldp_shell::ssd::Insets {
            left: 2,
            top: 58,
            right: 2,
            bottom: 2,
        };
        let shape = ChromeShape {
            w: 200,
            h: 100,
            top: 58,
            left: 2,
            right: 2,
            bottom: 2,
            close: 40,
            margin: 10,
        };
        let ink = paint_chrome(200, 100, band, shape);
        let close_word = prem_word(CLOSE_RGB, CLOSE_ALPHA);
        let glyph_word = prem_word(GLYPH_RGB, GLYPH_ALPHA);
        // 2×: the button is 40×40 at (200-10-40=150, 10); its center
        // (170, 30) carries the glyph, a point off both strokes (the
        // circle's top, r=20) stays capsule ink.
        assert_eq!(word_at(&ink, 200, 170, 15), close_word);
        assert_eq!(word_at(&ink, 200, 170, 30), glyph_word);
        // The band and ring ride their own insets.
        let band_word = prem_word(BAND_RGB, BAND_ALPHA);
        assert_eq!(word_at(&ink, 200, 100, 20), band_word);
    }

    #[test]
    fn degenerate_frames_paint_saturated_bands() {
        // A frame thinner than its own band: the painter saturates
        // per side and never writes out of bounds.
        let band = ldp_shell::ssd::Insets {
            left: 1,
            top: 29,
            right: 1,
            bottom: 1,
        };
        let ink = paint_chrome(
            4,
            3,
            band,
            ChromeShape {
                w: 4,
                h: 3,
                top: 29,
                left: 1,
                right: 1,
                bottom: 1,
                close: 20,
                margin: 5,
            },
        );
        assert_eq!(ink.len(), 4 * 3 * 4);
        // The degenerate overlap: the saturated band (top=29 over
        // h=3) covers every row, and the bottom border (b=1) claims
        // the last one back — the ring paints after the band, its
        // crisp edge winning the overlap (never outside the frame).
        let band_word = prem_word(BAND_RGB, BAND_ALPHA);
        let ring_word = prem_word(RING_RGB, RING_ALPHA);
        for y in 0..2u32 {
            for x in 0..4u32 {
                assert_eq!(word_at(&ink, 4, x, y), band_word);
            }
        }
        for x in 0..4u32 {
            assert_eq!(word_at(&ink, 4, x, 2), ring_word);
        }
    }

    #[test]
    fn prem_word_is_premultiplied() {
        assert_eq!(prem_word([255, 255, 255], 255), 0xFFFF_FFFF);
        // 50% white: 128 premultiplied (round half up).
        assert_eq!(prem_word([255, 255, 255], 128), 0x8080_8080);
        // The band's own word (the oracle the pixel tests read).
        let b = prem_word(BAND_RGB, BAND_ALPHA);
        let r = (u32::from(BAND_RGB[0]) * 250 + 127) / 255;
        assert_eq!((b >> 16) & 0xFF, r);
    }

    #[test]
    fn chrome_shapes_key_their_whole_scale_effect() {
        let insets = ldp_shell::ssd::Insets {
            left: 1,
            top: 29,
            right: 1,
            bottom: 1,
        };
        let frame = Rect::new(0, 0, 100, 50);
        let sf = |v: f32| ldp_core::scale::ScaleFactor::from_f32_lossy(v).unwrap();
        let one = ChromeShape::of(frame, insets, sf(1.0));
        let two = ChromeShape::of(frame, insets, sf(2.0));
        assert_eq!(one, lion_shape(100, 50));
        assert_ne!(one, two);
        assert_eq!(two.close, 40);
        assert_eq!(two.margin, 10);
        // Identical inputs share the key (the cache's whole point).
        assert_eq!(ChromeShape::of(frame, insets, sf(1.0)), one);
    }
}
