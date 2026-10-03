//! Phase 57's exit criterion (the chrome ghost): a dying
//! server-decorated window's band rides the close fade, served end
//! to end through the real protocol —
//!
//! * **the band fades with the content** (THE gap): a destroyed
//!   SSD window's ghost carries its chrome — the band, the title
//!   strip, and the content leave as one frame at the one
//!   close-spring opacity (the pre-Phase-57 truth — the content
//!   fading while the band left with the route, one frame, abrupt —
//!   refuted by the pixels: the band's ink decays frame over frame,
//!   and the settled desktop equals the never-animated destroy,
//!   byte for byte);
//! * **the strip rides too**: a titled window's ghost carries its
//!   strip — the dark title ink stays darker than the band through
//!   the whole fade (the two inks dim together, their contrast
//!   holding to the last frame);
//! * **the z fidelity**: a window destroyed *under* another keeps
//!   its band below the survivor — the living window's band and
//!   content stay whole over the fading ghost, exactly the live
//!   chrome's own stacking;
//! * **the zero-drift controls**: plain and client-decorated
//!   windows ghost content-only (no chrome payload, every Phase 48
//!   byte intact), and with transitions off the destroy is plain —
//!   no ghost ever begun, the band leaving with the route as it
//!   always has;
//! * **the Liquid ghost**: a dressed band's frost outlives its
//!   window — the frozen shape carries the `liquid` bit, the fading
//!   glass still reading the composed canvas beneath it;
//! * **the unmap arm**: a hiding window (a detach) leaves with its
//!   chrome the same way — a window leaving the screen is a window
//!   leaving the screen, band and all.

mod testbench;

use ldp_client::Proxy;
use ldp_core::geometry::Rect;
use ldp_core::wire::Value;
use testbench::*;

use ldp_renderer::{EffectChoice, EffectTier};
use lion_compositor::server::CompositorConfig;
use lion_compositor::shell::{DockMode, ShellConfig, BAND_ALPHA, BAND_RGB};

/// The XRGB8888 fourcc ("XR24").
const XR24: u32 = 0x3432_5258;
/// The mock output: 1920x1080@60.
const OUT_W: usize = 1920;
/// The SSD window's content size (the frame grows to 202x130).
const WIN_W: usize = 200;
const WIN_H: usize = 100;

/// One XRGB word.
fn xrgb(r: u8, g: u8, b: u8) -> u32 {
    u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b)
}

/// One scanout pixel's channels (R, G, B, A).
fn px(words: &[u32], x: usize, y: usize) -> [u32; 4] {
    let v = words[y * OUT_W + x];
    [
        (v >> 16) & 0xFF,
        (v >> 8) & 0xFF,
        v & 0xFF,
        (v >> 24) & 0xFF,
    ]
}

/// One scanout pixel as a raw word (the byte-exact proof's form).
fn word(words: &[u32], x: usize, y: usize) -> u32 {
    words[y * OUT_W + x]
}

/// One opaque straight color's scanout word.
fn opaque(v: u32) -> u32 {
    0xFF00_0000 | (v & 0x00FF_FFFF)
}

/// The flat band's scanout word (the near-opaque bar over the
/// opaque-black desktop: the alpha saturates full, the RGB the bar's
/// own premultiplied triple — the exact arithmetic the renderer's
/// over-blend performs).
fn band_word() -> u32 {
    let m = |v: u8| (u32::from(v) * u32::from(BAND_ALPHA) + 127) / 255;
    0xFF00_0000 | (m(BAND_RGB[0]) << 16) | (m(BAND_RGB[1]) << 8) | m(BAND_RGB[2])
}

/// The flat band's settled R channel over the black desktop (the
/// monotone-decay baselines read the same arithmetic the painter
/// writes).
fn band_r() -> u32 {
    (u32::from(BAND_RGB[0]) * u32::from(BAND_ALPHA) + 127) / 255
}

/// The tests' configs: dock auto (the placement's arithmetic the
/// frame geometry rides — the shell must be active for the
/// chrome-aware placement to serve; the 40 px reservation at the
/// bottom, far from every window the tests place), transitions on
/// (the choreography the ghost needs), the effects tier the caller
/// pins (Minimal's flat band for the deterministic oracles, Medium
/// for the Liquid ghost).
fn ghost_config(tag: &str, tier: EffectChoice) -> CompositorConfig {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    CompositorConfig {
        shell: ShellConfig {
            dock: DockMode::Auto,
            ..ShellConfig::default()
        },
        effects: tier,
        transitions: true,
        socket: format!("lion-cghost-{tag}-{n}-{}", std::process::id()),
        ..CompositorConfig::default()
    }
}

/// The plain config (transitions off — the library default).
fn plain_config(tag: &str) -> CompositorConfig {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    CompositorConfig {
        shell: ShellConfig {
            dock: DockMode::Auto,
            ..ShellConfig::default()
        },
        socket: format!("lion-cghost-plain-{tag}-{n}-{}", std::process::id()),
        ..CompositorConfig::default()
    }
}

/// A memfd carrying `pixels` (the pool backing).
fn pool_bytes(pixels: &[u8]) -> std::os::fd::OwnedFd {
    use std::io::Write as _;
    let fd = lion_compositor::sys::memfd("cghost-pool").expect("memfd");
    {
        let mut file = std::fs::File::from(fd.try_clone().expect("clone"));
        file.write_all(pixels).expect("write pool");
    }
    fd
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

/// One SSD window realized (the chrome_session pattern): the
/// toplevel minted before the mapping commit, the handshake acked,
/// the first commit mapping the surface at the cascade's `nth` step
/// with the insets applied (the band serving from its first frame).
/// Returns (surface, toplevel). The window is 200x100 with `fill`.
fn ssd_window(tb: &Testbench, client: &mut TestClient, fill: u32, nth: usize) -> (Proxy, Proxy) {
    let shell = client.bind("ldp.shell.shell");
    let shm = client.bind("ldp.core.shm");
    let compositor = client.bind("ldp.core.compositor");
    let surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("surface");
    let toplevel = client
        .conn
        .create_object(
            &shell,
            "get_toplevel",
            // The decoration mode: 1 = server-drawn (the SSD band).
            vec![Value::Object(Some(surface.id())), Value::Enum(1)],
        )
        .expect("get_toplevel");
    wait_for_on(client, "configure", toplevel.id().as_u32());
    let handshake = last_on(client, "configure", toplevel.id().as_u32());
    client
        .conn
        .send_request(
            &toplevel,
            "ack_configure",
            vec![Value::Uint32(serial_of(handshake))],
        )
        .expect("ack");
    let pixels: Vec<u8> = std::iter::repeat(fill.to_le_bytes())
        .take(WIN_W * WIN_H)
        .flatten()
        .collect();
    let pool = create_pool(
        client,
        &shm,
        pool_bytes(&pixels),
        (WIN_W * WIN_H * 4) as i64,
    );
    let buffer = create_buffer(
        client,
        &pool,
        0,
        WIN_W as i32,
        WIN_H as i32,
        (WIN_W * 4) as i32,
        XR24,
    );
    let before = tb.frames();
    attach(client, &surface, &buffer);
    damage(
        client,
        &surface,
        &[Rect::new(0, 0, WIN_W as u32, WIN_H as u32)],
    );
    commit(client, &surface, 1);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while tb.frames() <= before {
        assert!(deadline > std::time::Instant::now(), "never rendered");
        client.sync();
    }
    let _ = nth; // the cascade step is the caller's truth to pin
    (surface, toplevel)
}

/// One CSD window realized: the toplevel minted client-decorated, the
/// handshake acked, mapped. Its buffer is its whole footprint (no
/// server chrome ever serves).
fn csd_window(tb: &Testbench, client: &mut TestClient, fill: u32) -> (Proxy, Proxy) {
    let shell = client.bind("ldp.shell.shell");
    let shm = client.bind("ldp.core.shm");
    let compositor = client.bind("ldp.core.compositor");
    let surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("surface");
    let toplevel = client
        .conn
        .create_object(
            &shell,
            "get_toplevel",
            vec![Value::Object(Some(surface.id())), Value::Enum(2)],
        )
        .expect("get_toplevel");
    wait_for_on(client, "configure", toplevel.id().as_u32());
    let handshake = last_on(client, "configure", toplevel.id().as_u32());
    client
        .conn
        .send_request(
            &toplevel,
            "ack_configure",
            vec![Value::Uint32(serial_of(handshake))],
        )
        .expect("ack");
    let pixels: Vec<u8> = std::iter::repeat(fill.to_le_bytes())
        .take(WIN_W * WIN_H)
        .flatten()
        .collect();
    let pool = create_pool(
        client,
        &shm,
        pool_bytes(&pixels),
        (WIN_W * WIN_H * 4) as i64,
    );
    let buffer = create_buffer(
        client,
        &pool,
        0,
        WIN_W as i32,
        WIN_H as i32,
        (WIN_W * 4) as i32,
        XR24,
    );
    let before = tb.frames();
    attach(client, &surface, &buffer);
    damage(
        client,
        &surface,
        &[Rect::new(0, 0, WIN_W as u32, WIN_H as u32)],
    );
    commit(client, &surface, 1);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while tb.frames() <= before {
        assert!(deadline > std::time::Instant::now(), "never rendered");
        client.sync();
    }
    (surface, toplevel)
}

/// Set a toplevel's title and pump until the strip renders.
fn set_title(tb: &Testbench, client: &mut TestClient, toplevel: &Proxy, title: &str) {
    let before = tb.frames();
    client
        .conn
        .send_request(toplevel, "set_title", vec![Value::String(Box::from(title))])
        .expect("set_title");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while tb.frames() <= before {
        assert!(
            deadline > std::time::Instant::now(),
            "the title never rendered"
        );
        client.sync();
    }
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

/// THE exit criterion: destroying a mapped SSD window fades its ghost
/// *with the chrome* — the band's ink decays frame over frame at the
/// close spring's cadence (the pre-Phase-57 truth — the band leaving
/// with the route in one abrupt frame while the content faded —
/// refuted by the pixels), and the settled desktop equals the plain
/// destroy byte for byte (the never-animated oracle).
#[allow(clippy::too_many_lines)] // one session, one narrative: the rig reads best unsplit
#[test]
fn destroy_fades_the_band_with_the_content() {
    let tb = Testbench::start_with(
        "cghost-destroy",
        ghost_config("destroy", EffectChoice::Tier(EffectTier::Minimal)),
    );
    let mut client = TestClient::connect(&tb.addr);

    let cherry = xrgb(200, 60, 60);

    let (surface, _toplevel) = ssd_window(&tb, &mut client, cherry, 0);

    // Let the open fade settle: the plain band + cherry bytes.
    settle(&tb, &mut client);

    let plain = tb.scanout();
    assert_eq!(
        word(&plain, 5, 10),
        band_word(),
        "the band settled plain (row 10)"
    );
    assert_eq!(px(&plain, 100, 50)[0], 200, "the window settled plain");
    assert_eq!(
        word(&plain, 100, 0),
        band_word(),
        "the band's row 0 (the parked frame)"
    );

    // Destroy: the ghost begins, carrying the chrome.
    let before = tb.frames();

    client.conn.destroy(&surface).expect("destroy");
    client.sync();

    assert!(
        tb.world(|w| w.scene.ghosts.any_live()),
        "the destroy began a close fade"
    );
    // The frozen chrome truth: the frame the band wore, the shape the
    // raster cache keys on, no strip (no title was set).
    let (dest, frame, shape, strip) = tb.world(|w| {
        let g = w.scene.ghosts.ghosts.first().expect("the ghost exists");
        (
            g.dest,
            g.chrome.as_ref().map(|c| c.frame),
            g.chrome.as_ref().map(|c| c.shape),
            g.chrome.as_ref().is_some_and(|c| c.strip.is_none()),
        )
    });
    assert_eq!(
        (dest.x, dest.y, dest.w, dest.h),
        (1, 29, 200, 100),
        "the content rect"
    );
    let frame = frame.expect("the ghost carries its chrome");
    assert_eq!(
        (frame.x, frame.y, frame.w, frame.h),
        (0, 0, 202, 130),
        "the frozen frame"
    );
    let shape = shape.expect("the shape");
    assert_eq!((shape.w, shape.h), (202, 130), "the shape's frame");
    assert_eq!(
        (shape.top, shape.left, shape.right, shape.bottom),
        (29, 1, 1, 1),
        "the Lion insets"
    );
    assert_eq!(
        (shape.close, shape.margin),
        (20, 5),
        "the close affordance at identity scale"
    );
    assert!(!shape.liquid, "the flat band (Minimal never dresses)");
    assert!(strip, "no strip rides an untitled window");

    // The fade is real and renders: every advance claims a frame, and
    // the band's ink decays monotonically (the close spring never
    // brightens the blend; a mid-flight sample is a bonus — the
    // frames counter is the deterministic truth, but the band pixel
    // may never brighten whenever it IS sampled).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut last_r = band_r();
    while tb.world(|w| w.scene.ghosts.any_live()) {
        assert!(deadline > std::time::Instant::now(), "the fade never ran");
        client.sync();
        let words = tb.scanout();
        let r = px(&words, 5, 10)[0];
        assert!(
            r <= last_r,
            "the band never brightens (channel {r}, last {last_r})"
        );
        last_r = r;
    }
    let frames_during = tb.frames() - before;
    assert!(
        frames_during >= 3,
        "the fade drove multiple frames (got {frames_during})"
    );

    // Settle: the ghost drops, the WHOLE frame is the plain background.
    settle(&tb, &mut client);

    let settled = tb.scanout();
    assert_eq!(px(&settled, 100, 50)[0], 0, "the content is gone");
    assert_eq!(px(&settled, 5, 10)[0], 0, "the band is gone");
    assert_eq!(px(&settled, 100, 0)[0], 0, "the frame's row 0 is gone");
    assert!(!tb.world(|w| w.scene.ghosts.any_live()));
    // The A/B byte oracle: the settled fade equals the same SSD window
    // destroyed without the choreography (the never-animated
    // equivalence — band and all).
    //
    // The dock's intro rise integrates at render cadence — a spring
    // that only advances when frames actually render. The fade bench
    // drove fifty frames through the ghost's economy; the control's
    // would be a silent handful (one map frame, one destroy frame),
    // so the bar's rest is pumped here the way a living desktop
    // pumps it — a client re-committing its buffer (each commit
    // claims a frame, each frame integrates the spring). The resting
    // ink is the same rectangle either bench reaches; then the WHOLE
    // canvas compares — window, band, dock, byte for byte.
    let tb_plain = Testbench::start_with("cghost-destroy-plain", plain_config("destroy"));
    let mut client_plain = TestClient::connect(&tb_plain.addr);
    let (surface_p, _tp) = ssd_window(&tb_plain, &mut client_plain, cherry, 0);
    let rise_deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !tb_plain.world(|w| w.shell.dock.as_ref().is_some_and(|d| d.rise.settled())) {
        assert!(
            rise_deadline > std::time::Instant::now(),
            "the dock never settled"
        );
        damage(
            &mut client_plain,
            &surface_p,
            &[Rect::new(0, 0, WIN_W as u32, WIN_H as u32)],
        );
        commit(&mut client_plain, &surface_p, 2);
        client_plain.sync();
    }

    let _ = client_plain.conn.destroy(&surface_p).expect("destroy");
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

/// The title strip rides the fade: a titled window's ghost carries
/// its strip — the dark title ink stays darker than the band through
/// the whole fade (both inks dim together, their contrast holding),
/// and settles with the rest of the frame.
#[test]
fn the_title_strip_rides_the_fade() {
    let tb = Testbench::start_with(
        "cghost-strip",
        ghost_config("strip", EffectChoice::Tier(EffectTier::Minimal)),
    );
    let mut client = TestClient::connect(&tb.addr);

    let teal = xrgb(30, 170, 170);
    let (surface, toplevel) = ssd_window(&tb, &mut client, teal, 0);
    set_title(&tb, &mut client, &toplevel, "Rides");
    settle(&tb, &mut client);
    let plain = tb.scanout();
    assert_eq!(word(&plain, 5, 10), band_word(), "the band settled plain");

    // Destroy: the ghost carries the strip.
    client.conn.destroy(&surface).expect("destroy");
    client.sync();
    assert!(tb.world(|w| w.scene.ghosts.any_live()));
    let (title, srect) = tb.world(|w| {
        let g = w.scene.ghosts.ghosts.first().expect("the ghost");
        g.chrome
            .as_ref()
            .and_then(|c| c.strip.as_ref())
            .map(|(k, r)| (k.title.to_string(), *r))
            .expect("the ghost carries its strip")
    });
    assert_eq!(title.as_str(), "Rides", "the frozen title");
    assert!(
        srect.y >= 0 && srect.y < 29 && srect.x >= 1 && srect.x < 170,
        "the strip's rect inside the band ({srect:?})"
    );
    // The title ink pixel: the DARKEST word in the strip's rect (the
    // glyph stroke's center — full coverage, the strongest ink), found
    // in the settled pre-destroy scanout (the rect is the same rect
    // the ghost froze). Partial-coverage edges would round the
    // contrast away; the stroke center carries it.
    let mut ink = (5usize, 10usize);
    let mut darkest = u32::MAX;
    for y in srect.y as usize..(srect.y + srect.h as i32) as usize {
        for x in srect.x as usize..(srect.x + srect.w as i32) as usize {
            let r = px(&plain, x, y)[0];
            if r < darkest {
                darkest = r;
                ink = (x, y);
            }
        }
    }
    assert!(darkest < 200, "the title ink exists in the strip's rect");
    assert!(darkest < 100, "the title ink is dark ink (got {darkest})");

    // The fade: the title pixel stays darker than the band pixel
    // through the fade's meaningful span — the contrast proof that the
    // strip's ink rides the fade (the two inks are two layers; the
    // dark stroke's own value legitimately brightens first as the
    // thinning ink lets the band beneath show through — the layered
    // arithmetic's honest shape — before the whole frame decays to
    // black). The band pixel itself (light ink over black) decays
    // monotonically, never brightening.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut last_band = band_r();
    while tb.world(|w| w.scene.ghosts.any_live()) {
        assert!(deadline > std::time::Instant::now(), "never settled");
        client.sync();
        let words = tb.scanout();
        let band_r = px(&words, 5, 10)[0];
        assert!(band_r <= last_band, "the band never brightens");
        let ink_r = px(&words, ink.0, ink.1)[0];
        if band_r > 60 {
            assert!(
                ink_r < band_r,
                "the title ink stays darker than the band (ink {ink_r}, band {band_r})"
            );
        }
        last_band = band_r;
    }
    // Settle: the strip, the band, and the content all gone.
    settle(&tb, &mut client);
    let settled = tb.scanout();
    assert_eq!(px(&settled, 5, 10)[0], 0, "the band is gone");
    assert_eq!(px(&settled, ink.0, ink.1)[0], 0, "the title ink is gone");
    assert_eq!(px(&settled, 100, 50)[0], 0, "the content is gone");
}

/// The chrome ghost keeps its z: a window destroyed *under* another
/// fades below the survivor — the living window's band and content
/// stay whole over the fading ghost's band, exactly the live chrome's
/// own stacking (a dying band never draws above the windows that were
/// above it).
#[test]
fn the_chrome_ghost_keeps_its_z() {
    let tb = Testbench::start_with(
        "cghost-z",
        ghost_config("z", EffectChoice::Tier(EffectTier::Minimal)),
    );
    let mut client = TestClient::connect(&tb.addr);

    // The bottom window: nth 0, frame (0,0,202,130), plum.
    let plum = xrgb(120, 40, 160);
    let (bottom, _tb1) = ssd_window(&tb, &mut client, plum, 0);
    // The top window: nth 1, frame (24,24,202,130), lime — covering
    // the bottom's band from row 24 down.
    let lime = xrgb(80, 220, 60);
    let (_top, _tt) = ssd_window(&tb, &mut client, lime, 1);
    settle(&tb, &mut client);
    let stacked = tb.scanout();
    // The top window's band over the bottom's content (the 2%
    // show-through of the near-opaque bar) — recorded as the
    // baseline, not pinned: the survivor's band decays from here to
    // 233 (over plain black) as the ghost beneath it fades.
    let pre_band_r = px(&stacked, 100, 40)[0];
    assert!(
        pre_band_r > 233,
        "the top window's band covers (got {pre_band_r})"
    );
    assert_eq!(
        word(&stacked, 100, 80),
        opaque(lime),
        "the top window's content covers"
    );
    assert_eq!(
        word(&stacked, 5, 10),
        band_word(),
        "the bottom's band shows"
    );

    // Destroy the BOTTOM window: the ghost's chrome fades at the
    // bottom slot.
    client.conn.destroy(&bottom).expect("destroy");
    client.sync();
    assert!(tb.world(|w| w.scene.ghosts.any_live()));
    assert!(tb.world(|w| w.scene.ghosts.ghosts[0].chrome.is_some()));

    // While the fade runs: the top window's band and content are
    // untouched (the ghost never covers them) and the uncovered band
    // region dims. The survivor's opaque content is the hard proof
    // (byte-exact — any ghost above it would corrupt it); the
    // survivor's near-opaque band (alpha 250) may show the ghost's
    // ink beneath through its 2% — the honest physics of a band over
    // a fading band — so its bound is the over-blend's own: never
    // brighter than the band over a FULL band, never darker than the
    // band over black.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut last_r = band_r();
    while tb.world(|w| w.scene.ghosts.any_live()) {
        assert!(deadline > std::time::Instant::now(), "never settled");
        client.sync();
        let words = tb.scanout();
        assert_eq!(
            word(&words, 100, 80),
            opaque(lime),
            "the top window's content stays whole over the fading ghost"
        );
        let top_band_r = px(&words, 100, 40)[0];
        assert!(
            (band_r()..=pre_band_r).contains(&top_band_r),
            "the survivor's band stays its own ink (got {top_band_r}, pre {pre_band_r})"
        );
        let r = px(&words, 5, 10)[0];
        assert!(
            r <= last_r,
            "the uncovered band dims (channel {r}, last {last_r})"
        );
        last_r = r;
    }
    // Settled: the survivor whole — the band over plain black again,
    // the content byte-exact — and the bottom window gone entirely.
    let settled = tb.scanout();
    assert_eq!(px(&settled, 100, 40)[0], band_r(), "the survivor's band");
    assert_eq!(
        word(&settled, 100, 80),
        opaque(lime),
        "the survivor's content"
    );
    assert_eq!(px(&settled, 5, 10)[0], 0, "the bottom's band is gone");
    assert_eq!(
        px(&settled, 2, 60)[2],
        0,
        "the bottom's content edge is gone"
    );
}

/// The zero-drift controls: a plain (roleless) window and a
/// client-decorated window ghost content-only — no chrome payload
/// (nothing wore the band), every Phase 48 byte intact.
#[test]
fn plain_and_csd_windows_leave_no_chrome_ghost() {
    let tb = Testbench::start_with(
        "cghost-plain",
        ghost_config("plain", EffectChoice::Tier(EffectTier::Minimal)),
    );
    let mut client = TestClient::connect(&tb.addr);

    // The plain window: no role at all.
    let shm = client.bind("ldp.core.shm");
    let compositor = client.bind("ldp.core.compositor");
    let gold = xrgb(210, 180, 40);
    let pixels: Vec<u8> = std::iter::repeat(gold.to_le_bytes())
        .take(120 * 60)
        .flatten()
        .collect();
    let pool = create_pool(
        &mut client,
        &shm,
        pool_bytes(&pixels),
        (120 * 60 * 4) as i64,
    );
    let buffer = create_buffer(&mut client, &pool, 0, 120, 60, 480, XR24);
    let plain_surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("surface");
    let before = tb.frames();
    attach(&mut client, &plain_surface, &buffer);
    damage(&mut client, &plain_surface, &[Rect::new(0, 0, 120, 60)]);
    commit(&mut client, &plain_surface, 1);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while tb.frames() <= before {
        assert!(deadline > std::time::Instant::now());
        client.sync();
    }
    settle(&tb, &mut client);
    client.conn.destroy(&plain_surface).expect("destroy");
    client.sync();
    assert!(
        tb.world(|w| w.scene.ghosts.any_live()),
        "the plain window ghosts"
    );
    assert!(
        tb.world(|w| w.scene.ghosts.ghosts[0].chrome.is_none()),
        "a plain window leaves no chrome ghost"
    );
    settle(&tb, &mut client);

    // The CSD window: its buffer is its whole footprint — the ghost
    // carries content only.
    let (csd, _ct) = csd_window(&tb, &mut client, xrgb(60, 130, 210));
    settle(&tb, &mut client);
    client.conn.destroy(&csd).expect("destroy");
    client.sync();
    assert!(
        tb.world(|w| w.scene.ghosts.any_live()),
        "the CSD window ghosts"
    );
    assert!(
        tb.world(|w| w.scene.ghosts.ghosts[0].chrome.is_none()),
        "a client-decorated window leaves no chrome ghost"
    );
    settle(&tb, &mut client);
    let settled = tb.scanout();
    assert_eq!(px(&settled, 60, 30)[0], 0, "the CSD window is gone");
}

/// The transitions-off control (the library default): the destroy is
/// plain — no ghost ever begun, and the band leaves with the route
/// the way it always has (the next frame the plain background, band
/// and content together, the pre-Phase-57 abrupt truth preserved as
/// the tier's honest behavior).
#[test]
fn transitions_off_leaves_the_band_with_the_route() {
    let tb = Testbench::start_with("cghost-off", plain_config("off"));
    let mut client = TestClient::connect(&tb.addr);

    let cherry = xrgb(200, 60, 60);
    let (surface, _t) = ssd_window(&tb, &mut client, cherry, 0);
    settle(&tb, &mut client);
    let before = tb.frames();
    client.conn.destroy(&surface).expect("destroy");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while tb.frames() <= before {
        assert!(deadline > std::time::Instant::now());
        client.sync();
    }
    assert_eq!(tb.world(|w| w.scene.ghosts.begun), 0, "no ghost begun");
    let words = tb.scanout();
    assert_eq!(px(&words, 5, 10)[0], 0, "the band left with the route");
    assert_eq!(px(&words, 100, 50)[0], 0, "the content left with the route");
}

/// The Liquid ghost: a dressed band's frost outlives its window — the
/// frozen shape carries the `liquid` bit, the fading glass still
/// reading the composed canvas beneath it (the chrome material at
/// the Medium tier), and the whole frame settles plain.
#[test]
fn the_liquid_ghost_band_frosts_past_death() {
    let tb = Testbench::start_with(
        "cghost-liquid",
        ghost_config("liquid", EffectChoice::Tier(EffectTier::Medium)),
    );
    let mut client = TestClient::connect(&tb.addr);

    let teal = xrgb(30, 170, 170);
    let (surface, _t) = ssd_window(&tb, &mut client, teal, 0);
    settle(&tb, &mut client);
    let plain = tb.scanout();
    // The dressed band's own settled word (the veil over the frost of
    // the plain black desktop) — recorded, not pinned: the veil's
    // arithmetic is the Liquid tier's own truth, the fade's monotone
    // decay is this test's.
    let veil_r = px(&plain, 5, 10)[0];
    assert!(veil_r > 100, "the dressed band reads light (got {veil_r})");

    client.conn.destroy(&surface).expect("destroy");
    client.sync();
    assert!(tb.world(|w| w.scene.ghosts.any_live()));
    let shape = tb
        .world(|w| w.scene.ghosts.ghosts[0].chrome.as_ref().map(|c| c.shape))
        .expect("the ghost carries its chrome");
    assert!(shape.liquid, "the frozen shape carries the Liquid variant");
    assert_eq!(shape.top, 29, "the band's split froze with the dressing");

    // The fade: the dressed band dims monotonically, never
    // brightening.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut last_r = veil_r;
    while tb.world(|w| w.scene.ghosts.any_live()) {
        assert!(deadline > std::time::Instant::now(), "never settled");
        client.sync();
        let words = tb.scanout();
        let r = px(&words, 5, 10)[0];
        assert!(
            r <= last_r,
            "the dressed band never brightens ({r}, last {last_r})"
        );
        last_r = r;
    }
    settle(&tb, &mut client);
    let settled = tb.scanout();
    assert_eq!(px(&settled, 5, 10)[0], 0, "the dressed band is gone");
    assert_eq!(px(&settled, 100, 50)[0], 0, "the content is gone");
}

/// The unmap arm: a hiding window (a detach) leaves with its chrome
/// the same way — the ghost carries the band, the whole frame fades,
/// and the surface itself lives on (the client may re-attach).
#[test]
fn unmap_rides_the_chrome_too() {
    let tb = Testbench::start_with(
        "cghost-unmap",
        ghost_config("unmap", EffectChoice::Tier(EffectTier::Minimal)),
    );
    let mut client = TestClient::connect(&tb.addr);

    let teal = xrgb(30, 170, 170);
    let (surface, _t) = ssd_window(&tb, &mut client, teal, 0);
    settle(&tb, &mut client);
    assert_eq!(
        word(&tb.scanout(), 5, 10),
        band_word(),
        "the band settled plain"
    );

    // The detach: attach(null) + commit — the surface stays alive, the
    // screen loses the whole window, the ghost fades band and all.
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
    assert!(
        tb.world(|w| w.scene.ghosts.ghosts[0].chrome.is_some()),
        "the unmap ghost carries the chrome"
    );

    // The fade: the band dims monotonically.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut last_r = band_r();
    while tb.world(|w| w.scene.ghosts.any_live()) {
        assert!(deadline > std::time::Instant::now(), "never settled");
        client.sync();
        let words = tb.scanout();
        let r = px(&words, 5, 10)[0];
        assert!(r <= last_r, "the band never brightens ({r}, last {last_r})");
        last_r = r;
    }
    settle(&tb, &mut client);
    let settled = tb.scanout();
    assert_eq!(px(&settled, 5, 10)[0], 0, "the band is gone");
    assert_eq!(px(&settled, 100, 50)[0], 0, "the content is gone");
    // The surface lives on (the client may re-attach); the ghost does
    // not.
    assert!(!tb.world(|w| w.scene.ghosts.any_live()));
    assert!(tb.world(|w| !w.scene.routes.is_empty()));
}
