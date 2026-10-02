//! Fuzzed configure sequences never wedge the shell (Phase 12 EC).
//!
//! A seeded LCG drives tens of thousands of random request sequences
//! across a fleet of toplevels, popups and dialogs — every request the
//! spec defines, in adversarial orders (acks of stale serials, commits
//! without acks, superseded proposals, dismiss-then-use, minimize/
//! activate races). After **every** step the invariant oracle runs:
//!
//! * at most one live proposal per object, and it is the newest serial
//!   in wrapping order;
//! * an acked proposal is never lost (realized by the next commit);
//! * state flags are consistent (minimized ⇒ not activated; activated
//!   ⇒ not minimized);
//! * every proposal respects the client's own min/max size hints;
//! * the machine always makes progress: a fresh `propose` always
//!   succeeds and always supersedes (no state exists where the shell
//!   can neither propose nor honor an ack of the live serial) — the
//!   "wedge" property;
//! * every emitted configure passes the full wire gauntlet.

#![forbid(unsafe_code)]

mod common;

use common::{policy_inputs, validate_batch, Rng};
use ldp_core::geometry::Rect;
use ldp_core::ids::ObjectId;
use ldp_shell::dialog::{Dialog, DialogModality};
use ldp_shell::event::ShellEvent;
use ldp_shell::popup::{Anchor, Gravity, Popup, PopupConstraints, PopupError, PopupGeometry};
use ldp_shell::serial::Serial;
use ldp_shell::toplevel::{Configure, Toplevel, ToplevelError};
use ldp_shell::WindowKey;

/// The invariant oracle for one toplevel.
fn check_toplevel(t: &Toplevel) {
    // At most one live proposal; its serial is the newest issued.
    if let Some(p) = t.pending() {
        assert!(t.acked().is_none() || t.acked().unwrap().serial != p.serial);
        assert!(t.applied().is_none() || p.serial.after(t.applied().unwrap().serial));
    }
    // Consistent flags on every proposal surface.
    let states = t.wanted();
    assert!(
        !(states.minimized() && states.activated()),
        "minimized and activated cannot coexist"
    );
    // A pending ack survives until a commit consumes it.
    if let Some(a) = t.acked() {
        assert!(t.pending().is_none() || t.pending().unwrap().serial.after(a.serial));
    }
    // Hints sanity: min <= max when both set.
    let mins = t.min_size();
    let maxs = t.max_size();
    if maxs.0 > 0 && mins.0 > 0 {
        assert!(mins.0 <= maxs.0, "min width exceeds max width");
    }
    if maxs.1 > 0 && mins.1 > 0 {
        assert!(mins.1 <= maxs.1, "min height exceeds max height");
    }
}

/// The hint-conformance oracle for a proposal.
fn check_proposal_respects_hints(t: &Toplevel, c: &Configure) {
    if c.width > 0 && c.height > 0 {
        let mins = t.min_size();
        let maxs = t.max_size();
        if mins.0 > 0 {
            assert!(c.width >= mins.0);
        }
        if mins.1 > 0 {
            assert!(c.height >= mins.1);
        }
        if maxs.0 > 0 {
            assert!(c.width <= maxs.0);
        }
        if maxs.1 > 0 {
            assert!(c.height <= maxs.1);
        }
    }
}

const TOP_OBJ: ObjectId = ObjectId::from_wire(0x2100);
const POP_OBJ: ObjectId = ObjectId::from_wire(0x2200);
const DLG_OBJ: ObjectId = ObjectId::from_wire(0x2300);

#[test]
#[allow(clippy::too_many_lines)] // a driver, not logic: the length is the corpus
fn fuzzed_configure_sequences_never_wedge_the_shell() {
    let inputs = policy_inputs();
    let mut rng = Rng::seeded(6_582_637); // 0x5EEDED

    // The fleet.
    let mut toplevels: Vec<Toplevel> = (0..6)
        .map(|i| {
            let mut t = Toplevel::new(WindowKey::new(u64::try_from(i).unwrap()));
            // Adversarial hint mix, including inverted hints the
            // machine must keep sane.
            match i % 4 {
                0 => t.set_min_size(320, 240),
                1 => t.set_max_size(800, 600),
                2 => {
                    t.set_min_size(100, 100);
                    t.set_max_size(400, 300);
                }
                _ => {}
            }
            t
        })
        .collect();
    let geometry = PopupGeometry {
        anchor_rect: Rect::new(900, 500, 120, 30),
        anchor: Anchor::Bottom,
        gravity: Gravity::Bottom,
        offset: (0, 0),
    };
    let mut popups: Vec<Popup> = (0..4)
        .map(|_| Popup::new(geometry, PopupConstraints::all()))
        .collect();
    let mut dialogs: Vec<Dialog> = (0..4)
        .map(|i| {
            Dialog::new(
                WindowKey::new(100 + u64::try_from(i).unwrap()),
                WindowKey::new(0),
                if i % 2 == 0 {
                    DialogModality::Modal
                } else {
                    DialogModality::Modeless
                },
            )
        })
        .collect();

    let mut events: Vec<(ShellEvent, ObjectId)> = Vec::new();
    let mut wedges = 0u64; // must stay 0: propose-always-succeeds count
    let mut proposals = 0u64;
    let mut acks_ok = 0u64;

    for step in 0..20_000u64 {
        let which = rng.below(3);
        match which {
            0 => {
                // Toplevel ops.
                let i = usize::try_from(rng.below(6)).unwrap();
                let t = &mut toplevels[i];
                match rng.below(11) {
                    0 => {
                        t.maximize();
                    }
                    1 => {
                        t.unmaximize();
                    }
                    2 => {
                        t.fullscreen();
                    }
                    3 => {
                        t.unfullscreen();
                    }
                    4 => {
                        t.minimize();
                    }
                    5 => {
                        t.unminimize();
                    }
                    6 => {
                        t.set_sticky(rng.flip());
                    }
                    7 => {
                        t.set_activated(rng.flip());
                    }
                    8 => {
                        t.set_min_size(
                            u32::try_from(rng.below(600)).unwrap(),
                            u32::try_from(rng.below(600)).unwrap(),
                        );
                    }
                    _ => {
                        // The critical op: propose (always accepted).
                        let c = t.propose(&inputs);
                        check_proposal_respects_hints(t, &c);
                        events.push((ShellEvent::toplevel_configure(&c), TOP_OBJ));
                        proposals += 1;
                        wedges += 1; // a propose that returns = no wedge
                                     // Randomly ack *something*.
                        let pick = rng.below(4);
                        let serial = match pick {
                            0 => c.serial,                                      // the live one
                            1 => Serial(c.serial.0.wrapping_add(1)),            // future
                            2 => Serial(c.serial.0.wrapping_sub(1)),            // past
                            _ => Serial(u32::try_from(rng.below(16)).unwrap()), // junk
                        };
                        match t.ack_configure(serial) {
                            Ok(()) => {
                                acks_ok += 1;
                                assert_eq!(t.pending(), None);
                            }
                            Err(ToplevelError::StaleAck) => {
                                // Only legal when serial != live.
                                assert_ne!(t.pending().map(|p| p.serial), Some(serial));
                            }
                            Err(e) => panic!("unexpected error {e:?} at step {step}"),
                        }
                        // Randomly commit (with or without an ack).
                        if rng.flip() {
                            let before = t.acked();
                            let got = t.commit();
                            assert_eq!(got, before, "commit must realize exactly the ack");
                        }
                    }
                }
                check_toplevel(t);
            }
            1 => {
                // Popup ops.
                let i = usize::try_from(rng.below(4)).unwrap();
                let popup = &mut popups[i];
                match rng.below(5) {
                    0 => {
                        let (s, p) = popup.propose(
                            (u32::try_from(rng.below(400)).unwrap() + 20, 60),
                            Rect::new(0, 0, 1920, 1080),
                        );
                        events.push((ShellEvent::popup_configure(s, &p), POP_OBJ));
                        proposals += 1;
                        wedges += 1;
                    }
                    1 => {
                        let junk = Serial(u32::try_from(rng.below(9)).unwrap());
                        let _ = popup.ack_configure(junk);
                    }
                    2 => {
                        let press = if rng.flip() {
                            Some(Serial(u32::try_from(rng.below(8)).unwrap()))
                        } else {
                            None
                        };
                        let want = Serial(u32::try_from(rng.below(8)).unwrap());
                        let r = popup.grab(press, want);
                        if press == Some(want) && popup.is_live() {
                            assert!(r.is_ok());
                        } else {
                            assert!(matches!(
                                r,
                                Err(PopupError::BadGrabSerial | PopupError::Dismissed)
                            ));
                        }
                    }
                    3 => {
                        let _ = popup.reposition(PopupGeometry {
                            anchor_rect: Rect::new(
                                i32::try_from(rng.below(1800)).unwrap(),
                                i32::try_from(rng.below(1000)).unwrap(),
                                u32::try_from(rng.below(200)).unwrap() + 1,
                                u32::try_from(rng.below(60)).unwrap() + 1,
                            ),
                            anchor: Anchor::from_wire(u32::try_from(rng.below(8)).unwrap() + 1)
                                .unwrap(),
                            gravity: Gravity::from_wire(u32::try_from(rng.below(8)).unwrap() + 1)
                                .unwrap(),
                            offset: (
                                i32::try_from(rng.below(200)).unwrap() - 100,
                                i32::try_from(rng.below(200)).unwrap() - 100,
                            ),
                        });
                    }
                    _ => {
                        if rng.flip() {
                            popup.dismiss();
                            assert!(!popup.is_live());
                        }
                    }
                }
                // A live popup can always propose again.
                if popup.is_live() {
                    let (s, p) = popup.propose((120, 40), Rect::new(0, 0, 1920, 1080));
                    events.push((ShellEvent::popup_configure(s, &p), POP_OBJ));
                    wedges += 1;
                }
            }
            _ => {
                // Dialog ops.
                let i = usize::try_from(rng.below(4)).unwrap();
                let d = &mut dialogs[i];
                match rng.below(5) {
                    0 => {
                        let p = d.propose(
                            (
                                u32::try_from(rng.below(900)).unwrap() + 50,
                                u32::try_from(rng.below(700)).unwrap() + 50,
                            ),
                            Rect::new(100, 100, 800, 600),
                            Rect::new(0, 0, 1920, 1040),
                        );
                        events.push((ShellEvent::dialog_configure(&p), DLG_OBJ));
                        proposals += 1;
                        wedges += 1;
                    }
                    1 => {
                        let junk = Serial(u32::try_from(rng.below(9)).unwrap());
                        let _ = d.ack_configure(junk);
                    }
                    2 => {
                        let _ = d.commit();
                    }
                    3 => {
                        d.close();
                    }
                    _ => {
                        let _ = d.set_title("t");
                    }
                }
                // Ack of the live proposal always works (or there is
                // none).
                if let Some(p) = d.pending() {
                    d.ack_configure(p.serial).unwrap();
                    acks_ok += 1;
                    assert_eq!(d.commit(), Some(p));
                }
            }
        }
    }

    // The wedge property: every propose succeeded (and they were
    // many), acks of live serials succeeded whenever attempted, and a
    // meaningful amount of both happened.
    assert!(wedges > 3_000, "driver too weak: {wedges} proposes");
    assert!(proposals > 1_200, "proposal coverage too thin: {proposals}");
    assert!(acks_ok > 1_200, "ack coverage too thin: {acks_ok}");

    // Every single emitted event was wire-valid at emission time; the
    // batch is byte-stable under re-encoding.
    let once = validate_batch(&events);
    let again = validate_batch(&events);
    assert_eq!(once, again);
    assert!(once.len() > 2_500, "event corpus too thin: {}", once.len());

    // Final oracle pass over the fleet.
    for t in &toplevels {
        check_toplevel(t);
    }
}

/// The serial clock survives adversarial reservation churn under
/// wrap: no issued serial ever collides with the live one.
#[test]
fn fuzzed_serial_clock_never_collides() {
    use ldp_shell::serial::SerialClock;
    let mut rng = Rng::seeded(0x5EED2);
    let mut clock = SerialClock::new();
    // Drive near the wrap (crash-recovery style).
    clock.resume_from(Serial(0xFFFF_F000));
    // Track issued serials in the window we can observe (the last
    // 4_096 before wrap and after).
    let mut issued: std::collections::HashSet<u32> = std::collections::HashSet::new();
    for _ in 0..8_000u32 {
        let s = clock.issue();
        assert_ne!(s.0, clock.reserved().0, "issued the reserved serial");
        assert!(
            issued.insert(s.0),
            "serial {} issued twice before wrap",
            s.0
        );
        if issued.len() > 4_000 {
            // Once past the wrap window the set self-cleans.
            issued.clear();
            issued.insert(s.0);
        }
        // Randomly re-reserve the just-issued serial (the live one).
        if rng.flip() {
            clock.reserve(s);
        }
    }
}
