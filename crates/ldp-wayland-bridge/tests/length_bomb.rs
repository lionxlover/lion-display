//! The length-bomb regression suite.
//!
//! The Phase 19 fuzzer surfaced a real gap: the bridge's input buffer
//! grew without bound while a decoder `Incomplete` waited for a
//! message that could never fit — a client declaring a ~2 GiB string
//! or array length (and then streaming slowly) wedged unbounded
//! memory growth into the server. The fix (`dispatch.rs`) mirrors the
//! X11 bridge's `max_request_bytes()` doctrine:
//!
//! * a *per-message* guard — an `Incomplete(need)` bound past one LDP
//!   `large_messages` frame is fatal immediately (a message that
//!   large could never be forwarded whole),
//! * a *total-buffer* guard — the input buffer itself never grows
//!   past the ceiling (defense in depth against future schema or
//!   decoder drift),
//! * and the *control*: ordinary partial delivery — split frames that
//!   fit — still completes and dispatches.

mod common;

use ldp_wayland_bridge::dispatch::{Client, Host};
use ldp_wayland_bridge::protocol;
use ldp_wayland_bridge::wire::{self, max_message_bytes, Message, Value};

/// A host with no pools: these tests never reach pool reads.
struct NullHost;

impl Host for NullHost {
    fn read_pool(&mut self, _pool: u32, _offset: usize, _len: usize) -> Option<Vec<u8>> {
        None
    }
    fn keymap_fd(&mut self) -> u32 {
        0
    }
    fn keymap(&mut self) -> Vec<u8> {
        b"xkb_keymap { };\0".to_vec()
    }
}

/// The opcode of `name` among `iface`'s requests.
fn req_opcode(iface: &protocol::Interface, name: &str) -> u32 {
    iface.requests.iter().find(|m| m.name == name).map_or_else(
        || panic!("no request '{name}' on {}", iface.name),
        |m| m.opcode,
    )
}

/// Encode one request through the real wire encoder.
fn request(iface: &protocol::Interface, object_id: u32, opcode: u32, args: Vec<Value>) -> Vec<u8> {
    let schema = iface
        .requests
        .iter()
        .find(|m| m.opcode == opcode)
        .expect("request opcode exists");
    let msg = Message {
        object_id,
        opcode,
        args,
    };
    wire::encode(&msg, schema.args).0
}

/// A `wl_registry.bind` bomb: header + uint name + a string length
/// field claiming `claimed` bytes. The registry's bind signature is
/// `[uint, string, uint, new_id]`, so the string length sits at
/// offset 8.
fn bind_bomb(claimed: u32) -> Vec<u8> {
    let bind_opcode = req_opcode(&protocol::WL_REGISTRY, "bind");
    let mut bomb = vec![0u8; 12];
    bomb[0..4].copy_from_slice(&wire::header(2, bind_opcode).to_le_bytes());
    bomb[4..8].copy_from_slice(&1u32.to_le_bytes()); // the uint name
    bomb[8..12].copy_from_slice(&claimed.to_le_bytes());
    bomb
}

/// Bootstrap a client through `wl_display.get_registry` so object 2
/// (the registry) exists for the bind-shaped bombs.
fn bootstrapped() -> Client {
    let mut client = Client::new(Box::new(NullHost));
    let opcode = req_opcode(&protocol::WL_DISPLAY, "get_registry");
    let bytes = request(&protocol::WL_DISPLAY, 1, opcode, vec![Value::NewId(2)]);
    client.feed(&bytes).expect("get_registry dispatches");
    client
}

#[test]
fn huge_string_length_claim_is_fatal_not_buffered() {
    let mut client = bootstrapped();
    let verdict = client.feed(&bind_bomb(0x7F00_0000));
    assert!(
        verdict.is_err(),
        "a length claim past the ceiling must be fatal, not buffered"
    );
    // And the connection stays dead for further input.
    assert!(client.feed(&[0u8; 16]).is_err());
}

#[test]
fn the_ceiling_is_one_large_frame() {
    // The doctrine: a foreign message larger than an LDP large frame
    // can never be forwarded whole — same bound the X11 bridge
    // enforces (`x11::wire::max_request_bytes`).
    assert_eq!(max_message_bytes(), 64 * 1024 * 1024);
    // And the codec is honest about the bound it needs: the huge
    // claim decodes as Incomplete(~2 GiB), never silently.
    let mut bytes = bind_bomb(0x7F00_0000);
    let schema = protocol::WL_REGISTRY
        .requests
        .iter()
        .find(|m| m.name == "bind")
        .expect("bind schema");
    let verdict = wire::decode(&bytes, 2, schema.opcode, schema.args);
    assert!(matches!(
        verdict,
        Err(wire::DecodeStatus::Incomplete(need)) if need > 0x7F00_0000
    ));
    bytes.resize(16, 0);
    let _ = &mut bytes; // silence unused-assignments in profiles
}

#[test]
fn total_buffer_ceiling_is_fatal() {
    let mut client = bootstrapped();
    // A bind whose string claims 63 MiB: under the per-message
    // ceiling, so the buffer grows as bytes arrive — the total guard
    // must stop it at the ceiling.
    assert!(
        client.feed(&bind_bomb(63 * 1024 * 1024)).is_ok(),
        "a sub-ceiling claim starts buffered (the normal partial path)"
    );
    let chunk = vec![0u8; 1024 * 1024];
    let mut fatal = false;
    for _ in 0..70 {
        if client.feed(&chunk).is_err() {
            fatal = true;
            break;
        }
    }
    assert!(
        fatal,
        "the total-buffer guard must fire before ~70 MiB of streaming"
    );
}

#[test]
fn ordinary_partial_delivery_still_completes() {
    let mut client = Client::new(Box::new(NullHost));
    let opcode = req_opcode(&protocol::WL_DISPLAY, "get_registry");
    let bytes = request(&protocol::WL_DISPLAY, 1, opcode, vec![Value::NewId(2)]);
    // Split the 8-byte message 5/3.
    assert!(
        client.feed(&bytes[..5]).is_ok(),
        "a partial frame is buffered, not fatal"
    );
    assert!(
        client.feed(&bytes[5..]).is_ok(),
        "completing the frame dispatches normally"
    );
    // The registry object now exists: a huge bind claim against it
    // proves the pipeline is alive and still guarded.
    assert!(client.feed(&bind_bomb(0x7F00_0000)).is_err());
}
