//! Object lifecycle conformance: bind, destroy, ID reuse, staleness,
//! version pinning — the central lifecycle of `docs/protocol.md` §3.

mod common;

use common::{op, MockClient, TestServer, CONNECTION};
use ldp_core::bitset::Bitset128;
use ldp_core::error::ErrorCode;
use ldp_core::ids::ObjectId;
use ldp_core::wire::Value;
use ldp_protocol::Message;

use ldp_server::{connection_options, GlobalAdvert, ServerConfig};

/// A fully handshaken client with a registry at object 2 (the replay
/// list: registry + output + compositor + shm).
fn session_with_registry(server: &TestServer) -> MockClient {
    let mut c = MockClient::connect(&server.addr);
    c.hello(1, Bitset128::single(connection_options::INTROSPECTION));
    c.expect_welcome();
    c.get_registry(1, 2);
    for _ in 0..4 {
        let _ = c.expect_global();
    }
    c
}

#[test]
fn bind_confirms_with_bound_event() {
    let server = TestServer::start("lifecycle-bind", common::standard_config());
    let mut c = session_with_registry(&server);
    c.bind(2, "ldp.core.output", 1, 9);
    let (interface, version) = c.expect_bound();
    assert_eq!(interface, "ldp.core.output");
    assert_eq!(version, 1);
    c.sync(1);
}

#[test]
fn bind_rejects_unknown_and_unadvertised_interfaces() {
    let server = TestServer::start("lifecycle-bind-bad", common::standard_config());
    let mut c = session_with_registry(&server);

    // Not in the schema at all.
    c.bind(2, "ldp.nope.thing", 1, 9);
    c.expect_fatal(ErrorCode::InvalidInterface);
    drop(c);

    // In the schema but not advertised (surface is not a global).
    let mut c = session_with_registry(&server);
    c.bind(2, "ldp.core.surface", 1, 9);
    c.expect_fatal(ErrorCode::InvalidInterface);
    drop(c);

    // Advertised but out-of-range version.
    let mut c = session_with_registry(&server);
    c.bind(2, "ldp.core.output", 2, 9);
    c.expect_fatal(ErrorCode::UnsupportedVersion);
    drop(c);

    let mut c = session_with_registry(&server);
    c.bind(2, "ldp.core.output", 0, 9);
    c.expect_fatal(ErrorCode::UnsupportedVersion);
}

#[test]
fn new_id_hygiene_is_enforced() {
    let server = TestServer::start("lifecycle-newid", common::standard_config());

    // The reserved bootstrap ID as new_id: invalid_object (stage 3).
    let mut c = session_with_registry(&server);
    let m = Message::new(2, op("ldp.core.registry", "bind"))
        .arg(Value::String("ldp.core.output".into()))
        .arg(Value::Uint32(1))
        .arg(Value::NewId(ObjectId::from_wire(1)));
    c.send(&m);
    c.expect_fatal(ErrorCode::InvalidObject);
    drop(c);

    // Server-range new_id: invalid_object (stage 3 ownership rule).
    let mut c = session_with_registry(&server);
    let m = Message::new(2, op("ldp.core.registry", "bind"))
        .arg(Value::String("ldp.core.output".into()))
        .arg(Value::Uint32(1))
        .arg(Value::NewId(ObjectId::server(0x8000_0009).unwrap()));
    c.send(&m);
    c.expect_fatal(ErrorCode::InvalidObject);
    drop(c);

    // Collision with a live object: invalid_state.
    let mut c = session_with_registry(&server);
    c.bind(2, "ldp.core.output", 1, 9);
    c.expect_bound();
    c.bind(2, "ldp.core.output", 1, 9);
    c.expect_fatal(ErrorCode::InvalidState);
}

#[test]
fn destroy_lifecycle_and_double_destroy() {
    let server = TestServer::start("lifecycle-destroy", common::standard_config());
    let mut c = session_with_registry(&server);

    // Never-bound ID.
    let m = Message::new(CONNECTION, op("ldp.core.connection", "destroy"))
        .arg(Value::Uint32(77))
        .arg(Value::Uint32(1));
    c.send(&m);
    c.expect_fatal(ErrorCode::InvalidObject);
    drop(c);

    let mut c = session_with_registry(&server);
    c.bind(2, "ldp.core.output", 1, 9);
    c.expect_bound();

    // Normal destroy: echoed cookie, ID free afterwards.
    c.destroy(9, 0xC00C_0001);

    // Double destroy: invalid_state.
    let m = Message::new(CONNECTION, op("ldp.core.connection", "destroy"))
        .arg(Value::Uint32(9))
        .arg(Value::Uint32(2));
    c.send(&m);
    c.expect_fatal(ErrorCode::InvalidState);
    drop(c);

    // The bootstrap object cannot be destroyed.
    let mut c = session_with_registry(&server);
    let m = Message::new(CONNECTION, op("ldp.core.connection", "destroy"))
        .arg(Value::Uint32(1))
        .arg(Value::Uint32(3));
    c.send(&m);
    c.expect_fatal(ErrorCode::InvalidState);
    drop(c);

    // Object 0 is not an object.
    let mut c = session_with_registry(&server);
    let m = Message::new(CONNECTION, op("ldp.core.connection", "destroy"))
        .arg(Value::Uint32(0))
        .arg(Value::Uint32(4));
    c.send(&m);
    c.expect_fatal(ErrorCode::InvalidObject);
}

#[test]
fn stale_id_after_destroy_is_rejected() {
    let server = TestServer::start("lifecycle-stale", common::standard_config());
    let mut c = session_with_registry(&server);
    c.bind(2, "ldp.core.output", 1, 9);
    c.expect_bound();
    c.destroy(9, 1);

    // Requests to the destroyed object: stale_object.
    let m = Message::new(9, op("ldp.core.registry", "bind"))
        .arg(Value::String("ldp.core.output".into()))
        .arg(Value::Uint32(1))
        .arg(Value::NewId(ObjectId::client(10).unwrap()));
    c.send(&m);
    c.expect_fatal(ErrorCode::StaleObject);
    drop(c);

    // Destroying the registry object itself, then targeting it: stale.
    let mut c = session_with_registry(&server);
    c.destroy(2, 5);
    c.bind(2, "ldp.core.output", 1, 9);
    c.expect_fatal(ErrorCode::StaleObject);
}

#[test]
fn id_reuse_after_destroy_rebinds() {
    let server = TestServer::start("lifecycle-reuse", common::standard_config());
    let mut c = session_with_registry(&server);

    for round in 0..3u32 {
        c.bind(2, "ldp.core.output", 1, 9);
        c.expect_bound();
        c.destroy(9, round);
        // The ID is free again; the next round rebinds it (the server's
        // generation table advanced underneath).
    }
    c.sync(0x5EED);
}

#[test]
fn get_registry_versions_and_multiplicity() {
    let server = TestServer::start("lifecycle-getreg", common::standard_config());

    // Out-of-range version: unsupported_version.
    let mut c = MockClient::connect(&server.addr);
    c.hello(1, Bitset128::EMPTY);
    c.expect_welcome();
    c.get_registry(2, 2);
    c.expect_fatal(ErrorCode::UnsupportedVersion);
    drop(c);

    // Two registries coexist; each replays the globals (registry + 3).
    let mut c = MockClient::connect(&server.addr);
    c.hello(1, Bitset128::EMPTY);
    c.expect_welcome();
    c.get_registry(1, 2);
    for _ in 0..4 {
        c.expect_global();
    }
    // The registry is itself a global: bind a second one through the first.
    c.bind(2, "ldp.core.registry", 1, 3);
    c.expect_bound();
    // Both registries serve binds.
    c.bind(3, "ldp.core.output", 1, 9);
    c.expect_bound();
    c.sync(9);
}

#[test]
fn audit_trail_records_lifecycle() {
    let audit = std::sync::Arc::new(ldp_server::AuditRecorder::new(256));
    let cfg = ServerConfig {
        globals: vec![GlobalAdvert::new("ldp.core.output")],
        audit: std::sync::Arc::clone(&audit) as std::sync::Arc<dyn ldp_server::AuditSink>,
        ..ServerConfig::default()
    };
    let server = TestServer::start("lifecycle-audit", cfg);
    let mut c = MockClient::connect(&server.addr);
    c.hello(3, Bitset128::EMPTY);
    c.expect_welcome();
    c.get_registry(1, 2);
    for _ in 0..2 {
        c.expect_global();
    }
    c.bind(2, "ldp.core.output", 1, 9);
    c.expect_bound();
    c.destroy(9, 1);
    drop(c);
    assert!(server.wait_live_zero(std::time::Duration::from_secs(2)));

    let records = audit.snapshot();
    assert!(records.iter().any(|r| matches!(
        r,
        ldp_server::AuditRecord::Connected { pid, .. } if *pid == std::process::id() as i32
    )));
    assert!(records
        .iter()
        .any(|r| matches!(r, ldp_server::AuditRecord::Handshake { release: 3, .. })));
    assert!(records.iter().any(|r| matches!(
        r,
        ldp_server::AuditRecord::Bind { object, .. } if object.as_u32() == 2
    )));
    assert!(records.iter().any(|r| matches!(
        r,
        ldp_server::AuditRecord::Bind { object, .. } if object.as_u32() == 9
    )));
    assert!(records.iter().any(|r| matches!(
        r,
        ldp_server::AuditRecord::Destroy { object, .. } if object.as_u32() == 9
    )));
    assert!(records.iter().any(|r| matches!(
        r,
        ldp_server::AuditRecord::Disconnected {
            reason: ldp_server::SessionEnd::Clean,
            ..
        }
    )));
}

#[test]
fn object_ceiling_is_enforced() {
    let cfg = ServerConfig {
        globals: vec![GlobalAdvert::new("ldp.core.output")],
        limits: ldp_core::limits::Limits {
            client_objects: 4, // bootstrap + registry + 2 binds
            ..ldp_core::limits::Limits::DEFAULT
        },
        ..ServerConfig::default()
    };
    let server = TestServer::start("lifecycle-ceiling", cfg);
    let mut c = MockClient::connect(&server.addr);
    c.hello(1, Bitset128::EMPTY);
    c.expect_welcome();
    c.get_registry(1, 2);
    for _ in 0..2 {
        c.expect_global();
    }
    c.bind(2, "ldp.core.output", 1, 9);
    c.expect_bound();
    c.bind(2, "ldp.core.output", 1, 10);
    c.expect_bound();
    // Fifth live object: over the ceiling.
    c.bind(2, "ldp.core.output", 1, 11);
    c.expect_fatal(ErrorCode::LimitExceeded);
}
