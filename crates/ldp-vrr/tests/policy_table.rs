//! Phase 15 exit criterion 1 — policy table conformance.
//!
//! The decision table is walked **exhaustively**: every combination of
//! policy × support × adaptive demand × battery state is checked
//! against an independently hand-derived expected row (the table below
//! is written out literally, not computed by calling the engine). The
//! widening arithmetic is pinned to the nanosecond against the panel
//! constants, and the wire vocabulary round-trips.

use ldp_core::time::PresentationMode;
use ldp_vrr::caps::{OutputVrrSupport, PanelWindow, VrrCaps, WindowError};
use ldp_vrr::policy::{decide, mode_demands_vrr, PolicyInputs, Rationale, VrrPolicy};

// 48–144 Hz around a 60 Hz mode (the mock panel's window).
const MIN: u64 = 1_000_000_000 / 144;
const MAX: u64 = 1_000_000_000 / 48;
const NOM: u64 = 16_666_666;

fn support(caps: VrrCaps) -> OutputVrrSupport {
    OutputVrrSupport::new(MIN, MAX, caps, NOM).unwrap()
}

const BOTH: VrrCaps = VrrCaps {
    seamless: true,
    fixed_rate: true,
};
const NEITHER: VrrCaps = VrrCaps {
    seamless: false,
    fixed_rate: false,
};

/// The hand-derived table row for one combination.
struct Row {
    enabled: bool,
    has_window: bool,
    widening: u64,
    rationale: Rationale,
}

/// The normative table, written out literally: (policy, supported,
/// demand, battery) → (enabled, window, widening, rationale).
fn expected_row(policy: VrrPolicy, supported: bool, demand: bool, battery: bool) -> Row {
    use Rationale as R;
    // Policy off wins over everything.
    if policy == VrrPolicy::Off {
        return Row {
            enabled: false,
            has_window: false,
            widening: 0,
            rationale: R::PolicyOff,
        };
    }
    // An unsupported panel cannot run VRR at all.
    if !supported {
        return Row {
            enabled: false,
            has_window: false,
            widening: 0,
            rationale: R::Unsupported,
        };
    }
    // Always policy is its own demand; deadline needs an adaptive
    // surface.
    let wants = demand || policy == VrrPolicy::Always;
    if !wants {
        return Row {
            enabled: false,
            has_window: false,
            widening: 0,
            rationale: R::NoDemand,
        };
    }
    // Battery saver conservatively forces fixed sync.
    if battery {
        return Row {
            enabled: false,
            has_window: false,
            widening: 0,
            rationale: R::BatterySaver,
        };
    }
    match policy {
        VrrPolicy::Deadline => Row {
            enabled: true,
            has_window: true,
            // The stretch a late commit may consume beyond the nominal
            // grid point it targets.
            widening: MAX - NOM,
            rationale: R::DeadlineWindow,
        },
        _ => Row {
            enabled: true,
            has_window: true,
            // The full selectable span: every in-window commit is on
            // time under commit-at-ready.
            widening: MAX - MIN,
            rationale: R::AlwaysOn,
        },
    }
}

#[test]
fn exhaustive_table_conformance() {
    let policies = [VrrPolicy::Off, VrrPolicy::Deadline, VrrPolicy::Always];
    let supports = [None, Some(support(BOTH)), Some(support(NEITHER))];
    let mut rows = 0usize;
    for &policy in &policies {
        for support in &supports {
            for &demand in &[false, true] {
                for &battery in &[false, true] {
                    let supported = support.is_some();
                    let row = expected_row(policy, supported, demand, battery);
                    let d = decide(&PolicyInputs {
                        policy,
                        support: support.as_ref(),
                        adaptive_demand: demand,
                        battery_saver: battery,
                    });
                    assert_eq!(
                        d.vrr_enabled, row.enabled,
                        "policy={policy:?} supported={supported} demand={demand} battery={battery}"
                    );
                    assert_eq!(
                        d.window.is_some(), row.has_window,
                        "window presence, policy={policy:?} supported={supported} demand={demand} battery={battery}"
                    );
                    assert_eq!(
                        d.scheduler_window_ns, row.widening,
                        "widening, policy={policy:?} supported={supported} demand={demand} battery={battery}"
                    );
                    assert_eq!(
                        d.rationale, row.rationale,
                        "rationale, policy={policy:?} supported={supported} demand={demand} battery={battery}"
                    );
                    if let Some(w) = d.window {
                        assert_eq!((w.min_ns(), w.max_ns()), (MIN, MAX));
                    }
                    rows += 1;
                }
            }
        }
    }
    // 3 policies × 3 support states × 2 demand × 2 battery = 36 rows.
    assert_eq!(rows, 36, "the walk must be exhaustive");
}

#[test]
fn engaged_decision_carries_exact_window_and_widening() {
    let s = support(BOTH);
    let d = decide(&PolicyInputs {
        policy: VrrPolicy::Deadline,
        support: Some(&s),
        adaptive_demand: true,
        battery_saver: false,
    });
    let w = d.window.expect("window engaged");
    assert_eq!(w.min_ns(), 6_944_444);
    assert_eq!(w.max_ns(), 20_833_333);
    assert_eq!(d.scheduler_window_ns, 4_166_667); // MAX - NOM exactly
    assert_eq!(w.scheduler_window(NOM, false), MAX - NOM);
    assert_eq!(w.scheduler_window(NOM, true), MAX - MIN);
}

#[test]
fn seamless_and_fixed_rate_bits_flow_from_caps_only() {
    // The capability bits never change enablement or widening — they
    // shape the KMS toggle and the deferral path only.
    for caps in [BOTH, NEITHER] {
        let s = support(caps);
        for &policy in &[VrrPolicy::Deadline, VrrPolicy::Always] {
            let d = decide(&PolicyInputs {
                policy,
                support: Some(&s),
                adaptive_demand: true,
                battery_saver: false,
            });
            assert!(d.vrr_enabled);
            assert!(d.window.is_some());
            assert_eq!(
                d.scheduler_window_ns,
                match policy {
                    VrrPolicy::Always => MAX - MIN,
                    _ => MAX - NOM,
                }
            );
        }
    }
}

#[test]
fn policy_wire_round_trips_every_value() {
    for v in 1..=3u32 {
        let p = VrrPolicy::from_wire(v).expect("defined values parse");
        assert_eq!(p.to_wire(), v);
    }
    for v in [0u32, 4, 5, u32::MAX] {
        assert!(VrrPolicy::from_wire(v).is_none());
    }
    // The spec vocabulary: off=1, deadline=2, always=3.
    assert_eq!(VrrPolicy::Off.to_wire(), 1);
    assert_eq!(VrrPolicy::Deadline.to_wire(), 2);
    assert_eq!(VrrPolicy::Always.to_wire(), 3);
    assert_eq!(VrrPolicy::default(), VrrPolicy::Off);
}

#[test]
fn caps_wire_bits_match_spec() {
    // vrr_caps: seamless=0, fixed_rate=1.
    assert_eq!(BOTH.to_wire(), 0b11);
    assert_eq!(NEITHER.to_wire(), 0);
    assert_eq!(
        VrrCaps::from_wire(0b01),
        VrrCaps {
            seamless: true,
            fixed_rate: false
        }
    );
    assert_eq!(
        VrrCaps::from_wire(0b10),
        VrrCaps {
            seamless: false,
            fixed_rate: true
        }
    );
    // Unknown bits are ignored on decode.
    assert_eq!(
        VrrCaps::from_wire(0b1111_1100),
        VrrCaps {
            seamless: false,
            fixed_rate: false
        }
    );
}

#[test]
fn panel_window_wire_conversions() {
    // The output.vrr event fields: min_refresh_millihz=48000,
    // max_refresh_millihz=144000.
    let w = PanelWindow::from_millihz(48_000, 144_000).expect("legal window");
    assert_eq!(w.min_ns(), 1_000_000_000_000 / 144_000);
    assert_eq!(w.max_ns(), 1_000_000_000_000 / 48_000);
    let (lo, hi) = w.as_millihz();
    assert_eq!((lo, hi), (48_000, 144_000));
    assert!(PanelWindow::from_millihz(144_000, 48_000).is_err());
    assert!(PanelWindow::from_millihz(0, 144_000).is_err());
    assert!(PanelWindow::from_millihz(48_000, 0).is_err());
}

#[test]
fn support_validation_rejects_out_of_window_nominals() {
    for bad_nominal in [6_060_606u64, 25_000_000, 0] {
        assert_eq!(
            OutputVrrSupport::new(MIN, MAX, BOTH, bad_nominal).unwrap_err(),
            if bad_nominal == 0 {
                WindowError::ZeroNominal
            } else {
                WindowError::NominalOutside
            },
            "nominal {bad_nominal} must not run VRR"
        );
    }
    assert_eq!(
        OutputVrrSupport::new(MAX, MIN, BOTH, NOM).unwrap_err(),
        WindowError::Inverted
    );
}

#[test]
fn input_helpers() {
    let s = support(BOTH);
    for &policy in &[VrrPolicy::Off, VrrPolicy::Deadline, VrrPolicy::Always] {
        let inputs = PolicyInputs {
            policy,
            support: Some(&s),
            adaptive_demand: false,
            battery_saver: false,
        };
        assert_eq!(inputs.wants_vrr(), policy != VrrPolicy::Off);
        assert_eq!(inputs.has_demand(), policy == VrrPolicy::Always);
        let inputs = PolicyInputs {
            policy,
            support: Some(&s),
            adaptive_demand: true,
            battery_saver: false,
        };
        assert!(inputs.has_demand());
    }
    assert!(mode_demands_vrr(PresentationMode::Adaptive));
    assert!(!mode_demands_vrr(PresentationMode::Vsync));
    assert!(!mode_demands_vrr(PresentationMode::Immediate));
}

#[test]
fn rationale_set_is_the_audit_vocabulary() {
    // Every rationale is reachable by some row (the table walk above
    // pins which); this test keeps the set honest against accidental
    // additions by listing them exhaustively.
    let all = [
        Rationale::PolicyOff,
        Rationale::Unsupported,
        Rationale::NoDemand,
        Rationale::BatterySaver,
        Rationale::DeadlineWindow,
        Rationale::AlwaysOn,
    ];
    let s = support(BOTH);
    let reached = [
        decide(&PolicyInputs {
            policy: VrrPolicy::Off,
            support: Some(&s),
            adaptive_demand: true,
            battery_saver: false,
        })
        .rationale,
        decide(&PolicyInputs {
            policy: VrrPolicy::Deadline,
            support: None,
            adaptive_demand: true,
            battery_saver: false,
        })
        .rationale,
        decide(&PolicyInputs {
            policy: VrrPolicy::Deadline,
            support: Some(&s),
            adaptive_demand: false,
            battery_saver: false,
        })
        .rationale,
        decide(&PolicyInputs {
            policy: VrrPolicy::Deadline,
            support: Some(&s),
            adaptive_demand: true,
            battery_saver: true,
        })
        .rationale,
        decide(&PolicyInputs {
            policy: VrrPolicy::Deadline,
            support: Some(&s),
            adaptive_demand: true,
            battery_saver: false,
        })
        .rationale,
        decide(&PolicyInputs {
            policy: VrrPolicy::Always,
            support: Some(&s),
            adaptive_demand: false,
            battery_saver: false,
        })
        .rationale,
    ];
    for (i, r) in reached.iter().enumerate() {
        assert_eq!(r, &all[i], "rationale {i} must be reachable in order");
    }
}
