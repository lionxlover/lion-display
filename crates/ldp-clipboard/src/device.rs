//! The per-seat selection model: clipboard and primary selection.
//!
//! One [`SeatData`] per seat (architecture §14: two seats never share
//! focus, clipboard, or spaces). Each seat carries two slots — the
//! clipboard selection and the primary (middle-click) selection —
//! unified in LDP as the same mechanism with two names.
//!
//! Rules enforced here:
//!
//! * Setting a slot requires a serial referencing an input event of
//!   the seat (integrator-supplied record — the `Popup::grab`
//!   precedent).
//! * The setter's source must be a live, unattached source *of that
//!   client*; attaching evicts the previous owner (it sees
//!   `cancelled`).
//! * Clearing with a null source only works for the current owner
//!   (a client cannot clear another client's selection).
//! * Destroying the owner source clears the slot (receivers see the
//!   null selection event).
//!
//! The DnD machine is owned here too — one drag per seat, and a drag
//! starting on a seat implicitly evicts nothing (drag and selection
//! coexist: the same source may even be both, attached twice is not
//! allowed though — a second attach is refused; clients create a
//! fresh source per role).

#![forbid(unsafe_code)]

use crate::dnd::{ClientKey, DndMachine};
use crate::source::{Attachment, SourceError, SourceKey};

/// Which slot an operation addresses.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Slot {
    /// The clipboard selection.
    Clipboard,
    /// The primary selection.
    Primary,
}

/// Why a device request was rejected.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DeviceError {
    /// The serial does not reference an input event of this seat.
    BadSerial,
    /// The source belongs to another client.
    ForeignSource,
    /// The null clear arrived from a client that does not own the
    /// slot.
    NotOwner,
    /// The source is dead or already attached elsewhere.
    SourceUnavailable(SourceError),
    /// Unknown source key.
    UnknownSource,
}

impl std::fmt::Display for DeviceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeviceError::BadSerial => f.write_str("serial does not reference this seat's input"),
            DeviceError::ForeignSource => f.write_str("the data source belongs to another client"),
            DeviceError::NotOwner => f.write_str("only the owner may clear the selection"),
            DeviceError::SourceUnavailable(e) => write!(f, "data source unusable: {e}"),
            DeviceError::UnknownSource => f.write_str("unknown data source"),
        }
    }
}

impl std::error::Error for DeviceError {}

/// The current holder of one slot.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SelectionOwner {
    /// The owning client.
    pub client: ClientKey,
    /// The attached source.
    pub source: SourceKey,
}

/// One seat's data state.
#[derive(Clone, Debug)]
pub struct SeatData {
    clipboard: Option<SelectionOwner>,
    primary: Option<SelectionOwner>,
    /// The drag machine (one drag per seat).
    pub dnd: DndMachine,
}

impl SeatData {
    /// A seat with empty slots and no drag.
    #[must_use]
    pub fn new() -> SeatData {
        SeatData {
            clipboard: None,
            primary: None,
            dnd: DndMachine::new(),
        }
    }

    /// The clipboard owner.
    #[must_use]
    pub const fn clipboard(&self) -> Option<SelectionOwner> {
        self.clipboard
    }

    /// The primary-selection owner.
    #[must_use]
    pub const fn primary(&self) -> Option<SelectionOwner> {
        self.primary
    }

    /// `set_selection`/`set_primary_selection` rule 1: the serial
    /// must reference an input event of this seat.
    ///
    /// # Errors
    ///
    /// [`DeviceError::BadSerial`] on mismatch.
    pub fn check_serial(input_serial: Option<u32>, serial: u32) -> Result<(), DeviceError> {
        if input_serial == Some(serial) {
            Ok(())
        } else {
            Err(DeviceError::BadSerial)
        }
    }

    /// The current owner of one slot.
    #[must_use]
    pub const fn owner(&self, slot: Slot) -> Option<SelectionOwner> {
        match slot {
            Slot::Clipboard => self.clipboard,
            Slot::Primary => self.primary,
        }
    }

    /// Install a new owner (the manager has already validated the
    /// source: live, owned by the same client, unattached, and
    /// attached to this slot's role). Returns the evicted owner.
    pub fn install(&mut self, slot: Slot, owner: SelectionOwner) -> Option<SelectionOwner> {
        match slot {
            Slot::Clipboard => self.clipboard.replace(owner),
            Slot::Primary => self.primary.replace(owner),
        }
    }

    /// Null clear: only the current owner may clear a held slot.
    /// Returns the cleared owner.
    ///
    /// # Errors
    ///
    /// [`DeviceError::NotOwner`] when a non-owner clears a held
    /// slot (clearing an empty slot is a no-op `Ok(None)`).
    pub fn clear(
        &mut self,
        slot: Slot,
        client: ClientKey,
    ) -> Result<Option<SelectionOwner>, DeviceError> {
        if let Some(owner) = self.owner(slot) {
            if owner.client != client {
                return Err(DeviceError::NotOwner);
            }
            match slot {
                Slot::Clipboard => self.clipboard = None,
                Slot::Primary => self.primary = None,
            }
            return Ok(Some(owner));
        }
        Ok(None)
    }

    /// The attachment role of one slot (source-table validation
    /// companion).
    #[must_use]
    pub const fn slot_attachment(slot: Slot) -> Attachment {
        match slot {
            Slot::Clipboard => Attachment::Selection,
            Slot::Primary => Attachment::Primary,
        }
    }

    /// A source was destroyed: clear whichever slot it owned (the
    /// receivers see the null event) and return the cleared slot.
    pub fn source_gone(&mut self, source: SourceKey) -> Option<Slot> {
        let mut cleared = None;
        if self.clipboard.is_some_and(|o| o.source == source) {
            self.clipboard = None;
            cleared = Some(Slot::Clipboard);
        }
        if self.primary.is_some_and(|o| o.source == source) {
            self.primary = None;
            cleared = match cleared {
                Some(Slot::Clipboard) => cleared, // both were held; report first
                _ => Some(Slot::Primary),
            };
        }
        // A dead drag source cancels the drag too.
        if self.dnd.drag().is_some_and(|d| d.source == source) {
            self.dnd.cancel();
        }
        cleared
    }

    /// A client disconnected: clear its slots. Returns the cleared
    /// owners (their sources are gone with the client).
    pub fn client_gone(&mut self, client: ClientKey) -> Vec<SelectionOwner> {
        let mut gone = Vec::new();
        if self.clipboard.is_some_and(|o| o.client == client) {
            if let Some(o) = self.clipboard.take() {
                gone.push(o);
            }
        }
        if self.primary.is_some_and(|o| o.client == client) {
            if let Some(o) = self.primary.take() {
                gone.push(o);
            }
        }
        gone
    }
}

impl Default for SeatData {
    fn default() -> Self {
        SeatData::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: ClientKey = ClientKey(1);
    const B: ClientKey = ClientKey(2);
    const S1: SourceKey = SourceKey::new(10);
    const S2: SourceKey = SourceKey::new(11);

    fn owner_of(client: ClientKey, source: SourceKey) -> SelectionOwner {
        SelectionOwner { client, source }
    }

    #[test]
    fn install_and_evict() {
        let mut seat = SeatData::new();
        assert!(seat.clipboard().is_none());
        let evicted = seat.install(Slot::Clipboard, owner_of(A, S1));
        assert_eq!(evicted, None);
        assert_eq!(seat.owner(Slot::Clipboard), Some(owner_of(A, S1)));
        let evicted = seat.install(Slot::Clipboard, owner_of(B, S2));
        assert_eq!(evicted, Some(owner_of(A, S1)));
        assert_eq!(seat.owner(Slot::Clipboard), Some(owner_of(B, S2)));
    }

    #[test]
    fn serial_rule() {
        assert!(SeatData::check_serial(Some(7), 7).is_ok());
        assert_eq!(
            SeatData::check_serial(Some(7), 8).unwrap_err(),
            DeviceError::BadSerial
        );
        assert_eq!(
            SeatData::check_serial(None, 8).unwrap_err(),
            DeviceError::BadSerial
        );
    }

    #[test]
    fn null_clear_is_owner_only() {
        let mut seat = SeatData::new();
        seat.install(Slot::Clipboard, owner_of(A, S1));
        assert_eq!(
            seat.clear(Slot::Clipboard, B).unwrap_err(),
            DeviceError::NotOwner
        );
        let cleared = seat.clear(Slot::Clipboard, A).unwrap();
        assert_eq!(cleared, Some(owner_of(A, S1)));
        assert!(seat.clipboard().is_none());
        // Clearing an empty slot is a no-op.
        assert_eq!(seat.clear(Slot::Clipboard, A).unwrap(), None);
    }

    #[test]
    fn both_slots_coexist() {
        let mut seat = SeatData::new();
        seat.install(Slot::Clipboard, owner_of(A, S1));
        seat.install(Slot::Primary, owner_of(A, S2));
        assert_eq!(seat.owner(Slot::Clipboard).unwrap().source, S1);
        assert_eq!(seat.owner(Slot::Primary).unwrap().source, S2);
    }

    #[test]
    fn slot_roles_map_to_attachments() {
        assert_eq!(
            SeatData::slot_attachment(Slot::Clipboard),
            Attachment::Selection
        );
        assert_eq!(
            SeatData::slot_attachment(Slot::Primary),
            Attachment::Primary
        );
    }

    #[test]
    fn source_gone_clears_slots_and_drag() {
        let mut seat = SeatData::new();
        seat.install(Slot::Clipboard, owner_of(A, S1));
        seat.install(Slot::Primary, owner_of(A, S2));
        assert_eq!(seat.source_gone(S1), Some(Slot::Clipboard));
        assert!(seat.clipboard().is_none());
        assert_eq!(seat.primary().unwrap().source, S2);
        seat.dnd
            .start(S2, crate::dnd::SurfaceKey(1), None, Some(3), 3)
            .unwrap();
        seat.source_gone(S2);
        assert_eq!(seat.dnd.phase(), crate::dnd::DndPhase::Idle);
    }

    #[test]
    fn client_gone_clears_its_slots() {
        let mut seat = SeatData::new();
        seat.install(Slot::Clipboard, owner_of(A, S1));
        seat.install(Slot::Primary, owner_of(B, S2));
        let gone = seat.client_gone(B);
        assert_eq!(gone.len(), 1);
        assert_eq!(gone[0].client, B);
        assert!(seat.primary().is_none());
        assert_eq!(seat.clipboard().unwrap().client, A);
    }
}
