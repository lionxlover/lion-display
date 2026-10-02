//! EC: an xclock-class client — the oclock shape — runs through the
//! bridge: a round face drawn with arcs and pie fills, hands as wide
//! lines, a resize that re-exposes, and a repaint cycle.

mod common;

use common::*;

/// A clock window at the center of the screen.
fn clock_window(wid: u32) -> Vec<u8> {
    create_window(
        wid,
        0x40,
        312,
        232,
        400,
        300,
        (1 << 1) | (1 << 8), // back-pixel + event-mask
        &[
            0x0033_3300, // dark face
            0x0000_8080, // Exposure | VisibilityChange | StructureNotify
        ],
    )
}

#[test]
fn oclock_draws_resizes_and_repaints() {
    let (mut server, client, _) = connect(screen());

    // Create + map the clock with exposure interest.
    let mut cx = Vec::new();
    cx.extend_from_slice(&clock_window(0x41));
    // A GC pair: face fill and hand strokes (width 3).
    cx.extend_from_slice(&create_gc(
        0x50,
        0x41,
        (1 << 2) | (1 << 4),
        &[0x00ff_ff00, 3],
    ));
    cx.extend_from_slice(&map_window(0x41));
    server.feed(client, &cx).unwrap();
    let out = server.take_output(client);
    assert!(
        envelopes(&out).iter().any(|e| e[0] == 12),
        "the initial Expose arrives"
    );

    // The face: a full-circle pie fill.
    let mut cx = Vec::new();
    cx.extend_from_slice(&poly_fill_arc(0x41, 0x50, &[(0, 0, 400, 300, 0, 360 * 64)]));
    // The rim: the outline on top.
    cx.extend_from_slice(&poly_arc(0x41, 0x50, &[(0, 0, 400, 300, 0, 360 * 64)]));
    // The hands: 12 o'clock and 3 o'clock from the center (200, 150).
    cx.extend_from_slice(&poly_line(0x41, 0x50, &[(200, 150), (200, 40)]));
    cx.extend_from_slice(&poly_line(0x41, 0x50, &[(200, 150), (330, 150)]));
    server.feed(client, &cx).unwrap();
    let _ = server.take_output(client);

    let store = server.root_store().expect("root store");
    let (ox, oy) = (312i32, 232i32);
    // The center pixel lies inside the filled face (yellow).
    assert_eq!(store.pixel(ox + 200, oy + 150), Some(0x00ff_ff00));
    // The vertical hand is drawn (3-wide stroke around x=200).
    assert_eq!(
        store.pixel(ox + 200, oy + 80),
        Some(0x00ff_ff00),
        "hand pixels"
    );
    // The horizontal hand too.
    assert_eq!(store.pixel(ox + 280, oy + 150), Some(0x00ff_ff00));
    // Outside the ellipse (corner of the bounding box) shows the
    // clock window's own background.
    assert_eq!(store.pixel(ox + 3, oy + 3), Some(0x0033_3300));

    // ---- resize: the clock grows; the client is re-exposed ----
    let mut cx = Vec::new();
    cx.extend_from_slice(&configure_window(0x41, 0x4 | 0x8, &[500, 400]));
    server.feed(client, &cx).unwrap();
    assert!(server.take_damage().is_some(), "resize dirties the root");
    let out = server.take_output(client);
    let exposes: Vec<&[u8]> = envelopes(&out).into_iter().filter(|e| e[0] == 12).collect();
    assert!(!exposes.is_empty(), "repaint exposures arrive");
    // The new geometry is visible to a GetGeometry query.
    let mut cx = Vec::new();
    cx.extend_from_slice(&request(14, 0, &0x41u32.to_le_bytes())); // GetGeometry
    server.feed(client, &cx).unwrap();
    let out = server.take_output(client);
    let reply = envelopes(&out)[0];
    assert_eq!(reply[0], 1);
    assert_eq!(u16::from_le_bytes(reply[16..18].try_into().unwrap()), 500);
    assert_eq!(u16::from_le_bytes(reply[18..20].try_into().unwrap()), 400);

    // ---- the repaint: the face again at the new size ----
    let mut cx = Vec::new();
    cx.extend_from_slice(&poly_fill_arc(0x41, 0x50, &[(0, 0, 500, 400, 0, 360 * 64)]));
    server.feed(client, &cx).unwrap();
    let _ = server.take_output(client);
    let store = server.root_store().expect("root store");
    assert_eq!(
        store.pixel(ox + 250, oy + 200),
        Some(0x00ff_ff00),
        "new center"
    );

    // ---- the pie-slice semantics: a quarter wedge ----
    let mut cx = Vec::new();
    // Erase, then a wedge from 45 degrees spanning 90 (pie mode).
    cx.extend_from_slice(&create_gc(0x51, 0x41, 1 << 2, &[0x0000_ff00]));
    cx.extend_from_slice(&poly_fill_arc(
        0x41,
        0x51,
        &[(0, 0, 500, 400, 45 * 64, 90 * 64)],
    ));
    server.feed(client, &cx).unwrap();
    let _ = server.take_output(client);
    let store = server.root_store().expect("root store");
    // The wedge spans 45..=135 degrees from the center: the 60-degree
    // direction (down-right) is inside it.
    assert_eq!(
        store.pixel(ox + 350, oy + 338),
        Some(0x0000_ff00),
        "wedge body"
    );
    // The opposite direction is untouched face.
    assert_eq!(
        store.pixel(ox + 150, oy + 120),
        Some(0x00ff_ff00),
        "outside the wedge"
    );
}

#[test]
fn dashed_second_hand_strokes_correctly() {
    let (mut server, client, _) = connect(screen());
    let mut cx = Vec::new();
    cx.extend_from_slice(&clock_window(0x41));
    // A dashed GC: line-style OnOffDash + a dash pattern.
    cx.extend_from_slice(&create_gc(
        0x50,
        0x41,
        (1 << 2) | (1 << 5) | (1 << 20),
        &[0x00ff_ffff, 1, 2], // foreground, line-style OnOffDash, dash-offset
    ));
    cx.extend_from_slice(&set_dashes(0x50, 0, &[3, 4]));
    cx.extend_from_slice(&map_window(0x41));
    // A "second hand": dashed line across the face.
    cx.extend_from_slice(&poly_line(0x41, 0x50, &[(10, 150), (390, 150)]));
    server.feed(client, &cx).unwrap();
    let _ = server.take_output(client);
    let store = server.root_store().expect("root store");
    let (ox, oy) = (312i32, 232i32);
    // The dash pattern [3, 4]: the first 3 pixels drawn, the next 4
    // show the window's background, then on again.
    let bg = 0x0033_3300;
    assert_eq!(store.pixel(ox + 10, oy + 150), Some(0x00ff_ffff), "dash on");
    assert_eq!(store.pixel(ox + 12, oy + 150), Some(0x00ff_ffff), "dash on");
    assert_eq!(store.pixel(ox + 13, oy + 150), Some(bg), "dash off");
    assert_eq!(store.pixel(ox + 16, oy + 150), Some(bg), "dash off");
    assert_eq!(
        store.pixel(ox + 17, oy + 150),
        Some(0x00ff_ffff),
        "dash on again"
    );
}
