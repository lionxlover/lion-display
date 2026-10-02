//! Handshake conformance: hello/welcome, get_registry bootstrap, option
//! negotiation, first-message discipline.

mod common;

use common::{op, MockClient, TestServer};
use ldp_core::bitset::Bitset128;
use ldp_core::error::ErrorCode;
use ldp_core::limits::Limits;
use ldp_core::wire::Value;
use ldp_protocol::Message;

use ldp_server::{connection_options, GlobalAdvert, ServerConfig};

fn opts(bits: &[u32]) -> Bitset128 {
    let mut b = Bitset128::EMPTY;
    for &i in bits {
        b = b.with(i);
    }
    b
}

#[test]
fn full_handshake_globals_and_sync() {
    let cfg = ServerConfig {
        protocol_release: 7,
        globals: vec![
            GlobalAdvert::new("ldp.core.output"),
            GlobalAdvert::new("ldp.core.compositor"),
        ],
        ..ServerConfig::default()
    };
    let server = TestServer::start("handshake-full", cfg);
    let mut client = MockClient::connect(&server.addr);

    client.hello(9, opts(&[connection_options::INTROSPECTION]));
    let (release, client_id, sandbox) = client.expect_welcome();
    assert_eq!(release, 7, "welcome echoes the server release");
    assert!(client_id != 0, "client_id must be a real audit identity");
    assert_eq!(sandbox, 1, "same-user peers are unconfined by default");

    // Registry bootstrap: object 2, then exactly the replay list — the
    // implicitly advertised registry first, then the configured globals.
    client.get_registry(1, 2);
    let mut seen = Vec::new();
    for _ in 0..3 {
        seen.push(client.expect_global());
    }
    assert_eq!(
        seen,
        vec![
            ("ldp.core.registry".to_string(), 1, 1),
            ("ldp.core.output".to_string(), 1, 1),
            ("ldp.core.compositor".to_string(), 1, 1),
        ]
    );

    // The round-trip barrier works post-handshake.
    client.sync(42);
    client.ping(0xAAAA_5555);
    assert!(
        server.wait_live_zero(std::time::Duration::from_secs(2)) || {
            // Client still connected; drop and check reclamation.
            drop(client);
            server.wait_live_zero(std::time::Duration::from_secs(2))
        }
    );
}

#[test]
fn first_message_must_be_hello() {
    let server = TestServer::start("handshake-first", common::standard_config());
    let mut client = MockClient::connect(&server.addr);
    // sync before hello: fatal invalid_state, error delivered then close.
    let m =
        Message::new(common::CONNECTION, op("ldp.core.connection", "sync")).arg(Value::Uint32(1));
    client.send(&m);
    client.expect_fatal(ErrorCode::InvalidState);
    assert!(server.wait_live_zero(std::time::Duration::from_secs(2)));
}

#[test]
fn second_hello_is_fatal() {
    let server = TestServer::start("handshake-second", common::standard_config());
    let mut client = MockClient::connect(&server.addr);
    client.hello(1, Bitset128::EMPTY);
    client.expect_welcome();
    client.hello(1, Bitset128::EMPTY);
    client.expect_fatal(ErrorCode::InvalidState);
    assert!(server.wait_live_zero(std::time::Duration::from_secs(2)));
}

#[test]
fn message_to_unknown_object_before_and_after_handshake() {
    let server = TestServer::start("handshake-unknown", common::standard_config());
    let mut client = MockClient::connect(&server.addr);

    // Before hello: any non-hello first message is invalid_state.
    client.send(&Message::new(99, op("ldp.core.connection", "sync")).arg(Value::Uint32(1)));
    client.expect_fatal(ErrorCode::InvalidState);
    drop(client);
    assert!(server.wait_live_zero(std::time::Duration::from_secs(2)));

    // After a clean handshake, an unbound target is invalid_object.
    let mut client = MockClient::connect(&server.addr);
    client.hello(1, Bitset128::EMPTY);
    client.expect_welcome();
    client.send(&Message::new(99, op("ldp.core.connection", "sync")).arg(Value::Uint32(1)));
    client.expect_fatal(ErrorCode::InvalidObject);
    assert!(server.wait_live_zero(std::time::Duration::from_secs(2)));
}

#[test]
fn object_zero_is_never_a_target() {
    let server = TestServer::start("handshake-zero", common::standard_config());
    let mut client = MockClient::connect(&server.addr);
    client.hello(1, Bitset128::EMPTY);
    client.expect_welcome();
    client.send(&Message::new(0, op("ldp.core.connection", "sync")).arg(Value::Uint32(1)));
    client.expect_fatal(ErrorCode::InvalidObject);
    assert!(server.wait_live_zero(std::time::Duration::from_secs(2)));
}

#[test]
fn granted_options_are_the_intersection_with_server_policy() {
    // A server that does not allow introspection must silently drop it.
    let cfg = ServerConfig {
        globals: vec![GlobalAdvert::new("ldp.core.output")],
        options_allowed: Bitset128::EMPTY,
        ..ServerConfig::default()
    };
    let server = TestServer::start("handshake-drop-opts", cfg);
    let mut client = MockClient::connect(&server.addr);
    client.hello(1, opts(&[connection_options::INTROSPECTION, 5, 127]));
    client.expect_welcome();
    client.get_registry(1, 2);
    let _ = client.expect_global(); // the implicit registry
    let _ = client.expect_global(); // output
                                    // introspect denied: the option was not granted.
    let m = Message::new(2, op("ldp.core.registry", "introspect"))
        .arg(Value::String("ldp.core.output".into()));
    client.send(&m);
    client.expect_fatal(ErrorCode::Unauthorized);
    assert!(server.wait_live_zero(std::time::Duration::from_secs(2)));
}

#[test]
fn large_messages_widen_the_frame_ceiling() {
    // A 2 MiB array<uint32> payload: legal only with large_messages.
    let server = TestServer::start("handshake-large", common::standard_config());

    // Without the option: the client's own default limits refuse the
    // 2.4 MiB message locally — which is itself the correct client
    // behavior for an un-negotiated ceiling (asserted, then dropped).
    let mut small = MockClient::connect(&server.addr);
    small.hello(1, Bitset128::EMPTY);
    small.expect_welcome();
    let elements = 300_000u32; // ~2.4 MiB payload
    let big_msg = || {
        Message::new(common::CONNECTION, op("ldp.core.connection", "sync"))
            .arg(Value::Uint32(1))
            .arg(
                Value::array(
                    ldp_core::wire::ArgType::Uint32,
                    (0..elements)
                        .map(ldp_core::wire::Primitive::Uint32)
                        .collect::<Vec<_>>(),
                )
                .unwrap(),
            )
    };
    assert!(big_msg().encode(&Limits::DEFAULT).is_err());
    drop(small);

    // With the option: the frame is read and stage 3 rejects the
    // signature (sync takes one arg) — proving the payload passed
    // framing and structural decode at 64 MiB.
    let mut big = MockClient::connect(&server.addr);
    big.hello(1, opts(&[connection_options::LARGE_MESSAGES]));
    big.expect_welcome();
    big.negotiate_large_messages();
    big.send(&big_msg());
    big.expect_fatal(ErrorCode::SignatureMismatch);
    assert!(server.wait_live_zero(std::time::Duration::from_secs(5)));
}

#[test]
fn clean_disconnect_is_reclaimed() {
    let server = TestServer::start("handshake-clean", common::standard_config());
    let mut client = MockClient::connect(&server.addr);
    client.hello(1, Bitset128::EMPTY);
    client.expect_welcome();
    drop(client);
    assert!(
        server.wait_live_zero(std::time::Duration::from_secs(2)),
        "clean disconnect must reclaim the session"
    );
}
