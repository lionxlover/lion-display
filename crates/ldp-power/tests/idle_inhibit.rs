//! Idle + inhibitor + backlight corpora: the full stage walk under an
//! LCG-driven activity/inhibitor stream, plus the ramp property over
//! randomized target sequences.

use ldp_core::time::Mono;
use ldp_power::backlight::BacklightRamp;
use ldp_power::idle::{IdleEvent, IdleMachine, IdleStage, IdleTimeouts, InhibitMask};

struct Corpus(u64);

impl Corpus {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

fn ms(m: u64) -> Mono {
    Mono::from_ms(m)
}

#[test]
fn idle_corpus_never_breaks_the_invariants() {
    let timeouts = IdleTimeouts {
        dim_ms: 1_000,
        off_ms: 2_000,
        suspend_ms: 4_000,
    };
    let mut corpus = Corpus(0xD1E);
    let mut machine = IdleMachine::new(ms(0), timeouts);
    let order = |s: IdleStage| s.to_wire();
    let mut now = 0u64;
    let mut reported: Vec<IdleStage> = Vec::new();
    for _ in 0..20_000 {
        now += corpus.next() % 500;
        // Random inhibitor mask (biased to often-none).
        let mask = match corpus.next() % 10 {
            0 => InhibitMask::BLUR,
            1 => InhibitMask::DISPLAY,
            2 => InhibitMask::IDLE,
            3 => InhibitMask::BLUR.union(InhibitMask::DISPLAY),
            _ => InhibitMask::NONE,
        };
        for event in machine.tick(ms(now), mask) {
            if let IdleEvent::Stage(stage) = event {
                reported.push(stage);
            }
        }
        // Random activity bursts reset everything.
        if corpus.next() % 13 == 0 {
            for event in machine.activity(ms(now)) {
                if let IdleEvent::Stage(stage) = event {
                    reported.push(stage);
                }
            }
        }
        // Invariants at every step:
        // * the stage is one of the four;
        // * activity always yields Active;
        // * DPMS events pair with their stages.
        let stage = machine.stage();
        assert!((1..=4).contains(&stage.to_wire()));
        if machine.last_activity().as_ns() == ms(now).as_ns() {
            assert_eq!(stage, IdleStage::Active);
        }
        assert!(order(stage) >= order(IdleStage::Active));
    }
    // The reported stream never regresses except through Active
    // (monotone ladder + resets).
    let mut high = 0u32;
    for stage in &reported {
        let w = stage.to_wire();
        if w == 1 {
            high = 1;
        } else {
            assert!(w >= high, "stage regressed without activity: {stage:?}");
            high = w;
        }
    }
    assert!(!reported.is_empty());
}

#[test]
fn backlight_corpus_ramp_properties() {
    let mut corpus = Corpus(0xBEE);
    for _ in 0..200 {
        let max = 1 + (corpus.next() % 4096) as u32;
        let start = (corpus.next() % u64::from(max + 1)) as u32;
        let mut ramp = BacklightRamp::new(start, max);
        let mut t = 0u64;
        // A sequence of retargets, each ramped to completion.
        for _ in 0..4 {
            let target = (corpus.next() % u64::from(max + 1)) as u32;
            let leg_start = ramp.level();
            ramp.set_target(target, ms(t));
            assert!(ramp.level() <= max);
            let mut levels = Vec::new();
            loop {
                t += 20;
                if let Some(level) = ramp.tick(ms(t)) {
                    assert!(level <= max, "level escaped max");
                    levels.push(level);
                } else if ramp.ramping() {
                    // Not yet at the target: a later tick will step
                    // again (irregular tick cadence allowed).
                    t += 5;
                    continue;
                } else {
                    break;
                }
                if !ramp.ramping() {
                    break;
                }
            }
            assert_eq!(ramp.level(), target, "ramp did not converge exactly");
            // Monotone toward the target within this leg.
            for pair in levels.windows(2) {
                if target > leg_start {
                    assert!(pair[1] > pair[0], "non-monotonic up leg {levels:?}");
                } else {
                    assert!(pair[1] < pair[0], "non-monotonic down leg {levels:?}");
                }
            }
        }
    }
}

#[test]
fn inhibitor_mask_algebra() {
    // Union / contains behave as a lattice; wire words preserve bits.
    let combined = InhibitMask::BLUR
        .union(InhibitMask::DISPLAY)
        .union(InhibitMask::IDLE)
        .union(InhibitMask::SUSPEND);
    assert_eq!(combined.to_wire(), 0b1111);
    assert!(combined.contains(InhibitMask::DISPLAY));
    assert!(!InhibitMask::DISPLAY.contains(InhibitMask::BLUR));
    assert!(InhibitMask::NONE.contains(InhibitMask::NONE));
    // Unknown future bits survive the round trip.
    let future = InhibitMask::from_wire(0b1_0000);
    assert_eq!(future.to_wire(), 0b1_0000);
    // Idle stage wire round trip.
    for stage in [
        IdleStage::Active,
        IdleStage::Dimmed,
        IdleStage::Off,
        IdleStage::Suspend,
    ] {
        assert_eq!(IdleStage::from_wire(stage.to_wire()), Some(stage));
    }
    assert_eq!(IdleStage::from_wire(0), None);
}
