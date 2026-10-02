//! The clipboard manager: the cross-client coordinator.
//!
//! One [`ClipboardManager`] per server. It owns the source, offer,
//! and device tables, the per-seat [`SeatData`], and the transfer
//! registry; every method is a protocol request (or integrator
//! transition, like pointer-driven DnD) evaluated as *pure policy*,
//! returning the exact [`Routed`] event batch the integrator must
//! deliver — including which client, which object, and (for `send`)
//! which FD to attach. The integrator never has to re-derive policy;
//! it only maps keys to connections and writes bytes.
//!
//! Object identity: the manager works with the protocol
//! [`ObjectId`]s the integrator supplies at object creation (the
//! Phase 4 object store owns allocation); fresh offer ids come from
//! the `next_object` supplier closure each publication needs (one
//! per receiving client — object ids are per-connection namespaces).
//!
//! Publication rules (pinned):
//!
//! * A selection/primary change publishes an offer to every client
//!   with a device on that seat **except the owner** (the owner
//!   knows its own content; announcing it would only invite
//!   self-paste loops).
//! * A drag publishes **one** offer for the whole drag, created at
//!   the first `enter`, reused for every later enter of the same
//!   drag, killed when the drag ends.
//! * Killing an offer cancels its in-flight transfers.

#![forbid(unsafe_code)]

use crate::device::{DeviceError, SeatData, Slot};
use crate::dnd::ClientKey;
use crate::event::DataEvent;
use crate::mime::Mime;
use crate::offer::{DataOffer, OfferError, OfferKey, OfferKind};
use crate::permission::{self, GateOp, GateRecord};
use crate::source::{DataSource, SourceError, SourceKey};
use crate::transfer::{AdmissionError, TransferId, TransferRegistry};
use ldp_core::caps::ScopeSet;
use ldp_core::ids::ObjectId;
use ldp_core::limits::Limits;
use ldp_core::token::AccessToken;
use std::collections::BTreeMap;
use std::os::fd::OwnedFd;

/// Opaque seat identity.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct SeatKey(pub u64);

/// One event the integrator must deliver.
#[derive(Debug)]
pub struct Routed {
    /// The receiving client.
    pub client: ClientKey,
    /// The emitting protocol object.
    pub object: ObjectId,
    /// The typed event.
    pub event: DataEvent,
    /// The ancillary FD to attach (`send` events; exactly the pipe
    /// write end the receiver created).
    pub fd: Option<OwnedFd>,
}

impl Routed {
    pub(crate) fn new(client: ClientKey, object: ObjectId, event: DataEvent) -> Routed {
        Routed {
            client,
            object,
            event,
            fd: None,
        }
    }
}

/// A client's device on one seat (one per seat per client).
#[derive(Clone, Debug)]
pub(crate) struct DeviceState {
    pub(crate) obj: ObjectId,
    pub(crate) selection_offer: Option<OfferKey>,
    pub(crate) primary_offer: Option<OfferKey>,
    pub(crate) drag_offer: Option<OfferKey>,
}

/// A tracked source: owner + protocol object + machine.
#[derive(Clone, Debug)]
pub(crate) struct SourceEntry {
    /// Owning client.
    pub(crate) client: ClientKey,
    /// Protocol object id.
    pub(crate) obj: ObjectId,
    /// The machine.
    pub(crate) machine: DataSource,
}

/// A tracked offer: receiving client + protocol object + machine.
#[derive(Clone, Debug)]
pub(crate) struct OfferEntry {
    /// Receiving client.
    pub(crate) client: ClientKey,
    /// Protocol object id.
    pub(crate) obj: ObjectId,
    /// The machine.
    pub(crate) machine: DataOffer,
    /// The seat the offer was published on.
    pub(crate) seat: SeatKey,
}

/// Why a receive was rejected.
#[derive(Clone, Debug)]
pub enum ReceiveError {
    /// The permission gate denied it (the record joins the audit
    /// trail; the dispatcher answers `unauthorized`).
    Denied(GateRecord),
    /// Offer-level rejection.
    Offer(OfferError),
    /// Unknown offer key.
    UnknownOffer,
    /// The transfer registry refused admission.
    Admission(AdmissionError),
}

impl std::fmt::Display for ReceiveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReceiveError::Denied(r) => write!(f, "denied by the permission gate: {r:?}"),
            ReceiveError::Offer(e) => write!(f, "{e}"),
            ReceiveError::UnknownOffer => f.write_str("unknown data offer"),
            ReceiveError::Admission(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ReceiveError {}

/// The manager (one per server).
#[derive(Clone, Debug)]
pub struct ClipboardManager {
    limits: Limits,
    pub(crate) sources: BTreeMap<SourceKey, SourceEntry>,
    pub(crate) offers: BTreeMap<OfferKey, OfferEntry>,
    pub(crate) devices: BTreeMap<(ClientKey, SeatKey), DeviceState>,
    pub(crate) seats: BTreeMap<SeatKey, SeatData>,
    transfers: TransferRegistry,
    pub(crate) next_source: u64,
    pub(crate) next_offer: u64,
    next_transfer: u64,
}

/// Supplier of fresh per-client object ids (the integrator's object
/// store allocation).
pub type ObjectSupplier<'a> = dyn FnMut(ClientKey) -> ObjectId + 'a;

impl ClipboardManager {
    /// An empty manager under `limits`.
    #[must_use]
    pub const fn new(limits: Limits) -> ClipboardManager {
        ClipboardManager {
            limits,
            sources: BTreeMap::new(),
            offers: BTreeMap::new(),
            devices: BTreeMap::new(),
            seats: BTreeMap::new(),
            transfers: TransferRegistry::new(limits),
            next_source: 0,
            next_offer: 0,
            next_transfer: 0,
        }
    }

    /// The limits in force.
    #[must_use]
    pub const fn limits(&self) -> Limits {
        self.limits
    }

    /// The transfer registry (read view).
    #[must_use]
    pub const fn transfers(&self) -> &TransferRegistry {
        &self.transfers
    }

    /// Mutable transfer registry (settle/advance from the pump
    /// driver).
    pub fn transfers_mut(&mut self) -> &mut TransferRegistry {
        &mut self.transfers
    }

    pub(crate) fn seat(&mut self, seat: SeatKey) -> &mut SeatData {
        self.seats.entry(seat).or_default()
    }

    /// `get_data_device` — create the device (one per seat per
    /// client; a duplicate re-binds the same object).
    pub fn create_device(&mut self, client: ClientKey, seat: SeatKey, obj: ObjectId) {
        self.devices.entry((client, seat)).or_insert(DeviceState {
            obj,
            selection_offer: None,
            primary_offer: None,
            drag_offer: None,
        });
        self.seat(seat);
    }

    /// `create_data_source` — register a fresh source. Returns its
    /// key.
    #[must_use]
    pub fn create_source(&mut self, client: ClientKey, obj: ObjectId) -> SourceKey {
        let key = SourceKey::new(self.next_source);
        self.next_source += 1;
        self.sources.insert(
            key,
            SourceEntry {
                client,
                obj,
                machine: DataSource::new(key),
            },
        );
        key
    }

    /// `data_source.offer` — add a MIME type.
    ///
    /// # Errors
    ///
    /// [`SourceError`] (late offer, bad MIME, flood).
    pub fn source_offer(&mut self, key: SourceKey, mime: &str) -> Result<(), SourceError> {
        self.sources
            .get_mut(&key)
            .ok_or(SourceError::Dead)?
            .machine
            .offer(mime, &self.limits)
    }

    /// A source's offered list (empty for unknown keys).
    #[must_use]
    pub fn source_offers(&self, key: SourceKey) -> &[Mime] {
        self.sources.get(&key).map_or(&[], |e| e.machine.offers())
    }

    /// The owner of a source.
    #[must_use]
    pub fn source_client(&self, key: SourceKey) -> Option<ClientKey> {
        self.sources.get(&key).map(|e| e.client)
    }

    /// The protocol object of a source.
    #[must_use]
    pub fn source_object(&self, key: SourceKey) -> Option<ObjectId> {
        self.sources.get(&key).map(|e| e.obj)
    }

    /// The dispatch path's reverse lookup: a request arrived on a
    /// source object — find the key behind it (ownership-checked:
    /// a borrowed guess from another client resolves to nothing).
    #[must_use]
    pub fn source_of_object(&self, client: ClientKey, obj: ObjectId) -> Option<SourceKey> {
        self.sources
            .iter()
            .find(|(_, e)| e.client == client && e.obj == obj)
            .map(|(k, _)| *k)
    }

    /// The dispatch path's reverse lookup: a request arrived on an
    /// offer object — find the key behind it (ownership-checked).
    #[must_use]
    pub fn offer_of_object(&self, client: ClientKey, obj: ObjectId) -> Option<OfferKey> {
        self.offers
            .iter()
            .find(|(_, e)| e.client == client && e.obj == obj)
            .map(|(k, _)| *k)
    }

    /// The dispatch path's reverse lookup: a request arrived on a
    /// data device object — find the seat it serves. A device is one
    /// per seat per client, so the object names the seat exactly.
    #[must_use]
    pub fn seat_of_device(&self, client: ClientKey, obj: ObjectId) -> Option<SeatKey> {
        self.devices
            .iter()
            .find(|((c, _), d)| *c == client && d.obj == obj)
            .map(|((_, s), _)| *s)
    }

    /// `data_offer` destruction by its receiver: the offer dies and
    /// the device's slot pointer clears (a stale pointer would hand
    /// [`Self::clipboard_offer`] a dead key). No events route: the
    /// receiver owned the object it destroyed, and selection offers
    /// carry no follow-up. (Drag offers never outlive their
    /// choreography in the served path — the drag FSM cancels
    /// through its own routes.)
    pub fn offer_destroy(&mut self, client: ClientKey, obj: ObjectId) -> Vec<Routed> {
        let Some(key) = self.offer_of_object(client, obj) else {
            return Vec::new();
        };
        let seat = self.offers.get(&key).map(|e| e.seat);
        self.kill_offer(key);
        if let Some(seat) = seat {
            if let Some(d) = self.devices.get_mut(&(client, seat)) {
                if d.selection_offer == Some(key) {
                    d.selection_offer = None;
                }
                if d.primary_offer == Some(key) {
                    d.primary_offer = None;
                }
                if d.drag_offer == Some(key) {
                    d.drag_offer = None;
                }
            }
        }
        Vec::new()
    }

    /// `set_selection` / `set_primary_selection`.
    ///
    /// On success returns the routed batch: `cancelled` to an evicted
    /// owner, and the offer publication + `selection` /
    /// `primary_selection` events to every other device client on
    /// the seat.
    ///
    /// # Errors
    ///
    /// [`DeviceError`] per the seat rules (serial, ownership,
    /// source liveness/attachment).
    // The argument list mirrors the wire request one-for-one
    // (slot, source, serial) plus the integrator context (seat,
    // client, record, object supplier); grouping them into a struct
    // would obscure the protocol correspondence.
    #[allow(clippy::too_many_arguments)]
    pub fn set_slot(
        &mut self,
        seat: SeatKey,
        slot: Slot,
        client: ClientKey,
        source: Option<SourceKey>,
        input_serial: Option<u32>,
        serial: u32,
        next_object: &mut ObjectSupplier<'_>,
    ) -> Result<Vec<Routed>, DeviceError> {
        let mut out = Vec::new();
        SeatData::check_serial(input_serial, serial)?;
        let evicted = match source {
            Some(sk) => {
                // The source must be live, owned by the requester, and
                // unattached.
                let entry = self.sources.get(&sk).ok_or(DeviceError::UnknownSource)?;
                if entry.client != client {
                    return Err(DeviceError::ForeignSource);
                }
                if entry.machine.dead() {
                    return Err(DeviceError::SourceUnavailable(SourceError::Dead));
                }
                if entry.machine.attachment().is_some() {
                    return Err(DeviceError::SourceUnavailable(SourceError::AlreadyAttached));
                }
                let new_owner = crate::device::SelectionOwner { client, source: sk };
                let attach_to = SeatData::slot_attachment(slot);
                let evicted = self.seat(seat).install(slot, new_owner);
                if let Some(entry) = self.sources.get_mut(&sk) {
                    entry
                        .machine
                        .attach(attach_to)
                        .map_err(DeviceError::SourceUnavailable)?;
                }
                evicted
            }
            None => self.seat(seat).clear(slot, client)?,
        };
        // The evicted source loses its attachment and dies.
        if let Some(ev) = evicted {
            if let Some(entry) = self.sources.get_mut(&ev.source) {
                entry.machine.cancel();
                out.push(Routed::new(
                    entry.client,
                    entry.obj,
                    DataEvent::SourceCancelled,
                ));
            }
        }
        if let Some(src) = source {
            self.publish_slot(seat, slot, src, next_object, &mut out);
        } else {
            self.clear_slot_offers(seat, slot, &mut out);
        }
        Ok(out)
    }

    /// Publish the new selection owner to every non-owner device on
    /// the seat (kill their previous offer of this slot first).
    fn publish_slot(
        &mut self,
        seat: SeatKey,
        slot: Slot,
        owner: SourceKey,
        next_object: &mut ObjectSupplier<'_>,
        out: &mut Vec<Routed>,
    ) {
        let Some(owner_client) = self.source_client(owner) else {
            return;
        };
        let offers: Vec<Mime> = self.source_offers(owner).to_vec();
        let targets: Vec<ClientKey> = self
            .devices
            .keys()
            .filter(|(c, s)| *s == seat && *c != owner_client)
            .map(|(c, _)| *c)
            .collect();
        for receiver in targets {
            // Kill the previous offer of this slot for this client.
            if let Some(old) = self
                .devices
                .get(&(receiver, seat))
                .and_then(|d| match slot {
                    Slot::Clipboard => d.selection_offer,
                    Slot::Primary => d.primary_offer,
                })
            {
                self.kill_offer(old);
            }
            let key = OfferKey::new(self.next_offer);
            self.next_offer += 1;
            let obj = next_object(receiver);
            let kind = OfferKind::Selection;
            self.offers.insert(
                key,
                OfferEntry {
                    client: receiver,
                    obj,
                    machine: DataOffer::new(key, receiver, owner, kind, offers.clone()),
                    seat,
                },
            );
            if let Some(d) = self.devices.get_mut(&(receiver, seat)) {
                match slot {
                    Slot::Clipboard => d.selection_offer = Some(key),
                    Slot::Primary => d.primary_offer = Some(key),
                }
            }
            for event in DataEvent::publication(obj, &offers, kind) {
                let object = if matches!(event, DataEvent::DeviceDataOffer { .. }) {
                    self.devices.get(&(receiver, seat)).map_or(obj, |d| d.obj)
                } else {
                    obj
                };
                out.push(Routed::new(receiver, object, event));
            }
            let announce = match slot {
                Slot::Clipboard => DataEvent::DeviceSelection { offer: Some(obj) },
                Slot::Primary => DataEvent::DevicePrimarySelection { offer: Some(obj) },
            };
            let device_obj = self.devices.get(&(receiver, seat)).map_or(obj, |d| d.obj);
            out.push(Routed::new(receiver, device_obj, announce));
        }
    }

    /// The owner cleared the slot: kill the offers and announce null
    /// — but only to clients that actually held an offer (the owner
    /// never saw a selection event for its own content, so it must
    /// not see a null either).
    fn clear_slot_offers(&mut self, seat: SeatKey, slot: Slot, out: &mut Vec<Routed>) {
        let clients: Vec<(ClientKey, ObjectId, Option<OfferKey>)> = self
            .devices
            .iter()
            .filter(|((_, s), _)| *s == seat)
            .map(|((c, _), d)| {
                (
                    *c,
                    d.obj,
                    match slot {
                        Slot::Clipboard => d.selection_offer,
                        Slot::Primary => d.primary_offer,
                    },
                )
            })
            .collect();
        for (client, device_obj, old) in clients {
            if let Some(old) = old {
                self.kill_offer(old);
                let announce = match slot {
                    Slot::Clipboard => DataEvent::DeviceSelection { offer: None },
                    Slot::Primary => DataEvent::DevicePrimarySelection { offer: None },
                };
                out.push(Routed::new(client, device_obj, announce));
            }
        }
    }

    /// Kill an offer: dead machine + cancel its transfers.
    pub(crate) fn kill_offer(&mut self, key: OfferKey) {
        if let Some(entry) = self.offers.get_mut(&key) {
            entry.machine.kill();
        }
        self.transfers.cancel_offer(key);
        self.offers.remove(&key);
    }

    /// `data_offer.accept` — the receiver announces its target.
    ///
    /// # Errors
    ///
    /// [`OfferError`] (dead offer, unoffered MIME, bad MIME).
    pub fn offer_accept(
        &mut self,
        client: ClientKey,
        key: OfferKey,
        mime: &str,
    ) -> Result<Vec<Routed>, OfferError> {
        let entry = self.offers.get(&key).ok_or(OfferError::Dead)?;
        if entry.client != client {
            return Err(OfferError::Dead);
        }
        let source = entry.machine.source();
        let entry = self.offers.get_mut(&key).ok_or(OfferError::Dead)?;
        entry.machine.accept(mime, &self.limits)?;
        let accepted = entry.machine.accepted().map(str::to_owned);
        if let Some(src) = self.sources.get(&source) {
            let event = DataEvent::SourceTarget {
                mime: accepted.unwrap_or_default(),
            };
            return Ok(vec![Routed::new(src.client, src.obj, event)]);
        }
        Ok(Vec::new())
    }

    /// `data_offer.receive` — the full gate → validate → admit →
    /// `send` pipeline. On success returns the routed `send` (with
    /// the FD attached) plus the transfer id.
    ///
    /// # Errors
    ///
    /// [`ReceiveError::Denied`] (gate), [`ReceiveError::Offer`]
    /// (validation), [`ReceiveError::Admission`] (FD budget).
    pub fn offer_receive(
        &mut self,
        client: ClientKey,
        key: OfferKey,
        mime: &str,
        fd: OwnedFd,
        manifest: ScopeSet,
        token: Option<&AccessToken>,
    ) -> Result<(Vec<Routed>, TransferId), ReceiveError> {
        let entry = self.offers.get(&key).ok_or(ReceiveError::UnknownOffer)?;
        if entry.client != client {
            return Err(ReceiveError::UnknownOffer);
        }
        let kind = entry.machine.kind();
        let source_key = entry.machine.source();
        let op = match kind {
            OfferKind::Selection => GateOp::ClipboardRead,
            OfferKind::Drag => GateOp::DropReceive,
        };
        let (decision, record) = permission::check(op, manifest, token);
        if !decision.is_allowed() {
            return Err(ReceiveError::Denied(record));
        }
        // Validation before commitment: structural checks (offered,
        // not already received) run *before* admission so a rejected
        // receive leaves no trace; the gate ran first (the §16.3
        // order: scope checks before argument validation).
        let entry = self.offers.get(&key).ok_or(ReceiveError::UnknownOffer)?;
        let checked = entry
            .machine
            .check_receive(mime, &self.limits)
            .map_err(ReceiveError::Offer)?;
        let tid = TransferId::new(self.next_transfer);
        self.next_transfer += 1;
        self.transfers
            .admit(tid, client, key, source_key, mime)
            .map_err(ReceiveError::Admission)?;
        // Commit: the MIME is now received on this offer.
        if let Some(entry) = self.offers.get_mut(&key) {
            entry.machine.commit_receive(&checked);
        }
        let (source_client, source_obj) = self
            .sources
            .get(&source_key)
            .map(|s| (s.client, s.obj))
            .ok_or(ReceiveError::UnknownOffer)?;
        if let Some(s) = self.sources.get_mut(&source_key) {
            s.machine.record_send().ok();
        }
        let mut routed = Routed::new(
            source_client,
            source_obj,
            DataEvent::SourceSend {
                mime: mime.to_owned(),
                fd_index: 0,
            },
        );
        routed.fd = Some(fd);
        Ok((vec![routed], tid))
    }

    /// `data_source` destroy (or client teardown for one source):
    /// clear slots, cancel transfers, announce null selections.
    #[must_use]
    pub fn source_destroy(&mut self, key: SourceKey) -> Vec<Routed> {
        let mut out = Vec::new();
        // The drag dies FIRST — the machine still knows the source,
        // so the cancel choreography (leave + cancelled) can route.
        out.extend(self.drag_source_died(key));
        let seats: Vec<SeatKey> = self.seats.keys().copied().collect();
        for seat in seats {
            if self.seat(seat).source_gone(key).is_some() {
                self.clear_slot_offers(seat, Slot::Clipboard, &mut out);
                self.clear_slot_offers(seat, Slot::Primary, &mut out);
            }
        }
        self.transfers.cancel_source(key);
        self.sources.remove(&key);
        out
    }

    /// Client teardown: devices, sources, offers, transfers, slots.
    #[must_use]
    pub fn client_gone(&mut self, client: ClientKey) -> Vec<Routed> {
        let mut out = Vec::new();
        // Clear its sources from every seat (null announcements to
        // the *remaining* clients).
        let seats: Vec<SeatKey> = self.seats.keys().copied().collect();
        for seat in seats {
            self.seat(seat).client_gone(client);
        }
        let sources: Vec<SourceKey> = self
            .sources
            .iter()
            .filter(|(_, e)| e.client == client)
            .map(|(k, _)| *k)
            .collect();
        for s in sources {
            out.extend(self.source_destroy(s));
        }
        // Kill its offers and devices.
        let offers: Vec<OfferKey> = self
            .offers
            .iter()
            .filter(|(_, e)| e.client == client)
            .map(|(k, _)| *k)
            .collect();
        for o in offers {
            self.kill_offer(o);
        }
        self.devices.retain(|(c, _), _| *c != client);
        self.transfers.cancel_client(client);
        // Announce the current selection to the remaining clients of
        // affected seats (the destroyed offers vanished silently for
        // them; the null announcement below is what they observe).
        let seats: Vec<SeatKey> = self.seats.keys().copied().collect();
        for seat in seats {
            self.reannounce(seat, &mut out);
        }
        out
    }

    /// Re-announce both slots' current state (null when empty) to
    /// every device client on the seat. The announced offer is the
    /// one the client currently holds (the device state); owners
    /// announce nothing of their own slots.
    fn reannounce(&mut self, seat: SeatKey, out: &mut Vec<Routed>) {
        let clients: Vec<(ClientKey, ObjectId)> = self
            .devices
            .iter()
            .filter(|((_, s), _)| *s == seat)
            .map(|((c, _), d)| (*c, d.obj))
            .collect();
        for (client, device_obj) in clients {
            let held = |slot: Slot| -> Option<ObjectId> {
                self.devices
                    .get(&(client, seat))
                    .and_then(|d| match slot {
                        Slot::Clipboard => d.selection_offer,
                        Slot::Primary => d.primary_offer,
                    })
                    .and_then(|k| self.offers.get(&k))
                    .map(|e| e.obj)
            };
            out.push(Routed::new(
                client,
                device_obj,
                DataEvent::DeviceSelection {
                    offer: held(Slot::Clipboard),
                },
            ));
            out.push(Routed::new(
                client,
                device_obj,
                DataEvent::DevicePrimarySelection {
                    offer: held(Slot::Primary),
                },
            ));
        }
    }

    /// Seat teardown (integrator-driven): cancels the seat's drag and
    /// clears slots; remaining clients see null selections.
    #[must_use]
    pub fn seat_gone(&mut self, seat: SeatKey) -> Vec<Routed> {
        let mut out = Vec::new();
        if let Some(src) = self.seat(seat).dnd.cancel() {
            out.extend(self.drag_cancel_routed(src, seat));
        }
        self.seats.remove(&seat);
        self.devices.retain(|k, _| k.1 != seat);
        out
    }

    /// The seat's drag machine (read view, for the router).
    #[must_use]
    pub fn seat_dnd(&self, seat: SeatKey) -> Option<&SeatData> {
        self.seats.get(&seat)
    }

    /// The client's live clipboard-selection offer on one seat (the
    /// dispatcher resolves `data_offer` requests through this).
    #[must_use]
    pub fn clipboard_offer(&self, client: ClientKey, seat: SeatKey) -> Option<OfferKey> {
        self.devices
            .get(&(client, seat))
            .and_then(|d| d.selection_offer)
    }

    /// The client's live primary-selection offer on one seat.
    #[must_use]
    pub fn primary_offer(&self, client: ClientKey, seat: SeatKey) -> Option<OfferKey> {
        self.devices
            .get(&(client, seat))
            .and_then(|d| d.primary_offer)
    }

    /// The client's live drag offer on one seat.
    #[must_use]
    pub fn drag_offer(&self, client: ClientKey, seat: SeatKey) -> Option<OfferKey> {
        self.devices.get(&(client, seat)).and_then(|d| d.drag_offer)
    }

    /// One offer's receiving client and object (routing helper).
    #[must_use]
    pub fn offer_route(&self, key: OfferKey) -> Option<(ClientKey, ObjectId)> {
        self.offers.get(&key).map(|e| (e.client, e.obj))
    }
}
