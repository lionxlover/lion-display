//! EC: the rootless door — a real X11 client's *windows* (plural, each
//! its own LDP surface) travel a real Unix socket, through the bridge
//! process, onto the compositor's scanout as first-class windows: the
//! cascade places them, an override-redirect menu anchors as a popup
//! at its own screen rect, damage isolates per window, input arrives
//! with the X coordinate fiction bridged (surface-local → the X
//! window's own geometry), and an unmap tears the export down.
//!
//! The v0.10.3 doctrine under test: the X protocol's geometry is the
//! X clients' truth; the LDP display's placement is the screen's
//! truth; the two coexist without a single X window being moved.

#![allow(clippy::too_many_lines)]

mod common;

use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;

use ldp_transport::sys::{recv_plain, send_msg};
use lion_bridge::{BridgeArgs, Engine};
use lion_compositor::server::CompositorConfig;
use lion_compositor::shell::{DockMode, ShellConfig};

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

    /// One framed request with an explicit flag byte (PutImage's
    /// ZPixmap format rides there).
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

    /// Drain until a predicate over the accumulated bytes holds.
    fn await_bytes_where(&mut self, ms: u64, pred: impl Fn(&[u8]) -> bool, what: &str) -> Vec<u8> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(ms);
        loop {
            let got = self.drain();
            if !got.is_empty() && pred(&got) {
                return got;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the X bridge never answered: {what}"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

/// CreateWindow with a background pixel and an event mask (the
/// subset's CW order: back-pixel bit 1, event-mask bit 8,
/// override-redirect bit 10).
#[allow(clippy::too_many_arguments)]
fn create_window_masked(
    wid: u32,
    parent: u32,
    x: i16,
    y: i16,
    w: u16,
    h: u16,
    event_mask: u32,
    or: bool,
) -> Vec<u8> {
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
    let mut mask: u32 = (1 << 1) | (1 << 8); // back-pixel + event-mask
    if or {
        mask |= 1 << 10;
    }
    p.extend_from_slice(&mask.to_le_bytes());
    p.extend_from_slice(&0x00ff_ff00u32.to_le_bytes()); // background
    p.extend_from_slice(&event_mask.to_le_bytes());
    if or {
        p.push(1); // override-redirect: true
        p.extend_from_slice(&[0, 0, 0]);
    }
    XClient::request(1, &p)
}

/// CreateGC with a foreground pixel.
fn create_gc(gcid: u32, drawable: u32, fg: u32) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&gcid.to_le_bytes());
    p.extend_from_slice(&drawable.to_le_bytes());
    p.extend_from_slice(&(1u32 << 2).to_le_bytes()); // foreground
    p.extend_from_slice(&fg.to_le_bytes());
    XClient::request(55, &p)
}

/// PutImage (opcode 72, ZPixmap): w, h, x, y, drawable, gc, left-pad,
/// depth, pixels.
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

/// UnmapWindow (opcode 10).
fn unmap_window(wid: u32) -> Vec<u8> {
    XClient::request(10, &wid.to_le_bytes())
}

/// One solid-color pixel run (B, G, R, pad per pixel).
fn solid(rgb: [u8; 3], count: usize) -> Vec<u8> {
    std::iter::repeat([rgb[0], rgb[1], rgb[2], 0])
        .take(count)
        .flatten()
        .collect()
}

/// The scanout word at (x, y) of the 1920-wide panel.
fn word_at(words: &[u32], x: usize, y: usize) -> u32 {
    words[y * 1920 + x]
}

/// Where a color first appears on the scanout (the failure dump).
fn find_word(words: &[u32], needle: u32) -> Option<(usize, usize)> {
    words
        .iter()
        .position(|w| *w == needle)
        .map(|i| (i % 1920, i / 1920))
}

/// The full rootless lifecycle through the real stack.
#[test]
fn x11_windows_ride_their_own_surfaces_rootlessly() {
    // The shell serves: the dock on (the CLI default doctrine), the
    // cascade placing the X windows' toplevels. The effects stay
    // Minimal — raw pixels, the assertions tight.
    let config = CompositorConfig {
        socket: format!("lion-bridge-rootless-{}", std::process::id()),
        shell: ShellConfig {
            dock: DockMode::Auto,
            ..ShellConfig::default()
        },
        ..CompositorConfig::default()
    };
    let tb = common::Testbench::start_with("bridge-rootless", config);

    // The bridge token.
    let token_path = std::env::temp_dir().join(format!(
        "lion-bridge-rootless-token-{}-{}.txt",
        std::process::id(),
        line!()
    ));
    std::fs::write(&token_path, "1 2 3 4 5 6 7 8").expect("token file");

    let x11_path = format!(
        "/tmp/lion-bridge-rootless-{}-{}.sock",
        std::process::id(),
        line!()
    );
    // NO --x11-rootful: the rootless default is the doctrine under
    // test.
    let args = BridgeArgs {
        socket: Some(tb.addr.display_string().trim_start_matches('@').to_owned()),
        wayland: Vec::new(),
        x11: vec![x11_path.clone()],
        token: Some(token_path.display().to_string()),
        screen: (512, 384),
        ..BridgeArgs::with_defaults()
    };

    let engine = std::thread::Builder::new()
        .name("bridge-engine-rootless".to_owned())
        .spawn(move || {
            let mut engine = Engine::start(&args, Box::new(std::io::stdout())).expect("engine");
            let _ = engine.run(Some(20_000));
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
    assert_eq!(setup_reply[0], 1, "success byte (not an error)");

    // ---- three windows, three X geometries, three fates ----------
    //
    // The two managed windows carry the POINTER_MOTION mask (the
    // input proof below); the OR window (a menu) does not need one.
    let root = 0x40;
    let w1 = 0x0021_0001; // X geometry (100, 60) — the X truth
    let w2 = 0x0021_0002; // X geometry (200, 100)
    let w3 = 0x0021_0003; // OR: X geometry (12, 2) — a menu
    const MOTION: u32 = 1 << 6;
    client.send(&create_window_masked(
        w1, root, 100, 60, 8, 4, MOTION, false,
    ));
    client.send(&create_gc(0x0021_0101, w1, 0x0033_4455));
    client.send(&put_image(
        w1,
        0x0021_0101,
        0,
        0,
        8,
        4,
        &solid([0x55, 0x44, 0x33], 32),
    ));
    client.send(&map_window(w1));
    client.send(&create_window_masked(
        w2, root, 200, 100, 8, 4, MOTION, false,
    ));
    client.send(&create_gc(0x0021_0102, w2, 0x0055_6677));
    client.send(&put_image(
        w2,
        0x0021_0102,
        0,
        0,
        8,
        4,
        &solid([0x77, 0x66, 0x55], 32),
    ));
    client.send(&map_window(w2));
    client.send(&create_window_masked(w3, root, 12, 2, 6, 2, 0, true));
    client.send(&create_gc(0x0021_0103, w3, 0x0088_99aa));
    client.send(&put_image(
        w3,
        0x0021_0103,
        0,
        0,
        6,
        2,
        &solid([0xaa, 0x99, 0x88], 12),
    ));
    client.send(&map_window(w3));
    // Any X error envelopes would explain a stall.
    for env in client.drain().chunks_exact(32) {
        if env[0] == 0 {
            panic!(
                "the X server rejected a request: error code {} on {:?}",
                env[1],
                &env[4..8]
            );
        }
    }

    // ---- the placement doctrine -----------------------------------
    //
    // The cascade places the toplevels by *attach order* (the X
    // stacking order): window 1 at (0, 0), window 2 at (24, 24). The
    // OR window is a popup — anchored at its own screen rect (12, 2)
    // by the null-parent doctrine, never cascaded.
    let expect = |x: usize, y: usize| y * 1920 + x;
    deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let words = tb.scanout();
        let one = words.get(expect(0, 0)).copied();
        let two = words.get(expect(24, 24)).copied();
        let menu = words.get(expect(12, 2)).copied();
        if one == Some(0xFF33_4455) && two == Some(0xFF55_6677) && menu == Some(0xFF88_99AA) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the rootless windows never reached the scanout: \
             one={one:08x?} two={two:08x?} menu={menu:08x?} \
             found_one={:?} found_two={:?} found_menu={:?} frames={}",
            find_word(&tb.scanout(), 0xFF33_4455),
            find_word(&tb.scanout(), 0xFF55_6677),
            find_word(&tb.scanout(), 0xFF88_99AA),
            tb.frames()
        );
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    // Window 2's own interior pixel (25, 25) and the menu's (14, 3).
    {
        let words = tb.scanout();
        assert_eq!(word_at(&words, 25, 25), 0xFF55_6677);
        assert_eq!(word_at(&words, 14, 3), 0xFF88_99AA);
        // Beyond every window: the desktop's own background.
        assert_eq!(word_at(&words, 100, 100), 0xFF00_0000);
    }

    // ---- the input translation (the coordinate fiction) ----------
    //
    // The compositor's pointer moves to (26, 27) — inside window 2's
    // LDP rect (24, 24)+8x4, surface-local (2, 3). The bridge
    // translates: X root = the X window's own origin (200, 100) +
    // local = (202, 103). The X client sees MotionNotify with
    // root_x = 202, event_x = 2 — the click hits the pixel the user
    // sees; the X client sees the click at its own geometry.
    let routed = tb.world_mut(|w| {
        w.queue_input(
            ldp_input::device::DeviceClass::Mouse,
            vec![ldp_input::normalizer::InputEvent::PointerMotion { dx: 26, dy: 27 }],
        );
        w.pump_input()
    });
    assert!(routed > 0, "the motion routed to the bridge's surface");
    // MotionNotify (code 6): root_x at bytes 20-21, event_x at 24-25.
    let motion = client.await_bytes_where(5_000, |b| b.first() == Some(&6), "the motion event");
    let root_x = i16::from_le_bytes([motion[20], motion[21]]);
    let root_y = i16::from_le_bytes([motion[22], motion[23]]);
    let event_x = i16::from_le_bytes([motion[24], motion[25]]);
    let event_y = i16::from_le_bytes([motion[26], motion[27]]);
    assert_eq!((root_x, root_y), (202, 103), "the X root coordinates");
    assert_eq!((event_x, event_y), (2, 3), "the window-local coordinates");

    // ---- the damage isolation -------------------------------------
    //
    // Draw ONLY into window 2 (a 2x2 patch at (1, 1)): the next
    // commit carries window 2's surface alone. The scanout shows the
    // patch; every other window's pixels stand untouched.
    client.send(&put_image(
        w2,
        0x0021_0102,
        1,
        1,
        2,
        2,
        &solid([0xff, 0xee, 0xdd], 4),
    ));
    deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let words = tb.scanout();
        if word_at(&words, 25, 25) == 0xFFDD_EEFF {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the isolated damage never landed"
        );
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    {
        let words = tb.scanout();
        // Window 1 and the menu stand untouched by window 2's commit.
        assert_eq!(word_at(&words, 0, 0), 0xFF33_4455);
        assert_eq!(word_at(&words, 12, 2), 0xFF88_99AA);
        // The patch's neighborhood outside the 2x2 keeps the old color.
        assert_eq!(word_at(&words, 24, 24), 0xFF55_6677);
    }

    // ---- the teardown ---------------------------------------------
    //
    // Unmap window 1: its export tears down (the role, the buffer,
    // the surface — the generic destroy path), the scanout drops its
    // pixels back to the desktop's background, and the others stand.
    client.send(&unmap_window(w1));
    deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let words = tb.scanout();
        if word_at(&words, 0, 0) == 0xFF00_0000 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the unmapped window's pixels never left the scanout"
        );
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    {
        let words = tb.scanout();
        // The patch (25, 25) — window 2 local (1, 1), inside the 2x2.
        assert_eq!(word_at(&words, 25, 25), 0xFFDD_EEFF, "window 2 stands");
        assert_eq!(word_at(&words, 12, 2), 0xFF88_99AA, "the menu stands");
    }

    assert!(tb.frames() >= 1, "the X frames rendered");

    drop(client);
    let _ = engine.join();
    let _ = std::fs::remove_file(&token_path);
    let _ = std::fs::remove_file(&x11_path);
}
