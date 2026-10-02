//! Exit criterion 2 (roadmap Phase 5): event class latency — input
//! never starves under presentation floods.
//!
//! Two layers:
//!
//! * **Property suite** (pure queue scheduler): seeded randomized
//!   interleavings of all five classes with barriers sprinkled in;
//!   asserts the four invariants — input-beats-later-presentation,
//!   barriers-see-the-whole-past, FIFO-within-class, and progress —
//!   across thousands of events and many budgets.
//! * **Live flood**: a scripted `ldp-server` dispatcher buries one
//!   pointer motion inside 27 000 presentation events; the real client
//!   (handshake, proxies, pump, dispatch) reads the ENTIRE flood into
//!   its queue and then dispatches in one batch — the lane scheduler
//!   must surface the motion first and close with the sync barrier.

mod common;

use common::{factories, FloodDispatcher, FloodPlan, ServerHarness};
use ldp_client::{Connection, Event, EventClass, EventHandler, EventQueue};
use ldp_core::wire::Value;
use ldp_server::ServerConfig;

// ---------------------------------------------------------------------------
// Property suite (pure scheduler)
// ---------------------------------------------------------------------------

/// A seeded xorshift64 — deterministic, dependency-free.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed.max(1))
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// A minimal event signature for scheduler-level tests.
static OP: ldp_protocol::OpSchema = ldp_protocol::OpSchema {
    name: "prop",
    opcode: 1,
    since: 1,
    doc: "",
    reply: None,
    args: &[],
};

fn synthetic(class: EventClass) -> Event {
    Event {
        seq: 0,
        target: ldp_core::ids::ObjectId::CONNECTION,
        interface: "ldp.core.connection",
        version: 1,
        op: &OP,
        args: Vec::new(),
        fds: ldp_transport::FdList::new(),
        class,
        urgent: false,
    }
}

const CLASSES: [EventClass; 5] = [
    EventClass::Input,
    EventClass::Data,
    EventClass::Control,
    EventClass::Configuration,
    EventClass::Presentation,
];

/// One randomized trial: push `n` events of weighted-random classes,
/// dispatch with budget `budget`, verify all four invariants.
fn trial(seed: u64, n: usize, budget: usize) {
    let mut rng = Rng::new(seed);
    let mut queue = EventQueue::new();

    // Class weights: presentation-heavy (the flood regime), with
    // barriers and input sprinkled in.
    let weights: [(EventClass, u64); 5] = [
        (EventClass::Presentation, 70),
        (EventClass::Configuration, 10),
        (EventClass::Control, 8),
        (EventClass::Data, 4),
        (EventClass::Input, 8),
    ];
    let total: u64 = weights.iter().map(|(_, w)| w).sum();
    let mut pushed: Vec<EventClass> = Vec::with_capacity(n);
    for _ in 0..n {
        let pick = rng.below(total);
        let mut acc = 0u64;
        let mut chosen = EventClass::Presentation;
        for (class, w) in weights {
            acc += w;
            if pick < acc {
                chosen = class;
                break;
            }
        }
        pushed.push(chosen);
        queue.push(synthetic(chosen));
    }

    // Dispatch in bounded batches, recording (class, seq) order.
    let mut order: Vec<(EventClass, u64)> = Vec::with_capacity(n);
    loop {
        let batch = queue.pop_with_budget(budget);
        if batch.is_empty() {
            break;
        }
        for e in batch {
            order.push((e.class, e.seq));
        }
    }
    assert_eq!(order.len(), n, "the queue must drain completely");

    // Invariant 1: input beats every later-arrived event (the EC's
    // starvation property). Events that arrived *earlier* may trail the
    // input (that is lane priority doing its job) — but nothing that
    // arrived after the input may be dispatched before it.
    for (i, (class_i, seq_i)) in order.iter().enumerate() {
        if *class_i != EventClass::Input {
            continue;
        }
        for (j, (class_j, seq_j)) in order.iter().enumerate() {
            assert!(
                !(j < i && *seq_j > *seq_i),
                "seed {seed}: input event (seq {seq_i}) dispatched at {i} \
                 AFTER-arriving event (class {class_j:?}, seq {seq_j}) dispatched at {j}"
            );
        }
    }

    // Invariant 2: barriers see the whole past — every earlier-arrived
    // event is dispatched before the barrier (later arrivals may
    // overtake it; that is not a barrier violation).
    for (i, (class_i, seq_i)) in order.iter().enumerate() {
        if !class_i.is_barrier() {
            continue;
        }
        for (j, (_, seq_j)) in order.iter().enumerate() {
            assert!(
                !(j > i && *seq_j < *seq_i),
                "seed {seed}: barrier (seq {seq_i}) dispatched at {i} \
                 before earlier event (seq {seq_j}) dispatched at {j}"
            );
        }
    }

    // Invariant 3: FIFO within every class.
    for class in CLASSES {
        let seqs: Vec<u64> = order
            .iter()
            .filter(|(c, _)| *c == class)
            .map(|(_, s)| *s)
            .collect();
        assert!(
            seqs.windows(2).all(|w| w[0] < w[1]),
            "seed {seed}: class {class:?} dispatched out of FIFO order"
        );
    }

    // Invariant 4 (progress): drained fully (asserted by len above).
}

#[test]
fn input_latency_holds_under_randomized_floods() {
    // Seeds, lengths, budgets: enough randomized trials to make a lane
    // bug vanishingly unlikely, cheap enough for CI.
    let seeds: [u64; 12] = [
        1,
        2,
        3,
        4,
        5,
        6,
        7,
        8,
        0xDEAD,
        0xBEEF,
        0x00C0_FFEE,
        0x1234_5678,
    ];
    for (i, seed) in seeds.iter().enumerate() {
        let n = 400 + i * 350; // 400..4245 events per trial
        trial(*seed, n, 1 + (i % 5)); // budgets 1..5
    }
}

#[test]
fn single_input_jumps_a_hundred_thousand_presentation_events() {
    let mut queue = EventQueue::new();
    for _ in 0..100_000 {
        queue.push(synthetic(EventClass::Presentation));
    }
    queue.push(synthetic(EventClass::Input));
    for _ in 0..100_000 {
        queue.push(synthetic(EventClass::Presentation));
    }
    // The very first dispatch call (budget 1!) must return the input
    // event — a backlog of ANY size cannot bury it.
    let first = queue.pop_with_budget(1);
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].class, EventClass::Input);
}

#[test]
fn barrier_does_not_wedge_behind_lanes() {
    // A control event between floods: it must not block later events
    // from dispatching, and must itself dispatch after its own past.
    let mut queue = EventQueue::new();
    queue.push(synthetic(EventClass::Presentation));
    queue.push(synthetic(EventClass::Input));
    queue.push(synthetic(EventClass::Control)); // barrier, seq 3
    for _ in 0..10_000 {
        queue.push(synthetic(EventClass::Presentation));
    }
    let mut order = Vec::new();
    while let Some(e) = queue.pop_next() {
        order.push((e.class, e.seq));
    }
    let pos = |s: u64| order.iter().position(|(_, seq)| *seq == s).unwrap();
    assert!(pos(3) > pos(1) && pos(3) > pos(2), "barrier after its past");
    assert_eq!(order.len(), 10_003);
}

#[test]
fn budgets_do_not_change_the_dispatch_set() {
    // Budget affects batching, never membership or the invariants.
    let mut rng = Rng::new(99);
    let mut classes = Vec::new();
    for _ in 0..2000 {
        classes.push(CLASSES[rng.below(5) as usize]);
    }
    let mut q1 = EventQueue::new();
    let mut q2 = EventQueue::new();
    for &c in &classes {
        q1.push(synthetic(c));
        q2.push(synthetic(c));
    }
    let mut a = Vec::new();
    while let Some(e) = q1.pop_next() {
        a.push((e.class, e.seq));
    }
    let mut b = Vec::new();
    loop {
        let batch = q2.pop_with_budget(3);
        if batch.is_empty() {
            break;
        }
        b.extend(batch.into_iter().map(|e| (e.class, e.seq)));
    }
    assert_eq!(a, b, "identical schedules under different budgets");
}

// ---------------------------------------------------------------------------
// Live flood through ldp-server (the full client stack)
// ---------------------------------------------------------------------------

/// Records `(op, frame-or-0)` in dispatch order for the live flood.
struct FloodRecorder {
    order: Vec<(&'static str, u64)>,
    motions: usize,
}

impl FloodRecorder {
    fn new() -> FloodRecorder {
        FloodRecorder {
            order: Vec::new(),
            motions: 0,
        }
    }
}

impl EventHandler for FloodRecorder {
    fn on_event(&mut self, event: &Event) -> ldp_core::error::Result<()> {
        let id = match (event.interface, event.op.name) {
            ("ldp.core.surface", "frame_target" | "presented") => {
                let Value::Uint64(frame) = event.args[0] else {
                    unreachable!("stage 3 guarantees the frame shape");
                };
                frame
            }
            ("ldp.input.pointer", "motion") => {
                self.motions += 1;
                u64::MAX // sentinel: identifiable in the order log
            }
            _ => 0,
        };
        self.order.push((event.op.name, id));
        Ok(())
    }
}

#[test]
fn live_presentation_flood_cannot_starve_pointer_motion() {
    const PRE: u32 = 12_000;
    const POST: u32 = 12_000;
    let plan = FloodPlan {
        pre_motion: PRE,
        post_motion: POST,
        post_presented: 3_000,
        post_config: 8,
    };
    let config = ServerConfig {
        globals: factories(),
        ..ServerConfig::default()
    };
    let mut server = ServerHarness::start_with("live-flood", config, FloodDispatcher::new(plan));
    let mut conn = Connection::connect(&server.addr).unwrap();

    // Build the session: compositor + seat, one surface, one pointer.
    let compositor = conn.bind("ldp.core.compositor").unwrap();
    let seat = conn.bind("ldp.input.seat").unwrap();
    let surface = conn
        .create_object(&compositor, "create_surface", vec![])
        .unwrap();
    let _pointer = conn.create_object(&seat, "get_pointer", vec![]).unwrap();

    // Flush the setup traffic (global replay, bound confirmations) so
    // the flood round-trip below counts exactly the flood.
    conn.roundtrip(&mut FloodRecorder::new()).unwrap();

    // One commit unleashes the flood (frame_target x PRE, motion,
    // frame_target x POST, presented x 3000, preferred_scale x 8,
    // committed); the trailing sync barriers the whole batch.
    conn.send_request(&surface, "commit", vec![Value::Uint32(7)])
        .unwrap();
    conn.send_request(&conn.connection_proxy(), "sync", vec![Value::Uint32(99)])
        .unwrap();

    // Read the entire flood WITHOUT dispatching: the queue builds the
    // real backlog the lane scheduler exists for (this is the regime a
    // busy main loop produces when it cannot keep up with the socket).
    let expected = usize::try_from(plan.total()).unwrap() + 2;
    for _ in 0..expected {
        conn.pump_one().unwrap();
    }
    let mut recorder = FloodRecorder::new();
    let dispatched = conn.dispatch_pending(&mut recorder).unwrap();
    assert_eq!(dispatched, expected);

    assert_eq!(recorder.motions, 1, "exactly one motion event");
    // Flood + committed + sync_done.
    assert_eq!(
        recorder.order.len(),
        usize::try_from(plan.total()).unwrap() + 2
    );

    // Under lane priority the motion is the FIRST event dispatched —
    // a backlog of 27 000 presentation events cannot bury it at all.
    let motion_pos = recorder
        .order
        .iter()
        .position(|(_, id)| *id == u64::MAX)
        .expect("the motion must have been dispatched");
    assert_eq!(motion_pos, 0, "the pointer motion jumps the entire flood");
    let after_motion = recorder
        .order
        .iter()
        .filter(|(op, _)| matches!(*op, "frame_target" | "presented"))
        .count();
    // Every presentation event (before AND after the motion in arrival
    // order) dispatched behind it.
    assert_eq!(
        after_motion,
        usize::try_from(PRE + POST + plan.post_presented).unwrap(),
        "all presentation events dispatch behind the motion"
    );

    // The barrier: sync_done dispatched last, after the whole flood.
    assert_eq!(recorder.order.last().unwrap().0, "sync_done");

    drop(conn);
    assert!(server.wait_live_zero(std::time::Duration::from_secs(10)));
    server.stop();
}
