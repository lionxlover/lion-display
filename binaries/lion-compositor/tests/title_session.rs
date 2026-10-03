//! Phase 54's exit criterion (the title glyph): the drawn title
//! served end to end through the real protocol —
//!
//! * the **drawn strip**: a server-decorated window's title renders
//!   as real glyphs over the band (CPU coverage tinted with the
//!   compositor's own typeface, the exact words pinned against the
//!   same face and the renderer's own over-rule — zero drift by
//!   construction);
//! * the **pixel-true truncation**: a title wider than the band's
//!   budget ends in the ellipsis glyph, the ink never reaching the
//!   close affordance's territory;
//! * the **title change**: `set_title` on a serving window repaints
//!   the strip with no client commit — the ink is the server's, the
//!   claim the server's own (the set_material doctrine);
//! * the **empty title**: no set_title, no strip — the whole scanout
//!   byte-identical to the Phase 52 chrome;
//! * the **territory**: CSD windows and fullscreen covers draw no
//!   title (the chrome truth gates the strip);
//! * the **sharing proof**: same-shaped windows share the base
//!   raster, their distinct titles ride distinct strips (the cache's
//!   own observables).

mod testbench;

use ldp_client::Proxy;
use ldp_core::geometry::Rect;
use ldp_core::scale::ScaleFactor;
use ldp_core::wire::Value;
use ldp_font::LION_SANS;
use ldp_renderer::over_premul;
use testbench::*;

use ldp_shell::ssd::{CLOSE_MARGIN, CLOSE_SIZE, TITLE_SIDE};
use lion_compositor::server::CompositorConfig;
use lion_compositor::shell::{self, DockMode, ShellConfig, BAND_ALPHA, BAND_RGB, TITLE_RGB};

/// The XRGB8888 fourcc ("XR24").
const XR24: u32 = 0x3432_5258;
/// The mock output: 1920x1080@60.
const OUT_W: usize = 1920;

/// One XRGB word.
fn xrgb(r: u8, g: u8, b: u8) -> u32 {
    u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b)
}

/// One scanout pixel as a raw word.
fn word(words: &[u32], x: usize, y: usize) -> u32 {
    words[y * OUT_W + x]
}

/// One opaque straight color's scanout word.
fn opaque(v: u32) -> u32 {
    0xFF00_0000 | (v & 0x00FF_FFFF)
}

/// The band's scanout word over the opaque-black desktop (the
/// Phase 52 oracle's own arithmetic).
fn band_word() -> u32 {
    let m = |v: u8| (u32::from(v) * u32::from(BAND_ALPHA) + 127) / 255;
    0xFF00_0000 | (m(BAND_RGB[0]) << 16) | (m(BAND_RGB[1]) << 8) | m(BAND_RGB[2])
}

/// The tests' configs: dock auto, far from every window.
fn title_config(tag: &str) -> CompositorConfig {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    CompositorConfig {
        shell: ShellConfig {
            dock: DockMode::Auto,
            ..ShellConfig::default()
        },
        socket: format!("lion-title-{tag}-{n}-{}", std::process::id()),
        ..CompositorConfig::default()
    }
}

/// A memfd carrying `pixels` (the pool backing).
fn pool_bytes(pixels: &[u8]) -> std::os::fd::OwnedFd {
    use std::io::Write as _;
    let fd = lion_compositor::sys::memfd("title-pool").expect("memfd");
    {
        let mut file = std::fs::File::from(fd.try_clone().expect("clone"));
        file.write_all(pixels).expect("write pool");
    }
    fd
}

/// Map one plain window at its creation position (no role — the
/// cascade's filler rows). Returns the surface.
fn map_plain(tb: &Testbench, client: &mut TestClient, fill: u32, w: usize, h: usize) -> Proxy {
    let shm = client.bind("ldp.core.shm");
    let compositor = client.bind("ldp.core.compositor");
    let pixels: Vec<u8> = std::iter::repeat(fill.to_le_bytes())
        .take(w * h)
        .flatten()
        .collect();
    let pool = create_pool(client, &shm, pool_bytes(&pixels), (w * h * 4) as i64);
    let buffer = create_buffer(client, &pool, 0, w as i32, h as i32, (w * 4) as i32, XR24);
    let surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("surface");
    let before = tb.frames();
    attach(client, &surface, &buffer);
    damage(client, &surface, &[Rect::new(0, 0, w as u32, h as u32)]);
    commit(client, &surface, 1);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while tb.frames() <= before {
        assert!(deadline > std::time::Instant::now(), "never rendered");
        client.sync();
    }
    surface
}

/// One SSD window realized (the chrome_session recipe, verbatim) —
/// the toplevel minted before the mapping commit, the handshake
/// acked, the first commit mapping at the cascade's `nth` step with
/// the insets applied. Returns (surface, toplevel). 200x100.
fn ssd_window(tb: &Testbench, client: &mut TestClient, fill: u32, nth: usize) -> (Proxy, Proxy) {
    let shell_proxy = client.bind("ldp.shell.shell");
    let shm = client.bind("ldp.core.shm");
    let compositor = client.bind("ldp.core.compositor");
    let surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("surface");
    let toplevel = client
        .conn
        .create_object(
            &shell_proxy,
            "get_toplevel",
            vec![Value::Object(Some(surface.id())), Value::Enum(1)],
        )
        .expect("get_toplevel");
    client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "configure" && r.target == toplevel.id().as_u32())
    });
    let handshake = last_on(client, "configure", toplevel.id().as_u32());
    client
        .conn
        .send_request(
            &toplevel,
            "ack_configure",
            vec![Value::Uint32(serial_of(handshake))],
        )
        .expect("ack");
    let (w, h) = (200usize, 100usize);
    let pixels: Vec<u8> = std::iter::repeat(fill.to_le_bytes())
        .take(w * h)
        .flatten()
        .collect();
    let pool = create_pool(client, &shm, pool_bytes(&pixels), (w * h * 4) as i64);
    let buffer = create_buffer(client, &pool, 0, w as i32, h as i32, (w * 4) as i32, XR24);
    let before = tb.frames();
    attach(client, &surface, &buffer);
    damage(client, &surface, &[Rect::new(0, 0, w as u32, h as u32)]);
    commit(client, &surface, 1);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while tb.frames() <= before {
        assert!(deadline > std::time::Instant::now(), "never rendered");
        client.sync();
    }
    // Phase 56 — the chrome-aware placement: the frame takes the
    // slot; the content rides inside at the slot + insets (the band
    // on-screen from the first frame).
    let step = 24 * nth as i32;
    assert_eq!(position_of(tb, &surface), (step + 1, step + 29));
    (surface, toplevel)
}

/// One SSD window with its title set before the mapping commit (the
/// well-behaved client's order — the title serves from the first
/// frame).
fn titled_window(
    tb: &Testbench,
    client: &mut TestClient,
    fill: u32,
    _nth: usize,
    title: &str,
) -> (Proxy, Proxy) {
    let shell_proxy = client.bind("ldp.shell.shell");
    let shm = client.bind("ldp.core.shm");
    let compositor = client.bind("ldp.core.compositor");
    let surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("surface");
    let toplevel = client
        .conn
        .create_object(
            &shell_proxy,
            "get_toplevel",
            vec![Value::Object(Some(surface.id())), Value::Enum(1)],
        )
        .expect("get_toplevel");
    client
        .conn
        .send_request(
            &toplevel,
            "set_title",
            vec![Value::String(Box::from(title))],
        )
        .expect("set_title");
    client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "configure" && r.target == toplevel.id().as_u32())
    });
    let handshake = last_on(client, "configure", toplevel.id().as_u32());
    client
        .conn
        .send_request(
            &toplevel,
            "ack_configure",
            vec![Value::Uint32(serial_of(handshake))],
        )
        .expect("ack");
    let (w, h) = (200usize, 100usize);
    let pixels: Vec<u8> = std::iter::repeat(fill.to_le_bytes())
        .take(w * h)
        .flatten()
        .collect();
    let pool = create_pool(client, &shm, pool_bytes(&pixels), (w * h * 4) as i64);
    let buffer = create_buffer(client, &pool, 0, w as i32, h as i32, (w * 4) as i32, XR24);
    let before = tb.frames();
    attach(client, &surface, &buffer);
    damage(client, &surface, &[Rect::new(0, 0, w as u32, h as u32)]);
    commit(client, &surface, 1);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while tb.frames() <= before {
        assert!(deadline > std::time::Instant::now(), "never rendered");
        client.sync();
    }
    (surface, toplevel)
}

/// The world's position of one surface.
fn position_of(tb: &Testbench, surface: &Proxy) -> (i32, i32) {
    tb.world(|w| {
        for (sid, route) in &w.scene.routes {
            if route.surface_obj.as_u32() == surface.id().as_u32() {
                return w
                    .scene
                    .tree
                    .get(*sid)
                    .map_or((0, 0), ldp_compositor::surface::Surface::position);
            }
        }
        panic!("the surface has no route");
    })
}

/// The serial of a recorded configure (arg 0).
fn serial_of(record: &Recorded) -> u32 {
    match &record.args[0] {
        Value::Uint32(s) => *s,
        other => panic!("configure's serial is not a uint: {other:?}"),
    }
}

/// The latest event of one name on one object.
fn last_on<'a>(client: &'a TestClient, name: &str, target: u32) -> &'a Recorded {
    client
        .events
        .records
        .iter()
        .filter(|r| r.event == name && r.target == target)
        .next_back()
        .unwrap_or_else(|| panic!("no '{name}' event on object {target}"))
}

/// Render the expected strip ink for one title: the coverage tinted
/// premultiplied — the painter's own arithmetic, reproduced from the
/// face alone (the oracle's zero-drift doctrine).
fn expected_strip_words(title: &str) -> (u32, u32, Vec<u8>) {
    // The same spec the shell derives for the 202-wide frame at
    // identity scale (the cascade's own geometry).
    let frame_w = 202u32;
    let top = 29u32;
    let px = ldp_shell::ssd::TITLE_PX;
    let side = TITLE_SIDE;
    let avail = frame_w - side - (CLOSE_SIZE + 2 * CLOSE_MARGIN);
    let _ = top;
    let run = LION_SANS.layout(title, px, avail);
    let ink = run.ink.expect("the oracle's title inks");
    let strip_w = ink.width().min(avail).max(1);
    let strip_h = ink.height().max(1);
    let mut words = vec![0u32; strip_w as usize * strip_h as usize];
    for placed in &run.placed {
        let glyph = LION_SANS.bitmap(placed.c, px);
        if glyph.w == 0 || glyph.h == 0 {
            continue;
        }
        let glyph_x = placed.x + glyph.x_off - ink.min_x;
        let glyph_y = ink.top - glyph.y_top;
        for row in 0..glyph.h as i32 {
            for col in 0..glyph.w as i32 {
                let x = glyph_x + col;
                let y = glyph_y + row;
                if x < 0 || y < 0 || x >= strip_w as i32 || y >= strip_h as i32 {
                    continue;
                }
                let coverage = glyph.alpha[row as usize * glyph.w as usize + col as usize];
                if coverage > 0 {
                    let scaled = |chan: u8| (u32::from(chan) * u32::from(coverage) + 127) / 255;
                    words[y as usize * strip_w as usize + x as usize] = u32::from(coverage) << 24
                        | scaled(TITLE_RGB[0]) << 16
                        | scaled(TITLE_RGB[1]) << 8
                        | scaled(TITLE_RGB[2]);
                }
            }
        }
    }
    let mut out = Vec::with_capacity(words.len() * 4);
    for word in &words {
        out.extend_from_slice(&word.to_le_bytes());
    }
    (strip_w, strip_h, out)
}

/// The band layer's own premultiplied word bytes (the layer's ink,
/// before any blend): `[B, G, R, A]` little-endian.
fn band_layer() -> [u8; 4] {
    let m = |v: u8| (u32::from(v) * u32::from(BAND_ALPHA) + 127) / 255;
    [
        m(BAND_RGB[2]) as u8,
        m(BAND_RGB[1]) as u8,
        m(BAND_RGB[0]) as u8,
        BAND_ALPHA,
    ]
}

/// The expected scanout word at one strip pixel: the glyph's ink over
/// the band layer over the pixel's underlay (the renderer's own
/// over-rule, imported — zero drift). `underlay` is the opaque word
/// the band itself landed on.
fn strip_pixel_word(ink: &[u8], w: u32, x_in: u32, y_in: u32, underlay: u32) -> u32 {
    let idx = (y_in as usize * w as usize + x_in as usize) * 4;
    let px = [ink[idx], ink[idx + 1], ink[idx + 2], ink[idx + 3]];
    let after_band = over_premul(band_layer(), underlay.to_le_bytes(), 255);
    u32::from_le_bytes(over_premul(px, after_band, 255))
}

/// The strip truth from the world (one window's live answer).
fn strip_truth(tb: &Testbench, surface: &Proxy) -> (shell::StripKey, Rect) {
    tb.world(|w| {
        let sid = match w
            .scene
            .routes
            .iter()
            .find(|(_, route)| route.surface_obj.as_u32() == surface.id().as_u32())
        {
            Some((sid, _)) => *sid,
            None => panic!("no route"),
        };
        let (pos_x, pos_y) = w
            .scene
            .tree
            .get(sid)
            .map_or((0, 0), ldp_compositor::surface::Surface::position);
        let bounds = Rect::new(pos_x, pos_y, 200, 100);
        let entry = w.toplevels.by_surface(sid).expect("the toplevel serves");
        let applied = entry.machine.applied().expect("the insets applied");
        let frame = applied.insets.frame_around(bounds);
        shell::title_strip(
            &w.toplevels,
            sid,
            frame,
            applied.insets,
            ScaleFactor::IDENTITY,
        )
        .unwrap_or_else(|| panic!("the strip serves"))
    })
}

/// THE pixel oracle: the drawn title's exact words — the strip
/// rendered over the band, every pixel pinned.
#[test]
#[allow(clippy::too_many_lines)] // one session, one narrative
fn the_title_renders_as_ink() {
    let tb = Testbench::start_with("title-ink", title_config("ink"));
    let mut client = TestClient::connect(&tb.addr);
    let _w1 = ssd_window(&tb, &mut client, xrgb(200, 40, 40), 0);
    let _w2 = ssd_window(&tb, &mut client, xrgb(40, 80, 200), 1);
    let (w3, t3) = titled_window(&tb, &mut client, xrgb(30, 140, 150), 2, "Image Viewer");

    // The strip's truth, read from the world itself.
    let (skey, rect) = strip_truth(&tb, &w3);
    assert_eq!(&*skey.title, "Image Viewer");
    assert_eq!(skey.px, ldp_shell::ssd::TITLE_PX);

    // The oracle's expected bytes, from the face alone. The strip
    // rides over w2's content (the second window's 200x100 buffer at
    // (24,24) covers the whole strip's territory under w3's band).
    let (ew, _eh, ink) = expected_strip_words("Image Viewer");
    let underlay = opaque(xrgb(40, 80, 200));
    let words = tb.scanout();
    let band = band_word();
    for y in rect.y..rect.bottom() {
        for x in rect.x..rect.right() {
            let xi = (x - rect.x) as u32;
            let yi = (y - rect.y) as u32;
            let expect = strip_pixel_word(&ink, ew, xi.min(ew - 1), yi, underlay);
            assert_eq!(word(&words, x as usize, y as usize), expect, "at ({x},{y})");
        }
    }
    // The glyphs are not the band: the strip carries real ink.
    let mut inked = 0;
    for y in rect.y..rect.bottom() {
        for x in rect.x..rect.right() {
            if word(&words, x as usize, y as usize) != band {
                inked += 1;
            }
        }
    }
    assert!(inked > 20, "the title carries real ink ({inked} pixels)");
    // A pixel left of the strip (the margin) is the band alone over
    // its underlay.
    let band_over_w2 = u32::from_le_bytes(over_premul(
        band_layer(),
        opaque(xrgb(40, 80, 200)).to_le_bytes(),
        255,
    ));
    assert_eq!(
        word(&words, (rect.x - 2).max(0) as usize, (rect.y + 2) as usize),
        band_over_w2,
        "the margin stays the band"
    );
    let _ = t3;
}

/// THE truncation oracle: a title wider than the budget ends in the
/// ellipsis, the ink never reaching the close button's territory.
#[test]
fn the_long_title_truncates_with_the_ellipsis() {
    let tb = Testbench::start_with("title-trunc", title_config("trunc"));
    let mut client = TestClient::connect(&tb.addr);
    let long = "The Rise and Fall of the Lion Display Empire";
    let _w1 = map_plain(&tb, &mut client, xrgb(90, 30, 30), 150, 90);
    let _w2 = map_plain(&tb, &mut client, xrgb(30, 90, 30), 200, 100);
    let (w3, _t3) = titled_window(&tb, &mut client, xrgb(30, 140, 150), 2, long);

    // The run the strip drew: truncated, ending in the ellipsis.
    let avail = 202 - TITLE_SIDE - (CLOSE_SIZE + 2 * CLOSE_MARGIN);
    let run = LION_SANS.layout(long, ldp_shell::ssd::TITLE_PX, avail);
    assert!(run.truncated, "the 45-char title truncates at {avail} px");
    assert!(
        run.placed.last().is_some_and(|p| p.c == '\u{2026}'),
        "the truncated run ends in the ellipsis"
    );
    assert!(run.width <= avail + 24, "the run respects the budget");

    // The strip serves with the truncated key.
    let (skey, rect) = strip_truth(&tb, &w3);
    assert_eq!(skey.avail, avail);
    assert!(rect.right() <= 24 * 2 - 1 + 202 - (CLOSE_SIZE + CLOSE_MARGIN) as i32);

    // The truncated strip's own oracle, exact — the strip rides over
    // the first plain window's content (its 150x90 buffer covers the
    // whole strip territory).
    let (ew, _eh, ink) = expected_strip_words(long);
    let underlay = opaque(xrgb(30, 90, 30));
    let band_over = u32::from_le_bytes(over_premul(
        band_layer(),
        opaque(xrgb(30, 90, 30)).to_le_bytes(),
        255,
    ));
    let words = tb.scanout();
    for y in rect.y..rect.bottom() {
        for x in rect.x..rect.right() {
            let xi = (x - rect.x) as u32;
            let yi = (y - rect.y) as u32;
            let expect = strip_pixel_word(&ink, ew, xi.min(ew - 1), yi, underlay);
            assert_eq!(word(&words, x as usize, y as usize), expect, "at ({x},{y})");
        }
    }
    // The band just left of the button's territory: the title's ink
    // stops before it (band or title — never ink past the budget).
    let stop_x = 24 * 2 - 1 + 202 - (CLOSE_SIZE + 2 * CLOSE_MARGIN) as i32;
    for y in rect.y..rect.bottom() {
        let w = word(&words, stop_x as usize, y as usize);
        assert_eq!(
            w, band_over,
            "no title ink in the button's margin at ({stop_x},{y})"
        );
    }
    // The strip carries ink (the truncated run draws).
    let inked = (rect.x..rect.right())
        .map(|x| word(&words, x as usize, (rect.y + rect.h as i32 / 2) as usize))
        .filter(|&w| w != band_over)
        .count();
    assert!(inked > 10, "the truncated title inks ({inked} columns)");
}

/// THE title-change oracle: `set_title` on a serving window repaints
/// the strip — no client commit, the server's own claim.
#[test]
fn the_title_change_claims_and_renders() {
    let tb = Testbench::start_with("title-change", title_config("change"));
    let mut client = TestClient::connect(&tb.addr);
    let _w1 = map_plain(&tb, &mut client, xrgb(90, 30, 30), 150, 90);
    let _w2 = map_plain(&tb, &mut client, xrgb(30, 90, 30), 200, 100);
    let (w3, t3) = titled_window(&tb, &mut client, xrgb(30, 140, 150), 2, "First");

    // The first strip inks.
    let rect1 = strip_truth(&tb, &w3).1;
    let first_words = tb.scanout();
    // Sample inside the 'F's top bar (the stem is one pixel wide at
    // this size — the bar carries the proof).
    let first_sample = word(&first_words, (rect1.x + 4) as usize, (rect1.y + 2) as usize);
    let band_over = u32::from_le_bytes(over_premul(
        band_layer(),
        opaque(xrgb(30, 90, 30)).to_le_bytes(),
        255,
    ));
    assert_ne!(first_sample, band_over, "the first title inks");

    // The change: set_title with no commit — the server's claim.
    let frames_before = tb.frames();
    client
        .conn
        .send_request(&t3, "set_title", vec![Value::String(Box::from("Second"))])
        .expect("set_title");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while tb.frames() <= frames_before {
        assert!(
            deadline > std::time::Instant::now(),
            "the change never rendered"
        );
        client.sync();
    }
    // The ledger carries the new title.
    let (skey2, rect2) = strip_truth(&tb, &w3);
    assert_eq!(&*skey2.title, "Second");

    // The pixels: the second title's own oracle, exact — over the
    // first plain window's content (the strip's whole territory).
    let (ew, _eh, ink) = expected_strip_words("Second");
    let underlay = opaque(xrgb(30, 90, 30));
    let band_over = u32::from_le_bytes(over_premul(
        band_layer(),
        opaque(xrgb(30, 90, 30)).to_le_bytes(),
        255,
    ));
    let words = tb.scanout();
    for y in rect2.y..rect2.bottom() {
        for x in rect2.x..rect2.right() {
            let xi = (x - rect2.x) as u32;
            let yi = (y - rect2.y) as u32;
            let expect = strip_pixel_word(&ink, ew, xi.min(ew - 1), yi, underlay);
            assert_eq!(word(&words, x as usize, y as usize), expect, "at ({x},{y})");
        }
    }
    // The old ink's tail (beyond the shorter title) is gone: the
    // band's word returns where 'First' used to ink.
    if rect2.right() < rect1.right() {
        let x = (rect1.right() + rect2.right()) / 2;
        for y in rect1.y..rect1.bottom() {
            assert_eq!(
                word(&words, x as usize, y as usize),
                band_over,
                "the old ink's tail cleared"
            );
        }
    }
}

/// The empty-title doctrine: no set_title, no strip — the scanout
/// differs from a titled window's by the strip's ink alone.
#[test]
fn the_empty_title_draws_no_strip() {
    let tb = Testbench::start_with("title-empty", title_config("empty"));
    let mut client = TestClient::connect(&tb.addr);
    let _f1 = map_plain(&tb, &mut client, xrgb(90, 30, 30), 150, 90);
    let _f2 = map_plain(&tb, &mut client, xrgb(30, 90, 30), 200, 100);
    let (w_plain, _t_plain) = ssd_window(&tb, &mut client, xrgb(30, 140, 150), 2);
    let plain = tb.scanout();
    let strips = tb.world(|w| w.chrome.strip_count());
    assert_eq!(strips, 0, "the empty title mints no strip");
    let _ = w_plain;

    // A titled window in a fresh session: the scanout differs.
    let tb2 = Testbench::start_with("title-empty-2", title_config("empty2"));
    let mut client2 = TestClient::connect(&tb2.addr);
    let _f1 = map_plain(&tb2, &mut client2, xrgb(90, 30, 30), 150, 90);
    let _f2 = map_plain(&tb2, &mut client2, xrgb(30, 90, 30), 200, 100);
    let (_w_titled, _t_titled) = titled_window(&tb2, &mut client2, xrgb(30, 140, 150), 2, "Title");
    let titled = tb2.scanout();
    assert_eq!(plain.len(), titled.len());
    assert_ne!(plain, titled, "the strip's ink separates them");
}

/// THE territory: CSD windows and fullscreen covers draw no title.
#[test]
#[allow(clippy::too_many_lines)] // one session, one narrative
fn csd_and_fullscreen_draw_no_title() {
    let tb = Testbench::start_with("title-csd", title_config("csd"));
    let mut client = TestClient::connect(&tb.addr);
    let shm = client.bind("ldp.core.shm");
    let compositor = client.bind("ldp.core.compositor");
    let shell_proxy = client.bind("ldp.shell.shell");

    // The CSD window: client decorations, a title that never draws.
    let surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("surface");
    let toplevel = client
        .conn
        .create_object(
            &shell_proxy,
            "get_toplevel",
            vec![Value::Object(Some(surface.id())), Value::Enum(2)],
        )
        .expect("get_toplevel (CSD)");
    client
        .conn
        .send_request(
            &toplevel,
            "set_title",
            vec![Value::String(Box::from("CSD"))],
        )
        .expect("set_title");
    client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "configure" && r.target == toplevel.id().as_u32())
    });
    let handshake = last_on(&client, "configure", toplevel.id().as_u32());
    client
        .conn
        .send_request(
            &toplevel,
            "ack_configure",
            vec![Value::Uint32(serial_of(handshake))],
        )
        .expect("ack");
    let (w, h) = (200usize, 100usize);
    let pixels: Vec<u8> = std::iter::repeat(xrgb(90, 160, 60).to_le_bytes())
        .take(w * h)
        .flatten()
        .collect();
    let pool = create_pool(&mut client, &shm, pool_bytes(&pixels), (w * h * 4) as i64);
    let buffer = create_buffer(
        &mut client,
        &pool,
        0,
        w as i32,
        h as i32,
        (w * 4) as i32,
        XR24,
    );
    let before = tb.frames();
    attach(&mut client, &surface, &buffer);
    damage(
        &mut client,
        &surface,
        &[Rect::new(0, 0, w as u32, h as u32)],
    );
    commit(&mut client, &surface, 1);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while tb.frames() <= before {
        assert!(deadline > std::time::Instant::now(), "never rendered");
        client.sync();
    }
    let words = tb.scanout();
    for x in [5, 100, 195] {
        assert_eq!(
            word(&words, x, 5),
            opaque(xrgb(90, 160, 60)),
            "the CSD top row is the client's"
        );
    }
    assert_eq!(tb.world(|w| w.chrome.strip_count()), 0, "no CSD strip");

    // Fullscreen: zero insets cover everything — the SSD window's
    // title goes band-less with the chrome.
    let (w3, t3) = titled_window(&tb, &mut client, xrgb(30, 140, 150), 2, "Full");
    client
        .conn
        .send_request(&t3, "fullscreen", vec![Value::Object(None)])
        .expect("fullscreen");
    client.wait_until(|c| {
        c.records
            .iter()
            .filter(|r| r.event == "configure" && r.target == t3.id().as_u32())
            .count()
            >= 2
    });
    let proposal = last_on(&client, "configure", t3.id().as_u32());
    assert_eq!(proposal.args[4], Value::Int32(0), "fullscreen's insets");
    client
        .conn
        .send_request(
            &t3,
            "ack_configure",
            vec![Value::Uint32(serial_of(proposal))],
        )
        .expect("ack");
    let px: Vec<u8> = std::iter::repeat(xrgb(30, 140, 150).to_le_bytes())
        .take(1920 * 1080)
        .flatten()
        .collect();
    let pool = create_pool(&mut client, &shm, pool_bytes(&px), (1920 * 1080 * 4) as i64);
    let buffer = create_buffer(&mut client, &pool, 0, 1920, 1080, 7680, XR24);
    let before = tb.frames();
    attach(&mut client, &w3, &buffer);
    damage(&mut client, &w3, &[Rect::new(0, 0, 1920, 1080)]);
    commit(&mut client, &w3, 1);
    while tb.frames() <= before {
        assert!(deadline > std::time::Instant::now(), "never rendered");
        client.sync();
    }
    let words = tb.scanout();
    assert_eq!(word(&words, 960, 5), opaque(xrgb(30, 140, 150)));
    assert_eq!(word(&words, 100, 26), opaque(xrgb(30, 140, 150)));
}

/// THE sharing proof: same-shaped windows share the base raster,
/// their distinct titles ride distinct strips.
#[test]
fn same_shape_different_titles_share_the_band_not_the_ink() {
    let tb = Testbench::start_with("title-share", title_config("share"));
    let mut client = TestClient::connect(&tb.addr);
    let _f1 = map_plain(&tb, &mut client, xrgb(90, 30, 30), 150, 90);
    let (w1, _t1) = titled_window(&tb, &mut client, xrgb(200, 40, 40), 2, "Alpha");
    let (w2, _t2) = titled_window(&tb, &mut client, xrgb(40, 80, 200), 3, "Beta");

    let (rasters, strips) = tb.world(|w| (w.chrome.raster_count(), w.chrome.strip_count()));
    assert_eq!(rasters, 1, "same-shaped windows share one base raster");
    assert_eq!(strips, 2, "distinct titles ride distinct strips");

    // Both titles ink on screen — each window's own strip rows, read
    // from the world's rects (Phase 56: each strip rides its own
    // window's band; the non-ink words there are the band over the
    // desktop, over the filler's red, or over the first window's
    // content — any other word is glyph ink).
    let words = tb.scanout();
    let band_over_filler = u32::from_le_bytes(over_premul(
        band_layer(),
        opaque(xrgb(90, 30, 30)).to_le_bytes(),
        255,
    ));
    let band_over_alpha = u32::from_le_bytes(over_premul(
        band_layer(),
        opaque(xrgb(200, 40, 40)).to_le_bytes(),
        255,
    ));
    let ink_in = |rect: Rect| {
        (rect.y..rect.bottom())
            .flat_map(|y| (rect.x..rect.x + 60).map(move |x| (x, y)))
            .any(|(x, y)| {
                let w = word(&words, x as usize, y as usize);
                w != band_over_filler && w != band_over_alpha && w != band_word()
            })
    };
    let (_, r1) = strip_truth(&tb, &w1);
    let (_, r2) = strip_truth(&tb, &w2);
    assert!(ink_in(r1), "Alpha inks");
    assert!(ink_in(r2), "Beta inks");
}
