//! The dialog machine: transient windows attached to a toplevel.
//!
//! A dialog is a toplevel's child window (the shell treats it as a
//! separate role because its lifecycle, geometry policy, and input
//! gating differ): modal dialogs gate input delivery to their parent's
//! whole tree until closed; centering is server-side per LionOS policy
//! (centered over the parent's *content* area, clamped into the
//! workspace area); configuration reuses the two-phase commit with the
//! dialog's own serial clock.

#![forbid(unsafe_code)]

use ldp_core::geometry::Rect;

use crate::serial::{Serial, SerialClock};
use crate::ssd::bounded_string;
use crate::WindowKey;

/// Dialog modality (wire `dialog_modality`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DialogModality {
    /// Gates input to the parent's tree (wire 1).
    Modal,
    /// Independent input (wire 2).
    Modeless,
}

impl DialogModality {
    /// The wire value.
    #[must_use]
    pub const fn wire(self) -> u32 {
        match self {
            DialogModality::Modal => 1,
            DialogModality::Modeless => 2,
        }
    }

    /// From the wire value.
    #[must_use]
    pub const fn from_wire(v: u32) -> Option<DialogModality> {
        match v {
            1 => Some(DialogModality::Modal),
            2 => Some(DialogModality::Modeless),
            _ => None,
        }
    }
}

/// Dialog misuse.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DialogError {
    /// The acked serial is not the live proposal.
    StaleAck,
    /// A string argument was rejected.
    BadString,
}

impl std::fmt::Display for DialogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DialogError::StaleAck => f.write_str("ack_configure serial is not the live proposal"),
            DialogError::BadString => f.write_str("string argument rejected"),
        }
    }
}

impl std::error::Error for DialogError {}

/// One dialog's size proposal.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DialogState {
    /// The proposal serial.
    pub serial: Serial,
    /// Content width (0 = client chooses).
    pub width: u32,
    /// Content height (0 = client chooses).
    pub height: u32,
}

/// A transient dialog attached to a toplevel.
#[derive(Debug)]
pub struct Dialog {
    key: WindowKey,
    parent: WindowKey,
    modality: DialogModality,
    serials: SerialClock,
    /// The live proposal.
    pending: Option<DialogState>,
    /// The last acked proposal (realized by the next commit).
    acked: Option<DialogState>,
    /// Realized state.
    applied: Option<DialogState>,
    title: Box<str>,
    closed: bool,
}

impl Dialog {
    /// A new dialog attached to `parent`.
    #[must_use]
    pub fn new(key: WindowKey, parent: WindowKey, modality: DialogModality) -> Dialog {
        Dialog {
            key,
            parent,
            modality,
            serials: SerialClock::new(),
            pending: None,
            acked: None,
            applied: None,
            title: Box::from(""),
            closed: false,
        }
    }

    /// The dialog key.
    #[must_use]
    pub const fn key(&self) -> WindowKey {
        self.key
    }

    /// The parent toplevel's key.
    #[must_use]
    pub const fn parent(&self) -> WindowKey {
        self.parent
    }

    /// The modality.
    #[must_use]
    pub const fn modality(&self) -> DialogModality {
        self.modality
    }

    /// The live proposal.
    #[must_use]
    pub fn pending(&self) -> Option<DialogState> {
        self.pending
    }

    /// The last acked proposal.
    #[must_use]
    pub fn acked(&self) -> Option<DialogState> {
        self.acked
    }

    /// The realized state.
    #[must_use]
    pub fn applied(&self) -> Option<DialogState> {
        self.applied
    }

    /// The title.
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Whether the user asked to close.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// `set_title`.
    ///
    /// # Errors
    /// [`DialogError::BadString`] when the title is rejected.
    pub fn set_title(&mut self, title: &str) -> Result<(), DialogError> {
        self.title = bounded_string(title).map_err(|_| DialogError::BadString)?;
        Ok(())
    }

    /// Whether this dialog gates input to `window`'s tree: modal and
    /// `window` is the parent (or the dialog itself — the dialog takes
    /// its own input).
    #[must_use]
    pub fn gates(&self, window: WindowKey) -> bool {
        self.modality == DialogModality::Modal && window == self.parent && !self.closed
    }

    /// Propose a size: clamped to the workspace area (position policy
    /// is [`Dialog::center_over`], applied by the integrator).
    /// Returns the proposal to emit.
    #[must_use]
    pub fn propose(
        &mut self,
        size: (u32, u32),
        _parent_content: Rect,
        workspace_area: Rect,
    ) -> DialogState {
        let serial = self.serials.issue();
        let d = DialogState {
            serial,
            width: size.0.min(workspace_area.w),
            height: size.1.min(workspace_area.h),
        };
        self.serials.reserve(serial);
        self.pending = Some(d);
        d
    }

    /// The centered position (parent-relative derivation is the
    /// integrator's job; the shell policy is: centered over the
    /// parent's content area, clamped into the workspace area).
    #[must_use]
    pub fn center_over(parent_content: Rect, size: (u32, u32), workspace_area: Rect) -> (i32, i32) {
        let w = size.0.min(workspace_area.w);
        let h = size.1.min(workspace_area.h);
        let cx = parent_content.x + parent_content.w as i32 / 2;
        let cy = parent_content.y + parent_content.h as i32 / 2;
        let x = (cx - w as i32 / 2).clamp(workspace_area.x, workspace_area.right() - w as i32);
        let y = (cy - h as i32 / 2).clamp(workspace_area.y, workspace_area.bottom() - h as i32);
        (x, y)
    }

    /// `ack_configure`.
    ///
    /// # Errors
    /// [`DialogError::StaleAck`] on any non-live serial.
    pub fn ack_configure(&mut self, serial: Serial) -> Result<(), DialogError> {
        match self.pending {
            Some(p) if p.serial == serial => {
                self.acked = Some(p);
                self.pending = None;
                Ok(())
            }
            _ => Err(DialogError::StaleAck),
        }
    }

    /// The client's commit: realize the acked proposal.
    #[must_use]
    pub fn commit(&mut self) -> Option<DialogState> {
        if let Some(a) = self.acked {
            self.acked = None;
            self.applied = Some(a);
            Some(a)
        } else {
            None
        }
    }

    /// The user asked to close (the `close` event).
    pub fn close(&mut self) {
        self.closed = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modality_wire_round_trip() {
        assert_eq!(DialogModality::Modal.wire(), 1);
        assert_eq!(DialogModality::Modeless.wire(), 2);
        assert_eq!(DialogModality::from_wire(1), Some(DialogModality::Modal));
        assert_eq!(DialogModality::from_wire(2), Some(DialogModality::Modeless));
        assert_eq!(DialogModality::from_wire(0), None);
    }

    #[test]
    fn modal_gates_only_the_parent_tree() {
        let parent = WindowKey::new(1);
        let dialog = Dialog::new(WindowKey::new(2), parent, DialogModality::Modal);
        assert!(dialog.gates(parent));
        assert!(!dialog.gates(WindowKey::new(3)));
        assert!(!dialog.gates(WindowKey::new(2)));
        let modeless = Dialog::new(WindowKey::new(2), parent, DialogModality::Modeless);
        assert!(!modeless.gates(parent));
    }

    #[test]
    fn closed_modal_no_longer_gates() {
        let parent = WindowKey::new(1);
        let mut d = Dialog::new(WindowKey::new(2), parent, DialogModality::Modal);
        d.close();
        assert!(d.is_closed());
        assert!(!d.gates(parent));
    }

    #[test]
    fn dialog_commit_lifecycle() {
        let mut d = Dialog::new(WindowKey::new(2), WindowKey::new(1), DialogModality::Modal);
        let ws = Rect::new(0, 0, 1920, 1040);
        let pc = Rect::new(100, 100, 800, 600);
        let p1 = d.propose((400, 300), pc, ws);
        assert_eq!(p1.serial, Serial(1));
        assert_eq!(d.ack_configure(Serial(4)), Err(DialogError::StaleAck));
        d.ack_configure(p1.serial).unwrap();
        assert_eq!(d.commit(), Some(p1));
        assert_eq!(d.commit(), None);
        assert_eq!(d.ack_configure(p1.serial), Err(DialogError::StaleAck));
    }

    #[test]
    fn proposal_clamps_to_the_workspace() {
        let mut d = Dialog::new(
            WindowKey::new(2),
            WindowKey::new(1),
            DialogModality::Modeless,
        );
        let ws = Rect::new(0, 0, 800, 600);
        let p = d.propose((2000, 1500), Rect::new(0, 0, 800, 600), ws);
        assert_eq!((p.width, p.height), (800, 600));
    }

    #[test]
    fn centering_follows_parent_content_and_clamps() {
        let ws = Rect::new(0, 0, 1920, 1040);
        // Centered over a parent's content area.
        let pc = Rect::new(560, 220, 800, 600);
        assert_eq!(Dialog::center_over(pc, (400, 300), ws), (760, 370));
        // A dialog near the workspace edge clamps inside.
        let pc2 = Rect::new(1600, 700, 300, 300);
        let (x, y) = Dialog::center_over(pc2, (400, 300), ws);
        assert!(x >= 0 && x + 400 <= 1920);
        assert!(y >= 0 && y + 300 <= 1040);
        // Center (1750, 850) minus half-size → (1550, 700); the x
        // clamps to 1520, y fits unclamped.
        assert_eq!((x, y), (1520, 700));
        // Bigger than the workspace: fills it, anchored at origin.
        assert_eq!(Dialog::center_over(pc, (3000, 3000), ws), (0, 0));
    }

    #[test]
    fn titles_are_bounded() {
        let mut d = Dialog::new(WindowKey::new(2), WindowKey::new(1), DialogModality::Modal);
        d.set_title("Save changes?").unwrap();
        assert_eq!(d.title(), "Save changes?");
        assert_eq!(d.set_title(&"y".repeat(5000)), Err(DialogError::BadString));
        assert_eq!(d.title(), "Save changes?");
    }
}
