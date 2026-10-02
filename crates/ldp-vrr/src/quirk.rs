//! The VRR quirk ledger (Phase 41): the operator's honest floor.
//!
//! The comparison row's named remainder — "the multi-year driver
//! quirk table" — is not one mechanism but a *ledger*: per-panel
//! overrides the giants' decades accrued behind their driver panels
//! (NVIDIA's per-monitor refresh-range lists, AMD's EDID quirk
//! tables, Windows' per-display VRR tuning). None of that knowledge
//! is reproducible in CI; what ships honestly is the **structure**
//! the decades fill, with the first rows tabled as mechanism-grade
//! operator escapes.
//!
//! This module is the ledger's first page: the **effective floor**
//! (`--vrr-floor N`). Real panels advertise a VRR range whose bottom
//! end flickers — the backlight pumps when the scanout clock
//! stretches toward the advertised minimum rate, because the EDID
//! claims a stretch the hardware cannot sustain (the same
//! advertising class as the HDR bloated peak: the *advertisement* is
//! the bloated side, the panel's own behavior the truth). The
//! operator who has watched their panel flicker raises the floor to
//! the rate the panel honestly sustains, and every consumer of the
//! window — the scheduler's deadline widening, the wire
//! advertisement, the LFC cadence — sees the clamped truth:
//!
//! * the deepest stretches never *schedule* (the widened window
//!   shrinks by exactly the flickering band),
//! * the client is told the honest range (the `output.vrr` event),
//! * content slower than the floor rides the LFC repeats instead of
//!   the flickering stretch (the floor's cost, paid back as the
//!   latch's stable cadence).
//!
//! The clamp is *scheduling-side only*: the kernel's own window (the
//! mock's `vrr_capable`) keeps its advertised truth — exactly where
//! the giants' overrides live, in the display stack above the
//! driver, never rewriting the hardware's claim.
//!
//! # Determinism doctrine
//!
//! Pure integer arithmetic over `(window, floor, nominal)`; the same
//! triple always yields the same answer. No clock reads.

#![forbid(unsafe_code)]

/// A floor the operator asked for that cannot be served.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FloorError {
    /// The floor rate sits above the mode's own refresh rate: the
    /// fixed grid the deadline scheduler paces against would fall
    /// outside the effective window — every deadline a lie.
    AboveNominal,
    /// The floor rate sits above the panel's own maximum rate: the
    /// effective window would invert (no legal interval at all).
    AboveRange,
}

impl FloorError {
    /// The operator-facing message (the boot failure's context).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            FloorError::AboveNominal => {
                "the floor exceeds the mode's refresh rate — the fixed \
                 grid would fall outside the effective window (choose a \
                 floor at or below the mode's rate)"
            }
            FloorError::AboveRange => {
                "the floor exceeds the panel's maximum refresh rate — the \
                 effective window would invert"
            }
        }
    }
}

impl std::fmt::Display for FloorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::error::Error for FloorError {}

/// What the floor pass did to one output's window (the audit trail —
/// the startup line and the session proofs print this shape).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FloorOutcome {
    /// No window on this output (or no floor asked): nothing to
    /// clamp — the advertisement stands.
    Passthrough,
    /// The floor sits at or below the advertised minimum rate: the
    /// advertised window already tells the truth the operator wants.
    BelowAdvertised,
    /// The window was clamped: the advertised longest stretch
    /// `from_max_ns` became the floor's `to_max_ns`.
    Clamped {
        /// The advertised maximum stretch (nanoseconds).
        from_max_ns: u64,
        /// The effective maximum stretch (nanoseconds).
        to_max_ns: u64,
    },
}

impl FloorOutcome {
    /// Whether this outcome changed the window.
    #[must_use]
    pub const fn clamped(&self) -> bool {
        matches!(self, FloorOutcome::Clamped { .. })
    }

    /// The one-line report fragment (the startup line's parenthetical).
    #[must_use]
    pub fn report(&self) -> String {
        match self {
            FloorOutcome::Passthrough => "passthrough".to_owned(),
            FloorOutcome::BelowAdvertised => "below advertised".to_owned(),
            FloorOutcome::Clamped {
                from_max_ns,
                to_max_ns,
            } => format!("{from_max_ns} ns -> {to_max_ns} ns"),
        }
    }
}

/// The effective-floor pass: clamp one output's raw window to the
/// operator's floor rate.
///
/// `raw` is the window the hardware advertises (`(min_ns, max_ns)`,
/// shortest and longest legal interval); `floor_hz` is the effective
/// minimum refresh *rate* the panel honestly sustains (`0`: no floor
/// — passthrough); `nominal_ns` is the mode's own period (the grid
/// the deadline scheduler paces against).
///
/// # Errors
/// [`FloorError::AboveNominal`] when the floor exceeds the mode's
/// rate (the fixed grid would leave the effective window);
/// [`FloorError::AboveRange`] when it exceeds the panel's own
/// maximum rate (the window would invert).
///
/// # Panics
/// Never; the arithmetic saturates.
pub fn apply_floor(
    raw: Option<(u64, u64)>,
    floor_hz: u32,
    nominal_ns: u64,
) -> Result<(Option<(u64, u64)>, FloorOutcome), FloorError> {
    let Some((min_ns, max_ns)) = raw else {
        return Ok((None, FloorOutcome::Passthrough));
    };
    if floor_hz == 0 {
        return Ok((raw, FloorOutcome::Passthrough));
    }
    // The floor's own period: the longest stretch the floor allows.
    // A floor of 57 Hz stretches no longer than 17,543,859 ns.
    let floor_ns = nanos_per_hz(floor_hz);
    // The honest order of checks: the mode's rate first (the operator
    // most likely asked past their own mode — the message names the
    // fixable mistake), then the panel's ceiling.
    if floor_ns < nominal_ns {
        return Err(FloorError::AboveNominal);
    }
    if floor_ns < min_ns {
        return Err(FloorError::AboveRange);
    }
    if floor_ns >= max_ns {
        // The floor's stretch is no tighter than the panel's own: the
        // advertisement already sustains it.
        return Ok((raw, FloorOutcome::BelowAdvertised));
    }
    // The clamp: the effective window keeps the panel's slip floor
    // (the high-rate end is the panel's own, not the operator's) and
    // takes the floor's period as its stretch ceiling.
    Ok((
        Some((min_ns, floor_ns)),
        FloorOutcome::Clamped {
            from_max_ns: max_ns,
            to_max_ns: floor_ns,
        },
    ))
}

/// The period of one hertz tick, nanoseconds (`1_000_000_000 / hz`,
/// floor). A floor of 0 maps to the saturating maximum (no clamp can
/// ever name it — `apply_floor` short-circuits first).
#[must_use]
pub fn nanos_per_hz(hz: u32) -> u64 {
    if hz == 0 {
        u64::MAX
    } else {
        1_000_000_000 / u64::from(hz)
    }
}

/// Parse `--vrr-floor`'s value: one or more comma-separated rates.
///
/// The grammar mirrors `--scale`'s (Phase 37): a lone rate is the
/// blanket doctrine (every output), a comma list names one rate per
/// output in output order (`57,0` — the eDP floored at 57 Hz, the
/// HDMI passthrough), extras reusing the last entry (the stretch
/// rule), `0` the per-output passthrough. The compositor treats a
/// one-element list as the blanket value.
///
/// # Errors
/// A usage message for empty segments, non-numeric values, negative
/// values, or rates past the honest ceiling (1000 Hz — nothing
/// serves a floor above it; the *mode-rate* check happens later, at
/// the clamp, where the mode is known).
pub fn parse_floor_list(value: &str) -> Result<Vec<u32>, String> {
    let mut floors = Vec::new();
    for segment in value.split(',') {
        let segment = segment.trim();
        if segment.is_empty() {
            return Err(format!("bad vrr-floor '{value}' (empty segment)"));
        }
        let hz: u32 = segment
            .parse()
            .map_err(|_| format!("bad vrr-floor rate '{segment}' (whole hertz)"))?;
        if hz > 1000 {
            return Err(format!(
                "bad vrr-floor rate '{segment}' (past the 1000 Hz ceiling)"
            ));
        }
        floors.push(hz);
    }
    if floors.is_empty() {
        return Err(format!("bad vrr-floor '{value}' (no rates)"));
    }
    Ok(floors)
}

#[cfg(test)]
mod tests {
    use super::*;

    // The mock panel's window: 48-144 Hz around a 60 Hz mode.
    const MIN: u64 = 1_000_000_000 / 144;
    const MAX: u64 = 1_000_000_000 / 48;
    const NOM: u64 = 16_666_666;
    const RAW: Option<(u64, u64)> = Some((MIN, MAX));

    #[test]
    fn no_window_or_no_floor_passes_through() {
        assert_eq!(
            apply_floor(None, 57, NOM),
            Ok((None, FloorOutcome::Passthrough))
        );
        assert_eq!(
            apply_floor(RAW, 0, NOM),
            Ok((RAW, FloorOutcome::Passthrough))
        );
    }

    #[test]
    fn a_floor_below_the_advertised_minimum_is_a_no_op() {
        // 40 Hz is below the advertised 48 Hz minimum: the panel
        // already claims a deeper stretch than the operator asks.
        let (window, outcome) = apply_floor(RAW, 40, NOM).unwrap();
        assert_eq!(window, RAW);
        assert_eq!(outcome, FloorOutcome::BelowAdvertised);
        assert!(!outcome.clamped());
        // Exactly the advertised minimum: still no clamp (inclusive).
        let (window, outcome) = apply_floor(RAW, 48, NOM).unwrap();
        assert_eq!(window, RAW);
        assert_eq!(outcome, FloorOutcome::BelowAdvertised);
    }

    #[test]
    fn a_floor_inside_the_window_clamps_the_stretch_ceiling() {
        // 57 Hz: the flickering band below 57 Hz leaves the window.
        let floor_ns = 1_000_000_000 / 57;
        let (window, outcome) = apply_floor(RAW, 57, NOM).unwrap();
        assert_eq!(window, Some((MIN, floor_ns)));
        assert_eq!(
            outcome,
            FloorOutcome::Clamped {
                from_max_ns: MAX,
                to_max_ns: floor_ns,
            }
        );
        assert!(outcome.clamped());
        assert_eq!(outcome.report(), format!("{MAX} ns -> {floor_ns} ns"));
        // The slip floor (the high-rate end) is the panel's own — the
        // operator never widens the panel's capability, only narrows
        // the stretch they do not trust.
        assert_eq!(window.unwrap().0, MIN);
    }

    #[test]
    fn a_floor_above_the_mode_rate_is_rejected_with_the_fixable_message() {
        // 61 Hz on a 60 Hz mode: the fixed grid itself would sit
        // outside the effective window.
        assert_eq!(apply_floor(RAW, 61, NOM), Err(FloorError::AboveNominal));
        // Exactly the mode's rate: legal (the ceiling meets the grid).
        assert!(apply_floor(RAW, 60, NOM).is_ok());
    }

    #[test]
    fn a_floor_above_the_panel_ceiling_is_rejected() {
        // 240 Hz on a 144 Hz-max panel: the window would invert. The
        // mode check fires first (240 > 60 too) — the range check
        // guards the structurally malformed window (a device whose
        // nominal sits outside its own window — the install site's
        // other defense is the scheduler's own bounds check).
        assert_eq!(apply_floor(RAW, 240, NOM), Err(FloorError::AboveNominal));
        // The pure range inversion: a malformed raw window whose
        // minimum period sits above the mode's own (min 20 ms,
        // nominal 16.67 ms) — a 60 Hz floor clears the nominal but
        // cannot clear the panel's own slip floor.
        let malformed = Some((20_000_000, 25_000_000));
        assert_eq!(apply_floor(malformed, 60, NOM), Err(FloorError::AboveRange));
    }

    #[test]
    fn the_error_messages_name_the_operators_mistake() {
        assert!(FloorError::AboveNominal.as_str().contains("mode's"));
        assert!(FloorError::AboveRange.as_str().contains("invert"));
        assert!(FloorError::AboveNominal.as_str().len() > 40);
    }

    #[test]
    fn nanos_per_hz_is_the_periods_arithmetic() {
        assert_eq!(nanos_per_hz(0), u64::MAX);
        assert_eq!(nanos_per_hz(1), 1_000_000_000);
        assert_eq!(nanos_per_hz(57), 17_543_859);
        assert_eq!(nanos_per_hz(144), 6_944_444);
    }

    #[test]
    fn the_floor_grammar_mirrors_the_scale_grammar() {
        // The blanket form.
        assert_eq!(parse_floor_list("57"), Ok(vec![57]));
        // The per-output form, 0 the passthrough.
        assert_eq!(parse_floor_list("57,0"), Ok(vec![57, 0]));
        assert_eq!(parse_floor_list("57, 0"), Ok(vec![57, 0]));
        // The stretch rule's extras reuse the last entry — the parser
        // hands the list through; the consumer applies the rule.
        assert_eq!(parse_floor_list("48,57,60"), Ok(vec![48, 57, 60]));
        // The failures: empty, non-numeric, negative, absurd.
        assert!(parse_floor_list("").is_err());
        assert!(parse_floor_list("57,,0").is_err());
        assert!(parse_floor_list("57,x").is_err());
        assert!(parse_floor_list("-1").is_err());
        assert!(parse_floor_list("1001").is_err());
        // The honest ceiling passes the parser (the mode check owns
        // the rejection where the mode is known).
        assert_eq!(parse_floor_list("1000"), Ok(vec![1000]));
    }

    #[test]
    fn the_outcome_report_shapes() {
        assert_eq!(FloorOutcome::Passthrough.report(), "passthrough");
        assert_eq!(FloorOutcome::BelowAdvertised.report(), "below advertised");
        assert_eq!(
            FloorOutcome::Clamped {
                from_max_ns: 20_833_333,
                to_max_ns: 17_543_859
            }
            .report(),
            "20833333 ns -> 17543859 ns"
        );
    }
}
