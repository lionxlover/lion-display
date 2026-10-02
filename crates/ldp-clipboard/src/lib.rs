//! LDP clipboard layer: data sources/offers, MIME negotiation,
//! streaming transfers over pipes, primary selection, and the
//! drag-and-drop state machine.
//!
//! The layering (architecture §15): this crate implements the
//! `ldp.data` module as *exchange policy* over the foundation types
//! and the wire schema:
//!
//! * [`mime`] — MIME validation (the RFC 6838 subset), canonical
//!   form, charset folding for `text/plain`, and preference-ordered
//!   negotiation.
//! * [`source`] — [`source::DataSource`]: the offer-list machine and
//!   its attach/cancel/finish lifecycle.
//! * [`offer`] — [`offer::DataOffer`]: the receiver's mirror with
//!   accept/receive/set_actions/finish validation.
//! * [`device`] — [`device::SeatData`]: the per-seat selection and
//!   primary-selection slots with serial binding and ownership.
//! * [`dnd`] — [`dnd::DndMachine`]: the drag FSM and the action
//!   negotiation (receiver narrows; compositor picks copy > move >
//!   ask; the singleton announcement doctrine).
//! * [`transfer`] — the transfer registry (FD budget from
//!   `ldp_core::limits::Limits`) and the page-bounded pump: the
//!   server never buffers more than one page.
//! * [`pipe`] — the audited libc seam: `pipe2`/`fcntl`/`read`/`write`
//!   (the `ldp-input` sys precedent; every other module is
//!   `#![forbid(unsafe_code)]`).
//! * [`permission`] — the deny-by-default gates: clipboard *read*
//!   needs the `clipboard_read` scope; *setting* the selection and
//!   *receiving drops* are user-intent-driven.
//! * [`manager`] — [`manager::ClipboardManager`]: the cross-client
//!   coordinator; every protocol request evaluates to a routed
//!   event batch.
//! * [`drag`] — the drag choreography on the manager (enter/motion/
//!   drop/leave routing, cancel funnels).
//! * [`event`] — the typed data-event vocabulary and its encoding
//!   against the compiled protocol schema.
//!
//! Timing doctrine: no code in this crate reads a clock. I/O
//! doctrine: everything is pure policy except [`pipe`], the single
//! audited syscall boundary.
//!
//! Safety doctrine: every module carries its own
//! `#![forbid(unsafe_code)]` — all except [`pipe`], the audited
//! boundary (the `ldp-input`/`ldp-display` precedent: a crate-level
//! forbid would forbid the one module that legitimately needs it).

pub mod device;
pub mod dnd;
pub mod drag;
pub mod event;
pub mod manager;
pub mod mime;
pub mod offer;
pub mod permission;
pub mod pipe;
pub mod source;
pub mod transfer;

pub use device::{DeviceError, SeatData, SelectionOwner, Slot};
pub use dnd::{
    ActionSet, ClientKey, DndAction, DndError, DndMachine, DndPhase, SurfaceKey, Target,
};
pub use drag::{DragStartError, FinishError};
pub use event::DataEvent;
pub use manager::{ClipboardManager, ObjectSupplier, ReceiveError, Routed, SeatKey};
pub use mime::{Mime, MimeError, MAX_PARAMS as MIME_MAX_PARAMS};
pub use offer::{DataOffer, OfferError, OfferKey, OfferKind};
pub use permission::{AllowBasis, GateDecision, GateOp, GateRecord};
pub use source::{Attachment, DataSource, SourceError, SourceKey, MAX_OFFERS};
pub use transfer::{
    AdmissionError, PumpStep, ReadHalf, Transfer, TransferId, TransferRegistry, TransferState,
    WriteHalf, PAGE,
};
