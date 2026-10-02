//! Phase 47's exit criterion (the choreography half): the
//! compositor-owned window-open fade, served end to end through the
//! real protocol —
//!
//! * a freshly mapped window renders its **first frame at the fade's
//!   start** (the blend over the black background — not the plain
//!   pixels), advances at the pump's cadence, and **settles to the
//!   exact plain bytes** (the never-animated oracle, byte-for-byte —
//!   every pixel oracle the equivalence corpora pin keeps its
//!   meaning);
//! * a surface claiming an ephemeral role (tooltip) appears
//!   instantly — the catalog never animates it in;
//! * the library default (transitions off) keeps every byte plain
//!   from the first frame — the byte-exactness doctrine the 25
//!   sibling suites pin implicitly.

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
    let fd = lion_compositor::sys::memfd("transition-pool").expect("memfd");
    {
        let mut file = std::fs::File::from(fd.try_clone().expect("clone"));
        file.write_all(pixels).expect("write pool");
    }
    fd
}

/// One scanout pixel as an RGBA quadruple.
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
fn transitions_config() -> CompositorConfig {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    CompositorConfig {
        shell: ShellConfig {
            dock: DockMode::Off,
            ..ShellConfig::default()
        },
        transitions: true,
        socket: format!("lion-transitions-{n}-{}", std::process::id()),
        ..CompositorConfig::default()
    }
}

/// `shell.get_toplevel(surface, client-decorations)`.
fn get_toplevel(client: &mut TestClient, shell: &Proxy, surface: &Proxy) -> Proxy {
    client
        .conn
        .create_object(
            shell,
            "get_toplevel",
            vec![Value::Object(Some(surface.id())), Value::Enum(2)],
        )
        .expect("get_toplevel")
}

/// Attach + damage + commit, then pump until the compositor rendered
/// once (the mapping frame — the fade's first sample).
fn present_first(tb: &Testbench, client: &mut TestClient, surface: &Proxy, buffer: &Proxy) {
    let before = tb.frames();
    attach(client, surface, buffer);
    damage(client, surface, &[Rect::new(0, 0, 200, 100)]);
    commit(client, surface, 1);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while tb.frames() <= before {
        assert!(
            deadline > std::time::Instant::now(),
            "the compositor never rendered the buffer"
        );
        client.sync();
    }
}

/// Pump until every live transition settles (the 10 s deadline the
/// sibling suites share). The settle's own frame (the
/// exact-terminal render) lands inside the same pump that removed
/// the transition — the advance that settles still claims its dirty
/// frame, so the loop's exit IS the final frame's landing.
fn settle(tb: &Testbench, client: &mut TestClient) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while tb.world(|w| w.scene.transitions.any_live()) {
        assert!(
            deadline > std::time::Instant::now(),
            "the fade never settled"
        );
        client.sync();
    }
}

/// One window's lifetime under the choreography: map → fade from the
/// black background → settle to the exact plain bytes.
#[test]
fn the_window_open_fade_settles_to_the_exact_bytes() {
    let tb = Testbench::start_with("transition-fade", transitions_config());
    let mut client = TestClient::connect(&tb.addr);

    let shm = client.bind("ldp.core.shm");
    let compositor = client.bind("ldp.core.compositor");
    let shell = client.bind("ldp.shell.shell");
    client.bind("ldp.core.output");

    // A 200x100 cherry window at the cascade's first step.
    let cherry = xrgb(200, 60, 60);
    let pixels: Vec<u8> = std::iter::repeat(cherry.to_le_bytes())
        .take(200 * 100)
        .flatten()
        .collect();
    let pool = create_pool(
        &mut client,
        &shm,
        pool_bytes(&pixels),
        (200 * 100 * 4) as i64,
    );
    let buffer = create_buffer(&mut client, &pool, 0, 200, 100, 800, XR24);
    let surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("surface");
    let _toplevel = get_toplevel(&mut client, &shell, &surface);

    // The mapping frame: the fade's start — the window's ink at the
    // spring's opening position over the black background: near-black
    // ink (a whisper of cherry from the first pump-wakes' integration),
    // demonstrably NOT the plain pixels — the choreography ran.
    present_first(&tb, &mut client, &surface, &buffer);
    {
        let scanout = tb.scanout();
        let first = px(&scanout, 50, 30);
        assert!(
            first != [200, 60, 60, 255],
            "the first frame is not the plain ink (the fade demonstrably ran)"
        );
        assert!(
            first[0] < 50 && first[1] < 50 && first[2] < 50,
            "the first frame is the fade's near-black start (got {first:?})"
        );
    }
    // And the host confirms the motion began.
    assert!(tb.world(|w| w.scene.transitions.any_live()));
    assert_eq!(tb.world(|w| w.scene.transitions.begun), 1);

    // The fade advances at the pump's cadence and settles.
    settle(&tb, &mut client);

    // THE oracle: the settled frame is byte-identical with the
    // never-animated one — the plain ink, exactly.
    {
        let scanout = tb.scanout();
        assert_eq!(
            px(&scanout, 50, 30),
            [200, 60, 60, 255],
            "the settled frame is the exact plain ink"
        );
        assert_eq!(px(&scanout, 150, 80), [200, 60, 60, 255]);
    }
    // Mid-fade demonstrably differed (the A/B narrative: black →
    // cherry, with the pixels in between the spring's own).
    assert!(!tb.world(|w| w.scene.transitions.any_live()));
}

/// The ephemeral roles never animate in: a surface claiming `tooltip`
/// appears at its plain pixels on the mapping frame itself — the
/// instant-appearance contract.
#[test]
fn ephemeral_roles_appear_instantly() {
    let tb = Testbench::start_with("transition-tooltip", transitions_config());
    let mut client = TestClient::connect(&tb.addr);

    let shm = client.bind("ldp.core.shm");
    let compositor = client.bind("ldp.core.compositor");
    let shell = client.bind("ldp.shell.shell");
    client.bind("ldp.core.output");

    let amber = xrgb(220, 170, 40);
    let pixels: Vec<u8> = std::iter::repeat(amber.to_le_bytes())
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
    let surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("surface");
    let toplevel = get_toplevel(&mut client, &shell, &surface);

    // The tooltip claim arrives *before* the mapping commit (the
    // claim's teeth are the catalog's eligibility at begin time).
    client
        .conn
        .send_request(&toplevel, "set_semantic_role", vec![Value::Enum(3)])
        .expect("set_semantic_role(tooltip)");
    client.sync();

    present_first(&tb, &mut client, &surface, &buffer);
    // Instant: the plain pixels on the very first frame, no motion
    // ever begun for it.
    {
        let scanout = tb.scanout();
        assert_eq!(
            px(&scanout, 60, 30),
            [220, 170, 40, 255],
            "the tooltip appears at its plain pixels"
        );
    }
    assert!(!tb.world(|w| w.scene.transitions.any_live()));
    assert_eq!(tb.world(|w| w.scene.transitions.begun), 0);
}

/// The library default keeps the bytes plain: transitions off (the
/// config's default), the mapping frame IS the plain ink — the
/// byte-exactness doctrine every sibling suite pins implicitly, made
/// explicit here.
#[test]
fn the_library_default_keeps_the_bytes_plain() {
    let tb = Testbench::start("transition-default-off");
    let mut client = TestClient::connect(&tb.addr);

    let shm = client.bind("ldp.core.shm");
    let compositor = client.bind("ldp.core.compositor");
    let shell = client.bind("ldp.shell.shell");
    client.bind("ldp.core.output");

    let teal = xrgb(30, 160, 170);
    let pixels: Vec<u8> = std::iter::repeat(teal.to_le_bytes())
        .take(90 * 70)
        .flatten()
        .collect();
    let pool = create_pool(&mut client, &shm, pool_bytes(&pixels), (90 * 70 * 4) as i64);
    let buffer = create_buffer(&mut client, &pool, 0, 90, 70, 360, XR24);
    let surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("surface");
    let _toplevel = get_toplevel(&mut client, &shell, &surface);

    present_first(&tb, &mut client, &surface, &buffer);
    {
        let scanout = tb.scanout();
        assert_eq!(
            px(&scanout, 45, 35),
            [30, 160, 170, 255],
            "the default serves the plain ink from the first frame"
        );
    }
    assert!(!tb.world(|w| w.scene.transitions.any_live()));
    assert_eq!(tb.world(|w| w.scene.transitions.begun), 0);
}
