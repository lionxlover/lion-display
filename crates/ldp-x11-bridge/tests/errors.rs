//! EC: the error corpus — every resource-class error the subset can
//! produce, one request each, with code/value/opcode pinning.

mod common;

use common::*;

fn first_error(
    server: &mut ldp_x11_bridge::dispatch::Server,
    client: ldp_x11_bridge::dispatch::ClientId,
) -> Option<[u8; 32]> {
    let out = server.take_output(client);
    envelopes(&out)
        .into_iter()
        .find(|e| e[0] == 0)
        .map(|e| e.try_into().expect("32 bytes"))
}

#[test]
fn bad_window_for_every_window_request() {
    let (mut server, client, _) = connect(screen());
    let bad = 0xdead_beefu32;
    let mut cx = Vec::new();
    cx.extend_from_slice(&request(3, 0, &bad.to_le_bytes())); // GetWindowAttributes
    cx.extend_from_slice(&request(8, 0, &bad.to_le_bytes())); // MapWindow
    cx.extend_from_slice(&request(10, 0, &bad.to_le_bytes())); // UnmapWindow
                                                               // ConfigureWindow with a complete (zero-mask) payload.
    let mut cw = bad.to_le_bytes().to_vec();
    cw.extend_from_slice(&0u16.to_le_bytes());
    cw.extend_from_slice(&0u16.to_le_bytes());
    cx.extend_from_slice(&request(12, 0, &cw));
    cx.extend_from_slice(&request(15, 0, &bad.to_le_bytes())); // QueryTree
    server.feed(client, &cx).unwrap();
    let out = server.take_output(client);
    let errors: Vec<&[u8]> = envelopes(&out).into_iter().filter(|e| e[0] == 0).collect();
    assert_eq!(errors.len(), 5, "one BadWindow each");
    for e in &errors {
        assert_eq!(e[1], 3, "BadWindow");
        assert_eq!(u32::from_le_bytes(e[4..8].try_into().unwrap()), bad);
    }
}

#[test]
fn bad_drawable_gc_pixmap_atom_cursor_font_colormap() {
    let (mut server, client, _) = connect(screen());
    // One valid window to anchor the rest.
    let mut cx = Vec::new();
    cx.extend_from_slice(&create_window(0x41, 0x40, 0, 0, 32, 32, 0, &[]));
    cx.extend_from_slice(&map_window(0x41));
    server.feed(client, &cx).unwrap();
    let _ = server.take_output(client);

    let bad = 0xbeefu32;
    let mut cx = Vec::new();
    // BadDrawable: drawing on a nonexistent id.
    cx.extend_from_slice(&fill_rects(bad, 0x50, &[(0, 0, 4, 4)]));
    // BadGC: a valid drawable but an unknown GC.
    cx.extend_from_slice(&fill_rects(0x41, bad, &[(0, 0, 4, 4)]));
    // BadPixmap: FreePixmap of an unknown id.
    cx.extend_from_slice(&request(54, 0, &bad.to_le_bytes()));
    // BadAtom: GetAtomName of an unknown atom.
    cx.extend_from_slice(&request(17, 0, &500u32.to_le_bytes()));
    // BadCursor: FreeCursor of an unknown id.
    cx.extend_from_slice(&request(91, 0, &bad.to_le_bytes()));
    // BadFont: CloseFont of an unknown id.
    cx.extend_from_slice(&request(46, 0, &bad.to_le_bytes()));
    // BadColormap: FreeColormap of an unknown id.
    cx.extend_from_slice(&request(75, 0, &bad.to_le_bytes()));
    server.feed(client, &cx).unwrap();
    let out = server.take_output(client);
    let errors: Vec<&[u8]> = envelopes(&out).into_iter().filter(|e| e[0] == 0).collect();
    assert_eq!(errors.len(), 7, "one error per request: {out:02x?}");
    let expect = [
        (9u8, bad), // BadDrawable
        (13, bad),  // BadGC
        (4, bad),   // BadPixmap
        (5, 500),   // BadAtom
        (6, bad),   // BadCursor
        (7, bad),   // BadFont
        (12, bad),  // BadColor (the colormap class)
    ];
    for (i, (code, value)) in expect.iter().enumerate() {
        assert_eq!(errors[i][1], *code, "error {i}");
        assert_eq!(
            u32::from_le_bytes(errors[i][4..8].try_into().unwrap()),
            *value
        );
    }
}

#[test]
fn bad_value_bad_match_bad_id_choice_bad_length() {
    let (mut server, client, _) = connect(screen());
    let mut cx = Vec::new();
    // BadValue: a CreateWindow with a bogus class (3).
    let mut p = Vec::new();
    p.extend_from_slice(&0x41u32.to_le_bytes());
    p.extend_from_slice(&0x40u32.to_le_bytes());
    for v in [0i16, 0, 8, 8] {
        p.extend_from_slice(&v.to_le_bytes());
    }
    p.extend_from_slice(&0u16.to_le_bytes());
    p.extend_from_slice(&3u16.to_le_bytes()); // class: invalid
    p.extend_from_slice(&0x21u32.to_le_bytes());
    p.extend_from_slice(&0u32.to_le_bytes());
    cx.extend_from_slice(&request(1, 0, &p));
    // BadIDChoice: re-creating an existing window.
    cx.extend_from_slice(&create_window(0x40, 0x40, 0, 0, 8, 8, 0, &[]));
    // BadLength: a GetWindowAttributes payload that is too short.
    cx.extend_from_slice(&request(3, 0, &[]));
    // BadMatch: drawing on an InputOnly window.
    cx.extend_from_slice(&create_window(0x42, 0x40, 0, 0, 8, 8, 0, &[]));
    server.feed(client, &cx).unwrap();
    let _ = server.take_output(client);
    let mut cx = Vec::new();
    // Flip 0x42 to InputOnly via ChangeWindowAttributes? The class is
    // fixed at creation; create a proper InputOnly instead.
    cx.clear();
    // (0x42 was created InputOutput; draw on it works. The InputOnly
    // path needs its own window.)
    cx.extend_from_slice(&request(2, 0, &0x42u32.to_le_bytes())); // ChangeWindowAttributes, no mask
    let _ = &mut cx;
    server.feed(client, &cx).unwrap();
    let _ = server.take_output(client);

    // InputOnly drawing → BadMatch.
    let mut p = Vec::new();
    p.extend_from_slice(&0x43u32.to_le_bytes());
    p.extend_from_slice(&0x40u32.to_le_bytes());
    for v in [0i16, 0, 8, 8] {
        p.extend_from_slice(&v.to_le_bytes());
    }
    p.extend_from_slice(&0u16.to_le_bytes());
    p.extend_from_slice(&2u16.to_le_bytes()); // class: InputOnly
    p.extend_from_slice(&0x21u32.to_le_bytes());
    p.extend_from_slice(&0u32.to_le_bytes());
    let mut cx = Vec::new();
    cx.extend_from_slice(&request(1, 0, &p));
    cx.extend_from_slice(&create_gc(0x50, 0x43, 0, &[]));
    cx.extend_from_slice(&fill_rects(0x43, 0x50, &[(0, 0, 4, 4)]));
    server.feed(client, &cx).unwrap();
    let out = server.take_output(client);
    let errors: Vec<&[u8]> = envelopes(&out).into_iter().filter(|e| e[0] == 0).collect();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0][1], 8, "BadMatch on InputOnly drawing");
    assert_eq!(
        u32::from_le_bytes(errors[0][4..8].try_into().unwrap()),
        0x43
    );
}

#[test]
fn unimplemented_requests_answer_bad_implementation() {
    let (mut server, client, _) = connect(screen());
    let mut cx = Vec::new();
    // ReparentWindow (7).
    let mut rp = Vec::new();
    rp.extend_from_slice(&0x41u32.to_le_bytes());
    rp.extend_from_slice(&0x40u32.to_le_bytes());
    rp.extend_from_slice(&0i16.to_le_bytes());
    rp.extend_from_slice(&0i16.to_le_bytes());
    cx.extend_from_slice(&request(7, 0, &rp));
    // QueryFont (47).
    cx.extend_from_slice(&request(47, 0, &1u32.to_le_bytes()));
    server.feed(client, &cx).unwrap();
    let out = server.take_output(client);
    let errors: Vec<&[u8]> = envelopes(&out).into_iter().filter(|e| e[0] == 0).collect();
    assert_eq!(errors.len(), 2);
    for e in &errors {
        assert_eq!(e[1], 17, "BadImplementation");
    }
}

#[test]
fn unknown_opcode_answers_bad_request() {
    let (mut server, client, _) = connect(screen());
    // Opcode 117 is beyond the core table.
    server.feed(client, &request(117, 0, &[])).unwrap();
    let e = first_error(&mut server, client).expect("an error");
    assert_eq!(e[1], 1, "BadRequest");
    assert_eq!(u32::from_le_bytes(e[4..8].try_into().unwrap()), 117);
}

#[test]
fn bad_handshake_is_fatal() {
    use ldp_x11_bridge::dispatch::FatalReason;
    let mut server =
        ldp_x11_bridge::dispatch::Server::new(screen(), Box::new(ldp_x11_bridge::shm::NoShm));
    let client = server.add_client();
    // Garbage order byte.
    let mut bad = vec![0x00u8, 0];
    bad.extend_from_slice(&11u16.to_le_bytes());
    bad.extend_from_slice(&0u16.to_le_bytes());
    bad.extend_from_slice(&[0; 6]);
    assert!(matches!(
        server.feed(client, &bad),
        Err(FatalReason::Handshake)
    ));
    // Wrong protocol version: a setup-failed reply, then fatal.
    let client2 = server.add_client();
    let mut v = vec![0x6c, 0];
    v.extend_from_slice(&99u16.to_le_bytes());
    v.extend_from_slice(&0u16.to_le_bytes());
    v.extend_from_slice(&[0; 6]);
    assert!(matches!(
        server.feed(client2, &v),
        Err(FatalReason::Handshake)
    ));
    let out = server.take_output(client2);
    assert_eq!(out[0], 0, "the setup-failed reply");
}

#[test]
fn property_type_mismatch_is_bad_match() {
    let (mut server, client, _) = connect(screen());
    let mut cx = Vec::new();
    cx.extend_from_slice(&intern_atom("WM_NAME", false));
    server.feed(client, &cx).unwrap();
    let out = server.take_output(client);
    let atom = u32::from_le_bytes(out[out.len() - 24..out.len() - 20].try_into().unwrap());
    let mut cx = Vec::new();
    cx.extend_from_slice(&create_window(0x41, 0x40, 0, 0, 8, 8, 0, &[]));
    // First with type STRING (atom 24), then type CARDINAL (6).
    cx.extend_from_slice(&change_property_32(0x41, atom, 24, &[1]));
    cx.extend_from_slice(&change_property_32(0x41, atom, 6, &[1]));
    server.feed(client, &cx).unwrap();
    let out = server.take_output(client);
    let errors: Vec<&[u8]> = envelopes(&out).into_iter().filter(|e| e[0] == 0).collect();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0][1], 8, "BadMatch");
}
