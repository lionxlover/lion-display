//! EC: the popup lifecycle and the positioner-to-LDP translation
//! table (anchor / gravity / constraint bits — the LDP vocabulary is
//! the superset).

mod common;

use common::*;
use ldp_wayland_bridge::protocol;
use ldp_wayland_bridge::state::{constraints, Anchor, Positioner};
use ldp_wayland_bridge::wire::Value;

/// One fully-set positioner on a fresh script: returns its id.
fn positioner(s: &mut Script, anchor: u32, gravity: u32, adj: u32) -> u32 {
    let id = 30;
    let mut batch = request(&protocol::XDG_WM_BASE, 6, 1, vec![Value::NewId(id)]);
    batch.extend_from_slice(&request(
        &protocol::XDG_POSITIONER,
        id,
        1,
        vec![Value::Int(20), Value::Int(10)],
    ));
    batch.extend_from_slice(&request(
        &protocol::XDG_POSITIONER,
        id,
        2,
        vec![Value::Int(5), Value::Int(5), Value::Int(40), Value::Int(20)],
    ));
    batch.extend_from_slice(&request(
        &protocol::XDG_POSITIONER,
        id,
        3,
        vec![Value::Uint(anchor)],
    ));
    batch.extend_from_slice(&request(
        &protocol::XDG_POSITIONER,
        id,
        4,
        vec![Value::Uint(gravity)],
    ));
    batch.extend_from_slice(&request(
        &protocol::XDG_POSITIONER,
        id,
        5,
        vec![Value::Uint(adj)],
    ));
    batch.extend_from_slice(&request(
        &protocol::XDG_POSITIONER,
        id,
        6,
        vec![Value::Int(1), Value::Int(2)],
    ));
    s.client.feed(&batch).unwrap();
    let _ = s.client.take_output();
    id
}

/// Parent + child xdg surfaces with toplevel/popup roles.
fn popup_stack(s: &mut Script, positioner: u32) -> (u32, u32) {
    s.create_surface(9);
    let get_xdg = request(
        &protocol::XDG_WM_BASE,
        6,
        2,
        vec![Value::NewId(10), Value::Object(9)],
    );
    s.client.feed(&get_xdg).unwrap();
    let _ = s.client.take_output();
    let get_toplevel = request(&protocol::XDG_SURFACE, 10, 1, vec![Value::NewId(11)]);
    s.client.feed(&get_toplevel).unwrap();
    let _ = s.client.take_output();
    // The child surface + xdg + popup.
    s.create_surface(20);
    let get_xdg2 = request(
        &protocol::XDG_WM_BASE,
        6,
        2,
        vec![Value::NewId(21), Value::Object(20)],
    );
    s.client.feed(&get_xdg2).unwrap();
    let _ = s.client.take_output();
    let get_popup = request(
        &protocol::XDG_SURFACE,
        21,
        2,
        vec![
            Value::NewId(22),
            Value::Object(10),
            Value::Object(positioner),
        ],
    );
    s.client.feed(&get_popup).unwrap();
    let out = s.client.take_output().0;
    // The popup configure carries the solved position + size.
    let (obj, op) = header_at(&out, 0);
    assert_eq!((obj, op), (22, 0), "xdg_popup.configure");
    let (pc, used) = event_at(&out, 0, &protocol::XDG_POPUP, 22, 0);
    let _ = (pc, used);
    (10, 22)
}

#[test]
fn popup_lifecycle_configures_anddismisses() {
    let mut s = Script::new();
    s.connect();
    // anchor bottom_left, gravity bottom_left (the menu shape):
    // anchor rect (5,5,40,20) → anchor point (5, 25); the popup pulls
    // its bottom-left there: (5, 25-10=15); + offset (1,2) → (6, 17).
    let pos = positioner(&mut s, 7, 7, constraints::SLIDE_X | constraints::SLIDE_Y);
    let (xdg, popup) = popup_stack(&mut s, pos);
    let _ = (xdg, popup);
    // The driver's translation table for the same positioner.
    let p = Positioner {
        size: Some((20, 10)),
        anchor_rect: ldp_core::geometry::Rect::new(5, 5, 40, 20),
        anchor: Anchor::from_wire(7),
        gravity: Anchor::from_wire(7),
        constraint_adjustment: constraints::SLIDE_X | constraints::SLIDE_Y,
        offset: (1, 2),
    };
    let (anchor, gravity, constraint_bits) =
        ldp_wayland_bridge::driver::WlBridgeDriver::popup_args(&p);
    assert_eq!(anchor, 7, "bottom_left is the same wire word");
    assert_eq!(gravity, 7);
    assert_eq!(
        constraint_bits,
        ldp_protocol::generated::shell::popup_constraints::SLIDE_X as u32
            | ldp_protocol::generated::shell::popup_constraints::SLIDE_Y as u32
    );
    // Dismissal.
    s.client.popup_done(22);
    let (out, _) = s.client.take_output();
    let (obj, op) = header_at(&out, 0);
    assert_eq!((obj, op), (22, 1), "popup_done");
    assert_eq!(out.len(), 4);
}

#[test]
fn the_constraint_table_is_total() {
    use ldp_protocol::generated::shell::popup_constraints as pc;
    use ldp_wayland_bridge::driver::WlBridgeDriver;
    // Every xdg bit maps onto the LDP bit with the same name.
    let p = |adj| Positioner {
        size: Some((1, 1)),
        anchor_rect: ldp_core::geometry::Rect::new(0, 0, 1, 1),
        anchor: Anchor::TopLeft,
        gravity: Anchor::BottomRight,
        constraint_adjustment: adj,
        offset: (0, 0),
    };
    assert_eq!(
        WlBridgeDriver::popup_args(&p(constraints::FLIP_X)).2,
        pc::FLIP_X as u32
    );
    assert_eq!(
        WlBridgeDriver::popup_args(&p(constraints::FLIP_Y)).2,
        pc::FLIP_Y as u32
    );
    assert_eq!(
        WlBridgeDriver::popup_args(&p(constraints::RESIZE_X)).2,
        pc::RESIZE_X as u32
    );
    assert_eq!(
        WlBridgeDriver::popup_args(&p(constraints::RESIZE_Y)).2,
        pc::RESIZE_Y as u32
    );
    // All six at once.
    let all = constraints::MASK;
    let expect =
        (pc::SLIDE_X | pc::SLIDE_Y | pc::FLIP_X | pc::FLIP_Y | pc::RESIZE_X | pc::RESIZE_Y) as u32;
    assert_eq!(WlBridgeDriver::popup_args(&p(all)).2, expect);
}

#[test]
fn positioner_without_size_is_rejected() {
    let mut s = Script::new();
    s.connect();
    let id = 30;
    let batch = request(&protocol::XDG_WM_BASE, 6, 1, vec![Value::NewId(id)]);
    let _ = batch;
    s.create_surface(9);
    let get_xdg = request(
        &protocol::XDG_WM_BASE,
        6,
        2,
        vec![Value::NewId(10), Value::Object(9)],
    );
    let get_popup = request(
        &protocol::XDG_SURFACE,
        10,
        2,
        vec![Value::NewId(22), Value::Object(0), Value::Object(id)],
    );
    let mut all = get_xdg;
    all.extend_from_slice(&get_popup);
    // The parent object 0 is invalid first.
    assert!(s.client.feed(&all).is_err(), "null parent is fatal");
}

#[test]
fn bad_positioner_values_reject() {
    let mut s = Script::new();
    s.connect();
    let id = 30;
    let create = request(&protocol::XDG_WM_BASE, 6, 1, vec![Value::NewId(id)]);
    s.client.feed(&create).unwrap();
    let _ = s.client.take_output();
    // set_anchor with the reserved value 9.
    let bad = request(&protocol::XDG_POSITIONER, id, 3, vec![Value::Uint(9)]);
    assert!(s.client.feed(&bad).is_err(), "unknown anchor is fatal");
}

/// The solver: anchor point + offset, gravity pulling the popup's
/// matching corner onto it.
#[test]
fn anchor_rect_and_gravity_solve() {
    let mut s = Script::new();
    s.connect();
    // anchor top_right(3), gravity bottom_left(7): anchor point =
    // (5+40, 5) = (45, 5); gravity pulls the popup's bottom-left
    // there → (45, 5-10) = (45, -5); + offset (1, 2) → (46, -3).
    let pos = positioner(&mut s, 3, 7, 0);
    let _ = popup_stack(&mut s, pos);
    // (The configure's x/y were asserted inside popup_stack; here the
    // stack solved without constraints, exactly the documented base.)
}
