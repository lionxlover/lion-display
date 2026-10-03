//! Phase 53's exit criterion (the caption grip): the drawn band is a
//! *move grip* — the server-drawn title bar drags the window through
//! the same machinery every desktop's caption serves, end to end over
//! the real protocol —
//!
//! * the **caption drag**: a press the band claims (outside the close
//!   affordance) arms the grip as Phase 52 served it (consumed,
//!   focusing); the pointer's first motion *converts* it — the server
//!   mints the move drag itself (`start_move`'s own machinery, no
//!   client request involved), and every motion batch moves the
//!   window by exactly the pointer's travel (server truth — the
//!   toplevel never sees a configure for the move), the release
//!   consumed like every band button, the geometry standing;
//! * the **click doctrine**: a press without motion never drags — the
//!   Phase 52 band behavior (a consumed, focusing press) is the whole
//!   story, no close, no move, no configure beyond the activation;
//! * the **demotion**: dragging a *maximized* SSD window by its drawn
//!   caption releases the states and restores the floating size under
//!   the pointer's proportional grip — measured over the *frame* (the
//!   caption the hand actually grips), so the hand keeps its hold on
//!   the title bar of the restored window (the Windows 11 / macOS
//!   title-drag doctrine, never tearing — the size realizes at the
//!   client's ack+commit cadence);
//! * the **close territory**: the close affordance's own grip never
//!   converts (the drag-away cancel is its doctrine — a click that
//!   left the button never happened, and it never becomes a move);
//! * the **death sweep**: the dragged window's destroy ends both the
//!   drag and the grip; a later motion moves nothing, the world stays
//!   healthy.

mod testbench;

use ldp_client::Proxy;
use ldp_core::geometry::Rect;
use ldp_core::wire::Value;
use ldp_input::device::DeviceClass;
use ldp_input::normalizer::InputEvent;
use testbench::*;

use ldp_compositor::surface::Surface;
use lion_compositor::server::CompositorConfig;
use lion_compositor::shell::{DockMode, ShellConfig, BAND_ALPHA, BAND_RGB};

/// The XRGB8888 fourcc ("XR24").
const XR24: u32 = 0x3432_5258;
/// The mock output: 1920x1080@60 (the dock off — the whole output
/// usable, the drag arithmetic's clean grid).
const OUT_W: usize = 1920;
/// BTN_LEFT (the evdev button code).
const BTN_LEFT: u32 = 0x110;

/// One XRGB word.
fn xrgb(r: u8, g: u8, b: u8) -> u32 {
    u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b)
}

/// One scanout pixel as a raw word (the premultiplied truth the
/// painter's words land in, uninterpreted).
fn word(words: &[u32], x: usize, y: usize) -> u32 {
    words[y * OUT_W + x]
}

/// The band's scanout word (the near-opaque bar over the opaque-black
/// desktop — the exact arithmetic the renderer's over-blend performs).
fn band_word() -> u32 {
    let m = |v: u8| (u32::from(v) * u32::from(BAND_ALPHA) + 127) / 255;
    0xFF00_0000 | (m(BAND_RGB[0]) << 16) | (m(BAND_RGB[1]) << 8) | m(BAND_RGB[2])
}

/// Wait until `n` events of one name have arrived on one object (the
/// count-based doctrine — the wait must fire on the *new* arrival,
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
    let fd = lion_compositor::sys::memfd("caption-pool").expect("memfd");
    {
        let mut file = std::fs::File::from(fd.try_clone().expect("clone"));
        file.write_all(pixels).expect("write pool");
    }
    fd
}

/// The caption tests' configs: dock auto (the cascade's arithmetic the
/// band geometry rides — the dock itself stays at the bottom, far
/// from every window the tests place; the usable area 1920x996, the
/// maximized frame filling it).
fn caption_config(tag: &str) -> CompositorConfig {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    CompositorConfig {
        shell: ShellConfig {
            dock: DockMode::Auto,
            ..ShellConfig::default()
        },
        socket: format!("lion-caption-{tag}-{n}-{}", std::process::id()),
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
    wait_for_on_with(client, "configure", toplevel.id().as_u32(), 1);
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

/// The world's bounds of one surface (the tree's own truth).
fn bounds_of(tb: &Testbench, surface: &Proxy) -> Rect {
    tb.world(|w| {
        for (sid, route) in &w.scene.routes {
            if route.surface_obj.as_u32() == surface.id().as_u32() {
                return w
                    .scene
                    .tree
                    .get(*sid)
                    .map_or(Rect::new(0, 0, 0, 0), Surface::last_bounds);
            }
        }
        panic!("the surface has no route");
    })
}

/// The world's live-drag introspection (the tests' oracle): the mode
/// and the window-start rect, or `None` when no drag is live.
fn drag_view(tb: &Testbench) -> Option<(String, Rect)> {
    tb.world(|w| {
        w.drags.live().map(|d| {
            let mode = match d.mode {
                lion_compositor::shell::DragMode::Move => "move".to_owned(),
                lion_compositor::shell::DragMode::Resize(_) => "resize".to_owned(),
            };
            (mode, d.window_start)
        })
    })
}

/// The world's router pointer (the tests' oracle — the drag's own
/// arithmetic source).
fn pointer_of(tb: &Testbench) -> (f32, f32) {
    tb.world(lion_compositor::World::input_pointer_position)
}

/// One scripted input batch through the same pump the real devices
/// take (the caption grip's own heartbeat).
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
/// pointer lands exactly where the arithmetic says; the drag suite's
/// own hard lesson, respected here).
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

/// The pointer device's mint (the routed events the press and motions
/// would deliver — the caption's press never becomes one).
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

/// The caption suite's stage: two fillers, the SSD window at the
/// cascade's third step (Phase 56: content (49, 77), frame
/// (48, 48, 202, 130), the band's rows [48, 77)), the pointer
/// minted. Returns (w1, w2, surface, toplevel, pointer).
fn caption_stage(tb: &Testbench, client: &mut TestClient) -> (Proxy, Proxy, Proxy, Proxy, Proxy) {
    let w1 = map_plain(tb, client, xrgb(200, 40, 40), 150, 90);
    let w2 = map_plain(tb, client, xrgb(40, 80, 200), 200, 100);
    let (w3, t3) = ssd_window(tb, client, xrgb(30, 140, 150), 2);
    let pointer = mint_pointer(client);
    (w1, w2, w3, t3, pointer)
}

/// THE caption drag: the press on the band arms (consumed — no client
/// button, the window focused and activated), the pointer's first
/// motion converts the grip into the server-minted move drag, every
/// motion batch moves the window by exactly the pointer's travel
/// (server truth — the toplevel never sees a configure for the move),
/// the ink follows (the claims ledger repaints both ends — the band
/// the desktop it left), and the release ends the drag with the
/// geometry standing and nothing fired.
#[test]
#[allow(clippy::too_many_lines)] // one session, one narrative
fn the_caption_drags_the_window_at_the_pumps_cadence() {
    let tb = Testbench::start_with("caption-move", caption_config("mv"));
    let mut client = TestClient::connect(&tb.addr);
    let (_w1, _w2, w3, t3, pointer) = caption_stage(&tb, &mut client);

    // The press on the band: (201, 51) — the band's middle row,
    // over the second filler's content underneath (the z-true claim:
    // the visible band owns its presses, the hidden ink never
    // steals them back), far from the close affordance at
    // (225, 53, 20, 20).
    move_to(&tb, 201, 51);
    button(&tb, true);
    client.sync();
    wait_for_on_with(&mut client, "configure", t3.id().as_u32(), 2);
    assert!(
        states_of(last_on(&client, "configure", t3.id().as_u32())).test(3),
        "the band's press activated the window (bit 3)"
    );
    assert_eq!(
        count_on(&client, "button", pointer.id().as_u32()),
        0,
        "the band's press never reaches the client"
    );
    assert!(
        drag_view(&tb).is_none(),
        "the press arms, it never drags alone"
    );

    // The conversion: the first motion batch mints the move drag —
    // the anchor is the press point (the window tracks the hand
    // rigidly from the grip), the window's start the content rect the
    // cascade placed.
    motion(&tb, 120, 60);
    let (mode, anchor) = drag_view(&tb).expect("the caption drag is live");
    assert_eq!(mode, "move");
    assert_eq!(
        (anchor.x, anchor.y),
        (49, 77),
        "the content rect at the grip"
    );
    assert_eq!((anchor.w, anchor.h), (200, 100));
    assert_eq!(position_of(&tb, &w3), (169, 137), "the window followed");

    // The cadence: every batch moves by exactly the pointer's travel,
    // and no configure ever rides the move (position is server truth).
    motion(&tb, 30, -20);
    assert_eq!(position_of(&tb, &w3), (199, 117));
    let configures = count_on(&client, "configure", t3.id().as_u32());
    assert_eq!(
        configures, 2,
        "a caption move never proposes — position is server truth"
    );

    // The ink follows: the band at the settled position, the desktop
    // where the band was (the claims ledger's both ends).
    let before = tb.frames();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while tb.frames() <= before {
        assert!(deadline > std::time::Instant::now(), "never rendered");
        client.sync();
    }
    let words = tb.scanout();
    // The frame sits at (198, 88): the band's rows [88, 117). The
    // sampled point sits right of both fillers' ink (x >= 224) — the
    // band over the opaque-black desktop, its word exact.
    assert_eq!(word(&words, 230, 100), band_word(), "the band followed");
    assert_eq!(
        word(&words, 208, 130),
        0xFF00_0000 | xrgb(30, 140, 150),
        "the content below the band"
    );
    assert_eq!(
        word(&words, 224, 51),
        0xFF00_0000,
        "the desktop the band left behind"
    );

    // The release: consumed (the band's buttons never route), the
    // drag ends, the geometry stands, nothing fired.
    button(&tb, false);
    client.sync();
    assert!(drag_view(&tb).is_none(), "the caption drag ended");
    assert_eq!(position_of(&tb, &w3), (199, 117), "the geometry stands");
    assert_eq!(
        count_on(&client, "close", t3.id().as_u32()),
        0,
        "a caption drag never asks to close"
    );
    assert_eq!(
        count_on(&client, "button", pointer.id().as_u32()),
        0,
        "the band's release never reaches the client either"
    );
}

/// THE click doctrine: a press without motion never drags. The Phase
/// 52 band behavior is the whole story — the press consumed and
/// focusing, the release consumed, no close ask, no move, no
/// configure beyond the activation, the window exactly where it was.
#[test]
fn the_caption_click_without_motion_never_drags() {
    let tb = Testbench::start_with("caption-click", caption_config("ck"));
    let mut client = TestClient::connect(&tb.addr);
    let (_w1, _w2, w3, t3, pointer) = caption_stage(&tb, &mut client);

    // The press and the release, no motion between them.
    move_to(&tb, 201, 51);
    button(&tb, true);
    client.sync();
    wait_for_on_with(&mut client, "configure", t3.id().as_u32(), 2);
    assert!(
        states_of(last_on(&client, "configure", t3.id().as_u32())).test(3),
        "the press focused its window"
    );
    assert!(drag_view(&tb).is_none(), "the press alone never drags");
    button(&tb, false);
    client.sync();

    // Nothing moved, nothing fired, nothing more was proposed.
    assert!(drag_view(&tb).is_none(), "the click never became a drag");
    assert_eq!(position_of(&tb, &w3), (49, 77), "the window never moved");
    assert_eq!(
        count_on(&client, "configure", t3.id().as_u32()),
        2,
        "the handshake and the activation — nothing else"
    );
    assert_eq!(count_on(&client, "close", t3.id().as_u32()), 0);
    assert_eq!(
        count_on(&client, "button", pointer.id().as_u32()),
        0,
        "the band's buttons never route"
    );
}

/// THE demotion: dragging a *maximized* SSD window by its drawn
/// caption releases the states, restores the floating size under the
/// pointer's proportional grip — measured over the *frame* (the
/// caption the hand actually grips; the hand keeps its hold on the
/// restored window's title band) — and carries the drag on from
/// there, the size realizing at the client's ack+commit cadence
/// (never tearing).
#[test]
#[allow(clippy::too_many_lines)] // one session, one narrative
fn the_caption_drag_demotes_the_maximized_window_under_the_pointer() {
    let tb = Testbench::start_with("caption-demote", caption_config("dm"));
    let mut client = TestClient::connect(&tb.addr);
    let (_w1, _w2, w3, t3, _pointer) = caption_stage(&tb, &mut client);

    // The maximize narrative's prelude: engage, ack, realize — the
    // frame fills the whole usable area (the dock-auto workspace
    // 1920x996), the content inset by the band (the proposal:
    // 1918 x 966 at (1, 29)).
    client
        .conn
        .send_request(&t3, "maximize", vec![])
        .expect("maximize");
    wait_for_on_with(&mut client, "configure", t3.id().as_u32(), 2);
    let max = last_on(&client, "configure", t3.id().as_u32());
    let max_serial = serial_of(max);
    assert!(states_of(max).test(0), "the maximized flag");
    assert_eq!(max.args[2], Value::Uint32(1918), "the content width");
    assert_eq!(max.args[3], Value::Uint32(966), "the content height");
    let shm = client.bind("ldp.core.shm");
    let big: Vec<u8> = std::iter::repeat(xrgb(30, 140, 150).to_le_bytes())
        .take(1918 * 966)
        .flatten()
        .collect();
    let big_pool = create_pool(&mut client, &shm, pool_bytes(&big), (1918 * 966 * 4) as i64);
    let big_buffer = create_buffer(&mut client, &big_pool, 0, 1918, 966, 1918 * 4, XR24);
    // The maximize realize: the 1918 x 966 content lands at (1, 29).
    let before = tb.frames();
    client
        .conn
        .send_request(&t3, "ack_configure", vec![Value::Uint32(max_serial)])
        .expect("ack");
    attach(&mut client, &w3, &big_buffer);
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
        "maximized, the frame at origin"
    );

    // The grip: the pointer on the maximized window's band — (400,
    // 15) — then the press (the activation beyond the handshake and
    // the maximize).
    move_to(&tb, 400, 15);
    let (px0, py0) = pointer_of(&tb);
    button(&tb, true);
    client.sync();
    wait_for_on_with(&mut client, "configure", t3.id().as_u32(), 3);
    assert!(
        states_of(last_on(&client, "configure", t3.id().as_u32())).test(3),
        "the press activated the window (bit 3)"
    );
    assert!(
        drag_view(&tb).is_none(),
        "the press arms, the demotion waits"
    );

    // The conversion: the first motion demotes — the states released,
    // the floating size (200 x 100, the engagement's capture)
    // restored, the window detached under the pointer's proportional
    // grip *on the frame* (the restored frame: 202 x 130 — the
    // content plus the applied insets), the content inset inside it.
    // The demotion's configure rides the outbox; the same batch's
    // advance carries the drag on by the motion's own delta.
    motion(&tb, 0, 20);
    wait_for_on_with(&mut client, "configure", t3.id().as_u32(), 4);
    let demoted = last_on(&client, "configure", t3.id().as_u32());
    assert!(
        !states_of(demoted).test(0) && !states_of(demoted).test(1),
        "the geometry states released"
    );
    assert!(states_of(demoted).test(3), "still the key window");
    assert_eq!(demoted.args[2], Value::Uint32(200), "the restored width");
    assert_eq!(demoted.args[3], Value::Uint32(100), "the restored height");
    // The anchor's own arithmetic, derived from the pointer oracle
    // (the same truth the server computed from) — over the frame
    // (the maximized 1920 x 996, the restore frame 202 x 130), the
    // content inset inside it, the motion's delta applied.
    let expect_anchor = (
        (px0 - (px0 / 1920.0) * 202.0).round() as i32,
        (py0 - (py0 / 996.0) * 130.0).round() as i32,
    );
    let expect_content = (expect_anchor.0 + 1, expect_anchor.1 + 29);
    let expect_landed = (expect_content.0, expect_content.1 + 20);
    assert_eq!(position_of(&tb, &w3), expect_landed, "the anchored detach");
    let (mode, anchor) = drag_view(&tb).expect("the demoted drag is live");
    assert_eq!(mode, "move");
    assert_eq!(
        (anchor.x, anchor.y),
        expect_content,
        "the drag anchors on the restored content rect"
    );
    assert_eq!((anchor.w, anchor.h), (200, 100));

    // The drag carries on: the deltas move the (still-big) window —
    // the shrink realizes at the client's cadence, never tearing.
    motion(&tb, 40, 30);
    let (px1, py1) = pointer_of(&tb);
    let landed = (
        expect_content.0 + (px1 - px0) as i32,
        expect_content.1 + (py1 - py0) as i32,
    );
    assert_eq!(
        position_of(&tb, &w3),
        landed,
        "the window follows the pointer"
    );

    // The realize: the restore-size buffer, the dragged position.
    let d_serial = serial_of(demoted);
    client
        .conn
        .send_request(&t3, "ack_configure", vec![Value::Uint32(d_serial)])
        .expect("ack");
    let small: Vec<u8> = std::iter::repeat(xrgb(30, 140, 150).to_le_bytes())
        .take(200 * 100)
        .flatten()
        .collect();
    let small_pool = create_pool(
        &mut client,
        &shm,
        pool_bytes(&small),
        (200 * 100 * 4) as i64,
    );
    let small_buffer = create_buffer(&mut client, &small_pool, 0, 200, 100, 200 * 4, XR24);
    let before = tb.frames();
    attach(&mut client, &w3, &small_buffer);
    damage(&mut client, &w3, &[Rect::new(0, 0, 200, 100)]);
    commit(&mut client, &w3, 3);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while tb.frames() <= before {
        assert!(deadline > std::time::Instant::now(), "never rendered");
        client.sync();
    }
    assert_eq!(position_of(&tb, &w3), landed, "the dragged position");
    assert_eq!((bounds_of(&tb, &w3).w, bounds_of(&tb, &w3).h), (200, 100));

    // The pixel truth: the hand kept its grip on the *caption* — the
    // pointer (441, 65) sits on the restored frame's own band (the
    // proportional grip mapped the press's band point onto the
    // restore's band rows).
    let words = tb.scanout();
    assert_eq!(
        word(&words, px1 as usize, py1 as usize),
        band_word(),
        "the hand still grips the band"
    );

    // The release ends the drag; nothing fired.
    button(&tb, false);
    client.sync();
    assert!(drag_view(&tb).is_none(), "the demoted drag ended");
    assert_eq!(position_of(&tb, &w3), landed, "the geometry stands");
    assert_eq!(count_on(&client, "close", t3.id().as_u32()), 0);
}

/// THE close territory: the close affordance's own grip never
/// converts — a press on the button, a motion away, a release: no
/// move drag is ever minted (the caption is the *band's* grip, the
/// button's own arm-fire narrative stands), no close fires (the
/// drag-away cancel every desktop serves), the window never moves.
#[test]
fn the_close_affordances_grip_never_becomes_a_move() {
    let tb = Testbench::start_with("caption-close", caption_config("cl"));
    let mut client = TestClient::connect(&tb.addr);
    let (_w1, _w2, w3, t3, pointer) = caption_stage(&tb, &mut client);

    // The press on the drawn close button (its center — the same
    // affordance the Phase 52 narrative fires).
    move_to(&tb, 235, 63);
    button(&tb, true);
    client.sync();
    wait_for_on_with(&mut client, "configure", t3.id().as_u32(), 2);
    assert!(
        states_of(last_on(&client, "configure", t3.id().as_u32())).test(3),
        "the button's press focused its window"
    );

    // The motion away: the close grip never converts (its drag-away
    // cancel is its doctrine — and it never becomes a move either).
    motion(&tb, 60, 40);
    client.sync();
    assert!(
        drag_view(&tb).is_none(),
        "the close arm's grip never becomes a move drag"
    );
    assert_eq!(position_of(&tb, &w3), (49, 77), "the window never moved");

    // The release: the cancel (no close ask), everything consumed.
    button(&tb, false);
    client.sync();
    assert!(drag_view(&tb).is_none());
    assert_eq!(position_of(&tb, &w3), (49, 77));
    assert_eq!(
        count_on(&client, "close", t3.id().as_u32()),
        0,
        "the drag away cancelled the ask"
    );
    assert_eq!(
        count_on(&client, "button", pointer.id().as_u32()),
        0,
        "the band's buttons never route"
    );
    assert_eq!(
        count_on(&client, "configure", t3.id().as_u32()),
        2,
        "the handshake and the activation — nothing else"
    );
}

/// THE death sweep: the caption-dragged window's destroy ends both
/// the drag and the grip (a later motion moves nothing), and the
/// world stays healthy.
#[test]
fn the_caption_dragged_windows_death_ends_the_grip() {
    let tb = Testbench::start_with("caption-death", caption_config("dd"));
    let mut client = TestClient::connect(&tb.addr);
    let (_w1, _w2, w3, t3, _pointer) = caption_stage(&tb, &mut client);

    // The caption drag: press, convert, carry.
    move_to(&tb, 201, 51);
    button(&tb, true);
    client.sync();
    wait_for_on_with(&mut client, "configure", t3.id().as_u32(), 2);
    motion(&tb, 40, 20);
    assert!(drag_view(&tb).is_some(), "the caption drag is live");
    assert_eq!(position_of(&tb, &w3), (89, 97), "the window followed");

    // The window dies mid-drag (the surface's own destroy).
    client.conn.destroy(&w3).expect("destroy");
    client.sync();
    assert!(drag_view(&tb).is_none(), "the grip died with the window");

    // A late motion moves nothing (the drag is gone, the grip is
    // gone — no conversion, no advance), and the world stays healthy.
    motion(&tb, 100, 100);
    client.sync();
    assert!(drag_view(&tb).is_none());
    for _ in 0..3 {
        client.sync();
    }
    assert_eq!(count_on(&client, "close", t3.id().as_u32()), 0);
}
