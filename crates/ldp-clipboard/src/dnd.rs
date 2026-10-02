//! The drag-and-drop state machine.
//!
//! One machine per seat, driven by the seat's pointer and the
//! receiver's `data_offer` requests. The locked negotiation semantics
//! (the reading of `spec/ldp.data` this crate implements):
//!
//! * A drag starts with **all actions available** (copy, move, ask).
//! * The entered receiver narrows the acceptable set with
//!   `data_offer.set_actions`; the server announces the current set
//!   to the *source* with `data_source.actions` events (the source
//!   renders its drag UI from these).
//! * At **drop** the compositor picks the highest-priority action in
//!   the narrowed set — `copy` over `move` over `ask` (copy is
//!   non-destructive; deterministic preference, never coin flips) —
//!   announces it to the receiver (`data_offer.action`), announces
//!   the singleton set to the source (its last `actions` event is the
//!   negotiated action — `dnd_finished` carries no argument), then
//!   delivers `data_source.dnd_drop_performed`.
//! * The receiver streams the payload (`receive`) and calls
//!   `data_offer.finish`, which ends the drag with
//!   `data_source.dnd_finished`.
//! * A narrowed set that is **empty** means the receiver declines; a
//!   drop onto a declining receiver ends the drag with
//!   `data_source.cancelled` and no drop events.
//! * `ask` is pickable like any other; the integrator may resolve it
//!   to a concrete action (user prompt) before the finish.
//!
//! Cancel paths: the user releases outside any receiver, the origin
//! surface dies, the source is destroyed, or the integrator cancels —
//! every path ends `Idle` + `cancelled` to the source.
//!
//! Pure policy: the machine reasons in opaque keys; the manager maps
//! transitions to wire events.

#![forbid(unsafe_code)]

use crate::source::SourceKey;

/// Opaque client identity (the integrator's connection key).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct ClientKey(pub u64);

/// Opaque surface identity.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct SurfaceKey(pub u64);

/// A set of drag actions (the `dnd_actions` bitset: copy=bit0,
/// move=bit1, ask=bit2).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ActionSet(u8);

impl ActionSet {
    /// The empty set (receiver declines).
    pub const NONE: ActionSet = ActionSet(0);
    /// All v1 actions.
    pub const ALL: ActionSet = ActionSet(0b111);

    /// Build from discrete flags.
    #[must_use]
    pub const fn build(copy: bool, mv: bool, ask: bool) -> ActionSet {
        ActionSet((copy as u8) | ((mv as u8) << 1) | ((ask as u8) << 2))
    }

    /// Whether `copy` is present.
    #[must_use]
    pub const fn copy(self) -> bool {
        self.0 & 0b001 != 0
    }

    /// Whether `move` is present.
    #[must_use]
    pub const fn move_(self) -> bool {
        self.0 & 0b010 != 0
    }

    /// Whether `ask` is present.
    #[must_use]
    pub const fn ask(self) -> bool {
        self.0 & 0b100 != 0
    }

    /// Whether the set is empty.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The wire word (bit N set per the spec's bitset layout).
    #[must_use]
    pub const fn to_bits(self) -> u32 {
        self.0 as u32
    }

    /// From a wire word (unknown high bits are masked off — forward
    /// compatibility, the bitset doctrine of `docs/spec-format.md`).
    #[must_use]
    pub const fn from_bits(word: u32) -> ActionSet {
        ActionSet((word as u8) & 0b111)
    }

    /// The highest-priority present action, copy over move over ask.
    #[must_use]
    pub const fn pick(self) -> Option<DndAction> {
        if self.copy() {
            Some(DndAction::Copy)
        } else if self.move_() {
            Some(DndAction::Move)
        } else if self.ask() {
            Some(DndAction::Ask)
        } else {
            None
        }
    }
}

/// The negotiated action (wire enum `dnd_action`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DndAction {
    /// `copy` = 1.
    Copy,
    /// `move` = 2.
    Move,
    /// `ask` = 3.
    Ask,
}

impl DndAction {
    /// The wire value.
    #[must_use]
    pub const fn to_wire(self) -> u32 {
        match self {
            DndAction::Copy => 1,
            DndAction::Move => 2,
            DndAction::Ask => 3,
        }
    }

    /// The singleton set holding just this action.
    #[must_use]
    pub const fn as_set(self) -> ActionSet {
        match self {
            DndAction::Copy => ActionSet::build(true, false, false),
            DndAction::Move => ActionSet::build(false, true, false),
            DndAction::Ask => ActionSet::build(false, false, true),
        }
    }
}

/// Where the drag is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DndPhase {
    /// No drag on this seat.
    Idle,
    /// Dragging; a receiver may be entered.
    Dragging,
    /// Dropped on an accepting receiver; payload streaming / awaiting
    /// `finish`.
    Dropped,
}

/// The entered receiver.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Target {
    /// The receiver's client.
    pub client: ClientKey,
    /// The surface the pointer is over.
    pub surface: SurfaceKey,
}

/// Why a drag operation was rejected.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DndError {
    /// A drag is already in flight on this seat.
    AlreadyDragging,
    /// No drag is in flight.
    NoDrag,
    /// The operation requires an entered receiver.
    NoTarget,
    /// `set_actions` arrived from a client that is not the entered
    /// receiver.
    WrongClient,
    /// `start_drag`'s serial does not reference a button press.
    BadSerial,
    /// The drag was already dropped (`set_actions` too late).
    AlreadyDropped,
    /// `finish` before the drop, or twice.
    NotDropped,
    /// `resolve_ask` when the picked action is not `ask`.
    NotAsk,
    /// The origin surface must be alive to start a drag.
    DeadOrigin,
}

impl std::fmt::Display for DndError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            DndError::AlreadyDragging => "a drag is already in flight",
            DndError::NoDrag => "no drag in flight",
            DndError::NoTarget => "the drag has no entered receiver",
            DndError::WrongClient => "not the entered receiver",
            DndError::BadSerial => "start_drag serial does not reference a press",
            DndError::AlreadyDropped => "the drag was already dropped",
            DndError::NotDropped => "the drag has not been dropped",
            DndError::NotAsk => "the negotiated action is not ask",
            DndError::DeadOrigin => "the origin surface is dead",
        };
        f.write_str(s)
    }
}

impl std::error::Error for DndError {}

/// The live drag state.
#[derive(Clone, Copy, Debug)]
pub struct Drag {
    /// The payload source.
    pub source: SourceKey,
    /// The surface the drag started from.
    pub origin: SurfaceKey,
    /// Optional drag-icon surface.
    pub icon: Option<SurfaceKey>,
    /// The entered receiver, if any.
    pub target: Option<Target>,
    /// The receiver-narrowed acceptable set (starts as ALL).
    pub acceptable: ActionSet,
    /// The action picked at drop time.
    pub picked: Option<DndAction>,
}

/// The seat's drag machine.
#[derive(Clone, Debug)]
pub struct DndMachine {
    phase: DndPhase,
    drag: Option<Drag>,
}

impl Default for DndMachine {
    fn default() -> Self {
        DndMachine::new()
    }
}

impl DndMachine {
    /// A machine with no drag.
    #[must_use]
    pub const fn new() -> DndMachine {
        DndMachine {
            phase: DndPhase::Idle,
            drag: None,
        }
    }

    /// The current phase.
    #[must_use]
    pub const fn phase(&self) -> DndPhase {
        self.phase
    }

    /// The live drag (`None` when idle).
    #[must_use]
    pub const fn drag(&self) -> Option<&Drag> {
        self.drag.as_ref()
    }

    /// `start_drag`: the serial must reference the seat's current
    /// button-press serial (the integrator supplies the record — the
    /// `Popup::grab` precedent).
    ///
    /// # Errors
    ///
    /// [`DndError::AlreadyDragging`], [`DndError::BadSerial`].
    pub fn start(
        &mut self,
        source: SourceKey,
        origin: SurfaceKey,
        icon: Option<SurfaceKey>,
        press_serial: Option<u32>,
        serial: u32,
    ) -> Result<(), DndError> {
        if self.phase != DndPhase::Idle {
            return Err(DndError::AlreadyDragging);
        }
        if press_serial != Some(serial) {
            return Err(DndError::BadSerial);
        }
        self.phase = DndPhase::Dragging;
        self.drag = Some(Drag {
            source,
            origin,
            icon,
            target: None,
            acceptable: ActionSet::ALL,
            picked: None,
        });
        Ok(())
    }

    /// The pointer entered a receiver surface. Legal only while
    /// dragging with no current target (the router calls `leave`
    /// first on surface changes).
    ///
    /// # Errors
    ///
    /// [`DndError::NoDrag`], [`DndError::NoTarget`] (already entered).
    pub fn enter(&mut self, target: Target) -> Result<(), DndError> {
        let drag = self.live()?;
        if drag.target.is_some() {
            return Err(DndError::NoTarget);
        }
        drag.target = Some(target);
        Ok(())
    }

    /// Pointer motion within the entered surface (position bookkeeping
    /// is the manager's; the machine only validates ordering).
    ///
    /// # Errors
    ///
    /// [`DndError::NoDrag`], [`DndError::NoTarget`].
    pub fn motion(&mut self) -> Result<(), DndError> {
        let drag = self.live()?;
        if drag.target.is_none() {
            return Err(DndError::NoTarget);
        }
        Ok(())
    }

    /// The pointer left the receiver. The acceptable set resets (a
    /// future receiver starts clean — no stale narrowing leaks
    /// between receivers).
    ///
    /// # Errors
    ///
    /// [`DndError::NoDrag`], [`DndError::NoTarget`].
    pub fn leave(&mut self) -> Result<(), DndError> {
        let drag = self.live()?;
        if drag.target.take().is_none() {
            return Err(DndError::NoTarget);
        }
        drag.acceptable = ActionSet::ALL;
        Ok(())
    }

    /// The receiver narrowed the acceptable set
    /// (`data_offer.set_actions`). Returns the set to announce to the
    /// source. Resets to `ALL` when called with an empty set twice?
    /// No — narrowing is absolute: the receiver states the exact set
    /// it accepts; an empty set declares decline.
    ///
    /// # Errors
    ///
    /// [`DndError::NoDrag`], [`DndError::NoTarget`],
    /// [`DndError::WrongClient`], [`DndError::AlreadyDropped`].
    pub fn set_actions(
        &mut self,
        client: ClientKey,
        actions: ActionSet,
    ) -> Result<ActionSet, DndError> {
        let drag = self.live()?;
        let target = drag.target.ok_or(DndError::NoTarget)?;
        if target.client != client {
            return Err(DndError::WrongClient);
        }
        drag.acceptable = actions;
        Ok(actions)
    }

    /// The user dropped. Returns the picked action (`None`: the
    /// receiver declined — the manager then treats the drag as
    /// cancelled). Moves to `Dropped` only on an accepting receiver.
    ///
    /// # Errors
    ///
    /// [`DndError::NoDrag`], [`DndError::NoTarget`].
    pub fn drop(&mut self) -> Result<Option<DndAction>, DndError> {
        let drag = self.live()?;
        if drag.target.is_none() {
            return Err(DndError::NoTarget);
        }
        let picked = drag.acceptable.pick();
        let Some(action) = picked else {
            // Declined: the drag dies here (cancelled to the source,
            // leave to the receiver — the manager's job).
            self.phase = DndPhase::Idle;
            self.drag = None;
            return Ok(None);
        };
        drag.picked = picked;
        // The negotiated action is final for this drag: the
        // acceptable set becomes the announced singleton.
        drag.acceptable = action.as_set();
        self.phase = DndPhase::Dropped;
        Ok(picked)
    }

    /// The receiver's `data_offer.finish`: the drag completes with the
    /// negotiated action. The manager emits `dnd_finished` to the
    /// source.
    ///
    /// # Errors
    ///
    /// [`DndError::NotDropped`] (no drag, or still dragging).
    pub fn receiver_finish(&mut self) -> Result<DndAction, DndError> {
        if self.phase != DndPhase::Dropped {
            return Err(DndError::NotDropped);
        }
        let picked = self
            .drag
            .and_then(|d| d.picked)
            .ok_or(DndError::NotDropped)?;
        self.phase = DndPhase::Idle;
        self.drag = None;
        Ok(picked)
    }

    /// Resolve an `ask` pick to a concrete action (the integrator's
    /// user prompt). Only legal while dropped with `ask` pending.
    ///
    /// # Errors
    ///
    /// [`DndError::NotDropped`], [`DndError::NotAsk`].
    pub fn resolve_ask(&mut self, action: DndAction) -> Result<DndAction, DndError> {
        if self.phase != DndPhase::Dropped {
            return Err(DndError::NotDropped);
        }
        let drag = self.drag.as_mut().ok_or(DndError::NotDropped)?;
        if drag.picked != Some(DndAction::Ask) {
            return Err(DndError::NotAsk);
        }
        drag.picked = Some(action);
        drag.acceptable = action.as_set();
        Ok(action)
    }

    /// Cancel from any live phase (user released outside, origin
    /// died, integrator policy). Returns the source that must see
    /// `cancelled`, if any.
    pub fn cancel(&mut self) -> Option<SourceKey> {
        let source = self.drag.map(|d| d.source);
        self.phase = DndPhase::Idle;
        self.drag = None;
        source
    }

    /// A surface died. The target leaving clears the receiver; the
    /// origin or icon dying cancels the drag. Returns whether the
    /// drag was cancelled (the manager then emits `cancelled`).
    pub fn surface_gone(&mut self, surface: SurfaceKey) -> bool {
        let Some(drag) = self.drag.as_mut() else {
            return false;
        };
        if drag.target.is_some_and(|t| t.surface == surface) {
            drag.target = None;
            drag.acceptable = ActionSet::ALL;
            return false;
        }
        if drag.origin == surface || drag.icon == Some(surface) {
            self.cancel();
            return true;
        }
        false
    }

    fn live(&mut self) -> Result<&mut Drag, DndError> {
        if self.phase == DndPhase::Idle {
            return Err(DndError::NoDrag);
        }
        self.drag.as_mut().ok_or(DndError::NoDrag)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: SourceKey = SourceKey::new(7);
    const ORIGIN: SurfaceKey = SurfaceKey(1);
    const RECV_A: ClientKey = ClientKey(10);
    const SURF_A: SurfaceKey = SurfaceKey(2);

    #[test]
    fn action_set_algebra() {
        assert!(ActionSet::ALL.copy() && ActionSet::ALL.move_() && ActionSet::ALL.ask());
        assert!(ActionSet::NONE.is_empty());
        assert_eq!(ActionSet::ALL.to_bits(), 0b111);
        assert_eq!(ActionSet::from_bits(0b1111_0111), ActionSet::ALL);
        assert_eq!(
            ActionSet::build(true, false, false).pick(),
            Some(DndAction::Copy)
        );
        assert_eq!(
            ActionSet::build(false, true, true).pick(),
            Some(DndAction::Move)
        );
        assert_eq!(
            ActionSet::build(false, false, true).pick(),
            Some(DndAction::Ask)
        );
        assert_eq!(ActionSet::NONE.pick(), None);
        assert_eq!(DndAction::Move.as_set().pick(), Some(DndAction::Move));
    }

    #[test]
    fn full_happy_path() {
        let mut m = DndMachine::new();
        m.start(SRC, ORIGIN, None, Some(42), 42).unwrap();
        assert_eq!(m.phase(), DndPhase::Dragging);
        m.enter(Target {
            client: RECV_A,
            surface: SURF_A,
        })
        .unwrap();
        m.motion().unwrap();
        // Narrow to move|ask: copy outranked but absent.
        let set = m
            .set_actions(RECV_A, ActionSet::build(false, true, true))
            .unwrap();
        assert!(set.move_() && set.ask() && !set.copy());
        assert_eq!(m.drop().unwrap(), Some(DndAction::Move));
        assert_eq!(m.phase(), DndPhase::Dropped);
        // The acceptable set is the singleton after the pick.
        assert_eq!(m.drag().unwrap().acceptable, DndAction::Move.as_set());
        assert_eq!(m.receiver_finish().unwrap(), DndAction::Move);
        assert_eq!(m.phase(), DndPhase::Idle);
        assert!(m.drag().is_none());
    }

    #[test]
    fn bad_serial_rejected() {
        let mut m = DndMachine::new();
        assert_eq!(
            m.start(SRC, ORIGIN, None, Some(1), 2).unwrap_err(),
            DndError::BadSerial
        );
        assert_eq!(
            m.start(SRC, ORIGIN, None, None, 2).unwrap_err(),
            DndError::BadSerial
        );
        assert_eq!(m.phase(), DndPhase::Idle);
    }

    #[test]
    fn double_start_rejected() {
        let mut m = DndMachine::new();
        m.start(SRC, ORIGIN, None, Some(5), 5).unwrap();
        assert_eq!(
            m.start(SourceKey::new(8), ORIGIN, None, Some(5), 5)
                .unwrap_err(),
            DndError::AlreadyDragging
        );
    }

    #[test]
    fn wrong_client_cannot_narrow() {
        let mut m = DndMachine::new();
        m.start(SRC, ORIGIN, None, Some(9), 9).unwrap();
        m.enter(Target {
            client: RECV_A,
            surface: SURF_A,
        })
        .unwrap();
        let other = ClientKey(11);
        assert_eq!(
            m.set_actions(other, ActionSet::NONE).unwrap_err(),
            DndError::WrongClient
        );
        // The entered receiver can.
        assert!(m.set_actions(RECV_A, ActionSet::NONE).is_ok());
    }

    #[test]
    fn declined_drop_cancels_drag() {
        let mut m = DndMachine::new();
        m.start(SRC, ORIGIN, None, Some(3), 3).unwrap();
        m.enter(Target {
            client: RECV_A,
            surface: SURF_A,
        })
        .unwrap();
        m.set_actions(RECV_A, ActionSet::NONE).unwrap();
        assert_eq!(m.drop().unwrap(), None);
        assert_eq!(m.phase(), DndPhase::Idle);
        // Finish is now meaningless.
        assert_eq!(m.receiver_finish().unwrap_err(), DndError::NotDropped);
    }

    #[test]
    fn leave_resets_narrowing() {
        let mut m = DndMachine::new();
        m.start(SRC, ORIGIN, None, Some(4), 4).unwrap();
        m.enter(Target {
            client: RECV_A,
            surface: SURF_A,
        })
        .unwrap();
        m.set_actions(RECV_A, ActionSet::build(true, false, false))
            .unwrap();
        m.leave().unwrap();
        assert_eq!(m.drag().unwrap().acceptable, ActionSet::ALL);
        // Re-enter: a fresh receiver sees no stale narrowing.
        m.enter(Target {
            client: ClientKey(12),
            surface: SurfaceKey(3),
        })
        .unwrap();
        assert_eq!(m.drag().unwrap().acceptable, ActionSet::ALL);
        // Leave is legal again, and the second consecutive leave is
        // the error.
        m.leave().unwrap();
        assert_eq!(m.leave().unwrap_err(), DndError::NoTarget);
    }

    #[test]
    fn ask_resolution() {
        let mut m = DndMachine::new();
        m.start(SRC, ORIGIN, None, Some(6), 6).unwrap();
        m.enter(Target {
            client: RECV_A,
            surface: SURF_A,
        })
        .unwrap();
        m.set_actions(RECV_A, ActionSet::build(false, false, true))
            .unwrap();
        assert_eq!(m.drop().unwrap(), Some(DndAction::Ask));
        assert_eq!(m.resolve_ask(DndAction::Copy).unwrap(), DndAction::Copy);
        assert_eq!(m.drag().unwrap().picked, Some(DndAction::Copy));
        assert_eq!(m.receiver_finish().unwrap(), DndAction::Copy);
        // Resolve on a non-ask pick is an error.
        let mut m2 = DndMachine::new();
        m2.start(SRC, ORIGIN, None, Some(6), 6).unwrap();
        m2.enter(Target {
            client: RECV_A,
            surface: SURF_A,
        })
        .unwrap();
        assert_eq!(m2.drop().unwrap(), Some(DndAction::Copy));
        assert_eq!(
            m2.resolve_ask(DndAction::Move).unwrap_err(),
            DndError::NotAsk
        );
    }

    #[test]
    fn surface_death_paths() {
        let mut m = DndMachine::new();
        m.start(SRC, ORIGIN, Some(SurfaceKey(9)), Some(8), 8)
            .unwrap();
        m.enter(Target {
            client: RECV_A,
            surface: SURF_A,
        })
        .unwrap();
        // Target death: just leaves.
        assert!(!m.surface_gone(SURF_A));
        assert!(m.drag().unwrap().target.is_none());
        // Origin death: cancels.
        assert!(m.surface_gone(ORIGIN));
        assert_eq!(m.phase(), DndPhase::Idle);
        // Icon death cancels too.
        let mut m2 = DndMachine::new();
        m2.start(SRC, ORIGIN, Some(SurfaceKey(9)), Some(8), 8)
            .unwrap();
        assert!(m2.surface_gone(SurfaceKey(9)));
        assert_eq!(m2.phase(), DndPhase::Idle);
    }

    #[test]
    fn drop_without_target_rejected() {
        let mut m = DndMachine::new();
        m.start(SRC, ORIGIN, None, Some(2), 2).unwrap();
        assert_eq!(m.drop().unwrap_err(), DndError::NoTarget);
        assert_eq!(m.motion().unwrap_err(), DndError::NoTarget);
    }
}
