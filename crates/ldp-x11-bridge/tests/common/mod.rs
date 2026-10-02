//! Shared X-client scripting for the bridge's EC suites.
//!
//! The suites play real X clients at the byte level: a setup
//! handshake, then core requests framed exactly as the wire demands.
//! Every builder here mirrors the protocol's field tables; the suites
//! assert the bytes that come back.

#![allow(dead_code)] // each suite uses a different subset
#![allow(clippy::too_many_arguments)] // the builders restate flat wire records
#![allow(clippy::many_single_char_names)] // x/y/w/h restate the wire vocabulary

use ldp_x11_bridge::dispatch::{ClientId, Server};
use ldp_x11_bridge::setup::ScreenParams;
use ldp_x11_bridge::shm::NoShm;

/// The 12-byte setup prefix (LSB, core 11.0, no auth).
pub fn handshake() -> Vec<u8> {
    let mut b = vec![0x6c, 0];
    b.extend_from_slice(&11u16.to_le_bytes());
    b.extend_from_slice(&0u16.to_le_bytes());
    b.extend_from_slice(&[0; 6]);
    b
}

/// One framed request: opcode, flag byte, length in 4-byte units
/// (header included), payload, zero padding to the unit boundary.
pub fn request(opcode: u8, flag: u8, payload: &[u8]) -> Vec<u8> {
    let mut r = vec![opcode, flag];
    let units = 1 + (payload.len().div_ceil(4));
    r.extend_from_slice(&(units as u16).to_le_bytes());
    r.extend_from_slice(payload);
    while r.len() % 4 != 0 {
        r.push(0);
    }
    r
}

/// CreateWindow (opcode 1). `mask`/`values` are the CW bitmask and
/// its 4-byte-slotted value list.
pub fn create_window(
    wid: u32,
    parent: u32,
    x: i16,
    y: i16,
    w: u16,
    h: u16,
    mask: u32,
    values: &[u32],
) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&wid.to_le_bytes());
    p.extend_from_slice(&parent.to_le_bytes());
    p.extend_from_slice(&x.to_le_bytes());
    p.extend_from_slice(&y.to_le_bytes());
    p.extend_from_slice(&w.to_le_bytes());
    p.extend_from_slice(&h.to_le_bytes());
    p.extend_from_slice(&0u16.to_le_bytes()); // border width
    p.extend_from_slice(&1u16.to_le_bytes()); // class: InputOutput
    p.extend_from_slice(&0x21u32.to_le_bytes()); // visual
    p.extend_from_slice(&mask.to_le_bytes());
    for v in values {
        p.extend_from_slice(&v.to_le_bytes());
    }
    request(1, 0, &p)
}

/// MapWindow (8).
pub fn map_window(wid: u32) -> Vec<u8> {
    request(8, 0, &wid.to_le_bytes())
}

/// UnmapWindow (10).
pub fn unmap_window(wid: u32) -> Vec<u8> {
    request(10, 0, &wid.to_le_bytes())
}

/// DestroyWindow (4).
pub fn destroy_window(wid: u32) -> Vec<u8> {
    request(4, 0, &wid.to_le_bytes())
}

/// CreateGC (55) with a value list.
pub fn create_gc(cid: u32, drawable: u32, mask: u32, values: &[u32]) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&cid.to_le_bytes());
    p.extend_from_slice(&drawable.to_le_bytes());
    p.extend_from_slice(&mask.to_le_bytes());
    for v in values {
        p.extend_from_slice(&v.to_le_bytes());
    }
    request(55, 0, &p)
}

/// PolyFillRectangle (70).
pub fn fill_rects(drawable: u32, gc: u32, rects: &[(i16, i16, u16, u16)]) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&drawable.to_le_bytes());
    p.extend_from_slice(&gc.to_le_bytes());
    for &(x, y, w, h) in rects {
        p.extend_from_slice(&x.to_le_bytes());
        p.extend_from_slice(&y.to_le_bytes());
        p.extend_from_slice(&w.to_le_bytes());
        p.extend_from_slice(&h.to_le_bytes());
    }
    request(70, 0, &p)
}

/// PolyLine (65) in origin mode.
pub fn poly_line(drawable: u32, gc: u32, pts: &[(i16, i16)]) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&drawable.to_le_bytes());
    p.extend_from_slice(&gc.to_le_bytes());
    for &(x, y) in pts {
        p.extend_from_slice(&x.to_le_bytes());
        p.extend_from_slice(&y.to_le_bytes());
    }
    request(65, 0, &p)
}

/// PolyPoint (64) in origin mode.
pub fn poly_point(drawable: u32, gc: u32, pts: &[(i16, i16)]) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&drawable.to_le_bytes());
    p.extend_from_slice(&gc.to_le_bytes());
    for &(x, y) in pts {
        p.extend_from_slice(&x.to_le_bytes());
        p.extend_from_slice(&y.to_le_bytes());
    }
    request(64, 0, &p)
}

/// PolyArc (68).
pub fn poly_arc(drawable: u32, gc: u32, arcs: &[(i16, i16, u16, u16, i16, i16)]) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&drawable.to_le_bytes());
    p.extend_from_slice(&gc.to_le_bytes());
    for &(x, y, w, h, a1, a2) in arcs {
        p.extend_from_slice(&x.to_le_bytes());
        p.extend_from_slice(&y.to_le_bytes());
        p.extend_from_slice(&w.to_le_bytes());
        p.extend_from_slice(&h.to_le_bytes());
        p.extend_from_slice(&a1.to_le_bytes());
        p.extend_from_slice(&a2.to_le_bytes());
    }
    request(68, 0, &p)
}

/// PolyFillArc (71).
pub fn poly_fill_arc(drawable: u32, gc: u32, arcs: &[(i16, i16, u16, u16, i16, i16)]) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&drawable.to_le_bytes());
    p.extend_from_slice(&gc.to_le_bytes());
    for &(x, y, w, h, a1, a2) in arcs {
        p.extend_from_slice(&x.to_le_bytes());
        p.extend_from_slice(&y.to_le_bytes());
        p.extend_from_slice(&w.to_le_bytes());
        p.extend_from_slice(&h.to_le_bytes());
        p.extend_from_slice(&a1.to_le_bytes());
        p.extend_from_slice(&a2.to_le_bytes());
    }
    request(71, 0, &p)
}

/// FillPoly (69) in origin mode.
pub fn fill_poly(drawable: u32, gc: u32, pts: &[(i16, i16)]) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&drawable.to_le_bytes());
    p.extend_from_slice(&gc.to_le_bytes());
    p.extend_from_slice(&0i16.to_le_bytes()); // coordinate mode
    p.extend_from_slice(&[0; 2]); // pad to the point list
    for &(x, y) in pts {
        p.extend_from_slice(&x.to_le_bytes());
        p.extend_from_slice(&y.to_le_bytes());
    }
    request(69, 0, &p)
}

/// PutImage (72), ZPixmap depth 24.
pub fn put_image_zpixmap(
    drawable: u32,
    gc: u32,
    w: u16,
    h: u16,
    dst_x: i16,
    dst_y: i16,
    data: &[u8],
) -> Vec<u8> {
    // The real xPutImageReq payload order: width, height, dst-x,
    // dst-y, drawable, gc, left-pad, depth — then the data; the
    // format (ZPixmap=2) rides the header's flag byte.
    let mut p = Vec::new();
    p.extend_from_slice(&w.to_le_bytes());
    p.extend_from_slice(&h.to_le_bytes());
    p.extend_from_slice(&dst_x.to_le_bytes());
    p.extend_from_slice(&dst_y.to_le_bytes());
    p.extend_from_slice(&drawable.to_le_bytes());
    p.extend_from_slice(&gc.to_le_bytes());
    p.push(0); // left pad
    p.push(24); // depth
    p.extend_from_slice(data);
    request(72, 2, &p)
}

/// InternAtom (16).
pub fn intern_atom(name: &str, only_if_exists: bool) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&(name.len() as u16).to_le_bytes());
    p.extend_from_slice(&[0; 2]);
    p.extend_from_slice(name.as_bytes());
    request(16, u8::from(only_if_exists), &p)
}

/// ChangeProperty (18), Replace mode, format 32.
pub fn change_property_32(window: u32, property: u32, type_: u32, data: &[u32]) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&window.to_le_bytes());
    p.extend_from_slice(&property.to_le_bytes());
    p.extend_from_slice(&type_.to_le_bytes());
    p.extend_from_slice(&32u8.to_le_bytes());
    p.extend_from_slice(&[0; 3]);
    p.extend_from_slice(&(data.len() as u32).to_le_bytes());
    for v in data {
        p.extend_from_slice(&v.to_le_bytes());
    }
    request(18, 0, &p)
}

/// ChangeProperty (18), Replace mode, format 8 (strings).
pub fn change_property_8(window: u32, property: u32, type_: u32, data: &[u8]) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&window.to_le_bytes());
    p.extend_from_slice(&property.to_le_bytes());
    p.extend_from_slice(&type_.to_le_bytes());
    p.extend_from_slice(&1u8.to_le_bytes());
    p.extend_from_slice(&[0; 3]);
    p.extend_from_slice(&(data.len() as u32).to_le_bytes());
    p.extend_from_slice(data);
    request(18, 0, &p)
}

/// ConfigureWindow (12).
pub fn configure_window(wid: u32, mask: u16, values: &[u32]) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&wid.to_le_bytes());
    p.extend_from_slice(&mask.to_le_bytes());
    p.extend_from_slice(&[0; 2]);
    for v in values {
        p.extend_from_slice(&v.to_le_bytes());
    }
    request(12, 0, &p)
}

/// QueryExtension (94).
pub fn query_extension(name: &str) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&(name.len() as u16).to_le_bytes());
    p.extend_from_slice(&[0; 2]);
    p.extend_from_slice(name.as_bytes());
    request(94, 0, &p)
}

/// ClearArea (61) with exposures.
pub fn clear_area(window: u32, x: i16, y: i16, w: u16, h: u16, exposures: bool) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&window.to_le_bytes());
    p.extend_from_slice(&x.to_le_bytes());
    p.extend_from_slice(&y.to_le_bytes());
    p.extend_from_slice(&w.to_le_bytes());
    p.extend_from_slice(&h.to_le_bytes());
    request(61, u8::from(exposures), &p)
}

/// CopyArea (62).
pub fn copy_area(
    src: u32,
    dst: u32,
    gc: u32,
    src_x: i16,
    src_y: i16,
    dst_x: i16,
    dst_y: i16,
    w: u16,
    h: u16,
) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&src.to_le_bytes());
    p.extend_from_slice(&dst.to_le_bytes());
    p.extend_from_slice(&gc.to_le_bytes());
    p.extend_from_slice(&src_x.to_le_bytes());
    p.extend_from_slice(&src_y.to_le_bytes());
    p.extend_from_slice(&dst_x.to_le_bytes());
    p.extend_from_slice(&dst_y.to_le_bytes());
    p.extend_from_slice(&w.to_le_bytes());
    p.extend_from_slice(&h.to_le_bytes());
    request(62, 0, &p)
}

/// CreatePixmap (53), depth 24 in the flag byte.
pub fn create_pixmap(pid: u32, drawable: u32, w: u16, h: u16) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&pid.to_le_bytes());
    p.extend_from_slice(&drawable.to_le_bytes());
    p.extend_from_slice(&w.to_le_bytes());
    p.extend_from_slice(&h.to_le_bytes());
    request(53, 24, &p)
}

/// SetDashes (58).
pub fn set_dashes(gc: u32, offset: u16, dashes: &[u8]) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&gc.to_le_bytes());
    p.extend_from_slice(&offset.to_le_bytes());
    p.extend_from_slice(&(dashes.len() as u16).to_le_bytes());
    p.extend_from_slice(dashes);
    request(58, 0, &p)
}

/// SetClipRectangles (59), Unsorted.
pub fn set_clip_rectangles(gc: u32, ox: i16, oy: i16, rects: &[(i16, i16, u16, u16)]) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&gc.to_le_bytes());
    p.extend_from_slice(&ox.to_le_bytes());
    p.extend_from_slice(&oy.to_le_bytes());
    for &(x, y, w, h) in rects {
        p.extend_from_slice(&x.to_le_bytes());
        p.extend_from_slice(&y.to_le_bytes());
        p.extend_from_slice(&w.to_le_bytes());
        p.extend_from_slice(&h.to_le_bytes());
    }
    request(59, 0, &p)
}

/// BIG-REQUESTS Enable (major 131, minor 0).
pub fn bigreq_enable() -> Vec<u8> {
    request(131, 0, &[])
}

/// MIT-SHM Attach (minor 1) — the real wire order: shmseg (the
/// client's XID), shmid (the SysV key), read-only, three pad bytes.
/// The `shmid` is distinct from the XID so a parser that mixes the
/// two fields cannot pass the suites.
pub fn shm_attach(seg: u32, shmid: u32, read_only: bool) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&seg.to_le_bytes());
    p.extend_from_slice(&shmid.to_le_bytes());
    p.push(u8::from(read_only));
    p.extend_from_slice(&[0; 3]);
    request(130, 1, &p)
}

/// MIT-SHM PutImage (minor 3).
pub fn shm_put_image(
    drawable: u32,
    gc: u32,
    total_w: u16,
    total_h: u16,
    src_x: u16,
    src_y: u16,
    src_w: u16,
    src_h: u16,
    dst_x: i16,
    dst_y: i16,
    seg: u32,
    offset: u32,
) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&drawable.to_le_bytes());
    p.extend_from_slice(&gc.to_le_bytes());
    p.extend_from_slice(&total_w.to_le_bytes());
    p.extend_from_slice(&total_h.to_le_bytes());
    p.extend_from_slice(&src_x.to_le_bytes());
    p.extend_from_slice(&src_y.to_le_bytes());
    p.extend_from_slice(&src_w.to_le_bytes());
    p.extend_from_slice(&src_h.to_le_bytes());
    p.extend_from_slice(&dst_x.to_le_bytes());
    p.extend_from_slice(&dst_y.to_le_bytes());
    p.push(24); // depth
    p.push(2); // ZPixmap
    p.push(0); // no completion event
    p.push(0); // pad
    p.extend_from_slice(&seg.to_le_bytes());
    p.extend_from_slice(&offset.to_le_bytes());
    request(130, 3, &p)
}

/// MIT-SHM QueryVersion (minor 0).
pub fn shm_query_version() -> Vec<u8> {
    request(130, 0, &[])
}

/// A fresh server + client with the handshake already exchanged; the
/// setup reply is returned for field assertions.
pub fn connect(screen: ScreenParams) -> (Server, ClientId, Vec<u8>) {
    let mut server = Server::new(screen, Box::new(NoShm));
    let client = server.add_client();
    server.feed(client, &handshake()).unwrap();
    let setup_reply = server.take_output(client);
    (server, client, setup_reply)
}

/// The default screen.
pub fn screen() -> ScreenParams {
    ScreenParams::default()
}

/// Split a client's output into 32-byte envelopes.
pub fn envelopes(out: &[u8]) -> Vec<&[u8]> {
    assert_eq!(out.len() % 32, 0, "envelopes are 32 bytes each");
    out.chunks_exact(32).collect()
}
