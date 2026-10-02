//! Phase 7 exit criterion: event coalescing classes obeyed (§10.4).
//!
//! The scheduler's emissions flow through the class-aware queue under a
//! dispatch backlog: presentation feedback coalesces to the latest per
//! surface and kind, per-frame terminal signals survive, input never
//! drops and never reorders, and data never drops.
//!
//! Phase 39 adds the position-state doctrine: a pointer's absolute
//! motion sample and its frame terminator collapse to the freshest
//! value in the oldest pending slot, the discrete barrier seals the
//! pending sample, and delta streams deliver in full.

#[path = "sched/mod.rs"]
mod sched;

use ldp_compositor::coalesce::{Coalescible, CoalescingQueue, EventClass};
use ldp_compositor::scheduler::SchedEvent;
use ldp_compositor::SurfaceId;
use ldp_core::time::FrameDropReason;
use sched::{scenario_slow_client, scenario_stall_resync};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Wire {
    /// Input-class traffic (pointer motion, keys).
    Inp(u32),
    /// Data-class traffic (clipboard offers).
    Data(u32),
    /// Scheduler emission.
    Sched(SchedEvent),
    /// Position-state traffic: `(key, sample)` — the pointer's
    /// absolute motion sample or its frame terminator, keyed by the
    /// pointer object and kind.
    Pos(u64, u8, u32),
}

/// The pointer-motion kind tag.
const MOTION: u8 = 0;
/// The pointer-frame kind tag.
const FRAME: u8 = 1;

impl Coalescible for Wire {
    fn class(&self) -> EventClass {
        match *self {
            Wire::Inp(_) => EventClass::Input,
            Wire::Data(_) => EventClass::Data,
            Wire::Sched(_) => EventClass::Presentation,
            Wire::Pos(..) => EventClass::InputState,
        }
    }
    fn coalesce_key(&self) -> Option<ldp_compositor::CoalesceKey> {
        match *self {
            Wire::Sched(ref event) => event.coalesce_key(),
            Wire::Pos(object, kind, _) => Some(ldp_compositor::CoalesceKey::new(object, kind)),
            _ => None,
        }
    }
}

/// Drain the queue to a vector (delivery order).
fn drain_all(queue: &mut CoalescingQueue<Wire>) -> Vec<Wire> {
    let mut v = Vec::new();
    while let Some(item) = queue.pop() {
        v.push(item);
    }
    v
}

fn input_of(event: &SchedEvent) -> Option<u64> {
    match *event {
        SchedEvent::FrameDropped { frame, .. } => Some(frame),
        _ => None,
    }
}

#[test]
fn backlog_collapse_of_presentation_feedback() {
    // A full slow-client run pushed into an undrained queue: the only
    // surviving presentation items are the LATEST frame_target and the
    // LATEST presented for the surface — every drop survives.
    let scenario = scenario_slow_client(1);
    let mut queue: CoalescingQueue<Wire> = CoalescingQueue::new();
    for event in &scenario.outputs {
        queue.push(Wire::Sched(*event)).unwrap();
    }
    let targets = scenario
        .outputs
        .iter()
        .filter(|e| matches!(e, SchedEvent::FrameTarget { .. }))
        .count();
    let presented = scenario
        .outputs
        .iter()
        .filter(|e| matches!(e, SchedEvent::Presented { .. }))
        .count();
    let drops = scenario
        .outputs
        .iter()
        .filter(|e| matches!(e, SchedEvent::FrameDropped { .. }))
        .count();
    assert!(targets > 10 && presented > 10 && drops > 0);
    // One surface: exactly one target + one presented + all drops.
    assert_eq!(queue.len(), 2 + drops);

    // The surviving presentation events are the LATEST ones.
    let mut drained = Vec::new();
    while let Some(item) = queue.pop() {
        drained.push(item);
    }
    let last_target = scenario
        .outputs
        .iter()
        .rev()
        .find(|e| matches!(e, SchedEvent::FrameTarget { .. }))
        .copied();
    let last_presented = scenario
        .outputs
        .iter()
        .rev()
        .find(|e| matches!(e, SchedEvent::Presented { .. }))
        .copied();
    assert!(drained.contains(&Wire::Sched(last_target.unwrap())));
    assert!(drained.contains(&Wire::Sched(last_presented.unwrap())));
    // Every dropped frame's terminal signal survived, in order.
    let survived: Vec<u64> = drained
        .iter()
        .filter_map(|item| match *item {
            Wire::Sched(SchedEvent::FrameDropped { frame, .. }) => Some(frame),
            _ => None,
        })
        .collect();
    let expected: Vec<u64> = scenario.outputs.iter().filter_map(input_of).collect();
    assert_eq!(survived, expected);
}

#[test]
fn input_survives_and_never_reorders_under_pressure() {
    let scenario = scenario_stall_resync(2);
    let mut queue: CoalescingQueue<Wire> = CoalescingQueue::with_capacities([8, 64, 64, 512, 64]);
    let mut delivered: Vec<u32> = Vec::new();
    let mut pressure_events = 0usize;
    let mut sent = 0u32;
    for event in &scenario.outputs {
        // Two input events per scheduler emission. The dispatcher does
        // NOT drain continuously here: pressure builds, and every push
        // retries after delivering the oldest item — input is never
        // dropped, never reordered.
        for _ in 0..2 {
            let id = sent;
            sent += 1;
            while let Err(err) = queue.push(Wire::Inp(id)) {
                assert_eq!(err.class, EventClass::Input);
                pressure_events += 1;
                match queue.pop() {
                    Some(Wire::Inp(d)) => delivered.push(d),
                    Some(Wire::Sched(_) | Wire::Data(_) | Wire::Pos(..)) => {}
                    None => break,
                }
            }
        }
        queue.push(Wire::Sched(*event)).unwrap();
    }
    while let Some(item) = queue.pop() {
        if let Wire::Inp(d) = item {
            delivered.push(d);
        }
    }
    // Input never drops: every sent id was delivered exactly once.
    assert_eq!(delivered.len(), sent as usize);
    // Delivery order is exactly arrival order (FIFO, never reordered).
    let mut sorted = delivered.clone();
    sorted.sort_unstable();
    assert_eq!(delivered, sorted, "input reordered");
    // The pressure path actually fired.
    assert!(pressure_events > 0, "test never hit backpressure");
}

#[test]
fn data_never_drops() {
    let scenario = scenario_slow_client(3);
    let mut queue: CoalescingQueue<Wire> = CoalescingQueue::with_capacities([64, 4, 64, 512, 64]);
    let mut delivered: Vec<u32> = Vec::new();
    let mut pressure_events = 0usize;
    let mut sent = 0u32;
    for event in &scenario.outputs {
        for _ in 0..3 {
            let id = sent;
            sent += 1;
            // Data is small and rare; under pressure the dispatcher
            // blocks (delivers oldest, retries) — never drops.
            while let Err(err) = queue.push(Wire::Data(id)) {
                assert_eq!(err.class, EventClass::Data);
                pressure_events += 1;
                match queue.pop() {
                    Some(Wire::Data(d)) => delivered.push(d),
                    Some(Wire::Inp(_) | Wire::Sched(_) | Wire::Pos(..)) => {}
                    None => break,
                }
            }
        }
        queue.push(Wire::Sched(*event)).unwrap();
    }
    while let Some(item) = queue.pop() {
        if let Wire::Data(d) = item {
            delivered.push(d);
        }
    }
    assert!(pressure_events > 0, "test never hit data pressure");
    // Every data item delivered exactly once, FIFO.
    assert_eq!(delivered.len(), sent as usize);
    let mut sorted = delivered.clone();
    sorted.sort_unstable();
    assert_eq!(delivered, sorted);
}

#[test]
fn coalesced_events_keep_latest_position() {
    // Scheduler emissions interleaved with input: the surviving
    // presentation event keeps its LATEST arrival position (after the
    // input that was queued before it).
    let event_a = SchedEvent::FrameDropped {
        surface: SurfaceId::from_raw(1),
        frame: 1,
        reason: FrameDropReason::Superseded,
    };
    let event_b = SchedEvent::FrameDropped {
        surface: SurfaceId::from_raw(1),
        frame: 2,
        reason: FrameDropReason::DeadlineMissed,
    };
    let mut queue: CoalescingQueue<Wire> = CoalescingQueue::new();
    queue.push(Wire::Inp(1)).unwrap();
    queue.push(Wire::Sched(event_a)).unwrap();
    queue.push(Wire::Inp(2)).unwrap();
    // A second drop with a DIFFERENT frame still never replaces the
    // first (drops are unkeyed terminal signals).
    queue.push(Wire::Sched(event_b)).unwrap();
    let drained: Vec<Wire> = {
        let mut v = Vec::new();
        while let Some(item) = queue.pop() {
            v.push(item);
        }
        v
    };
    assert_eq!(
        drained,
        vec![
            Wire::Inp(1),
            Wire::Sched(event_a),
            Wire::Inp(2),
            Wire::Sched(event_b)
        ]
    );
}

// -- Phase 39: the position-state doctrine --------------------------------

/// One device batch as the router emits it: the absolute sample, the
/// delta stream, and the batch terminator.
fn batch(queue: &mut CoalescingQueue<Wire>, object: u64, sample: u32) {
    queue.push(Wire::Pos(object, MOTION, sample)).unwrap();
    queue.push(Wire::Inp(sample)).unwrap(); // the relative-motion delta
    queue.push(Wire::Pos(object, FRAME, sample)).unwrap();
}

#[test]
fn position_state_collapses_to_the_freshest_sample_in_its_slot() {
    // A 1000 Hz device parking sixteen batches between display frames
    // (the client drains once per frame): ONE motion carries the
    // freshest coordinates, at the FIRST batch's slot; every delta
    // delivers in full; ONE frame terminator survives.
    let mut queue: CoalescingQueue<Wire> = CoalescingQueue::new();
    for sample in 1..=16u32 {
        batch(&mut queue, 7, sample);
    }
    assert_eq!(queue.len_class(EventClass::InputState), 2);
    let delivered = drain_all(&mut queue);
    let motions: Vec<u32> = delivered
        .iter()
        .filter_map(|w| match *w {
            Wire::Pos(7, MOTION, v) => Some(v),
            _ => None,
        })
        .collect();
    let frames: Vec<u32> = delivered
        .iter()
        .filter_map(|w| match *w {
            Wire::Pos(7, FRAME, v) => Some(v),
            _ => None,
        })
        .collect();
    let deltas: Vec<u32> = delivered
        .iter()
        .filter_map(|w| match *w {
            Wire::Inp(v) => Some(v),
            _ => None,
        })
        .collect();
    // One motion, the FRESHEST sample's coordinates.
    assert_eq!(motions, vec![16]);
    // One frame terminator.
    assert_eq!(frames.len(), 1);
    // Every delta delivered, in arrival order (full fidelity).
    assert_eq!(deltas, (1..=16).collect::<Vec<u32>>());
    // The surviving motion occupies the FIRST batch's slot: it
    // delivers before the oldest delta (the slot never moves).
    assert!(matches!(delivered[0], Wire::Pos(7, MOTION, 16)));
    // The frame terminator rides its own first-slot position.
    assert!(matches!(delivered[2], Wire::Pos(7, FRAME, _)));
}

#[test]
fn the_discrete_barrier_seals_the_pending_sample() {
    // Motion, a button, then more motion: the click's context is the
    // sample it rode with (frozen at the barrier — the X11
    // flush-before-button doctrine), and the post-click samples queue
    // fresh slots after it.
    let mut queue: CoalescingQueue<Wire> = CoalescingQueue::new();
    batch(&mut queue, 7, 1);
    // The button: discrete input, never dropped — and it seals the
    // object's position state (what the served outbox does at the
    // discrete barrier).
    queue.seal(
        EventClass::InputState,
        ldp_compositor::CoalesceKey::new(7, MOTION),
    );
    queue.seal(
        EventClass::InputState,
        ldp_compositor::CoalesceKey::new(7, FRAME),
    );
    queue.push(Wire::Inp(99)).unwrap(); // the click
    batch(&mut queue, 7, 2);
    let delivered = drain_all(&mut queue);
    assert_eq!(
        delivered,
        vec![
            Wire::Pos(7, MOTION, 1), // frozen: the click's own sample
            Wire::Inp(1),
            Wire::Pos(7, FRAME, 1),
            Wire::Inp(99),           // the click, after its sample's batch
            Wire::Pos(7, MOTION, 2), // fresh slot, after the click
            Wire::Inp(2),
            Wire::Pos(7, FRAME, 2),
        ]
    );
}

#[test]
fn position_state_never_overtakes_queued_input() {
    // A sample parked, discrete input queued after it, a FRESHER sample
    // arrives: the newer value overwrites the older sample's slot —
    // the discrete input's delivery position never moves.
    let mut queue: CoalescingQueue<Wire> = CoalescingQueue::new();
    queue.push(Wire::Pos(7, MOTION, 1)).unwrap();
    queue.push(Wire::Inp(42)).unwrap();
    queue.push(Wire::Pos(7, MOTION, 2)).unwrap();
    assert_eq!(
        drain_all(&mut queue),
        vec![Wire::Pos(7, MOTION, 2), Wire::Inp(42)]
    );
}

#[test]
fn position_state_keeps_its_slot_across_classes() {
    // The feedback classes keep the LATEST arrival position (§10.4);
    // position state keeps its FIRST slot — the two replacement
    // doctrines side by side.
    let mut queue: CoalescingQueue<Wire> = CoalescingQueue::new();
    let presented = |frame: u64| SchedEvent::Presented {
        surface: SurfaceId::from_raw(1),
        timing: ldp_core::time::PresentationTiming {
            frame,
            presented_at: ldp_core::time::Mono::from_ns(1_000_000 * frame),
            refresh: ldp_core::time::RefreshInterval::from_ns(16_666_666).unwrap(),
            flags: ldp_core::time::PresentationFlags::default(),
        },
    };
    queue.push(Wire::Pos(7, MOTION, 1)).unwrap();
    queue.push(Wire::Sched(presented(1))).unwrap();
    queue.push(Wire::Pos(7, MOTION, 2)).unwrap();
    queue.push(Wire::Sched(presented(2))).unwrap();
    let delivered = drain_all(&mut queue);
    // The motion stays at its FIRST slot (before both feedback
    // events); the presented pair coalesced to the LATEST, at the
    // LATEST's arrival position (after the first feedback slot).
    assert_eq!(
        delivered,
        vec![Wire::Pos(7, MOTION, 2), Wire::Sched(presented(2)),]
    );
}

#[test]
fn sustained_position_churn_allocates_no_new_slots() {
    // The memory-bound proof in its strongest form: in-place overwrite
    // allocates nothing — ten thousand undelivered samples of the same
    // key occupy ONE slot.
    let mut queue: CoalescingQueue<Wire> = CoalescingQueue::new();
    for sample in 0..10_000u32 {
        queue.push(Wire::Pos(7, MOTION, sample)).unwrap();
    }
    assert_eq!(queue.slots_allocated(), 1);
    assert_eq!(queue.len(), 1);
    assert!(matches!(
        drain_all(&mut queue).as_slice(),
        [Wire::Pos(7, MOTION, 9_999)]
    ));
}

#[test]
fn position_state_at_capacity_evicts_oldest_never_blocks() {
    // Distinct keys beyond the class capacity: the oldest position
    // state is expendable under pressure (a superseded sample of
    // another pointer) — never backpressure, never a discrete loss.
    let mut queue: CoalescingQueue<Wire> = CoalescingQueue::with_capacities([8, 8, 8, 8, 2]);
    queue.push(Wire::Pos(1, MOTION, 10)).unwrap();
    queue.push(Wire::Pos(2, MOTION, 20)).unwrap();
    queue.push(Wire::Pos(3, MOTION, 30)).unwrap();
    assert_eq!(queue.len(), 2);
    let delivered = drain_all(&mut queue);
    // The oldest (object 1) was evicted; the two newest survive.
    assert_eq!(
        delivered,
        vec![Wire::Pos(2, MOTION, 20), Wire::Pos(3, MOTION, 30)]
    );
}

#[test]
fn sealing_absent_keys_is_a_noop() {
    let mut queue: CoalescingQueue<Wire> = CoalescingQueue::new();
    queue.seal(
        EventClass::InputState,
        ldp_compositor::CoalesceKey::new(9, MOTION),
    );
    queue.seal(
        EventClass::Input,
        ldp_compositor::CoalesceKey::new(7, MOTION),
    );
    assert!(queue.is_empty());
    // And a sealed key that never existed just starts fresh.
    queue.push(Wire::Pos(9, MOTION, 1)).unwrap();
    assert_eq!(drain_all(&mut queue), vec![Wire::Pos(9, MOTION, 1)]);
}

/// Phase 43: the per-contact position-state key. Touch and tablet
/// streams coalesce per contact — the pointer's doctrine extended so
/// that two contacts sharing one target object each keep their own
/// slot, and the discrete barrier seals one contact without touching
/// another's.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TouchWire {
    /// A motion sample of `(contact, sample)`.
    Motion(u64, u32),
    /// A discrete event of `contact` (touch.down).
    Down(u64),
}

impl Coalescible for TouchWire {
    fn class(&self) -> EventClass {
        match *self {
            TouchWire::Motion(..) => EventClass::InputState,
            TouchWire::Down(_) => EventClass::Input,
        }
    }
    fn coalesce_key(&self) -> Option<ldp_compositor::CoalesceKey> {
        match *self {
            // The per-contact key: the contact id rides the `sub`
            // discriminator — two contacts never share a slot.
            TouchWire::Motion(contact, _) => {
                Some(ldp_compositor::CoalesceKey::contact(10, MOTION, contact))
            }
            TouchWire::Down(_) => None,
        }
    }
}

#[test]
fn per_contact_keys_collapse_independently_and_keep_their_slots() {
    let drain = |q: &mut CoalescingQueue<TouchWire>| -> Vec<TouchWire> {
        let mut v = Vec::new();
        while let Some(item) = q.pop() {
            v.push(item);
        }
        v
    };
    let mut q = CoalescingQueue::new();
    // Two contacts, interleaved: each collapses to its own freshest
    // sample in the slot its first sample took.
    for sample in 1..=16u32 {
        q.push(TouchWire::Motion(1, sample)).unwrap();
        q.push(TouchWire::Motion(2, 1000 + sample)).unwrap();
    }
    assert_eq!(q.len(), 2);
    assert_eq!(
        drain(&mut q),
        vec![TouchWire::Motion(1, 16), TouchWire::Motion(2, 1016)]
    );

    // The discrete barrier, per contact: a `Down` of contact 1 seals
    // contact 1's pending sample; contact 2's stays replaceable, and
    // a post-down contact-1 motion queues after the barrier.
    q.push(TouchWire::Motion(1, 1)).unwrap();
    q.push(TouchWire::Motion(2, 2)).unwrap();
    q.seal(
        EventClass::InputState,
        ldp_compositor::CoalesceKey::contact(10, MOTION, 1),
    );
    q.push(TouchWire::Down(1)).unwrap();
    q.push(TouchWire::Motion(2, 3)).unwrap(); // contact 2: still replaceable
    q.push(TouchWire::Motion(1, 9)).unwrap(); // contact 1: fresh slot after the barrier
    assert_eq!(
        drain(&mut q),
        vec![
            TouchWire::Motion(1, 1), // sealed at its sample, not replaced by 9
            TouchWire::Motion(2, 3), // replaced in its own slot
            TouchWire::Down(1),
            TouchWire::Motion(1, 9), // post-barrier fresh slot
        ]
    );

    // The sub discriminator is part of key identity: same group and
    // kind, different contact — different keys.
    let a = ldp_compositor::CoalesceKey::contact(10, MOTION, 1);
    let b = ldp_compositor::CoalesceKey::contact(10, MOTION, 2);
    assert_ne!(a, b);
    // ... and the single-stream spelling stays the contact-0 case:
    // every pre-Phase-43 key keeps its exact meaning.
    assert_eq!(ldp_compositor::CoalesceKey::new(10, MOTION).sub, 0);
}
