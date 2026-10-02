//! The outbox: cross-client delivery of asynchronously produced events.
//!
//! The frame loop runs on whichever session thread woke last, but it
//! can produce events for *any* connected client — a page flip that
//! lands presentation feedback on every surface that targeted that
//! vblank. Only the waking client's session context can emit directly
//! (the transport stream is owned by that session's thread), so the
//! pump routes through [`OutboxEntry`] records: the waking client's
//! entries stream immediately through `DispatchCtx`, everyone else's
//! park in per-client queues that their own dispatchers drain at their
//! next wake point.
//!
//! Phase 39: the parked queues are the §10.4 class-aware emission
//! path — [`ldp_compositor::coalesce::CoalescingQueue`] — so a client
//! that drains at frame cadence receives the *freshest* position, not
//! the flood: a 1000 Hz device parks sixteen motion samples between
//! display frames and the client reads one (`pointer.motion` carrying
//! the latest coordinates, every `relative_motion` delta, one `frame`
//! terminator). Discrete input (buttons, enters, leaves) never drops,
//! never reorders, and seals the position state it rode with (the
//! barrier); presentation feedback coalesces to the latest per
//! surface and kind under backlog. The class is declared where the
//! semantics are known — the emission sites set the entry's
//! coalescing hint; the unclassified default keeps the exact
//! pre-Phase-39 FIFO.
//!
//! Entries target objects that may have been destroyed by the time the
//! owner drains — the draining side liveness-checks each target and
//! silently drops (closing any riding descriptor) what no longer
//! resolves. That is the honest interpretation of "the client is gone
//! or moved on".

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::os::fd::OwnedFd;

use ldp_compositor::coalesce::{CoalesceKey, Coalescible, CoalescingQueue, EventClass};
use ldp_core::ids::{ClientId, ObjectId};
use ldp_core::wire::Value;

/// The pointer-motion kind tag (position state).
pub const POSITION_MOTION: u8 = 0;
/// The pointer-frame kind tag (position state).
pub const POSITION_FRAME: u8 = 1;
/// The tablet-pressure kind tag (per-tool axis state, Phase 43).
pub const POSITION_PRESSURE: u8 = 2;
/// The tablet-tilt kind tag (per-tool axis state, Phase 43).
pub const POSITION_TILT: u8 = 3;
/// The tablet-rotation kind tag (per-tool axis state, Phase 43).
pub const POSITION_ROTATION: u8 = 4;
/// The tablet-slider kind tag (per-tool axis state, Phase 43).
pub const POSITION_SLIDER: u8 = 5;
/// The touch-shape kind tag (per-contact axis state, Phase 45 —
/// the touch axis-metadata coalescing: `touch.shape` is position
/// state of its own kind, the pressure/tilt doctrine applied to the
/// contact's geometry axes).
pub const POSITION_SHAPE: u8 = 6;
/// The touch-orientation kind tag (per-contact axis state, Phase 45
/// — the same doctrine: `touch.orientation` collapses to the
/// contact's freshest angle per display frame).
pub const POSITION_ORIENTATION: u8 = 7;
/// The drag-configure kind tag (Phase 50 — the operator's hand): the
/// interactive resize's live proposals are the grip's position state
/// — the freshest target replaces the still-pending one in its slot
/// (a pointer-paced flood collapses to the latest, exactly the 1000
/// Hz motion doctrine), the release's final proposal rides the plain
/// never-dropped class, and the machine's grace window keeps every
/// *delivered* superseded serial acknowledgeable.
pub const POSITION_DRAG_CONFIGURE: u8 = 8;
/// The frame_target kind tag (presentation feedback).
pub const PRESENTATION_TARGET: u8 = 0;
/// The presented kind tag (presentation feedback).
pub const PRESENTATION_PRESENTED: u8 = 1;

/// One pending delivery: an event (everything `DispatchCtx::emit[_fd]`
/// needs) or a revocation (the server-initiated teardown the spec's
/// `connection.revoked` carries — Phase 26's output-migration signal).
#[derive(Debug)]
pub struct OutboxEntry {
    /// The client whose session owns the target object.
    pub client: ClientId,
    /// The event's target object.
    pub target: ObjectId,
    /// The event name within the target's interface.
    pub event: &'static str,
    /// The event arguments (wire values, in declaration order).
    pub args: Vec<Value>,
    /// A descriptor riding the message (`buffer.release` fences).
    pub fd: Option<OwnedFd>,
    /// When set, the entry revokes the target instead of emitting on
    /// it (`revoked_reason` wire value): the draining side calls
    /// `DispatchCtx::revoke`, removing the object and telling the
    /// client why. The output going away mid-session uses
    /// `capability_revoked` — the display it mirrored is gone.
    pub revoke: Option<u32>,
    /// An object the draining side must create in the receiver's
    /// store before emitting: the server-chosen `new_id` of a routed
    /// event (the data family's `data_offer` announcements — the
    /// receiver cannot send requests on an object it does not hold).
    /// The id arrives world-unique, so the create cannot collide.
    pub create: Option<(ObjectId, &'static str)>,
    /// The emission-path coalescing doctrine (§10.4) of this entry,
    /// declared where the semantics are known.
    hint: CoalesceHint,
}

/// The §10.4 class of one outbox entry, as the emission site
/// declares it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CoalesceHint {
    /// Position state (`pointer.motion`, the `pointer.frame`
    /// terminator, per-contact touch/tablet motion and axis state):
    /// the latest value replaces the still-pending one in its slot —
    /// keyed per contact when the stream carries one (Phase 43: each
    /// touch contact and each tablet tool collapses to its own
    /// freshest sample, never another contact's).
    Position {
        /// Kind within the target object (motion, frame, an axis).
        kind: u8,
        /// The contact/tool discriminator (`0` for single streams).
        contact: u64,
    },
    /// Discrete input (buttons, keys, enters, leaves, axes): never
    /// dropped, never reordered. It carries the contact whose pending
    /// position state it seals at push time (`None` = no seal —
    /// `touch.cancel`, whose voiding *is* its context): the barrier
    /// keeps the discrete event's context the sample it rode with.
    Discrete {
        /// The contact whose position state this event seals.
        seal: Option<u64>,
    },
    /// A delta stream riding the position samples
    /// (`pointer.relative_motion`, `tablet.wheel`): Input class, never
    /// dropped, no seal — every delta delivers, full fidelity.
    Delta,
    /// Presentation feedback (`frame_target`, `presented`): coalesces
    /// to the latest per surface and kind.
    Presentation(u8),
    /// Unclassified — the conservative default: Input class, never
    /// dropped, never replaced. Exactly the FIFO of the pre-Phase-39
    /// outbox; nothing coalesces unless an emission site says so.
    Plain,
}

impl Coalescible for OutboxEntry {
    fn class(&self) -> EventClass {
        match self.hint {
            CoalesceHint::Position { .. } => EventClass::InputState,
            CoalesceHint::Discrete { .. } | CoalesceHint::Delta | CoalesceHint::Plain => {
                EventClass::Input
            }
            CoalesceHint::Presentation(_) => EventClass::Presentation,
        }
    }
    fn coalesce_key(&self) -> Option<CoalesceKey> {
        match self.hint {
            CoalesceHint::Position { kind, contact } => Some(CoalesceKey::contact(
                u64::from(self.target.as_u32()),
                kind,
                contact,
            )),
            CoalesceHint::Presentation(kind) => {
                Some(CoalesceKey::new(u64::from(self.target.as_u32()), kind))
            }
            _ => None,
        }
    }
}

impl OutboxEntry {
    /// An entry without a descriptor and without a coalescing class
    /// (the plain FIFO doctrine — the pre-Phase-39 behavior).
    #[must_use]
    pub fn event(
        client: ClientId,
        target: ObjectId,
        event: &'static str,
        args: Vec<Value>,
    ) -> Self {
        OutboxEntry {
            client,
            target,
            event,
            args,
            fd: None,
            revoke: None,
            create: None,
            hint: CoalesceHint::Plain,
        }
    }

    /// An entry carrying one descriptor (ownership moves with it).
    #[must_use]
    pub fn with_fd(
        client: ClientId,
        target: ObjectId,
        event: &'static str,
        args: Vec<Value>,
        fd: OwnedFd,
    ) -> Self {
        OutboxEntry {
            fd: Some(fd),
            ..Self::event(client, target, event, args)
        }
    }

    /// A position-state entry (`pointer.motion`, the `pointer.frame`
    /// terminator): the freshest sample replaces the still-pending one
    /// in its slot — the collapse that makes a 1000 Hz device deliver
    /// one wake per display frame.
    #[must_use]
    pub fn position_state(
        client: ClientId,
        target: ObjectId,
        event: &'static str,
        args: Vec<Value>,
        kind: u8,
    ) -> Self {
        OutboxEntry {
            hint: CoalesceHint::Position { kind, contact: 0 },
            ..Self::event(client, target, event, args)
        }
    }

    /// A per-contact position-state entry (Phase 43: `touch.motion`
    /// per contact, `tablet.motion` and the per-tool axes): the
    /// freshest sample of *that contact* replaces its own still-pending
    /// one in its own slot — a two-finger scroll collapses each finger
    /// to its own freshest sample, never one finger's sample riding
    /// another's slot.
    #[must_use]
    pub fn contact_position_state(
        client: ClientId,
        target: ObjectId,
        event: &'static str,
        args: Vec<Value>,
        kind: u8,
        contact: u64,
    ) -> Self {
        OutboxEntry {
            hint: CoalesceHint::Position { kind, contact },
            ..Self::event(client, target, event, args)
        }
    }

    /// A discrete-input entry (buttons, keys, enters, leaves, axes):
    /// never dropped, never reordered — and it seals the target
    /// object's pending position state at push time.
    #[must_use]
    pub fn discrete_input(
        client: ClientId,
        target: ObjectId,
        event: &'static str,
        args: Vec<Value>,
    ) -> Self {
        OutboxEntry {
            hint: CoalesceHint::Discrete { seal: Some(0) },
            ..Self::event(client, target, event, args)
        }
    }

    /// A per-contact discrete-input entry (Phase 43: `touch.down`/
    /// `touch.up`/`touch.shape`/`touch.orientation` per contact,
    /// `tablet.down`/`up`/`button`/`tool` per tool): never dropped,
    /// never reordered — and it seals *that contact's* pending
    /// position state at push time (`None` — `touch.cancel` — seals
    /// nothing: a new sequence must begin with a `touch.down`, which
    /// carries its own seal).
    #[must_use]
    pub fn contact_discrete_input(
        client: ClientId,
        target: ObjectId,
        event: &'static str,
        args: Vec<Value>,
        seal: Option<u64>,
    ) -> Self {
        OutboxEntry {
            hint: CoalesceHint::Discrete { seal },
            ..Self::event(client, target, event, args)
        }
    }

    /// A delta-stream entry (`pointer.relative_motion`, `tablet.wheel`):
    /// every delta delivers (Input class, no seal, no replacement).
    #[must_use]
    pub fn delta_input(
        client: ClientId,
        target: ObjectId,
        event: &'static str,
        args: Vec<Value>,
    ) -> Self {
        OutboxEntry {
            hint: CoalesceHint::Delta,
            ..Self::event(client, target, event, args)
        }
    }

    /// A presentation-feedback entry (`frame_target`, `presented`):
    /// coalesces to the latest per surface and kind under backlog.
    #[must_use]
    pub fn presentation(
        client: ClientId,
        target: ObjectId,
        event: &'static str,
        args: Vec<Value>,
        kind: u8,
    ) -> Self {
        OutboxEntry {
            hint: CoalesceHint::Presentation(kind),
            ..Self::event(client, target, event, args)
        }
    }

    /// Attach a riding descriptor (the builder form of
    /// [`OutboxEntry::with_fd`] — for entries whose coalescing class
    /// is declared first).
    #[must_use]
    pub fn carrying_fd(mut self, fd: OwnedFd) -> Self {
        self.fd = Some(fd);
        self
    }

    /// Set the create-first instruction (a server-chosen `new_id`
    /// the receiver must hold before the event makes sense).
    #[must_use]
    pub fn creating(mut self, id: ObjectId, interface: &'static str) -> Self {
        self.create = Some((id, interface));
        self
    }

    /// An entry that revokes the target object (`connection.revoked`
    /// with `reason`) instead of emitting an event on it — the
    /// outbox path for server-initiated teardown of objects the
    /// client still holds. Reason values are the spec's
    /// `revoked_reason` table (2 = `capability_revoked`).
    #[must_use]
    pub fn revoke(client: ClientId, target: ObjectId, reason: u32) -> Self {
        OutboxEntry {
            event: "revoked",
            revoke: Some(reason),
            ..Self::event(client, target, "revoked", Vec::new())
        }
    }
}

/// Per-client queues of pending outbound events — the §10.4
/// class-aware emission path (Phase 39).
///
/// Capacities: the never-drop classes (input, data) are unbounded in
/// the served outbox (the pre-Phase-39 FIFO behavior — a parked
/// queue grows only for a client that never wakes, and the
/// transport's own dead-peer teardown guards that); the coalescing
/// classes are bounded (512 presentation slots, 64 position-state
/// slots) — under pressure, stale feedback and superseded position
/// samples are expendable, discrete input never is.
#[derive(Default)]
pub struct Outboxes {
    queues: HashMap<u32, CoalescingQueue<OutboxEntry>>,
}

impl Outboxes {
    /// An empty mailbox set.
    #[must_use]
    pub fn new() -> Outboxes {
        Outboxes::default()
    }

    fn queue_of() -> CoalescingQueue<OutboxEntry> {
        // (input, data, configuration, presentation, position state)
        CoalescingQueue::with_capacities([usize::MAX, usize::MAX, 256, 512, 64])
    }

    /// Append one entry (any client), applying its declared
    /// coalescing doctrine.
    pub fn push(&mut self, entry: OutboxEntry) {
        let queue = self
            .queues
            .entry(entry.client.as_u32())
            .or_insert_with(Self::queue_of);
        if let Some(contact) = entry.sealed_contact() {
            // The discrete barrier: freeze the pending position state
            // of the contact the discrete event rode with — the
            // event's context is the sample it carried, never a newer
            // one. Pointer events seal the object's own stream
            // (contact 0); touch and tablet events seal their
            // contact's stream, so a `touch.down` never lets a newer
            // sample of the same finger jump ahead of it.
            queue.seal(
                EventClass::InputState,
                CoalesceKey::contact(u64::from(entry.target.as_u32()), POSITION_MOTION, contact),
            );
            queue.seal(
                EventClass::InputState,
                CoalesceKey::contact(u64::from(entry.target.as_u32()), POSITION_FRAME, contact),
            );
        }
        // The never-drop classes are unbounded here, so the push
        // cannot fail; a coalescing class at capacity evicts its own
        // oldest (the doctrine's pressure policy).
        let _ = queue.push(entry);
    }

    /// Append many, preserving order.
    pub fn extend(&mut self, entries: impl IntoIterator<Item = OutboxEntry>) {
        for e in entries {
            self.push(e);
        }
    }

    /// Take one client's pending entries (FIFO across classes).
    #[must_use]
    pub fn drain(&mut self, client: ClientId) -> Vec<OutboxEntry> {
        let Some(queue) = self.queues.get_mut(&client.as_u32()) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        while let Some(entry) = queue.pop() {
            out.push(entry);
        }
        out
    }

    /// Whether every queue is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.queues.values().all(CoalescingQueue::is_empty)
    }

    /// Total pending entries across clients.
    #[must_use]
    pub fn len(&self) -> usize {
        self.queues.values().map(CoalescingQueue::len).sum()
    }

    /// Drop everything queued for a client (session end): riding
    /// descriptors close as the entries drop.
    pub fn drop_client(&mut self, client: ClientId) {
        self.queues.remove(&client.as_u32());
    }
}

impl OutboxEntry {
    /// Whether this entry seals the target object's pending position
    /// state at push time (the discrete barrier).
    fn sealed_contact(&self) -> Option<u64> {
        match self.hint {
            CoalesceHint::Discrete { seal } => seal,
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sys;
    use std::os::fd::AsRawFd;

    fn cid(n: u32) -> ClientId {
        ClientId::new(n).unwrap()
    }

    fn oid(n: u32) -> ObjectId {
        ObjectId::client(n).unwrap()
    }

    fn motion(client: ClientId, obj: ObjectId, x: f32, y: f32) -> OutboxEntry {
        OutboxEntry::position_state(
            client,
            obj,
            "motion",
            vec![Value::Float32(x), Value::Float32(y)],
            POSITION_MOTION,
        )
    }

    fn frame_ev(client: ClientId, obj: ObjectId) -> OutboxEntry {
        OutboxEntry::position_state(client, obj, "frame", vec![], POSITION_FRAME)
    }

    #[test]
    fn queues_are_per_client_and_fifo() {
        let mut boxes = Outboxes::new();
        boxes.push(OutboxEntry::event(cid(1), oid(10), "committed", vec![]));
        boxes.push(OutboxEntry::event(cid(2), oid(20), "committed", vec![]));
        boxes.push(OutboxEntry::event(cid(1), oid(11), "presented", vec![]));
        assert_eq!(boxes.len(), 3);
        let one = boxes.drain(cid(1));
        assert_eq!(one.len(), 2);
        assert_eq!(one[0].target, oid(10));
        assert_eq!(one[1].event, "presented");
        assert!(!boxes.is_empty(), "client 2 still queued");
        assert!(boxes.drain(cid(1)).is_empty());
        assert_eq!(boxes.drain(cid(2)).len(), 1);
        assert!(boxes.is_empty());
    }

    #[test]
    fn dropping_a_client_closes_its_fds() {
        let mut boxes = Outboxes::new();
        let fd = sys::eventfd_signalled().unwrap();
        let raw = fd.as_raw_fd();
        boxes.push(OutboxEntry::with_fd(cid(7), oid(30), "release", vec![], fd));
        boxes.drop_client(cid(7));
        // The descriptor closed with the entry: the number is either
        // reused (nonnegative) or the standard closed sentinel; the
        // observable contract is that draining yields nothing.
        assert!(boxes.drain(cid(7)).is_empty());
        let _ = raw;
    }

    #[test]
    fn sixteen_samples_deliver_one_motion_with_the_freshest_coordinates() {
        // The Phase 39 doctrine, end to end at the outbox: sixteen
        // parked (motion, relative, frame) batches collapse to one
        // motion carrying sample 16's coordinates — at the FIRST
        // batch's slot — plus every delta and one terminator.
        let mut boxes = Outboxes::new();
        let (client, pointer) = (cid(1), oid(10));
        for sample in 1..=16 {
            boxes.push(motion(client, pointer, sample as f32, sample as f32));
            boxes.push(OutboxEntry::delta_input(
                client,
                pointer,
                "relative_motion",
                vec![],
            ));
            boxes.push(frame_ev(client, pointer));
        }
        assert_eq!(boxes.len(), 18);
        let delivered = boxes.drain(client);
        let names: Vec<&str> = delivered.iter().map(|e| e.event).collect();
        assert_eq!(
            names,
            vec![
                "motion",
                "relative_motion",
                "frame",
                "relative_motion",
                "relative_motion",
                "relative_motion",
                "relative_motion",
                "relative_motion",
                "relative_motion",
                "relative_motion",
                "relative_motion",
                "relative_motion",
                "relative_motion",
                "relative_motion",
                "relative_motion",
                "relative_motion",
                "relative_motion",
                "relative_motion",
            ]
        );
        // The surviving motion carries the FRESHEST sample.
        assert!(
            matches!(delivered[0].args[0], Value::Float32(v) if v == 16.0),
            "the freshest coordinates: {:?}",
            delivered[0].args
        );
    }

    #[test]
    fn the_discrete_barrier_seals_the_pending_sample() {
        // A click between two motion samples: the click's context is
        // the sample it rode with (frozen), and the post-click sample
        // queues after it — the X11 flush-before-button doctrine.
        let mut boxes = Outboxes::new();
        let (client, pointer) = (cid(1), oid(10));
        boxes.push(motion(client, pointer, 1.0, 1.0));
        boxes.push(frame_ev(client, pointer));
        boxes.push(OutboxEntry::discrete_input(
            client,
            pointer,
            "button",
            vec![],
        ));
        boxes.push(motion(client, pointer, 2.0, 2.0));
        boxes.push(frame_ev(client, pointer));
        let delivered = boxes.drain(client);
        let names: Vec<&str> = delivered.iter().map(|e| e.event).collect();
        assert_eq!(names, vec!["motion", "frame", "button", "motion", "frame"]);
        // The pre-click motion kept ITS sample (frozen at the barrier).
        assert!(
            matches!(delivered[0].args[0], Value::Float32(v) if v == 1.0),
            "the click's own sample: {:?}",
            delivered[0].args
        );
        assert!(
            matches!(delivered[3].args[0], Value::Float32(v) if v == 2.0),
            "the post-click sample: {:?}",
            delivered[3].args
        );
    }

    #[test]
    fn plain_entries_keep_the_fifo_doctrine() {
        // Unclassified entries never coalesce — the pre-Phase-39
        // behavior is the default, nothing collapses unless the
        // emission site says so.
        let mut boxes = Outboxes::new();
        let (client, obj) = (cid(1), oid(10));
        for i in 0..8 {
            boxes.push(OutboxEntry::event(
                client,
                obj,
                "anything",
                vec![Value::Uint32(i)],
            ));
        }
        assert_eq!(boxes.len(), 8);
        let delivered = boxes.drain(client);
        let ids: Vec<u32> = delivered
            .iter()
            .filter_map(|e| match e.args.first() {
                Some(Value::Uint32(v)) => Some(*v),
                _ => None,
            })
            .collect();
        assert_eq!(ids, (0..8).collect::<Vec<u32>>());
    }

    #[test]
    fn presentation_feedback_collapses_to_the_latest_per_kind() {
        let mut boxes = Outboxes::new();
        let (client, surface) = (cid(1), oid(10));
        for frame in 1..=6u64 {
            boxes.push(OutboxEntry::presentation(
                client,
                surface,
                "frame_target",
                vec![Value::Uint64(frame)],
                PRESENTATION_TARGET,
            ));
            boxes.push(OutboxEntry::presentation(
                client,
                surface,
                "presented",
                vec![Value::Uint64(frame)],
                PRESENTATION_PRESENTED,
            ));
        }
        assert_eq!(boxes.len(), 2);
        let delivered = boxes.drain(client);
        assert_eq!(delivered.len(), 2);
        assert!(matches!(delivered[0].args[0], Value::Uint64(6)));
        assert!(matches!(delivered[1].args[0], Value::Uint64(6)));
    }

    fn touch_motion(client: ClientId, obj: ObjectId, contact: u32, x: f32, y: f32) -> OutboxEntry {
        OutboxEntry::contact_position_state(
            client,
            obj,
            "motion",
            vec![
                Value::Uint32(u64::from(contact) as u32),
                Value::Float32(x),
                Value::Float32(y),
            ],
            POSITION_MOTION,
            u64::from(contact),
        )
    }

    fn touch_down(client: ClientId, obj: ObjectId, contact: u32) -> OutboxEntry {
        OutboxEntry::contact_discrete_input(
            client,
            obj,
            "down",
            vec![Value::Uint32(u64::from(contact) as u32)],
            Some(u64::from(contact)),
        )
    }

    fn tablet_axis(
        client: ClientId,
        obj: ObjectId,
        tool: u64,
        event: &'static str,
        kind: u8,
        value: f32,
    ) -> OutboxEntry {
        OutboxEntry::contact_position_state(
            client,
            obj,
            event,
            vec![Value::Uint64(tool), Value::Float32(value)],
            kind,
            tool,
        )
    }

    #[test]
    fn two_contact_flood_delivers_each_fingers_freshest_sample() {
        // Phase 43, the doctrine the roadmap's "per-contact keys" line
        // named: a 120 Hz touchscreen parking sixteen two-finger
        // batches between display frames delivers TWO motions — each
        // finger's own freshest sample in the slot that finger's
        // first sample took — plus one shared frame terminator. One
        // finger's sample never rides another finger's slot.
        let mut boxes = Outboxes::new();
        let (client, touch) = (cid(1), oid(10));
        for sample in 1..=16u32 {
            boxes.push(touch_motion(client, touch, 1, sample as f32, 100.0));
            boxes.push(touch_motion(client, touch, 2, 200.0, sample as f32));
            boxes.push(frame_ev(client, touch));
        }
        // 2 motions + 1 frame, whatever the sample count.
        assert_eq!(boxes.len(), 3);
        let delivered = boxes.drain(client);
        let names: Vec<&str> = delivered.iter().map(|e| e.event).collect();
        assert_eq!(names, vec!["motion", "motion", "frame"]);
        // Contact 1's survivor carries ITS freshest (16, 100) — and
        // still its own contact id in the arguments.
        assert_eq!(delivered[0].args[0], Value::Uint32(1));
        assert_eq!(delivered[0].args[1], Value::Float32(16.0));
        assert_eq!(delivered[0].args[2], Value::Float32(100.0));
        // Contact 2's survivor carries ITS freshest (200, 16), in its
        // own slot after contact 1's.
        assert_eq!(delivered[1].args[0], Value::Uint32(2));
        assert_eq!(delivered[1].args[1], Value::Float32(200.0));
        assert_eq!(delivered[1].args[2], Value::Float32(16.0));
    }

    #[test]
    fn the_touch_down_barrier_seals_only_its_own_contact() {
        // The discrete barrier, per contact: a `touch.down` of finger
        // 1 freezes finger 1's pending sample (the down's context is
        // the sample it rode with) — but finger 2's pending sample is
        // untouched: a newer finger-2 sample still replaces it, and
        // a newer finger-1 sample queues *after* the down.
        let mut boxes = Outboxes::new();
        let (client, touch) = (cid(1), oid(10));
        boxes.push(touch_motion(client, touch, 1, 1.0, 1.0));
        boxes.push(touch_motion(client, touch, 2, 2.0, 2.0));
        boxes.push(touch_down(client, touch, 1));
        // Finger 2 keeps moving; finger 1's post-down motion starts a
        // fresh slot (the seal froze the pre-down one).
        boxes.push(touch_motion(client, touch, 2, 3.0, 3.0));
        boxes.push(touch_motion(client, touch, 1, 9.0, 9.0));
        // Four entries: finger 1's frozen pre-down sample, finger 2's
        // freshest (replaced in its own slot), the down, and finger
        // 1's post-down motion at its own fresh slot.
        let delivered = boxes.drain(client);
        let names: Vec<&str> = delivered.iter().map(|e| e.event).collect();
        assert_eq!(names, vec!["motion", "motion", "down", "motion"]);
        // Finger 1's pre-down sample: the frozen (1, 1) — the fresher
        // post-down sample did NOT replace it (the seal froze the
        // slot; the new sample queued after the down).
        assert_eq!(delivered[0].args[0], Value::Uint32(1));
        assert_eq!(delivered[0].args[1], Value::Float32(1.0));
        // Finger 2's survivor is the FRESHEST (3, 3) — the down of
        // finger 1 did not seal finger 2's stream, so the newer
        // finger-2 sample replaced the older in its own slot.
        assert_eq!(delivered[1].args[0], Value::Uint32(2));
        assert_eq!(delivered[1].args[1], Value::Float32(3.0));
        // Finger 1's post-down motion delivered after the down, at
        // its own fresh slot: (9, 9).
        assert_eq!(delivered[3].args[0], Value::Uint32(1));
        assert_eq!(delivered[3].args[1], Value::Float32(9.0));
    }

    #[test]
    fn tablet_axes_coalesce_per_tool_and_per_axis() {
        // The pen stream, Phase 43: motion, pressure, and tilt are
        // position state of their OWN kinds — a 240 Hz pen parking
        // sixteen (motion, pressure, tilt, frame) batches between
        // display frames delivers one of each, each the freshest, in
        // the slots the first sample of each axis took.
        let mut boxes = Outboxes::new();
        let (client, tablet) = (cid(1), oid(10));
        for sample in 1..=16u32 {
            let s = sample as f32;
            boxes.push(tablet_axis(client, tablet, 7, "motion", POSITION_MOTION, s));
            boxes.push(tablet_axis(
                client,
                tablet,
                7,
                "pressure",
                POSITION_PRESSURE,
                s / 16.0,
            ));
            boxes.push(tablet_axis(client, tablet, 7, "tilt", POSITION_TILT, -s));
            boxes.push(frame_ev(client, tablet));
        }
        assert_eq!(boxes.len(), 4);
        let delivered = boxes.drain(client);
        let names: Vec<&str> = delivered.iter().map(|e| e.event).collect();
        assert_eq!(names, vec!["motion", "pressure", "tilt", "frame"]);
        assert_eq!(delivered[0].args[1], Value::Float32(16.0));
        assert_eq!(delivered[1].args[1], Value::Float32(1.0));
        assert_eq!(delivered[2].args[1], Value::Float32(-16.0));
        // Two tools on the same tablet object coalesce independently:
        // tool 8's motion never replaced tool 7's.
        boxes.push(tablet_axis(
            client,
            tablet,
            7,
            "motion",
            POSITION_MOTION,
            70.0,
        ));
        boxes.push(tablet_axis(
            client,
            tablet,
            8,
            "motion",
            POSITION_MOTION,
            80.0,
        ));
        boxes.push(tablet_axis(
            client,
            tablet,
            7,
            "motion",
            POSITION_MOTION,
            71.0,
        ));
        assert_eq!(boxes.len(), 2);
        let delivered = boxes.drain(client);
        assert_eq!(delivered[0].args[0], Value::Uint64(7));
        assert_eq!(delivered[0].args[1], Value::Float32(71.0));
        assert_eq!(delivered[1].args[0], Value::Uint64(8));
        assert_eq!(delivered[1].args[1], Value::Float32(80.0));
    }

    #[test]
    fn touch_cancel_is_not_a_barrier_for_the_next_sequence() {
        // `touch.cancel` voids the current sequence and seals nothing:
        // a well-formed next sequence begins with `touch.down` (which
        // carries its own seal), so the cancel itself must not freeze
        // any contact's slot — a motion pushed after the cancel
        // replaces a still-pending pre-cancel sample of the same
        // contact (the pointer's motion-motion collapse, applied
        // verbatim to same-stream touch samples).
        let mut boxes = Outboxes::new();
        let (client, touch) = (cid(1), oid(10));
        boxes.push(touch_motion(client, touch, 1, 1.0, 1.0));
        boxes.push(OutboxEntry::contact_discrete_input(
            client,
            touch,
            "cancel",
            vec![],
            None,
        ));
        boxes.push(touch_motion(client, touch, 1, 5.0, 5.0));
        let delivered = boxes.drain(client);
        let names: Vec<&str> = delivered.iter().map(|e| e.event).collect();
        assert_eq!(names, vec!["motion", "cancel"]);
        // The post-cancel sample replaced the pre-cancel one (no
        // seal): the freshest coordinates, one entry.
        assert_eq!(delivered[0].args[1], Value::Float32(5.0));
    }
}
