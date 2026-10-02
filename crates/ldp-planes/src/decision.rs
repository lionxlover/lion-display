//! The composition decision engine (Phase 47): the frame's path,
//! named and auditable.
//!
//! The split-point solver has always *decided* — the plan's
//! assignments, demotions, and zero-composite flags encode the
//! verdict. What the decision engine adds is the **first-class
//! vocabulary**: one [`CompositionPath`] per frame, one
//! [`DecisionRecord`] carrying the verdict's shape and its first
//! named reason, so the operator, the tools, and the tests all read
//! the same words the frame loop acted on. The user's ladder —
//! direct scanout, hardware overlay, partial composition, full
//! composition — is exactly the four paths the solver's shapes
//! already produce; this module is the doctrine made total.
//!
//! Classification (over a solved [`ScanoutPlan`]):
//!
//! * **DirectScanout** — a zero-composite frame whose bottom layer
//!   rides the *primary*: the client's own buffer is the panel's
//!   scanout, the renderer emitted no pass. The classic fullscreen
//!   game frame.
//! * **HardwareOverlay** — a zero-composite frame whose bottom layer
//!   rides an *overlay* (the primary refuses its format): every
//!   layer rides hardware above a covered canvas. The classic
//!   fullscreen-video shape on older primaries.
//! * **SplitComposition** — a partial-composite frame: some layers
//!   render into the canvas on the primary, the eligible suffix
//!   rides overlays above it. The underlay shape Windows MPO serves.
//! * **FullComposition** — nothing rides hardware; the renderer's
//!   canvas is the whole frame (the empty stack's black canvas
//!   included — the honest phrase for a frame of nothing).
//!
//! The record's `first_demotion` is the demotion ledger's first
//! entry — the named, honest reason the frame is not cheaper. The
//! [`DecisionRecord::report`] line is the one the operator reads;
//! the plan's own `report` stays the assignment-level detail.

use crate::plan::{Demotion, PlaneRole, ScanoutPlan};

/// One frame's composition path — the decision engine's verdict.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum CompositionPath {
    /// Zero composite passes; the bottom layer's own buffer is the
    /// primary scanout (the client's pixels, untouched).
    DirectScanout,
    /// Zero composite passes; every layer rides overlays (the
    /// fullscreen-video shape on a format-refusing primary).
    HardwareOverlay,
    /// A partial-composite frame: the canvas carries the prefix on
    /// the primary, overlays carry the suffix above it.
    SplitComposition,
    /// Nothing rides hardware; the renderer's canvas is the frame.
    FullComposition,
}

impl CompositionPath {
    /// The one-line doctrine phrase.
    #[must_use]
    pub const fn phrase(self) -> &'static str {
        match self {
            Self::DirectScanout => "direct scanout (the client's buffer is the panel's)",
            Self::HardwareOverlay => "hardware overlay (every layer rides planes)",
            Self::SplitComposition => "split composition (canvas prefix, overlay suffix)",
            Self::FullComposition => "full composition (the canvas is the frame)",
        }
    }
}

/// The decision engine's auditable verdict for one frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecisionRecord {
    /// The path the frame took.
    pub path: CompositionPath,
    /// How many layers ride hardware planes.
    pub assigned: usize,
    /// How many layers composite into the canvas.
    pub composited: usize,
    /// The first named *cause* when anything composites — the
    /// demotion ledger's first entry that is not a `BelowSplit`
    /// (a BelowSplit layer composites only as a *consequence* of
    /// something above it compositing; the honest cause is that
    /// something). The ledger head itself when every entry is a
    /// consequence.
    pub first_demotion: Option<Demotion>,
}

impl DecisionRecord {
    /// The one-line operator report: the path, the shape, and the
    /// first reason when the path is not the zero-composite ideal.
    #[must_use]
    pub fn report(&self) -> String {
        let first = self
            .first_demotion
            .map_or_else(String::new, |d| format!(" — first: {}", d.phrase()));
        format!(
            "path: {} — {} assigned, {} composite{}",
            self.path.phrase(),
            self.assigned,
            self.composited,
            first
        )
    }
}

impl ScanoutPlan {
    /// The frame's composition path (the decision engine's
    /// classification — the module docs above name the four
    /// shapes).
    #[must_use]
    pub fn path(&self) -> CompositionPath {
        if self.zero_composite {
            // The bottom layer's seat names the zero shape: the
            // primary's own scanout (direct) or the lowest capable
            // overlay (the video shape).
            if self
                .assignments
                .first()
                .is_some_and(|a| a.role == PlaneRole::Primary)
            {
                CompositionPath::DirectScanout
            } else {
                CompositionPath::HardwareOverlay
            }
        } else if self.assignments.is_empty() {
            CompositionPath::FullComposition
        } else {
            CompositionPath::SplitComposition
        }
    }

    /// The decision engine's full verdict for the frame: the path,
    /// the shape, and the ledger's first named cause.
    #[must_use]
    pub fn decision(&self) -> DecisionRecord {
        DecisionRecord {
            path: self.path(),
            assigned: self.assignments.len(),
            composited: self.composite_count,
            first_demotion: self
                .demotions
                .iter()
                .find(|(_, d)| !matches!(d, Demotion::BelowSplit))
                .or_else(|| self.demotions.first())
                .map(|(_, d)| *d),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::{LayerFacts, OutputFacts};
    use crate::inventory::PlaneCaps;
    use crate::solver::Assigner;
    use crate::PlaneInventory;
    use ldp_core::buffer::{FourCC, Modifier};
    use ldp_core::color::ColorDescription;
    use ldp_core::geometry::{Rect, Region, Transform};
    use ldp_display::ids::{CrtcMask, FbId, PlaneId};
    use ldp_display::plane::{PlaneInfo, PlaneType};

    /// The reference output: 1920x1080, XRGB, sRGB.
    fn output() -> OutputFacts {
        OutputFacts {
            width: 1920,
            height: 1080,
            format: FourCC::XRGB8888,
            color: ColorDescription::srgb_sdr(),
        }
    }

    fn fb(raw: u32) -> Option<FbId> {
        FbId::new(raw)
    }

    /// One fully-capable client layer covering the output opaquely
    /// (the fullscreen candidate).
    fn covering_layer() -> LayerFacts {
        LayerFacts {
            dest: Rect::new(0, 0, 1920, 1080),
            width: 1920,
            height: 1080,
            format: FourCC::XRGB8888,
            modifier: Modifier::LINEAR,
            transform: Transform::Normal,
            color: ColorDescription::srgb_sdr(),
            opacity: 1.0,
            opaque: Region::from_rect(Rect::new(0, 0, 1920, 1080)),
            fb: fb(1),
            needs_backdrop: false,
            styled: false,
            system: false,
        }
    }

    /// A XRGB video layer in a format the synthetic primary refuses
    /// (NV12): the fullscreen-video shape rides the lowest capable
    /// overlay.
    fn video_layer() -> LayerFacts {
        LayerFacts {
            dest: Rect::new(0, 0, 1920, 1080),
            width: 1920,
            height: 1080,
            format: FourCC::NV12,
            modifier: Modifier::LINEAR,
            transform: Transform::Normal,
            color: ColorDescription::srgb_sdr(),
            opacity: 1.0,
            opaque: Region::from_rect(Rect::new(0, 0, 1920, 1080)),
            fb: fb(2),
            needs_backdrop: false,
            styled: false,
            system: false,
        }
    }

    /// A plain window layer (in-bounds, 1:1, but not covering).
    fn window_layer(raw: u32, y: i32) -> LayerFacts {
        LayerFacts {
            dest: Rect::new(0, y, 800, 600),
            width: 800,
            height: 600,
            format: FourCC::XRGB8888,
            modifier: Modifier::LINEAR,
            transform: Transform::Normal,
            color: ColorDescription::srgb_sdr(),
            opacity: 1.0,
            opaque: Region::from_rect(Rect::new(0, y, 800, 600)),
            fb: fb(raw),
            needs_backdrop: false,
            styled: false,
            system: false,
        }
    }

    /// A styled layer (the render path dresses it — corners, a
    /// shadow): the pixels a plane would scan out do not exist.
    fn styled_layer() -> LayerFacts {
        let mut l = window_layer(9, 0);
        l.styled = true;
        l
    }

    /// One plane's info with a legacy format list (linear layouts
    /// pass, the kernel's own fallback contract).
    fn plane_info(id: u32, kind: PlaneType, formats: Vec<FourCC>) -> PlaneInfo {
        PlaneInfo {
            id: PlaneId::new(id).expect("plane id"),
            kind,
            possible_crtcs: CrtcMask::covering(2),
            current_crtc: None,
            formats,
            in_formats: None,
        }
    }

    /// The synthetic video-driver shape: a primary that refuses
    /// 4:2:0 (the older-primary quirk) above two overlays that take
    /// it — the exact capability split the fullscreen-video shape
    /// exists for.
    fn inventory() -> PlaneInventory {
        PlaneInventory {
            crtc_index: 0,
            primary: Some(PlaneCaps {
                info: plane_info(10, PlaneType::Primary, vec![FourCC::XRGB8888]),
                zpos: 0,
            }),
            overlays: vec![
                PlaneCaps {
                    info: plane_info(11, PlaneType::Overlay, vec![FourCC::XRGB8888, FourCC::NV12]),
                    zpos: 1,
                },
                PlaneCaps {
                    info: plane_info(12, PlaneType::Overlay, vec![FourCC::XRGB8888, FourCC::NV12]),
                    zpos: 2,
                },
            ],
            cursor: None,
            below_primary: Vec::new(),
        }
    }

    fn solve(stack: &[LayerFacts]) -> ScanoutPlan {
        Assigner.solve(stack, &inventory(), &output())
    }

    #[test]
    fn the_classic_fullscreen_game_frame_direct_scans_out() {
        let plan = solve(&[covering_layer()]);
        let decision = plan.decision();
        assert_eq!(decision.path, CompositionPath::DirectScanout);
        assert_eq!(decision.assigned, 1);
        assert_eq!(decision.composited, 0);
        assert!(decision.first_demotion.is_none());
        assert_eq!(
            decision.report(),
            "path: direct scanout (the client's buffer is the panel's) — 1 assigned, 0 composite"
        );
    }

    #[test]
    fn the_video_shape_rides_overlays_when_the_primary_refuses() {
        // Primary takes XRGB only; the NV12 bottom covers opaquely →
        // the zero-on-overlay shape.
        let plan = solve(&[video_layer()]);
        let decision = plan.decision();
        assert_eq!(decision.path, CompositionPath::HardwareOverlay);
        assert_eq!(decision.composited, 0);
        // The whole stack rides: zero composite passes.
        assert!(plan.zero_composite);
    }

    #[test]
    fn a_styled_bottom_with_a_plain_window_above_splits() {
        // The split doctrine's shape: the composite set is a *prefix*,
        // the offloaded set a *suffix*. A styled layer at the bottom
        // (its pixels exist only in the render path) composites into
        // the canvas on the primary; the plain window above it rides
        // an overlay — Windows MPO's underlay shape.
        let stack = vec![styled_layer(), window_layer(3, 0)];
        let plan = solve(&stack);
        let decision = plan.decision();
        assert_eq!(decision.path, CompositionPath::SplitComposition);
        assert_eq!(decision.assigned, 1);
        assert_eq!(decision.composited, 1);
        assert_eq!(
            decision.first_demotion,
            Some(Demotion::Styled),
            "the styled layer is the reason the frame composites"
        );
        assert!(decision.report().contains("first: render-path styling"));
    }

    #[test]
    fn a_styled_solo_window_composites_fully() {
        let plan = solve(&[styled_layer()]);
        let decision = plan.decision();
        assert_eq!(decision.path, CompositionPath::FullComposition);
        assert_eq!(decision.assigned, 0);
        assert_eq!(decision.composited, 1);
        assert_eq!(decision.first_demotion, Some(Demotion::Styled));
    }

    #[test]
    fn an_empty_stack_is_the_full_composition_of_nothing() {
        let plan = solve(&[]);
        assert_eq!(plan.path(), CompositionPath::FullComposition);
        assert_eq!(plan.decision().composited, 0);
    }

    #[test]
    fn the_overlay_video_under_a_plain_window_stays_zero() {
        // Fullscreen NV12 video with an XRGB window above: the video
        // takes the lowest overlay, the window rides above it —
        // still zero composite passes (HardwareOverlay, both ride).
        let stack = vec![video_layer(), window_layer(3, 0)];
        let plan = solve(&stack);
        let decision = plan.decision();
        assert_eq!(decision.path, CompositionPath::HardwareOverlay);
        assert_eq!(decision.assigned, 2);
        assert_eq!(decision.composited, 0);
    }

    #[test]
    fn the_translucent_demotion_names_the_cause_honestly() {
        // A translucent layer above a covering bottom: there is no
        // GL surface between hardware planes, so everything
        // composites (the bottom's BelowSplit is a *consequence* —
        // the record's cause is the opacity).
        let mut translucent = window_layer(4, 0);
        translucent.opacity = 0.5;
        let stack = vec![covering_layer(), translucent.clone()];
        let plan = solve(&stack);
        let decision = plan.decision();
        assert_eq!(decision.path, CompositionPath::FullComposition);
        assert_eq!(
            decision.first_demotion,
            Some(Demotion::Translucent),
            "the ledger's first cause is the opacity, not the consequence"
        );
        // And the split arm of the same doctrine: a translucent layer
        // at the *bottom* with a plain window above — the window
        // rides, the translucent layer composites.
        let stack = vec![translucent, window_layer(5, 0)];
        let plan = solve(&stack);
        let decision = plan.decision();
        assert_eq!(decision.path, CompositionPath::SplitComposition);
        assert_eq!(decision.first_demotion, Some(Demotion::Translucent));
    }
}
