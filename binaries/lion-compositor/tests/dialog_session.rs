//! Phase 48's exit criterion (the knock half): the dialog role served
//! end to end through the real protocol — the frozen `get_dialog`
//! surface, finally driven —
//!
//! * `shell.get_dialog(surface, parent_toplevel, modality)` mints the
//!   `ldp.shell.dialog` object and proposes the client's own size
//!   (0x0, the xdg doctrine) under the machine's serial clock; the
//!   `ack_configure` handshake is strict (a stale serial is the
//!   protocol error the two-phase contract names);
//! * the dialog's first attach centers it over its parent (the
//!   spec's server-side centering policy), the position riding the
//!   pending queue so the dialog lands placed with the mapping
//!   commit — pixel-proven against the parent's own ink;
//! * a **modal** dialog gates its parent's tree: the hit-test never
//!   finds the parent while the dialog is mapped, and the keyboard
//!   follows the gate (the parent's focus leaves, the dialog's
//!   enters); a **modeless** dialog gates nothing;
//! * a dialog cannot outlive its window: the parent's death closes
//!   the dialog (`dialog.close`), and the gate lifts with it;
//! * a surface may take at most one role: the second claim is
//!   `invalid_state`, refused by name.

mod testbench;

use ldp_client::Proxy;
use ldp_core::geometry::Rect;
use ldp_core::wire::Value;
use testbench::*;

use ldp_client::ClientError;
use ldp_input::device::DeviceClass;
use ldp_input::normalizer::InputEvent;
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

/// One scanout pixel as an RGB triple.
fn px(words: &[u32], x: usize, y: usize) -> [u32; 4] {
    let v = words[y * OUT_W + x];
    [
        (v >> 16) & 0xFF,
        (v >> 8) & 0xFF,
        v & 0xFF,
        (v >> 24) & 0xFF,
    ]
}

/// Events of one name in the collector (the wait predicates' helper).
fn named(c: &Collector, name: &str) -> usize {
    c.records.iter().filter(|r| r.event == name).count()
}

/// Wait for one event of one name *on one object* (the dialog's own
/// configure — the toplevel's configure lives in the same stream).
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

/// The first keyboard enter for `surface` among the first `skip`
/// enters' successors (Phase 51's gate return: the keys came home).
fn last_keyboard_enter_after(
    client: &TestClient,
    skip: usize,
    surface: ldp_core::ids::ObjectId,
) -> &Recorded {
    client
        .events
        .records
        .iter()
        .filter(|r| r.event == "enter")
        .skip(skip)
        .find(|r| {
            r.interface == "ldp.input.keyboard"
                && r.args.first() == Some(&Value::Object(Some(surface)))
        })
        .unwrap_or_else(|| panic!("no keyboard enter for surface {surface:?} after enter #{skip}"))
}

/// Whether a recorded configure's states bitset carries `bit` (the
/// activated round trips read it).
fn states_bit(record: &Recorded, bit: u32) -> bool {
    match &record.args[1] {
        Value::Bitset(bits) => bits.test(bit),
        other => panic!("configure's states argument is not a bitset: {other:?}"),
    }
}

/// A memfd carrying `pixels` (the pool backing).
fn pool_bytes(pixels: &[u8]) -> std::os::fd::OwnedFd {
    use std::io::Write as _;
    let fd = lion_compositor::sys::memfd("dialog-pool").expect("memfd");
    {
        let mut file = std::fs::File::from(fd.try_clone().expect("clone"));
        file.write_all(pixels).expect("write pool");
    }
    fd
}

/// The dialog config: the dock off (a clean desktop — the legacy
/// placement keeps creation positions, so the parent sits at the
/// origin and the centering arithmetic is the pixel oracle's own).
fn dialog_config() -> CompositorConfig {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    CompositorConfig {
        shell: ShellConfig {
            dock: DockMode::Off,
            ..ShellConfig::default()
        },
        socket: format!("lion-dialog-{n}-{}", std::process::id()),
        ..CompositorConfig::default()
    }
}

/// Map one plain window at the origin (the routing target, the
/// dialog's parent). Returns (surface, buffer, pool).
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

/// `shell.get_dialog(surface, parent_toplevel, modality)` — the
/// modality's wire value (1 modal, 2 modeless).
fn get_dialog(
    client: &mut TestClient,
    shell: &Proxy,
    surface: &Proxy,
    parent_toplevel: &Proxy,
    modality: u32,
) -> Proxy {
    client
        .conn
        .create_object(
            shell,
            "get_dialog",
            vec![
                Value::Object(Some(surface.id())),
                Value::Object(Some(parent_toplevel.id())),
                Value::Enum(modality),
            ],
        )
        .expect("get_dialog")
}

/// `dialog.ack_configure(serial)`.
fn ack_configure(client: &mut TestClient, dialog: &Proxy, serial: u32) {
    client
        .conn
        .send_request(dialog, "ack_configure", vec![Value::Uint32(serial)])
        .expect("ack_configure");
}

/// THE mint: `get_dialog` proposes the client's own size (0x0, the
/// xdg doctrine) under the machine's serial clock, the ack lands,
/// and the dialog's first attach centers it over its parent — the
/// placement lands with the mapping commit (pixel-proven: the
/// dialog's ink at the centered rect, the parent's ink around it).
#[test]
fn the_dialog_mints_acks_and_centers_over_its_parent() {
    let tb = Testbench::start_with("dialog-mint", dialog_config());
    let mut client = TestClient::connect(&tb.addr);
    let shell = client.bind("ldp.shell.shell");

    // The parent: 200x100 plum at the origin.
    let plum = xrgb(120, 40, 160);
    let (parent, _b, _p) = map_window(&tb, &mut client, plum, 200, 100);
    let parent_toplevel = get_toplevel(&mut client, &shell, &parent);
    client.sync();

    // The dialog's surface: 100x80 ivory.
    let ivory = xrgb(240, 240, 220);
    let shm = client.bind("ldp.core.shm");
    let pixels: Vec<u8> = std::iter::repeat(ivory.to_le_bytes())
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
    let compositor = client.bind("ldp.core.compositor");
    let dialog_surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("surface");
    let dialog = get_dialog(&mut client, &shell, &dialog_surface, &parent_toplevel, 1);

    // The initial configure: the machine's first serial, the client's
    // own size (0x0) — the *dialog's own* event (the toplevel's
    // configure lives in the same stream).
    wait_for_on(&mut client, "configure", dialog.id().as_u32());
    let configure = last_on(&client, "configure", dialog.id().as_u32());
    assert_eq!(configure.interface, "ldp.shell.dialog");
    assert_eq!(configure.target, dialog.id().as_u32());
    let Value::Uint32(serial) = configure.args[0] else {
        panic!("the configure carries a serial");
    };
    assert_eq!(serial, 1, "the machine's serial clock starts at 1");
    assert_eq!(configure.args[1], Value::Uint32(0));
    assert_eq!(configure.args[2], Value::Uint32(0));

    // The two-phase commit: ack, then commit the buffer.
    ack_configure(&mut client, &dialog, serial);

    // The mapping commit: the centered placement lands with it.
    let before = tb.frames();
    attach(&mut client, &dialog_surface, &buffer);
    damage(&mut client, &dialog_surface, &[Rect::new(0, 0, 100, 80)]);
    commit(&mut client, &dialog_surface, 1);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while tb.frames() <= before {
        assert!(deadline > std::time::Instant::now(), "never rendered");
        client.sync();
    }

    // The centering oracle: a 100x80 dialog centered over a 200x100
    // parent sits at (50, 10) — the dialog's ink inside, the parent's
    // around it.
    let words = tb.scanout();
    assert_eq!(px(&words, 50 + 10, 10 + 10)[0], 240, "the dialog's ink");
    assert_eq!(px(&words, 50 + 90, 10 + 70)[2], 220, "the dialog's corner");
    assert_eq!(px(&words, 10, 10)[2], 160, "the parent's ink left of it");
    assert_eq!(px(&words, 180, 90)[0], 120, "the parent's ink right of it");
    assert_eq!(px(&words, 55, 60)[1], 240, "inside the dialog");
    assert_eq!(px(&words, 40, 55)[1], 40, "outside it (the parent)");
}

/// A surface may take at most one role: a surface already carrying
/// the toplevel role is refused `get_dialog` by name.
#[test]
fn a_second_role_is_refused() {
    let tb = Testbench::start_with("dialog-role", dialog_config());
    let mut client = TestClient::connect(&tb.addr);
    let shell = client.bind("ldp.shell.shell");

    let (parent, _b, _p) = map_window(&tb, &mut client, xrgb(90, 90, 90), 120, 60);
    let toplevel = get_toplevel(&mut client, &shell, &parent);
    client.sync();

    // The tooplevel'd surface asks for a second role.
    let _ = get_dialog(&mut client, &shell, &parent, &toplevel, 1);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match client.conn.roundtrip(&mut client.events) {
            Err(ClientError::ServerError { code, .. }) => {
                assert_eq!(
                    code,
                    ldp_core::error::ErrorCode::InvalidState,
                    "the second role is invalid_state"
                );
                break;
            }
            Err(other) => panic!("expected invalid_state, got {other:?}"),
            Ok(()) => assert!(
                std::time::Instant::now() < deadline,
                "the server never refused the second role"
            ),
        }
    }
}

/// The strict two-phase commit: a stale ack serial is the protocol
/// error the contract names (the dialog's size proposal is a
/// contract, not the popup's advisory placement).
#[test]
fn a_stale_ack_is_refused() {
    let tb = Testbench::start_with("dialog-ack", dialog_config());
    let mut client = TestClient::connect(&tb.addr);
    let shell = client.bind("ldp.shell.shell");

    let (parent, _b, _p) = map_window(&tb, &mut client, xrgb(90, 90, 90), 120, 60);
    let parent_toplevel = get_toplevel(&mut client, &shell, &parent);
    client.sync();

    let compositor = client.bind("ldp.core.compositor");
    let dialog_surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("surface");
    let dialog = get_dialog(&mut client, &shell, &dialog_surface, &parent_toplevel, 2);
    wait_for_on(&mut client, "configure", dialog.id().as_u32());

    // A serial the machine never issued.
    ack_configure(&mut client, &dialog, 99);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match client.conn.roundtrip(&mut client.events) {
            Err(ClientError::ServerError { code, .. }) => {
                assert_eq!(
                    code,
                    ldp_core::error::ErrorCode::InvalidState,
                    "the stale ack is invalid_state"
                );
                break;
            }
            Err(other) => panic!("expected invalid_state, got {other:?}"),
            Ok(()) => assert!(
                std::time::Instant::now() < deadline,
                "the server never refused the stale ack"
            ),
        }
    }
}

/// The bounded-string doctrine: a title carrying an embedded NUL is
/// `invalid_string` (the over-length arm is the *client's* own
/// `string_bytes` budget — the ldp-shell machine's own test pins it
/// there; the wire never carries it).
#[test]
fn dialog_titles_are_bounded() {
    let tb = Testbench::start_with("dialog-title", dialog_config());
    let mut client = TestClient::connect(&tb.addr);
    let shell = client.bind("ldp.shell.shell");

    let (parent, _b, _p) = map_window(&tb, &mut client, xrgb(90, 90, 90), 120, 60);
    let parent_toplevel = get_toplevel(&mut client, &shell, &parent);
    client.sync();

    let compositor = client.bind("ldp.core.compositor");
    let dialog_surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("surface");
    let dialog = get_dialog(&mut client, &shell, &dialog_surface, &parent_toplevel, 2);
    wait_for_on(&mut client, "configure", dialog.id().as_u32());

    client
        .conn
        .send_request(
            &dialog,
            "set_title",
            vec![Value::String("Save\u{0}changes?".into())],
        )
        .expect("send the NUL title");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match client.conn.roundtrip(&mut client.events) {
            Err(ClientError::ServerError { code, .. }) => {
                assert_eq!(
                    code,
                    ldp_core::error::ErrorCode::InvalidString,
                    "the embedded NUL is invalid_string"
                );
                break;
            }
            Err(other) => panic!("expected invalid_string, got {other:?}"),
            Ok(()) => assert!(
                std::time::Instant::now() < deadline,
                "the server never rejected the long title"
            ),
        }
    }
}

/// THE gate: a modal dialog gates its parent's tree — the pointer
/// never routes to the parent while the dialog is mapped, the
/// keyboard leaves the parent for the dialog at the mapping commit,
/// and the gate lifts when the dialog dies (the parent's input
/// returns). A modeless dialog gates nothing (the control half of
/// the same session).
#[test]
#[allow(clippy::too_many_lines)] // one session, one narrative
fn the_modal_dialog_gates_its_parents_input() {
    let tb = Testbench::start_with("dialog-gate", dialog_config());
    let mut client = TestClient::connect(&tb.addr);
    let shell = client.bind("ldp.shell.shell");

    // The parent: 200x100 at the origin (the routing target).
    let (parent, _b, _p) = map_window(&tb, &mut client, xrgb(90, 90, 90), 200, 100);
    let parent_toplevel = get_toplevel(&mut client, &shell, &parent);
    client.sync();

    // The input devices: the router's addresses for this client.
    let seat = client.bind("ldp.input.seat");
    let pointer = client
        .conn
        .create_object(&seat, "get_pointer", vec![])
        .expect("pointer");
    let keyboard = client
        .conn
        .create_object(&seat, "get_keyboard", vec![])
        .expect("keyboard");
    client.sync();
    let _ = pointer;
    let _ = keyboard;

    // The pointer enters the parent: motion (100, 50) routes.
    tb.world_mut(|w| {
        w.queue_input(
            DeviceClass::Mouse,
            vec![InputEvent::PointerMotion { dx: 100, dy: 50 }],
        );
        let routed = w.pump_input();
        assert!(routed > 0, "the ungated parent takes its motion");
    });
    client.sync();
    client.wait_until(|c| named(c, "relative_motion") >= 1);

    // The click focuses the parent (click-to-focus).
    tb.world_mut(|w| {
        w.queue_input(
            DeviceClass::Mouse,
            vec![InputEvent::Button {
                button: 0x110,
                pressed: true,
            }],
        );
        w.pump_input();
        w.queue_input(
            DeviceClass::Mouse,
            vec![InputEvent::Button {
                button: 0x110,
                pressed: false,
            }],
        );
        w.pump_input();
    });
    client.sync();
    client.wait_until(|c| named(c, "enter") >= 1);
    let enters_before = client.events_of("enter").len();

    // The modal dialog: 100x80 ivory, centered at (50, 10).
    let ivory = xrgb(240, 240, 220);
    let shm = client.bind("ldp.core.shm");
    let pixels: Vec<u8> = std::iter::repeat(ivory.to_le_bytes())
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
    let compositor = client.bind("ldp.core.compositor");
    let dialog_surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("surface");
    let dialog = get_dialog(&mut client, &shell, &dialog_surface, &parent_toplevel, 1);
    wait_for_on(&mut client, "configure", dialog.id().as_u32());
    let Value::Uint32(serial) = last_on(&client, "configure", dialog.id().as_u32()).args[0] else {
        panic!("the configure carries a serial");
    };
    ack_configure(&mut client, &dialog, serial);
    let before = tb.frames();
    attach(&mut client, &dialog_surface, &buffer);
    damage(&mut client, &dialog_surface, &[Rect::new(0, 0, 100, 80)]);
    commit(&mut client, &dialog_surface, 1);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while tb.frames() <= before {
        assert!(deadline > std::time::Instant::now(), "never rendered");
        client.sync();
    }

    // The keyboard followed the gate: the dialog's mapping commit
    // retargeted the parent's focus to the dialog (leave + enter).
    client.sync();
    client.wait_until(|c| named(c, "enter") > enters_before);
    let enters = client.events_of("enter");
    let dialog_enter = enters
        .iter()
        .find(|r| r.args.first() == Some(&Value::Object(Some(dialog_surface.id()))))
        .expect("the keyboard entered the dialog");
    assert_eq!(dialog_enter.interface, "ldp.input.keyboard");

    // THE gate: a motion over the parent-only region (the pointer at
    // (100, 50), moved to (20, 20) — inside the parent, outside the
    // dialog) routes nowhere.
    let motion_before = client.events_of("relative_motion").len();
    tb.world_mut(|w| {
        w.queue_input(
            DeviceClass::Mouse,
            vec![InputEvent::PointerMotion { dx: -80, dy: -30 }],
        );
        let routed = w.pump_input();
        assert_eq!(routed, 0, "the gated parent takes no input");
    });
    client.sync();
    assert_eq!(
        client.events_of("relative_motion").len(),
        motion_before,
        "no motion reached the client while gated"
    );

    // The pointer over the DIALOG itself still routes (the dialog is
    // its own target — never gated by its own modality).
    tb.world_mut(|w| {
        w.queue_input(
            DeviceClass::Mouse,
            vec![InputEvent::PointerMotion { dx: 80, dy: 30 }],
        );
        let routed = w.pump_input();
        assert!(routed > 0, "the dialog takes its own input");
    });
    // The dialog's own enter lands before the teardown proceeds (the
    // outbox delivery rides the next wake — waited, never assumed).
    client.wait_until(|c| {
        c.records.iter().any(|r| {
            r.event == "enter" && r.args.first() == Some(&Value::Object(Some(dialog_surface.id())))
        })
    });
    client.sync();

    // The dialog dies: the client destroys the object and the
    // surface — the gate lifts, the parent's input returns.
    client.conn.destroy(&dialog).expect("destroy dialog");
    client
        .conn
        .destroy(&dialog_surface)
        .expect("destroy surface");
    // The gate's return, both truths (Phase 51 made the key half
    // real): the dying dialog's focus hands the keys to the
    // frontmost remaining window — the parent — so the *keyboard*
    // re-enters it first (the key window restored, the `activated`
    // bit riding its configure), and the pointer's own re-enter
    // names the parent's surface again (the routing gate lifted —
    // the hit-test finds it once more).
    let enters_before = client.events_of("enter").len();
    client.sync();
    client.wait_until(|c| {
        c.records.iter().any(|r| {
            r.event == "enter"
                && r.interface == "ldp.input.keyboard"
                && r.args.first() == Some(&Value::Object(Some(parent.id())))
        })
    });
    let key_return = last_keyboard_enter_after(&client, enters_before, parent.id());
    assert_eq!(key_return.interface, "ldp.input.keyboard");
    // The parent's own activated round trip: the bit set at the
    // click, cleared at the dialog's handoff, set again at the
    // return — the configure count grew twice since the mint.
    let proposal = last_on(&client, "configure", parent_toplevel.id().as_u32());
    assert!(
        states_bit(proposal, 3),
        "the parent's latest configure carries the activated flag"
    );
    // The pointer re-enters that are already on the wire (the
    // initial one from the first motion); the wait below must fire
    // on a *new* one.
    let parent_ptr_enters_before = client
        .events_of("enter")
        .iter()
        .filter(|r| {
            r.interface == "ldp.input.pointer"
                && r.args.first() == Some(&Value::Object(Some(parent.id())))
        })
        .count();
    tb.world_mut(|w| {
        w.queue_input(
            DeviceClass::Mouse,
            vec![InputEvent::PointerMotion { dx: -85, dy: -35 }],
        );
        let routed = w.pump_input();
        assert!(routed > 0, "the parent's input returned");
    });
    // The pointer's re-enter names the parent's surface again (the
    // routing gate lifted — the hit-test finds it once more).
    client.wait_until(|c| {
        c.records
            .iter()
            .filter(|r| {
                r.event == "enter"
                    && r.interface == "ldp.input.pointer"
                    && r.args.first() == Some(&Value::Object(Some(parent.id())))
            })
            .count()
            > parent_ptr_enters_before
    });
    let enters = client.events_of("enter");
    let reenter = enters
        .iter()
        .skip(enters_before)
        .find(|r| {
            r.interface == "ldp.input.pointer"
                && r.args.first() == Some(&Value::Object(Some(parent.id())))
        })
        .unwrap_or_else(|| {
            panic!(
                "no parent re-enter; the enters were: {:#?}",
                enters
                    .iter()
                    .skip(enters_before)
                    .map(|r| (r.interface.clone(), r.target, r.args.clone()))
                    .collect::<Vec<_>>()
            )
        });
    assert_eq!(reenter.interface, "ldp.input.pointer");
}

/// A dialog cannot outlive its window: the parent's death closes the
/// dialog (`dialog.close`), and the machine's gate dies with it.
#[test]
fn the_parents_death_closes_the_dialog() {
    let tb = Testbench::start_with("dialog-orphan", dialog_config());
    let mut client = TestClient::connect(&tb.addr);
    let shell = client.bind("ldp.shell.shell");

    // The parent and its modal dialog.
    let (parent, _b, _p) = map_window(&tb, &mut client, xrgb(90, 90, 90), 200, 100);
    let parent_toplevel = get_toplevel(&mut client, &shell, &parent);
    client.sync();
    let compositor = client.bind("ldp.core.compositor");
    let dialog_surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("surface");
    let dialog = get_dialog(&mut client, &shell, &dialog_surface, &parent_toplevel, 1);
    wait_for_on(&mut client, "configure", dialog.id().as_u32());
    let Value::Uint32(serial) = last_on(&client, "configure", dialog.id().as_u32()).args[0] else {
        panic!("the configure carries a serial");
    };
    ack_configure(&mut client, &dialog, serial);
    client.sync();
    assert!(
        tb.world(|w| w.dialogs.gating_pairs().len() == 1),
        "the modal dialog gates its parent"
    );

    // The parent dies: the dialog closes.
    client.conn.destroy(&parent).expect("destroy parent");
    client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "close" && r.target == dialog.id().as_u32())
    });
    // The gate died with it.
    assert!(
        tb.world(|w| w.dialogs.gating_pairs().is_empty()),
        "the closed dialog gates nothing"
    );
}

/// The modeless control: a modeless dialog is a window, not a gate —
/// the parent's input never stops routing.
#[test]
fn a_modeless_dialog_gates_nothing() {
    let tb = Testbench::start_with("dialog-modeless", dialog_config());
    let mut client = TestClient::connect(&tb.addr);
    let shell = client.bind("ldp.shell.shell");

    let (parent, _b, _p) = map_window(&tb, &mut client, xrgb(90, 90, 90), 200, 100);
    let parent_toplevel = get_toplevel(&mut client, &shell, &parent);
    client.sync();

    // The modeless dialog, mapped.
    let ivory = xrgb(240, 240, 220);
    let shm = client.bind("ldp.core.shm");
    let pixels: Vec<u8> = std::iter::repeat(ivory.to_le_bytes())
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
    let compositor = client.bind("ldp.core.compositor");
    let dialog_surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("surface");
    let dialog = get_dialog(&mut client, &shell, &dialog_surface, &parent_toplevel, 2);
    wait_for_on(&mut client, "configure", dialog.id().as_u32());
    let Value::Uint32(serial) = last_on(&client, "configure", dialog.id().as_u32()).args[0] else {
        panic!("the configure carries a serial");
    };
    ack_configure(&mut client, &dialog, serial);
    let before = tb.frames();
    attach(&mut client, &dialog_surface, &buffer);
    damage(&mut client, &dialog_surface, &[Rect::new(0, 0, 100, 80)]);
    commit(&mut client, &dialog_surface, 1);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while tb.frames() <= before {
        assert!(deadline > std::time::Instant::now(), "never rendered");
        client.sync();
    }
    assert!(
        tb.world(|w| w.dialogs.gating_pairs().is_empty()),
        "the modeless dialog gates nothing"
    );

    // The parent's input still routes (the pointer into the
    // parent-only region).
    let seat = client.bind("ldp.input.seat");
    let _pointer = client
        .conn
        .create_object(&seat, "get_pointer", vec![])
        .expect("pointer");
    client.sync();
    tb.world_mut(|w| {
        w.queue_input(
            DeviceClass::Mouse,
            vec![InputEvent::PointerMotion { dx: 20, dy: 20 }],
        );
        let routed = w.pump_input();
        assert!(routed > 0, "the modeless parent takes its motion");
    });
    // The modeless dialog is a real window, on the screen (its ink at
    // the centered rect), while the parent still routes — the role is
    // real, only the gate is absent.
    let words = tb.scanout();
    assert_eq!(px(&words, 60, 20)[0], 240, "the modeless dialog's ink");
    assert_eq!(px(&words, 10, 10)[2], 90, "the parent's ink around it");
}
