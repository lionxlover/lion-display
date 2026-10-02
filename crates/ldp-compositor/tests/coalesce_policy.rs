//! Coalescing-queue policy tests (§10.4).
//!
//! Synthetic items over the four dispatch classes pin the exact queue
//! policies: FIFO across classes, same-key replacement for the
//! coalescing classes, kind-scoped keys, backpressure for the
//! never-drop classes, oldest-first eviction under class pressure,
//! tombstone bookkeeping, and compaction (observed through the
//! `slots_allocated` diagnostic).

use ldp_compositor::coalesce::{CoalesceKey, Coalescible, CoalescingQueue, EventClass, QueueFull};
use ldp_compositor::scheduler::SchedEvent;
use ldp_compositor::SurfaceId;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Item {
    class: EventClass,
    key: Option<CoalesceKey>,
    id: u32,
}

impl Coalescible for Item {
    fn class(&self) -> EventClass {
        self.class
    }
    fn coalesce_key(&self) -> Option<CoalesceKey> {
        self.key
    }
}

fn input(id: u32) -> Item {
    Item {
        class: EventClass::Input,
        key: None,
        id,
    }
}

fn pres(group: u64, id: u32) -> Item {
    Item {
        class: EventClass::Presentation,
        key: Some(CoalesceKey::new(group, 0)),
        id,
    }
}

fn cfg(group: u64, id: u32) -> Item {
    Item {
        class: EventClass::Configuration,
        key: Some(CoalesceKey::new(group, 0)),
        id,
    }
}

fn data(id: u32) -> Item {
    Item {
        class: EventClass::Data,
        key: None,
        id,
    }
}

fn drain_all(q: &mut CoalescingQueue<Item>) -> Vec<u32> {
    let mut ids = Vec::new();
    while let Some(item) = q.pop() {
        ids.push(item.id);
    }
    ids
}

#[test]
fn fifo_order_preserved_across_classes() {
    let mut q = CoalescingQueue::new();
    for item in [input(1), pres(7, 2), cfg(3, 3), data(4), input(5)] {
        q.push(item).unwrap();
    }
    assert_eq!(drain_all(&mut q), vec![1, 2, 3, 4, 5]);
}

#[test]
fn presentation_coalesces_to_latest_per_key() {
    let mut q = CoalescingQueue::new();
    q.push(pres(7, 1)).unwrap();
    q.push(pres(8, 2)).unwrap();
    q.push(pres(7, 3)).unwrap(); // replaces pres(7, 1)
    q.push(pres(7, 4)).unwrap(); // replaces pres(7, 3)
    assert_eq!(q.len(), 2);
    assert_eq!(drain_all(&mut q), vec![2, 4]);
}

#[test]
fn different_kinds_do_not_replace_each_other() {
    let mut q = CoalescingQueue::new();
    let a = Item {
        class: EventClass::Presentation,
        key: Some(CoalesceKey::new(7, 0)),
        id: 1,
    };
    let b = Item {
        class: EventClass::Presentation,
        key: Some(CoalesceKey::new(7, 1)),
        id: 2,
    };
    q.push(a).unwrap();
    q.push(b).unwrap();
    assert_eq!(q.len(), 2);
    assert_eq!(drain_all(&mut q), vec![1, 2]);
}

#[test]
fn classes_do_not_cross_coalesce() {
    let mut q = CoalescingQueue::new();
    q.push(cfg(7, 1)).unwrap();
    q.push(pres(7, 2)).unwrap();
    assert_eq!(q.len(), 2);
    assert_eq!(drain_all(&mut q), vec![1, 2]);
}

#[test]
fn unkeyed_presentation_never_replaced() {
    let mut q = CoalescingQueue::new();
    let a = Item {
        class: EventClass::Presentation,
        key: None,
        id: 1,
    };
    let b = a;
    q.push(a).unwrap();
    q.push(b).unwrap();
    assert_eq!(q.len(), 2);
}

#[test]
fn input_and_data_backpressure() {
    let mut q = CoalescingQueue::with_capacities([2, 1, 8, 8, 8]);
    q.push(input(1)).unwrap();
    q.push(input(2)).unwrap();
    assert_eq!(
        q.push(input(3)),
        Err(QueueFull {
            class: EventClass::Input
        })
    );
    q.push(data(1)).unwrap();
    assert_eq!(
        q.push(data(2)),
        Err(QueueFull {
            class: EventClass::Data
        })
    );
    // Popping one input frees exactly one slot.
    assert_eq!(q.pop().map(|i| i.id), Some(1));
    q.push(input(3)).unwrap();
    // The rejected pushes never queued.
    assert_eq!(q.len(), 3);
}

#[test]
fn coalescing_class_evicts_oldest_under_pressure() {
    let mut q = CoalescingQueue::with_capacities([8, 8, 8, 2, 8]);
    q.push(pres(7, 1)).unwrap();
    q.push(pres(8, 2)).unwrap();
    q.push(pres(9, 3)).unwrap(); // evicts pres(7, 1)
    assert_eq!(q.len(), 2);
    assert_eq!(drain_all(&mut q), vec![2, 3]);
}

#[test]
fn zero_capacity_coalescing_class_swallows() {
    let mut q = CoalescingQueue::with_capacities([8, 8, 0, 8, 8]);
    q.push(cfg(1, 1)).unwrap();
    assert_eq!(q.len(), 0);
    assert!(q.is_empty());
}

#[test]
fn interleaved_coalescing_keeps_latest_position() {
    let mut q = CoalescingQueue::new();
    q.push(pres(7, 1)).unwrap();
    q.push(input(9)).unwrap();
    q.push(pres(7, 2)).unwrap(); // tombstones pres(7,1) at its slot
                                 // Drain: input(9) then pres(7,2): the surviving value keeps the
                                 // LATEST arrival position.
    assert_eq!(drain_all(&mut q), vec![9, 2]);
}

#[test]
fn length_bookkeeping_under_tombstones() {
    let mut q = CoalescingQueue::new();
    for id in 0..100u32 {
        q.push(pres(42, id)).unwrap();
    }
    assert_eq!(q.len(), 1);
    assert_eq!(q.len_class(EventClass::Presentation), 1);
    assert_eq!(q.pop().map(|i| i.id), Some(99));
    assert!(q.is_empty());
    // Queue still usable after full tombstoning.
    q.push(pres(43, 1)).unwrap();
    q.push(input(2)).unwrap();
    assert_eq!(drain_all(&mut q), vec![1, 2]);
}

#[test]
fn compaction_reclaims_tombstones() {
    let mut q = CoalescingQueue::new();
    for id in 0..200u32 {
        q.push(pres(42, id)).unwrap(); // 199 tombstones accumulate
    }
    // After the pop below, compaction drains the leading tombstones.
    assert_eq!(q.pop().map(|i| i.id), Some(199));
    assert!(q.slots_allocated() < 32, "compaction should have reclaimed");
}

#[test]
fn clear_resets_everything() {
    let mut q = CoalescingQueue::new();
    q.push(pres(7, 1)).unwrap();
    q.push(input(2)).unwrap();
    q.clear();
    assert!(q.is_empty());
    assert_eq!(q.pop(), None);
    // Fresh keys still coalesce correctly after clear.
    q.push(pres(7, 3)).unwrap();
    q.push(pres(7, 4)).unwrap();
    assert_eq!(q.len(), 1);
}

#[test]
fn scheduler_events_carry_presentation_class() {
    let surface = SurfaceId::from_raw(5);
    let target = SchedEvent::FrameDropped {
        surface,
        frame: 1,
        reason: ldp_core::time::FrameDropReason::DeadlineMissed,
    };
    assert_eq!(target.class(), EventClass::Presentation);
    assert_eq!(target.coalesce_key(), None);

    let mut q: CoalescingQueue<SchedEvent> = CoalescingQueue::new();
    let ft = SchedEvent::FrameTarget {
        surface,
        frame: 2,
        deadline: ldp_core::time::FrameDeadline {
            deadline: ldp_core::time::Mono::from_ns(1),
            target_vblank: ldp_core::time::Mono::from_ns(2),
            refresh: ldp_core::time::RefreshInterval::from_ns(3).unwrap(),
            budget_ns: 1,
            mode: ldp_core::time::PresentationMode::Vsync,
        },
    };
    q.push(ft).unwrap();
    q.push(target).unwrap();
    q.push(ft).unwrap(); // replaces the earlier FrameTarget
    assert_eq!(q.len(), 2);
    let classes: Vec<EventClass> = [q.pop(), q.pop()]
        .into_iter()
        .flatten()
        .map(|e| e.class())
        .collect();
    assert_eq!(classes, vec![EventClass::Presentation; 2]);
}
