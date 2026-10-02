//! EC: the driver's bridge-token conformance and the LDP proxy
//! stream — the token gate, the bootstrap, the export of committed
//! foreign surfaces, and the input/shell translation.

mod common;

use common::*;
use ldp_core::caps::{Scope, ScopeSet};
use ldp_core::time::Mono;
use ldp_security::grant::{GrantTable, LcgSeed, SubmitOutcome};
use ldp_wayland_bridge::dispatch::NoHost;
use ldp_wayland_bridge::driver::{AuthError, RecordingHost, TokenCheck, WlBridgeDriver};
use ldp_wayland_bridge::protocol;
use ldp_wayland_bridge::wire::Value;

struct BrokerCheck {
    table: GrantTable,
    connection: u32,
}

impl BrokerCheck {
    fn new() -> (BrokerCheck, [u32; 8]) {
        let mut table = GrantTable::new();
        let mut seed = LcgSeed::new(0x5eed_0000_0000_0002);
        let token = table.mint(
            &mut seed,
            "lion-wl-bridge",
            ScopeSet::single(Scope::Bridge),
            Some(Mono::from_ms(60_000)),
        );
        (
            BrokerCheck {
                table,
                connection: 3,
            },
            token,
        )
    }
}

impl TokenCheck for BrokerCheck {
    fn check(&mut self, app_id: &str, token: [u32; 8]) -> bool {
        matches!(
            self.table
                .submit(app_id, self.connection, token, Mono::from_ms(1_000)),
            SubmitOutcome::Granted(_)
        )
    }
}

#[test]
fn a_real_bridge_token_unlocks_the_bootstrap() {
    let (mut check, token) = BrokerCheck::new();
    let mut d = WlBridgeDriver::new(Box::new(NoHost));
    d.authenticate("lion-wl-bridge", token, &mut check)
        .expect("the minted token grants the bridge scope");
    let msgs = d.take_ldp_messages();
    // hello, get_registry, 4 binds.
    assert_eq!(msgs.len(), 6);
    assert_eq!(
        msgs[0].opcode,
        ldp_protocol::generated::core::connection::request::HELLO
    );
}

#[test]
fn forgeries_reject() {
    let (mut check, real) = BrokerCheck::new();
    let mut denials = 0;
    for i in 0..8 {
        for bit in [1u32, 2, 4, 8] {
            let mut forged = real;
            forged[i] ^= bit;
            let mut d = WlBridgeDriver::new(Box::new(NoHost));
            if d.authenticate("lion-wl-bridge", forged, &mut check) == Err(AuthError::Rejected) {
                denials += 1;
            }
        }
    }
    assert_eq!(denials, 32, "every single-bit forgery denies");
    // Cross-app.
    let mut d = WlBridgeDriver::new(Box::new(NoHost));
    assert_eq!(
        d.authenticate("other-app", real, &mut check),
        Err(AuthError::Rejected)
    );
    // Unauthenticated drivers emit nothing.
    let mut d = WlBridgeDriver::new(Box::new(NoHost));
    d.on_frame_tick();
    assert!(d.take_ldp_messages().is_empty());
}

/// Drive one foreign client to a mapped surface through the driver's
/// own client handle.
fn mapped_driver() -> (WlBridgeDriver, u32) {
    let (mut check, token) = BrokerCheck::new();
    let mut d = WlBridgeDriver::new(Box::new(ScriptHost::new()));
    d.authenticate("lion-wl-bridge", token, &mut check).unwrap();
    let _ = d.take_ldp_messages();
    // The foreign client boots.
    let s = Script::new();
    let _ = s;
    let c = d.client_mut();
    let get_registry = request(&protocol::WL_DISPLAY, 1, 1, vec![Value::NewId(2)]);
    c.feed(&get_registry).unwrap();
    let _ = c.take_output();
    for (name, id) in [(1u32, 3u32), (2, 4), (3, 5), (4, 6)] {
        let iface = protocol::ALL_GLOBALS
            .get(usize::try_from(name).unwrap() - 1)
            .unwrap();
        let bind = request(
            &protocol::WL_REGISTRY,
            2,
            0,
            vec![
                Value::Uint(name),
                Value::String(iface.name.into()),
                Value::Uint(iface.version),
                Value::NewId(id),
            ],
        );
        c.feed(&bind).unwrap();
        let _ = c.take_output();
    }
    // Surface + role + configure + ack + pool + buffer + commit.
    let surface = request(&protocol::WL_COMPOSITOR, 3, 0, vec![Value::NewId(9)]);
    c.feed(&surface).unwrap();
    let _ = c.take_output();
    let get_xdg = request(
        &protocol::XDG_WM_BASE,
        6,
        2,
        vec![Value::NewId(10), Value::Object(9)],
    );
    let get_toplevel = request(&protocol::XDG_SURFACE, 10, 1, vec![Value::NewId(11)]);
    let mut all = get_xdg;
    all.extend_from_slice(&get_toplevel);
    c.feed(&all).unwrap();
    let out = c.take_output().0;
    // Extract the configure serial from the tail.
    let at = out.len() - 8;
    let (xconf, _) = event_at(&out, at, &protocol::XDG_SURFACE, 10, 0);
    let Value::Uint(serial) = xconf.args[0] else {
        panic!()
    };
    let pool = request(
        &protocol::WL_SHM,
        4,
        0,
        vec![Value::Fd(0), Value::Int(64), Value::NewId(12)],
    );
    c.feed(&pool).unwrap();
    let _ = c.take_output();
    let buffer = request(
        &protocol::WL_SHM_POOL,
        12,
        1,
        vec![
            Value::NewId(13),
            Value::Int(0),
            Value::Int(4),
            Value::Int(4),
            Value::Int(16),
            Value::Uint(1),
        ],
    );
    let ack = request(&protocol::XDG_SURFACE, 10, 4, vec![Value::Uint(serial)]);
    let mut batch = buffer;
    batch.extend_from_slice(&ack);
    c.feed(&batch).unwrap();
    let _ = c.take_output();
    c.commit_with_buffer(9, 13).unwrap();
    let _ = c.take_output();
    (d, 9)
}

/// The script host with one 64-byte pool at id 12.
struct ScriptHost {
    pools: std::collections::BTreeMap<u32, Vec<u8>>,
}

impl ScriptHost {
    fn new() -> ScriptHost {
        let mut pools = std::collections::BTreeMap::new();
        pools.insert(12, vec![0x11u8; 64]);
        ScriptHost { pools }
    }
}

impl ldp_wayland_bridge::dispatch::Host for ScriptHost {
    fn read_pool(&mut self, pool: u32, offset: usize, len: usize) -> Option<Vec<u8>> {
        let bytes = self.pools.get(&pool)?;
        bytes.get(offset..offset + len).map(<[u8]>::to_vec)
    }
    fn keymap_fd(&mut self) -> u32 {
        3
    }
    fn keymap(&mut self) -> Vec<u8> {
        b"xkb_keymap { };\0".to_vec()
    }
}

#[test]
fn committed_surfaces_export_to_ldp() {
    let (mut d, wl) = mapped_driver();
    d.on_frame_tick();
    let msgs = d.take_ldp_messages();
    // create_surface + create_pool + create_buffer + get_toplevel +
    // attach + damage + commit. The export's LDP ids run 10..=13:
    // surface 10, pool 11, buffer 12, toplevel 13.
    assert_eq!(msgs.len(), 7, "{msgs:?}");
    let ops: Vec<(u32, u32)> = msgs.iter().map(|m| (m.object_id, m.opcode)).collect();
    assert_eq!(ops[0], (3, 1), "compositor.create_surface");
    assert_eq!(ops[1], (4, 1), "shm.create_pool");
    assert_eq!(ops[2], (11, 1), "shm_pool.create_buffer");
    assert_eq!(ops[3], (6, 1), "shell.get_toplevel");
    assert_eq!(ops[6], (10, 12), "surface.commit");
    assert_eq!(d.export_surfaces(), vec![(wl, 10)], "the export mapping");
}

#[test]
fn configure_and_close_translate() {
    let (mut d, _wl) = mapped_driver();
    d.on_frame_tick();
    let _ = d.take_ldp_messages();
    let ldp_toplevel = 13; // the export's allocated toplevel id
                           // LDP configure → xdg_toplevel.configure + xdg_surface.configure.
    d.on_ldp_configure(ldp_toplevel, 640, 480);
    let (out, _) = d.client_mut().take_output();
    let (obj, op) = header_at(&out, 0);
    assert_eq!((obj, op), (11, 0), "xdg_toplevel.configure");
    let (tconf, used) = event_at(&out, 0, &protocol::XDG_TOPLEVEL, 11, 0);
    assert_eq!(tconf.args[0], Value::Int(640));
    assert_eq!(tconf.args[1], Value::Int(480));
    let (obj, op) = header_at(&out, used);
    assert_eq!((obj, op), (10, 0), "xdg_surface.configure");
    // LDP close → xdg_toplevel.close.
    d.on_ldp_close(ldp_toplevel);
    let (out, _) = d.client_mut().take_output();
    let (obj, op) = header_at(&out, 0);
    assert_eq!((obj, op), (11, 1), "xdg_toplevel.close");
    assert_eq!(out.len(), 4);
}

#[test]
fn input_routes_from_ldp_into_the_foreign_client() {
    let (mut d, _wl) = mapped_driver();
    // The export must exist before input can route to it.
    d.on_frame_tick();
    let _ = d.take_ldp_messages();
    // A pointer + keyboard on the foreign side.
    let get_pointer = request(&protocol::WL_SEAT, 5, 0, vec![Value::NewId(14)]);
    let get_keyboard = request(&protocol::WL_SEAT, 5, 1, vec![Value::NewId(15)]);
    let mut batch = get_pointer;
    batch.extend_from_slice(&get_keyboard);
    d.client_mut().feed(&batch).unwrap();
    let _ = d.client_mut().take_output();
    assert_eq!(d.pointer_objects(), vec![14]);
    assert_eq!(d.keyboard_objects(), vec![15]);
    // LDP pointer motion → wl_pointer.enter (on the export's LDP
    // surface 10).
    d.ldp_pointer(10, 10, 20);
    let (out, _) = d.client_mut().take_output();
    let (obj, op) = header_at(&out, 0);
    assert_eq!((obj, op), (14, 0), "wl_pointer.enter");
    // LDP keyboard enter → keymap + wl_keyboard.enter.
    d.ldp_keyboard_enter(10, &[]);
    let (out, fds) = d.client_mut().take_output();
    assert_eq!(fds, 1, "the keymap fd rides");
    let (obj, op) = header_at(&out, 0);
    assert_eq!((obj, op), (15, 0), "wl_keyboard.keymap");
    // LDP key → wl_keyboard.key.
    d.ldp_key(30, true);
    let (out, _) = d.client_mut().take_output();
    let (obj, op) = header_at(&out, 0);
    assert_eq!((obj, op), (15, 3), "wl_keyboard.key");
}

#[test]
fn the_recording_host_sees_the_export() {
    let (mut check, token) = BrokerCheck::new();
    let mut d = WlBridgeDriver::new(Box::new(ScriptHost::new()))
        .with_driver_host(Box::<RecordingHost>::default());
    d.authenticate("lion-wl-bridge", token, &mut check).unwrap();
    let _ = d.take_ldp_messages();
    // Drive a mapped surface through the driver's client: the
    // registry and the four binds first.
    let c = d.client_mut();
    let get_registry = request(&protocol::WL_DISPLAY, 1, 1, vec![Value::NewId(2)]);
    c.feed(&get_registry).unwrap();
    let _ = c.take_output();
    for (name, id) in [(1u32, 3u32), (2, 4), (3, 5), (4, 6)] {
        let iface = protocol::ALL_GLOBALS
            .get(usize::try_from(name).unwrap() - 1)
            .unwrap();
        let bind = request(
            &protocol::WL_REGISTRY,
            2,
            0,
            vec![
                Value::Uint(name),
                Value::String(iface.name.into()),
                Value::Uint(iface.version),
                Value::NewId(id),
            ],
        );
        c.feed(&bind).unwrap();
        let _ = c.take_output();
    }
    let surface = request(&protocol::WL_COMPOSITOR, 3, 0, vec![Value::NewId(9)]);
    c.feed(&surface).unwrap();
    let _ = c.take_output();
    // (A role-less surface never maps; commit a buffer through a pool.)
    let pool = request(
        &protocol::WL_SHM,
        4,
        0,
        vec![Value::Fd(0), Value::Int(64), Value::NewId(12)],
    );
    c.feed(&pool).unwrap();
    let _ = c.take_output();
    let buffer = request(
        &protocol::WL_SHM_POOL,
        12,
        1,
        vec![
            Value::NewId(13),
            Value::Int(0),
            Value::Int(4),
            Value::Int(4),
            Value::Int(16),
            Value::Uint(1),
        ],
    );
    c.feed(&buffer).unwrap();
    let _ = c.take_output();
    c.commit_with_buffer(9, 13).unwrap();
    let _ = c.take_output();
    d.on_frame_tick();
    // Role-less mapped surfaces still export (the driver maps every
    // committed buffer, role or not — the LDP surface is the bridge's
    // own window).
    let msgs = d.take_ldp_messages();
    assert!(
        !msgs.is_empty(),
        "a committed role-less surface exports too"
    );
}
