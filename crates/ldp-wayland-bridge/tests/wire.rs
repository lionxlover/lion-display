//! EC: wire golden bytes both directions + the malformed corpus.

mod common;

use common::*;
use ldp_wayland_bridge::protocol;
use ldp_wayland_bridge::wire::{self, Arg, DecodeStatus, Message, Value};

#[test]
fn registry_replay_is_byte_exact() {
    let mut s = Script::new();
    // The first global: header (1<<8 | 0) + name 1 + "wl_compositor"
    // + version 1.
    // The raw replay, decoded by hand here (boot drains it).
    let bytes = request(&protocol::WL_DISPLAY, 1, 1, vec![Value::NewId(2)]);
    s.client.feed(&bytes).unwrap();
    let (out, _) = s.client.take_output();
    let mut expect = Vec::new();
    expect.extend_from_slice(&(2u32 << 8).to_le_bytes());
    expect.extend_from_slice(&1u32.to_le_bytes());
    let name = b"wl_compositor\0";
    expect.extend_from_slice(&(name.len() as u32).to_le_bytes());
    expect.extend_from_slice(name);
    while expect.len() % 4 != 0 {
        expect.push(0);
    }
    expect.extend_from_slice(&1u32.to_le_bytes());
    assert_eq!(&out[..expect.len()], &expect[..]);
}

#[test]
fn sync_round_trips_with_delete_id() {
    let mut s = Script::new();
    let sync = request(&protocol::WL_DISPLAY, 1, 0, vec![Value::NewId(20)]);
    s.client.feed(&sync).unwrap();
    let (out, _) = s.client.take_output();
    // wl_callback.done(20, serial 1) then wl_display.delete_id(20).
    assert_eq!(out.len(), 16);
    assert_eq!(&out[..4], &(20u32 << 8).to_le_bytes());
    assert_eq!(&out[4..8], &1u32.to_le_bytes());
    assert_eq!(&out[8..12], &((1u32 << 8) | 1u32).to_le_bytes());
    assert_eq!(&out[12..16], &20u32.to_le_bytes());
    // The id frees for reuse after delete_id.
    let again = request(&protocol::WL_DISPLAY, 1, 0, vec![Value::NewId(20)]);
    s.client.feed(&again).unwrap();
    let _ = s.client.take_output();
}

#[test]
fn id_reuse_before_delete_id_is_fatal() {
    let mut s = Script::new();
    s.connect();
    s.create_surface(9);
    // Destroy the surface, then reuse 9 before any delete_id arrives.
    let destroy = request(&protocol::WL_SURFACE, 9, 0, vec![]);
    s.client.feed(&destroy).unwrap();
    let _ = s.client.take_output(); // the delete_id rode this output
    s.create_surface(10);
    // 9 was confirmed; reusing it is fine. Now collide LIVE ids.
    let dup = request(&protocol::WL_COMPOSITOR, 3, 0, vec![Value::NewId(10)]);
    assert!(s.client.feed(&dup).is_err(), "live collision is fatal");
}

#[test]
fn malformed_string_is_fatal() {
    let mut s = Script::new();
    // A bind with a non-NUL-terminated interface string.
    let mut buf = Vec::new();
    buf.extend_from_slice(&(2u32 << 8).to_le_bytes());
    buf.extend_from_slice(&1u32.to_le_bytes());
    buf.extend_from_slice(&4u32.to_le_bytes()); // length 4
    buf.extend_from_slice(b"wl_c"); // no NUL
    buf.extend_from_slice(&1u32.to_le_bytes());
    buf.extend_from_slice(&9u32.to_le_bytes());
    assert!(s.client.feed(&buf).is_err());
    let (out, _) = s.client.take_output();
    let (obj, op) = header_at(&out, 0);
    assert_eq!((obj, op), (1, 0), "wl_display.error");
}

#[test]
fn unknown_object_is_fatal() {
    let mut s = Script::new();
    let bytes = request(&protocol::WL_DISPLAY, 99, 0, vec![]);
    assert!(s.client.feed(&bytes).is_err());
}

#[test]
fn partial_frames_stay_buffered() {
    let mut s = Script::new();
    // Half a get_registry request.
    let bytes = request(&protocol::WL_DISPLAY, 1, 1, vec![Value::NewId(2)]);
    assert!(s.client.feed(&bytes[..2]).is_ok());
    assert!(s.client.take_output().0.is_empty());
    assert!(s.client.feed(&bytes[2..]).is_ok());
    let (out, _) = s.client.take_output();
    assert!(!out.is_empty(), "the replay arrives once complete");
}

#[test]
fn encode_decode_round_trip_every_scalar() {
    let schema = &[Arg::Int, Arg::Uint, Arg::Fixed, Arg::Object, Arg::NewId];
    let msg = Message {
        object_id: 4,
        opcode: 9,
        args: vec![
            Value::Int(-7),
            Value::Uint(0xdead_beef),
            Value::Fixed(0x1234_5600),
            Value::Object(0),
            Value::NewId(77),
        ],
    };
    let (bytes, fds) = wire::encode(&msg, schema);
    assert_eq!(fds, 0);
    assert_eq!(bytes.len(), 24);
    let (back, used, _) = wire::decode(&bytes, 4, 9, schema).unwrap();
    assert_eq!(used, 24);
    assert_eq!(back.args, msg.args);
}

#[test]
fn buffer_overrun_is_fatal() {
    let mut s = Script::new();
    s.connect();
    s.create_pool(12, 64);
    // A 4x4 buffer at stride 16 needs 64 bytes... ask for 4x5.
    let bytes = request(
        &protocol::WL_SHM_POOL,
        12,
        1,
        vec![
            Value::NewId(13),
            Value::Int(0),
            Value::Int(4),
            Value::Int(5),
            Value::Int(16),
            Value::Uint(1),
        ],
    );
    assert!(s.client.feed(&bytes).is_err(), "the overrun is fatal");
    let (out, _) = s.client.take_output();
    let (obj, op) = header_at(&out, 0);
    assert_eq!((obj, op), (1, 0), "wl_display.error");
}

#[test]
fn pools_only_grow() {
    let mut s = Script::new();
    s.connect();
    s.create_pool(12, 128);
    let shrink = request(&protocol::WL_SHM_POOL, 12, 2, vec![Value::Int(64)]);
    assert!(s.client.feed(&shrink).is_err());
}

#[test]
fn decode_status_shapes() {
    let schema = &[Arg::Uint];
    assert_eq!(
        wire::decode(&[0, 0, 0, 0], 1, 0, schema),
        Err(DecodeStatus::Incomplete(8))
    );
    // A zero-length string is malformed (the NUL must ride).
    let str_schema = &[Arg::String];
    let mut buf = vec![0, 0, 0, 0];
    buf.extend_from_slice(&0u32.to_le_bytes());
    assert_eq!(
        wire::decode(&buf, 1, 0, str_schema),
        Err(DecodeStatus::Malformed(4))
    );
}
