//! EC: an xeyes-class client runs end to end through the X bridge,
//! byte-for-byte, Xvfb-free.
//!
//! The real xeyes creates two eye windows, selects exposures, sets
//! `WM_PROTOCOLS`/`WM_DELETE_WINDOW`, maps, draws the eye outlines
//! with arcs and fills, tracks the pointer with points/lines for the
//! pupils, repaints on Expose, and exits on the `WM_DELETE_WINDOW`
//! ClientMessage. This suite scripts exactly that lifecycle against
//! one [`Server`] and asserts the pixels that land in the root store,
//! the event stream the client sees, and the close handshake.

mod common;

use common::*;
use ldp_x11_bridge::dispatch::ClosePolicy;
use ldp_x11_bridge::events::mask;

const EYE_W: u16 = 100;
const EYE_H: u16 = 100;

/// One eye window with exposure + structure masks and a background.
fn eye_window(wid: u32, x: i16) -> Vec<u8> {
    create_window(
        wid,
        0x40,
        x,
        20,
        EYE_W,
        EYE_H,
        (1 << 1) | (1 << 8) | (1 << 10), // back-pixel, event-mask, override-redirect
        &[
            0x00ff_ff00, // yellow background
            mask::EXPOSURE | mask::STRUCTURE_NOTIFY | mask::KEY_PRESS,
            1, // override-redirect: true
        ],
    )
}

/// The full xeyes lifecycle — the Phase 17 exit criterion, as one
/// flat scripted session.
#[allow(clippy::too_many_lines)]
#[test]
fn xeyes_lifecycle_runs_to_completion() {
    let (mut server, client, setup) = connect(screen());
    // The setup reply advertises the one screen at 1024x768.
    assert_eq!(setup[0], 1);
    assert_eq!(u16::from_le_bytes([setup[84], setup[85]]), 1024);

    // ---- the client boots: atoms, GCs, windows ----
    let mut cx = Vec::new();
    cx.extend_from_slice(&intern_atom("WM_PROTOCOLS", false));
    cx.extend_from_slice(&intern_atom("WM_DELETE_WINDOW", false));
    cx.extend_from_slice(&eye_window(0x41, 80));
    cx.extend_from_slice(&eye_window(0x42, 200));
    cx.extend_from_slice(&create_gc(
        0x50,
        0x41,
        (1 << 2) | (1 << 3),
        &[0x0000_0000, 0x00ff_ffff],
    ));
    cx.extend_from_slice(&map_window(0x41));
    cx.extend_from_slice(&map_window(0x42));
    server.feed(client, &cx).unwrap();
    let out = server.take_output(client);
    let envs = envelopes(&out);
    // Two atom replies + two Expose events (one per mapped eye) +
    // two MapNotify events (StructureNotify selected).
    assert_eq!(envs.len(), 6, "{out:02x?}");
    assert_eq!(envs[0][0], 1); // reply
    assert_eq!(u32::from_le_bytes(envs[0][8..12].try_into().unwrap()), 69);
    assert_eq!(u32::from_le_bytes(envs[1][8..12].try_into().unwrap()), 70);
    let expose_codes: Vec<u8> = envs[2..].iter().map(|e| e[0]).collect();
    assert!(
        expose_codes.contains(&12),
        "exposes present: {expose_codes:?}"
    );
    assert!(
        expose_codes.contains(&19),
        "MapNotify present: {expose_codes:?}"
    );

    // ---- the WM protocols property on both windows ----
    let mut cx = Vec::new();
    cx.extend_from_slice(&change_property_32(0x41, 69, 4, &[70]));
    cx.extend_from_slice(&change_property_32(0x42, 69, 4, &[70]));
    server.feed(client, &cx).unwrap();
    let _ = server.take_output(client);

    // ---- the eyes draw: outline arcs + filled pupils ----
    let mut cx = Vec::new();
    // Outline: a full-circle arc on each eye.
    cx.extend_from_slice(&poly_arc(0x41, 0x50, &[(0, 0, EYE_W, EYE_H, 0, 360 * 64)]));
    cx.extend_from_slice(&poly_arc(0x42, 0x50, &[(0, 0, EYE_W, EYE_H, 0, 360 * 64)]));
    // Pupils: filled circles in the center, tracked with the pointer.
    cx.extend_from_slice(&poly_fill_arc(0x41, 0x50, &[(40, 40, 20, 20, 0, 360 * 64)]));
    cx.extend_from_slice(&poly_fill_arc(0x42, 0x50, &[(40, 40, 20, 20, 0, 360 * 64)]));
    server.feed(client, &cx).unwrap();
    let _ = server.take_output(client);

    // Pixels: the eye interiors are yellow, the pupils black, the arc
    // boundary is drawn, and the outside is the root background.
    let store = server.root_store().expect("root store after drawing");
    let (ox1, oy) = (80i32, 20i32);
    assert_eq!(
        store.pixel(ox1 + 50, oy + 10),
        Some(0x00ff_ff00),
        "inside eye 1"
    );
    assert_eq!(store.pixel(ox1 + 50, oy + 50), Some(0), "the pupil");
    assert_eq!(store.pixel(5, 5), Some(0), "outside the eyes");
    // The arc's topmost pixel row is drawn (black outline).
    assert_eq!(
        store.pixel(ox1 + 50, oy),
        Some(0x0000_0000),
        "the outline top"
    );

    // ---- the pointer moves: pupils follow (erase + redraw) ----
    server.pointer_move(150, 70, 1000);
    // The client erases the old pupil with the background color and
    // draws the smaller tracked one.
    let mut cx = Vec::new();
    cx.extend_from_slice(&create_gc(0x51, 0x41, 1 << 2, &[0x00ff_ff00]));
    cx.extend_from_slice(&fill_rects(0x41, 0x51, &[(40, 40, 20, 20)]));
    cx.extend_from_slice(&fill_rects(0x41, 0x50, &[(45, 45, 10, 10)]));
    server.feed(client, &cx).unwrap();
    let _ = server.take_output(client);
    let store = server.root_store().expect("root store");
    assert_eq!(store.pixel(ox1 + 50, oy + 50), Some(0), "the tracked pupil");
    assert_eq!(
        store.pixel(ox1 + 42, oy + 42),
        Some(0x00ff_ff00),
        "old pupil cleared"
    );

    // ---- exposure: a resize forces a repaint ----
    let dirty = {
        let mut cx = Vec::new();
        cx.extend_from_slice(&configure_window(0x41, 0x4 | 0x8, &[120, 120])); // w, h
        server.feed(client, &cx).unwrap();
        server.take_damage()
    };
    assert!(dirty.is_some(), "resize dirties the root");
    let out = server.take_output(client);
    let exposes: Vec<&[u8]> = envelopes(&out).into_iter().filter(|e| e[0] == 12).collect();
    assert!(
        !exposes.is_empty(),
        "the client gets Expose events to repaint"
    );

    // ---- close: WM_DELETE_WINDOW through the ICCCM ----
    assert_eq!(
        server.request_close(0x41, 2000),
        ClosePolicy::ProtocolMessage
    );
    let out = server.take_output(client);
    let envs = envelopes(&out);
    assert_eq!(envs.len(), 1);
    let ev = envs[0];
    assert_eq!(ev[0], 32); // ClientMessage
    assert_eq!(ev[1], 32); // format 32
    assert_eq!(u32::from_le_bytes(ev[4..8].try_into().unwrap()), 0x41);
    assert_eq!(u32::from_le_bytes(ev[8..12].try_into().unwrap()), 69);
    assert_eq!(u32::from_le_bytes(ev[12..16].try_into().unwrap()), 70);
    assert_eq!(u32::from_le_bytes(ev[16..20].try_into().unwrap()), 2000);

    // The client honors it: destroys both windows.
    let mut cx = Vec::new();
    cx.extend_from_slice(&destroy_window(0x41));
    cx.extend_from_slice(&destroy_window(0x42));
    server.feed(client, &cx).unwrap();
    let out = server.take_output(client);
    let destroy_notifies: Vec<&[u8]> = envelopes(&out).into_iter().filter(|e| e[0] == 17).collect();
    assert_eq!(destroy_notifies.len(), 2, "DestroyNotify per window");
    assert!(server.top_level_windows().is_empty());
}

#[test]
fn xeyes_rejects_drawing_without_a_gc() {
    let (mut server, client, _) = connect(screen());
    let mut cx = Vec::new();
    cx.extend_from_slice(&eye_window(0x41, 10));
    cx.extend_from_slice(&map_window(0x41));
    // Fill with an unknown GC id.
    cx.extend_from_slice(&fill_rects(0x41, 0x99, &[(0, 0, 5, 5)]));
    server.feed(client, &cx).unwrap();
    let out = server.take_output(client);
    let errors: Vec<&[u8]> = envelopes(&out).into_iter().filter(|e| e[0] == 0).collect();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0][1], 13, "BadGC");
    assert_eq!(
        u32::from_le_bytes(errors[0][4..8].try_into().unwrap()),
        0x99
    );
}
