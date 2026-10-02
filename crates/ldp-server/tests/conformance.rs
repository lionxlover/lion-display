//! Protocol conformance: type confusion, stale-ID attacks, malformed
//! input, and introspection.

mod common;

use common::{op, MockClient, TestServer, CONNECTION};
use ldp_core::bitset::Bitset128;
use ldp_core::error::ErrorCode;
use ldp_core::ids::ObjectId;
use ldp_core::wire::Value;
use ldp_protocol::Message;

use ldp_server::{connection_options, GlobalAdvert, ServerConfig};

fn ready(server: &TestServer) -> MockClient {
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
fn type_confusion_targets_the_objects_own_interface() {
    // Bind an output (events-only interface) at ID 9, then send it a
    // connection-shaped request: the signature check runs against the
    // TARGET's interface — output has no requests at all.
    let server = TestServer::start("conf-type", common::standard_config());
    let mut c = ready(&server);
    c.bind(2, "ldp.core.output", 1, 9);
    c.expect_bound();

    let m = Message::new(9, op("ldp.core.connection", "sync")).arg(Value::Uint32(1));
    c.send(&m);
    c.expect_fatal(ErrorCode::InvalidOpcode);
    drop(c);

    // And a registry-shaped request (opcode 1 = bind, 3 args) to the
    // same output object: also invalid_opcode for the output interface.
    let mut c = ready(&server);
    c.bind(2, "ldp.core.output", 1, 9);
    c.expect_bound();
    let m = Message::new(9, op("ldp.core.registry", "bind"))
        .arg(Value::String("ldp.core.output".into()))
        .arg(Value::Uint32(1))
        .arg(Value::NewId(ObjectId::client(10).unwrap()));
    c.send(&m);
    c.expect_fatal(ErrorCode::InvalidOpcode);
}

#[test]
fn signature_mismatch_on_hello_is_rejected_pre_handshake() {
    let server = TestServer::start("conf-sig", common::standard_config());
    let mut c = MockClient::connect(&server.addr);
    // hello with three arguments: stage 3 rejects before any state
    // changes — the handshake never completes.
    let m = Message::new(CONNECTION, op("ldp.core.connection", "hello"))
        .arg(Value::Uint32(1))
        .arg(Value::Bitset(Bitset128::EMPTY))
        .arg(Value::Uint32(2));
    c.send(&m);
    c.expect_fatal(ErrorCode::SignatureMismatch);
}

#[test]
fn malformed_payload_is_a_fatal_malformed_message() {
    let server = TestServer::start("conf-malformed", common::standard_config());
    let mut c = MockClient::connect(&server.addr);
    c.hello(1, Bitset128::EMPTY);
    c.expect_welcome();

    // A structurally valid frame whose payload carries an unknown
    // argument tag (0x7F): stage 2 rejects with malformed_message.
    let mut bytes = vec![0u8; 24];
    bytes[0..4].copy_from_slice(&1u32.to_le_bytes()); // payload_words = 1
    bytes[4..8].copy_from_slice(&1u32.to_le_bytes()); // object 1
    bytes[8..12].copy_from_slice(&2u32.to_le_bytes()); // opcode sync
    bytes[16] = 0x7F; // unknown tag
    c.send_raw(&bytes);
    c.expect_fatal(ErrorCode::MalformedMessage);
}

#[test]
fn introspection_single_interface() {
    let server = TestServer::start("conf-introspect", common::standard_config());
    let mut c = ready(&server);
    let m = Message::new(2, op("ldp.core.registry", "introspect"))
        .arg(Value::String("ldp.core.connection".into()));
    c.send(&m);
    let back = c.recv();
    assert_eq!(back.object_id, 2);
    assert_eq!(back.opcode, common::ev("ldp.core.registry", "schema"));
    let Value::String(name) = &back.args[0] else {
        panic!("schema interface")
    };
    assert_eq!(&**name, "ldp.core.connection");
    let Value::String(json) = &back.args[1] else {
        panic!("schema json")
    };
    assert!(json.contains("\"name\":\"get_registry\""));
    assert!(json.contains("\"opcode\":5"));
    assert!(json.contains("\"reply\":\"welcome\""));
    // Compact JSON, within the string limit.
    assert!(json.len() <= 4096);
}

#[test]
fn introspection_whole_protocol_streams_per_interface() {
    let server = TestServer::start("conf-introspect-all", common::standard_config());
    let mut c = ready(&server);
    let m = Message::new(2, op("ldp.core.registry", "introspect"))
        .arg(Value::String(String::new().into_boxed_str()));
    c.send(&m);
    let mut names = Vec::new();
    for _ in 0..34 {
        // 33 v1 interfaces + capture_manager (Phase 22)
        let back = c.recv();
        assert_eq!(back.opcode, common::ev("ldp.core.registry", "schema"));
        let Value::String(name) = &back.args[0] else {
            panic!("schema interface")
        };
        let Value::String(json) = &back.args[1] else {
            panic!("schema json")
        };
        assert!(json.len() <= 4096, "{name} renders {} bytes", json.len());
        names.push(name.to_string());
    }
    // Every interface of the compiled protocol appears exactly once.
    assert_eq!(names.len(), 34);
    assert!(names.contains(&"ldp.core.connection".to_string()));
    assert!(names.contains(&"ldp.session.display_config".to_string()));
    assert!(names.contains(&"ldp.capture.capture_manager".to_string()));
    // Completion observed through the sync barrier.
    c.sync(1);
}

#[test]
fn introspection_unknown_interface() {
    let server = TestServer::start("conf-introspect-bad", common::standard_config());
    let mut c = ready(&server);
    let m = Message::new(2, op("ldp.core.registry", "introspect"))
        .arg(Value::String("ldp.nope.thing".into()));
    c.send(&m);
    c.expect_fatal(ErrorCode::InvalidInterface);
}

#[test]
fn error_event_identifies_the_offending_object() {
    let server = TestServer::start("conf-error-object", common::standard_config());
    let mut c = ready(&server);
    // Destroy a never-bound ID: the error event carries that ID.
    let m = Message::new(CONNECTION, op("ldp.core.connection", "destroy"))
        .arg(Value::Uint32(1234))
        .arg(Value::Uint32(1));
    c.send(&m);
    let back = c.recv();
    assert_eq!(back.opcode, common::ev("ldp.core.connection", "error"));
    let Value::Enum(code) = back.args[0] else {
        panic!("code")
    };
    assert_eq!(code, ErrorCode::InvalidObject.to_wire());
    let Value::Uint32(object) = back.args[1] else {
        panic!("object")
    };
    assert_eq!(object, 1234);
    let Value::String(message) = &back.args[2] else {
        panic!("message")
    };
    assert!(!message.is_empty(), "the diagnostic must be present");
    // Then the socket closes.
    let err = c.recv_err();
    assert!(ldp_transport::is_disconnect(&err));
}

#[test]
fn requests_on_bound_globals_reach_the_dispatcher_silently() {
    // With the NullDispatcher, requests on bound non-core objects are
    // consumed without reply: the connection stays healthy.
    let server = TestServer::start("conf-null", common::standard_config());
    let mut c = ready(&server);
    c.bind(2, "ldp.core.compositor", 1, 9);
    c.expect_bound();
    // compositor.create_surface? Use a request the compositor declares;
    // NullDispatcher consumes it, nothing is emitted — sync still works.
    let comp = ldp_protocol::REGISTRY
        .interface("ldp.core.compositor")
        .unwrap();
    let first = comp.requests.first().unwrap();
    let mut m = Message::new(9, first.opcode);
    for a in first.args {
        m = m.arg(match a.ty {
            ldp_core::wire::ArgType::Object => Value::Object(None),
            ldp_core::wire::ArgType::NewId => Value::NewId(ObjectId::client(10).unwrap()),
            ldp_core::wire::ArgType::Uint32 => Value::Uint32(1),
            ldp_core::wire::ArgType::Int32 => Value::Int32(1),
            ldp_core::wire::ArgType::Int64 => Value::Int64(1),
            ldp_core::wire::ArgType::Uint64 => Value::Uint64(1),
            ldp_core::wire::ArgType::Float32 => Value::Float32(1.0),
            ldp_core::wire::ArgType::Float64 => Value::Float64(1.0),
            ldp_core::wire::ArgType::Bool => Value::Bool(false),
            ldp_core::wire::ArgType::String => Value::String(String::new().into_boxed_str()),
            ldp_core::wire::ArgType::Enum => Value::Enum(1),
            ldp_core::wire::ArgType::Bitset => Value::Bitset(Bitset128::EMPTY),
            ldp_core::wire::ArgType::Fd => Value::Fd(0),
            ldp_core::wire::ArgType::Array => {
                Value::array(ldp_core::wire::ArgType::Int32, Vec::new()).unwrap()
            }
            ldp_core::wire::ArgType::Rect => Value::Rect(ldp_core::geometry::Rect::new(0, 0, 1, 1)),
            _ => unreachable!("schema arg types are exhaustively covered"),
        });
    }
    c.send(&m);
    c.sync(7);
    c.ping(8);
}

#[test]
fn sandbox_verdict_comes_from_policy() {
    // A custom policy that always says "portal".
    fn always_portal(_: &ldp_transport::PeerCreds) -> ldp_server::SandboxFlavor {
        ldp_server::SandboxFlavor::Portal
    }
    let cfg = ServerConfig {
        globals: vec![GlobalAdvert::new("ldp.core.output")],
        sandbox_policy: always_portal,
        ..ServerConfig::default()
    };
    let server = TestServer::start("conf-sandbox", cfg);
    let mut c = MockClient::connect(&server.addr);
    c.hello(1, Bitset128::EMPTY);
    let (_, _, sandbox) = c.expect_welcome();
    assert_eq!(sandbox, ldp_server::SandboxFlavor::Portal.to_wire());
    c.sync(1);
}
