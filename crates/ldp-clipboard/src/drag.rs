//! Drag routing: the pointer-driven side of drag-and-drop.
//!
//! A second [`ClipboardManager`] impl block: the request/transition
//! methods that drive the seat's [`DndMachine`](crate::dnd::DndMachine)
//! and translate its decisions into [`Routed`] batches. The manager
//! owns the tables; this file owns the drag choreography:
//!
//! * `start_drag` validates the press serial, attaches the source,
//!   and announces the initial action set to the source client.
//! * `drag_enter` creates (or reuses) the drag offer for the entered
//!   client — one offer per drag, published with `data_offer`,
//!   `offer`* and `source_actions` — then emits `enter`.
//! * `drag_motion` / `drag_leave` are pure routing.
//! * `offer_set_actions` narrows and re-announces to the source.
//! * `drag_drop` picks the action (copy > move > ask over the
//!   narrowed set), announces `data_offer.action` + the singleton
//!   `actions` + `dnd_drop_performed`, and emits `drop`.
//! * `offer_finish` completes: `dnd_finished` to the source, the
//!   offer dies.
//!
//! Every abort path (declined drop, user release, origin/icon death,
//! source death, client teardown, seat teardown) funnels through one
//! shared cancel choreography so `cancelled` + `leave` + offer death
//! happen exactly once per aborted drag.

#![forbid(unsafe_code)]

use crate::device::DeviceError;
use crate::dnd::{ActionSet, ClientKey, DndAction, DndError, SurfaceKey, Target};
use crate::event::DataEvent;
use crate::manager::{ClipboardManager, ObjectSupplier, Routed, SeatKey};
use crate::mime::Mime;
use crate::offer::{DataOffer, OfferError, OfferKey, OfferKind};
use crate::source::{Attachment, SourceKey};
use ldp_core::geometry::PointF;
use ldp_core::ids::ObjectId;

/// Why a drag could not start.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DragStartError {
    /// The machine refused (bad serial, double drag).
    Machine(DndError),
    /// The source failed the ownership/attachment rules.
    Device(DeviceError),
}

impl std::fmt::Display for DragStartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DragStartError::Machine(e) => write!(f, "{e}"),
            DragStartError::Device(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for DragStartError {}

/// Why a finish failed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FinishError {
    /// The offer refused (not a drag offer, not dropped, dead).
    Offer(OfferError),
    /// The machine refused.
    Machine(DndError),
}

impl std::fmt::Display for FinishError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FinishError::Offer(e) => write!(f, "{e}"),
            FinishError::Machine(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for FinishError {}

impl ClipboardManager {
    /// `data_device.start_drag`.
    ///
    /// # Errors
    ///
    /// [`DragStartError::Machine`] (bad serial, double drag);
    /// [`DragStartError::Device`] (unknown/foreign/unattachable
    /// source).
    // The argument list mirrors the wire request one-for-one
    // (source, origin, icon, serial) plus the integrator context
    // (seat, client, press-serial record).
    #[allow(clippy::too_many_arguments)]
    pub fn start_drag(
        &mut self,
        seat: SeatKey,
        client: ClientKey,
        source: SourceKey,
        origin: SurfaceKey,
        icon: Option<SurfaceKey>,
        press_serial: Option<u32>,
        serial: u32,
    ) -> Result<Vec<Routed>, DragStartError> {
        let source_client = self
            .sources
            .get(&source)
            .map(|s| s.client)
            .ok_or(DragStartError::Device(DeviceError::UnknownSource))?;
        if source_client != client {
            return Err(DragStartError::Device(DeviceError::ForeignSource));
        }
        // Validate the source fully BEFORE the machine starts (a
        // rejected start must not leave a phantom drag).
        let entry = self
            .sources
            .get(&source)
            .ok_or(DragStartError::Device(DeviceError::UnknownSource))?;
        if entry.machine.dead() {
            return Err(DragStartError::Device(DeviceError::SourceUnavailable(
                crate::source::SourceError::Dead,
            )));
        }
        if entry.machine.attachment().is_some() {
            return Err(DragStartError::Device(DeviceError::SourceUnavailable(
                crate::source::SourceError::AlreadyAttached,
            )));
        }
        self.seat(seat)
            .dnd
            .start(source, origin, icon, press_serial, serial)
            .map_err(DragStartError::Machine)?;
        let entry = self
            .sources
            .get_mut(&source)
            .ok_or(DragStartError::Device(DeviceError::UnknownSource))?;
        // Attach as the drag payload; a source already attached to a
        // slot is refused (fresh source per role).
        entry
            .machine
            .attach(Attachment::Drag)
            .map_err(|e| DragStartError::Device(DeviceError::SourceUnavailable(e)))?;
        Ok(vec![Routed::new(
            entry.client,
            entry.obj,
            DataEvent::SourceActions {
                actions: ActionSet::ALL,
            },
        )])
    }

    /// The pointer entered `surface` (of `client`) at `pos`.
    ///
    /// Creates the drag offer on first enter; reuses it after
    /// leave/re-enter. Emits the publication + `enter`.
    ///
    /// # Errors
    ///
    /// [`DndError`] from the machine.
    pub fn drag_enter(
        &mut self,
        seat: SeatKey,
        client: ClientKey,
        surface: SurfaceKey,
        surface_obj: ObjectId,
        pos: PointF,
        next_object: &mut ObjectSupplier<'_>,
    ) -> Result<Vec<Routed>, DndError> {
        let Some(drag) = self.seat(seat).dnd.drag().copied() else {
            return Err(DndError::NoDrag);
        };
        self.seat(seat).dnd.enter(Target { client, surface })?;
        // The drag offer: one per drag, created at first enter and
        // reused for every later enter of the same drag.
        let existing = self.devices.get(&(client, seat)).and_then(|d| d.drag_offer);
        let offers: Vec<Mime> = self.source_offers(drag.source).to_vec();
        let fresh = existing.is_none();
        let offer_obj = if let Some(k) = existing {
            self.offers.get(&k).map(|e| e.obj).ok_or(DndError::NoDrag)?
        } else {
            let key = OfferKey::new(self.next_offer);
            self.next_offer += 1;
            let obj = next_object(client);
            self.offers.insert(
                key,
                crate::manager::OfferEntry {
                    client,
                    obj,
                    machine: DataOffer::new(
                        key,
                        client,
                        drag.source,
                        OfferKind::Drag,
                        offers.clone(),
                    ),
                    seat,
                },
            );
            if let Some(d) = self.devices.get_mut(&(client, seat)) {
                d.drag_offer = Some(key);
            }
            obj
        };
        let mut out = Vec::new();
        if fresh {
            for event in DataEvent::publication(offer_obj, &offers, OfferKind::Drag) {
                let object = if matches!(event, DataEvent::DeviceDataOffer { .. }) {
                    self.devices
                        .get(&(client, seat))
                        .map_or(offer_obj, |d| d.obj)
                } else {
                    offer_obj
                };
                out.push(Routed::new(client, object, event));
            }
        }
        let device_obj = self
            .devices
            .get(&(client, seat))
            .map_or(offer_obj, |d| d.obj);
        out.push(Routed::new(
            client,
            device_obj,
            DataEvent::DeviceEnter {
                surface: surface_obj,
                pos,
                offer: Some(offer_obj),
            },
        ));
        Ok(out)
    }

    /// Pointer motion within the entered surface.
    ///
    /// # Errors
    ///
    /// [`DndError::NoDrag`] / [`DndError::NoTarget`].
    pub fn drag_motion(
        &mut self,
        seat: SeatKey,
        client: ClientKey,
        pos: PointF,
    ) -> Result<Vec<Routed>, DndError> {
        self.seat(seat).dnd.motion()?;
        let device_obj = self
            .devices
            .get(&(client, seat))
            .map(|d| d.obj)
            .ok_or(DndError::NoTarget)?;
        Ok(vec![Routed::new(
            client,
            device_obj,
            DataEvent::DeviceMotion { pos },
        )])
    }

    /// The pointer left the entered receiver. The leaving client is
    /// the machine's target (the pointer is over exactly one
    /// surface; the router tells the manager it left, the manager
    /// knows whose).
    ///
    /// # Errors
    ///
    /// [`DndError::NoDrag`] / [`DndError::NoTarget`].
    pub fn drag_leave(&mut self, seat: SeatKey) -> Result<Vec<Routed>, DndError> {
        let target = self
            .seat_dnd(seat)
            .and_then(|s| s.dnd.drag().and_then(|d| d.target))
            .ok_or(DndError::NoTarget)?;
        self.seat(seat).dnd.leave()?;
        let device_obj = self
            .devices
            .get(&(target.client, seat))
            .map(|d| d.obj)
            .ok_or(DndError::NoTarget)?;
        Ok(vec![Routed::new(
            target.client,
            device_obj,
            DataEvent::DeviceLeave,
        )])
    }

    /// `data_offer.set_actions` — the receiver narrows.
    ///
    /// # Errors
    ///
    /// [`DndError`] from the machine.
    pub fn offer_set_actions(
        &mut self,
        seat: SeatKey,
        client: ClientKey,
        actions: ActionSet,
    ) -> Result<Vec<Routed>, DndError> {
        let narrowed = self.seat(seat).dnd.set_actions(client, actions)?;
        let Some(drag) = self.seat(seat).dnd.drag().copied() else {
            return Err(DndError::NoDrag);
        };
        let Some(entry) = self.sources.get_mut(&drag.source) else {
            return Ok(Vec::new());
        };
        entry.machine.set_actions(narrowed).ok();
        Ok(vec![Routed::new(
            entry.client,
            entry.obj,
            DataEvent::SourceActions { actions: narrowed },
        )])
    }

    /// The user dropped. A declined receiver (empty narrowed set)
    /// routes the full cancel batch (`cancelled` + `leave`).
    ///
    /// # Errors
    ///
    /// [`DndError::NoDrag`] / [`DndError::NoTarget`].
    ///
    /// # Panics
    ///
    /// Never in practice (the `expect` guards an invariant the
    /// machine itself maintains: a dropped drag always has a
    /// target).
    pub fn drag_drop(&mut self, seat: SeatKey) -> Result<Vec<Routed>, DndError> {
        // Capture the source before a declined drop clears the
        // machine — the cancel choreography needs it.
        let source_before = self
            .seat_dnd(seat)
            .and_then(|s| s.dnd.drag().map(|d| d.source));
        let picked = self.seat(seat).dnd.drop()?;
        let Some(picked) = picked else {
            let Some(source) = source_before else {
                return Err(DndError::NoDrag);
            };
            return Ok(self.drag_cancel_routed(source, seat));
        };
        let drag = self
            .seat(seat)
            .dnd
            .drag()
            .copied()
            .ok_or(DndError::NoDrag)?;
        let target = drag.target.expect("a drop implies an entered target");
        let mut out = Vec::new();
        // Receiver: drop on the device, action on the offer.
        if let Some(d) = self.devices.get(&(target.client, seat)) {
            out.push(Routed::new(target.client, d.obj, DataEvent::DeviceDrop));
        }
        if let Some(key) = self
            .devices
            .get(&(target.client, seat))
            .and_then(|d| d.drag_offer)
        {
            if let Some(entry) = self.offers.get_mut(&key) {
                entry.machine.set_actions(picked.as_set()).ok();
                out.push(Routed::new(
                    target.client,
                    entry.obj,
                    DataEvent::OfferAction { action: picked },
                ));
            }
        }
        // Source: singleton actions + drop performed.
        if let Some(entry) = self.sources.get_mut(&drag.source) {
            entry.machine.set_actions(picked.as_set()).ok();
            out.push(Routed::new(
                entry.client,
                entry.obj,
                DataEvent::SourceActions {
                    actions: picked.as_set(),
                },
            ));
            out.push(Routed::new(
                entry.client,
                entry.obj,
                DataEvent::SourceDropPerformed,
            ));
        }
        Ok(out)
    }

    /// `data_offer.finish` — the drag completes (`dnd_finished` to
    /// the source; the offer dies).
    ///
    /// # Errors
    ///
    /// [`FinishError::Offer`] when the offer refuses;
    /// [`FinishError::Machine`] when the machine refuses.
    pub fn offer_finish(
        &mut self,
        seat: SeatKey,
        client: ClientKey,
    ) -> Result<Vec<Routed>, FinishError> {
        let dropped = self
            .seat_dnd(seat)
            .is_some_and(|s| s.dnd.phase() == crate::dnd::DndPhase::Dropped);
        let offer_key = self.devices.get(&(client, seat)).and_then(|d| d.drag_offer);
        if let Some(k) = offer_key {
            if let Some(entry) = self.offers.get_mut(&k) {
                entry.machine.finish(dropped).map_err(FinishError::Offer)?;
            }
        }
        // Capture the drag source before the machine clears it.
        let source = self
            .seat_dnd(seat)
            .and_then(|s| s.dnd.drag().map(|g| g.source));
        self.seat(seat)
            .dnd
            .receiver_finish()
            .map_err(FinishError::Machine)?;
        let mut out = Vec::new();
        if let Some(k) = offer_key {
            self.kill_offer(k);
        }
        if let Some(d) = self.devices.get_mut(&(client, seat)) {
            d.drag_offer = None;
        }
        // The source learns the outcome: dnd_finished (the action was
        // already announced as the singleton `actions` set).
        if let Some(entry) = source.and_then(|s| self.sources.get_mut(&s)) {
            entry.machine.finish();
            out.push(Routed::new(
                entry.client,
                entry.obj,
                DataEvent::SourceFinished,
            ));
        }
        Ok(out)
    }

    /// Resolve an `ask` pick to a concrete action (the integrator's
    /// user prompt). Re-announces the action to both sides.
    ///
    /// # Errors
    ///
    /// [`DndError`] from the machine.
    pub fn resolve_ask(
        &mut self,
        seat: SeatKey,
        action: DndAction,
    ) -> Result<Vec<Routed>, DndError> {
        let action = self.seat(seat).dnd.resolve_ask(action)?;
        let mut out = Vec::new();
        let drag = self
            .seat(seat)
            .dnd
            .drag()
            .copied()
            .ok_or(DndError::NoDrag)?;
        if let Some(entry) = self.sources.get_mut(&drag.source) {
            entry.machine.set_actions(action.as_set()).ok();
            out.push(Routed::new(
                entry.client,
                entry.obj,
                DataEvent::SourceActions {
                    actions: action.as_set(),
                },
            ));
        }
        if let Some(target) = drag.target {
            if let Some(k) = self
                .devices
                .get(&(target.client, seat))
                .and_then(|d| d.drag_offer)
            {
                if let Some(entry) = self.offers.get_mut(&k) {
                    entry.machine.set_actions(action.as_set()).ok();
                    out.push(Routed::new(
                        target.client,
                        entry.obj,
                        DataEvent::OfferAction { action },
                    ));
                }
            }
        }
        Ok(out)
    }

    /// A surface died: the entered receiver gets `leave`; the origin
    /// or icon dying cancels the whole drag.
    #[must_use]
    pub fn drag_surface_gone(&mut self, seat: SeatKey, surface: SurfaceKey) -> Vec<Routed> {
        let drag_before = self.seat_dnd(seat).and_then(|s| s.dnd.drag().copied());
        let Some(drag) = drag_before else {
            return Vec::new();
        };
        let target_before = drag.target;
        let cancelled = self.seat(seat).dnd.surface_gone(surface);
        let mut out = Vec::new();
        if cancelled {
            out.extend(self.drag_cancel_routed(drag.source, seat));
            return out;
        }
        if let Some(t) = target_before {
            if t.surface == surface {
                if let Some(d) = self.devices.get(&(t.client, seat)) {
                    out.push(Routed::new(t.client, d.obj, DataEvent::DeviceLeave));
                }
            }
        }
        out
    }

    /// Cancel the seat's drag (user released outside, policy).
    #[must_use]
    pub fn drag_cancel(&mut self, seat: SeatKey) -> Vec<Routed> {
        let Some(source) = self.seat(seat).dnd.cancel() else {
            return Vec::new();
        };
        self.drag_cancel_routed(source, seat)
    }

    /// The cancel choreography shared by every abort path: `leave`
    /// to entered receivers, `cancelled` to the source, drag-offer
    /// death.
    pub(crate) fn drag_cancel_routed(&mut self, source: SourceKey, seat: SeatKey) -> Vec<Routed> {
        let mut out = Vec::new();
        // Leave to every client still holding a drag offer on this
        // seat (the entered receiver).
        let receivers: Vec<(ClientKey, ObjectId)> = self
            .devices
            .iter()
            .filter(|((_, s), _)| *s == seat)
            .filter_map(|((c, _), d)| d.drag_offer.map(|_| (*c, d.obj)))
            .collect();
        for (client, device_obj) in receivers {
            out.push(Routed::new(client, device_obj, DataEvent::DeviceLeave));
        }
        if let Some(entry) = self.sources.get_mut(&source) {
            entry.machine.cancel();
            out.push(Routed::new(
                entry.client,
                entry.obj,
                DataEvent::SourceCancelled,
            ));
        }
        // Kill every drag offer on the seat.
        let keys: Vec<OfferKey> = self
            .offers
            .iter()
            .filter(|(_, e)| e.seat == seat && e.machine.kind() == OfferKind::Drag)
            .map(|(k, _)| *k)
            .collect();
        for k in keys {
            self.kill_offer(k);
        }
        for d in self.devices.values_mut() {
            d.drag_offer = None;
        }
        out
    }

    /// A drag source died: cancel its drag (called from
    /// `source_destroy`).
    pub(crate) fn drag_source_died(&mut self, source: SourceKey) -> Vec<Routed> {
        let seats: Vec<SeatKey> = self
            .seats
            .iter()
            .filter(|(_, s)| s.dnd.drag().is_some_and(|d| d.source == source))
            .map(|(k, _)| *k)
            .collect();
        let mut out = Vec::new();
        for seat in seats {
            self.seat(seat).dnd.cancel();
            out.extend(self.drag_cancel_routed(source, seat));
        }
        out
    }
}
