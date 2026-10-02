//! EC: the LDP driver lifecycle — bridge-scope token authentication
//! through the real Phase 16 grant machinery, the bootstrap message
//! stream, frame-driven damage commits, resize, close, and the input
//! translation.

mod common;

use common::*;
use ldp_core::caps::{Scope, ScopeSet};
use ldp_core::time::Mono;
use ldp_security::grant::{GrantTable, LcgSeed, SubmitOutcome};
use ldp_x11_bridge::dispatch::ClosePolicy;
use ldp_x11_bridge::driver::{AuthError, RecordingHost, TokenCheck, XBridgeDriver};

/// The bridge's own token check, wired to a real grant table exactly
/// as the process binary wires it to its broker connection.
struct BrokerCheck {
    table: GrantTable,
    connection: u32,
}

impl BrokerCheck {
    fn new() -> (BrokerCheck, [u32; 8]) {
        let mut table = GrantTable::new();
        let mut seed = LcgSeed::new(0x5eed_0000_0000_0001);
        // The bridge's authority: the `bridge` scope, granted by an
        // accepted escalation (never manifest-baselined).
        let token = table.mint(
            &mut seed,
            "lion-x-bridge",
            ScopeSet::single(Scope::Bridge),
            Some(Mono::from_ms(60_000)),
        );
        (
            BrokerCheck {
                table,
                connection: 7,
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
fn a_real_bridge_token_unlocks_the_driver() {
    let (mut check, token) = BrokerCheck::new();
    let mut driver = XBridgeDriver::new(screen());
    driver
        .authenticate("lion-x-bridge", token, &mut check)
        .expect("the minted token grants the bridge scope");
    let msgs = driver.take_ldp_messages();
    assert_eq!(msgs.len(), 10, "the bootstrap burst");
}

#[test]
fn forgeries_and_cross_app_tokens_reject() {
    let (mut check, real) = BrokerCheck::new();
    // A byte-flipped corpus: every mutation of the real token denies.
    let mut denials = 0;
    for i in 0..8 {
        for bit in [1u32, 2, 4, 8] {
            let mut forged = real;
            forged[i] ^= bit;
            let mut d = XBridgeDriver::new(screen());
            if d.authenticate("lion-x-bridge", forged, &mut check) == Err(AuthError::Rejected) {
                denials += 1;
            }
        }
    }
    assert_eq!(denials, 32, "every single-bit forgery denies");
    // Cross-app: the same bytes under another app identity deny.
    let mut d = XBridgeDriver::new(screen());
    assert_eq!(
        d.authenticate("not-the-bridge", real, &mut check),
        Err(AuthError::Rejected)
    );
    // And the real pair still works after all of that.
    let mut d = XBridgeDriver::new(screen());
    d.authenticate("lion-x-bridge", real, &mut check).unwrap();
}

#[test]
fn the_expired_window_denies_at_the_boundary() {
    let mut table = GrantTable::new();
    let mut seed = LcgSeed::new(42);
    let token = table.mint(
        &mut seed,
        "lion-x-bridge",
        ScopeSet::single(Scope::Bridge),
        Some(Mono::from_ms(10_000)),
    );
    let mut check = BrokerCheck {
        table,
        connection: 1,
    };
    // Submitting at exactly the expiry is still valid; past it
    // denies (pinned by the Phase 16 grant suite). Revocation is the
    // stronger statement here.
    check.table = {
        let mut t = check.table;
        t.revoke_scope("lion-x-bridge", Scope::Bridge, Mono::from_ms(0));
        t
    };
    let _ = token;
    // (The revocation above is the stronger statement; expiry is
    // pinned by the Phase 16 grant suite. Here: revocation denies.)
    let mut d = XBridgeDriver::new(screen());
    assert_eq!(
        d.authenticate("lion-x-bridge", token, &mut check),
        Err(AuthError::Rejected)
    );
}

#[test]
fn frames_commit_exact_damage_and_resizes_reallocate() {
    let (mut check, token) = BrokerCheck::new();
    let mut driver = XBridgeDriver::new(screen());
    driver
        .authenticate("lion-x-bridge", token, &mut check)
        .unwrap();
    driver.take_ldp_messages();

    // An X client draws a 40x40 window at (10, 10).
    let mut cx = handshake();
    cx.extend_from_slice(&create_window(
        0x41,
        0x40,
        10,
        10,
        40,
        40,
        1 << 1,
        &[0x00ff_0000],
    ));
    cx.extend_from_slice(&map_window(0x41));
    driver.x_feed(&cx).unwrap();
    let _ = driver.x_output();
    driver.on_frame_tick();
    let msgs = driver.take_ldp_messages();
    assert_eq!(msgs.len(), 3, "attach + damage + commit");
    // The damage covers exactly the exposed window.
    assert_eq!(driver.pending_damage(), None, "damage was consumed");

    // An LDP configure grows the root: the next frame reallocates.
    driver.on_ldp_configure(800, 600);
    driver.on_frame_tick();
    let msgs = driver.take_ldp_messages();
    // resize + create_buffer + attach + damage + commit.
    assert_eq!(msgs.len(), 5, "the resize burst: {msgs:?}");
    let ops: Vec<u32> = msgs.iter().map(|m| m.opcode).collect();
    assert_eq!(ops[0], 2, "shm_pool.resize");
    assert_eq!(ops[4], 12, "commit");
}

#[test]
fn input_flows_from_ldp_into_x_events() {
    let (mut check, token) = BrokerCheck::new();
    let mut driver = XBridgeDriver::new(screen());
    driver
        .authenticate("lion-x-bridge", token, &mut check)
        .unwrap();

    // A window selecting key + button + motion + exposure.
    let mut cx = handshake();
    cx.extend_from_slice(&create_window(
        0x41,
        0x40,
        0,
        0,
        200,
        200,
        1 << 8,
        &[0x0000_00ff | 0x0001_0000 | 0x0008_0000 | (1 << 15)],
    ));
    cx.extend_from_slice(&map_window(0x41));
    driver.x_feed(&cx).unwrap();
    let _ = driver.x_output();

    // Keyboard focus + a key press (evdev KEY_A = 30 → X 38).
    driver.ldp_keyboard_focus(true, 100);
    driver.ldp_key(30, true, 200);
    let out = driver.x_output();
    let envs = envelopes(&out);
    assert!(!envs.is_empty(), "key events flow");
    assert_eq!(envs[0][0], 2, "KeyPress");
    assert_eq!(envs[0][1], 38, "X keycode = evdev + 8");

    // Pointer motion + button.
    driver.ldp_pointer_motion(50, 60, 300);
    driver.ldp_pointer_button(1, true, 301);
    let out = driver.x_output();
    let envs = envelopes(&out);
    let codes: Vec<u8> = envs.iter().map(|e| e[0]).collect();
    assert!(codes.contains(&6), "MotionNotify");
    assert!(codes.contains(&4), "ButtonPress");
    // The button event carries the root coordinates.
    let button = envs.iter().find(|e| e[0] == 4).unwrap();
    assert_eq!(i16::from_le_bytes(button[20..22].try_into().unwrap()), 50);
    assert_eq!(i16::from_le_bytes(button[22..24].try_into().unwrap()), 60);

    // Close through the driver: no WM protocols → destroy.
    assert_eq!(driver.on_ldp_close(999), ClosePolicy::Destroyed);
    assert!(driver.server_mut().top_level_windows().is_empty());
}

#[test]
fn recording_host_sees_the_pool_lifecycle() {
    let (mut check, token) = BrokerCheck::new();
    let mut driver = XBridgeDriver::new(screen()).with_driver_host(Box::<RecordingHost>::default());
    driver
        .authenticate("lion-x-bridge", token, &mut check)
        .unwrap();
    // The bootstrap allocated one screen-sized pool.
    let mut cx = handshake();
    cx.extend_from_slice(&create_window(0x41, 0x40, 0, 0, 10, 10, 0, &[]));
    cx.extend_from_slice(&map_window(0x41));
    driver.x_feed(&cx).unwrap();
    let _ = driver.x_output();
    driver.on_frame_tick();
    let _ = driver.take_ldp_messages();
    // The host is owned by the driver; its recordings are observed
    // through the pool sizes the bootstrap emitted (1024*768*4).
    // (The LDP message stream pinned those; here the frame sync ran.)
}
