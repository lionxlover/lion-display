//! Class-based event coalescing (`docs/architecture.md` §10.4).
//!
//! The emission path between compositor and clients under pressure:
//!
//! | Class | Policy |
//! |---|---|
//! | [`EventClass::Input`] | never dropped, never reordered (backpressure instead) |
//! | [`EventClass::InputState`] | position state: the latest value replaces the still-pending one *in its slot* (the oldest pending position), so a discrete event is never pre-empted by a newer sample; the discrete barrier ([`CoalescingQueue::seal`]) freezes the pending sample a discrete event rode with |
//! | [`EventClass::Presentation`] | coalesce to the latest per coalescing key |
//! | [`EventClass::Configuration`] | coalesce to the latest per coalescing key |
//! | [`EventClass::Data`] | never dropped (small, rare; backpressure instead) |
//!
//! Coalescing replaces an older still-queued item with the newer one
//! carrying the same key — the surviving value is the latest. For the
//! feedback classes (presentation, configuration) the survivor keeps
//! the *latest* arrival position, so cross-class ordering stays the
//! arrival order. For **position state** (the Phase 39 doctrine: a
//! pointer's absolute `motion` sample, the batch's `frame`
//! terminator) the survivor overwrites the *oldest* pending slot in
//! place — a 1000 Hz device parking sixteen samples between display
//! frames delivers one motion carrying the freshest coordinates, at
//! the position the first sample would have occupied, so discrete
//! input (buttons, enters, leaves) that queued between samples is
//! never overtaken. Delta streams (`relative_motion`) stay
//! [`EventClass::Input`]: every delta delivers, full fidelity. Items
//! without a key are never replaced (per-frame terminal signals such
//! as `frame_dropped` must survive).
//!
//! When a coalescing class itself hits capacity the oldest queued item
//! of that class is evicted: under pressure, stale presentation and
//! configuration state is expendable, stale input never is.
//!
//! The control class is deliberately absent: control messages are
//! barriers in the dispatch lanes (`ldp-server` §9), not queue content.

use std::collections::BTreeMap;
use std::collections::VecDeque;

use crate::scheduler::SchedEvent;
use crate::surface::SurfaceId;

/// Dispatch class of one queued event.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum EventClass {
    /// Input events: never dropped, never reordered.
    Input,
    /// Data transfers: never dropped.
    Data,
    /// Configuration state: coalesce to the latest per key.
    Configuration,
    /// Presentation feedback: coalesce to the latest per key.
    Presentation,
    /// Position state (Phase 39): the latest value replaces the
    /// still-pending one *in its slot*. A pointer's absolute motion
    /// sample and the batch's frame terminator are position state —
    /// the freshest coordinates ride the oldest pending slot, so
    /// discrete input queued between samples is never pre-empted.
    InputState,
}

impl EventClass {
    /// Whether same-key items of this class replace each other.
    #[must_use]
    pub const fn coalesces(self) -> bool {
        matches!(
            self,
            Self::Configuration | Self::Presentation | Self::InputState
        )
    }

    const fn ordinal(self) -> u8 {
        match self {
            Self::Input => 0,
            Self::Data => 1,
            Self::Configuration => 2,
            Self::Presentation => 3,
            Self::InputState => 4,
        }
    }
}

/// Identity of a coalescing group: `(group, kind, sub)` — for
/// scheduler events, the surface and the event kind (`frame_target`
/// vs `presented` coalesce independently); for input position state,
/// the target object, the event kind, and the **contact** (Phase 43:
/// touch and tablet position state coalesce per contact/tool, so a
/// two-finger scroll collapses each finger to its own freshest
/// sample instead of one finger's sample replacing the other's).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct CoalesceKey {
    /// Coalescing group (a surface, an object, ...).
    pub group: u64,
    /// Distinguishes kinds within the group.
    pub kind: u8,
    /// The sub-group discriminator: a touch contact id, a tablet
    /// tool id. `0` for single-stream keys (the pointer's own
    /// position state, scheduler feedback) — every pre-Phase-43 key
    /// keeps its exact meaning.
    pub sub: u64,
}

impl CoalesceKey {
    /// Build a key (single-stream: no contact discriminator).
    #[must_use]
    pub const fn new(group: u64, kind: u8) -> Self {
        Self {
            group,
            kind,
            sub: 0,
        }
    }

    /// Build a per-contact key (Phase 43): `contact` distinguishes
    /// the streams that share one target object — touch contact ids,
    /// tablet tool ids. The pointer's single stream is the
    /// `contact == 0` case of the same arithmetic.
    #[must_use]
    pub const fn contact(group: u64, kind: u8, contact: u64) -> Self {
        Self {
            group,
            kind,
            sub: contact,
        }
    }
}

/// Items a [`CoalescingQueue`] accepts.
pub trait Coalescible {
    /// Dispatch class.
    fn class(&self) -> EventClass;
    /// Coalescing key; `None` means "never replaced".
    fn coalesce_key(&self) -> Option<CoalesceKey>;
}

impl Coalescible for SchedEvent {
    fn class(&self) -> EventClass {
        // All scheduler emissions are presentation-class feedback.
        EventClass::Presentation
    }

    fn coalesce_key(&self) -> Option<CoalesceKey> {
        match *self {
            // §10.4: frame_target and presented coalesce to the latest
            // per surface. frame_dropped is a per-frame terminal signal
            // and must survive coalescing.
            SchedEvent::FrameTarget { surface, .. } => Some(target_key(surface)),
            SchedEvent::Presented { surface, .. } => Some(presented_key(surface)),
            SchedEvent::FrameDropped { .. } => None,
        }
    }
}

const KIND_TARGET: u8 = 0;
const KIND_PRESENTED: u8 = 1;

fn target_key(surface: SurfaceId) -> CoalesceKey {
    CoalesceKey::new(surface.raw(), KIND_TARGET)
}

fn presented_key(surface: SurfaceId) -> CoalesceKey {
    CoalesceKey::new(surface.raw(), KIND_PRESENTED)
}

/// A queue slot; tombstoned when superseded.
struct Slot<T> {
    class: EventClass,
    key: Option<CoalesceKey>,
    seq: u64,
    item: Option<T>,
}

/// The queue is full for a never-drop class; the caller must drain (or
/// otherwise apply backpressure) before pushing more.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct QueueFull {
    /// Which class hit its capacity.
    pub class: EventClass,
}

/// Class-aware coalescing queue.
///
/// FIFO across all classes; per-key replacement for the coalescing
/// classes (in-place for position state, tombstone-then-push for the
/// feedback classes); backpressure for input/data; oldest-first
/// eviction for coalescing classes at capacity.
pub struct CoalescingQueue<T> {
    slots: VecDeque<Slot<T>>,
    /// Sequence number of `slots.front()`.
    base_seq: u64,
    next_seq: u64,
    /// Live slot per (class, key), by sequence number.
    positions: BTreeMap<(u8, CoalesceKey), u64>,
    live: [usize; 5],
    capacities: [usize; 5],
}

impl<T: Coalescible> CoalescingQueue<T> {
    /// A queue with the default capacities (input 1024, data 256,
    /// configuration 256, presentation 512, position state 64).
    #[must_use]
    pub fn new() -> Self {
        Self::with_capacities([1024, 256, 256, 512, 64])
    }

    /// A queue with explicit per-class capacities, in class-ordinal
    /// order (input, data, configuration, presentation, position
    /// state). A capacity of zero rejects/evicts immediately.
    #[must_use]
    pub fn with_capacities(capacities: [usize; 5]) -> Self {
        Self {
            slots: VecDeque::new(),
            base_seq: 0,
            next_seq: 0,
            positions: BTreeMap::new(),
            live: [0; 5],
            capacities,
        }
    }

    /// Total live items queued.
    #[must_use]
    pub fn len(&self) -> usize {
        self.live.iter().sum()
    }

    /// Whether no live items are queued.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Live items of one class.
    #[must_use]
    pub fn len_class(&self, class: EventClass) -> usize {
        self.live[usize::from(class.ordinal())]
    }

    /// Push an item, applying the class policy.
    ///
    /// # Errors
    /// [`QueueFull`] when a never-drop class (input/data) is at
    /// capacity — the item is *not* queued and the caller must apply
    /// backpressure.
    pub fn push(&mut self, item: T) -> Result<(), QueueFull> {
        let class = item.class();
        let key = item.coalesce_key();
        let ord = usize::from(class.ordinal());
        // Same-key replacement for coalescing classes.
        if class.coalesces() {
            if let Some(&old_seq) = key
                .as_ref()
                .and_then(|k| self.positions.get(&(class.ordinal(), *k)))
            {
                if class == EventClass::InputState {
                    // Position state: the latest value overwrites the
                    // oldest pending slot IN PLACE — the slot's order
                    // relative to every other queued event never moves,
                    // so discrete input that queued between samples is
                    // never pre-empted by a fresher one.
                    let index = (old_seq - self.base_seq) as usize;
                    if let Some(slot) = self.slots.get_mut(index) {
                        if slot.seq == old_seq {
                            slot.item = Some(item);
                            return Ok(());
                        }
                    }
                    // The bookkeeping cannot go stale (pop/tombstone
                    // keep it exact); if it ever did, fall through and
                    // queue a fresh slot — self-healing, never lossy.
                } else {
                    self.tombstone(old_seq);
                }
            }
        }
        if self.live[ord] >= self.capacities[ord] {
            if class.coalesces() {
                // Pressure: expend the oldest queued item of this class.
                self.evict_oldest(class);
                if self.live[ord] >= self.capacities[ord] {
                    // Capacity zero: nothing may be queued at all.
                    return Ok(());
                }
            } else {
                return Err(QueueFull { class });
            }
        }
        let seq = self.next_seq;
        self.next_seq += 1;
        if let Some(k) = key {
            if class.coalesces() {
                self.positions.insert((class.ordinal(), k), seq);
            }
        }
        self.live[ord] += 1;
        self.slots.push_back(Slot {
            class,
            key,
            seq,
            item: Some(item),
        });
        self.compact();
        Ok(())
    }

    /// Pop the oldest live item (any class), skipping tombstones.
    pub fn pop(&mut self) -> Option<T> {
        loop {
            // Peek the metadata (all `Copy`) without holding a borrow.
            let front = self.slots.front()?;
            let (seq, class, key) = (front.seq, front.class, front.key);
            if front.item.is_none() {
                self.slots.pop_front();
                self.base_seq = seq + 1;
                continue;
            }
            // The front slot is live; take it whole.
            let slot = self.slots.pop_front()?;
            self.base_seq = seq + 1;
            self.live[usize::from(class.ordinal())] -= 1;
            if let Some(k) = key {
                if self.positions.get(&(class.ordinal(), k)) == Some(&seq) {
                    self.positions.remove(&(class.ordinal(), k));
                }
            }
            self.compact();
            return slot.item;
        }
    }

    /// Drop everything.
    pub fn clear(&mut self) {
        self.slots.clear();
        self.positions.clear();
        self.live = [0; 5];
        self.base_seq = self.next_seq;
    }

    /// Seal a live coalescing key (the discrete-input barrier): the
    /// pending item keeps its slot and its value — frozen — and later
    /// same-key items queue fresh slots instead of overwriting it.
    ///
    /// A button, an enter/leave, or any other discrete pointer event
    /// seals the position state that preceded it: the discrete event's
    /// context is the sample it rode with, never a newer one (the
    /// X11 flush-before-button doctrine, expressed as a freeze).
    /// Sealing an absent key is a no-op.
    pub fn seal(&mut self, class: EventClass, key: CoalesceKey) {
        if !class.coalesces() {
            return;
        }
        self.positions.remove(&(class.ordinal(), key));
    }

    /// Tombstone the slot with sequence number `seq` (still live).
    fn tombstone(&mut self, seq: u64) {
        let index = (seq - self.base_seq) as usize;
        if let Some(slot) = self.slots.get_mut(index) {
            if slot.seq == seq && slot.item.take().is_some() {
                self.live[usize::from(slot.class.ordinal())] -= 1;
                if let Some(k) = slot.key {
                    self.positions.remove(&(slot.class.ordinal(), k));
                }
            }
        }
    }

    /// Evict the oldest live item of `class` (pressure policy).
    fn evict_oldest(&mut self, class: EventClass) {
        for index in 0..self.slots.len() {
            if self.slots[index].class == class && self.slots[index].item.is_some() {
                let seq = self.slots[index].seq;
                self.tombstone(seq);
                return;
            }
        }
    }

    /// Diagnostic: total slots currently allocated (live + tombstoned).
    ///
    /// Memory accounting for embedders and tests: coalescing tombstones
    /// superseded slots and `pop`/`compact` reclaim them, so this value
    /// staying bounded under sustained same-key churn is the observable
    /// proof that the queue does not leak.
    #[must_use]
    pub fn slots_allocated(&self) -> usize {
        self.slots.len()
    }

    /// Reclaim leading tombstones once they dominate.
    fn compact(&mut self) {
        let dead = self
            .slots
            .iter()
            .take_while(|slot| slot.item.is_none())
            .count();
        if dead >= 32 || dead * 2 >= self.slots.len().max(1) {
            self.slots.drain(0..dead);
            self.base_seq += dead as u64;
        }
    }
}

impl<T: Coalescible> Default for CoalescingQueue<T> {
    fn default() -> Self {
        Self::new()
    }
}
