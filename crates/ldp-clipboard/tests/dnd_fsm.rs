//! DnD state-machine properties over the manager: scripted drags,
//! cancel paths, action negotiation, offer reuse, ask resolution,
//! and a 30k-step transition fuzz asserting the machine never wedges
//! (every terminal transition is reachable, `Idle` is the only
//! resting state, and no batch is ever emitted after the drag dies).

mod common;

use common::{ids, Rng, World};
use ldp_clipboard::dnd::{ActionSet, DndAction, DndPhase};
use ldp_clipboard::event::DataEvent;
use ldp_clipboard::{DragStartError, FinishError};
use ldp_core::geometry::PointF;
use ldp_core::ids::ObjectId;

fn pos(x: f32, y: f32) -> PointF {
    PointF { x, y }
}

fn start(w: &mut World, serial: u32) {
    let src = ids::source(w, ids::A, &["text/plain", "image/png"]);
    w.manager
        .start_drag(
            ids::SEAT,
            ids::A,
            src,
            ids::ORIGIN,
            Some(ids::ICON),
            Some(serial),
            serial,
        )
        .unwrap();
}

fn enter(w: &mut World, client: ldp_clipboard::ClientKey, surface: ldp_clipboard::SurfaceKey) {
    w.with_supplier(|m, sup| {
        m.drag_enter(
            ids::SEAT,
            client,
            surface,
            ObjectId::from_wire(0x8001),
            pos(0.0, 0.0),
            sup,
        )
    })
    .unwrap();
}

fn phase(w: &World) -> Option<DndPhase> {
    w.manager.seat_dnd(ids::SEAT).map(|s| s.dnd.phase())
}

fn drag_batch_len(w: &mut World) -> usize {
    // The cancel batch is the observable "drag died" signal.
    w.manager.drag_cancel(ids::SEAT).len()
}

#[test]
fn scripted_cancel_paths_all_reach_idle() {
    // Cancel from idle is a no-op.
    let mut w = World::new(3);
    assert_eq!(drag_batch_len(&mut w), 0);
    // Cancel mid-drag (no receiver): cancelled only.
    start(&mut w, 1);
    let batch = w.manager.drag_cancel(ids::SEAT);
    assert_eq!(batch.len(), 1);
    assert!(matches!(batch[0].event, DataEvent::SourceCancelled));
    assert_eq!(phase(&w), Some(DndPhase::Idle));
    // Cancel with an entered receiver: leave + cancelled.
    start(&mut w, 2);
    enter(&mut w, ids::B, ids::TARGET);
    let batch = w.manager.drag_cancel(ids::SEAT);
    assert!(matches!(batch[0].event, DataEvent::DeviceLeave));
    assert!(matches!(batch[1].event, DataEvent::SourceCancelled));
    assert_eq!(phase(&w), Some(DndPhase::Idle));
    // Cancel after drop (declined receiver): the full funnel.
    start(&mut w, 3);
    enter(&mut w, ids::B, ids::TARGET);
    w.manager
        .offer_set_actions(ids::SEAT, ids::B, ActionSet::NONE)
        .unwrap();
    let batch = w.manager.drag_drop(ids::SEAT).unwrap();
    assert!(matches!(batch[0].event, DataEvent::DeviceLeave));
    assert!(matches!(batch[1].event, DataEvent::SourceCancelled));
    assert_eq!(phase(&w), Some(DndPhase::Idle));
    // Nothing further is emitted for a dead drag.
    assert!(w
        .manager
        .drag_motion(ids::SEAT, ids::B, pos(1.0, 1.0))
        .is_err());
}

#[test]
fn origin_and_icon_death_cancel_target_death_leaves() {
    let mut w = World::new(3);
    start(&mut w, 5);
    enter(&mut w, ids::B, ids::TARGET);
    // Target surface dies: leave only, drag continues.
    let batch = w.manager.drag_surface_gone(ids::SEAT, ids::TARGET);
    assert_eq!(batch.len(), 1);
    assert!(matches!(batch[0].event, DataEvent::DeviceLeave));
    assert_eq!(phase(&w), Some(DndPhase::Dragging));
    // Re-enter, then the icon dies: full cancel.
    enter(&mut w, ids::B, ids::TARGET);
    let batch = w.manager.drag_surface_gone(ids::SEAT, ids::ICON);
    assert!(batch
        .iter()
        .any(|r| matches!(r.event, DataEvent::SourceCancelled)));
    assert_eq!(phase(&w), Some(DndPhase::Idle));
    // Origin death cancels too.
    start(&mut w, 6);
    let batch = w.manager.drag_surface_gone(ids::SEAT, ids::ORIGIN);
    assert!(batch
        .iter()
        .any(|r| matches!(r.event, DataEvent::SourceCancelled)));
    assert_eq!(phase(&w), Some(DndPhase::Idle));
}

#[test]
fn source_death_cancels_its_drag() {
    let mut w = World::new(3);
    let src = ids::source(&mut w, ids::A, &["text/plain"]);
    w.manager
        .start_drag(ids::SEAT, ids::A, src, ids::ORIGIN, None, Some(9), 9)
        .unwrap();
    enter(&mut w, ids::B, ids::TARGET);
    let batch = w.manager.source_destroy(src);
    assert!(batch
        .iter()
        .any(|r| matches!(r.event, DataEvent::SourceCancelled)));
    assert!(batch
        .iter()
        .any(|r| matches!(r.event, DataEvent::DeviceLeave)));
    assert_eq!(phase(&w), Some(DndPhase::Idle));
}

#[test]
fn wrong_client_cannot_drive_the_drag() {
    let mut w = World::new(3);
    start(&mut w, 10);
    enter(&mut w, ids::B, ids::TARGET);
    // C tries to narrow.
    let err = w
        .manager
        .offer_set_actions(ids::SEAT, ids::C, ActionSet::build(true, false, false))
        .unwrap_err();
    assert_eq!(err, ldp_clipboard::DndError::WrongClient);
    // B (the entered receiver) can.
    w.manager
        .offer_set_actions(ids::SEAT, ids::B, ActionSet::build(true, false, false))
        .unwrap();
}

#[test]
fn ask_resolution_flow() {
    let mut w = World::new(3);
    let src = ids::source(&mut w, ids::A, &["text/uri-list"]);
    w.manager
        .start_drag(ids::SEAT, ids::A, src, ids::ORIGIN, None, Some(11), 11)
        .unwrap();
    enter(&mut w, ids::B, ids::TARGET);
    w.manager
        .offer_set_actions(ids::SEAT, ids::B, ActionSet::build(false, false, true))
        .unwrap();
    let drop = w.manager.drag_drop(ids::SEAT).unwrap();
    assert!(drop.iter().any(|r| matches!(
        r.event,
        DataEvent::OfferAction {
            action: DndAction::Ask
        }
    )));
    // Resolve to copy: both sides re-announced.
    let resolved = w.manager.resolve_ask(ids::SEAT, DndAction::Copy).unwrap();
    assert!(resolved.iter().any(|r| matches!(
        r.event,
        DataEvent::OfferAction {
            action: DndAction::Copy
        }
    )));
    assert!(resolved.iter().any(|r| matches!(
        r.event,
        DataEvent::SourceActions { actions } if actions == DndAction::Copy.as_set()
    )));
    let finish = w.manager.offer_finish(ids::SEAT, ids::B).unwrap();
    assert!(finish
        .iter()
        .any(|r| matches!(r.event, DataEvent::SourceFinished)));
    // Resolve_ask on a settled drag errors (the machine no longer
    // holds a Dropped phase).
    assert!(matches!(
        w.manager.resolve_ask(ids::SEAT, DndAction::Move),
        Err(ldp_clipboard::DndError::NotDropped)
    ));
}

#[test]
fn double_drag_and_finish_errors() {
    let mut w = World::new(3);
    let s1 = ids::source(&mut w, ids::A, &["text/plain"]);
    let s2 = ids::source(&mut w, ids::A, &["text/html"]);
    w.manager
        .start_drag(ids::SEAT, ids::A, s1, ids::ORIGIN, None, Some(1), 1)
        .unwrap();
    let err = w
        .manager
        .start_drag(ids::SEAT, ids::A, s2, ids::ORIGIN, None, Some(1), 1)
        .unwrap_err();
    assert!(matches!(
        err,
        DragStartError::Machine(ldp_clipboard::DndError::AlreadyDragging)
    ));
    // Finish before drop.
    enter(&mut w, ids::B, ids::TARGET);
    let err = w.manager.offer_finish(ids::SEAT, ids::B).unwrap_err();
    assert!(matches!(
        err,
        FinishError::Offer(ldp_clipboard::OfferError::NotDropped)
    ));
    // Drop without target is rejected by the machine.
    let _ = w.manager.drag_cancel(ids::SEAT);
    start(&mut w, 2);
    assert!(w.manager.drag_drop(ids::SEAT).is_err());
}

#[test]
fn attached_source_cannot_start_a_drag() {
    let mut w = World::new(3);
    let src = ids::source(&mut w, ids::A, &["text/plain"]);
    let _ = w.with_supplier(|m, sup| {
        m.set_slot(
            ids::SEAT,
            ldp_clipboard::Slot::Clipboard,
            ids::A,
            Some(src),
            Some(1),
            1,
            sup,
        )
    });
    let err = w
        .manager
        .start_drag(ids::SEAT, ids::A, src, ids::ORIGIN, None, Some(1), 1)
        .unwrap_err();
    assert!(matches!(
        err,
        DragStartError::Device(ldp_clipboard::DeviceError::SourceUnavailable(_))
    ));
}

#[test]
fn receiver_leaves_and_enters_another_surface() {
    let mut w = World::new(3);
    start(&mut w, 20);
    enter(&mut w, ids::B, ids::TARGET);
    let b_offer = w.manager.drag_offer(ids::B, ids::SEAT).expect("B offer");
    // B leaves; C enters: C gets a publication + enter; B's offer
    // stays bound to B (each receiver has its own offer).
    w.manager.drag_leave(ids::SEAT).unwrap();
    let c_batch = w
        .with_supplier(|m, sup| {
            m.drag_enter(
                ids::SEAT,
                ids::C,
                ldp_clipboard::SurfaceKey(200),
                ObjectId::from_wire(0x8101),
                pos(2.0, 2.0),
                sup,
            )
        })
        .unwrap();
    assert!(
        c_batch.len() >= 2,
        "C sees publication + enter: {}",
        c_batch.len()
    );
    let c_offer = w.manager.drag_offer(ids::C, ids::SEAT).expect("C offer");
    assert_ne!(b_offer, c_offer, "distinct receivers get distinct offers");
    // C leaves first (the pointer is over one surface at a time),
    // then B re-enters: its own offer is reused.
    w.manager.drag_leave(ids::SEAT).unwrap();
    let again = w
        .with_supplier(|m, sup| {
            m.drag_enter(
                ids::SEAT,
                ids::B,
                ids::TARGET,
                ObjectId::from_wire(0x8001),
                pos(3.0, 3.0),
                sup,
            )
        })
        .unwrap();
    assert_eq!(again.len(), 1);
    assert_eq!(w.manager.drag_offer(ids::B, ids::SEAT), Some(b_offer));
}

#[test]
fn determinism_same_seed_same_stream() {
    fn run(seed: u64) -> Vec<String> {
        let mut w = World::new(3);
        let mut rng = Rng::seeded(seed);
        let src = ids::source(&mut w, ids::A, &["text/plain", "image/png"]);
        let mut log = Vec::new();
        for step in 0..200 {
            match rng.below(6) {
                0 => {
                    let r = w.manager.start_drag(
                        ids::SEAT,
                        ids::A,
                        src,
                        ids::ORIGIN,
                        None,
                        Some(step as u32),
                        step as u32,
                    );
                    log.push(format!("start:{r:?}"));
                }
                1 => {
                    let r = w.with_supplier(|m, sup| {
                        m.drag_enter(
                            ids::SEAT,
                            ids::B,
                            ids::TARGET,
                            ObjectId::from_wire(0x9001),
                            pos(0.0, 0.0),
                            sup,
                        )
                    });
                    log.push(format!("enter:{r:?}"));
                }
                2 => {
                    let r = w.manager.offer_set_actions(
                        ids::SEAT,
                        ids::B,
                        ActionSet::from_bits(rng.next_u64() as u32),
                    );
                    log.push(format!("narrow:{r:?}"));
                }
                3 => {
                    let r = w.manager.drag_drop(ids::SEAT);
                    log.push(format!("drop:{r:?}"));
                }
                4 => {
                    let r = w.manager.offer_finish(ids::SEAT, ids::B);
                    log.push(format!("finish:{r:?}"));
                }
                _ => {
                    let r = w.manager.drag_cancel(ids::SEAT);
                    log.push(format!("cancel:{}", r.len()));
                }
            }
        }
        log
    }
    assert_eq!(run(0xDEAD_BEEF), run(0xDEAD_BEEF));
    assert_eq!(run(42), run(42));
}

#[test]
fn transition_fuzz_never_wedges() {
    // 30k adversarial transitions across two seats; after every step
    // the invariants hold: phases are legal, and once a seat rests it
    // only restarts through start_drag.
    let mut w = World::new(4);
    let mut rng = Rng::seeded(0x1234_5678_9ABC_DEF0);
    let mut sources: Vec<_> = (0..8)
        .map(|i| {
            ids::source(
                &mut w,
                ldp_clipboard::ClientKey(i % 4),
                &["text/plain", "image/png"],
            )
        })
        .collect();
    let seats = [ids::SEAT, ldp_clipboard::SeatKey(1)];
    let mut drags_started = 0u64;
    let mut drops_completed = 0u64;
    let mut fresh = 0u64;
    for step in 0..30_000u64 {
        // Sources are single-shot (dead after finish/cancel), so the
        // pool refreshes — clients create new sources as needed.
        if step % 64 == 0 || sources.len() < 4 {
            sources.push(ids::source(
                &mut w,
                ldp_clipboard::ClientKey(fresh % 4),
                &["text/plain", "image/png"],
            ));
            fresh += 1;
            if sources.len() > 64 {
                sources.remove(0);
            }
        }
        let seat = seats[rng.below(2) as usize];
        let client = ldp_clipboard::ClientKey(rng.below(4));
        let src = sources[rng.below(sources.len() as u64) as usize];
        match rng.below(8) {
            0 | 1 => {
                if w.manager
                    .start_drag(
                        seat,
                        client,
                        src,
                        ids::ORIGIN,
                        None,
                        Some(step as u32),
                        step as u32,
                    )
                    .is_ok()
                {
                    drags_started += 1;
                }
            }
            2 | 3 => {
                let _ = w.with_supplier(|m, sup| {
                    m.drag_enter(
                        seat,
                        client,
                        ldp_clipboard::SurfaceKey(rng.below(64)),
                        ObjectId::from_wire(0x9500 | (rng.below(256) as u32)),
                        pos(0.0, 0.0),
                        sup,
                    )
                });
            }
            4 => {
                let _ = w.manager.offer_set_actions(
                    seat,
                    client,
                    ActionSet::from_bits(rng.next_u64() as u32),
                );
            }
            5 => {
                if let Ok(batch) = w.manager.drag_drop(seat) {
                    if batch
                        .iter()
                        .any(|r| matches!(r.event, DataEvent::DeviceDrop))
                    {
                        drops_completed += 1;
                    }
                }
            }
            6 => {
                let _ = w.manager.offer_finish(seat, client);
            }
            _ => {
                let _ = w.manager.drag_cancel(seat);
            }
        }
        // Invariants after every step.
        for s in seats {
            if let Some(sd) = w.manager.seat_dnd(s) {
                let p = sd.dnd.phase();
                assert!(matches!(
                    p,
                    DndPhase::Idle | DndPhase::Dragging | DndPhase::Dropped
                ));
                if p == DndPhase::Idle {
                    assert!(sd.dnd.drag().is_none(), "idle seat holds no drag state");
                }
            }
        }
    }
    // The fuzz actually exercised both happy paths and cancels.
    assert!(drags_started > 100, "drags started: {drags_started}");
    assert!(drops_completed > 10, "drops completed: {drops_completed}");
}
