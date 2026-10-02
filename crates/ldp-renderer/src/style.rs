//! The Liquid style model (Phase 27): the macOS-class visual vocabulary.
//!
//! A [`LayerStyle`] is what the *system* adds around a client's pixels —
//! the material doctrine of macOS and iOS, where the system owns the
//! frosted glass, the rounded corners and the shadows, and applications
//! just draw content. Three effects make the language:
//!
//! * **Corner radius** — antialiased rounded-rectangle clipping
//!   ([`crate::effects::rounded_coverage`]),
//! * **Shadow** — a soft blurred silhouette under the layer,
//! * **Backdrop** (frost) — the backdrop *behind* a translucent layer
//!   is blurred and tinted before the layer draws over it (the
//!   frosted-glass material),
//! * **Edge light** (Phase 40) — the luminous 1-pixel hairline traced
//!   inside the rounded silhouette (the stroke that makes glass read
//!   as glass — WindowServer's material edge, DWM acrylic's 1-px
//!   border).
//!
//! Everything is parameterized by integers (`blur` is a box-blur
//! half-extent, `passes` a pass count — three box passes approximate a
//! Gaussian) so a style is deterministic, portable, and honest about
//! cost. The [`EffectTier`] encodes the low-end doctrine: a phone with
//! a weak GPU asks for `Low` and still gets the look — the blur goes
//! away, the tint veil stays.
//!
//! The default style is *plain* (radius 0, no shadow, no backdrop):
//! a layer built the Phase 26 way renders byte-identically — the whole
//! 1,727-test corpus pins that.

use ldp_core::geometry::Rect;

/// Shadow parameters: the soft silhouette under a layer.
///
/// The shadow is the layer's destination shape (rounded by
/// [`Self::radius`], which usually equals the style's corner radius),
/// offset by [`Self::offset`], blurred by `passes` separable box passes
/// of half-extent [`Self::blur`], colored [`Self::color`] at opacity
/// [`Self::alpha`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShadowParams {
    /// Corner radius of the silhouette (px).
    pub radius: u32,
    /// Box-blur half-extent per pass (px). `0` = a hard-edged shadow.
    pub blur: u32,
    /// Separable box passes (1..=3); 3 approximates a Gaussian.
    pub passes: u32,
    /// Straight shadow color.
    pub color: [u8; 3],
    /// Shadow opacity `0..=255`.
    pub alpha: u8,
    /// Offset of the silhouette relative to the layer (px, +x right,
    /// +y down). macOS shadows sit slightly below.
    pub offset: (i32, i32),
}

impl ShadowParams {
    /// The classic macOS panel shadow: soft, dark, slightly below.
    #[must_use]
    pub const fn panel() -> ShadowParams {
        ShadowParams {
            radius: 18,
            blur: 12,
            passes: 3,
            color: [0, 0, 0],
            alpha: 96,
            offset: (0, 12),
        }
    }
}

/// Backdrop (frost) parameters: the frosted-glass material.
///
/// The backdrop region under the layer is blurred, its chroma
/// re-weighed by [`Self::saturation`] (desaturated towards its
/// luminance below 255, **boosted away from it above 255** — Phase
/// 40's vibrant domain, the distance Windows' acrylic and macOS's
/// vibrant materials keep over a plain blur), tinted by
/// [`Self::tint`] at [`Self::tint_alpha`], and the result replaces
/// what shows through the translucent layer above it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BackdropParams {
    /// Box-blur half-extent per pass (px). `0` = no blur (the tier's
    /// low-end fallback: a pure tint veil).
    pub blur: u32,
    /// Separable box passes (0..=3).
    pub passes: u32,
    /// Saturation kept, the vibrant domain: `0..=255` desaturates
    /// towards luminance (`255` = the backdrop's own color, the
    /// Phase 27 identity), `256..=510` boosts *away* from luminance
    /// (`510` = the full luma-distance added once more — the acrylic
    /// family's ~1.5–2× saturation reads land here). Integer-exact:
    /// each channel extends by `(saturation - 255) / 255` of its own
    /// distance from luma, rounded half away from zero, clamped to
    /// the premultiplied domain.
    pub saturation: u16,
    /// Straight tint color (the material's own hue).
    pub tint: [u8; 3],
    /// Tint veil opacity `0..=255`.
    pub tint_alpha: u8,
}

impl BackdropParams {
    /// The classic frosted material: blurred, desaturated, lightly
    /// veiled in the system's light gray.
    #[must_use]
    pub const fn frosted_light() -> BackdropParams {
        BackdropParams {
            blur: 20,
            passes: 3,
            saturation: 128,
            tint: [0xF2, 0xF2, 0xF7],
            tint_alpha: 110,
        }
    }

    /// The vibrant light material (Phase 40): the menu glass — the
    /// backdrop blurred deep, its chroma *boosted* past itself (the
    /// 1.5× acrylic read), veiled lightly so the wallpaper's colors
    /// glow through the menu.
    #[must_use]
    pub const fn vibrant_light() -> BackdropParams {
        BackdropParams {
            blur: 22,
            passes: 3,
            saturation: 383, // 255 + 128: half the luma-distance added
            tint: [0xFA, 0xFA, 0xFF],
            tint_alpha: 76,
        }
    }

    /// The vibrant dark material (Phase 40): the control-center glass
    /// — the same boosted chroma under a heavy dark veil, the
    /// macOS `NSVisualEffectMaterial` dark-vibrant read.
    #[must_use]
    pub const fn vibrant_dark() -> BackdropParams {
        BackdropParams {
            blur: 24,
            passes: 3,
            saturation: 340, // 255 + 85: a third of the distance added
            tint: [0x25, 0x25, 0x2B],
            tint_alpha: 170,
        }
    }
}

/// Edge-light parameters (Phase 40): the luminous hairline — a
/// 1-pixel stroke traced inside the layer's rounded silhouette, the
/// light catch that separates glass from its backdrop (the macOS
/// material edge, acrylic's border light).
///
/// The width is fixed at one pixel and honest about why: the stroke
/// rides the same antialiased coverage curve the corners do, so a
/// 1-px ring resolves exactly (the coverage difference between the
/// silhouette and its 1-px erosion); wider strokes would need their
/// own coverage doctrine for no observed gain at desktop densities.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EdgeLightParams {
    /// Straight stroke color (the light itself).
    pub color: [u8; 3],
    /// Stroke opacity `0..=255`.
    pub alpha: u8,
}

impl EdgeLightParams {
    /// The light hairline: white at ~35% — the value the menu and
    /// sheet materials trace.
    #[must_use]
    pub const fn light() -> EdgeLightParams {
        EdgeLightParams {
            color: [0xFF, 0xFF, 0xFF],
            alpha: 90,
        }
    }

    /// The chrome hairline: the dock's gentler stroke.
    #[must_use]
    pub const fn chrome() -> EdgeLightParams {
        EdgeLightParams {
            color: [0xFF, 0xFF, 0xFF],
            alpha: 76,
        }
    }
}

/// What the system adds around a layer's pixels.
///
/// `LayerStyle::default()` is plain — no effect, byte-identical to the
/// Phase 26 render path. The compositor's effect policy builds styles
/// from an [`EffectTier`]; nothing in the protocol carries them (the
/// system owns the materials, exactly like macOS).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LayerStyle {
    /// Antialiased rounded-corner radius (px). `0` = square corners.
    pub corner_radius: u32,
    /// Drop shadow; `None` = none.
    pub shadow: Option<ShadowParams>,
    /// Frosted backdrop; `None` = none.
    pub backdrop: Option<BackdropParams>,
    /// The luminous hairline (Phase 40); `None` = none. Drawn over
    /// the ink at the rounded silhouette's inner 1-pixel ring —
    /// inside the ink's own bounds, so it claims no `effect_rect`
    /// spread of its own.
    pub edge_light: Option<EdgeLightParams>,
}

impl LayerStyle {
    /// Whether any effect is enabled at all.
    #[must_use]
    pub const fn is_plain(&self) -> bool {
        self.corner_radius == 0
            && self.shadow.is_none()
            && self.backdrop.is_none()
            && self.edge_light.is_none()
    }

    /// Clamp every parameter to its sane domain for a destination of
    /// `w` x `h` (the pixel pipeline never fails — out-of-domain
    /// styles clamp, the sampler doctrine):
    ///
    /// * corner radius ≤ `min(w, h) / 2` (a stadium at most),
    /// * shadow radius likewise (against its own silhouette box),
    /// * blur ≤ 64 and passes ≤ 3 (the cost ceiling a low-end device
    ///   can pay per frame),
    /// * frost blur ≤ 64, passes ≤ 3, saturation ≤ 510 (the vibrant
    ///   ceiling — one full luma-distance of boost). The hairline's
    ///   own domain (`u8` color and alpha) is already total — nothing
    ///   to clamp.
    #[must_use]
    pub fn sanitized(mut self, w: u32, h: u32) -> LayerStyle {
        let half_min = w.min(h) / 2;
        self.corner_radius = self.corner_radius.min(half_min);
        if let Some(s) = &mut self.shadow {
            s.radius = s.radius.min(half_min);
            s.blur = s.blur.min(64);
            s.passes = s.passes.min(3);
            // The offset bound is an absolute ceiling, NOT the layer's
            // own extents — a dock two pixels tall still casts its
            // shadow four pixels down (the clamp is arithmetic sanity
            // against i32 overflow in the rect math, nothing more).
            s.offset.0 = s.offset.0.clamp(-4096, 4096);
            s.offset.1 = s.offset.1.clamp(-4096, 4096);
        }
        if let Some(b) = &mut self.backdrop {
            b.blur = b.blur.min(64);
            b.passes = b.passes.min(3);
            b.saturation = b.saturation.min(510);
        }
        self
    }

    /// The layer's *ink* bounds: where its own pixels land.
    #[must_use]
    pub fn ink_rect(&self, dest: Rect) -> Rect {
        dest
    }

    /// The layer's *effect* bounds: the destination unioned with the
    /// shadow's extent (the silhouette box translated by the offset and
    /// padded by `blur * passes` on every side), or the destination
    /// when no shadow. Damage that covers this rect guarantees the
    /// effects re-render fully.
    #[must_use]
    pub fn effect_rect(&self, dest: Rect) -> Rect {
        let Some(s) = self.shadow else {
            return dest;
        };
        let pad = s.blur.saturating_mul(s.passes);
        let sx0 = dest.x + s.offset.0 - pad as i32;
        let sy0 = dest.y + s.offset.1 - pad as i32;
        let sx1 = dest.x + dest.w as i32 + s.offset.0 + pad as i32;
        let sy1 = dest.y + dest.h as i32 + s.offset.1 + pad as i32;
        // Union with the destination: both the ink and the shadow
        // re-render when this rect is damaged.
        let x0 = sx0.min(dest.x);
        let y0 = sy0.min(dest.y);
        let x1 = sx1.max(dest.x + dest.w as i32);
        let y1 = sy1.max(dest.y + dest.h as i32);
        Rect::new(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32)
    }
}

/// The quality tier of the visual language — the low-end doctrine.
///
/// The tiers trade blur passes for reach: a phone GPU serves `High`,
/// a phone CPU serves `Medium` on small outputs, `Low` keeps the
/// identity (tint veil, hard shadow) where blur would stutter, and
/// `Minimal` is the plain Phase 26 path (the library default —
/// deterministic CI pixels).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum EffectTier {
    /// No effects at all: the plain path, byte-identical to Phase 26.
    /// The `CompositorConfig` library default.
    Minimal,
    /// The identity without blur: tint-veil frost (blur 0), hard
    /// 1-pass shadows, corners allowed. For the weakest devices.
    Low,
    /// Blur at half strength (2 passes, radius 10): the CPU tier on
    /// phone-sized outputs.
    Medium,
    /// The full language: 3-pass blur, soft shadows, frosted glass.
    /// The GPU tier.
    High,
}

/// The system's material family (Phase 40): the named vocabulary
/// every surface role speaks — "backdrop blur everywhere the giants
/// mean it", one resolution instead of two ad-hoc styles.
///
/// The family's doctrine:
///
/// * [`Material::Panel`] and [`Material::Sheet`] are the Phase 27
///   policy verbatim — [`EffectTier::opaque_style`] and
///   [`EffectTier::translucent_style`] respectively, byte-equal at
///   every tier (the legacy contract, pinned);
/// * [`Material::Menu`] is the popup glass: the **vibrant** frost
///   (saturation boosted past the backdrop's own chroma — the
///   acrylic distance) plus the luminous hairline. Its shadow
///   parameters equal `Sheet`'s at every tier, so the damage
///   expansion (which resolves styles without popup knowledge)
///   computes identical `effect_rect`s — repaint spread is a
///   function of the shadow alone, and the two agree;
/// * [`Material::VibrantDark`] is the control-center glass: boosted
///   chroma under a heavy dark veil;
/// * [`Material::Chrome`] is the dock: `Sheet`'s frost, the shadow
///   cleared (the bar hugs the edge — the Phase 28 doctrine), the
///   chrome hairline tracing its top edge.
///
/// `Minimal` resolves every material to the plain style — the
/// library default keeps its byte-exact legacy pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Material {
    /// An opaque window: corners and a shadow (the Phase 27 policy).
    Panel,
    /// A translucent surface: the frosted sheet (the Phase 27
    /// policy).
    Sheet,
    /// A menu or popup: the vibrant light glass with the hairline.
    Menu,
    /// A dark control-center panel: the vibrant dark glass.
    VibrantDark,
    /// The system chrome (the dock): frost, no shadow, the chrome
    /// hairline.
    Chrome,
}

impl Material {
    /// The style this material serves at `tier`.
    #[must_use]
    pub fn style(&self, tier: EffectTier) -> LayerStyle {
        match self {
            // The legacy pair, verbatim.
            Material::Panel => tier.opaque_style(),
            Material::Sheet => tier.translucent_style(),
            Material::Menu => {
                let mut style = tier.translucent_style();
                if tier == EffectTier::Minimal {
                    return style; // plain — the library default's bytes
                }
                style.backdrop = Some(match tier {
                    EffectTier::Low => BackdropParams {
                        blur: 0,
                        passes: 0,
                        saturation: 383,
                        tint: [0xF5, 0xF5, 0xFA],
                        tint_alpha: 120,
                    },
                    EffectTier::Medium => BackdropParams {
                        blur: 10,
                        passes: 2,
                        saturation: 383,
                        tint: [0xF7, 0xF7, 0xFC],
                        tint_alpha: 96,
                    },
                    _ => BackdropParams::vibrant_light(),
                });
                style.corner_radius = match tier {
                    EffectTier::Low => 8,
                    EffectTier::Medium => 10,
                    _ => 12,
                };
                style.edge_light = Some(EdgeLightParams::light());
                style
            }
            Material::VibrantDark => {
                let mut style = tier.translucent_style();
                if tier == EffectTier::Minimal {
                    return style;
                }
                style.backdrop = Some(match tier {
                    EffectTier::Low => BackdropParams {
                        blur: 0,
                        passes: 0,
                        saturation: 340,
                        tint: [0x25, 0x25, 0x2B],
                        tint_alpha: 180,
                    },
                    EffectTier::Medium => BackdropParams {
                        blur: 10,
                        passes: 2,
                        saturation: 340,
                        tint: [0x25, 0x25, 0x2B],
                        tint_alpha: 175,
                    },
                    _ => BackdropParams::vibrant_dark(),
                });
                style.edge_light = Some(EdgeLightParams::light());
                style
            }
            Material::Chrome => {
                let mut style = tier.translucent_style();
                style.shadow = None; // the bar hugs the edge
                if tier != EffectTier::Minimal {
                    style.edge_light = Some(EdgeLightParams::chrome());
                }
                style
            }
        }
    }
}

impl EffectTier {
    /// The style an opaque window gets at this tier.
    ///
    /// Opaque windows get corners and a shadow; the frost is the
    /// translucent surfaces' material (see [`Self::translucent_style`]).
    #[must_use]
    pub fn opaque_style(&self) -> LayerStyle {
        match self {
            EffectTier::Minimal => LayerStyle::default(),
            EffectTier::Low => LayerStyle {
                corner_radius: 12,
                shadow: Some(ShadowParams {
                    radius: 12,
                    blur: 0,
                    passes: 1,
                    color: [0, 0, 0],
                    alpha: 80,
                    offset: (0, 4),
                }),
                backdrop: None,
                edge_light: None,
            },
            EffectTier::Medium => LayerStyle {
                corner_radius: 16,
                shadow: Some(ShadowParams {
                    radius: 16,
                    blur: 6,
                    passes: 2,
                    color: [0, 0, 0],
                    alpha: 90,
                    offset: (0, 8),
                }),
                backdrop: None,
                edge_light: None,
            },
            EffectTier::High => LayerStyle {
                corner_radius: 18,
                shadow: Some(ShadowParams::panel()),
                backdrop: None,
                edge_light: None,
            },
        }
    }

    /// The style a translucent surface gets at this tier: the opaque
    /// vocabulary plus the frosted backdrop (the material a dock, a
    /// control center, or a translucent panel shows through).
    #[must_use]
    pub fn translucent_style(&self) -> LayerStyle {
        let mut style = self.opaque_style();
        style.backdrop = match self {
            EffectTier::Minimal => None,
            EffectTier::Low => Some(BackdropParams {
                blur: 0,
                passes: 0,
                saturation: 128,
                tint: [0xEE, 0xEE, 0xF2],
                tint_alpha: 140,
            }),
            EffectTier::Medium => Some(BackdropParams {
                blur: 10,
                passes: 2,
                saturation: 140,
                tint: [0xF0, 0xF0, 0xF5],
                tint_alpha: 120,
            }),
            EffectTier::High => Some(BackdropParams::frosted_light()),
        };
        style
    }

    /// The honest one-line report of what this tier serves.
    #[must_use]
    pub fn report(&self) -> &'static str {
        match self {
            EffectTier::Minimal => "minimal (plain pixels, Phase 26 path)",
            EffectTier::Low => "low (tint-veil frost, hard shadows, rounded corners)",
            EffectTier::Medium => "medium (2-pass blur frost, soft shadows, corners)",
            EffectTier::High => "high (3-pass blur frost, panel shadows, corners)",
        }
    }
}

/// The CLI-facing choice for `--effects`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectChoice {
    /// Resolve the tier from the machine: GL renderer or a small
    /// output serves `High`; software on a large output serves
    /// `Medium`; the headless CI default stays `Minimal`.
    Auto,
    /// A fixed tier.
    Tier(EffectTier),
}

impl EffectChoice {
    /// Parse `--effects` values.
    ///
    /// # Errors
    /// `None` for an unknown value (the caller prints usage).
    #[must_use]
    pub fn parse(value: &str) -> Option<EffectChoice> {
        match value {
            "auto" => Some(EffectChoice::Auto),
            "high" => Some(EffectChoice::Tier(EffectTier::High)),
            "medium" => Some(EffectChoice::Tier(EffectTier::Medium)),
            "low" => Some(EffectChoice::Tier(EffectTier::Low)),
            "minimal" => Some(EffectChoice::Tier(EffectTier::Minimal)),
            _ => None,
        }
    }

    /// Resolve the auto doctrine against the machine facts.
    ///
    /// The heuristic is deliberately boring and documented: a hardware
    /// (GL) renderer serves the full language; a software renderer on
    /// a phone-sized output (≤ 2.5 MP — the low-end device class this
    /// exists for) serves `Medium`; software on a desktop-sized output
    /// serves `Low` (the blur would not hold 60 Hz on a weak CPU at
    /// 4K); the headless mock path (no GL attempt, test discipline)
    /// serves `Minimal` only when the caller asked for nothing —
    /// explicit tiers always win.
    #[must_use]
    pub fn resolve(
        &self,
        hardware_renderer: bool,
        output_pixels: u64,
        headless: bool,
    ) -> EffectTier {
        match self {
            EffectChoice::Tier(t) => *t,
            EffectChoice::Auto => {
                if hardware_renderer {
                    EffectTier::High
                } else if headless {
                    EffectTier::Minimal
                } else if output_pixels <= 2_500_000 {
                    EffectTier::Medium
                } else {
                    EffectTier::Low
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_style_is_plain() {
        assert!(LayerStyle::default().is_plain());
        assert_eq!(LayerStyle::default().corner_radius, 0);
        assert!(LayerStyle::default().shadow.is_none());
        assert!(LayerStyle::default().backdrop.is_none());
    }

    #[test]
    fn sanitization_clamps_the_domain() {
        let s = LayerStyle {
            corner_radius: 900,
            shadow: Some(ShadowParams {
                radius: 900,
                blur: 999,
                passes: 9,
                color: [0; 3],
                alpha: 255,
                offset: (i32::MAX, i32::MIN),
            }),
            backdrop: Some(BackdropParams {
                blur: 999,
                passes: 9,
                saturation: 900,
                tint: [0; 3],
                tint_alpha: 255,
            }),
            edge_light: Some(EdgeLightParams {
                color: [0; 3],
                alpha: 255,
            }),
        }
        .sanitized(100, 40);
        assert_eq!(s.corner_radius, 20, "radius clamps to min(w,h)/2");
        let sh = s.shadow.expect("shadow survives clamped");
        assert_eq!(sh.radius, 20);
        assert_eq!(sh.blur, 64, "blur cost ceiling");
        assert_eq!(sh.passes, 3, "pass ceiling");
        assert_eq!(
            sh.offset,
            (4096, -4096),
            "offset clamps to the geometry ceiling"
        );
        let b = s.backdrop.expect("backdrop survives clamped");
        assert_eq!((b.blur, b.passes), (64, 3));
        assert_eq!(b.saturation, 510, "the vibrant ceiling");
        assert!(s.edge_light.is_some(), "the hairline survives");
    }

    #[test]
    fn effect_rect_contains_the_destination() {
        let style = EffectTier::High.opaque_style();
        let dest = Rect::new(10, 10, 50, 50);
        let eff = style.effect_rect(dest);
        assert!(eff.x <= dest.x && eff.y <= dest.y);
        assert!(eff.x + eff.w as i32 >= dest.x + dest.w as i32);
        assert!(eff.y + eff.h as i32 >= dest.y + dest.h as i32);
        // The shadow offsets down: the pad below must exceed the pad above.
        assert!(eff.y + eff.h as i32 - (dest.y + dest.h as i32) > dest.y - eff.y);
    }

    #[test]
    fn plain_effect_rect_is_the_destination() {
        let dest = Rect::new(3, 4, 5, 6);
        assert_eq!(LayerStyle::default().effect_rect(dest), dest);
        assert_eq!(LayerStyle::default().ink_rect(dest), dest);
    }

    #[test]
    fn tiers_form_a_quality_ladder() {
        assert!(EffectTier::Minimal < EffectTier::Low);
        assert!(EffectTier::Low < EffectTier::Medium);
        assert!(EffectTier::Medium < EffectTier::High);
        // Minimal is plain; every other tier styles something.
        assert!(EffectTier::Minimal.opaque_style().is_plain());
        assert!(!EffectTier::Low.opaque_style().is_plain());
        // Frost is the translucent tier's addition.
        assert!(EffectTier::High.translucent_style().backdrop.is_some());
        assert!(EffectTier::Minimal.translucent_style().backdrop.is_none());
        // Low tier frost carries no blur (the low-end identity).
        let low = EffectTier::Low
            .translucent_style()
            .backdrop
            .expect("low frost");
        assert_eq!((low.blur, low.passes), (0, 0));
        // Phase 27's styles never traced a hairline (the legacy
        // contract — the family's new materials own it).
        for tier in [
            EffectTier::Minimal,
            EffectTier::Low,
            EffectTier::Medium,
            EffectTier::High,
        ] {
            assert!(tier.opaque_style().edge_light.is_none());
            assert!(tier.translucent_style().edge_light.is_none());
        }
    }

    // ---- the material family (Phase 40) -----------------------------

    const TIERS: [EffectTier; 4] = [
        EffectTier::Minimal,
        EffectTier::Low,
        EffectTier::Medium,
        EffectTier::High,
    ];

    #[test]
    fn panel_and_sheet_are_the_legacy_policy_verbatim() {
        for tier in TIERS {
            assert_eq!(Material::Panel.style(tier), tier.opaque_style());
            assert_eq!(Material::Sheet.style(tier), tier.translucent_style());
        }
    }

    #[test]
    fn minimal_serves_every_material_plain() {
        for material in [
            Material::Panel,
            Material::Sheet,
            Material::Menu,
            Material::VibrantDark,
            Material::Chrome,
        ] {
            assert!(
                material.style(EffectTier::Minimal).is_plain(),
                "the library default keeps its bytes"
            );
        }
    }

    #[test]
    fn menu_shadows_match_the_sheet_at_every_tier() {
        // The damage-equivalence constraint: the repaint spread is a
        // function of the shadow alone, so a damage path that resolves
        // Sheet where the draw path resolves Menu computes identical
        // effect rects.
        for tier in TIERS {
            assert_eq!(
                Material::Menu.style(tier).shadow,
                tier.translucent_style().shadow
            );
            assert_eq!(
                Material::VibrantDark.style(tier).shadow,
                tier.translucent_style().shadow
            );
        }
    }

    #[test]
    fn the_vibrant_materials_boost_and_trace() {
        let menu = Material::Menu.style(EffectTier::High);
        let backdrop = menu.backdrop.expect("the menu glass");
        assert!(backdrop.saturation > 255, "past the backdrop's own chroma");
        assert_eq!(backdrop, BackdropParams::vibrant_light());
        assert_eq!(menu.edge_light, Some(EdgeLightParams::light()));
        assert_eq!(menu.corner_radius, 12, "menus are crisper than windows");
        // The dark glass boosts less, veils heavier.
        let dark = Material::VibrantDark.style(EffectTier::High);
        let dark_backdrop = dark.backdrop.expect("the dark glass");
        assert_eq!(dark_backdrop.saturation, 340);
        assert_eq!(dark_backdrop.tint_alpha, 170);
        assert_eq!(dark.edge_light, Some(EdgeLightParams::light()));
        // Chrome: sheet frost, shadow cleared, the gentler stroke.
        let chrome = Material::Chrome.style(EffectTier::High);
        assert_eq!(chrome.backdrop, tier_frost(EffectTier::High));
        assert!(chrome.shadow.is_none(), "the bar hugs the edge");
        assert_eq!(chrome.edge_light, Some(EdgeLightParams::chrome()));
    }

    /// The tier's own translucent frost (the sheet's).
    fn tier_frost(tier: EffectTier) -> Option<BackdropParams> {
        tier.translucent_style().backdrop
    }

    #[test]
    fn low_tier_keeps_the_vibrant_identity_without_blur() {
        let menu = Material::Menu.style(EffectTier::Low);
        let backdrop = menu.backdrop.expect("the veil stands");
        assert_eq!((backdrop.blur, backdrop.passes), (0, 0));
        assert!(backdrop.saturation > 255, "the boost rides the veil");
        assert!(menu.edge_light.is_some(), "the hairline is the identity");
    }

    #[test]
    fn vibrant_constructors_are_documented_values() {
        let light = BackdropParams::vibrant_light();
        assert_eq!(light.saturation, 383); // 255 + 128
        let dark = BackdropParams::vibrant_dark();
        assert_eq!(dark.saturation, 340); // 255 + 85
                                          // The Phase 27 material keeps its desaturating identity.
        assert_eq!(BackdropParams::frosted_light().saturation, 128);
        // The hairline values.
        assert_eq!(EdgeLightParams::light().alpha, 90);
        assert_eq!(EdgeLightParams::chrome().alpha, 76);
    }

    #[test]
    fn choice_parse_and_resolve() {
        assert_eq!(EffectChoice::parse("auto"), Some(EffectChoice::Auto));
        assert_eq!(
            EffectChoice::parse("high"),
            Some(EffectChoice::Tier(EffectTier::High))
        );
        assert_eq!(EffectChoice::parse("bogus"), None);
        // The auto doctrine.
        let auto = EffectChoice::Auto;
        assert_eq!(auto.resolve(true, 8_000_000, false), EffectTier::High);
        assert_eq!(auto.resolve(false, 1_000_000, false), EffectTier::Medium);
        assert_eq!(auto.resolve(false, 9_000_000, false), EffectTier::Low);
        assert_eq!(auto.resolve(false, 1_000_000, true), EffectTier::Minimal);
        // Explicit tiers win over the machine facts.
        assert_eq!(
            EffectChoice::Tier(EffectTier::High).resolve(false, 9_000_000, true),
            EffectTier::High
        );
    }
}
