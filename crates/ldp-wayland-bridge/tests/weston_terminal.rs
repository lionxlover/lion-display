//! EC: a weston-terminal-class client maps, types, and resizes through
//! the Wayland bridge, byte-for-byte.
//!
//! The real terminal connects, binds the globals, creates a surface +
//! xdg_toplevel, acks the initial configure, attaches an shm buffer,
//! and commits — then receives keyboard input, redraws on keypresses,
//! survives a resize (configure → ack → new buffer), and exits on
//! close. This suite scripts exactly that lifecycle.

mod common;

use common::*;
use ldp_wayland_bridge::protocol;
use ldp_wayland_bridge::wire::Value;

/// The full terminal lifecycle: connect → map → type → resize →
/// close — the Phase 17 exit criterion as one scripted session.
#[allow(clippy::too_many_lines)]
#[test]
fn weston_terminal_maps_types_and_resizes() {
    let mut s = Script::new();

    // ---- connect: the registry replay ----
    let globals = s.boot();
    assert_eq!(
        globals,
        vec![
            (1, "wl_compositor".into(), 1),
            (2, "wl_shm".into(), 1),
            (3, "wl_seat".into(), 5),
            (4, "xdg_wm_base".into(), 1),
        ],
        "the four globals, in order"
    );

    // The four binds (ids 3..6), capturing the seat's capabilities.
    s.bind(1, 3);
    s.bind(2, 4);
    s.bind(3, 5);
    s.bind(4, 6);
    // (Capabilities and the shm formats rode those binds' output; the
    // suites below pin them. Here: bind the seat devices.)
    let get_pointer = request(&protocol::WL_SEAT, 5, 0, vec![Value::NewId(7)]);
    let get_keyboard = request(&protocol::WL_SEAT, 5, 1, vec![Value::NewId(8)]);
    let mut batch = get_pointer;
    batch.extend_from_slice(&get_keyboard);
    s.client.feed(&batch).unwrap();
    let _ = s.client.take_output();

    // ---- map: surface + xdg + toplevel + the configure dance ----
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
    let out = s.client.take_output().0;
    // The initial configure: xdg_toplevel.configure(0, 0, []) then
    // xdg_surface.configure(serial 1).
    let (obj, op) = header_at(&out, 0);
    assert_eq!((obj, op), (11, 0), "toplevel configure");
    let (tconf, used) = event_at(&out, 0, &protocol::XDG_TOPLEVEL, 11, 0);
    assert_eq!(tconf.args[0], Value::Int(0));
    assert_eq!(tconf.args[1], Value::Int(0));
    assert_eq!(tconf.args[2], Value::Array(Vec::new().into()));
    let (obj, op) = header_at(&out, used);
    assert_eq!((obj, op), (10, 0), "xdg_surface configure");
    let (xconf, used2) = event_at(&out, used, &protocol::XDG_SURFACE, 10, 0);
    let Value::Uint(serial) = xconf.args[0] else {
        panic!("configure serial");
    };
    assert_eq!(used + used2, out.len());

    // The title.
    let set_title = request(
        &protocol::XDG_TOPLEVEL,
        11,
        2,
        vec![Value::String("terminal".into())],
    );
    s.client.feed(&set_title).unwrap();
    let _ = s.client.take_output();

    // The pool + buffer: 80x24 cells worth of pixels.
    s.create_pool(12, 80 * 24 * 4);
    s.create_buffer(13, 12, 0, 80, 24, 320, 1);

    // Ack, then the first commit maps the surface.
    let ack = request(&protocol::XDG_SURFACE, 10, 4, vec![Value::Uint(serial)]);
    s.client.feed(&ack).unwrap();
    let _ = s.client.take_output();
    s.fill(12, 0, 80, 24, [0x33, 0x22, 0x11, 0xff]);
    s.commit(9, 13, &[(0, 0, 80, 24)]);
    assert!(
        s.client.surface(9).is_some_and(|surf| surf.mapped),
        "the surface mapped"
    );

    // ---- type: the keymap + key events ----
    // The driver sends the keymap on the keyboard object...
    s.client.send_keymap(8);
    let (out, fds) = s.client.take_output();
    assert_eq!(fds, 1, "the keymap rides one ancillary fd");
    let (obj, op) = header_at(&out, 0);
    assert_eq!((obj, op), (8, 0), "wl_keyboard.keymap");
    let (km, _used) = event_at(&out, 0, &protocol::WL_KEYBOARD, 8, 0);
    assert_eq!(km.args[1], Value::Uint(1), "xkb v1 format");
    let Value::Uint(size) = km.args[2] else {
        panic!("keymap size");
    };
    assert_eq!(
        size as usize,
        b"xkb_keymap { };\0".len() + 1,
        "size counts the NUL"
    );
    // ...then focus + keys (serials advance one per event).
    s.client.keyboard_focus(8, 9, &[]);
    s.client.key(8, 30, true); // evdev KEY_A
    s.client.key(8, 30, false);
    let (out, _) = s.client.take_output();
    // enter (serial s1), key (s2), key (s3) — each with a fresh serial.
    let mut at = 0usize;
    let (enter, u) = event_at(&out, at, &protocol::WL_KEYBOARD, 8, 1);
    let Value::Uint(s1) = enter.args[0] else {
        panic!()
    };
    at += u;
    let (k1, u) = event_at(&out, at, &protocol::WL_KEYBOARD, 8, 3);
    let Value::Uint(s2) = k1.args[0] else {
        panic!()
    };
    assert_eq!(k1.args[2], Value::Uint(30), "the keycode");
    assert_eq!(k1.args[3], Value::Uint(1), "pressed");
    at += u;
    let (k2, u) = event_at(&out, at, &protocol::WL_KEYBOARD, 8, 3);
    let Value::Uint(s3) = k2.args[0] else {
        panic!()
    };
    assert_eq!(k2.args[3], Value::Uint(0), "released");
    assert_eq!(at + u, out.len());
    assert!(s1 < s2 && s2 < s3, "serials strictly advance");

    // ---- resize: configure → ack → new buffer → commit ----
    let serial = s.client.toplevel_configure(11, 100, 30);
    let (out, _) = s.client.take_output();
    let (obj, op) = header_at(&out, 0);
    assert_eq!((obj, op), (11, 0), "toplevel configure carries the size");
    let (tconf, used) = event_at(&out, 0, &protocol::XDG_TOPLEVEL, 11, 0);
    assert_eq!(tconf.args[0], Value::Int(100));
    assert_eq!(tconf.args[1], Value::Int(30));
    let (xconf, used2) = event_at(&out, used, &protocol::XDG_SURFACE, 10, 0);
    assert_eq!(xconf.args[0], Value::Uint(serial));
    assert_eq!(used + used2, out.len());
    // The client resizes its pool, re-acks, and re-commits.
    let resize = request(
        &protocol::WL_SHM_POOL,
        12,
        2,
        vec![Value::Int(100 * 30 * 4)],
    );
    s.client.feed(&resize).unwrap();
    let _ = s.client.take_output();
    let ack = request(&protocol::XDG_SURFACE, 10, 4, vec![Value::Uint(serial)]);
    s.client.feed(&ack).unwrap();
    let _ = s.client.take_output();
    s.create_buffer(14, 12, 0, 100, 30, 400, 1);
    s.commit(9, 14, &[(0, 0, 100, 30)]);
    assert!(s.client.surface(9).is_some_and(|surf| surf.mapped));

    // ---- close: the xdg_toplevel.close event ----
    s.client.toplevel_close(11);
    let (out, _) = s.client.take_output();
    let (obj, op) = header_at(&out, 0);
    assert_eq!((obj, op), (11, 1), "xdg_toplevel.close");
    assert_eq!(out.len(), 4);
}

#[test]
fn commit_before_ack_is_the_unconfigured_buffer_error() {
    let mut s = Script::new();
    s.connect();
    s.create_surface(9);
    let get_xdg = request(
        &protocol::XDG_WM_BASE,
        6,
        2,
        vec![Value::NewId(10), Value::Object(9)],
    );
    let get_toplevel = request(&protocol::XDG_SURFACE, 10, 1, vec![Value::NewId(11)]);
    let mut batch = get_xdg;
    batch.extend_from_slice(&get_toplevel);
    s.client.feed(&batch).unwrap();
    let _ = s.client.take_output();
    s.create_pool(12, 64);
    s.create_buffer(13, 12, 0, 4, 4, 16, 1);
    // Commit without the ack: fatal, with the error event.
    assert!(s.client.commit_with_buffer(9, 13).is_err());
    let (out, _) = s.client.take_output();
    let (obj, op) = header_at(&out, 0);
    assert_eq!((obj, op), (1, 0), "wl_display.error");
    let (err, used) = event_at(&out, 0, &protocol::WL_DISPLAY, 1, 0);
    let Value::Uint(code) = err.args[1] else {
        panic!()
    };
    assert_eq!(code, 1, "invalid_argument");
    assert_eq!(used, out.len());
}

#[test]
fn role_assignment_is_one_shot() {
    let mut s = Script::new();
    s.connect();
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
    // A second role on the same xdg_surface: fatal.
    let again = request(&protocol::XDG_SURFACE, 10, 1, vec![Value::NewId(20)]);
    assert!(s.client.feed(&again).is_err(), "the role error is fatal");
    let (out, _) = s.client.take_output();
    let (obj, op) = header_at(&out, 0);
    assert_eq!((obj, op), (1, 0), "wl_display.error");
}
