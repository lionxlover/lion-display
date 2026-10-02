//! The scanout plan: what rides hardware, what composites, and why.

use ldp_display::ids::{FbId, PlaneId};

/// Which slot an assigned layer occupies.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PlaneRole {
    /// The primary plane — carrying either the composited canvas (a
    /// split plan) or the bottom layer's own buffer (a zero-composite
    /// plan).
    Primary,
    /// An overlay plane stacked above the primary.
    Overlay,
}

/// One layer-to-plane assignment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaneAssignment {
    /// The layer's index in the stack (bottom-to-front order).
    pub layer: usize,
    /// The plane that carries it.
    pub plane: PlaneId,
    /// The slot the plane occupies.
    pub role: PlaneRole,
    /// The kernel framebuffer to scan out.
    pub fb: FbId,
    /// Source crop, 16.16-free pixel rect `(x, y, w, h)` — the full
    /// buffer in v1 (no plane cropping yet).
    pub src: (u32, u32, u32, u32),
    /// Destination rect on the CRTC `(x, y, w, h)`.
    pub dst: (i32, i32, u32, u32),
    /// The zpos the commit must program for stacking order.
    pub zpos: u64,
}

/// Why one layer composites instead of riding a plane — the honest
/// ledger. Every composite layer in a [`ScanoutPlan`] carries exactly
/// one of these; the plan's report is the aggregate the operator
/// reads.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum Demotion {
    /// The layer's buffer has no registered kernel framebuffer — a
    /// CPU window, not GEM memory (the import walk never ran or
    /// failed; the composite path is the only correct rendering).
    NoFb,
    /// The placement is scaled — planes do not scale in v1.
    Scaled,
    /// The layer is rotated or mirrored — plane rotation caps are a
    /// named future line.
    Transformed,
    /// The layer's color description differs from the output's —
    /// v1 has no per-plane color management.
    ColorMismatch,
    /// The layer's destination is not fully inside the output.
    OutOfBounds,
    /// Surface opacity below 1 — there is no standard plane-alpha
    /// property to reproduce it on hardware.
    Translucent,
    /// The layer's own pixels sample the composed backdrop (frost):
    /// its truth is the composite, by definition.
    NeedsBackdrop,
    /// The layer carries render-path styling (rounded corners, a soft
    /// shadow): the pixels a plane would scan out do not exist in the
    /// buffer — the styled truth composites.
    Styled,
    /// The (format, modifier) pair scans out on no available plane.
    FormatUnsupported,
    /// No plane slot remained (the overlay budget ran out — every
    /// plane above the split is taken).
    NoSlot,
    /// The layer sits below the split point: something above it (or
    /// itself) composites, and layers under a composite layer must
    /// render into the canvas (there is no GL surface between
    /// hardware planes).
    BelowSplit,
    /// The bottom layer of an otherwise all-plane stack did not cover
    /// the output opaquely, so the canvas composites beneath it.
    BottomNotCovering,
}

impl Demotion {
    /// The one-line report phrase.
    #[must_use]
    pub const fn phrase(self) -> &'static str {
        match self {
            Self::NoFb => "cpu-window buffer (no kernel fb)",
            Self::Scaled => "scaled placement (planes do not scale in v1)",
            Self::Transformed => "transformed placement (plane rotation is a future line)",
            Self::ColorMismatch => "color description differs from the output",
            Self::OutOfBounds => "destination extends outside the output",
            Self::Translucent => "surface opacity below 1 (no plane alpha in v1)",
            Self::NeedsBackdrop => "material samples the backdrop (frost composites)",
            Self::Styled => "render-path styling (corners/shadows composite)",
            Self::FormatUnsupported => "format+modifier unsupported on every plane",
            Self::NoSlot => "no plane slot remained",
            Self::BelowSplit => "under the composite split (canvas renders it)",
            Self::BottomNotCovering => "bottom layer does not cover the output opaquely",
        }
    }
}

/// The solver's verdict for one frame of one output.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScanoutPlan {
    /// Assignments, in stack order (bottom layer first).
    pub assignments: Vec<PlaneAssignment>,
    /// How many stack layers composite into the canvas.
    pub composite_count: usize,
    /// Demotions, in stack order, one per composite layer.
    pub demotions: Vec<(usize, Demotion)>,
    /// Whether the compositor renders **zero** passes: every layer
    /// rides a plane and the primary carries the bottom layer's own
    /// buffer. The renderer's framebuffer is not touched at all.
    pub zero_composite: bool,
    /// Whether the primary plane carries the composited canvas (a
    /// split plan) rather than a client buffer.
    pub canvas_on_primary: bool,
}

impl ScanoutPlan {
    /// The assignment carrying `layer`, if any.
    #[must_use]
    pub fn assignment_of(&self, layer: usize) -> Option<&PlaneAssignment> {
        self.assignments.iter().find(|a| a.layer == layer)
    }

    /// The zpos values the commit programs, ascending (the stacking
    /// invariant's proof).
    #[must_use]
    pub fn zpos_ladder(&self) -> Vec<u64> {
        let mut zs: Vec<u64> = self.assignments.iter().map(|a| a.zpos).collect();
        zs.sort_unstable();
        zs
    }

    /// The one-line operator report: what rides hardware, what
    /// composites, and the first demotion phrase when anything
    /// composites.
    #[must_use]
    pub fn report(&self) -> String {
        if self.zero_composite {
            return format!(
                "planes: {} assigned, 0 composite passes (zero-composite frame)",
                self.assignments.len()
            );
        }
        let first = self
            .demotions
            .first()
            .map_or_else(String::new, |(_, d)| format!(" — first: {}", d.phrase()));
        format!(
            "planes: {} assigned, {} composite{}",
            self.assignments.len(),
            self.composite_count,
            first
        )
    }
}
