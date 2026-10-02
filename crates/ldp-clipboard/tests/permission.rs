//! THE Phase 13 exit criterion (part 4): permission gates enforced.
//!
//! The full-matrix proof through the manager (the dispatcher's eye
//! view): every `receive` on a selection offer consults the
//! `clipboard_read` scope — manifest baseline or escalated token —
//! while DnD drop receives pass on user intent. Denials are
//! deny-by-default, carry audit records, and leave *no trace* (no
//! transfer, no `send`, the offer's receive slot unstamped).
//!
//! The token escalation path is exercised with real `AccessToken`s
//! (wrong-scope tokens are as good as none; expired tokens are the
//! dispatcher's check, not the gate's — noted in the docs).

mod common;

use common::{ids, World};
use ldp_clipboard::event::DataEvent;
use ldp_clipboard::manager::ReceiveError;
use ldp_clipboard::permission::{audit_line, AllowBasis, GateDecision, GateOp};
use ldp_clipboard::ActionSet;
use ldp_clipboard::Transfer;
use ldp_core::caps::{Scope, ScopeSet};
use ldp_core::geometry::PointF;
use ldp_core::ids::ObjectId;
use ldp_core::time::Mono;
use ldp_core::token::AccessToken;

fn pipe_write() -> std::os::fd::OwnedFd {
    ldp_clipboard::pipe::pipe().expect("pipe").write.into_fd()
}

fn token(scopes: ScopeSet, expiry_ms: u64) -> AccessToken {
    AccessToken::from_words(
        [9, 9, 9, 9, 9, 9, 9, 9],
        scopes,
        Some(Mono::from_ms(expiry_ms)),
    )
}

fn publish(w: &mut World) -> ldp_clipboard::OfferKey {
    let src = ids::source(w, ids::A, &["text/plain", "image/png"]);
    w.with_supplier(|m, sup| {
        m.set_slot(
            ids::SEAT,
            ldp_clipboard::Slot::Clipboard,
            ids::A,
            Some(src),
            Some(1),
            1,
            sup,
        )
    })
    .expect("selection");
    w.manager
        .clipboard_offer(ids::B, ids::SEAT)
        .expect("B's offer")
}

#[test]
fn deny_by_default_and_no_trace_on_denial() {
    let mut w = World::new(3);
    let offer = publish(&mut w);
    let before = w.manager.transfers().len();
    let err = w
        .manager
        .offer_receive(
            ids::B,
            offer,
            "text/plain",
            pipe_write(),
            ScopeSet::NONE,
            None,
        )
        .unwrap_err();
    // The denial carries the audit record for the trail.
    match &err {
        ReceiveError::Denied(record) => {
            assert_eq!(record.op, GateOp::ClipboardRead);
            assert!(matches!(
                record.decision,
                GateDecision::Denied {
                    missing: Scope::ClipboardRead
                }
            ));
            let line = audit_line(ids::B.0, record);
            assert!(line.contains("\"decision\":\"deny\""));
            assert!(line.contains("\"client\":1"));
        }
        other => panic!("expected Denied, got {other:?}"),
    }
    // No trace: no transfer row, no send, and the MIME is still
    // receivable once granted (the denial did not consume the slot).
    assert_eq!(w.manager.transfers().len(), before);
    let ok = w.manager.offer_receive(
        ids::B,
        offer,
        "text/plain",
        pipe_write(),
        ScopeSet::single(Scope::ClipboardRead),
        None,
    );
    assert!(
        ok.is_ok(),
        "denial must not consume the receive slot: {ok:?}"
    );
    let (_, tid) = ok.unwrap();
    assert!(w.manager.transfers().get(tid).is_some());
}

#[test]
fn manifest_baseline_allows_read() {
    let mut w = World::new(3);
    let offer = publish(&mut w);
    // A broad manifest that includes the scope.
    let manifest = ScopeSet::single(Scope::ClipboardRead).with(Scope::GlobalShortcut);
    let ok = w
        .manager
        .offer_receive(ids::B, offer, "image/png", pipe_write(), manifest, None);
    assert!(ok.is_ok(), "{ok:?}");
    // A manifest without the scope denies.
    let offer_c = w
        .manager
        .clipboard_offer(ids::C, ids::SEAT)
        .expect("C's offer");
    let narrow = ScopeSet::single(Scope::GlobalShortcut);
    let err = w
        .manager
        .offer_receive(ids::C, offer_c, "text/plain", pipe_write(), narrow, None)
        .unwrap_err();
    assert!(matches!(err, ReceiveError::Denied(_)), "{err:?}");
}

#[test]
fn token_escalation_allows_read() {
    let mut w = World::new(3);
    let offer = publish(&mut w);
    // No manifest scope, but a token carrying it.
    let t = token(ScopeSet::single(Scope::ClipboardRead), 60_000);
    let ok = w.manager.offer_receive(
        ids::B,
        offer,
        "text/plain",
        pipe_write(),
        ScopeSet::NONE,
        Some(&t),
    );
    assert!(ok.is_ok(), "{ok:?}");
    // A token with the wrong scope is as good as none.
    let wrong = token(ScopeSet::single(Scope::Screenshot), 60_000);
    let offer_c = w
        .manager
        .clipboard_offer(ids::C, ids::SEAT)
        .expect("C's offer");
    let err = w
        .manager
        .offer_receive(
            ids::C,
            offer_c,
            "text/plain",
            pipe_write(),
            ScopeSet::NONE,
            Some(&wrong),
        )
        .unwrap_err();
    assert!(matches!(err, ReceiveError::Denied(_)), "{err:?}");
    // Manifest wins even without a token; token and manifest both
    // present is fine (manifest checked first).
    let manifest = ScopeSet::single(Scope::ClipboardRead);
    let ok = w
        .manager
        .offer_receive(ids::C, offer_c, "text/plain", pipe_write(), manifest, None);
    assert!(ok.is_ok(), "{ok:?}");
}

#[test]
fn dnd_drop_receive_is_user_intent_authorized() {
    let mut w = World::new(3);
    let src = ids::source(&mut w, ids::A, &["text/uri-list"]);
    w.manager
        .start_drag(ids::SEAT, ids::A, src, ids::ORIGIN, None, Some(5), 5)
        .expect("start_drag");
    w.with_supplier(|m, sup| {
        m.drag_enter(
            ids::SEAT,
            ids::B,
            ids::TARGET,
            ObjectId::from_wire(0xA001),
            PointF { x: 0.0, y: 0.0 },
            sup,
        )
    })
    .expect("enter");
    w.manager
        .offer_set_actions(ids::SEAT, ids::B, ActionSet::build(true, false, false))
        .unwrap();
    w.manager.drag_drop(ids::SEAT).expect("drop");
    // The dropped-on receiver receives with NO scope at all: the
    // user's drop is the authorization.
    let offer = w.manager.drag_offer(ids::B, ids::SEAT).expect("drag offer");
    let ok = w.manager.offer_receive(
        ids::B,
        offer,
        "text/uri-list",
        pipe_write(),
        ScopeSet::NONE,
        None,
    );
    assert!(ok.is_ok(), "drop receive must not need the scope: {ok:?}");
    // A third party (C) cannot use the drag offer — offers are
    // addressed (the manager rejects foreign clients outright).
    let err = w
        .manager
        .offer_receive(
            ids::C,
            offer,
            "text/uri-list",
            pipe_write(),
            ScopeSet::NONE,
            None,
        )
        .unwrap_err();
    assert!(matches!(err, ReceiveError::UnknownOffer), "{err:?}");
    // Finish the drag.
    let finish = w.manager.offer_finish(ids::SEAT, ids::B).expect("finish");
    assert!(finish
        .iter()
        .any(|r| matches!(r.event, DataEvent::SourceFinished)));
}

#[test]
fn primary_selection_reads_are_gated_identically() {
    let mut w = World::new(3);
    let src = ids::source(&mut w, ids::A, &["text/plain"]);
    w.with_supplier(|m, sup| {
        m.set_slot(
            ids::SEAT,
            ldp_clipboard::Slot::Primary,
            ids::A,
            Some(src),
            Some(2),
            2,
            sup,
        )
    })
    .expect("primary");
    let offer = w.manager.primary_offer(ids::B, ids::SEAT).expect("offer");
    let err = w
        .manager
        .offer_receive(
            ids::B,
            offer,
            "text/plain",
            pipe_write(),
            ScopeSet::NONE,
            None,
        )
        .unwrap_err();
    assert!(matches!(err, ReceiveError::Denied(_)), "{err:?}");
    let ok = w.manager.offer_receive(
        ids::B,
        offer,
        "text/plain",
        pipe_write(),
        ScopeSet::single(Scope::ClipboardRead),
        None,
    );
    assert!(ok.is_ok(), "{ok:?}");
}

#[test]
fn setting_the_selection_needs_no_scope() {
    // Ownership is not a read: an empty-scope client may own the
    // clipboard (and its own content flows back with no gate).
    let mut w = World::new(3);
    let src = ids::source(&mut w, ids::C, &["text/plain"]);
    let ok = w.with_supplier(|m, sup| {
        m.set_slot(
            ids::SEAT,
            ldp_clipboard::Slot::Clipboard,
            ids::C,
            Some(src),
            Some(1),
            1,
            sup,
        )
    });
    assert!(ok.is_ok(), "an unprivileged client may set: {ok:?}");
    // And other clients' reads of it are still gated.
    let offer = w.manager.clipboard_offer(ids::B, ids::SEAT).expect("offer");
    let err = w
        .manager
        .offer_receive(
            ids::B,
            offer,
            "text/plain",
            pipe_write(),
            ScopeSet::NONE,
            None,
        )
        .unwrap_err();
    assert!(matches!(err, ReceiveError::Denied(_)), "{err:?}");
}

#[test]
fn gate_basis_reporting() {
    // The pure gate: every allow basis is distinguishable.
    let (d, _) = ldp_clipboard::permission::check(
        GateOp::ClipboardRead,
        ScopeSet::single(Scope::ClipboardRead),
        None,
    );
    assert!(matches!(
        d,
        GateDecision::Allowed {
            basis: AllowBasis::Manifest
        }
    ));
    let (d, _) = ldp_clipboard::permission::check(
        GateOp::ClipboardRead,
        ScopeSet::NONE,
        Some(&token(ScopeSet::single(Scope::ClipboardRead), 60_000)),
    );
    assert!(matches!(
        d,
        GateDecision::Allowed {
            basis: AllowBasis::Token
        }
    ));
    let (d, _) = ldp_clipboard::permission::check(GateOp::DropReceive, ScopeSet::NONE, None);
    assert!(matches!(
        d,
        GateDecision::Allowed {
            basis: AllowBasis::Unrestricted
        }
    ));
}

#[test]
fn client_teardown_cleans_everything() {
    // The owner disconnecting: offers die, null announcements flow,
    // transfers cancel.
    let mut w = World::new(3);
    let offer = publish(&mut w);
    let (_, tid) = w
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
    assert!(w.manager.transfers().get(tid).is_some());
    let batch = w.manager.client_gone(ids::A);
    // B and C see selection(null) (their offers were A's content).
    assert!(batch
        .iter()
        .any(|r| matches!(r.event, DataEvent::DeviceSelection { offer: None })));
    // The receiver's transfer was not A's — it survives until B
    // disconnects; then it cancels.
    let batch_b = w.manager.client_gone(ids::B);
    assert!(batch_b
        .iter()
        .any(|r| matches!(r.event, DataEvent::DeviceSelection { offer: None })));
    let cancelled = w.manager.transfers().get(tid).map(Transfer::state);
    assert!(matches!(
        cancelled,
        Some(ldp_clipboard::TransferState::Cancelled(_))
    ));
    assert_eq!(w.manager.transfers().client_transfers(ids::B), 0);
}
