//! Phase 52's exit criterion (the drawn chrome): the SSD band served
//! end to end through the real protocol —
//!
//! * the **drawn band**: a server-decorated window's chrome renders
//!   around its content (the title bar, the border ring, and the
//!   close affordance — CPU ink on the dock's own model, exact words
//!   pinned against the palette the painter owns), and the band
//!   leaves with the window (the claims ledger repaints what the
//!   hidden window's chrome covered);
//! * the **close ask**: the drawn button's press arms and its
//!   release *fires* — the frozen `toplevel.close` event, sent for
//!   the first time in the protocol's life, over the real socket,
//!   parked in the outbox exactly like every routed event; the
//!   drag-away cancels it (the caption doctrine every desktop
//!   serves);
//! * the **honest protocol**: the client that ignores the close
//!   keeps its window (the server never force-kills), the press the
//!   band consumes never reaches the client, and the band's press
//!   focuses its own window (the activated bit riding the proposal
//!   the focus truth serves);
//! * the **territory**: CSD windows draw their own chrome (their
//!   buffer's top is their title bar — presses there route to them),
//!   and fullscreen covers everything (zero insets, no band, the
//!   former band's presses route as content);
//! * the **geometry regime**: a maximized SSD window's *frame* fills
//!   the workspace area (the content inset by the chrome — the band
//!   on-screen at every state).

mod testbench;

use ldp_client::Proxy;
use ldp_core::geometry::Rect;
use ldp_core::wire::Value;
use ldp_input::device::DeviceClass;
use ldp_input::normalizer::InputEvent;
use testbench::*;

use ldp_compositor::surface::Surface;
use lion_compositor::server::CompositorConfig;
use lion_compositor::shell::{
    DockMode, ShellConfig, BAND_ALPHA, BAND_RGB, CLOSE_ALPHA, CLOSE_RGB, GLYPH_ALPHA, GLYPH_RGB,
    RING_ALPHA, RING_RGB,
};

/// The XRGB8888 fourcc ("XR24").
const XR24: u32 = 0x3432_5258;
/// BTN_LEFT (the evdev button code).
const BTN_LEFT: u32 = 0x110;
/// The mock output: 1920x1080@60.
const OUT_W: usize = 1920;

/// One XRGB word.
fn xrgb(r: u8, g: u8, b: u8) -> u32 {
    u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b)
}

/// One scanout pixel as a raw word (the premultiplied truth the
/// painter's words land in, uninterpreted).
fn word(words: &[u32], x: usize, y: usize) -> u32 {
    words[y * OUT_W + x]
}

/// One scanout word for an opaque straight color (the XRGB fill's
/// landing — the scanout holds premultiplied words; an opaque
/// source's is its straight color with the full alpha).
fn opaque(v: u32) -> u32 {
    0xFF00_0000 | (v & 0x00FF_FFFF)
}

/// The band's scanout word (the near-opaque bar over the
/// opaque-black desktop: the alpha saturates full, the RGB is the
/// bar's own premultiplied triple — the exact arithmetic the
/// renderer's over-blend performs).
fn band_word() -> u32 {
    let m = |v: u8| (u32::from(v) * u32::from(BAND_ALPHA) + 127) / 255;
    0xFF00_0000 | (m(BAND_RGB[0]) << 16) | (m(BAND_RGB[1]) << 8) | m(BAND_RGB[2])
}

/// One premultiplied word from an opaque RGB triple (the ring, the
/// capsule, the glyph — their alpha is full, the word is exact over
/// any background).
fn prem(rgb: [u8; 3], a: u8) -> u32 {
    let m = |v: u8| (u32::from(v) * u32::from(a) + 127) / 255;
    (u32::from(a) << 24) | (m(rgb[0]) << 16) | (m(rgb[1]) << 8) | m(rgb[2])
}

/// Wait for one event of one name on one object.
fn wait_for_on(client: &mut TestClient, name: &str, target: u32) {
    client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == name && r.target == target)
    });
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

/// The serial of a recorded configure (arg 0).
fn serial_of(record: &Recorded) -> u32 {
    match &record.args[0] {
        Value::Uint32(s) => *s,
        other => panic!("configure's serial is not a uint: {other:?}"),
    }
}

/// The states bitset of a recorded configure (arg 1).
fn states_of(record: &Recorded) -> ldp_core::bitset::Bitset128 {
    match &record.args[1] {
        Value::Bitset(bits) => *bits,
        other => panic!("configure's states argument is not a bitset: {other:?}"),
    }
}

/// A memfd carrying `pixels` (the pool backing).
fn pool_bytes(pixels: &[u8]) -> std::os::fd::OwnedFd {
    use std::io::Write as _;
    let fd = lion_compositor::sys::memfd("chrome-pool").expect("memfd");
    {
        let mut file = std::fs::File::from(fd.try_clone().expect("clone"));
        file.write_all(pixels).expect("write pool");
    }
    fd
}

/// The chrome tests' configs: dock auto (the cascade's arithmetic
/// the band geometry rides — the dock itself stays at the bottom,
/// far from every window the tests place).
fn chrome_config(tag: &str) -> CompositorConfig {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    CompositorConfig {
        shell: ShellConfig {
            dock: DockMode::Auto,
            ..ShellConfig::default()
        },
        socket: format!("lion-chrome-{tag}-{n}-{}", std::process::id()),
        ..CompositorConfig::default()
    }
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

/// `shell.get_toplevel(surface, server-decorations)` — the SSD mint.
fn get_ssd_toplevel(client: &mut TestClient, shell: &Proxy, surface: &Proxy) -> Proxy {
    client
        .conn
        .create_object(
            shell,
            "get_toplevel",
            vec![Value::Object(Some(surface.id())), Value::Enum(1)],
        )
        .expect("get_toplevel")
}

/// One SSD window realized: the toplevel minted *before* the mapping
/// commit (the well-behaved client's order), the handshake acked, the
/// first commit mapping the surface at the cascade's `nth` step with
/// the insets already applied (the band serving from its first
/// frame). Returns (surface, toplevel). The window is 200x100 with
/// `fill`.
fn ssd_window(tb: &Testbench, client: &mut TestClient, fill: u32, nth: usize) -> (Proxy, Proxy) {
    let shell = client.bind("ldp.shell.shell");
    let shm = client.bind("ldp.core.shm");
    let compositor = client.bind("ldp.core.compositor");
    let surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("surface");
    let toplevel = get_ssd_toplevel(client, &shell, &surface);
    // The handshake: serial 1, the SSD insets (the Lion metrics at
    // identity scale — 1 / 29 / 1 / 1).
    wait_for_on(client, "configure", toplevel.id().as_u32());
    let handshake = last_on(client, "configure", toplevel.id().as_u32());
    assert_eq!(serial_of(handshake), 1);
    assert_eq!(handshake.args[4], Value::Int32(1), "the left border");
    assert_eq!(handshake.args[5], Value::Int32(29), "the title band");
    assert_eq!(handshake.args[6], Value::Int32(1), "the right border");
    assert_eq!(handshake.args[7], Value::Int32(1), "the bottom border");
    // The ack, then the mapping commit (the applied insets land with
    // it — the chrome serves from this first frame).
    client
        .conn
        .send_request(&toplevel, "ack_configure", vec![Value::Uint32(1)])
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
    // The cascade's `nth` step (24 px per step, the first at the
    // origin): the third window's frame sits fully on-screen.
    // Phase 56 — the chrome-aware placement: the frame takes the
    // slot; the content rides inside at the slot + insets (the band
    // on-screen from the first frame).
    let step = 24 * nth as i32;
    assert_eq!(position_of(tb, &surface), (step + 1, step + 29));
    (surface, toplevel)
}

/// The world's position of one surface (the tree's own truth).
fn position_of(tb: &Testbench, surface: &Proxy) -> (i32, i32) {
    tb.world(|w| {
        for (sid, route) in &w.scene.routes {
            if route.surface_obj.as_u32() == surface.id().as_u32() {
                return w.scene.tree.get(*sid).map_or((0, 0), Surface::position);
            }
        }
        panic!("the surface has no route");
    })
}

/// Whether a surface's toplevel entry still serves (the role's own
/// life, the chrome's truth).
fn role_serves(tb: &Testbench, surface: &Proxy) -> bool {
    tb.world(|w| {
        w.scene
            .routes
            .iter()
            .find(|(_, route)| route.surface_obj.as_u32() == surface.id().as_u32())
            .is_some_and(|(sid, _)| w.toplevels.by_surface(*sid).is_some())
    })
}

/// One scripted input batch through the same pump the real devices
/// take (the chrome grip's own heartbeat).
fn batch(tb: &Testbench, events: Vec<InputEvent>) {
    tb.world_mut(|w| {
        w.queue_input(DeviceClass::Mouse, events);
        w.pump_input();
    });
}

/// One button press/release.
fn button(tb: &Testbench, pressed: bool) {
    batch(
        tb,
        vec![InputEvent::Button {
            button: BTN_LEFT,
            pressed,
        }],
    );
}

/// One pointer motion by `(dx, dy)`.
fn motion(tb: &Testbench, dx: i32, dy: i32) {
    batch(tb, vec![InputEvent::PointerMotion { dx, dy }]);
}

/// Drive the pointer to an absolute position in accelerator-neutral
/// steps (each batch under the curve's 200-count threshold — the
/// pointer lands exactly where the arithmetic says; the drag
/// suite's own hard lesson, respected here).
fn move_to(tb: &Testbench, x: i32, y: i32) {
    loop {
        let (px, py) = tb.world(lion_compositor::World::input_pointer_position);
        let (dx, dy) = (x - px as i32, y - py as i32);
        if dx == 0 && dy == 0 {
            return;
        }
        // 100 per axis keeps the batch's magnitude at 141 — under
        // the threshold, the gain exactly 1.
        motion(tb, dx.clamp(-100, 100), dy.clamp(-100, 100));
    }
}

/// The pointer device's mint (the wake contract's own recipe — the
/// routed events the press and motions deliver).
fn mint_pointer(client: &mut TestClient) -> Proxy {
    let seat = client.bind("ldp.input.seat");
    client
        .conn
        .create_object(&seat, "get_pointer", vec![])
        .expect("pointer")
}

/// The client's recorded count of one event on one object.
fn count_on(client: &TestClient, name: &str, target: u32) -> usize {
    client
        .events
        .records
        .iter()
        .filter(|r| r.event == name && r.target == target)
        .count()
}

/// Wait until `n` button events have arrived on one pointer object
/// (the wake-point doctrine on the client side: the events ride the
/// wake that follows the message — the roundtrip that *answers* a
/// sync may stop reading before them, the next one always collects;
/// the count discipline never fires on an earlier arrival).
fn wait_for_buttons(client: &mut TestClient, pointer: u32, n: usize) {
    client.wait_until(|c| {
        c.records
            .iter()
            .filter(|r| r.event == "button" && r.target == pointer)
            .count()
            >= n
    });
}

/// THE close narrative: the drawn button's press arms (consumed — no
/// client button event, the window focused and activated), its
/// release fires the frozen `toplevel.close` over the real socket
/// (the first sender the event has ever had), and the client that
/// answers by destroying the toplevel leaves a clean desktop (the
/// entry gone, the surface free to live on roleless).
#[test]
#[allow(clippy::too_many_lines)] // one session, one narrative
fn the_drawn_close_button_asks_and_the_client_answers() {
    let tb = Testbench::start_with("chrome-close", chrome_config("close"));
    let mut client = TestClient::connect(&tb.addr);
    let pointer = mint_pointer(&mut client);
    let _ = &pointer;

    // The cascade: two fillers, the third carries the SSD band (its
    // frame fully on-screen at the third step).
    let _w1 = map_plain(&tb, &mut client, xrgb(200, 40, 40), 150, 90);
    let _w2 = map_plain(&tb, &mut client, xrgb(40, 80, 200), 200, 100);
    let (w3, t3) = ssd_window(&tb, &mut client, xrgb(30, 140, 150), 2);

    // The close button: the frame (48, 48, 202, 131) carries it at
    // (225, 53, 20, 20) — the center (235, 63) is over no content
    // (the band is nobody's ink but the chrome's).
    let (cx, cy) = (235i32, 63i32);
    move_to(&tb, cx, cy);
    // The press: consumed (the client's pointer never learns it), the
    // window focused (the activation bit riding its proposal).
    let before = count_on(&client, "configure", t3.id().as_u32());
    button(&tb, true);
    client.sync();
    wait_for_on_with(&mut client, "configure", t3.id().as_u32(), before + 1);
    assert!(
        states_of(last_on(&client, "configure", t3.id().as_u32())).test(3),
        "the band's press activated the window (bit 3)"
    );
    assert_eq!(
        count_on(&client, "button", pointer.id().as_u32()),
        0,
        "the band's press never reaches the client"
    );
    assert_eq!(
        count_on(&client, "close", t3.id().as_u32()),
        0,
        "the press arms, it never fires alone"
    );

    // The release: the ask fires — the frozen `close` event, over the
    // real socket, parked in the outbox exactly like every routed
    // event (the wake contract's own vehicle).
    button(&tb, false);
    client.sync();
    wait_for_on(&mut client, "close", t3.id().as_u32());
    let close = last_on(&client, "close", t3.id().as_u32());
    assert_eq!(close.interface, "ldp.shell.toplevel");
    assert!(close.args.is_empty(), "the close carries no arguments");
    assert_eq!(
        count_on(&client, "close", t3.id().as_u32()),
        1,
        "one ask, exactly"
    );
    // The release was consumed too (the whole click is the server's).
    assert_eq!(count_on(&client, "button", pointer.id().as_u32()), 0);

    // The client answers: the toplevel object dies (the surface lives
    // on roleless — the spec's "when ready" is the client's own
    // truth). The entry leaves, the band leaves with the role.
    client.conn.destroy(&t3).expect("destroy");
    for _ in 0..3 {
        client.sync();
    }
    assert!(
        !role_serves(&tb, &w3),
        "the role object's death dropped the entry"
    );
    // The surface's own ink remains (it outlived its role).
    let words = tb.scanout();
    assert_eq!(
        word(&words, 101, 100),
        opaque(xrgb(30, 140, 150)),
        "the content stays"
    );
}

/// The drag-away cancel: a press on the button, a motion out of the
/// band, a release — nothing fires (the caption doctrine: the click
/// that leaves never happened), and the next true click fires.
#[test]
fn the_drag_away_cancels_the_close_ask() {
    let tb = Testbench::start_with("chrome-cancel", chrome_config("cancel"));
    let mut client = TestClient::connect(&tb.addr);
    mint_pointer(&mut client);

    let _w1 = map_plain(&tb, &mut client, xrgb(200, 40, 40), 150, 90);
    let _w2 = map_plain(&tb, &mut client, xrgb(40, 80, 200), 200, 100);
    let (_w3, t3) = ssd_window(&tb, &mut client, xrgb(30, 140, 150), 2);

    // Armed on the button...
    move_to(&tb, 235, 63);
    button(&tb, true);
    // ...dragged away (out of the band, onto the window's own
    // content)...
    move_to(&tb, 200, 100);
    button(&tb, false);
    client.sync();
    assert_eq!(
        count_on(&client, "close", t3.id().as_u32()),
        0,
        "the click that left never fired"
    );

    // The next true click fires (the grip re-arms — a cancelled ask
    // never poisons the affordance).
    move_to(&tb, 235, 63);
    button(&tb, true);
    button(&tb, false);
    client.sync();
    wait_for_on(&mut client, "close", t3.id().as_u32());
    assert_eq!(count_on(&client, "close", t3.id().as_u32()), 1);
}

/// The honest protocol: the client that ignores the close keeps its
/// window (the server never force-kills — the frozen event's own doc:
/// "the client destroys the toplevel when ready"), and the desktop
/// keeps working with it (a content press routes normally).
#[test]
fn the_client_that_ignores_the_close_keeps_its_window() {
    let tb = Testbench::start_with("chrome-ignore", chrome_config("ignore"));
    let mut client = TestClient::connect(&tb.addr);
    let pointer = mint_pointer(&mut client);

    let _w1 = map_plain(&tb, &mut client, xrgb(200, 40, 40), 150, 90);
    let _w2 = map_plain(&tb, &mut client, xrgb(40, 80, 200), 200, 100);
    let (w3, t3) = ssd_window(&tb, &mut client, xrgb(30, 140, 150), 2);

    // The ask fires.
    move_to(&tb, 235, 63);
    button(&tb, true);
    button(&tb, false);
    client.sync();
    wait_for_on(&mut client, "close", t3.id().as_u32());

    // The client does nothing. The window stays (mapped, routed, its
    // chrome still serving) — "when ready" is the client's truth.
    assert!(role_serves(&tb, &w3));
    let words = tb.scanout();
    assert_eq!(word(&words, 101, 100), opaque(xrgb(30, 140, 150)));
    assert_eq!(word(&words, 224, 51), band_word());

    // The desktop keeps working: a press on the *content* routes
    // normally (the client's own ink — delivered, no zombie).
    move_to(&tb, 101, 100);
    button(&tb, true);
    wait_for_buttons(&mut client, pointer.id().as_u32(), 1);
    button(&tb, false);
}

/// The title band's press (away from the affordance): consumed
/// without delivery, the window focused (its activated bit), and no
/// ask ever fires — the band is the operator's, not the client's.
#[test]
fn the_title_band_press_focuses_without_delivery() {
    let tb = Testbench::start_with("chrome-band", chrome_config("band"));
    let mut client = TestClient::connect(&tb.addr);
    let pointer = mint_pointer(&mut client);

    let _w1 = map_plain(&tb, &mut client, xrgb(200, 40, 40), 150, 90);
    let _w2 = map_plain(&tb, &mut client, xrgb(40, 80, 200), 200, 100);
    let (_w3, t3) = ssd_window(&tb, &mut client, xrgb(30, 140, 150), 2);

    // A press in the band's middle (over the second window's content
    // underneath — the chrome is on top, the chrome claims it).
    move_to(&tb, 151, 51);
    let before = count_on(&client, "configure", t3.id().as_u32());
    button(&tb, true);
    client.sync();
    wait_for_on_with(&mut client, "configure", t3.id().as_u32(), before + 1);
    assert!(
        states_of(last_on(&client, "configure", t3.id().as_u32())).test(3),
        "the band's press focused its own window"
    );
    assert_eq!(
        count_on(&client, "button", pointer.id().as_u32()),
        0,
        "the band's press never delivers"
    );
    button(&tb, false);
    client.sync();
    assert_eq!(
        count_on(&client, "close", t3.id().as_u32()),
        0,
        "a band press away from the affordance never asks"
    );
}

/// The territory's first truth: CSD windows draw their own chrome —
/// the top of their buffer is their title bar, and presses there
/// route to them (the server's band never claims a client's ink).
#[test]
fn csd_windows_have_no_server_chrome() {
    let tb = Testbench::start_with("chrome-csd", chrome_config("csd"));
    let mut client = TestClient::connect(&tb.addr);
    let pointer = mint_pointer(&mut client);

    // One CSD window at the cascade's origin (its buffer starts at
    // (0,0) — the top of its ink is its own chrome).
    let shell = client.bind("ldp.shell.shell");
    let w1 = map_plain(&tb, &mut client, xrgb(90, 160, 60), 200, 100);
    client
        .conn
        .create_object(
            &shell,
            "get_toplevel",
            vec![Value::Object(Some(w1.id())), Value::Enum(2)],
        )
        .expect("get_toplevel");
    client.sync();

    // A press at the top of the CSD buffer (where a server band would
    // be — there is none): delivered to the client, never consumed.
    move_to(&tb, 100, 5);
    button(&tb, true);
    wait_for_buttons(&mut client, pointer.id().as_u32(), 1);
    button(&tb, false);
    // The pixel truth: no band ink anywhere around the buffer.
    let words = tb.scanout();
    assert_eq!(
        word(&words, 100, 5),
        opaque(xrgb(90, 160, 60)),
        "the client's own top"
    );
}

/// The territory's second truth: fullscreen covers everything (the
/// applied insets are zero, no band serves), and the former band's
/// presses route as content.
#[test]
fn fullscreen_covers_the_chrome() {
    let tb = Testbench::start_with("chrome-full", chrome_config("full"));
    let mut client = TestClient::connect(&tb.addr);
    let pointer = mint_pointer(&mut client);

    let _w1 = map_plain(&tb, &mut client, xrgb(200, 40, 40), 150, 90);
    let _w2 = map_plain(&tb, &mut client, xrgb(40, 80, 200), 200, 100);
    let (w3, t3) = ssd_window(&tb, &mut client, xrgb(30, 140, 150), 2);

    // Fullscreen: the proposal covers the output with zero insets.
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
    let serial = serial_of(proposal);
    assert_eq!(proposal.args[4], Value::Int32(0), "fullscreen's insets");
    assert_eq!(proposal.args[5], Value::Int32(0));

    // The realizing commit: the full-output buffer.
    let shm = client.bind("ldp.core.shm");
    let pixels: Vec<u8> = std::iter::repeat(xrgb(30, 140, 150).to_le_bytes())
        .take(1920 * 1080)
        .flatten()
        .collect();
    let pool = create_pool(
        &mut client,
        &shm,
        pool_bytes(&pixels),
        (1920 * 1080 * 4) as i64,
    );
    let buffer = create_buffer(&mut client, &pool, 0, 1920, 1080, 1920 * 4, XR24);
    let before = tb.frames();
    client
        .conn
        .send_request(&t3, "ack_configure", vec![Value::Uint32(serial)])
        .expect("ack");
    attach(&mut client, &w3, &buffer);
    damage(&mut client, &w3, &[Rect::new(0, 0, 1920, 1080)]);
    commit(&mut client, &w3, 2);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while tb.frames() <= before {
        assert!(deadline > std::time::Instant::now(), "never rendered");
        client.sync();
    }

    // The former band position is content now (delivered — no chrome
    // claim, no ask).
    move_to(&tb, 234, 34);
    button(&tb, true);
    wait_for_buttons(&mut client, pointer.id().as_u32(), 1);
    assert_eq!(
        count_on(&client, "button", pointer.id().as_u32()),
        1,
        "the covered band routes as content"
    );
    assert_eq!(count_on(&client, "close", t3.id().as_u32()), 0);
    button(&tb, false);
    // The pixel truth: the cover owns the former band's row.
    let words = tb.scanout();
    assert_eq!(word(&words, 150, 22), opaque(xrgb(30, 140, 150)));
}

/// THE pixel oracle: the drawn band's exact words (the palette the
/// painter and the tests share), the content hole transparent under
/// the client's ink, the borders crisp — and the claims ledger's own
/// truth (the minimize hides the chrome with the ink, the unminimize
/// restores the exact bytes).
#[test]
#[allow(clippy::too_many_lines)] // one session, one narrative
fn the_chrome_ink_renders_around_the_content() {
    let tb = Testbench::start_with("chrome-ink", chrome_config("ink"));
    let mut client = TestClient::connect(&tb.addr);

    let _w1 = map_plain(&tb, &mut client, xrgb(200, 40, 40), 150, 90);
    let _w2 = map_plain(&tb, &mut client, xrgb(40, 80, 200), 200, 100);
    let (w3, t3) = ssd_window(&tb, &mut client, xrgb(30, 140, 150), 2);

    // The frame: (48, 48) to (250, 178). The title band spans y
    // [48, 77) — the rows sit over the second window's content for
    // x < 224 and over the black desktop beyond it (the band's
    // near-opaque bar over either underlay is its own premultiplied
    // word; the black-underlay points are pinned here). The borders
    // and the button are fully opaque — their words are exact
    // wherever they land.
    let words = tb.scanout();
    let band = band_word();
    let ring = prem(RING_RGB, RING_ALPHA);
    let close = prem(CLOSE_RGB, CLOSE_ALPHA);
    let glyph = prem(GLYPH_RGB, GLYPH_ALPHA);
    // The title band (over the black desktop, exact — right of the
    // second window, left of the ring).
    assert_eq!(word(&words, 224, 51), band, "the band over the desktop");
    assert_eq!(word(&words, 248, 51), band, "the band's right reach");
    // The border ring (opaque — exact over anything).
    assert_eq!(word(&words, 48, 80), ring, "the left border");
    assert_eq!(word(&words, 249, 80), ring, "the right border");
    assert_eq!(word(&words, 151, 177), ring, "the bottom border");
    // The close affordance: capsule and glyph (the capsule's point
    // sits off both diagonal strokes, inside the circle).
    assert_eq!(word(&words, 231, 55), close, "the capsule's body");
    assert_eq!(word(&words, 235, 63), glyph, "the × at the center");
    // The content hole: the client's ink, untouched.
    assert_eq!(
        word(&words, 101, 100),
        opaque(xrgb(30, 140, 150)),
        "the content"
    );
    assert_eq!(
        word(&words, 201, 89),
        opaque(xrgb(30, 140, 150)),
        "the content's edge"
    );
    // Beyond the frame: no chrome claim (the second window's own
    // ink stands just left of the frame, unclaimed).
    assert_eq!(
        word(&words, 47, 51),
        opaque(xrgb(40, 80, 200)),
        "left of the frame is the second window's ink"
    );

    // The claims ledger: minimize hides the chrome with the ink (the
    // band's rect repaints — the desktop's black returns under it).
    // The wake that pumps the minimize's proposal renders the vacate
    // claims in the same breath (the states arm's own doctrine —
    // wait for the flag, read the canvas).
    client
        .conn
        .send_request(&t3, "minimize", vec![])
        .expect("minimize");
    wait_for_on_with(&mut client, "configure", t3.id().as_u32(), 2);
    assert!(
        states_of(last_on(&client, "configure", t3.id().as_u32())).test(2),
        "the minimized flag"
    );
    for _ in 0..3 {
        client.sync();
    }
    let words = tb.scanout();
    assert_eq!(
        word(&words, 224, 51),
        0xFF00_0000,
        "the hidden chrome left no ink"
    );
    assert_eq!(
        word(&words, 48, 80),
        opaque(xrgb(40, 80, 200)),
        "the ring's rect reverted"
    );
    assert_eq!(
        word(&words, 101, 100),
        opaque(xrgb(40, 80, 200)),
        "the content hid with it"
    );

    // The unminimize: the exact bytes return.
    client
        .conn
        .send_request(&t3, "unminimize", vec![])
        .expect("unminimize");
    wait_for_on_with(&mut client, "configure", t3.id().as_u32(), 3);
    for _ in 0..3 {
        client.sync();
    }
    let words = tb.scanout();
    assert_eq!(word(&words, 224, 51), band, "the band's bytes back");
    assert_eq!(word(&words, 48, 80), ring, "the ring's bytes back");
    assert_eq!(word(&words, 235, 63), glyph, "the affordance's bytes back");
    assert_eq!(word(&words, 101, 100), opaque(xrgb(30, 140, 150)));
    let _ = w3;
}

/// Wait until `n` events of one name have arrived on one object (the
/// count-based discipline — a wait that fires on the *new* arrival,
/// never one already collected).
fn wait_for_on_with(client: &mut TestClient, name: &str, target: u32, n: usize) {
    client.wait_until(|c| {
        c.records
            .iter()
            .filter(|r| r.event == name && r.target == target)
            .count()
            >= n
    });
}

/// The geometry regime: a maximized SSD window's *frame* fills the
/// workspace area — the machine proposes the content the usable
/// space minus the chrome, and the realizing commit insets the
/// content by the applied band (the title bar on-screen at every
/// state; a CSD window's buffer stays its own whole footprint, the
/// Phase 49 pins untouched).
#[test]
#[allow(clippy::too_many_lines)] // one session, one narrative
fn a_maximized_ssd_window_fills_the_area_with_its_frame() {
    let tb = Testbench::start_with("chrome-max", chrome_config("max"));
    let mut client = TestClient::connect(&tb.addr);

    // The cascade's fillers, then the SSD window at the third step.
    let w1 = map_plain(&tb, &mut client, xrgb(200, 40, 40), 150, 90);
    let w2 = map_plain(&tb, &mut client, xrgb(40, 80, 200), 200, 100);
    let (w3, t3) = ssd_window(&tb, &mut client, xrgb(30, 140, 150), 2);
    // The fillers leave (the desktop's black returns — the maximized
    // band's word lands exact over it).
    client.conn.destroy(&w1).expect("destroy");
    client.conn.destroy(&w2).expect("destroy");
    for _ in 0..3 {
        client.sync();
    }

    // The maximize intent: the proposal is the usable area minus the
    // chrome (1920 x (1080-84) minus 2 x 30 → 1918 x 966).
    client
        .conn
        .send_request(&t3, "maximize", vec![])
        .expect("maximize");
    wait_for_on_with(&mut client, "configure", t3.id().as_u32(), 2);
    let proposal = last_on(&client, "configure", t3.id().as_u32());
    let serial = serial_of(proposal);
    assert!(states_of(proposal).test(0), "the maximized flag");
    assert_eq!(proposal.args[2], Value::Uint32(1918), "the content width");
    assert_eq!(proposal.args[3], Value::Uint32(966), "the content height");
    assert_eq!(
        proposal.args[5],
        Value::Int32(29),
        "the band still proposed"
    );

    // The realizing commit: the content lands inset by the chrome —
    // the *frame* fills the area (the position is the chrome-aware
    // truth Phase 52 adds; the CSD origin stays for CSD windows).
    let shm = client.bind("ldp.core.shm");
    let pixels: Vec<u8> = std::iter::repeat(xrgb(30, 140, 150).to_le_bytes())
        .take(1918 * 966)
        .flatten()
        .collect();
    let pool = create_pool(
        &mut client,
        &shm,
        pool_bytes(&pixels),
        (1918 * 966 * 4) as i64,
    );
    let buffer = create_buffer(&mut client, &pool, 0, 1918, 966, 1918 * 4, XR24);
    let before = tb.frames();
    client
        .conn
        .send_request(&t3, "ack_configure", vec![Value::Uint32(serial)])
        .expect("ack");
    attach(&mut client, &w3, &buffer);
    damage(&mut client, &w3, &[Rect::new(0, 0, 1918, 966)]);
    commit(&mut client, &w3, 2);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while tb.frames() <= before {
        assert!(deadline > std::time::Instant::now(), "never rendered");
        client.sync();
    }
    assert_eq!(
        position_of(&tb, &w3),
        (1, 29),
        "the frame fills from the origin"
    );

    // The pixel truths: the title bar across the top (the exact band
    // word over the black desktop), the content below it, the ring
    // down the sides.
    let words = tb.scanout();
    assert_eq!(word(&words, 10, 10), band_word(), "the maximized band");
    assert_eq!(word(&words, 1000, 20), band_word());
    assert_eq!(
        word(&words, 10, 40),
        opaque(xrgb(30, 140, 150)),
        "the content below the band"
    );
    assert_eq!(
        word(&words, 0, 500),
        prem(RING_RGB, RING_ALPHA),
        "the left ring"
    );
    assert_eq!(
        word(&words, 1919, 500),
        prem(RING_RGB, RING_ALPHA),
        "the right ring"
    );
}
