//! EC: the seat serial discipline and the input event stream —
//! serials strictly advance, one per generated event, and the
//! pointer/keyboard semantics hold.

mod common;

use common::*;
use ldp_wayland_bridge::protocol;
use ldp_wayland_bridge::wire::Value;

/// Wire up a mapped surface with pointer + keyboard devices.
fn setup() -> Script {
    let mut s = Script::new();
    s.connect();
    let get_pointer = request(&protocol::WL_SEAT, 5, 0, vec![Value::NewId(7)]);
    let get_keyboard = request(&protocol::WL_SEAT, 5, 1, vec![Value::NewId(8)]);
    let mut batch = get_pointer;
    batch.extend_from_slice(&get_keyboard);
    s.client.feed(&batch).unwrap();
    let _ = s.client.take_output();
    s.create_surface(9);
    let get_xdg = request(
        &protocol::XDG_WM_BASE,
        6,
        2,
        vec![Value::NewId(10), Value::Object(9)],
    );
    let get_toplevel = request(&protocol::XDG_SURFACE, 10, 1, vec![Value::NewId(11)]);
    let mut all = get_xdg;
    all.extend_from_slice(&get_toplevel);
    s.client.feed(&all).unwrap();
    let _ = s.client.take_output();
    s.create_pool(12, 64);
    s.create_buffer(13, 12, 0, 4, 4, 16, 1);
    // Ack the initial configure (serial 1) before the first commit.
    let ack = request(&protocol::XDG_SURFACE, 10, 4, vec![Value::Uint(1)]);
    s.client.feed(&ack).unwrap();
    let _ = s.client.take_output();
    s.client.commit_with_buffer(9, 13).unwrap();
    let _ = s.client.take_output();
    s
}

#[test]
fn serials_advance_one_per_event() {
    let mut s = setup();
    s.client.pointer_focus(7, 9, 10, 20);
    s.client.pointer_motion(7, 11, 21);
    s.client.pointer_button(7, 1, true);
    s.client.pointer_button(7, 1, false);
    let (out, _) = s.client.take_output();
    // enter + frame + motion + frame + button + frame + button + frame.
    let serials: Vec<u32> = collect_serials(&out);
    assert_eq!(serials.len(), 3, "enter, button, button carry serials");
    for pair in serials.windows(2) {
        assert!(pair[0] < pair[1], "strictly advancing: {serials:?}");
    }
    assert_eq!(s.client.serial(), serials[2], "the counter is public");
}

fn collect_serials(out: &[u8]) -> Vec<u32> {
    let mut serials = Vec::new();
    let mut at = 0usize;
    while at + 4 <= out.len() {
        let (obj, op) = header_at(out, at);
        // Pointer events: enter(0)/leave(1)/button(3) carry serials;
        // motion(2)/axis(4) carry time; frame(5) is empty.
        if obj == 7 && matches!(op, 0 | 1 | 3) {
            let (msg, used) = event_at(out, at, &protocol::WL_POINTER, 7, op);
            if let Value::Uint(s) = msg.args[0] {
                serials.push(s);
            }
            at += used;
            continue;
        }
        if obj == 7 && matches!(op, 2 | 4) {
            let (_, used) = event_at(out, at, &protocol::WL_POINTER, 7, op);
            at += used;
            continue;
        }
        if obj == 7 && op == 5 {
            at += 4;
            continue;
        }
        at += 4;
    }
    serials
}

#[test]
fn pointer_enter_carries_fixed_coordinates() {
    let mut s = setup();
    s.client.pointer_focus(7, 9, 10, 20);
    let (out, _) = s.client.take_output();
    // enter(serial, surface, 10<<8, 20<<8) then frame.
    let (enter, used) = event_at(&out, 0, &protocol::WL_POINTER, 7, 0);
    assert_eq!(enter.args[1], Value::Object(9));
    assert_eq!(enter.args[2], Value::Fixed(10 << 8));
    assert_eq!(enter.args[3], Value::Fixed(20 << 8));
    assert_eq!(
        &out[used..used + 4],
        &((7u32 << 8) | 5).to_le_bytes(),
        "frame"
    );
    assert_eq!(used + 4, out.len());
}

#[test]
fn keyboard_focus_transitions() {
    let mut s = setup();
    s.client.keyboard_focus(8, 9, &[30]);
    let (out, _) = s.client.take_output();
    // enter(serial, surface, keys=[30]) — the keys ride as u32 words.
    let (enter, _used) = event_at(&out, 0, &protocol::WL_KEYBOARD, 8, 1);
    assert_eq!(enter.args[1], Value::Object(9));
    match &enter.args[2] {
        Value::Array(a) => assert_eq!(a.as_ref(), &30u32.to_le_bytes()[..]),
        other => panic!("keys array: {other:?}"),
    }
    // Leave: serial + surface.
    s.client.keyboard_focus(8, 0, &[]);
    let (out, _) = s.client.take_output();
    let (leave, used) = event_at(&out, 0, &protocol::WL_KEYBOARD, 8, 2);
    // The leave names the surface being left.
    assert_eq!(leave.args[1], Value::Object(9));
    assert_eq!(used, out.len());
}

#[test]
fn key_events_carry_the_serial_and_state() {
    let mut s = setup();
    s.client.key(8, 46, true);
    s.client.key(8, 46, false);
    let (out, _) = s.client.take_output();
    let (k1, used) = event_at(&out, 0, &protocol::WL_KEYBOARD, 8, 3);
    assert_eq!(k1.args[2], Value::Uint(46), "evdev keycode unchanged");
    assert_eq!(k1.args[3], Value::Uint(1));
    let (k2, used2) = event_at(&out, used, &protocol::WL_KEYBOARD, 8, 3);
    assert_eq!(k2.args[3], Value::Uint(0));
    assert_eq!(used + used2, out.len());
    let Value::Uint(s1) = k1.args[0] else {
        panic!()
    };
    let Value::Uint(s2) = k2.args[0] else {
        panic!()
    };
    assert!(s1 < s2);
}

#[test]
fn modifiers_arrive_as_five_words() {
    let mut s = setup();
    s.client.modifiers(8, 1, 0, 0, 0);
    let (out, _) = s.client.take_output();
    let (m, used) = event_at(&out, 0, &protocol::WL_KEYBOARD, 8, 4);
    assert_eq!(m.args[1], Value::Uint(1), "depressed shift");
    assert_eq!(used, out.len());
    let _ = used;
}

#[test]
fn keymap_is_send_once() {
    let mut s = setup();
    s.client.send_keymap(8);
    let (out, fds) = s.client.take_output();
    assert_eq!(fds, 1);
    assert!(!out.is_empty());
    // A second delivery is a no-op.
    s.client.send_keymap(8);
    let (out2, fds2) = s.client.take_output();
    assert!(out2.is_empty());
    assert_eq!(fds2, 0);
    assert!(s.client.keymap_delivered(8));
}
