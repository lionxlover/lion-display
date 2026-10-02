//! The solver's inputs: what the compositor knows about each layer
//! and the output it would scan out onto.

use ldp_core::buffer::{FourCC, Modifier};
use ldp_core::color::ColorDescription;
use ldp_core::geometry::{Rect, Region, Transform};
use ldp_display::ids::FbId;

/// One layer of the stack, as the plane engine sees it.
///
/// Pure data — no buffer bytes, no borrows: the compositor extracts
/// these facts per frame (cheap — a handful of copies and one region
/// clone), the solver decides on them, and the render path keeps its
/// own richer surface-layer model untouched. The one
/// heavyweight fact is [`LayerFacts::fb`]: whether the layer's buffer
/// has a **registered kernel framebuffer** behind it (the DMA-BUF
/// import walk's product). A CPU-window buffer with no GEM backing
/// cannot scan out — `fb = None` is the honest statement of that, and
/// the solver demotes the layer with `Demotion::NoFb` rather than
/// pretending.
#[derive(Clone, Debug)]
pub struct LayerFacts {
    /// Output-local placement (scale already folded in by the caller,
    /// desktop-origin already subtracted).
    pub dest: Rect,
    /// The buffer's width in pixels.
    pub width: u32,
    /// The buffer's height in pixels.
    pub height: u32,
    /// The buffer's pixel format.
    pub format: FourCC,
    /// The buffer's layout modifier.
    pub modifier: Modifier,
    /// Buffer-to-surface orientation.
    pub transform: Transform,
    /// The buffer's color description.
    pub color: ColorDescription,
    /// Surface-level opacity in `0..=1`.
    pub opacity: f32,
    /// The layer's opaque region in *output* coordinates (the caller
    /// translates surface-space coverage through the placement).
    pub opaque: Region,
    /// The registered kernel framebuffer for this buffer, when the
    /// import walk succeeded — the zero-copy truth.
    pub fb: Option<FbId>,
    /// Whether the layer's own pixels need the composed backdrop
    /// beneath them (a frosted material samples what is under it).
    /// Such a layer can never ride a plane: its truth *is* the
    /// composite.
    pub needs_backdrop: bool,
    /// Whether the compositor dresses this layer with styling whose
    /// pixels exist only in the render path (rounded-corner coverage,
    /// soft shadows). A styled layer composites: hardware planes show
    /// the buffer verbatim — a rounded window would scan out square.
    pub styled: bool,
    /// Whether this is system chrome (the dock) rather than client
    /// content — recorded for the plan's report, not a rule: chrome
    /// follows the same eligibility as everyone else (the honest
    /// subtraction happens in the facts, not the policy).
    pub system: bool,
}

impl LayerFacts {
    /// Whether the placement is 1:1 (dest extent equals the
    /// transform-swapped buffer extent) — planes do not scale in v1.
    #[must_use]
    pub fn is_one_to_one(&self) -> bool {
        let swapped = self
            .transform
            .transform_size(ldp_core::geometry::Size::new(self.width, self.height));
        self.dest.w == swapped.w && self.dest.h == swapped.h
    }

    /// Whether the layer's destination exactly covers `output`.
    #[must_use]
    pub fn covers_output(&self, output: &OutputFacts) -> bool {
        self.dest == Rect::new(0, 0, output.width, output.height)
    }

    /// Whether the layer's opaque region covers its whole destination
    /// (no holes).
    #[must_use]
    pub fn opaque_covers_dest(&self) -> bool {
        let covered = self.opaque.clipped_to(self.dest);
        let holes = Region::from_rect(self.dest).subtract(&covered);
        holes.is_empty()
    }
}

/// The output the stack would scan out onto.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputFacts {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// The scanout framebuffer's format.
    pub format: FourCC,
    /// The scanout pipeline's color description.
    pub color: ColorDescription,
}

impl OutputFacts {
    /// The output's bounds as a rect rooted at the origin.
    #[must_use]
    pub fn rect(&self) -> Rect {
        Rect::new(0, 0, self.width, self.height)
    }
}
