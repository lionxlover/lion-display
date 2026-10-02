//! Phase 7 exit criterion: deadline hit-rate properties under jitter.
//!
//! Every scenario is a full simulated timeline (see `sched/mod.rs`)
//! driven through the real scheduler. The properties assert *rates and
//! invariants*, not exact events (that is the golden suite's job):
//! hit rates hold across seeds, drop reasons stay in their lanes, the
//! PLL locks under drift, the escalation ladder converges slow clients,
//! stalls recover, and per-surface scheduling does not starve.

#[path = "sched/mod.rs"]
mod sched;

use ldp_compositor::scheduler::SchedEvent;
use ldp_compositor::SurfaceId;
use ldp_core::time::FrameDropReason;
use sched::{
    scenario_adaptive, scenario_adaptive_vsync_control, scenario_drift_lock, scenario_immediate,
    scenario_multi_client, scenario_nominal_jitter, scenario_slow_client, scenario_stall_resync,
};

/// Terminal outcomes as a hit/miss vector, in order.
fn terminals(events: &[SchedEvent]) -> Vec<bool> {
    events
        .iter()
        .filter_map(|event| match *event {
            SchedEvent::Presented { .. } => Some(true),
            SchedEvent::FrameDropped { reason, .. } => match reason {
                FrameDropReason::DeadlineMissed => Some(false),
                _ => None,
            },
            SchedEvent::FrameTarget { .. } => None,
        })
        .collect()
}

fn rate(slice: &[bool]) -> f64 {
    if slice.is_empty() {
        1.0
    } else {
        slice.iter().filter(|hit| **hit).count() as f64 / slice.len() as f64
    }
}

#[test]
fn nominal_jitter_keeps_hit_rate() {
    for seed in 1..=3u64 {
        let scenario = scenario_nominal_jitter(seed);
        // A comfortable client misses only on extreme jitter tails.
        assert!(
            scenario.stats.hit_rate() >= 0.95,
            "seed {seed}: hit rate {}",
            scenario.stats.hit_rate()
        );
        let terminals = terminals(&scenario.outputs);
        assert!(terminals.len() >= 400, "seed {seed}: too few terminals");
        // Same seed, same run: bit-identical statistics (determinism).
        let again = scenario_nominal_jitter(seed);
        assert_eq!(again.stats, scenario.stats);
        assert_eq!(again.outputs, scenario.outputs);
    }
}

#[test]
fn pll_locks_under_period_drift() {
    for seed in 1..=3u64 {
        let scenario = scenario_drift_lock(seed);
        let terminals = terminals(&scenario.outputs);
        assert!(terminals.len() >= 300);
        let quarter = terminals.len() / 4;
        let first = rate(&terminals[..quarter]);
        let last = rate(&terminals[terminals.len() / 2..]);
        // The drift is real: pre-lock frames missed (the nominal-grid
        // deadline is ~0.93 ms early against a 17.6 ms panel).
        assert!(
            scenario.stats.deadline_missed >= 1,
            "seed {seed}: no drift cost"
        );
        assert!(first < 1.0, "seed {seed}: pre-lock quarter was clean?!");
        // After lock the deadlines hold.
        assert!(last >= 0.98, "seed {seed}: no lock, last-half {last}");
        // The lock is visible in the contract itself: post-lock frame
        // targets carry a budget of (true period - costs), not the
        // nominal one.
        let budgets: Vec<u64> = scenario
            .outputs
            .iter()
            .rev()
            .filter_map(|event| match *event {
                SchedEvent::FrameTarget { deadline, .. } => Some(deadline.budget_ns),
                _ => None,
            })
            .take(20)
            .collect();
        let mean: u64 = budgets.iter().sum::<u64>() / budgets.len() as u64;
        assert!(
            (mean as i64 - 16_600_000).abs() < 200_000,
            "seed {seed}: post-lock budget {mean} did not track the true period"
        );
    }
}

#[test]
fn escalation_converges_slow_clients() {
    for seed in 1..=3u64 {
        let scenario = scenario_slow_client(seed);
        // ~80% expected: one miss buys extra lead, four hits pay it back.
        assert!(
            scenario.stats.hit_rate() >= 0.75,
            "seed {seed}: hit rate {}",
            scenario.stats.hit_rate()
        );
        // Only deadline misses — the ladder is the only machinery in play.
        assert_eq!(scenario.stats.superseded, 0);
        assert_eq!(scenario.stats.hidden, 0);
        assert_eq!(scenario.stats.output_off, 0);
        assert!(scenario.stats.deadline_missed > 0, "escalation must engage");
    }
}

#[test]
fn stall_reanchors_without_miss_storm() {
    for seed in 1..=3u64 {
        let scenario = scenario_stall_resync(seed);
        let terminals = terminals(&scenario.outputs);
        let last = rate(&terminals[terminals.len() / 2..]);
        assert!(last >= 0.95, "seed {seed}: post-stall rate {last}");
        assert!(
            scenario.stats.deadline_missed <= 3,
            "seed {seed}: {} misses around the stall",
            scenario.stats.deadline_missed
        );
    }
}

#[test]
fn immediate_mode_presents_everything_torn() {
    for seed in 1..=3u64 {
        let scenario = scenario_immediate(seed);
        assert_eq!(scenario.stats.deadline_missed, 0);
        assert!(
            scenario.stats.presented >= 150,
            "seed {seed}: only {} presentations",
            scenario.stats.presented
        );
        let torn = scenario
            .outputs
            .iter()
            .filter(|event| matches!(event, SchedEvent::Presented { .. }))
            .filter(|event| match *event {
                SchedEvent::Presented { timing, .. } => timing.flags.torn && !timing.flags.vblank,
                _ => false,
            })
            .count();
        assert_eq!(torn, scenario.stats.presented);
    }
}

#[test]
fn adaptive_window_eliminates_misses_vs_control() {
    for seed in 1..=3u64 {
        let adaptive = scenario_adaptive(seed);
        assert_eq!(
            adaptive.stats.deadline_missed, 0,
            "seed {seed}: the window must absorb the lateness"
        );
        // Control: same seed and render time without the window misses.
        let control = scenario_adaptive_vsync_control(seed);
        assert!(
            control.stats.deadline_missed > 0,
            "seed {seed}: control unexpectedly clean"
        );
    }
}

#[test]
fn multi_client_no_starvation() {
    for seed in 1..=3u64 {
        let scenario = scenario_multi_client(seed);
        for surface_raw in 1..=3u64 {
            let surface = SurfaceId::from_raw(surface_raw);
            let hits = scenario
                .outputs
                .iter()
                .filter(|event| matches!(event, SchedEvent::Presented { surface: s, .. } if *s == surface))
                .count();
            let misses = scenario
                .outputs
                .iter()
                .filter(
                    |event| matches!(event,
                        SchedEvent::FrameDropped { surface: s, reason: FrameDropReason::DeadlineMissed, .. }
                            if *s == surface),
                )
                .count();
            let total = hits + misses;
            assert!(
                total >= 250,
                "seed {seed} surface {surface_raw}: {total} terminals"
            );
            let rate = hits as f64 / total as f64;
            assert!(
                rate >= 0.8,
                "seed {seed} surface {surface_raw}: rate {rate}"
            );
        }
        // Global stats are the sum of per-surface outcomes.
        let sum: usize = (1..=3u64)
            .map(|s| {
                scenario
                    .outputs
                    .iter()
                    .filter(|e| matches!(e, SchedEvent::Presented { surface, .. } if surface.raw() == s))
                    .count()
            })
            .sum();
        assert_eq!(sum, scenario.stats.presented);
    }
}

#[test]
fn presented_refresh_tracks_the_panel_not_the_mode() {
    // Under drift, the `presented` refresh feedback must approach the
    // true panel period (17.6 ms), not stay at the nominal mode.
    let scenario = scenario_drift_lock(1);
    let last: Vec<u64> = scenario
        .outputs
        .iter()
        .rev()
        .filter_map(|event| match *event {
            SchedEvent::Presented { timing, .. } => Some(timing.refresh.as_ns()),
            _ => None,
        })
        .take(20)
        .collect();
    let mean: u64 = last.iter().sum::<u64>() / last.len() as u64;
    assert!(
        (mean as i64 - 17_600_000).abs() < 300_000,
        "measured refresh {mean} ns did not track the true period"
    );
}
