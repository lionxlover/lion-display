//! The display quirk table: named hardware quirks, their symptoms,
//! and the operator's escape (Phase 37).
//!
//! The display ecosystem's long tail is *quirks* — behaviors real
//! panels and display engines exhibit that the clean KMS model does
//! not name: a panel whose self-refresh visibly flickers, a VRR panel
//! whose brightness pumps while the scanout clock stretches. The
//! giants carry decades of per-vendor quirk tables behind their
//! drivers; most of that knowledge is NDA-flavored and none of it is
//! reproducible in CI. This crate's answer is the *mechanism*, shipped
//! honestly with the rows we can actually claim: every quirk the
//! table names has a **symptom** (what the operator sees), a
//! **detection** hint (how to confirm it is this quirk and not a
//! bug), and an **escape** (a CLI flag that routes the pipeline
//! around the offending feature). The table started one row deep
//! where the giants' are decades deep — Phase 38 accrued the HDR
//! bloated-peak class, Phase 41 (the quirk ledger) accrued the VRR
//! pair (the floor flicker and the sibling flicker), and Phase 42
//! (the mode foundry) accrues the display-size trio: the rotten
//! EDID, the preferred-timing lie, and the pixel-clock ceiling —
//! the timing-depth rows the "every display size" field's own
//! named remainder priced.
//!
//! The doctrine is *named, not guessed*: a quirk is only tabled when
//! the system can route around it deterministically. `psr-flicker`
//! is the first row (a real quirk of the eDP ecosystem — the reason
//! `--no-psr` exists); `vrr-flicker` is the second (the FreeSync
//! flicker class — the reason adaptive sync stays opt-in here). A
//! quirk with no escape is a *bug report*, not a table row.

#![forbid(unsafe_code)]

/// The class of hardware a quirk belongs to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum QuirkClass {
    /// The panel itself (eDP internals, backlight, self-refresh GRAM).
    Panel,
    /// The scanout clock (adaptive-sync stretching, fixed-sync tiles).
    Sync,
    /// The panel's advertised capabilities (the HDR bloated-peak
    /// class — the EDID claims more than the hardware sustains).
    Advertising,
    /// The sink's declared timing truth (the EDID's own bytes —
    /// rotten blocks, stale preferred timings, ceilings the
    /// enumerated list exceeds; Phase 42's foundry rows).
    Timing,
}

impl QuirkClass {
    /// The lowercase class name (the table's report column).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            QuirkClass::Panel => "panel",
            QuirkClass::Sync => "sync",
            QuirkClass::Advertising => "advertising",
            QuirkClass::Timing => "timing",
        }
    }
}

impl std::fmt::Display for QuirkClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One tabled quirk: the symptom, the detection, the escape.
#[derive(Clone, Copy, Debug)]
pub struct Quirk {
    /// The quirk's stable name (`psr-flicker`).
    pub name: &'static str,
    /// The hardware class.
    pub class: QuirkClass,
    /// What the operator sees (the symptom, in their words).
    pub symptom: &'static str,
    /// How to confirm this quirk is the cause (not a bug).
    pub detection: &'static str,
    /// The pipeline's route around it (the operator's CLI escape).
    pub escape: &'static str,
    /// What the escape costs (the honest trade).
    pub cost: &'static str,
}

impl Quirk {
    /// The one-line table row (the admin guide and the selftest print
    /// this shape): name, class, escape.
    #[must_use]
    pub fn row(&self) -> String {
        format!("{} ({}) — escape: {}", self.name, self.class, self.escape)
    }
}

impl std::fmt::Display for Quirk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} [{}] symptom: {}; detection: {}; escape: {}; cost: {}",
            self.name, self.class, self.symptom, self.detection, self.escape, self.cost
        )
    }
}

/// The table. Rows are stable once shipped (a row is a public
/// commitment — the name and the escape never move).
pub const TABLE: &[Quirk] = &[
    Quirk {
        name: "psr-flicker",
        class: QuirkClass::Panel,
        symptom: "visible flicker on static content while panel self-refresh is engaged",
        detection: "the flicker stops within one refresh of `--no-psr` (a static \
                    framebuffer never flickers — it is the self-refresh transition)",
        escape: "--no-psr",
        cost: "the display engine keeps scanning a static framebuffer — the \
               sleep-tier power savings are traded away",
    },
    Quirk {
        name: "vrr-flicker",
        class: QuirkClass::Sync,
        symptom: "brightness pumping while an adaptive-sync panel stretches its \
                  scanout clock inside the VRR window",
        detection: "the pumping stops with the fixed nominal grid (serve without \
                    `--vrr`) — it is the panel's backlight response to the \
                    stretched clock, not the content",
        escape: "serve without --vrr",
        cost: "the commit window stays at the fixed nominal — late commits \
              cannot borrow the panel's stretch",
    },
    // Phase 38's first accrued row (the v0.10.4 doctrine: "the
    // mechanism shipped; the rows accrue"): the HDR bloated-peak
    // class. Real panels advertise a peak their backlight cannot
    // sustain — highlights flatten late and the tone mapping aims
    // at a ceiling the panel never shows. The escape is the
    // operator's honesty knob.
    Quirk {
        name: "hdr-peak-bloat",
        class: QuirkClass::Advertising,
        symptom: "HDR highlights flatten or dim late — the tone mapping aims \
                  at a peak the panel's backlight never actually shows",
        detection: "the panel's EDID peak exceeds what a meter reads on a \
                    full-white field (the advertisement is the bloated side; \
                    the measurement is the sustained truth)",
        escape: "--hdr-peak N (the measured sustained peak)",
        cost: "highlights above the measured peak compress one stop earlier \
              — honestly, instead of the panel's own late clip",
    },
    // Phase 41's accrued pair (the quirk ledger: the VRR row's named
    // remainder — the multi-year driver quirk table — answered with
    // the structure the decades fill, the first rows tabled as
    // mechanism-grade escapes).
    //
    // The floor flicker: the advertising class's VRR arm — the
    // panel's advertised minimum refresh rate is the bloated side
    // (the same doctrine as the HDR peak: the EDID claims a stretch
    // the hardware cannot sustain). The escape raises the honest
    // floor; the LFC cadence bridges the content the removed band
    // used to stretch for.
    Quirk {
        name: "vrr-floor-flicker",
        class: QuirkClass::Advertising,
        symptom: "brightness pumping on slow content — the panel flickers \
                  only while the scanout clock stretches toward its \
                  advertised minimum refresh rate",
        detection: "the pumping stops with the fixed nominal grid (serve \
                    without --vrr) but not with faster content — it is the \
                    deepest stretch band, not the VRR mechanism; a floor \
                    above the band (--vrr-floor 57 on a panel advertising \
                    48, say) removes it",
        escape: "--vrr-floor N (the honest minimum rate, Hz)",
        cost: "content slower than the floor rides the LFC repeats \
              instead of the removed stretch band — the cadence is \
              phase-aligned, the deep stretch is gone",
    },
    // The sibling flicker: the sync class's cross-CRTC arm — a
    // fixed-sync display visibly flickers while a VRR sibling
    // stretches (the scanout engines' clock coupling the pre-2020
    // driver era worked around by disabling VRR on mixed desktops).
    // The default is the modern doctrine (per-output VRR — the
    // per-display behavior Windows serves); the escape is the
    // conservative uniform desktop.
    Quirk {
        name: "vrr-sibling-flicker",
        class: QuirkClass::Sync,
        symptom: "a fixed-sync display flickers while its VRR sibling \
                  stretches — visible only on a mixed desktop (one \
                  adaptive-sync output, one fixed)",
        detection: "the flicker stops when the whole desktop serves fixed \
                    (without --vrr) — the outputs' scanout clocks are \
                    coupled on this platform; the panel itself is healthy",
        escape: "--vrr-uniform (one fixed desktop across the seam)",
        cost: "the VRR output pays the fixed grid — the late-commit \
              stretch and the adaptive opportunity both lapse until \
              the desktop is uncoupled",
    },
    // Phase 42's accrued trio (the mode foundry: the display-size
    // row's own named remainder — "decades of EDID/timing quirk
    // coverage" — answered with the timing-depth rows the foundry
    // makes *escapable*). Each row's detection is the audit's own
    // named finding; each escape is deterministic.
    //
    // The rotten EDID: the sink's identity block fails its own
    // checksum or header — the connector still enumerates (the
    // display engine's probe carries the mode list), but the
    // identity is unknown and the timing truth absent.
    Quirk {
        name: "edid-rotten",
        class: QuirkClass::Timing,
        symptom: "the monitor reports no identity — the startup line \
                  names the connector, not the panel; persistence keys \
                  cannot bind to the monitor",
        detection: "the bring-up audit prints `edid-rotten` (the EDID \
                    bytes fail the checksum or header parse) while the \
                    connector still enumerates modes — the block is \
                    corrupt, the panel is not",
        escape: "--synth WxH (pour the known truth: a reduced-blanking \
                timing for the size the panel actually scans) or \
                --resolution WxH",
        cost: "the pour is a user-defined mode — the display engine \
              validates it at commit, and the panel's own preferred \
              timing stays unknown until the cable or firmware is \
              fixed",
    },
    // The preferred-timing lie: stale monitor firmware whose EDID
    // names a smaller preferred mode than the connector serves —
    // the desktop lands on the stale timing and wastes the panel.
    Quirk {
        name: "edid-preferred-lie",
        class: QuirkClass::Timing,
        symptom: "the desktop comes up smaller than the panel — the \
                  monitor's EDID prefers 720p-era timings while the \
                  connector enumerates the full-size modes",
        detection: "the bring-up audit prints `edid-preferred-lie` (the \
                    EDID's first detailed timing names a smaller \
                    active area than the connector's largest offered \
                    mode) — stale firmware, not a driver bug",
        escape: "--resolution WxH (force the panel's real size from \
                the offered list) or --synth WxH (pour it when even \
                the list is wrong)",
        cost: "the operator asserts what the firmware should have — \
              a wrong guess serves a wrong size, honestly refused \
              only when the connector does not offer it",
    },
    // The pixel-clock ceiling: the enumerated list carries modes
    // above the EDID's declared maximum clock — exactly the modes
    // that blank or drop on the panel when selected.
    Quirk {
        name: "pixel-clock-ceiling",
        class: QuirkClass::Timing,
        symptom: "the highest mode blanks the panel — the mode \
                  enumerates, the commit validates, the display \
                  stays dark or drops to black mid-scan",
        detection: "the bring-up audit prints `pixel-clock-ceiling` \
                    (the connector's best mode clocks above the \
                    EDID range-limits descriptor's declared maximum) \
                    — the panel's own envelope is the truth",
        escape: "--resolution WxH with a mode under the declared \
                ceiling, or --synth WxH@Hz (the foundry refuses \
                pours above the ceiling, naming it — never a silent \
                clamp)",
        cost: "the desktop stays inside the panel's honest envelope \
              — the above-ceiling modes the list tempts with are \
              traded away",
    },
];

/// Look a quirk up by its stable name.
#[must_use]
pub fn by_name(name: &str) -> Option<&'static Quirk> {
    TABLE.iter().find(|q| q.name == name)
}

/// Every quirk's escape, in table order — `(quirk name, escape)`.
#[must_use]
pub fn escapes() -> Vec<(&'static str, &'static str)> {
    TABLE.iter().map(|q| (q.name, q.escape)).collect()
}

/// The one-line report (the selftest's quirk line): the row count and
/// every escape in table order.
#[must_use]
pub fn report() -> String {
    let rows: Vec<String> = TABLE.iter().map(Quirk::row).collect();
    format!("{} quirk(s) tabled: {}", TABLE.len(), rows.join("; "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_row_names_an_escape() {
        // The doctrine: no quirk is tabled without a route around it.
        for q in TABLE {
            assert!(!q.name.is_empty());
            assert!(!q.escape.is_empty());
            assert!(!q.symptom.is_empty());
            assert!(!q.detection.is_empty());
            assert!(!q.cost.is_empty());
        }
    }

    #[test]
    fn names_are_unique_and_stable() {
        let mut names: Vec<&str> = TABLE.iter().map(|q| q.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), TABLE.len());
        // The shipped rows (a row is a public commitment — this pins
        // every one, sorted).
        assert_eq!(
            names,
            vec![
                "edid-preferred-lie",
                "edid-rotten",
                "hdr-peak-bloat",
                "pixel-clock-ceiling",
                "psr-flicker",
                "vrr-flicker",
                "vrr-floor-flicker",
                "vrr-sibling-flicker"
            ]
        );
    }

    #[test]
    fn lookup_finds_the_shipped_rows() {
        assert_eq!(by_name("psr-flicker").map(|q| q.escape), Some("--no-psr"));
        assert_eq!(
            by_name("vrr-flicker").map(|q| q.class),
            Some(QuirkClass::Sync)
        );
        // Phase 38's accrued row: the bloated-peak class, the
        // operator's honesty knob.
        assert_eq!(
            by_name("hdr-peak-bloat").map(|q| q.class),
            Some(QuirkClass::Advertising)
        );
        // Phase 41's accrued pair: the VRR floor (advertising) and
        // the sibling (sync).
        assert_eq!(
            by_name("vrr-floor-flicker").map(|q| q.class),
            Some(QuirkClass::Advertising)
        );
        assert_eq!(
            by_name("vrr-sibling-flicker").map(|q| q.class),
            Some(QuirkClass::Sync)
        );
        // Phase 42's accrued trio: the foundry rows (the timing
        // class), each with a deterministic escape.
        assert_eq!(
            by_name("edid-rotten").map(|q| q.class),
            Some(QuirkClass::Timing)
        );
        assert_eq!(
            by_name("edid-preferred-lie").map(|q| q.class),
            Some(QuirkClass::Timing)
        );
        assert_eq!(
            by_name("pixel-clock-ceiling").map(|q| q.class),
            Some(QuirkClass::Timing)
        );
        assert!(by_name("no-such-quirk").is_none());
    }

    #[test]
    fn report_names_every_escape() {
        let r = report();
        assert!(r.contains("8 quirk(s)"));
        assert!(r.contains("--no-psr"));
        assert!(r.contains("without --vrr"));
        assert!(r.contains("--hdr-peak"));
        assert!(r.contains("--vrr-floor"));
        assert!(r.contains("--vrr-uniform"));
        // Phase 42's trio: the foundry escapes.
        assert!(r.contains("--synth"));
        assert!(r.contains("--resolution"));
    }
}
