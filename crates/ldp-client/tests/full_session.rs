//! Exit criterion 1 (roadmap Phase 5): a test client completes a full
//! session against `ldp-server` — connect, handshake, registry
//! bootstrap, global replay, bind, factory object creation, sync
//! round-trips, keepalive, destroy lifecycle, clean disconnect — plus
//! the failure paths the client must survive correctly.

mod common;

use common::{factories, FactoryDispatcher, ServerHarness};
use ldp_client::{ClientError, Connection, Event, EventHandler, NoopHandler};
use ldp_core::wire::Value;
use ldp_server::ServerConfig;

/// Records `(interface, op, cookie-or-0)` in dispatch order.
struct Recorder {
    log: Vec<(&'static str, &'static str, u32)>,
    /// `(object_id, cookie)` of every `destroyed` event.
    destroyed: Vec<(u32, u32)>,
    disconnects: Vec<ldp_client::DisconnectKind>,
}

impl Recorder {
    fn new() -> Recorder {
        Recorder {
            log: Vec::new(),
            destroyed: Vec::new(),
            disconnects: Vec::new(),
        }
    }

    /// Positions of events matching `(op)`; empty when absent.
    fn positions(&self, op: &str) -> Vec<usize> {
        self.log
            .iter()
            .enumerate()
            .filter(|(_, (_, o, _))| *o == op)
            .map(|(i, _)| i)
            .collect()
    }

    /// The cookie of the one `destroyed` event for `object`.
    fn destroyed_cookie(&self, object: u32) -> Option<u32> {
        self.destroyed
            .iter()
            .find(|(o, _)| *o == object)
            .map(|(_, c)| *c)
    }
}

impl EventHandler for Recorder {
    fn on_event(&mut self, event: &Event) -> ldp_core::error::Result<()> {
        // Cookie-shaped events keep their cookie; everything else 0.
        let cookie = match event.arg("cookie") {
            Some(Value::Uint32(c)) => *c,
            _ => 0,
        };
        self.log.push((event.interface, event.op.name, cookie));
        if event.op.name == "destroyed" {
            let (Some(&Value::Uint32(object)), Some(&Value::Uint32(cookie))) =
                (event.arg("object_id"), event.arg("cookie"))
            else {
                return Err(ldp_core::error::LdpError::Logic {
                    what: "destroyed event lost its schema shape",
                });
            };
            self.destroyed.push((object, cookie));
        }
        Ok(())
    }

    fn on_disconnect(&mut self, reason: &ldp_client::DisconnectKind) {
        self.disconnects.push(reason.clone());
    }
}

#[test]
fn full_session_lifecycle() {
    let config = ServerConfig {
        protocol_release: 7,
        globals: factories(),
        ..ServerConfig::default()
    };
    let mut server = ServerHarness::start_with("full-session", config, FactoryDispatcher);

    // -- connect + handshake ------------------------------------------
    let mut conn = Connection::connect(&server.addr).unwrap();
    let welcome = conn.welcome().unwrap().clone();
    assert_eq!(welcome.protocol_release, 7);
    assert!(welcome.client_id != 0);
    assert!(welcome.app_id.is_empty());
    assert_eq!(welcome.sandbox, 1, "same-user peers are unconfined");

    // -- registry bootstrap + global replay + sync barrier -------------
    let mut recorder = Recorder::new();
    let _registry = conn.registry().unwrap();
    conn.roundtrip(&mut recorder).unwrap();
    let global_positions = recorder.positions("global");
    assert_eq!(
        global_positions.len(),
        factories().len() + 1,
        "the registry global itself plus every advertised global"
    );
    // sync_done was dispatched after every global (barrier).
    let sync_pos = recorder.positions("sync_done")[0];
    let last_global = *global_positions.last().unwrap();
    assert!(
        sync_pos > last_global,
        "sync_done at {sync_pos} must follow the last global at {last_global}"
    );

    // -- bind + bound confirmation -------------------------------------
    let output = conn.bind("ldp.core.output").unwrap();
    assert_eq!(output.interface(), "ldp.core.output");
    assert_eq!(output.version(), 1);
    conn.roundtrip(&mut recorder).unwrap();
    assert_eq!(recorder.positions("bound").len(), 1, "one bound per bind");

    // -- factory objects: surface and pointer ---------------------------
    let compositor = conn.bind("ldp.core.compositor").unwrap();
    let seat = conn.bind("ldp.input.seat").unwrap();
    let surface = conn
        .create_object(&compositor, "create_surface", vec![])
        .unwrap();
    assert_eq!(surface.interface(), "ldp.core.surface");
    let pointer = conn.create_object(&seat, "get_pointer", vec![]).unwrap();
    assert_eq!(pointer.interface(), "ldp.input.pointer");

    // -- keepalive -------------------------------------------------------
    conn.ping(&mut recorder).unwrap();
    assert_eq!(recorder.positions("pong").len(), 1);

    // -- destroy lifecycle ------------------------------------------------
    let cookie = conn.destroy(&surface).unwrap();
    conn.roundtrip(&mut recorder).unwrap();
    let destroyed = recorder.positions("destroyed");
    assert_eq!(destroyed.len(), 1, "one destroyed confirmation");
    assert_eq!(
        recorder.destroyed_cookie(surface.id().as_u32()),
        Some(cookie),
        "the destroyed event names our object"
    );
    // Using the destroyed proxy now is a *local* rejection.
    let err = conn
        .send_request(&surface, "commit", vec![Value::Uint32(1)])
        .unwrap_err();
    assert!(
        matches!(err, ClientError::UnknownProxy { .. }),
        "got {err:?}"
    );
    // The pointer proxy still works.
    conn.ping(&mut recorder).unwrap();
    assert_eq!(recorder.positions("pong").len(), 2);

    // -- clean disconnect --------------------------------------------------
    drop(conn);
    assert!(
        server.wait_live_zero(std::time::Duration::from_secs(5)),
        "server must reclaim the session after a clean disconnect"
    );
    server.stop();
}

#[test]
fn server_error_is_fatal_and_reported() {
    let config = ServerConfig {
        globals: factories(),
        ..ServerConfig::default()
    };
    let mut server = ServerHarness::start("server-error", config);
    let mut conn = Connection::connect(&server.addr).unwrap();

    // Bind an interface this server does not advertise. The send itself
    // succeeds (bind is fire-and-forget by design); the fatal rejection
    // surfaces at the next dispatch.
    conn.bind("ldp.shell.shell").unwrap();
    let mut noop = NoopHandler;
    let err = conn.roundtrip(&mut noop).unwrap_err();
    match &err {
        ClientError::ServerError { code, object, .. } => {
            assert_eq!(*code, ldp_core::error::ErrorCode::InvalidInterface);
            assert!(*object != 0);
        }
        other => panic!("expected ServerError, got {other:?}"),
    }
    assert!(!conn.is_alive());
    assert_eq!(
        conn.disconnect_reason(),
        Some(&ldp_client::DisconnectKind::LocalDrop)
    );

    // Further sends are refused locally, without touching the socket.
    let again = conn.registry().unwrap_err();
    assert!(matches!(again, ClientError::Disconnected(_)));

    assert!(server.wait_live_zero(std::time::Duration::from_secs(5)));
    server.stop();
}

#[test]
fn large_messages_negotiation_round_trips() {
    let config = ServerConfig::default();
    let mut server = ServerHarness::start("large-msg", config);
    let cfg = ldp_client::ClientConfig::default().with_large_messages();
    let mut conn = Connection::connect_with(cfg, &server.addr).unwrap();
    // The client switched its own framing to the wide limits.
    assert_eq!(conn.limits(), ldp_core::limits::Limits::LARGE_MESSAGES);
    let mut noop = NoopHandler;
    conn.roundtrip(&mut noop).unwrap();
    conn.ping(&mut noop).unwrap();
    drop(conn);
    assert!(server.wait_live_zero(std::time::Duration::from_secs(5)));
    server.stop();
}

#[test]
fn introspection_option_enables_registry_introspect() {
    let config = ServerConfig::default();
    let mut server = ServerHarness::start("introspect", config);
    let cfg = ldp_client::ClientConfig::default().with_introspection();
    let mut conn = Connection::connect_with(cfg, &server.addr).unwrap();
    let registry = conn.registry().unwrap();
    conn.send_request(
        &registry,
        "introspect",
        vec![Value::String("ldp.core.connection".into())],
    )
    .unwrap();
    // The schema reply arrives before the matching sync_done.
    let mut got_schema = false;
    let mut recorder = SchemaProbe {
        got: &mut got_schema,
    };
    conn.roundtrip(&mut recorder).unwrap();
    assert!(got_schema, "the schema event must be dispatched");
    drop(conn);
    assert!(server.wait_live_zero(std::time::Duration::from_secs(5)));
    server.stop();
}

/// Captures whether one `registry.schema` event was dispatched.
struct SchemaProbe<'a> {
    got: &'a mut bool,
}

impl EventHandler for SchemaProbe<'_> {
    fn on_event(&mut self, event: &Event) -> ldp_core::error::Result<()> {
        if event.interface == "ldp.core.registry" && event.op.name == "schema" {
            *self.got = true;
        }
        Ok(())
    }
}

#[test]
fn unknown_proxy_requests_are_rejected_locally() {
    let config = ServerConfig::default();
    let mut server = ServerHarness::start("unknown-proxy", config);
    let mut conn = Connection::connect(&server.addr).unwrap();
    // A proxy built for another connection never existed here.
    let foreign = ldp_client::Proxy::new(
        ldp_core::ids::ObjectId::client(0x777).unwrap(),
        "ldp.core.output",
        1,
    );
    let err = conn
        .send_request(&foreign, "commit", vec![Value::Uint32(1)])
        .unwrap_err();
    assert!(matches!(err, ClientError::UnknownProxy { .. }));
    // Unknown request names are also local rejections.
    let err = conn
        .send_request(&conn.connection_proxy(), "no_such_request", vec![])
        .unwrap_err();
    assert!(
        matches!(err, ClientError::NoSuchRequest { .. }),
        "got {err:?}"
    );
    // The connection itself is still healthy.
    let mut noop = NoopHandler;
    conn.roundtrip(&mut noop).unwrap();
    drop(conn);
    assert!(server.wait_live_zero(std::time::Duration::from_secs(5)));
    server.stop();
}

#[test]
fn handler_errors_abort_dispatch_and_propagate() {
    let config = ServerConfig::default();
    let mut server = ServerHarness::start("handler-err", config);
    let mut conn = Connection::connect(&server.addr).unwrap();
    // Any event (the registry global replay) makes the handler fail.
    let mut failing = FailingHandler;
    let err = conn.roundtrip(&mut failing).unwrap_err();
    assert!(
        matches!(err, ClientError::Ldp(_)),
        "handler failure must propagate, got {err:?}"
    );
    drop(conn);
    assert!(server.wait_live_zero(std::time::Duration::from_secs(5)));
    server.stop();
}

/// Fails on the first event it sees.
struct FailingHandler;

impl EventHandler for FailingHandler {
    fn on_event(&mut self, _event: &Event) -> ldp_core::error::Result<()> {
        Err(ldp_core::error::LdpError::Logic {
            what: "test: deliberate handler failure",
        })
    }
}

#[test]
fn reconnect_rebuilds_a_whole_session() {
    // The application's "session" here: one output binding + a ping.
    let config = ServerConfig {
        globals: factories(),
        ..ServerConfig::default()
    };
    let mut server = ServerHarness::start("reconnect", config);

    // First session: connect, bind, create a surface, then drop the
    // connection (crash). The server reclaims the session.
    let mut first = Connection::connect(&server.addr).unwrap();
    let _output = first.bind("ldp.core.output").unwrap();
    let compositor = first.bind("ldp.core.compositor").unwrap();
    let surface = first
        .create_object(&compositor, "create_surface", vec![])
        .unwrap();
    assert!(first.is_alive());
    drop(first);
    assert!(server.wait_live_zero(std::time::Duration::from_secs(5)));

    // The rebuild callback re-creates PART of the application state
    // (the output binding) — deliberately not the surface.
    let mut rebound = Vec::new();
    let mut rebuild = |conn: &mut Connection| -> ldp_client::Result<()> {
        let proxy = conn.bind("ldp.core.output")?;
        rebound.push(proxy.id().as_u32());
        let mut noop = NoopHandler;
        conn.roundtrip(&mut noop)?;
        Ok(())
    };
    let mut reconnector = ldp_client::Reconnector::new(
        ldp_client::ClientConfig::default(),
        server.addr.clone(),
        ldp_client::ReconnectPolicy::fast(),
    );
    let mut conn = reconnector.run(&mut rebuild).unwrap();
    assert_eq!(rebound.len(), 1);
    assert!(conn.is_alive());
    // The old SURFACE proxy is not valid on the new connection: the
    // rebuild did not re-create it, so its ID has no proxy here. Using
    // it is a local rejection, not a wire error.
    let err = conn
        .send_request(&surface, "commit", vec![Value::Uint32(1)])
        .unwrap_err();
    assert!(
        matches!(err, ClientError::UnknownProxy { .. }),
        "got {err:?}"
    );
    drop(conn);
    assert!(server.wait_live_zero(std::time::Duration::from_secs(5)));
    server.stop();
}

#[test]
fn reconnect_gives_up_after_bounded_attempts() {
    // Nothing ever listens here: every attempt fails at connect().
    let addr = ldp_transport::UnixAddr::abstract_name(
        format!("\u{0}ldp-cli-absent-{}", std::process::id()).as_bytes(),
    )
    .unwrap();
    let policy = ldp_client::ReconnectPolicy {
        max_attempts: 3,
        initial_delay: std::time::Duration::ZERO,
        max_delay: std::time::Duration::ZERO,
        factor: 1,
    };
    let started = std::time::Instant::now();
    let result = ldp_client::connect_with_retry(ldp_client::ClientConfig::default(), &addr, policy);
    assert!(result.is_err());
    assert!(
        started.elapsed() < std::time::Duration::from_secs(2),
        "zero-delay policy must not sleep"
    );
    match result.unwrap_err() {
        ClientError::Ldp(_) => {}
        other => panic!("expected a transport error, got {other:?}"),
    }
}

#[test]
fn fatal_error_state_is_sticky_across_operations() {
    let config = ServerConfig {
        globals: factories(),
        ..ServerConfig::default()
    };
    let mut server = ServerHarness::start("sticky", config);
    let mut conn = Connection::connect(&server.addr).unwrap();

    // A rejected bind kills the connection with a delivered error.
    conn.bind("ldp.shell.shell").unwrap();
    let mut noop = NoopHandler;
    let err = conn.roundtrip(&mut noop).unwrap_err();
    assert!(
        matches!(err, ClientError::ServerError { .. }),
        "got {err:?}"
    );

    // Every subsequent operation reports the SAME disconnect state —
    // the connection never half-recovers.
    for _ in 0..3 {
        let again = conn.roundtrip(&mut noop).unwrap_err();
        assert!(
            matches!(again, ClientError::Disconnected(_)),
            "got {again:?}"
        );
    }
    let send = conn.ping(&mut noop).unwrap_err();
    assert!(matches!(send, ClientError::Disconnected(_)));

    // And the server reclaimed the session after closing the socket.
    assert!(server.wait_live_zero(std::time::Duration::from_secs(5)));
    server.stop();
}
