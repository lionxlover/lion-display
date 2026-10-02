//! EC: a real X11 client's pixels travel a real Unix socket, through
//! the bridge process, into the compositor's scanout — the X half of
//! the ecosystem door's exit criterion.
//!
//! The testbench brings the compositor up; the engine serves an X11
//! display on a filesystem socket; a scripted X client connects,
//! completes the setup handshake, creates a window, draws into it
//! with `PutImage`, and maps it — and the compositor's scanout shows
//! the X pixels through the rootful toplevel.

#![allow(clippy::too_many_lines)]

mod common;

use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;

use ldp_transport::sys::{recv_plain, send_msg};

use lion_bridge::{BridgeArgs, Engine};

/// One scripted X client over a real socket (little-endian, core
/// protocol, no auth).
struct XClient {
    stream: UnixStream,
}

impl XClient {
    /// The 12-byte setup prefix (LSB, core 11.0, no auth).
    fn handshake() -> Vec<u8> {
        let mut b = vec![0x6c, 0];
        b.extend_from_slice(&11u16.to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes());
        b.extend_from_slice(&[0; 6]);
        b
    }

    /// One framed request: opcode, zero flag byte, length in 4-byte
    /// units (header included), payload, zero padding.
    fn request(opcode: u8, payload: &[u8]) -> Vec<u8> {
        Self::request_flagged(opcode, 0, payload)
    }

    /// One framed request with an explicit flag byte — PutImage's
    /// ZPixmap format rides there (the header's second byte).
    fn request_flagged(opcode: u8, flag: u8, payload: &[u8]) -> Vec<u8> {
        let mut r = vec![opcode, flag];
        let units = 1 + payload.len().div_ceil(4);
        r.extend_from_slice(&(units as u16).to_le_bytes());
        r.extend_from_slice(payload);
        while r.len() % 4 != 0 {
            r.push(0);
        }
        r
    }

    fn send(&mut self, bytes: &[u8]) {
        send_msg(self.stream.as_raw_fd(), bytes, None).expect("x send");
    }

    /// The nonblocking drain.
    fn drain(&mut self) -> Vec<u8> {
        let mut all = Vec::new();
        let mut buf = [0u8; 16 * 1024];
        loop {
            match recv_plain(self.stream.as_raw_fd(), &mut buf) {
                Ok(g) if g.bytes > 0 => all.extend_from_slice(&buf[..g.bytes]),
                _ => break,
            }
        }
        all
    }

    /// Drain until bytes arrive or the deadline passes.
    fn await_bytes(&mut self, ms: u64) -> Vec<u8> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(ms);
        loop {
            let got = self.drain();
            if !got.is_empty() {
                return got;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the X bridge never answered"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

/// CreateWindow (opcode 1) — the real payload order: wid, parent,
/// x, y, w, h, border-width, class (1=InputOutput), visual,
/// value-mask, then the value list.
fn create_window(wid: u32, parent: u32, x: i16, y: i16, w: u16, h: u16) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&wid.to_le_bytes());
    p.extend_from_slice(&parent.to_le_bytes());
    p.extend_from_slice(&x.to_le_bytes());
    p.extend_from_slice(&y.to_le_bytes());
    p.extend_from_slice(&w.to_le_bytes());
    p.extend_from_slice(&h.to_le_bytes());
    p.extend_from_slice(&1u16.to_le_bytes()); // border width
    p.extend_from_slice(&1u16.to_le_bytes()); // class: InputOutput
    p.extend_from_slice(&0x21u32.to_le_bytes()); // the visual
    p.extend_from_slice(&0u32.to_le_bytes()); // no value mask
    XClient::request(1, &p)
}

/// PutImage (opcode 72, ZPixmap) — the real xPutImageReq payload
/// order: width, height, dst-x, dst-y, drawable, gc, left-pad (1
/// byte), depth (1 byte), then the pixel data. The format rides the
/// header's flag byte.
fn put_image(drawable: u32, gc: u32, x: i16, y: i16, w: u16, h: u16, pixels: &[u8]) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&w.to_le_bytes());
    p.extend_from_slice(&h.to_le_bytes());
    p.extend_from_slice(&x.to_le_bytes());
    p.extend_from_slice(&y.to_le_bytes());
    p.extend_from_slice(&drawable.to_le_bytes());
    p.extend_from_slice(&gc.to_le_bytes());
    p.push(0); // left pad
    p.push(24); // depth
    p.extend_from_slice(pixels);
    XClient::request_flagged(72, 2, &p)
}

/// MapWindow (opcode 8).
fn map_window(wid: u32) -> Vec<u8> {
    XClient::request(8, &wid.to_le_bytes())
}

/// CreateGC (opcode 55) with no value list.
fn create_gc(gcid: u32, drawable: u32) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&gcid.to_le_bytes());
    p.extend_from_slice(&drawable.to_le_bytes());
    p.extend_from_slice(&0u32.to_le_bytes()); // no value mask
    XClient::request(55, &p)
}

/// The full X lifecycle through the real stack.
#[test]
fn an_x11_clients_pixels_reach_the_composits_scanout() {
    let tb = common::Testbench::start("bridge-x11");

    // The bridge token.
    let token_path = std::env::temp_dir().join(format!(
        "lion-bridge-x11-token-{}-{}.txt",
        std::process::id(),
        line!()
    ));
    std::fs::write(&token_path, "1 2 3 4 5 6 7 8").expect("token file");

    // A small root screen so the scanout assertions are tight.
    let x11_path = format!(
        "/tmp/lion-bridge-x11-{}-{}.sock",
        std::process::id(),
        line!()
    );
    let args = BridgeArgs {
        socket: Some(tb.addr.display_string().trim_start_matches('@').to_owned()),
        wayland: Vec::new(),
        x11: vec![x11_path.clone()],
        token: Some(token_path.display().to_string()),
        screen: (64, 48),
        // The rootful escape: this suite is the whole-screen
        // regression (the rootless default has its own session
        // proof below — `bridge_x11_rootless_session.rs`).
        x11_rootful: true,
        ..BridgeArgs::with_defaults()
    };

    let engine = std::thread::Builder::new()
        .name("bridge-engine".to_owned())
        .spawn(move || {
            let mut engine = Engine::start(&args, Box::new(std::io::stdout())).expect("engine");
            let _ = engine.run(Some(8_000));
        })
        .expect("engine thread");

    // Connect the X client once the listener is up.
    let mut deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let stream = loop {
        if let Ok(s) = UnixStream::connect(&x11_path) {
            let _ = s.set_nonblocking(true);
            break s;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the bridge's X socket never accepted"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    let mut client = XClient { stream };

    // ---- the setup handshake ----
    client.send(&XClient::handshake());
    let setup_reply = client.await_bytes(5_000);
    assert!(!setup_reply.is_empty(), "the setup reply arrived");
    assert_eq!(setup_reply[0], 1, "success byte (not an error)");

    // ---- window + gc + drawing ----
    let root = 0x40; // the bridge's root XID
    let wid = 0x0021_0001;
    let gcid = 0x0021_0002;
    client.send(&create_window(wid, root, 8, 8, 8, 4));
    client.send(&create_gc(gcid, wid));
    // 8x4 pixels of one color: bytes B,G,R,pad (XRGB little-endian).
    let pixels: Vec<u8> = std::iter::repeat([0x55u8, 0x44, 0x33, 0x00])
        .take(8 * 4)
        .flatten()
        .collect();
    client.send(&put_image(wid, gcid, 0, 0, 8, 4, &pixels));
    client.send(&map_window(wid));
    let replies = client.drain();
    // Any X error envelopes (first byte 0) would explain a stall.
    for env in replies.chunks_exact(32) {
        if env[0] == 0 {
            panic!(
                "the X server rejected a request: error code {} on {:?}",
                env[1],
                &env[4..8]
            );
        }
    }

    // ---- the exit criterion: the compositor's scanout ----
    // The X face mirrors the root store per frame tick; the window at
    // (8, 8) of the 64-wide root lands at word 8 + 8*64 on the LDP
    // surface. The mock panel is 1920x1080 — the rootful toplevel
    // sits at the origin, so the X screen's pixel (x, y) is the
    // scanout's (x, y) verbatim.
    deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let words = tb.scanout();
        // The X pixel (8, 8): B=55 G=44 R=33 → the LDP word with the
        // X-family alpha forced opaque.
        let at = 8 + 8 * 1920;
        if words.get(at).copied() == Some(0xFF33_4455) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the X pixels never reached the scanout (word at (8,8): {:08x})",
            words.get(at).copied().unwrap_or(0)
        );
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    // The window's own pixel just inside its rect (9, 9) shows too.
    let inside = 9 + 9 * 1920;
    let words = tb.scanout();
    assert_eq!(
        words[inside], 0xFF33_4455,
        "the second X pixel, through the root store"
    );
    // Outside the window stays the root's black.
    let outside = 40 + 40 * 1920;
    assert_eq!(words[outside], 0xFF00_0000, "the root beyond the window");

    assert!(tb.frames() >= 1, "the X frame rendered");

    drop(client);
    let _ = engine.join();
    let _ = std::fs::remove_file(&token_path);
    let _ = std::fs::remove_file(&x11_path);
}
