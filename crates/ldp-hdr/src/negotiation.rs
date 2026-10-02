//! The panel negotiation (Phase 38): the peak the system will actually
//! deliver, and the content bounds that reach the ink.
//!
//! The v0.10.0 HDR pipeline *advertised* a peak and *collected*
//! per-surface metadata, but the two never met: the PQ canvas was the
//! static 1000-nit description whatever the panel claimed, the
//! metadata fed only the mode controller's stack summary, and a panel
//! that advertises more peak than it sustains (the EDID bloated-peak
//! class) had no honesty knob. This module is the vocabulary that
//! closes those gaps:
//!
//! * [`PanelPeak`] — the panel's *effective* peak: the advertised
//!   luminance clamped by the operator's `--hdr-peak` cap. Real
//!   panels lie upward (the bloated-peak quirk class); the operator's
//!   cap is the honesty knob, and `output.hdr_caps` advertises the
//!   *effective* value — the negotiation's reply side, what clients
//!   can actually rely on.
//! * [`negotiated_ceiling`] — the canvas's ceiling for one frame:
//!   the stack's brightest content clamped to the panel (dimmer
//!   content negotiates to itself; brighter content maps down). This
//!   is the value the render pass tone-maps against — the comment the
//!   v0.10.1-era fold carried ("recorded for the render's tone-mapping
//!   ceiling") finally becomes true.
//! * [`layer_mastering`] — the per-surface refinement: the layer's
//!   static HDR10 mastering bounds when the client declared them
//!   (`set_hdr_metadata`), the description's own bounds otherwise.
//!   The CTA sentinels (`0` = unknown) keep the description's truth —
//!   never fabricated.
//!
//! Determinism doctrine as everywhere: pure functions over
//! `ldp-core` luminance types, no clocks, no randomness, every
//! clamp named. The DRM-side infoframe that carries the negotiated
//! ceiling to the physical panel is the existing HDR roadmap line;
//! when it lands, the bytes it forwards are the ones this module
//! computes.

use ldp_core::color::{ColorDescription, HdrMetadata, Luminance};

/// The PQ ceiling (nits) — `Luminance` values above this are
/// unrepresentable in the interchange space.
pub const PQ_CEILING_NITS: u32 = 10_000;

/// Negotiation construction failures.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NegotiationError {
    /// A peak luminance outside `(0, 10 000]` nits.
    BadPeak,
}

impl core::fmt::Display for NegotiationError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            NegotiationError::BadPeak => f.write_str("peak luminance outside (0, 10000] nits"),
        }
    }
}

impl std::error::Error for NegotiationError {}

/// The panel's effective peak: what the system will actually deliver.
///
/// `advertised` is what the panel claims (EDID, or the operator's
/// forced `--hdr` default); `operator` is the `--hdr-peak` cap — the
/// honesty knob for panels that advertise more than they sustain.
/// The effective peak is the *minimum*: a cap above the advertisement
/// is the operator guessing brighter than the panel's own claim, and
/// the claim wins.
///
/// The effective value is what `output.hdr_caps` advertises (the
/// negotiation's reply side) and what the render pass clamps against —
/// one truth, both directions.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PanelPeak {
    effective: Luminance,
}

impl PanelPeak {
    /// The effective peak from an advertisement and an optional
    /// operator cap (both in nits; the cap applies only when
    /// `Some`).
    ///
    /// # Errors
    /// [`NegotiationError::BadPeak`] when either input is outside
    /// `(0, 10 000]` nits — an advertisement of zero is a panel that
    /// claims nothing (the SDR doctrine, not a negotiation).
    pub fn new(
        advertised: Luminance,
        operator: Option<Luminance>,
    ) -> Result<PanelPeak, NegotiationError> {
        for candidate in Some(advertised).into_iter().chain(operator) {
            let nits = candidate.as_nits();
            if nits == 0 || nits > PQ_CEILING_NITS {
                return Err(NegotiationError::BadPeak);
            }
        }
        let effective = match operator {
            Some(cap) => advertised.min(cap),
            None => advertised,
        };
        Ok(PanelPeak { effective })
    }

    /// The effective peak (what the system delivers and advertises).
    #[must_use]
    pub const fn effective(&self) -> Luminance {
        self.effective
    }
}

/// The canvas's negotiated ceiling for one frame: the stack's
/// brightest content clamped to the panel's peak.
///
/// The doctrine (the v0.10.3 session's pinned table): content
/// brighter than the panel maps down (`panel`); dimmer content
/// negotiates to itself (its own max — the canvas carries content
/// at the content's level, wasting no code space above it); an
/// unknown content max (`0`, an all-description stack with no
/// metadata) keeps the panel's peak (the honest default).
#[must_use]
pub fn negotiated_ceiling(panel: Luminance, content_max: Luminance) -> Luminance {
    crate::adaptation::negotiate_peak(panel, content_max)
}

/// The per-surface mastering refinement: the layer's static HDR10
/// bounds when the client declared them, the description's own
/// otherwise. Returns `(min, max)` in nits.
///
/// The CTA sentinels (`0` = unknown) never fabricate: a metadata max
/// of zero keeps the description's bound, and a metadata *min* of
/// zero keeps the description's floor. When the metadata's max
/// exceeds the PQ ceiling the description's bound wins (the
/// dispatcher's validation gate rejects such metadata on the wire;
/// this is the belt under those braces).
#[must_use]
pub fn layer_mastering(
    desc: &ColorDescription,
    meta: Option<&HdrMetadata>,
) -> (Luminance, Luminance) {
    let (mut lo, mut hi) = (desc.luminance_min, desc.luminance_max);
    if let Some(meta) = meta {
        if meta.mastering_luminance_max.as_nits() != 0
            && meta.mastering_luminance_max.as_nits() <= PQ_CEILING_NITS
        {
            hi = hi.min(meta.mastering_luminance_max);
        }
        if meta.mastering_luminance_min.as_nits() != 0 {
            lo = lo.max(meta.mastering_luminance_min);
        }
    }
    (lo, hi)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nits(v: u32) -> Luminance {
        Luminance::from_nits(v)
    }

    #[test]
    fn the_operator_cap_clamps_the_advertisement() {
        // The bloated-peak class: EDID says 600, the operator knows
        // the panel sustains 450 — the effective peak is 450.
        let peak = PanelPeak::new(nits(600), Some(nits(450))).expect("valid");
        assert_eq!(peak.effective().as_nits(), 450);
        // A cap above the advertisement: the panel's own claim wins.
        let peak = PanelPeak::new(nits(600), Some(nits(1000))).expect("valid");
        assert_eq!(peak.effective().as_nits(), 600);
        // No cap: the advertisement stands.
        let peak = PanelPeak::new(nits(600), None).expect("valid");
        assert_eq!(peak.effective().as_nits(), 600);
    }

    #[test]
    fn zero_and_ceiling_peaks_are_refused() {
        assert!(PanelPeak::new(nits(0), None).is_err());
        assert!(PanelPeak::new(nits(10_001), None).is_err());
        // The cap is validated even when the advertisement is sane.
        assert!(PanelPeak::new(nits(600), Some(nits(0))).is_err());
        assert!(PanelPeak::new(nits(600), Some(nits(10_001))).is_err());
    }

    #[test]
    fn the_ceiling_negotiates_content_against_the_panel() {
        // Brighter content maps down to the panel.
        assert_eq!(negotiated_ceiling(nits(600), nits(4000)).as_nits(), 600);
        // Dimmer content negotiates to itself.
        assert_eq!(negotiated_ceiling(nits(600), nits(400)).as_nits(), 400);
        // Unknown content (0) keeps the panel.
        assert_eq!(negotiated_ceiling(nits(600), nits(0)).as_nits(), 600);
    }

    #[test]
    fn the_mastering_refinement_prefers_declared_metadata() {
        let desc = ColorDescription::pq_hdr();
        // No metadata: the description's own bounds.
        assert_eq!(
            layer_mastering(&desc, None),
            (desc.luminance_min, desc.luminance_max)
        );
        // Declared mastering bounds refine: the tighter max wins.
        let meta = HdrMetadata {
            mastering_luminance_min: nits(5),
            mastering_luminance_max: nits(4000),
            ..HdrMetadata::default()
        };
        let (lo, hi) = layer_mastering(&desc, Some(&meta));
        assert_eq!(lo.as_nits(), 5);
        assert_eq!(hi.as_nits(), 1000); // min(4000, desc's 1000)
                                        // The CTA sentinel (0 = unknown) keeps the description.
        let unknown = HdrMetadata {
            mastering_luminance_min: Luminance::from_nits(0),
            mastering_luminance_max: Luminance::from_nits(0),
            ..HdrMetadata::default()
        };
        assert_eq!(
            layer_mastering(&desc, Some(&unknown)),
            (desc.luminance_min, desc.luminance_max)
        );
        // Above the PQ ceiling: the description's bound wins (the
        // belt under the dispatcher gate's braces).
        let silly = HdrMetadata {
            mastering_luminance_max: nits(20_000),
            ..HdrMetadata::default()
        };
        let (_, hi) = layer_mastering(&desc, Some(&silly));
        assert_eq!(hi.as_nits(), 1000);
    }
}
