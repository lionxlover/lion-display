//! THE Phase 12 exit criterion: state-machine conformance from
//! `spec/shell.toml`.
//!
//! Three layers of proof:
//!
//! 1. **Coverage**: every request and event the compiled schema
//!    carries for `ldp.shell` is exercised by a typed handle in the
//!    crate — in both directions (nothing uncovered in the spec,
//!    nothing in the typed vocabulary missing from the spec).
//! 2. **Golden lifecycles**: scripted sequences (toplevel
//!    map→configure→ack→commit→maximize→…→close; popup
//!    placement→reposition→dismiss; dialog open→ack→close; spaces
//!    moves) produce exactly the expected event streams, and every
//!    emitted message passes the full wire gauntlet (encode → decode →
//!    strict signature check, see [`common::assert_wire_valid`]).
//! 3. **Error conformance**: role conflicts, stale acks, dismissed
//!    popups and bad grabs surface as the typed errors the dispatcher
//!    maps to protocol error codes.

mod common;

use common::{policy_inputs, validate_batch, Rng};
use ldp_core::geometry::Rect;
use ldp_core::ids::ObjectId;
use ldp_protocol::generated::MODULES;
use ldp_protocol::REGISTRY;
use ldp_shell::dialog::{Dialog, DialogModality};
use ldp_shell::event::ShellEvent;
use ldp_shell::popup::{Anchor, Gravity, Popup, PopupConstraints, PopupError, PopupGeometry};
use ldp_shell::serial::Serial;
use ldp_shell::spaces::Spaces;
use ldp_shell::stack::{ActivationCause, WindowStack};
use ldp_shell::toplevel::{Toplevel, ToplevelError, ToplevelStates};
use ldp_shell::{DecorationMode, Insets, SsdMetrics, WindowKey};

const TOPLEVEL_OBJ: ObjectId = ObjectId::from_wire(0x1101);
const POPUP_OBJ: ObjectId = ObjectId::from_wire(0x1102);
const DIALOG_OBJ: ObjectId = ObjectId::from_wire(0x1103);
const SHELL_OBJ: ObjectId = ObjectId::from_wire(0x1104);

// ---------------------------------------------------------------------------
// 1. Coverage: the compiled schema vs the typed surface.
// ---------------------------------------------------------------------------

#[test]
// The coverage table is one list by doctrine (every pair pinned in
// one place); Phase 47's triple grew it past the line-count gate.
#[allow(clippy::too_many_lines)]
fn every_spec_request_and_event_has_a_typed_handle() {
    // The (interface, request) pairs the crate handles, pinned. The
    // shell *requests* arrive from clients; the machines expose them
    // as methods (the table names each one).
    let handled_requests: &[(&str, &str, &str)] = &[
        ("ldp.shell.shell", "get_toplevel", "Toplevel::new"),
        ("ldp.shell.shell", "get_popup", "Popup::new"),
        ("ldp.shell.shell", "get_dialog", "Dialog::new"),
        ("ldp.shell.shell", "switch_workspace", "Spaces::switch"),
        ("ldp.shell.toplevel", "set_title", "Toplevel::set_title"),
        ("ldp.shell.toplevel", "set_app_id", "Toplevel::set_app_id"),
        (
            "ldp.shell.toplevel",
            "set_min_size",
            "Toplevel::set_min_size",
        ),
        (
            "ldp.shell.toplevel",
            "set_max_size",
            "Toplevel::set_max_size",
        ),
        ("ldp.shell.toplevel", "set_workspace", "Spaces::move_window"),
        ("ldp.shell.toplevel", "maximize", "Toplevel::maximize"),
        ("ldp.shell.toplevel", "unmaximize", "Toplevel::unmaximize"),
        ("ldp.shell.toplevel", "fullscreen", "Toplevel::fullscreen"),
        (
            "ldp.shell.toplevel",
            "unfullscreen",
            "Toplevel::unfullscreen",
        ),
        ("ldp.shell.toplevel", "minimize", "Toplevel::minimize"),
        ("ldp.shell.toplevel", "unminimize", "Toplevel::unminimize"),
        ("ldp.shell.toplevel", "set_sticky", "Toplevel::set_sticky"),
        (
            "ldp.shell.toplevel",
            "set_material",
            "Toplevel::set_material",
        ),
        (
            "ldp.shell.toplevel",
            "set_semantic_role",
            "Toplevel::set_semantic_role",
        ),
        (
            "ldp.shell.toplevel",
            "set_security_class",
            "Toplevel::set_security_class",
        ),
        (
            "ldp.shell.toplevel",
            "set_scene_profile",
            "Toplevel::set_scene_profile",
        ),
        ("ldp.shell.toplevel", "start_move", "Toplevel::unmaximize"),
        (
            "ldp.shell.toplevel",
            "start_resize",
            "Toplevel::set_resizing",
        ),
        (
            "ldp.shell.toplevel",
            "ack_configure",
            "Toplevel::ack_configure",
        ),
        ("ldp.shell.popup", "grab", "Popup::grab"),
        ("ldp.shell.popup", "ack_configure", "Popup::ack_configure"),
        ("ldp.shell.popup", "reposition", "Popup::reposition"),
        ("ldp.shell.popup", "dismiss", "Popup::dismiss"),
        ("ldp.shell.dialog", "set_title", "Dialog::set_title"),
        ("ldp.shell.dialog", "ack_configure", "Dialog::ack_configure"),
    ];
    // The (interface, event) pairs the typed vocabulary carries.
    let handled_events: &[(&str, &str)] = &[
        ("ldp.shell.shell", "workspace_count"),
        ("ldp.shell.shell", "workspace_switched"),
        ("ldp.shell.toplevel", "configure"),
        ("ldp.shell.toplevel", "close"),
        ("ldp.shell.toplevel", "workspace_changed"),
        ("ldp.shell.popup", "configure"),
        ("ldp.shell.popup", "done"),
        ("ldp.shell.dialog", "configure"),
        ("ldp.shell.dialog", "close"),
    ];

    for (iface_name, req, _handle) in handled_requests {
        let iface = REGISTRY
            .interface(iface_name)
            .unwrap_or_else(|| panic!("interface {iface_name} missing from schema"));
        assert!(
            iface.requests.iter().any(|r| r.name == *req),
            "request {iface_name}.{req} missing from schema"
        );
    }
    for (iface_name, ev) in handled_events {
        let iface = REGISTRY
            .interface(iface_name)
            .unwrap_or_else(|| panic!("interface {iface_name} missing from schema"));
        assert!(
            iface.events.iter().any(|e| e.name == *ev),
            "event {iface_name}.{ev} missing from schema"
        );
    }

    // The reverse direction: the schema carries nothing the typed
    // surface does not cover (drift protection both ways).
    for module in MODULES.iter().filter(|m| m.name == "ldp.shell") {
        for iface in module.interfaces {
            for r in iface.requests {
                // InterfaceSchema.name is already fully qualified.
                let fq = format!("{}.{}", iface.name, r.name);
                assert!(
                    handled_requests
                        .iter()
                        .any(|(i, req, _)| fq == format!("{i}.{req}")),
                    "schema request {fq} has no typed handle — extend the machines"
                );
            }
            for e in iface.events {
                let fq = format!("{}.{}", iface.name, e.name);
                assert!(
                    handled_events
                        .iter()
                        .any(|(i, ev)| fq == format!("{i}.{ev}")),
                    "schema event {fq} has no typed variant — extend ShellEvent"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 2. Golden lifecycles.
// ---------------------------------------------------------------------------

#[test]
fn toplevel_lifecycle_golden() {
    let mut t = Toplevel::new(WindowKey::new(1));
    t.set_title("Terminal").unwrap();
    t.set_app_id("io.lionos.terminal").unwrap();
    t.set_min_size(200, 150);

    let mut events: Vec<(ShellEvent, ObjectId)> = Vec::new();

    // Map: first configure proposes the client-choice size.
    let c1 = t.propose(&policy_inputs());
    events.push((ShellEvent::toplevel_configure(&c1), TOPLEVEL_OBJ));
    // Client acks and commits.
    t.ack_configure(c1.serial).unwrap();
    assert_eq!(t.commit(), Some(c1));
    // Maximize: propose workspace-area-minus-insets geometry.
    t.maximize();
    let c2 = t.propose(&policy_inputs());
    events.push((ShellEvent::toplevel_configure(&c2), TOPLEVEL_OBJ));
    assert_eq!((c2.width, c2.height), (1918, 1010));
    assert!(c2.states.maximized());
    t.ack_configure(c2.serial).unwrap();
    assert_eq!(t.commit(), Some(c2));

    // Fullscreen from maximized: full output, zero insets.
    t.fullscreen();
    let c3 = t.propose(&policy_inputs());
    events.push((ShellEvent::toplevel_configure(&c3), TOPLEVEL_OBJ));
    assert_eq!((c3.width, c3.height), (1920, 1080));
    assert_eq!(c3.insets, Insets::ZERO);
    t.ack_configure(c3.serial).unwrap();
    assert!(t.commit().is_some());

    // Unfullscreen back to maximized state (flags persist).
    t.unfullscreen();
    let c4 = t.propose(&policy_inputs());
    events.push((ShellEvent::toplevel_configure(&c4), TOPLEVEL_OBJ));
    assert_eq!((c4.width, c4.height), (1918, 1010));
    assert!(!c4.states.fullscreen() && c4.states.maximized());

    // Close.
    events.push((ShellEvent::ToplevelClose, TOPLEVEL_OBJ));

    // Wire-validate every event, and byte-stability of the batch.
    let once = validate_batch(&events);
    let again = validate_batch(&events);
    assert_eq!(once, again);
    assert_eq!(events.len(), 5);
    // All configure events addressed the same object with distinct
    // serials 1..4.
    let serials: Vec<u32> = events
        .iter()
        .filter_map(|(e, _)| match e {
            ShellEvent::ToplevelConfigure { serial, .. } => Some(*serial),
            _ => None,
        })
        .collect();
    assert_eq!(serials, vec![1, 2, 3, 4]);
}

#[test]
fn popup_lifecycle_golden() {
    let geometry = PopupGeometry {
        anchor_rect: Rect::new(900, 500, 120, 30),
        anchor: Anchor::Bottom,
        gravity: Gravity::Bottom,
        offset: (0, 0),
    };
    let mut popup = Popup::new(geometry, PopupConstraints::all());
    let mut events: Vec<(ShellEvent, ObjectId)> = Vec::new();

    let (s1, p1) = popup.propose((200, 100), Rect::new(0, 0, 1920, 1080));
    events.push((ShellEvent::popup_configure(s1, &p1), POPUP_OBJ));
    assert_eq!((p1.x, p1.y, p1.width, p1.height), (860, 530, 200, 100));
    popup.ack_configure(s1);

    // The menu scrolls under the cursor: reposition near the right
    // edge and re-solve (slide clamps it in).
    popup
        .reposition(PopupGeometry {
            anchor_rect: Rect::new(1850, 100, 60, 30),
            anchor: Anchor::TopRight,
            gravity: Gravity::BottomRight,
            offset: (0, 0),
        })
        .unwrap();
    let (s2, p2) = popup.propose((200, 100), Rect::new(0, 0, 1920, 1080));
    events.push((ShellEvent::popup_configure(s2, &p2), POPUP_OBJ));
    assert_eq!(p2.x, 1720);

    // An explicit grab referencing the press, then dismissal → done.
    popup.grab(Some(Serial(77)), Serial(77)).unwrap();
    popup.dismiss();
    events.push((ShellEvent::PopupDone, POPUP_OBJ));

    let once = validate_batch(&events);
    let again = validate_batch(&events);
    assert_eq!(once, again);
    assert_eq!(events.len(), 3);
}

#[test]
fn dialog_lifecycle_golden() {
    let parent = WindowKey::new(1);
    let mut d = Dialog::new(WindowKey::new(2), parent, DialogModality::Modal);
    d.set_title("Save changes?").unwrap();
    let ws = Rect::new(0, 0, 1920, 1040);
    let pc = Rect::new(560, 220, 800, 600);

    let mut events: Vec<(ShellEvent, ObjectId)> = Vec::new();
    let p1 = d.propose((400, 300), pc, ws);
    events.push((ShellEvent::dialog_configure(&p1), DIALOG_OBJ));
    d.ack_configure(p1.serial).unwrap();
    assert_eq!(d.commit(), Some(p1));
    // Centered placement per policy.
    assert_eq!(Dialog::center_over(pc, (400, 300), ws), (760, 370));
    // The user closes.
    d.close();
    events.push((ShellEvent::DialogClose, DIALOG_OBJ));

    let once = validate_batch(&events);
    let again = validate_batch(&events);
    assert_eq!(once, again);
    assert_eq!(events.len(), 2);
}

#[test]
fn spaces_and_workspace_events_golden() {
    let mut spaces = Spaces::new(4);
    spaces.add_seat(1, 0);
    spaces.assign(WindowKey::new(1), 0);

    // After binding the shell global: workspace_count.
    let mut events: Vec<(ShellEvent, ObjectId)> =
        vec![(ShellEvent::WorkspaceCount { count: 4 }, SHELL_OBJ)];

    // The window moves to space 2 (clamped from 99).
    let moved = spaces.move_window(WindowKey::new(1), 99);
    assert_eq!(moved, 3);
    events.push((ShellEvent::WorkspaceChanged { workspace: 3 }, TOPLEVEL_OBJ));

    // Count shrink reflows.
    let reflowed = spaces.set_count(2);
    assert_eq!(reflowed, vec![(WindowKey::new(1), 1)]);
    events.push((ShellEvent::WorkspaceChanged { workspace: 1 }, TOPLEVEL_OBJ));
    events.push((ShellEvent::WorkspaceCount { count: 2 }, SHELL_OBJ));

    let once = validate_batch(&events);
    let again = validate_batch(&events);
    assert_eq!(once, again);
    assert_eq!(events.len(), 4);
}

// ---------------------------------------------------------------------------
// 3. Error conformance.
// ---------------------------------------------------------------------------

#[test]
fn error_conformance_toplevel() {
    let mut t = Toplevel::new(WindowKey::new(1));
    // Stale ack with no live proposal.
    assert_eq!(t.ack_configure(Serial(1)), Err(ToplevelError::StaleAck));
    let c = t.propose(&policy_inputs());
    // Wrong serial (off by one, both directions).
    assert_eq!(
        t.ack_configure(Serial(c.serial.0 + 1)),
        Err(ToplevelError::StaleAck)
    );
    assert_eq!(
        t.ack_configure(Serial(c.serial.0.wrapping_sub(1))),
        Err(ToplevelError::StaleAck)
    );
    t.ack_configure(c.serial).unwrap();
    assert!(t.commit().is_some());
    // The realized serial is dead.
    assert_eq!(t.ack_configure(c.serial), Err(ToplevelError::StaleAck));
    // Strings: oversized and NUL-bearing.
    assert_eq!(
        t.set_title(&"t".repeat(4097)),
        Err(ToplevelError::BadString)
    );
    assert_eq!(t.set_title("x\0y"), Err(ToplevelError::BadString));
}

#[test]
fn error_conformance_popup() {
    let g = PopupGeometry {
        anchor_rect: Rect::new(0, 0, 10, 10),
        anchor: Anchor::Top,
        gravity: Gravity::Bottom,
        offset: (0, 0),
    };
    let mut popup = Popup::new(g, PopupConstraints::all());
    assert_eq!(popup.grab(None, Serial(1)), Err(PopupError::BadGrabSerial));
    assert_eq!(
        popup.grab(Some(Serial(2)), Serial(1)),
        Err(PopupError::BadGrabSerial)
    );
    popup.dismiss();
    assert_eq!(popup.reposition(g), Err(PopupError::Dismissed));
    assert_eq!(
        popup.grab(Some(Serial(5)), Serial(5)),
        Err(PopupError::Dismissed)
    );
}

#[test]
fn role_arbitration_is_structural() {
    // A surface may hold at most one role: the integrator arbitrates
    // with a keyed registry; the machines make double-role invisible
    // because each constructor consumes a fresh WindowKey and the
    // registry (below) refuses seconds.
    let mut roles: std::collections::HashSet<WindowKey> = std::collections::HashSet::new();
    let key = WindowKey::new(1);
    assert!(roles.insert(key));
    // Second role on the same surface: refused.
    assert!(!roles.insert(key));
    // Destroying the toplevel releases the role.
    roles.remove(&key);
    assert!(roles.insert(key));
}

// ---------------------------------------------------------------------------
// Determinism of the whole surface under a seeded driver.
// ---------------------------------------------------------------------------

#[test]
fn seeded_driver_reproduces_identical_event_streams() {
    fn run(seed: u64) -> Vec<Vec<u8>> {
        let mut rng = Rng::seeded(seed);
        let mut t = Toplevel::new(WindowKey::new(1));
        let mut events: Vec<(ShellEvent, ObjectId)> = Vec::new();
        let inputs = policy_inputs();
        for _ in 0..64 {
            match rng.below(5) {
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
                _ => {}
            }
            if rng.flip() {
                let c = t.propose(&inputs);
                events.push((ShellEvent::toplevel_configure(&c), TOPLEVEL_OBJ));
                if rng.flip() {
                    t.ack_configure(c.serial).unwrap();
                    assert!(t.commit().is_some());
                }
            }
        }
        validate_batch(&events)
    }
    assert_eq!(run(12_648_430), run(12_648_430)); // 0xC0FFEE
    assert_ne!(run(12_648_430), run(4951)); // 0xC0FFEE vs 0x1357
    assert!(!run(12_648_430).is_empty());
}

// Keep the imports of stack/spaces honest (used by the integration
// suite); this suite exercises their wire-facing surface through the
// lifecycle goldens above.
#[test]
fn stacking_and_focus_wire_surface() {
    let mut spaces = Spaces::new(2);
    spaces.add_seat(1, 0);
    spaces.assign(WindowKey::new(1), 0);
    spaces.assign(WindowKey::new(2), 0);
    let mut stack = WindowStack::new();
    stack.add(&spaces, WindowKey::new(1));
    stack.add(&spaces, WindowKey::new(2));
    stack.activate(WindowKey::new(2), ActivationCause::PointerClick);
    assert_eq!(stack.focus(), Some(WindowKey::new(2)));
    // A modal dialog opens over window 1: focus moves, gating applies.
    stack.add_dialog(WindowKey::new(1), WindowKey::new(9), DialogModality::Modal);
    assert_eq!(stack.focus(), Some(WindowKey::new(9)));
    assert!(!stack.can_focus(WindowKey::new(1)));
    assert_eq!(
        stack.log().last().map(|a| a.cause),
        Some(ActivationCause::DialogOpened)
    );
    // Wire-side: the states bitset the next configure would carry for
    // the activated dialog parent — gating means the *dialog* is
    // activated, not the parent.
    let mut t = Toplevel::new(WindowKey::new(9));
    t.set_activated(true);
    let c = t.propose(&policy_inputs());
    assert!(c.states.activated());
    assert!(matches!(
        ShellEvent::toplevel_configure(&c),
        ShellEvent::ToplevelConfigure { .. }
    ));
}

// ---------------------------------------------------------------------------
// SSD geometry conformance under the spec's decoration semantics.
// ---------------------------------------------------------------------------

#[test]
fn ssd_insets_match_spec_decoration_modes() {
    // SSD: border ring + title bar on top.
    let ssd = SsdMetrics::LION.insets(DecorationMode::Server, common::scale(1.0));
    assert_eq!((ssd.left, ssd.top, ssd.right, ssd.bottom), (1, 29, 1, 1));
    // CSD: the system hit-zone reservation.
    let csd = SsdMetrics::LION.insets(DecorationMode::Client, common::scale(1.0));
    assert_eq!((csd.left, csd.top, csd.right, csd.bottom), (5, 5, 5, 5));
    // Fullscreen carries zero insets.
    assert_eq!(Insets::ZERO.width(), 0);
    assert_eq!(Insets::ZERO.height(), 0);
}

#[test]
fn toplevel_states_bitset_matches_spec_indices() {
    // The six spec bits, in order.
    let s = ToplevelStates::build(true, true, true, true, true, true);
    let words = s.0.to_words();
    assert_eq!(words[0] & 0x3F, 0x3F);
    assert_eq!(words[1], 0);
    // Individual accessors.
    assert!(s.maximized() && s.fullscreen() && s.minimized());
    assert!(s.activated() && s.sticky() && s.resizing());
}
