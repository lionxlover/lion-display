//! DMA-BUF allocation feedback — the tranche model.
//!
//! The server side of "allocate like this": given what the GL device
//! can import (the `eglQueryDmaBufFormatsEXT` surface on real
//! hardware) and what the scanout planes can carry (the `IN_FORMATS`
//! walk), the feedback resolves into **ordered tranches** of
//! `(format, modifier)` pairs — the allocation advice a client
//! receives so its buffers are zero-copy end to end.
//!
//! * **Tranche 0 — scanout-preferred**: pairs the GL device imports
//!   *and* a plane scans out. A buffer allocated here never touches
//!   the CPU: it composites on hardware when it can and uploads as an
//!   EGLImage when it cannot. This is the tranche that makes
//!   direct scanout the *default* instead of a lucky accident.
//! * **Tranche 1 — GL-only**: pairs the GL device imports but no
//!   plane takes. Buffers here composite through the zero-copy
//!   EGLImage arm (still no CPU payload, but never plane offload).
//!
//! Ordering inside a tranche preserves the plane capability order
//! (the device's own preference vocabulary — the ordering the driver
//! published), with one deliberate promotion: a pair both sides take
//! in `LINEAR` sorts last within tranche 0 when compressed/tiling
//! alternatives exist, because a compressible allocation is the one
//! worth taking when the hardware can (the linear fallback stays
//! available, never preferred). An empty tranche 0 is honest: it says
//! no scanout-capable allocation exists on this machine, and the
//! caller reports that instead of inventing advice.
//!
//! Phase 33 ships the *policy*; the protocol surface that carries
//! tranches to clients (the dma-buf negotiation interface) is a named
//! roadmap line — this module is the decision engine it will serve.

#![forbid(unsafe_code)]

use ldp_core::buffer::{FourCC, Modifier};

/// One ordered tranche of allocation advice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeedbackTranche {
    /// The tranche's `(format, modifier)` pairs, in preference order.
    pub pairs: Vec<(FourCC, Modifier)>,
    /// Whether every pair in this tranche scans out on a plane (the
    /// zero-copy end-to-end promise). Tranche 0 sets this; the GL-only
    /// tranche does not.
    pub scanout: bool,
}

/// The resolved feedback: ordered tranches, best first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DmaBufFeedback {
    /// The tranches in preference order (scanout-preferred first).
    pub tranches: Vec<FeedbackTranche>,
}

impl DmaBufFeedback {
    /// Resolve the tranches from the two capability surfaces.
    ///
    /// `egl_formats` is what the GL device imports (fourcc list; the
    /// modifier negotiation is the `_modifiers` extension's, expressed
    /// here as "every egl format takes LINEAR, plus the pairs the
    /// planes prove"), `plane_pairs` the scanout planes'
    /// `(format, modifier)` walk. Deterministic: the output is a pure
    /// function of the inputs, ordered as the doc above promises.
    #[must_use]
    pub fn resolve(egl_formats: &[FourCC], plane_pairs: &[(FourCC, Modifier)]) -> Self {
        // Tranche 0: plane pairs whose format the GL side imports —
        // both surfaces take them.
        let mut scanout_pairs: Vec<(FourCC, Modifier)> = plane_pairs
            .iter()
            .copied()
            .filter(|(f, _)| egl_formats.contains(f))
            .collect();
        // The linear demotion: linear pairs sort after their
        // compressed/tiling alternatives within the tranche (stable —
        // the driver's order is otherwise preserved).
        scanout_pairs.sort_by_key(|(_, m)| u64::from(u32::from(m.is_linear())));
        // Tranche 1: GL-importable formats with LINEAR that no plane
        // pair proves (the upload-only allocations), in the EGL
        // surface's own order.
        let gl_only: Vec<(FourCC, Modifier)> = egl_formats
            .iter()
            .copied()
            .filter(|f| !plane_pairs.iter().any(|(pf, _)| pf == f))
            .map(|f| (f, Modifier::LINEAR))
            .collect();
        let mut tranches = Vec::new();
        if !scanout_pairs.is_empty() {
            tranches.push(FeedbackTranche {
                pairs: scanout_pairs,
                scanout: true,
            });
        }
        if !gl_only.is_empty() {
            tranches.push(FeedbackTranche {
                pairs: gl_only,
                scanout: false,
            });
        }
        Self { tranches }
    }

    /// The best pair to allocate (tranche 0's head), when one exists.
    #[must_use]
    pub fn preferred(&self) -> Option<(FourCC, Modifier)> {
        self.tranches.first().and_then(|t| t.pairs.first()).copied()
    }

    /// Whether every composite can stay zero-copy end to end (a
    /// non-empty scanout tranche exists).
    #[must_use]
    pub fn any_scanout(&self) -> bool {
        self.tranches.iter().any(|t| t.scanout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tranche_zero_is_the_intersection() {
        let egl = [FourCC::XRGB8888, FourCC::ARGB8888, FourCC::NV12];
        let planes = [
            (FourCC::XRGB8888, Modifier::LINEAR),
            (FourCC::XRGB8888, Modifier::INTEL_X),
            (FourCC::NV12, Modifier::LINEAR),
            (FourCC::RGB565, Modifier::LINEAR), // not EGL-importable
        ];
        let fb = DmaBufFeedback::resolve(&egl, &planes);
        assert!(fb.any_scanout());
        // XRGB + INTEL_X (compressed) precedes XRGB + LINEAR; NV12
        // rides too; RGB565 is excluded (the GL side refuses it).
        assert_eq!(
            fb.tranches[0].pairs,
            vec![
                (FourCC::XRGB8888, Modifier::INTEL_X),
                (FourCC::XRGB8888, Modifier::LINEAR),
                (FourCC::NV12, Modifier::LINEAR),
            ]
        );
        assert!(fb.tranches[0].scanout);
        // No GL-only formats remain (everything EGL takes, a plane
        // also takes — ARGB rides tranche 0? No: ARGB is
        // EGL-importable and no plane pair proves it → tranche 1.
        assert_eq!(
            fb.tranches[1].pairs,
            vec![(FourCC::ARGB8888, Modifier::LINEAR)]
        );
        assert!(!fb.tranches[1].scanout);
    }

    #[test]
    fn an_empty_intersection_is_honest() {
        // The GL device imports only RGB; the planes scan out only
        // YUV — no zero-copy end-to-end allocation exists.
        let fb = DmaBufFeedback::resolve(&[FourCC::ARGB8888], &[(FourCC::NV12, Modifier::LINEAR)]);
        assert!(!fb.any_scanout());
        assert!(fb.tranches.iter().all(|t| !t.scanout));
        assert_eq!(fb.preferred(), Some((FourCC::ARGB8888, Modifier::LINEAR)));
    }

    #[test]
    fn no_capabilities_no_tranches() {
        let fb = DmaBufFeedback::resolve(&[], &[]);
        assert!(fb.tranches.is_empty());
        assert_eq!(fb.preferred(), None);
        assert!(!fb.any_scanout());
    }

    #[test]
    fn resolution_is_deterministic() {
        let egl = [FourCC::XRGB8888, FourCC::NV12];
        let planes = [
            (FourCC::NV12, Modifier::LINEAR),
            (FourCC::XRGB8888, Modifier::LINEAR),
            (FourCC::XRGB8888, Modifier::INTEL_X),
        ];
        assert_eq!(
            DmaBufFeedback::resolve(&egl, &planes),
            DmaBufFeedback::resolve(&egl, &planes)
        );
    }
}
