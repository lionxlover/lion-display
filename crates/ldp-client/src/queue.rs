//! The class-lane event queue and the dispatch scheduler.
//!
//! This is the Phase-5 engine behind the architecture's latency
//! guarantee (*"a burst of `frame_target` events can never delay
//! pointer motion beyond one dispatch cycle"*). Events arrive in wire
//! order ([`EventQueue::push`]); they are *dispatched* by lane:
//!
//! * every lane is FIFO within itself,
//! * the scheduler always drains [`EventClass::Input`] first, then [`EventClass::Data`],
//!   then barrier-ready [`EventClass::Control`], then [`EventClass::Configuration`], then
//!   [`EventClass::Presentation`],
//! * a [`EventClass::Control`] event is a **barrier**: it is dispatchable only
//!   once no earlier-arrived event remains queued in *any* lane —
//!   exactly the `connection.sync_done` contract ("all events queued
//!   before the matching sync have been delivered") and the safest
//!   reading of the lifecycle events (`destroyed`, `revoked`,
//!   `global_remove`'s revoked-before rule).
//!
//! What this deliberately does *not* preserve is strict FIFO across
//! lanes: a configuration event may overtake a *later* presentation
//! event, and an input event overtakes everything that arrived after
//! it. Those reorderings are the feature. The two invariants that hold
//! absolutely:
//!
//! 1. **No starvation:** an input event is dispatched before every
//!    non-input event that arrived after it — whatever the backlog.
//! 2. **Barriers see the whole past:** a control event is dispatched
//!    only after every event that arrived before it.
//!
//! Progress is structural: the globally earliest queued event is
//! always dispatchable (it sits at some lane's head, and if it is a
//! barrier, nothing earlier exists) — the scheduler can never wedge.

use ldp_core::ids::ObjectId;
use ldp_core::wire::Value;
use ldp_protocol::OpSchema;
use ldp_transport::FdList;
use std::collections::VecDeque;

use crate::class::EventClass;

/// One dispatchable event: stage-3-validated, classified, carrying its
/// own FD table.
///
/// FD arguments are indices into `fds` exactly as on the wire; untaken
/// FDs close when the event is dropped (the same discipline the server
/// applies to dispatched requests).
#[derive(Debug)]
pub struct Event {
    /// Arrival order (monotonic per connection). Barriers reference it.
    pub seq: u64,
    /// The object the event was emitted on.
    pub target: ObjectId,
    /// The target's interface (resolved from the client proxy table).
    pub interface: &'static str,
    /// The target's pinned version.
    pub version: u32,
    /// The matched event signature.
    pub op: &'static OpSchema,
    /// Decoded arguments, in order.
    pub args: Vec<Value>,
    /// The message's ancillary FDs (owned; closed on drop if untaken).
    pub fds: FdList,
    /// The dispatch lane this event was classified into.
    pub class: EventClass,
    /// The envelope's `URGENT` hint (informational; input-class events
    /// carry it on the wire, and the lane scheduler honors the class
    /// regardless).
    pub urgent: bool,
}

impl Event {
    /// Look up one argument by name.
    #[must_use]
    pub fn arg(&self, name: &str) -> Option<&Value> {
        let i = self.op.args.iter().position(|a| a.name == name)?;
        self.args.get(i)
    }

    /// Take ownership of FD `index` of this event's table (`None` when
    /// the index is out of range — the wire's fd arguments always are,
    /// so a miss is a handler bug).
    pub fn take_fd(&mut self, index: u32) -> Option<std::os::fd::OwnedFd> {
        self.fds.remove(index as usize)
    }
}

/// The five dispatch lanes.
#[derive(Debug, Default)]
struct Lanes {
    input: VecDeque<Event>,
    data: VecDeque<Event>,
    control: VecDeque<Event>,
    configuration: VecDeque<Event>,
    presentation: VecDeque<Event>,
}

impl Lanes {
    fn lane_mut(&mut self, class: EventClass) -> &mut VecDeque<Event> {
        match class {
            EventClass::Input => &mut self.input,
            EventClass::Data => &mut self.data,
            EventClass::Control => &mut self.control,
            EventClass::Configuration => &mut self.configuration,
            EventClass::Presentation => &mut self.presentation,
        }
    }

    fn lane(&self, class: EventClass) -> &VecDeque<Event> {
        match class {
            EventClass::Input => &self.input,
            EventClass::Data => &self.data,
            EventClass::Control => &self.control,
            EventClass::Configuration => &self.configuration,
            EventClass::Presentation => &self.presentation,
        }
    }

    /// The smallest arrival sequence sitting at any lane head.
    fn min_head_seq(&self, skip: EventClass) -> Option<u64> {
        [
            EventClass::Input,
            EventClass::Data,
            EventClass::Control,
            EventClass::Configuration,
            EventClass::Presentation,
        ]
        .into_iter()
        .filter(|c| *c != skip)
        .filter_map(|c| self.lane(c).front().map(|e| e.seq))
        .min()
    }
}

/// The class-lane queue for one connection.
#[derive(Debug, Default)]
pub struct EventQueue {
    lanes: Lanes,
    next_seq: u64,
    pushed: u64,
    dispatched: u64,
}

impl EventQueue {
    /// An empty queue.
    #[must_use]
    pub const fn new() -> EventQueue {
        EventQueue {
            lanes: Lanes {
                input: VecDeque::new(),
                data: VecDeque::new(),
                control: VecDeque::new(),
                configuration: VecDeque::new(),
                presentation: VecDeque::new(),
            },
            next_seq: 1,
            pushed: 0,
            dispatched: 0,
        }
    }

    /// The sequence number the next push will assign.
    #[must_use]
    pub const fn next_seq(&self) -> u64 {
        self.next_seq
    }

    /// Total events pushed over this queue's lifetime.
    #[must_use]
    pub const fn pushed(&self) -> u64 {
        self.pushed
    }

    /// Total events dispatched over this queue's lifetime.
    #[must_use]
    pub const fn dispatched(&self) -> u64 {
        self.dispatched
    }

    /// Events currently queued, per class.
    #[must_use]
    pub fn len(&self, class: EventClass) -> usize {
        self.lanes.lane(class).len()
    }

    /// Whether any event is queued in any lane.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        [
            EventClass::Input,
            EventClass::Data,
            EventClass::Control,
            EventClass::Configuration,
            EventClass::Presentation,
        ]
        .into_iter()
        .all(|c| self.lanes.lane(c).is_empty())
    }

    /// Enqueue one event in its class's lane, assigning it the next
    /// arrival sequence.
    pub fn push(&mut self, mut event: Event) {
        event.seq = self.next_seq;
        self.next_seq += 1;
        self.pushed += 1;
        self.lanes.lane_mut(event.class).push_back(event);
    }

    /// Pop the next dispatchable event, or `None` when the queue is at
    /// a fixed point.
    ///
    /// Selection: input lane head, then data, then a barrier-ready
    /// control head, then configuration, then presentation.
    #[must_use]
    pub fn pop_next(&mut self) -> Option<Event> {
        if !self.lanes.input.is_empty() {
            self.dispatched += 1;
            return self.lanes.input.pop_front();
        }
        if !self.lanes.data.is_empty() {
            self.dispatched += 1;
            return self.lanes.data.pop_front();
        }
        if let Some(head) = self.lanes.control.front() {
            let seq = head.seq;
            let ready = self
                .lanes
                .min_head_seq(EventClass::Control)
                .map_or(true, |m| m > seq);
            if ready {
                self.dispatched += 1;
                return self.lanes.control.pop_front();
            }
        }
        if !self.lanes.configuration.is_empty() {
            self.dispatched += 1;
            return self.lanes.configuration.pop_front();
        }
        if !self.lanes.presentation.is_empty() {
            self.dispatched += 1;
            return self.lanes.presentation.pop_front();
        }
        None
    }

    /// Pop up to `budget` dispatchable events (bounded-batch dispatch;
    /// the starvation guarantee holds for any budget >= 1 because lane
    /// priority, not batch size, is what bounds input latency).
    pub fn pop_with_budget(&mut self, budget: usize) -> Vec<Event> {
        let mut out = Vec::with_capacity(budget.min(64));
        for _ in 0..budget {
            match self.pop_next() {
                Some(e) => out.push(e),
                None => break,
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal stand-in event signature for queue-level tests: the
    /// scheduler only reads `seq`/`class`, never the schema.
    static OP: OpSchema = OpSchema {
        name: "test",
        opcode: 1,
        since: 1,
        doc: "",
        reply: None,
        args: &[],
    };

    fn event(class: EventClass) -> Event {
        Event {
            seq: 0, // assigned by push
            target: ObjectId::CONNECTION,
            interface: "ldp.core.connection",
            version: 1,
            op: &OP,
            args: Vec::new(),
            fds: FdList::new(),
            class,
            urgent: false,
        }
    }

    fn drain(q: &mut EventQueue) -> Vec<(EventClass, u64)> {
        let mut out = Vec::new();
        while let Some(e) = q.pop_next() {
            out.push((e.class, e.seq));
        }
        out
    }

    #[test]
    fn input_beats_a_presentation_flood() {
        let mut q = EventQueue::new();
        for _ in 0..100_000 {
            q.push(event(EventClass::Presentation));
        }
        q.push(event(EventClass::Input));
        for _ in 0..50_000 {
            q.push(event(EventClass::Presentation));
        }
        let first = q.pop_next().unwrap();
        assert_eq!(first.class, EventClass::Input, "input must jump the flood");
        assert_eq!(q.dispatched(), 1);
    }

    #[test]
    fn lanes_are_fifo_within_themselves() {
        let mut q = EventQueue::new();
        for _ in 0..10 {
            q.push(event(EventClass::Presentation));
            q.push(event(EventClass::Presentation));
            q.push(event(EventClass::Input));
            q.push(event(EventClass::Configuration));
        }
        let out = drain(&mut q);
        let input: Vec<u64> = out
            .iter()
            .filter(|(c, _)| *c == EventClass::Input)
            .map(|(_, s)| *s)
            .collect();
        let pres: Vec<u64> = out
            .iter()
            .filter(|(c, _)| *c == EventClass::Presentation)
            .map(|(_, s)| *s)
            .collect();
        assert!(input.windows(2).all(|w| w[0] < w[1]));
        assert!(pres.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn barriers_wait_for_the_whole_past() {
        let mut q = EventQueue::new();
        // Arrival: presentation(1), input(2), barrier(3), presentation(4).
        q.push(event(EventClass::Presentation));
        q.push(event(EventClass::Input));
        q.push(event(EventClass::Control));
        q.push(event(EventClass::Presentation));
        let out = drain(&mut q);
        // The barrier (seq 3) must dispatch after 1 and 2. Event 4 may
        // overtake it (it arrived later) but must not beat 1 or 2's
        // class ordering constraints.
        let pos = |s: u64| out.iter().position(|(_, seq)| *seq == s).unwrap();
        assert!(pos(3) > pos(1), "barrier after earlier presentation");
        assert!(pos(3) > pos(2), "barrier after earlier input");
    }

    #[test]
    fn barrier_does_not_block_on_later_events() {
        let mut q = EventQueue::new();
        q.push(event(EventClass::Control)); // seq 1
        for _ in 0..1000 {
            q.push(event(EventClass::Presentation)); // seq 2..=1001
        }
        let out = drain(&mut q);
        assert_eq!(out[0], (EventClass::Control, 1));
    }

    #[test]
    fn budget_limits_each_batch() {
        let mut q = EventQueue::new();
        for _ in 0..10 {
            q.push(event(EventClass::Presentation));
        }
        assert_eq!(q.pop_with_budget(4).len(), 4);
        assert_eq!(q.pop_with_budget(0).len(), 0);
        assert_eq!(q.len(EventClass::Presentation), 6);
    }

    #[test]
    fn every_push_makes_progress_eventually() {
        // Randomized fixed-point check: pushing then draining always
        // drains everything (no wedge state exists).
        let mut q = EventQueue::new();
        let mut seed: u64 = 0x1234_5678;
        let classes = [
            EventClass::Input,
            EventClass::Data,
            EventClass::Control,
            EventClass::Configuration,
            EventClass::Presentation,
        ];
        for _ in 0..5000 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let class = classes[(seed % 5) as usize];
            q.push(event(class));
            if seed % 3 == 0 {
                let _ = q.pop_next();
            }
        }
        assert!(!q.is_empty());
        let before = q.dispatched();
        let drained = drain(&mut q);
        assert!(!drained.is_empty());
        assert!(q.is_empty());
        assert_eq!(q.dispatched(), before + drained.len() as u64);
        assert_eq!(q.dispatched(), q.pushed());
    }

    #[test]
    fn counters_track_pushes_and_dispatches() {
        let mut q = EventQueue::new();
        q.push(event(EventClass::Input));
        q.push(event(EventClass::Input));
        q.pop_next().unwrap();
        assert_eq!(q.pushed(), 2);
        assert_eq!(q.dispatched(), 1);
        assert_eq!(q.next_seq(), 3);
    }
}
