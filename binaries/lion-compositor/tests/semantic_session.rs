//! Phase 47's exit criterion (the semantic half): the semantic-scene
//! triple served end to end through the real protocol —
//!
//! * `toplevel.set_security_class(protected)`: the capture path
//!   redacts the surface's rectangle out of the client-visible frame
//!   while the display keeps showing it, and `normal` clears the
//!   claim restoring the pixel-exact bytes — the A/B/A proof the
//!   material family's own tests pin (the capture oracle's own
//!   discipline);
//! * the lock role's security floor: a surface claiming `lock`
//!   redacts whatever class it claimed beneath;
//! * `toplevel.set_scene_profile(gaming|creative)`: the claim lands
//!   in the scene's semantics map *and* the scheduler's slot (the two
//!   truths updated in one breath), and the frame-callback economy
//!   keeps flowing — the profile changes pacing doctrine, never
//!   breaks it.

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
    let fd = lion_compositor::sys::memfd("semantic-pool").expect("memfd");
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

/// The semantic-session config: the dock off (a clean desktop),
/// Minimal effects (the pixel oracle's own tier).
fn semantic_config() -> CompositorConfig {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    CompositorConfig {
        shell: ShellConfig {
            dock: DockMode::Off,
            ..ShellConfig::default()
        },
        socket: format!("lion-semantic-{n}-{}", std::process::id()),
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

/// Attach + damage + commit, then pump until the compositor rendered.
fn present(tb: &Testbench, client: &mut TestClient, surface: &Proxy, buffer: &Proxy, rect: Rect) {
    let before = tb.frames();
    attach(client, surface, buffer);
    damage(client, surface, &[rect]);
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

/// Bind the capture manager and grab once; returns (w, h, words).
/// Waits for a **fresh** frame record — the collector accumulates
/// across grabs, so a stale-frame match would read the previous
/// grab's snapshot (the repeat-grab race the capture suite's idle
/// scene never exercises).
fn grab(client: &mut TestClient) -> (u32, u32, Vec<u32>) {
    let capture = client.bind("ldp.capture.capture_manager");
    let before = client.events.records.len();
    client
        .conn
        .send_request(&capture, "grab", vec![])
        .expect("send grab");
    client.wait_until(|c| {
        c.records[before..]
            .iter()
            .any(|r| r.event == "frame" && r.snapshot.is_some())
    });
    let frame = client.last("frame");
    let (Value::Uint32(width), Value::Uint32(height)) = (&frame.args[1], &frame.args[2]) else {
        panic!("frame dims are not uint32: {:?}", frame.args)
    };
    let bytes = frame.snapshot.clone().expect("snapshot bytes");
    let words: Vec<u32> = bytes
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    (*width, *height, words)
}

/// THE exit criterion (the security-aware compositor): a `protected`
/// surface's rectangle paints black in the captured frame — the
/// display keeps showing the content, the screenshot does not — and
/// `normal` clears the claim restoring the pixel-exact bytes. The
/// A/B/A proof, the capture oracle's own discipline.
#[test]
fn protected_surfaces_redact_out_of_captures() {
    let tb = Testbench::start_with("semantic-security", semantic_config());
    let mut client = TestClient::connect(&tb.addr);

    // ---- a 200x100 cherry window at the cascade's first step ----
    let shm = client.bind("ldp.core.shm");
    let compositor = client.bind("ldp.core.compositor");
    let shell = client.bind("ldp.shell.shell");
    client.bind("ldp.core.output");

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
    let toplevel = get_toplevel(&mut client, &shell, &surface);
    present(
        &tb,
        &mut client,
        &surface,
        &buffer,
        Rect::new(0, 0, 200, 100),
    );

    // ---- A: the unclaimed surface captures pixel-exact ----
    let (w, h, plain) = grab(&mut client);
    assert_eq!((w, h), (1920, 1080));
    assert_eq!(px(&plain, 50, 30), [200, 60, 60, 255]);

    // ---- B: the protected claim redacts the capture, not the display
    client
        .conn
        .send_request(&toplevel, "set_security_class", vec![Value::Enum(3)])
        .expect("set_security_class(protected)");
    client.sync();
    let (_, _, redacted) = grab(&mut client);
    // The captured rectangle is opaque black...
    assert_eq!(px(&redacted, 50, 30), [0, 0, 0, 255]);
    assert_eq!(px(&redacted, 150, 80), [0, 0, 0, 255]);
    // ...the scanout (what the user's own display shows) keeps the
    // content — the enforcement point is *visibility*, not the panel.
    assert_eq!(px(&tb.scanout(), 50, 30), [200, 60, 60, 255]);

    // ---- A again: the clear restores the pixel-exact bytes
    client
        .conn
        .send_request(&toplevel, "set_security_class", vec![Value::Enum(1)])
        .expect("set_security_class(normal)");
    client.sync();
    let (_, _, restored) = grab(&mut client);
    assert_eq!(restored, plain, "the A/B/A byte-equality proof");
}

/// The lock role's security floor: a surface claiming `lock` redacts
/// whatever class it claimed beneath — the one claim a client cannot
/// talk its way below (the lock screen is never capturable).
#[test]
fn the_lock_role_floors_the_security_class() {
    let tb = Testbench::start_with("semantic-lock", semantic_config());
    let mut client = TestClient::connect(&tb.addr);

    let shm = client.bind("ldp.core.shm");
    let compositor = client.bind("ldp.core.compositor");
    let shell = client.bind("ldp.shell.shell");
    client.bind("ldp.core.output");

    let ink = xrgb(90, 90, 200);
    let pixels: Vec<u8> = std::iter::repeat(ink.to_le_bytes())
        .take(100 * 80)
        .flatten()
        .collect();
    let pool = create_pool(
        &mut client,
        &shm,
        pool_bytes(&pixels),
        (100 * 80 * 4) as i64,
    );
    let buffer = create_buffer(&mut client, &pool, 0, 100, 80, 400, XR24);
    let surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("surface");
    let toplevel = get_toplevel(&mut client, &shell, &surface);
    present(
        &tb,
        &mut client,
        &surface,
        &buffer,
        Rect::new(0, 0, 100, 80),
    );

    // The lock role alone (class unclaimed = normal) redacts.
    client
        .conn
        .send_request(&toplevel, "set_semantic_role", vec![Value::Enum(5)])
        .expect("set_semantic_role(lock)");
    client.sync();
    let (_, _, words) = grab(&mut client);
    assert_eq!(px(&words, 40, 30), [0, 0, 0, 255]);
    assert_eq!(px(&tb.scanout(), 40, 30), [90, 90, 200, 255]);

    // And even an explicit `normal` claim cannot sink below the floor.
    client
        .conn
        .send_request(&toplevel, "set_security_class", vec![Value::Enum(1)])
        .expect("set_security_class(normal)");
    client.sync();
    let (_, _, floored) = grab(&mut client);
    assert_eq!(px(&floored, 40, 30), [0, 0, 0, 255]);
}

/// The scene profile's exit criterion: the claim lands in the scene's
/// semantics map *and* the scheduler's slot — the two truths updated
/// in one breath under one lock — and the frame-callback economy
/// keeps flowing (the profile changes pacing doctrine, never breaks
/// it: a `frame` request still draws its `frame_target` answer).
#[test]
fn scene_profiles_serve_through_the_wire() {
    let tb = Testbench::start_with("semantic-profile", semantic_config());
    let mut client = TestClient::connect(&tb.addr);

    let shm = client.bind("ldp.core.shm");
    let compositor = client.bind("ldp.core.compositor");
    let shell = client.bind("ldp.shell.shell");
    client.bind("ldp.core.output");

    let pixels: Vec<u8> = vec![0x40; 64 * 64 * 4];
    let pool = create_pool(&mut client, &shm, pool_bytes(&pixels), (64 * 64 * 4) as i64);
    let buffer = create_buffer(&mut client, &pool, 0, 64, 64, 256, XR24);
    let surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("surface");
    let toplevel = get_toplevel(&mut client, &shell, &surface);
    present(&tb, &mut client, &surface, &buffer, Rect::new(0, 0, 64, 64));

    // Resolve the surface's id once (the world-side oracle's key).
    let surface_id = tb.world(|w| {
        w.scene
            .routes
            .iter()
            .find(|(_, r)| r.surface_obj == surface.id())
            .map(|(id, _)| *id)
            .expect("the surface is routed")
    });

    // The unclaimed surface: the default desktop doctrine, both truths.
    tb.world(|w| {
        assert_eq!(
            w.scene.scheduler.profile_of(surface_id),
            ldp_compositor::SceneProfile::Desktop
        );
        assert_eq!(
            w.scene.semantics_of(surface_id).profile,
            ldp_compositor::SceneProfile::Desktop
        );
    });

    // The gaming claim: one request, both truths.
    client
        .conn
        .send_request(&toplevel, "set_scene_profile", vec![Value::Enum(3)])
        .expect("set_scene_profile(gaming)");
    client.sync();
    tb.world(|w| {
        assert_eq!(
            w.scene.semantics_of(surface_id).profile,
            ldp_compositor::SceneProfile::Gaming,
            "the semantics map carries the claim"
        );
        assert_eq!(
            w.scene.scheduler.profile_of(surface_id),
            ldp_compositor::SceneProfile::Gaming,
            "the scheduler's slot carries the same truth"
        );
    });

    // The frame economy keeps flowing: a frame request draws its
    // answer (the target event, whatever the budget floor).
    frame(&mut client, &surface, 7);
    client.sync();
    client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "frame_target" && r.target == surface.id().as_u32())
    });

    // The clear: `desktop` restores the operator's doctrine.
    client
        .conn
        .send_request(&toplevel, "set_scene_profile", vec![Value::Enum(1)])
        .expect("set_scene_profile(desktop)");
    client.sync();
    tb.world(|w| {
        assert_eq!(
            w.scene.scheduler.profile_of(surface_id),
            ldp_compositor::SceneProfile::Desktop
        );
    });
}

/// The semantic role's carry: `set_semantic_role(dialog)` lands in the
/// semantics map (the plain `window` claim clears it).
#[test]
fn semantic_roles_carry_through_the_wire() {
    let tb = Testbench::start_with("semantic-role", semantic_config());
    let mut client = TestClient::connect(&tb.addr);

    let shm = client.bind("ldp.core.shm");
    let compositor = client.bind("ldp.core.compositor");
    let shell = client.bind("ldp.shell.shell");
    client.bind("ldp.core.output");

    let pixels: Vec<u8> = vec![0x20; 32 * 32 * 4];
    let pool = create_pool(&mut client, &shm, pool_bytes(&pixels), (32 * 32 * 4) as i64);
    let buffer = create_buffer(&mut client, &pool, 0, 32, 32, 128, XR24);
    let surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("surface");
    let toplevel = get_toplevel(&mut client, &shell, &surface);
    present(&tb, &mut client, &surface, &buffer, Rect::new(0, 0, 32, 32));

    client
        .conn
        .send_request(&toplevel, "set_semantic_role", vec![Value::Enum(2)])
        .expect("set_semantic_role(dialog)");
    client.sync();
    let surface_id = tb.world(|w| {
        w.scene
            .routes
            .iter()
            .find(|(_, r)| r.surface_obj == surface.id())
            .map(|(id, _)| *id)
            .expect("routed")
    });
    tb.world(|w| {
        assert_eq!(
            w.scene.semantics_of(surface_id).role,
            Some(ldp_compositor::SemanticRole::Dialog),
            "the dialog claim carried"
        );
    });

    // The plain-window claim clears it.
    client
        .conn
        .send_request(&toplevel, "set_semantic_role", vec![Value::Enum(1)])
        .expect("set_semantic_role(window)");
    client.sync();
    tb.world(|w| {
        assert_eq!(w.scene.semantics_of(surface_id).role, None);
    });
}
