//! Phase 48's exit criterion (the goodbye half): the compositor-owned
//! window-close fade, served end to end through the real protocol —
//!
//! * a destroyed window's last raster fades out under the catalog's
//!   close spring — the ghost is an *owned* copy (the client's pool
//!   is already gone), the fade moves every pixel under it, and the
//!   settled desktop is the plain post-destroy bytes (the
//!   never-animated oracle, byte-for-byte — the A/B proof);
//! * an unmapping commit (a detach) fades the same way — a window
//!   hiding is a window leaving the screen;
//! * the library default (transitions off) destroys plain, no ghost
//!   ever begun — the byte-exactness doctrine;
//! * the ghost keeps its window's own z slot: a window destroyed
//!   *under* another never draws above it (the fade's stack
//!   fidelity);
//! * popups and ephemeral roles leave no ghost (instant dismissal is
//!   their contract — the open fade's own eligibility rules,
//!   mirrored at the close).

mod testbench;

use ldp_client::Proxy;
use ldp_core::geometry::Rect;
use ldp_core::wire::Value;
use testbench::*;

use lion_compositor::server::CompositorConfig;
use lion_compositor::shell::{DockMode, ShellConfig};

/// The XRGB8888 fourcc ("XR24").
const XR24: u32 = 0x3432_5258;
/// The mock output: 1920x1080@60.
const OUT_W: usize = 1920;

/// One XRGB word.
fn xrgb(r: u8, g: u8, b: u8) -> u32 {
    u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b)
}

/// A memfd carrying `pixels` (the pool backing).
fn pool_bytes(pixels: &[u8]) -> std::os::fd::OwnedFd {
    use std::io::Write as _;
    let fd = lion_compositor::sys::memfd("ghost-pool").expect("memfd");
    {
        let mut file = std::fs::File::from(fd.try_clone().expect("clone"));
        file.write_all(pixels).expect("write pool");
    }
    fd
}

/// One scanout pixel as an RGB triple (the alpha is the blend's).
fn px(words: &[u32], x: usize, y: usize) -> [u32; 4] {
    let v = words[y * OUT_W + x];
    [
        (v >> 16) & 0xFF,
        (v >> 8) & 0xFF,
        v & 0xFF,
        (v >> 24) & 0xFF,
    ]
}

/// The choreography-on config (the library default is off; the suite
/// opts in — the `--transitions` switch's own doctrine).
fn ghost_config() -> CompositorConfig {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    CompositorConfig {
        shell: ShellConfig {
            dock: DockMode::Off,
            ..ShellConfig::default()
        },
        transitions: true,
        socket: format!("lion-ghost-{n}-{}", std::process::id()),
        ..CompositorConfig::default()
    }
}

/// The plain config (transitions off — the library default).
fn plain_config() -> CompositorConfig {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    CompositorConfig {
        shell: ShellConfig {
            dock: DockMode::Off,
            ..ShellConfig::default()
        },
        socket: format!("lion-ghost-plain-{n}-{}", std::process::id()),
        ..CompositorConfig::default()
    }
}

/// One window's full bring-up: surface + pool + buffer + commit,
/// pumped until the compositor rendered it (the mapping frame — the
/// open fade's first sample under the choreography config).
fn map_window(
    tb: &Testbench,
    client: &mut TestClient,
    fill: u32,
    w: usize,
    h: usize,
) -> (Proxy, Proxy, Proxy) {
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
    (surface, buffer, pool)
}

/// Pump until every live motion settles (the open fades and the
/// ghosts — the 10 s deadline the sibling suites share).
fn settle(tb: &Testbench, client: &mut TestClient) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while tb.world(|w| w.scene.transitions.any_live() || w.scene.ghosts.any_live()) {
        assert!(deadline > std::time::Instant::now(), "never settled");
        client.sync();
    }
}

/// THE exit criterion: destroying a mapped window fades its ghost
/// out — the first frame after the destroy still shows the window's
/// ink (dimming), the ink decays frame over frame, and the settled
/// desktop is the plain black background (the window gone, the ghost
/// dropped — the never-animated oracle).
#[test]
fn destroy_fades_the_ghost_to_the_plain_background() {
    let tb = Testbench::start_with("ghost-destroy", ghost_config());
    let mut client = TestClient::connect(&tb.addr);

    let cherry = xrgb(200, 60, 60);
    let (surface, _buffer, _pool) = map_window(&tb, &mut client, cherry, 200, 100);
    // Let the open fade settle: the plain cherry bytes.
    settle(&tb, &mut client);
    let plain = tb.scanout();
    assert_eq!(px(&plain, 100, 50)[0], 200, "the window settled plain");

    // Destroy: the ghost begins.
    let before = tb.frames();
    client.conn.destroy(&surface).expect("destroy");
    client.sync();
    assert!(
        tb.world(|w| w.scene.ghosts.any_live()),
        "the destroy began a close fade"
    );
    assert_eq!(tb.world(|w| w.scene.ghosts.begun), 1);

    // The fade is real and renders: every advance claims a frame, so
    // the destroy under the choreography drives MANY frames (the
    // transitions-off control renders exactly one — the A/B bench
    // below proves that half). Any mid-flight pixel sample is
    // monotone (the close spring never brightens the blend; the
    // serve loop's poll cadence may outrun this loop's sampling, so
    // a mid-flight sample is a bonus, never a requirement — the
    // frames counter is the deterministic truth).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut last_r = 200u32;
    while tb.world(|w| w.scene.ghosts.any_live()) {
        assert!(deadline > std::time::Instant::now(), "the fade never ran");
        client.sync();
        let words = tb.scanout();
        let r = px(&words, 100, 50)[0];
        assert!(
            r <= last_r,
            "the fade never brightens (channel {r}, last {last_r})"
        );
        last_r = r;
    }
    let frames_during = tb.frames() - before;
    assert!(
        frames_during >= 3,
        "the fade drove multiple frames (got {frames_during})"
    );

    // Settle: the ghost drops, the background is plain black.
    settle(&tb, &mut client);
    let settled = tb.scanout();
    assert_eq!(px(&settled, 100, 50)[0], 0, "the window is gone");
    assert!(!tb.world(|w| w.scene.ghosts.any_live()));
    // The settled desktop equals the same scene destroyed without
    // the choreography (the A/B byte oracle — the never-animated
    // equivalence).
    let tb_plain = Testbench::start_with("ghost-destroy-plain", plain_config());
    let mut client_plain = TestClient::connect(&tb_plain.addr);
    let (surface_p, _b, _p) = map_window(&tb_plain, &mut client_plain, cherry, 200, 100);
    client_plain.conn.destroy(&surface_p).expect("destroy");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let frames_before = tb_plain.frames();
    while tb_plain.frames() <= frames_before {
        assert!(deadline > std::time::Instant::now());
        client_plain.sync();
    }
    assert_eq!(
        tb_plain.scanout(),
        settled,
        "the settled fade equals the plain destroy, byte for byte"
    );
}

/// An unmapping commit (a detach) is a window leaving the screen: the
/// same ghost, the same fade, the same settle.
#[test]
fn unmap_fades_the_ghost() {
    let tb = Testbench::start_with("ghost-unmap", ghost_config());
    let mut client = TestClient::connect(&tb.addr);

    let teal = xrgb(30, 170, 170);
    let (surface, _buffer, _pool) = map_window(&tb, &mut client, teal, 160, 90);
    settle(&tb, &mut client);

    // The detach: attach(null) + commit — the surface stays alive
    // (roleless content), the screen loses it, the ghost fades.
    client
        .conn
        .send_request(&surface, "attach", vec![Value::Object(None)])
        .expect("detach");
    commit(&mut client, &surface, 2);
    client.sync();
    assert!(
        tb.world(|w| w.scene.ghosts.any_live()),
        "the unmapping commit began a close fade"
    );

    settle(&tb, &mut client);
    let settled = tb.scanout();
    assert_eq!(px(&settled, 80, 45)[1], 0, "the window is gone");
    // The surface lives on (the client may re-attach); the ghost does
    // not.
    assert!(!tb.world(|w| w.scene.ghosts.any_live()));
    assert!(tb.world(|w| !w.scene.routes.is_empty()));
}

/// The library default (transitions off): the destroy is plain, no
/// ghost ever begun, and the next frame is the plain background.
#[test]
fn transitions_off_destroys_plain() {
    let tb = Testbench::start_with("ghost-off", plain_config());
    let mut client = TestClient::connect(&tb.addr);

    let gold = xrgb(210, 180, 40);
    let (surface, _buffer, _pool) = map_window(&tb, &mut client, gold, 120, 60);
    let before = tb.frames();
    client.conn.destroy(&surface).expect("destroy");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while tb.frames() <= before {
        assert!(deadline > std::time::Instant::now());
        client.sync();
    }
    assert_eq!(tb.world(|w| w.scene.ghosts.begun), 0, "no ghost begun");
    let words = tb.scanout();
    assert_eq!(px(&words, 60, 30)[0], 0, "the plain destroy is immediate");
}

/// The ghost keeps its window's own z slot: a window destroyed
/// *under* another window never draws above it. The legacy placement
/// keeps creation positions — two windows at the origin, the second
/// (smaller) on top; destroying the bottom one fades a ghost the top
/// window keeps covering.
#[test]
fn the_ghost_stays_below_the_windows_above_it() {
    let tb = Testbench::start_with("ghost-z", ghost_config());
    let mut client = TestClient::connect(&tb.addr);

    // The bottom window: 200x100 plum.
    let plum = xrgb(120, 40, 160);
    let (bottom, _b1, _p1) = map_window(&tb, &mut client, plum, 200, 100);
    // The top window: 100x50 lime, covering the plum's upper-left.
    let lime = xrgb(80, 220, 60);
    let (top, _b2, _p2) = map_window(&tb, &mut client, lime, 100, 50);
    settle(&tb, &mut client);
    let stacked = tb.scanout();
    assert_eq!(px(&stacked, 50, 25)[1], 220, "the top window covers");
    assert_eq!(px(&stacked, 150, 75)[2], 160, "the bottom window shows");

    // Destroy the BOTTOM window: the ghost fades at the bottom slot.
    client.conn.destroy(&bottom).expect("destroy");
    client.sync();
    assert!(tb.world(|w| w.scene.ghosts.any_live()));

    // While the fade runs, the top window's ink is untouched (the
    // ghost never covers it) and the uncovered region dims.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while tb.world(|w| w.scene.ghosts.any_live()) {
        assert!(deadline > std::time::Instant::now(), "never settled");
        client.sync();
        let words = tb.scanout();
        assert_eq!(
            px(&words, 50, 25)[1],
            220,
            "the top window stays whole over the fading ghost"
        );
    }
    // Settled: the top window still whole, the uncovered region black.
    let settled = tb.scanout();
    assert_eq!(px(&settled, 50, 25)[1], 220);
    assert_eq!(px(&settled, 150, 75)[2], 0, "the bottom window is gone");
    let _ = top;
}

/// Popups and ephemeral roles leave no ghost: instant dismissal is
/// the popup's contract, and the ephemeral roles' doctrine holds at
/// the close exactly as it holds at the open.
#[test]
fn popups_and_tooltips_leave_no_ghost() {
    // The popup: an anchored menu surface.
    let tb = Testbench::start_with("ghost-popup", ghost_config());
    let mut client = TestClient::connect(&tb.addr);
    let shm = client.bind("ldp.core.shm");
    let compositor = client.bind("ldp.core.compositor");
    let shell = client.bind("ldp.shell.shell");

    // A parent window first (the popup's anchor lives on it).
    let (parent, _b, _p) = map_window(&tb, &mut client, xrgb(90, 90, 90), 200, 100);
    settle(&tb, &mut client);

    // The popup surface: 60x40, anchored at the parent's origin.
    let pixels: Vec<u8> = std::iter::repeat(xrgb(240, 240, 240).to_le_bytes())
        .take(60 * 40)
        .flatten()
        .collect();
    let pool = create_pool(&mut client, &shm, pool_bytes(&pixels), (60 * 40 * 4) as i64);
    let buffer = create_buffer(&mut client, &pool, 0, 60, 40, 240, XR24);
    let popup_surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("surface");
    let _popup = client.conn.create_object(
        &shell,
        "get_popup",
        vec![
            Value::Object(Some(popup_surface.id())),
            Value::Object(Some(parent.id())),
            Value::Rect(Rect::new(10, 10, 1, 1)),
            Value::Enum(1), // anchor: top_left
            Value::Enum(5), // gravity: bottom_right
            Value::Int32(0),
            Value::Int32(0),
            Value::Bitset(ldp_core::bitset::Bitset128::EMPTY),
        ],
    );
    let _ = buffer;
    attach(&mut client, &popup_surface, &buffer);
    damage(&mut client, &popup_surface, &[Rect::new(0, 0, 60, 40)]);
    commit(&mut client, &popup_surface, 1);
    client.sync();
    // The popup's surface dies (the dispatcher dismisses the machine
    // in the same breath) — no ghost.
    client.conn.destroy(&popup_surface).expect("destroy");
    client.sync();
    assert_eq!(
        tb.world(|w| w.scene.ghosts.begun),
        0,
        "the popup dismissed without a ghost"
    );

    // The ephemeral role: a tooltip-claimed window destroyed — no
    // ghost (the role's transitions_eligible is false at both ends).
    let (tip, _b2, _p2) = map_window(&tb, &mut client, xrgb(250, 240, 130), 80, 30);
    let toplevel = client
        .conn
        .create_object(
            &shell,
            "get_toplevel",
            vec![Value::Object(Some(tip.id())), Value::Enum(2)],
        )
        .expect("get_toplevel");
    client
        .conn
        .send_request(&toplevel, "set_semantic_role", vec![Value::Enum(3)])
        .expect("tooltip role");
    client.sync();
    client.conn.destroy(&tip).expect("destroy");
    client.sync();
    assert_eq!(
        tb.world(|w| w.scene.ghosts.begun),
        0,
        "the tooltip left no ghost"
    );
}
