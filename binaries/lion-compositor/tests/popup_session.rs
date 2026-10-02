//! Phase 36's exit criterion (the native half): the popup role
//! served end to end through the real protocol — `get_popup` mints
//! the machine, the surface's first attach solves the placement
//! (the buffer's size is the solver's input), `popup.configure`
//! carries the parent-relative proposal, the ack + commit land the
//! menu **placed** (never origin-then-jump), a `reposition`
//! re-solves and moves the mapped popup immediately, a `dismiss`
//! answers with `done`, and the parent's death dismisses every child
//! (a menu cannot outlive its window).

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
    use std::os::unix::fs::FileExt as _;
    let fd = lion_compositor::sys::memfd("popup-pool").expect("memfd");
    {
        let file = std::fs::File::from(fd.try_clone().expect("clone"));
        file.write_at(pixels, 0).expect("write pool");
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

/// A unique-socket config with the shell serving (the cascade and the
/// usable area the solver constrains against).
fn popup_config() -> CompositorConfig {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    CompositorConfig {
        shell: ShellConfig {
            dock: DockMode::Auto,
            ..ShellConfig::default()
        },
        socket: format!("lion-popup-{n}-{}", std::process::id()),
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

/// `shell.get_popup(surface, parent, anchor_rect, anchor, gravity,
/// offset, constraints)` — the menu shape: anchored at the rect's
/// bottom-left, growing down-right, every strategy granted.
fn get_menu_popup(
    client: &mut TestClient,
    shell: &Proxy,
    surface: &Proxy,
    parent: &Proxy,
    anchor_rect: Rect,
) -> Proxy {
    client
        .conn
        .create_object(
            shell,
            "get_popup",
            vec![
                Value::Object(Some(surface.id())),
                Value::Object(Some(parent.id())),
                Value::Rect(anchor_rect),
                Value::Enum(7), // anchor: bottom_left
                Value::Enum(5), // gravity: bottom_right
                Value::Int32(0),
                Value::Int32(0),
                Value::Bitset(
                    ldp_core::bitset::Bitset128::single(0)
                        .with(1)
                        .with(2)
                        .with(3)
                        .with(4)
                        .with(5),
                ),
            ],
        )
        .expect("get_popup")
}

/// The popup configure events collected for one popup object.
fn popup_configures<'a>(c: &'a Collector, popup: &Proxy) -> Vec<&'a Recorded> {
    c.records
        .iter()
        .filter(|r| r.event == "configure" && r.target == popup.id().as_u32())
        .collect()
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
            "the compositor never rendered the popup's buffer"
        );
        client.sync();
    }
}

/// THE exit criterion: the anchor/gravity machine through the real
/// wire — placement at attach, configure/ack, the reposition move,
/// the dismissal, and the parent-death sweep.
#[test]
#[allow(clippy::too_many_lines)] // one session, one narrative
fn popups_anchor_solve_ack_and_dismiss_through_the_real_wire() {
    let tb = Testbench::start_with("popup-native", popup_config());
    let mut client = TestClient::connect(&tb.addr);

    // ---- the parent: a 200x100 window at cascade step 0 = (0, 0) --
    let shm = client.bind("ldp.core.shm");
    let compositor = client.bind("ldp.core.compositor");
    let shell = client.bind("ldp.shell.shell");
    client.bind("ldp.core.output");

    let parent_word = xrgb(40, 60, 80);
    let parent_pixels: Vec<u8> = std::iter::repeat(parent_word.to_le_bytes())
        .take(200 * 100)
        .flatten()
        .collect();
    let parent_pool = create_pool(
        &mut client,
        &shm,
        pool_bytes(&parent_pixels),
        (200 * 100 * 4) as i64,
    );
    let parent_buffer = create_buffer(&mut client, &parent_pool, 0, 200, 100, 800, XR24);
    let parent_surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("parent surface");
    let _parent_toplevel = get_toplevel(&mut client, &shell, &parent_surface);
    present(
        &tb,
        &mut client,
        &parent_surface,
        &parent_buffer,
        Rect::new(0, 0, 200, 100),
    );

    // ---- the menu: 24x8, anchored at the parent's (100, 40) +8x4 ---
    //
    // The unconstrained placement: the popup's top-left at the anchor
    // rect's bottom-left corner = (100, 44) parent-relative; the
    // parent sits at (0, 0), so the menu lands at (100, 44) on the
    // output — placed by the attach, never origin-then-jump.
    let menu_word = xrgb(240, 200, 90);
    let menu_pixels: Vec<u8> = std::iter::repeat(menu_word.to_le_bytes())
        .take(24 * 8)
        .flatten()
        .collect();
    let menu_pool = create_pool(
        &mut client,
        &shm,
        pool_bytes(&menu_pixels),
        (24 * 8 * 4) as i64,
    );
    let menu_buffer = create_buffer(&mut client, &menu_pool, 0, 24, 8, 96, XR24);
    let menu_surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("menu surface");
    let menu = get_menu_popup(
        &mut client,
        &shell,
        &menu_surface,
        &parent_surface,
        Rect::new(100, 40, 8, 4),
    );

    // The attach solves and proposes: popup.configure(serial, x, y,
    // w, h) with the parent-relative placement.
    attach(&mut client, &menu_surface, &menu_buffer);
    client.sync();
    client.wait_until(|c| !popup_configures(c, &menu).is_empty());
    let proposal_args = {
        let records = popup_configures(&client.events, &menu);
        let last = records.last().expect("the configure arrived");
        last.args.clone()
    };
    match &proposal_args[..] {
        [Value::Uint32(serial), Value::Int32(x), Value::Int32(y), Value::Uint32(w), Value::Uint32(h)] =>
        {
            assert_eq!(
                (*x, *y),
                (100, 44),
                "the anchor solve (bottom-left, down-right)"
            );
            assert_eq!((*w, *h), (24, 8), "the buffer's size");
            // The ack references the proposal's serial.
            client
                .conn
                .send_request(&menu, "ack_configure", vec![Value::Uint32(*serial)])
                .expect("ack_configure");
        }
        other => panic!("the configure carries serial + placement (got {other:?})"),
    }

    // The commit lands the menu placed — the pixels at (100, 44).
    damage(&mut client, &menu_surface, &[Rect::new(0, 0, 24, 8)]);
    commit(&mut client, &menu_surface, 2);
    client.sync();
    client.wait_until(|_| tb.frames() >= 2);
    {
        let scanout = tb.scanout();
        assert_eq!(
            px(&scanout, 100, 44),
            [240, 200, 90, 255],
            "the menu's top-left, placed at the anchor"
        );
        assert_eq!(
            px(&scanout, 123, 51),
            [240, 200, 90, 255],
            "the menu's bottom-right"
        );
        // Just outside: the parent's own pixels.
        assert_eq!(px(&scanout, 99, 44), [40, 60, 80, 255]);
        assert_eq!(px(&scanout, 124, 51), [40, 60, 80, 255]);
    }

    // ---- the reposition: a new anchor, a fresh solve, a live move --
    //
    // The menu re-anchors at the parent's (0, 40): the proposal moves
    // to (0, 44), and the mapped popup moves immediately (the
    // re-layout arm's set_position_now doctrine).
    client
        .conn
        .send_request(
            &menu,
            "reposition",
            vec![
                Value::Rect(Rect::new(0, 40, 8, 4)),
                Value::Enum(7), // anchor: bottom_left
                Value::Enum(5), // gravity: bottom_right
                Value::Int32(0),
                Value::Int32(0),
            ],
        )
        .expect("reposition");
    client.wait_until(|c| popup_configures(c, &menu).len() >= 2);
    let moved_args = {
        let records = popup_configures(&client.events, &menu);
        let last = records.last().expect("the re-proposal arrived");
        last.args.clone()
    };
    match &moved_args[..] {
        [Value::Uint32(serial), Value::Int32(x), Value::Int32(y), Value::Uint32(_), Value::Uint32(_)] =>
        {
            assert_eq!((*x, *y), (0, 44), "the re-anchored placement");
            client
                .conn
                .send_request(&menu, "ack_configure", vec![Value::Uint32(*serial)])
                .expect("ack the move");
        }
        other => panic!("the re-proposal carries the placement (got {other:?})"),
    }
    // A menu that moved re-presents (the render trigger is the
    // commit; the position itself applied at the reposition).
    present(
        &tb,
        &mut client,
        &menu_surface,
        &menu_buffer,
        Rect::new(0, 0, 24, 8),
    );
    {
        let scanout = tb.scanout();
        assert_eq!(
            px(&scanout, 0, 44),
            [240, 200, 90, 255],
            "the menu moved to the re-anchored placement"
        );
        assert_eq!(
            px(&scanout, 100, 44),
            [40, 60, 80, 255],
            "the old anchor shows the parent again"
        );
    }

    // ---- the dismissal: done answers the client's own dismiss -----
    client
        .conn
        .send_request(&menu, "dismiss", vec![])
        .expect("dismiss");
    client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "done" && r.target == menu.id().as_u32())
    });

    // ---- the parent-death sweep: children cannot outlive the window
    //
    // A second popup anchors at (50, 40); the parent surface dies;
    // the child's `done` arrives (the compositor's own dismissal).
    let orphan_surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("menu2 surface");
    let orphan_menu = get_menu_popup(
        &mut client,
        &shell,
        &orphan_surface,
        &parent_surface,
        Rect::new(50, 40, 8, 4),
    );
    let _ = &orphan_menu;
    destroy(&mut client, &parent_surface);
    client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "done" && r.target == orphan_menu.id().as_u32())
    });
}
