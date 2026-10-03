//! Phase 55's exit criterion (the Liquid chrome-material dressing):
//! the drawn band wears the system's own glass, served end to end
//! through the real protocol at a Liquid tier —
//!
//! * the **glass band**: a server-decorated window's title bar renders
//!   as the chrome material — the frost pane beneath the veil, the
//!   chrome hairline over the frame's top edge — every word pinned
//!   against the world's own material builder and over-rule (zero
//!   drift by construction);
//! * the **backdrop read**: the band's bytes depend on what sits
//!   *beneath* it — a red underlay warms the glass, a blue one cools
//!   it, and the frost's blur genuinely mixes the boundary between
//!   them (the glass proof);
//! * the **underlay change**: a commit beneath the band flows through
//!   the glass — the claim propagates, the frost rebuilds, the band's
//!   bytes move;
//! * the **Minimal freeze**: the same scene at the Minimal tier keeps
//!   every Phase 52-54 byte (the flat bar, the square corners — the
//!   tier's honesty, never a lie);
//! * the **pane's tenant**: the client's content rides *above* the
//!   pane untouched (the frost never leaks into opaque content), the
//!   close capsule stays its opaque warm self, the title strip still
//!   reads over the glass;
//! * the **readability floor**: the veil anchors the band's lightness
//!   over any backdrop — the dark title ink keeps its contrast.

mod testbench;

use ldp_client::Proxy;
use ldp_core::geometry::Rect;
use ldp_core::wire::Value;
use ldp_renderer::{over_premul, EdgeLightParams, EffectChoice, EffectTier};
use testbench::*;

use lion_compositor::server::CompositorConfig;
use lion_compositor::shell::{
    self, DockMode, ShellConfig, BAND_ALPHA, BAND_ALPHA_LIQUID, BAND_RGB,
};

/// The XRGB8888 fourcc ("XR24").
const XR24: u32 = 0x3432_5258;
/// The mock output: 1920x1080@60.
const OUT_W: usize = 1920;

/// The tests' window geometry: the SSD content 200x100 at the
/// cascade's third step (Phase 56: the frame takes the slot — content
/// (49,77), frame (48,48,202,102)) — fully on-screen, the band's 29
/// rows clean of the content's own shadow.
const FRAME: Rect = Rect {
    x: 48,
    y: 48,
    w: 202,
    h: 102,
};

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

/// The tests' configs: dock auto (far from every window), the effects
/// tier explicit — Medium, the Liquid tier the oracles pin (the
/// default headless choice stays Minimal, the freeze the Minimal test
/// proves).
fn liquid_config(tag: &str, tier: EffectChoice) -> CompositorConfig {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    CompositorConfig {
        shell: ShellConfig {
            dock: DockMode::Auto,
            ..ShellConfig::default()
        },
        effects: tier,
        socket: format!("lion-liquid-{tag}-{n}-{}", std::process::id()),
        ..CompositorConfig::default()
    }
}

/// A memfd carrying `pixels` (the pool backing).
fn pool_bytes(pixels: &[u8]) -> std::os::fd::OwnedFd {
    use std::io::Write as _;
    let fd = lion_compositor::sys::memfd("liquid-pool").expect("memfd");
    {
        let mut file = std::fs::File::from(fd.try_clone().expect("clone"));
        file.write_all(pixels).expect("write pool");
    }
    fd
}

/// Map one plain window at its creation position (the cascade's filler
/// rows), the pixels a caller-owned generator (the split-buffer
/// underlays).
fn map_pixels(tb: &Testbench, client: &mut TestClient, pixels: &[u8], w: usize, h: usize) -> Proxy {
    let shm = client.bind("ldp.core.shm");
    let compositor = client.bind("ldp.core.compositor");
    let pool = create_pool(client, &shm, pool_bytes(pixels), (w * h * 4) as i64);
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

/// A plain window of one uniform fill.
fn map_plain(tb: &Testbench, client: &mut TestClient, fill: u32, w: usize, h: usize) -> Proxy {
    let pixels: Vec<u8> = std::iter::repeat(fill.to_le_bytes())
        .take(w * h)
        .flatten()
        .collect();
    map_pixels(tb, client, &pixels, w, h)
}

/// One SSD window realized (the title_session recipe, verbatim) — the
/// toplevel minted before the mapping commit, the handshake acked,
/// the first commit mapping at the cascade's `nth` step with the
/// insets applied. Returns (surface, toplevel). 200x100 content.
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

// ---------------------------------------------------------------------------
// The oracle kit (zero drift: the world's own resolution, the world's
// own material builder, the renderer's own over-rule)
// ---------------------------------------------------------------------------

/// The dressed band's veil layer bytes: the band tint at the Liquid
/// alpha, premultiplied, little-endian `[B, G, R, A]`.
fn veil_layer() -> [u8; 4] {
    let m = |v: u8| (u32::from(v) * u32::from(BAND_ALPHA_LIQUID) + 127) / 255;
    [
        m(BAND_RGB[2]) as u8,
        m(BAND_RGB[1]) as u8,
        m(BAND_RGB[0]) as u8,
        BAND_ALPHA_LIQUID,
    ]
}

/// The flat bar's layer bytes (the Minimal oracle's own).
fn flat_layer() -> [u8; 4] {
    let m = |v: u8| (u32::from(v) * u32::from(BAND_ALPHA) + 127) / 255;
    [
        m(BAND_RGB[2]) as u8,
        m(BAND_RGB[1]) as u8,
        m(BAND_RGB[0]) as u8,
        BAND_ALPHA,
    ]
}

/// The chrome hairline's layer bytes (white at the chrome stroke's
/// alpha, premultiplied).
fn edge_layer() -> [u8; 4] {
    let e = EdgeLightParams::chrome();
    let m = |v: u8| (u32::from(v) * u32::from(e.alpha) + 127) / 255;
    [
        m(e.color[2]) as u8,
        m(e.color[1]) as u8,
        m(e.color[0]) as u8,
        e.alpha,
    ]
}

/// The frost material over the pane for a given backdrop: the world's
/// own chrome-material resolution at Medium, built by the world's own
/// `frost_material` from the very words the canvas held.
fn frost_over(backdrop: &[u32]) -> Vec<u32> {
    let params = shell::chrome_style(EffectTier::Medium)
        .backdrop
        .expect("Medium dresses the chrome");
    ldp_renderer::frost_material(backdrop, FRAME.w, FRAME.h, &params)
}

/// The pane's saved backdrop words for a desktop of `under`: the
/// opaque word under every pixel of the frame (the uniform-underlay
/// tests' saved, exactly what the canvas held beneath the chrome).
fn saved_uniform(under: u32) -> Vec<u32> {
    vec![under; (FRAME.w as usize) * (FRAME.h as usize)]
}

/// The expected clean-row glass word: the veil over the frost word.
fn glass_word(frost_word: u32) -> u32 {
    u32::from_le_bytes(over_premul(veil_layer(), frost_word.to_le_bytes(), 255))
}

/// The expected hairline-row word: the hairline over the veil over
/// the frost word.
fn hairline_word(frost_word: u32) -> u32 {
    let glass = over_premul(veil_layer(), frost_word.to_le_bytes(), 255);
    u32::from_le_bytes(over_premul(edge_layer(), glass, 255))
}

/// One frame-local coordinate into a pane-sized vector.
fn pane_idx(x: i32, y: i32) -> usize {
    ((y - FRAME.y) as usize) * FRAME.w as usize + (x - FRAME.x) as usize
}

/// Two filler windows (the cascade's rows 0 and 1) so the SSD window
/// maps at the second step, its frame fully on-screen.
fn fillers(tb: &Testbench, client: &mut TestClient) {
    let _ = map_plain(tb, client, xrgb(20, 20, 24), 8, 8);
    let _ = map_plain(tb, client, xrgb(24, 24, 30), 8, 8);
}

/// THE glass oracle: the dressed band over the bare desktop — the
/// frost over the opaque-black pane, the veil over the frost, the
/// hairline over the veil, the corners cut to the backdrop.
#[test]
fn the_band_wears_glass_over_the_desktop() {
    let tb = Testbench::start_with(
        "liquid-desk",
        liquid_config("desk", EffectChoice::Tier(EffectTier::Medium)),
    );
    let mut client = TestClient::connect(&tb.addr);
    fillers(&tb, &mut client);
    let (w, _t) = ssd_window(&tb, &mut client, xrgb(120, 130, 145), 2);
    let _ = w;

    // The pane's saved backdrop: the opaque black desktop everywhere
    // (the fillers sit outside the frame, the content has not drawn
    // beneath its own chrome).
    let frost = frost_over(&saved_uniform(0xFF00_0000));
    let words = tb.scanout();

    // A clean row (row 11 of the band: below the hairline, above the
    // content's shadow box): the veil over the frost.
    let clean = (61i32, 59i32);
    assert_eq!(
        word(&words, clean.0 as usize, clean.1 as usize),
        glass_word(frost[pane_idx(clean.0, clean.1)]),
        "the clean glass row"
    );
    // Another clean point near the frame's right (left of the close
    // capsule's territory, x < 225).
    let clean2 = (201i32, 59i32);
    assert_eq!(
        word(&words, clean2.0 as usize, clean2.1 as usize),
        glass_word(frost[pane_idx(clean2.0, clean2.1)]),
        "the clean glass row, right side"
    );
    // The hairline row: the chrome stroke over the veil over the
    // frost, the light catch on the frame's top edge.
    let top = (100, FRAME.y as usize);
    assert_eq!(
        word(&words, top.0, top.1),
        hairline_word(frost[pane_idx(top.0 as i32, top.1 as i32)]),
        "the hairline row"
    );
    // The extreme corner: every coverage is zero there (the frost's
    // corner fold, the ink's fold, the hairline's erosion) — the
    // desktop's own black shows through the glass cut.
    assert_eq!(
        word(&words, FRAME.x as usize, FRAME.y as usize),
        0xFF00_0000,
        "the corner cuts to the backdrop"
    );
}

/// THE glass proof: the band's bytes depend on what sits beneath —
/// a red half warms the glass, a blue half cools it, and the frost's
/// blur mixes the boundary between them (a red|blue split underlay,
/// every word exact).
#[test]
fn the_glass_reads_its_backdrop() {
    let tb = Testbench::start_with(
        "liquid-read",
        liquid_config("read", EffectChoice::Tier(EffectTier::Medium)),
    );
    let mut client = TestClient::connect(&tb.addr);
    // The underlay: 500x200 at the origin, red left of x=150, blue
    // right of it — the split crosses the frame's whole width, the
    // blur's reach straddles the boundary.
    let (uw, uh) = (500usize, 200usize);
    let red = xrgb(200, 40, 40);
    let blue = xrgb(40, 60, 200);
    let mut pixels: Vec<u8> = Vec::with_capacity(uw * uh * 4);
    for _y in 0..uh {
        for x in 0..uw {
            let fill = if x < 150 { red } else { blue };
            pixels.extend_from_slice(&fill.to_le_bytes());
        }
    }
    let _under = map_pixels(&tb, &mut client, &pixels, uw, uh);
    // The cascade's spacer: the SSD window maps as the third
    // window (the second step), its frame fully on-screen.
    let _ = map_plain(&tb, &mut client, xrgb(28, 28, 34), 8, 8);
    let (w, _t) = ssd_window(&tb, &mut client, xrgb(120, 130, 145), 2);
    let _ = w;

    // The saved backdrop: the underlay's word under every pane pixel.
    let saved: Vec<u32> = (FRAME.y..FRAME.bottom())
        .flat_map(|_y| (FRAME.x..FRAME.right()).map(|x| opaque(if x < 150 { red } else { blue })))
        .collect();
    let frost = frost_over(&saved);
    let words = tb.scanout();

    // The pure-red side: the veil over the frost of red.
    let p = (61i32, 59i32);
    assert_eq!(
        word(&words, p.0 as usize, p.1 as usize),
        glass_word(frost[pane_idx(p.0, p.1)]),
        "the glass over the red half"
    );
    // The pure-blue side.
    let q = (201i32, 59i32);
    assert_eq!(
        word(&words, q.0 as usize, q.1 as usize),
        glass_word(frost[pane_idx(q.0, q.1)]),
        "the glass over the blue half"
    );
    // The boundary itself: the frost's blur mixing the two halves —
    // the word only matches if the pane really sampled the canvas.
    let b = (151i32, 59i32);
    assert_eq!(
        word(&words, b.0 as usize, b.1 as usize),
        glass_word(frost[pane_idx(b.0, b.1)]),
        "the glass at the red-blue boundary (the blur's mix)"
    );
    // And the halves read differently — the glass is genuinely
    // backdrop-dependent.
    assert_ne!(
        word(&words, p.0 as usize, p.1 as usize),
        word(&words, q.0 as usize, q.1 as usize),
        "the band reads its underlay"
    );
    // The corner cut over the underlay: the underlay's own word.
    assert_eq!(
        word(&words, FRAME.x as usize, FRAME.y as usize),
        opaque(red),
        "the corner cuts to the red underlay"
    );
}

/// THE propagation oracle: a commit beneath the band flows through
/// the glass — the underlay's change claims the overlap, the frost
/// rebuilds over the fresh canvas, the band's bytes move to the new
/// glass.
#[test]
fn the_underlay_change_flows_through_the_glass() {
    let tb = Testbench::start_with(
        "liquid-flow",
        liquid_config("flow", EffectChoice::Tier(EffectTier::Medium)),
    );
    let mut client = TestClient::connect(&tb.addr);
    let (uw, uh) = (500usize, 200usize);
    let first = xrgb(200, 40, 40);
    let under = map_plain(&tb, &mut client, first, uw, uh);
    // The cascade's spacer: the SSD window maps as the third
    // window (the second step), its frame fully on-screen.
    let _ = map_plain(&tb, &mut client, xrgb(28, 28, 34), 8, 8);
    let (w, _t) = ssd_window(&tb, &mut client, xrgb(120, 130, 145), 2);
    let _ = w;

    // The first glass, exact.
    let frost_before = frost_over(&saved_uniform(opaque(first)));
    let p = (101i32, 59i32);
    let before = word(&tb.scanout(), p.0 as usize, p.1 as usize);
    assert_eq!(before, glass_word(frost_before[pane_idx(p.0, p.1)]));

    // The change: a fresh buffer under the same window — a full
    // damage that covers the band's whole pane.
    let second = xrgb(40, 200, 90);
    let shm = client.bind("ldp.core.shm");
    let pixels: Vec<u8> = std::iter::repeat(second.to_le_bytes())
        .take(uw * uh)
        .flatten()
        .collect();
    let pool = create_pool(&mut client, &shm, pool_bytes(&pixels), (uw * uh * 4) as i64);
    let buffer = create_buffer(
        &mut client,
        &pool,
        0,
        uw as i32,
        uh as i32,
        (uw * 4) as i32,
        XR24,
    );
    let frames_before = tb.frames();
    attach(&mut client, &under, &buffer);
    damage(
        &mut client,
        &under,
        &[Rect::new(0, 0, uw as u32, uh as u32)],
    );
    commit(&mut client, &under, 1);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while tb.frames() <= frames_before {
        assert!(
            deadline > std::time::Instant::now(),
            "the change never rendered"
        );
        client.sync();
    }

    // The new glass, exact — the claim propagated, the frost rebuilt.
    let frost_after = frost_over(&saved_uniform(opaque(second)));
    let after = word(&tb.scanout(), p.0 as usize, p.1 as usize);
    assert_eq!(after, glass_word(frost_after[pane_idx(p.0, p.1)]));
    assert_ne!(after, before, "the band's bytes moved with the underlay");
}

/// THE Minimal freeze: the same scene at the headless default tier
/// keeps every Phase 52-54 byte — the flat near-opaque bar over the
/// underlay, the square corners, no frost, no hairline.
#[test]
fn the_minimal_tier_keeps_the_flat_bar() {
    let tb = Testbench::start_with(
        "liquid-min",
        liquid_config("min", EffectChoice::Tier(EffectTier::Minimal)),
    );
    let mut client = TestClient::connect(&tb.addr);
    let under = xrgb(90, 30, 140);
    let _u = map_plain(&tb, &mut client, under, 500, 200);
    // The cascade's spacer: the SSD window maps as the third
    // window (the second step), its frame fully on-screen.
    let _ = map_plain(&tb, &mut client, xrgb(28, 28, 34), 8, 8);
    let (w, _t) = ssd_window(&tb, &mut client, xrgb(120, 130, 145), 2);
    let _ = w;

    let words = tb.scanout();
    // The flat bar over the underlay (the v0.22.0 oracle's own
    // arithmetic, byte for byte).
    let flat_over = u32::from_le_bytes(over_premul(flat_layer(), opaque(under).to_le_bytes(), 255));
    assert_eq!(word(&words, 61, 59), flat_over, "the flat bar stands");
    assert_eq!(
        word(&words, 201, 59),
        flat_over,
        "the flat bar stands, right side"
    );
    // The corners: square ink, no rounding, no cut.
    assert_eq!(
        word(&words, FRAME.x as usize, FRAME.y as usize),
        flat_over,
        "the corner is the bar's own ink (no rounding at Minimal)"
    );
    // The dressing's veil never painted: the two alphas differ.
    assert_ne!(BAND_ALPHA, BAND_ALPHA_LIQUID);
}

/// THE pane's tenant: the client's content rides above the glass
/// untouched — the frost pane replaced the hole's canvas, the opaque
/// content covered it back, every content pixel the buffer's own.
#[test]
fn the_content_rides_above_the_pane() {
    let tb = Testbench::start_with(
        "liquid-hole",
        liquid_config("hole", EffectChoice::Tier(EffectTier::Medium)),
    );
    let mut client = TestClient::connect(&tb.addr);
    let under = xrgb(200, 40, 40);
    let _u = map_plain(&tb, &mut client, under, 500, 200);
    let fill = xrgb(120, 130, 145);
    // The cascade's spacer: the SSD window maps as the third
    // window (the second step), its frame fully on-screen.
    let _ = map_plain(&tb, &mut client, xrgb(28, 28, 34), 8, 8);
    let (w, _t) = ssd_window(&tb, &mut client, fill, 2);
    let _ = w;

    let words = tb.scanout();
    // Interior content pixels (clear of the content's own rounded
    // corners): the buffer's fill exactly, the frost beneath them
    // invisible.
    for (x, y) in [(101, 89), (151, 129), (201, 109), (121, 169)] {
        assert_eq!(
            word(&words, x, y),
            opaque(fill),
            "the content pixel at ({x},{y})"
        );
    }
    // The glass and the content differ (the pane never bled upward).
    let frost = frost_over(&saved_uniform(opaque(under)));
    assert_ne!(
        word(&words, 101, 59),
        opaque(fill),
        "the band above the content is glass, not content"
    );
    assert_eq!(
        word(&words, 101, 59),
        glass_word(frost[pane_idx(101, 59)]),
        "and it is the exact glass"
    );
}

/// THE farewell grip: the close capsule stays its opaque warm self
/// over the glass — the dressing dresses the band, the button stays a
/// button.
#[test]
fn the_close_capsule_stays_opaque_over_the_glass() {
    let tb = Testbench::start_with(
        "liquid-close",
        liquid_config("close", EffectChoice::Tier(EffectTier::Medium)),
    );
    let mut client = TestClient::connect(&tb.addr);
    let _u = map_plain(&tb, &mut client, xrgb(200, 40, 40), 500, 200);
    // The cascade's spacer: the SSD window maps as the third
    // window (the second step), its frame fully on-screen.
    let _ = map_plain(&tb, &mut client, xrgb(28, 28, 34), 8, 8);
    let (w, _t) = ssd_window(&tb, &mut client, xrgb(120, 130, 145), 2);
    let _ = w;

    let words = tb.scanout();
    // Inside the capsule, off the × glyph's strokes: the opaque warm
    // word exactly (frame-relative (184,23): inside the circle
    // (dx=-3, dy=8, dist^2=73 <= 100), off both diagonals (d1=-11,
    // d2=+5, both beyond the 2px stroke)).
    let close = opaque(xrgb(226, 88, 76));
    assert_eq!(
        word(&words, (FRAME.x + 184) as usize, (FRAME.y + 23) as usize),
        close,
        "the capsule's ink"
    );
}

/// THE strip over the glass: the title still reads — the glyph ink
/// over the veil over the frost, every word exact (the Phase 54
/// oracle's arithmetic over the Phase 55 glass).
#[test]
fn the_title_strip_still_reads_over_the_glass() {
    let tb = Testbench::start_with(
        "liquid-strip",
        liquid_config("strip", EffectChoice::Tier(EffectTier::Medium)),
    );
    let mut client = TestClient::connect(&tb.addr);
    let under = xrgb(40, 60, 200);
    let _u = map_plain(&tb, &mut client, under, 500, 200);
    // The titled SSD window (the title_session recipe).
    // The cascade's spacer: the SSD window maps as the third
    // window (the second step), its frame fully on-screen.
    let _ = map_plain(&tb, &mut client, xrgb(28, 28, 34), 8, 8);
    let (w, _t) = titled_window(&tb, &mut client, xrgb(120, 130, 145), 2, "First");

    // The strip's truth from the world.
    let rect = strip_truth(&tb, &w);
    // The sample inside the 'F's top bar (the title_session point).
    let sample = (rect.x + 4, rect.y + 2);
    // The strip's ink at that pixel, from the face alone.
    let ink = expected_strip_word("First", 4, 2);
    // The expected: the ink over the veil over the frost of the
    // underlay.
    let frost = frost_over(&saved_uniform(opaque(under)));
    let glass = over_premul(
        veil_layer(),
        frost[pane_idx(sample.0, sample.1)].to_le_bytes(),
        255,
    );
    let expect = u32::from_le_bytes(over_premul(ink.to_le_bytes(), glass, 255));
    assert_eq!(
        word(&tb.scanout(), sample.0 as usize, sample.1 as usize),
        expect,
        "the glyph ink over the glass"
    );
    // The margin left of the strip: the glass alone (no ink).
    assert_eq!(
        word(&tb.scanout(), (rect.x - 2) as usize, (rect.y + 2) as usize),
        glass_word(frost[pane_idx(rect.x - 2, rect.y + 2)]),
        "the strip's margin is the glass alone"
    );
}

/// THE readability floor: the veil anchors the band's lightness over
/// any backdrop — the dark title ink keeps its contrast wherever the
/// window lands (the design's stated invariant, computed from the
/// same zero-drift helpers).
#[test]
fn the_veil_keeps_the_band_readable_over_any_backdrop() {
    // Over black, over red, over blue, over white: the clean-row
    // glass word's BT.601 luma stays light (the title ink's ~30 luma
    // keeps its contrast).
    for under in [
        0xFF00_0000u32,
        opaque(xrgb(200, 40, 40)),
        opaque(xrgb(40, 60, 200)),
        0xFFFF_FFFF,
    ] {
        let frost = frost_over(&saved_uniform(under));
        let g = glass_word(frost[pane_idx(101, 59)]);
        let r = (g >> 16) & 0xFF;
        let gg = (g >> 8) & 0xFF;
        let b = g & 0xFF;
        let luma = (74u32 * r + 150 * gg + 28 * b) / 252;
        assert!(
            luma >= 140,
            "the band stays light over {under:#010x} (luma {luma})"
        );
    }
}

// ---- the titled-window helpers (title_session's own, compact) ------

/// One SSD window with its title set before the mapping commit.
fn titled_window(
    tb: &Testbench,
    client: &mut TestClient,
    fill: u32,
    nth: usize,
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
    // Phase 56 — the chrome-aware placement: the frame takes the
    // slot; the content rides inside at the slot + insets (the band
    // on-screen from the first frame).
    let step = 24 * nth as i32;
    assert_eq!(position_of(tb, &surface), (step + 1, step + 29));
    (surface, toplevel)
}

/// The strip's live rect from the world (the one truth).
fn strip_truth(tb: &Testbench, surface: &Proxy) -> Rect {
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
            ldp_core::scale::ScaleFactor::IDENTITY,
        )
        .expect("the strip serves")
        .1
    })
}

/// The strip's expected ink word at one glyph-box pixel (the
/// title_session oracle's arithmetic, one point).
fn expected_strip_word(title: &str, x: u32, y: u32) -> u32 {
    use ldp_font::LION_SANS;
    use lion_compositor::shell::TITLE_RGB;
    let avail = 202
        - ldp_shell::ssd::TITLE_SIDE
        - (ldp_shell::ssd::CLOSE_SIZE + 2 * ldp_shell::ssd::CLOSE_MARGIN);
    let run = LION_SANS.layout(title, ldp_shell::ssd::TITLE_PX, avail);
    let ink = run.ink.expect("the oracle's title inks");
    let strip_w = ink.width().min(avail).max(1);
    let _ = strip_w;
    let mut word = 0u32;
    for placed in &run.placed {
        let glyph = LION_SANS.bitmap(placed.c, ldp_shell::ssd::TITLE_PX);
        if glyph.w == 0 || glyph.h == 0 {
            continue;
        }
        let glyph_x = placed.x + glyph.x_off - ink.min_x;
        let glyph_y = ink.top - glyph.y_top;
        let gx = i64::from(x) - i64::from(glyph_x);
        let gy = i64::from(y) - i64::from(glyph_y);
        if gx < 0 || gy < 0 || gx >= i64::from(glyph.w) || gy >= i64::from(glyph.h) {
            continue;
        }
        let coverage = glyph.alpha[gy as usize * glyph.w as usize + gx as usize];
        if coverage > 0 {
            let scaled = |chan: u8| (u32::from(chan) * u32::from(coverage) + 127) / 255;
            word = u32::from(coverage) << 24
                | scaled(TITLE_RGB[0]) << 16
                | scaled(TITLE_RGB[1]) << 8
                | scaled(TITLE_RGB[2]);
        }
    }
    word
}
