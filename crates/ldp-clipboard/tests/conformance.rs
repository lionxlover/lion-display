//! THE Phase 13 exit criterion (part 1): state-machine conformance
//! from `spec/data.toml`.
//!
//! Three layers of proof:
//!
//! 1. **Coverage**: every request and event the compiled schema
//!    carries for `ldp.data` is exercised by a typed handle in the
//!    crate — in both directions (nothing uncovered in the spec,
//!    nothing in the typed surface missing from the spec).
//! 2. **Golden lifecycles**: scripted sequences (selection
//!    set→publish→accept→receive; primary selection; the full drag
//!    happy path; eviction; owner-destroy) produce exactly the
//!    expected event streams, and every emitted message passes the
//!    wire gauntlet (encode → decode → strict signature check).
//! 3. **Error conformance**: bad serials, foreign sources, late
//!    offers, unoffered receives, gate denials, budget exhaustion
//!    surface as the typed errors the dispatcher maps to protocol
//!    error codes.

mod common;

use common::{assert_wire_valid, ids, World};
use ldp_clipboard::device::{DeviceError, Slot};
use ldp_clipboard::dnd::{ActionSet, ClientKey, DndAction, DndPhase};
use ldp_clipboard::event::DataEvent;
use ldp_clipboard::manager::{ClipboardManager, ReceiveError};
use ldp_clipboard::mime::Mime;
use ldp_clipboard::offer::OfferError;
use ldp_clipboard::source::SourceError;
use ldp_clipboard::SourceKey;
use ldp_core::caps::{Scope, ScopeSet};
use ldp_core::geometry::PointF;
use ldp_core::ids::ObjectId;
use ldp_core::limits::Limits;
use ldp_protocol::generated::MODULES;
use std::os::fd::OwnedFd;

const DEVICE_OBJ_B: ObjectId = ObjectId::from_wire(0x5002);

// ---------------------------------------------------------------------------
// 1. Coverage: the compiled schema vs the typed surface.
// ---------------------------------------------------------------------------

#[test]
fn every_spec_request_has_a_typed_handle() {
    // The (interface, request) pairs the crate handles, pinned. The
    // table names each handle.
    let handled: &[(&str, &str, &str)] = &[
        (
            "ldp.data.data_device_manager",
            "get_data_device",
            "ClipboardManager::create_device",
        ),
        (
            "ldp.data.data_device_manager",
            "create_data_source",
            "ClipboardManager::create_source",
        ),
        (
            "ldp.data.data_source",
            "offer",
            "ClipboardManager::source_offer",
        ),
        (
            "ldp.data.data_device",
            "set_selection",
            "ClipboardManager::set_slot(Clipboard)",
        ),
        (
            "ldp.data.data_device",
            "set_primary_selection",
            "ClipboardManager::set_slot(Primary)",
        ),
        (
            "ldp.data.data_device",
            "start_drag",
            "ClipboardManager::start_drag",
        ),
        (
            "ldp.data.data_offer",
            "accept",
            "ClipboardManager::offer_accept",
        ),
        (
            "ldp.data.data_offer",
            "receive",
            "ClipboardManager::offer_receive",
        ),
        (
            "ldp.data.data_offer",
            "set_actions",
            "ClipboardManager::offer_set_actions",
        ),
        (
            "ldp.data.data_offer",
            "finish",
            "ClipboardManager::offer_finish",
        ),
    ];
    let data = MODULES
        .iter()
        .find(|m| m.name == "ldp.data")
        .expect("ldp.data module in schema");
    let mut missing = Vec::new();
    let mut count = 0;
    for iface in data.interfaces {
        for req in iface.requests {
            count += 1;
            // InterfaceSchema.name is already the FQ name.
            if !handled
                .iter()
                .any(|(i, r, _)| *i == iface.name && *r == req.name)
            {
                missing.push(format!("{}::{}", iface.name, req.name));
            }
        }
    }
    assert!(missing.is_empty(), "unhandled requests: {missing:?}");
    assert_eq!(count, handled.len(), "a pinned handle went stale");
}

#[test]
fn every_spec_event_has_a_typed_variant() {
    let data = MODULES
        .iter()
        .find(|m| m.name == "ldp.data")
        .expect("ldp.data module in schema");
    let mut spec_events = Vec::new();
    for iface in data.interfaces {
        for ev in iface.events {
            spec_events.push((iface.name, ev.name));
        }
    }
    // The typed vocabulary, pinned event-for-event.
    let typed: &[(&str, &str)] = &[
        ("ldp.data.data_source", "target"),
        ("ldp.data.data_source", "actions"),
        ("ldp.data.data_source", "send"),
        ("ldp.data.data_source", "cancelled"),
        ("ldp.data.data_source", "dnd_drop_performed"),
        ("ldp.data.data_source", "dnd_finished"),
        ("ldp.data.data_device", "data_offer"),
        ("ldp.data.data_device", "selection"),
        ("ldp.data.data_device", "primary_selection"),
        ("ldp.data.data_device", "enter"),
        ("ldp.data.data_device", "motion"),
        ("ldp.data.data_device", "drop"),
        ("ldp.data.data_device", "leave"),
        ("ldp.data.data_offer", "offer"),
        ("ldp.data.data_offer", "source_actions"),
        ("ldp.data.data_offer", "action"),
    ];
    assert_eq!(
        spec_events.len(),
        typed.len(),
        "spec and typed vocabulary disagree in size"
    );
    for (iface, name) in typed {
        assert!(
            spec_events.iter().any(|(i, n)| i == iface && n == name),
            "typed event {iface}::{name} missing from spec"
        );
    }
}

// ---------------------------------------------------------------------------
// 2. Golden lifecycles.
// ---------------------------------------------------------------------------

/// Wire-validate an entire routed batch (the gauntlet per event).
fn validate_batch(batch: &[ldp_clipboard::Routed]) -> usize {
    batch
        .iter()
        .map(|r| {
            let msg = assert_wire_valid(&r.event, r.object);
            msg.encode(&Limits::default()).expect("encode").len()
        })
        .count()
}

fn pipe_write() -> OwnedFd {
    ldp_clipboard::pipe::pipe().expect("pipe").write.into_fd()
}

#[test]
fn golden_selection_publish_accept_receive() {
    let mut w = World::new(3);
    let src = ids::source(&mut w, ids::A, &["text/plain;charset=utf-8", "image/png"]);
    // A sets the clipboard with a valid serial.
    let batch = w
        .with_supplier(|m, sup| {
            m.set_slot(
                ids::SEAT,
                Slot::Clipboard,
                ids::A,
                Some(src),
                Some(1),
                1,
                sup,
            )
        })
        .expect("set_selection");
    // Two other device clients on the seat each get: data_offer,
    // offer, offer, selection (4 events). The owner gets nothing.
    assert_eq!(batch.len(), 8, "batch: {batch:?}");
    for c in [ids::B, ids::C] {
        let theirs: Vec<&ldp_clipboard::Routed> = batch.iter().filter(|r| r.client == c).collect();
        assert_eq!(theirs.len(), 4);
        assert!(matches!(theirs[0].event, DataEvent::DeviceDataOffer { .. }));
        assert!(matches!(theirs[1].event, DataEvent::OfferOffer { .. }));
        assert!(matches!(theirs[2].event, DataEvent::OfferOffer { .. }));
        assert!(matches!(theirs[3].event, DataEvent::DeviceSelection { .. }));
    }
    assert_eq!(validate_batch(&batch), 8);
    // B accepts the PNG target.
    let offer = w.manager.clipboard_offer(ids::B, ids::SEAT).expect("offer");
    let accept = w
        .manager
        .offer_accept(ids::B, offer, "image/png")
        .expect("accept");
    assert_eq!(accept.len(), 1);
    assert!(matches!(
        accept[0].event,
        DataEvent::SourceTarget { ref mime } if mime == "image/png"
    ));
    // B receives (granted scope): the source gets send(mime, fd).
    let (send_batch, tid) = w
        .manager
        .offer_receive(
            ids::B,
            offer,
            "text/plain",
            pipe_write(),
            ScopeSet::single(Scope::ClipboardRead),
            None,
        )
        .expect("receive");
    assert_eq!(send_batch.len(), 1);
    assert!(matches!(
        send_batch[0].event,
        DataEvent::SourceSend { ref mime, fd_index: 0 } if mime == "text/plain"
    ));
    assert!(send_batch[0].fd.is_some(), "send must carry the pipe fd");
    validate_batch(&send_batch);
    assert!(w.manager.transfers().get(tid).is_some());
}

#[test]
fn golden_eviction_cancels_previous_owner() {
    let mut w = World::new(3);
    let s1 = ids::source(&mut w, ids::A, &["text/plain"]);
    let s2 = ids::source(&mut w, ids::B, &["text/html"]);
    let _ = w.with_supplier(|m, sup| {
        m.set_slot(
            ids::SEAT,
            Slot::Clipboard,
            ids::A,
            Some(s1),
            Some(1),
            1,
            sup,
        )
    });
    let batch = w
        .with_supplier(|m, sup| {
            m.set_slot(
                ids::SEAT,
                Slot::Clipboard,
                ids::B,
                Some(s2),
                Some(2),
                2,
                sup,
            )
        })
        .expect("takeover");
    // cancelled to A; C (the only non-owner besides B) sees the new
    // publication; B sees nothing of its own selection.
    let cancelled: Vec<ClientKey> = batch
        .iter()
        .filter(|r| matches!(r.event, DataEvent::SourceCancelled))
        .map(|r| r.client)
        .collect();
    assert_eq!(cancelled, vec![ids::A]);
    // Every *non-owner* device client sees the publication — the
    // evicted A included (it lost ownership; it is a plain receiver
    // now).
    let mut published: Vec<ClientKey> = batch
        .iter()
        .filter(|r| matches!(r.event, DataEvent::DeviceSelection { .. }))
        .map(|r| r.client)
        .collect();
    published.sort_by_key(|c| c.0);
    assert_eq!(
        published,
        vec![ids::A, ids::C],
        "non-owners see the publication"
    );
    validate_batch(&batch);
}

#[test]
fn golden_owner_destroy_announces_null() {
    let mut w = World::new(3);
    let src = ids::source(&mut w, ids::A, &["text/plain"]);
    let _ = w.with_supplier(|m, sup| {
        m.set_slot(
            ids::SEAT,
            Slot::Clipboard,
            ids::A,
            Some(src),
            Some(1),
            1,
            sup,
        )
    });
    let batch = w.manager.source_destroy(src);
    // B and C each see selection(null).
    let nulls: Vec<ClientKey> = batch
        .iter()
        .filter(|r| matches!(r.event, DataEvent::DeviceSelection { offer: None }))
        .map(|r| r.client)
        .collect();
    assert_eq!(nulls.len(), 2);
    assert!(nulls.contains(&ids::B) && nulls.contains(&ids::C));
    validate_batch(&batch);
}

#[test]
fn golden_drag_happy_path() {
    let mut w = World::new(3);
    let src = ids::source(&mut w, ids::A, &["text/uri-list", "text/plain"]);
    // Start the drag (press serial 7): the source sees actions(ALL).
    let start = w
        .manager
        .start_drag(
            ids::SEAT,
            ids::A,
            src,
            ids::ORIGIN,
            Some(ids::ICON),
            Some(7),
            7,
        )
        .expect("start_drag");
    assert!(matches!(
        &start[0].event,
        DataEvent::SourceActions { actions } if *actions == ActionSet::ALL
    ));
    validate_batch(&start);
    // Enter B's surface: data_offer + 2 offers + source_actions +
    // enter = 5 events.
    let enter = w
        .with_supplier(|m, sup| {
            m.drag_enter(
                ids::SEAT,
                ids::B,
                ids::TARGET,
                ObjectId::from_wire(0x7001),
                PointF { x: 10.0, y: 20.0 },
                sup,
            )
        })
        .expect("enter");
    assert_eq!(enter.len(), 5);
    assert!(matches!(enter[4].event, DataEvent::DeviceEnter { .. }));
    validate_batch(&enter);
    // Motion.
    let motion = w
        .manager
        .drag_motion(ids::SEAT, ids::B, PointF { x: 11.0, y: 21.0 })
        .expect("motion");
    assert!(matches!(motion[0].event, DataEvent::DeviceMotion { .. }));
    // Narrow to move|ask.
    let narrow = w
        .manager
        .offer_set_actions(ids::SEAT, ids::B, ActionSet::build(false, true, true))
        .expect("set_actions");
    assert!(matches!(
        &narrow[0].event,
        DataEvent::SourceActions { actions } if !actions.copy() && actions.move_() && actions.ask()
    ));
    // Drop: move wins (copy absent).
    let drop = w.manager.drag_drop(ids::SEAT).expect("drop");
    // drop + action(to B's offer) + singleton actions +
    // drop_performed = 4.
    assert_eq!(drop.len(), 4);
    assert!(batch_has(&drop, |e| matches!(e, DataEvent::DeviceDrop)));
    assert!(batch_has(&drop, |e| matches!(
        e,
        DataEvent::OfferAction {
            action: DndAction::Move
        }
    )));
    assert!(batch_has(&drop, |e| matches!(
        e,
        DataEvent::SourceActions { actions } if *actions == DndAction::Move.as_set()
    )));
    assert!(batch_has(&drop, |e| matches!(
        e,
        DataEvent::SourceDropPerformed
    )));
    validate_batch(&drop);
    assert_eq!(
        w.manager.seat_dnd(ids::SEAT).map(|s| s.dnd.phase()),
        Some(DndPhase::Dropped)
    );
    // Finish: dnd_finished to the source; the offer dies.
    let finish = w.manager.offer_finish(ids::SEAT, ids::B).expect("finish");
    assert!(batch_has(&finish, |e| matches!(
        e,
        DataEvent::SourceFinished
    )));
    assert_eq!(
        w.manager.seat_dnd(ids::SEAT).map(|s| s.dnd.phase()),
        Some(DndPhase::Idle)
    );
    assert!(w.manager.drag_offer(ids::B, ids::SEAT).is_none());
    validate_batch(&finish);
}

#[test]
fn golden_drag_leave_reenter_reuses_offer() {
    let mut w = World::new(3);
    let src = ids::source(&mut w, ids::A, &["text/plain"]);
    w.manager
        .start_drag(ids::SEAT, ids::A, src, ids::ORIGIN, None, Some(1), 1)
        .unwrap();
    let first = w
        .with_supplier(|m, sup| {
            m.drag_enter(
                ids::SEAT,
                ids::B,
                ids::TARGET,
                ObjectId::from_wire(0x7101),
                PointF { x: 0.0, y: 0.0 },
                sup,
            )
        })
        .unwrap();
    let offer_before = w.manager.drag_offer(ids::B, ids::SEAT).expect("offer");
    // Leave: B gets leave; the offer stays alive for re-entry.
    let leave = w.manager.drag_leave(ids::SEAT).unwrap();
    assert!(batch_has(&leave, |e| matches!(e, DataEvent::DeviceLeave)));
    assert!(w.manager.drag_offer(ids::B, ids::SEAT).is_some());
    // Re-enter: same offer object, publication not repeated.
    let again = w
        .with_supplier(|m, sup| {
            m.drag_enter(
                ids::SEAT,
                ids::B,
                ids::TARGET,
                ObjectId::from_wire(0x7101),
                PointF { x: 1.0, y: 1.0 },
                sup,
            )
        })
        .unwrap();
    let offer_after = w.manager.drag_offer(ids::B, ids::SEAT).expect("offer");
    assert_eq!(offer_before, offer_after, "one offer per drag, reused");
    // Only the enter event this time (no re-publication).
    assert_eq!(again.len(), 1);
    assert!(matches!(again[0].event, DataEvent::DeviceEnter { .. }));
    validate_batch(&first);
    validate_batch(&again);
}

fn batch_has(batch: &[ldp_clipboard::Routed], pred: impl Fn(&DataEvent) -> bool) -> bool {
    batch.iter().any(|r| pred(&r.event))
}

// ---------------------------------------------------------------------------
// 3. Error conformance.
// ---------------------------------------------------------------------------

#[test]
fn bad_serial_is_rejected() {
    let mut w = World::new(2);
    let src = ids::source(&mut w, ids::A, &["text/plain"]);
    let err = w.with_supplier(|m, sup| {
        m.set_slot(
            ids::SEAT,
            Slot::Clipboard,
            ids::A,
            Some(src),
            Some(1),
            2,
            sup,
        )
    });
    assert_eq!(err.unwrap_err(), DeviceError::BadSerial);
    let err = w.with_supplier(|m, sup| {
        m.set_slot(ids::SEAT, Slot::Primary, ids::A, Some(src), None, 1, sup)
    });
    assert_eq!(err.unwrap_err(), DeviceError::BadSerial);
    // start_drag with a non-press serial.
    let err = w
        .manager
        .start_drag(ids::SEAT, ids::A, src, ids::ORIGIN, None, Some(1), 2);
    assert!(err.is_err());
}

#[test]
fn foreign_and_unknown_sources_rejected() {
    let mut w = World::new(2);
    let src = ids::source(&mut w, ids::A, &["text/plain"]);
    let err = w.with_supplier(|m, sup| {
        m.set_slot(
            ids::SEAT,
            Slot::Clipboard,
            ids::B,
            Some(src),
            Some(1),
            1,
            sup,
        )
    });
    assert_eq!(err.unwrap_err(), DeviceError::ForeignSource);
    let err = w.with_supplier(|m, sup| {
        m.set_slot(
            ids::SEAT,
            Slot::Clipboard,
            ids::A,
            Some(SourceKey::new(9999)),
            Some(1),
            1,
            sup,
        )
    });
    assert_eq!(err.unwrap_err(), DeviceError::UnknownSource);
}

#[test]
fn late_offer_and_flood_rejected() {
    let mut w = World::new(2);
    let src = ids::source(&mut w, ids::A, &["text/plain"]);
    let _ = w.with_supplier(|m, sup| {
        m.set_slot(
            ids::SEAT,
            Slot::Clipboard,
            ids::A,
            Some(src),
            Some(1),
            1,
            sup,
        )
    });
    assert_eq!(
        w.manager.source_offer(src, "image/png").unwrap_err(),
        SourceError::OfferTooLate
    );
    let flood = ids::source(&mut w, ids::B, &[]);
    for i in 0..ldp_clipboard::MAX_OFFERS {
        w.manager
            .source_offer(flood, &format!("application/x-n{i}"))
            .unwrap();
    }
    assert_eq!(
        w.manager
            .source_offer(flood, "application/x-over")
            .unwrap_err(),
        SourceError::TooManyOffers
    );
    assert_eq!(
        w.manager.source_offer(flood, "not a mime").unwrap_err(),
        SourceError::BadMime
    );
}

#[test]
fn receive_errors_surface_typed() {
    let mut w = World::new(2);
    let src = ids::source(&mut w, ids::A, &["text/plain"]);
    let _ = w.with_supplier(|m, sup| {
        m.set_slot(
            ids::SEAT,
            Slot::Clipboard,
            ids::A,
            Some(src),
            Some(1),
            1,
            sup,
        )
    });
    let offer = w.manager.clipboard_offer(ids::B, ids::SEAT).expect("offer");
    // The gate runs FIRST (architecture §16.3: scope checks before
    // argument validation) — so an unprivileged client sees Denied
    // even for a garbage MIME.
    let err = w
        .manager
        .offer_receive(
            ids::B,
            offer,
            "application/json",
            pipe_write(),
            ScopeSet::NONE,
            None,
        )
        .unwrap_err();
    assert!(matches!(err, ReceiveError::Denied(_)), "{err:?}");
    // With the scope granted, the unoffered MIME surfaces.
    let grant = ScopeSet::single(Scope::ClipboardRead);
    let err = w
        .manager
        .offer_receive(ids::B, offer, "application/json", pipe_write(), grant, None)
        .unwrap_err();
    assert!(
        matches!(err, ReceiveError::Offer(OfferError::NotOffered)),
        "{err:?}"
    );
    // Granted: succeeds; duplicate receive of the same MIME is
    // rejected.
    let ok = w
        .manager
        .offer_receive(ids::B, offer, "text/plain", pipe_write(), grant, None);
    assert!(ok.is_ok(), "{ok:?}");
    let err = w
        .manager
        .offer_receive(ids::B, offer, "text/plain", pipe_write(), grant, None)
        .unwrap_err();
    assert!(
        matches!(err, ReceiveError::Offer(OfferError::AlreadyReceived)),
        "{err:?}"
    );
}

#[test]
fn fd_budget_exhaustion_is_typed() {
    let limits = Limits {
        client_fds: 2,
        ..Limits::default()
    };
    let mut m = ClipboardManager::new(limits);
    m.create_device(ClientKey(0), ldp_clipboard::SeatKey(0), DEVICE_OBJ_B);
    m.create_device(
        ClientKey(1),
        ldp_clipboard::SeatKey(0),
        ObjectId::from_wire(0x5003),
    );
    let src = m.create_source(ClientKey(0), ObjectId::from_wire(0x6001));
    for mime in ["text/plain", "text/html", "image/png"] {
        m.source_offer(src, mime).unwrap();
    }
    let grant = ScopeSet::single(Scope::ClipboardRead);
    let mut next = 0x7000u32;
    let mut sup = |c: ClientKey| {
        next += 1;
        ObjectId::from_wire(((c.0 as u32) << 24) | (next & 0xFF_FFFF))
    };
    m.set_slot(
        ldp_clipboard::SeatKey(0),
        Slot::Clipboard,
        ClientKey(0),
        Some(src),
        Some(1),
        1,
        &mut sup,
    )
    .unwrap();
    let offer = m
        .clipboard_offer(ClientKey(1), ldp_clipboard::SeatKey(0))
        .expect("offer");
    for mime in ["text/plain", "text/html"] {
        let ok = m.offer_receive(ClientKey(1), offer, mime, pipe_write(), grant, None);
        assert!(ok.is_ok(), "{ok:?}");
    }
    // The budget is exhausted: the third (distinct) MIME is refused
    // at admission, not marked received.
    let err = m
        .offer_receive(ClientKey(1), offer, "image/png", pipe_write(), grant, None)
        .unwrap_err();
    assert!(
        matches!(
            err,
            ReceiveError::Admission(ldp_clipboard::AdmissionError::FdBudget)
        ),
        "{err:?}"
    );
}

#[test]
fn mime_validation_conformance() {
    let l = Limits::default();
    assert!(Mime::parse("image/png", &l).is_ok());
    assert!(Mime::parse("text/plain;charset=utf-8", &l).is_ok());
    assert!(Mime::parse("TEXT/PLAIN;X=1;Y=2", &l).is_ok());
    for bad in [
        "",
        "text",
        "text/",
        "/png",
        "a b/c",
        "text/plain;=",
        "text/plain;a",
        "x/y;z=\"q",
    ] {
        assert!(Mime::parse(bad, &l).is_err(), "{bad:?} accepted");
    }
}

#[test]
fn primary_selection_is_a_unified_slot() {
    let mut w = World::new(3);
    let src = ids::source(&mut w, ids::A, &["text/plain;charset=utf-8"]);
    let batch = w
        .with_supplier(|m, sup| {
            m.set_slot(ids::SEAT, Slot::Primary, ids::A, Some(src), Some(4), 4, sup)
        })
        .expect("primary");
    // B and C see the primary publication (one offered MIME:
    // data_offer + offer + primary_selection = 3 events each).
    assert_eq!(batch.len(), 6);
    let prim: Vec<ClientKey> = batch
        .iter()
        .filter(|r| matches!(r.event, DataEvent::DevicePrimarySelection { .. }))
        .map(|r| r.client)
        .collect();
    assert_eq!(prim.len(), 2);
    validate_batch(&batch);
    // The clipboard slot stays untouched.
    let seat = w.manager.seat_dnd(ids::SEAT).unwrap();
    assert!(seat.clipboard().is_none());
    assert!(seat.primary().is_some());
}

#[test]
fn reverse_lookups_resolve_objects_to_keys() {
    let mut w = World::new(2);
    let src = ids::source(&mut w, ids::A, &["text/plain;charset=utf-8"]);
    let obj = w.manager.source_object(src).expect("the source's object");
    // The owner resolves; a borrowed guess from another client does not.
    assert_eq!(w.manager.source_of_object(ids::A, obj), Some(src));
    assert_eq!(w.manager.source_of_object(ids::B, obj), None);
    assert_eq!(
        w.manager.source_of_object(ids::A, ObjectId::from_wire(1)),
        None
    );
    // The device's seat: A's device on the default seat resolves (the
    // harness's first allocation: client 0, seat 0).
    let seat = w
        .manager
        .seat_of_device(ids::A, ObjectId::from_wire(0x4001));
    assert_eq!(seat, Some(ids::SEAT));
    assert_eq!(
        w.manager
            .seat_of_device(ids::B, ObjectId::from_wire(0x4001)),
        None
    );
}

#[test]
fn offer_destroy_clears_the_slot_pointer_and_kills_the_offer() {
    let mut w = World::new(2);
    let src = ids::source(&mut w, ids::A, &["text/plain;charset=utf-8"]);
    let _batch = w
        .with_supplier(|m, sup| {
            m.set_slot(
                ids::SEAT,
                Slot::Clipboard,
                ids::A,
                Some(src),
                Some(1),
                1,
                sup,
            )
        })
        .expect("set_selection");
    let offer = w
        .manager
        .clipboard_offer(ids::B, ids::SEAT)
        .expect("B holds the offer");
    let obj = w
        .manager
        .offer_route(offer)
        .map(|(_, o)| o)
        .expect("the offer's object");
    // B's reverse lookup resolves; A's guess does not.
    assert_eq!(w.manager.offer_of_object(ids::B, obj), Some(offer));
    assert_eq!(w.manager.offer_of_object(ids::A, obj), None);
    // The receiver destroys the offer: nothing routes, but the slot
    // pointer clears (a stale pointer would answer clipboard_offer
    // with a dead key).
    let routed = w.manager.offer_destroy(ids::B, obj);
    assert!(
        routed.is_empty(),
        "a selection offer's destruction routes nothing"
    );
    assert_eq!(w.manager.clipboard_offer(ids::B, ids::SEAT), None);
    // The offer is gone: the reverse lookup no longer resolves.
    assert_eq!(w.manager.offer_of_object(ids::B, obj), None);
    // An unknown object's destruction is a no-op.
    assert!(w
        .manager
        .offer_destroy(ids::B, ObjectId::from_wire(0x7777))
        .is_empty());
}
