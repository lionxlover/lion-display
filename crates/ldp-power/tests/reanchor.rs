//! THE Phase 16 exit criterion (part 5): suspend/resume re-anchoring —
//! the compositor's real frame clock produces correctly-spaced,
//! never-in-the-past deadlines after a sleep, when re-anchored on the
//! computed boundary.
//!
//! The scenario (repeated across refresh rates 144/90/60/40 Hz and
//! sleep depths): anchor a [`FrameClock`] on an exact pre-suspend flip
//! grid, sleep (no flips — the timeline is stale), wake, feed the
//! [`ReAnchor::grid_align`] boundary as the first post-resume flip,
//! and verify:
//!
//! 1. The re-anchor boundary is exactly on the pre-suspend grid
//!    (phase preservation) and strictly after the wake.
//! 2. Every post-resume prediction is strictly after the wake.
//! 3. The first prediction sits exactly one nominal interval after the
//!    re-anchoring flip (PLL lock at zero phase error).
//! 4. A chain of predictions spaces at exactly the nominal interval —
//!    no drift accumulated across the suspend.
//! 5. The stale timeline (no re-anchoring) disagrees: its extrapolated
//!    grid is not re-anchored, proving the test has teeth.

use ldp_compositor::predictor::FrameClock;
use ldp_core::time::{Mono, RefreshInterval};
use ldp_power::suspend::{ReAnchor, SleepKind, SuspendController};

fn ms(m: u64) -> Mono {
    Mono::from_ms(m)
}

fn rates() -> Vec<RefreshInterval> {
    [144_000u32, 90_000, 60_000, 40_000]
        .iter()
        .map(|mh| RefreshInterval::from_millihz(*mh).unwrap())
        .collect()
}

/// Run a pre-suspend grid of flips at exactly `nominal` spacing from
/// `t0` (PLL lock), returning the clock and the last flip.
fn anchored_clock(nominal: RefreshInterval, t0: Mono, flips: u64) -> (FrameClock, Mono) {
    let mut clock = FrameClock::new(nominal);
    let mut last = t0;
    for _ in 0..flips {
        clock.observe_flip(last);
        last = Mono::from_ns(last.as_ns() + nominal.as_ns());
    }
    (clock, Mono::from_ns(last.as_ns() - nominal.as_ns()))
}

#[test]
fn reanchored_clock_yields_exact_healthy_predictions() {
    for nominal in rates() {
        for sleep_ms in [500u64, 5_000, 60_000] {
            let t0 = ms(1_000);
            let (mut clock, last_flip) = anchored_clock(nominal, t0, 60);
            // Sleep: the controller reports duration + re-anchor.
            let mut controller = SuspendController::new();
            assert!(controller.sleep(SleepKind::Idle, last_flip));
            let wake = Mono::from_ns(last_flip.as_ns() + sleep_ms * 1_000_000);
            let report = controller.wake(wake, nominal, last_flip).unwrap();
            assert_eq!(report.suspended_ms, sleep_ms);

            // 1. On-grid and strictly after the wake.
            let anchor = report.re_anchor;
            assert!(anchor.as_ns() > wake.as_ns());
            assert_eq!(
                (anchor.as_ns() - last_flip.as_ns()) % nominal.as_ns(),
                0,
                "re-anchor lost the pre-suspend phase"
            );

            // Feed the re-anchoring flip.
            let obs = clock.observe_flip(anchor);
            assert_eq!(obs.ts, anchor);

            // 2 + 3 + 4: predictions after the wake, first one exactly
            // one interval out, chain spaced at nominal.
            let first = clock.next_vblank(wake).expect("anchored");
            assert!(first.as_ns() > wake.as_ns(), "prediction in the past");
            assert_eq!(first.as_ns(), anchor.as_ns() + nominal.as_ns());
            let mut prev = anchor;
            for n in 1..=10u32 {
                let nth = clock.nth_vblank(wake, n).unwrap();
                assert_eq!(nth.as_ns(), anchor.as_ns() + u64::from(n) * nominal.as_ns());
                assert!(nth.as_ns() > prev.as_ns());
                prev = nth;
            }
        }
    }
}

#[test]
fn reanchor_grid_align_boundaries() {
    for nominal in rates() {
        let t0 = ms(7);
        let (_, last_flip) = anchored_clock(nominal, t0, 3);
        let interval = nominal.as_ns();
        // Wake exactly on a boundary: next boundary out.
        let wake = Mono::from_ns(last_flip.as_ns() + 17 * interval);
        assert_eq!(
            ReAnchor::grid_align(wake, nominal, last_flip).as_ns(),
            last_flip.as_ns() + 18 * interval
        );
        // Wake between boundaries: round up.
        let wake = Mono::from_ns(last_flip.as_ns() + 17 * interval + interval / 2);
        assert_eq!(
            ReAnchor::grid_align(wake, nominal, last_flip).as_ns(),
            last_flip.as_ns() + 18 * interval
        );
        // Wake before the last flip: no phase — one interval after it.
        let wake = Mono::from_ns(last_flip.as_ns() / 2);
        assert_eq!(
            ReAnchor::grid_align(wake, nominal, last_flip).as_ns(),
            last_flip.as_ns() + interval
        );
        // Unknown anchor: one interval after the wake.
        let wake = ms(500);
        assert_eq!(
            ReAnchor::grid_align(wake, nominal, Mono::ZERO).as_ns(),
            wake.as_ns() + interval
        );
    }
}

#[test]
fn stale_timeline_disagrees_with_the_reanchored_one() {
    // The teeth check: without feeding the re-anchoring flip, the
    // pre-suspend clock's extrapolation is anchored far in the past —
    // its grid is NOT the post-resume grid (different phase than the
    // re-anchored clock's first prediction).
    let nominal = RefreshInterval::from_millihz(60_000).unwrap();
    let t0 = ms(0);
    let (mut clock, last_flip) = anchored_clock(nominal, t0, 10);
    let wake = Mono::from_ns(last_flip.as_ns() + 5_000_000_000);
    let anchor = ReAnchor::grid_align(wake, nominal, last_flip);

    // Stale: extrapolate from the pre-suspend grid without the new flip.
    let stale_next = clock.next_vblank(wake).unwrap();
    // Now re-anchor for real.
    clock.observe_flip(anchor);
    let fresh_next = clock.next_vblank(wake).unwrap();

    // Both are in the future, but they are NOT the same instant: the
    // stale grid preserves the pre-suspend phase relative to its own
    // anchor, while the fresh grid starts at the re-anchor flip. With a
    // wake chosen off-grid, the two differ.
    assert!(stale_next.as_ns() > wake.as_ns());
    assert!(fresh_next.as_ns() > wake.as_ns());
    assert_ne!(stale_next, fresh_next, "re-anchoring changed nothing");
    // The fresh grid is the honest one: exactly one interval past the
    // re-anchoring flip.
    assert_eq!(fresh_next.as_ns(), anchor.as_ns() + nominal.as_ns());
}

#[test]
fn controller_reports_and_double_transitions() {
    let mut controller = SuspendController::new();
    let nominal = RefreshInterval::from_millihz(60_000).unwrap();
    // Awake wake is a no-op.
    assert!(controller.wake(ms(10), nominal, ms(5)).is_none());
    // Sleep kinds round-trip on the wire.
    for (kind, wire) in [
        (SleepKind::Idle, 1u32),
        (SleepKind::Manual, 2),
        (SleepKind::Lid, 3),
        (SleepKind::LowBattery, 4),
    ] {
        assert_eq!(kind.to_wire(), wire);
        assert_eq!(SleepKind::from_wire(wire), Some(kind));
    }
    // Manual sleep, wake, sleep again — no nesting.
    assert!(controller.sleep(SleepKind::Manual, ms(100)));
    assert!(!controller.sleep(SleepKind::Lid, ms(100)));
    let report = controller.wake(ms(2_600), nominal, ms(90)).unwrap();
    assert_eq!(report.suspended_ms, 2_500);
    assert!(controller.sleep(SleepKind::LowBattery, ms(3_000)));
    assert!(controller.wake(ms(3_500), nominal, Mono::ZERO).is_some());
}
