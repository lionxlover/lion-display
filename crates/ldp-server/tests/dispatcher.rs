//! The dispatcher seam: requests on non-core objects, FD ownership,
//! object creation and revocation through `DispatchCtx`.

mod common;

use std::sync::{Arc, Mutex};

use common::{op, MockClient, Shared, TestServer, CONNECTION};
use ldp_core::bitset::Bitset128;
use ldp_core::error::Result;
use ldp_core::ids::ObjectId;
use ldp_core::wire::Value;
use ldp_protocol::Message;

use ldp_server::{
    connection_options, DispatchCtx, Dispatcher, IncomingRequest, ObjectEntry, SessionEnd,
};

#[derive(Default)]
struct Recording {
    /// (interface, op name, arg summary) per request.
    requests: Vec<(String, String, Vec<Value>)>,
    /// FDs taken by the dispatcher (owned — closed on session end).
    fds: Vec<std::os::fd::OwnedFd>,
    /// Session-end summaries: (client, reason, live-object count).
    ended: Vec<(u32, SessionEnd, usize)>,
    /// Objects created via ctx.create_object.
    created: Vec<u32>,
}

impl Dispatcher for Recording {
    fn on_request(
        &mut self,
        ctx: &mut DispatchCtx<'_>,
        request: &IncomingRequest<'_>,
    ) -> Result<()> {
        self.requests.push((
            request.interface.to_string(),
            request.op.name.to_string(),
            request.args.to_vec(),
        ));

        if request.op.name == "create_pool" {
            // shm.create_pool(fd, size, id): take the fd, create the
            // pool object, and emit one `format` event on the shm object.
            let fd = ctx.take_fd(0)?;
            self.fds.push(fd);
            let Value::NewId(pool_id) = request.args[2] else {
                unreachable!("stage 3 guarantees the new_id shape");
            };
            ctx.create_object(pool_id, "ldp.core.shm_pool", request.version)?;
            self.created.push(pool_id.as_u32());
            ctx.emit(
                request.object,
                "format",
                vec![Value::Uint32(0x3432_5258)], // 'XR24'
            )?;
        }
        if request.op.name == "introspect" {
            // (never: registry owns introspect; this arm is unreachable)
        }
        Ok(())
    }

    fn on_session_end(
        &mut self,
        client: ldp_core::ids::ClientId,
        reason: &SessionEnd,
        objects: &[(ObjectId, ObjectEntry)],
    ) {
        // Reclamation: closing the taken FDs is the dispatcher's job.
        self.fds.clear();
        self.ended.push((client.as_u32(), *reason, objects.len()));
    }
}

fn shared_server(tag: &str) -> (TestServer, Arc<Mutex<Recording>>) {
    let state = Arc::new(Mutex::new(Recording::default()));
    let shared = Arc::clone(&state);
    let factory = move || Shared(Arc::clone(&shared));
    let server = TestServer::start_with(tag, common::standard_config(), factory);
    (server, state)
}

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
fn dispatcher_receives_requests_and_args() {
    let (server, state) = shared_server("disp-recv");
    let mut c = ready(&server);
    c.bind(2, "ldp.core.compositor", 1, 9);
    c.expect_bound();

    // One compositor request with a full-typed argument set.
    let comp = ldp_protocol::REGISTRY
        .interface("ldp.core.compositor")
        .unwrap();
    let create = comp
        .requests
        .iter()
        .find(|r| r.name == "create_surface")
        .expect("compositor declares create_surface");
    let mut m = Message::new(9, create.opcode);
    for a in create.args {
        m = match a.ty {
            ldp_core::wire::ArgType::NewId => m.arg(Value::NewId(ObjectId::client(10).unwrap())),
            _ => m.arg(Value::Uint32(1)),
        };
    }
    c.send(&m);
    c.sync(1);

    let state = state.lock().unwrap();
    assert_eq!(state.requests.len(), 1);
    let (interface, name, _) = &state.requests[0];
    assert_eq!(interface, "ldp.core.compositor");
    assert_eq!(name, "create_surface");
}

#[test]
fn fd_ownership_transfers_through_ctx() {
    let (server, state) = shared_server("disp-fd");
    let mut c = ready(&server);
    c.bind(2, "ldp.core.shm", 1, 9);
    c.expect_bound();

    // shm.create_pool(fd, size, id) with a real FD riding the message.
    let m = Message::new(9, op("ldp.core.shm", "create_pool"))
        .arg(Value::Fd(0))
        .arg(Value::Int64(4096))
        .arg(Value::NewId(ObjectId::client(10).unwrap()));
    let mut fds = ldp_transport::FdList::new();
    let file = std::fs::File::open("/dev/null").unwrap();
    fds.push(file.into());
    c.send_with_fds(&m, &mut fds);

    // The dispatcher answers with a format event on the shm object.
    let back = c.recv();
    assert_eq!(back.object_id, 9);
    assert_eq!(back.opcode, common::ev("ldp.core.shm", "format"));
    let Value::Uint32(fmt) = back.args[0] else {
        panic!("format")
    };
    assert_eq!(fmt, 0x3432_5258);

    // The pool object exists: destroying it succeeds.
    c.destroy(10, 1);
    c.sync(2);

    let state = state.lock().unwrap();
    assert_eq!(state.created, vec![10]);
    assert_eq!(state.fds.len(), 1, "the dispatcher holds the pool fd");
}

#[test]
fn session_end_reports_objects_and_closes_fds() {
    let (server, state) = shared_server("disp-end");
    let mut c = ready(&server);
    c.bind(2, "ldp.core.shm", 1, 9);
    c.expect_bound();
    let m = Message::new(9, op("ldp.core.shm", "create_pool"))
        .arg(Value::Fd(0))
        .arg(Value::Int64(4096))
        .arg(Value::NewId(ObjectId::client(10).unwrap()));
    let mut fds = ldp_transport::FdList::new();
    fds.push(std::fs::File::open("/dev/null").unwrap().into());
    c.send_with_fds(&m, &mut fds);
    let _ = c.recv(); // format

    drop(c); // clean disconnect
    assert!(server.wait_live_zero(std::time::Duration::from_secs(2)));
    let state = state.lock().unwrap();
    assert_eq!(state.ended.len(), 1, "exactly one session-end callback");
    let (client, reason, live) = &state.ended[0];
    assert!(*client != 0);
    assert_eq!(*reason, SessionEnd::Clean);
    assert_eq!(
        *live, 4,
        "connection + registry + shm + pool still live at end"
    );
    assert!(state.fds.is_empty(), "reclamation closed the pool fd");
}

#[test]
fn dispatcher_can_revoke_objects() {
    struct Revoker;
    impl Dispatcher for Revoker {
        fn on_request(
            &mut self,
            ctx: &mut DispatchCtx<'_>,
            request: &IncomingRequest<'_>,
        ) -> Result<()> {
            // Revoke the target itself on any request.
            ctx.revoke(request.object, 1) // revoked_reason::interface_removed
        }
    }
    let server = TestServer::start_with("disp-revoke", common::standard_config(), || Revoker);
    let mut c = ready(&server);
    c.bind(2, "ldp.core.compositor", 1, 9);
    c.expect_bound();
    // Any request on the compositor: the server revokes it.
    let comp = ldp_protocol::REGISTRY
        .interface("ldp.core.compositor")
        .unwrap();
    let any = comp.requests.first().unwrap();
    let mut m = Message::new(9, any.opcode);
    for a in any.args {
        m = match a.ty {
            ldp_core::wire::ArgType::NewId => m.arg(Value::NewId(ObjectId::client(10).unwrap())),
            _ => m.arg(Value::Uint32(1)),
        };
    }
    c.send(&m);
    let back = c.recv();
    assert_eq!(back.object_id, CONNECTION);
    assert_eq!(back.opcode, common::ev("ldp.core.connection", "revoked"));
    let Value::Uint32(object) = back.args[0] else {
        panic!("revoked object")
    };
    assert_eq!(object, 9);
    let Value::Enum(reason) = back.args[1] else {
        panic!("revoked reason")
    };
    assert_eq!(reason, 1);
    // The revoked ID is gone: referencing it is stale.
    c.sync(1);
    let m = Message::new(9, any.opcode).arg(Value::Uint32(1));
    c.send(&m);
    c.expect_fatal(ErrorCode::StaleObject);
}

#[test]
fn dispatcher_errors_become_connection_errors() {
    struct Broken;
    impl Dispatcher for Broken {
        fn on_request(
            &mut self,
            _ctx: &mut DispatchCtx<'_>,
            _r: &IncomingRequest<'_>,
        ) -> Result<()> {
            Err(ldp_core::error::LdpError::protocol(
                ErrorCode::InvalidBuffer,
                None,
                "no such buffer",
            ))
        }
    }
    let server = TestServer::start_with("disp-err", common::standard_config(), || Broken);
    let mut c = ready(&server);
    c.bind(2, "ldp.core.compositor", 1, 9);
    c.expect_bound();
    let comp = ldp_protocol::REGISTRY
        .interface("ldp.core.compositor")
        .unwrap();
    let any = comp.requests.first().unwrap();
    let mut m = Message::new(9, any.opcode);
    for a in any.args {
        m = match a.ty {
            ldp_core::wire::ArgType::NewId => m.arg(Value::NewId(ObjectId::client(10).unwrap())),
            _ => m.arg(Value::Uint32(1)),
        };
    }
    c.send(&m);
    c.expect_fatal(ErrorCode::InvalidBuffer);
}

use ldp_core::error::ErrorCode;

#[derive(Default)]
struct HookLog {
    /// (interface, version) per on_bind.
    binds: Vec<(String, u32)>,
    /// (interface) per on_destroy.
    destroys: Vec<String>,
    /// Wake-point count (one per handled inbound message).
    wakes: usize,
}

impl Dispatcher for HookLog {
    fn on_request(&mut self, _ctx: &mut DispatchCtx<'_>, _r: &IncomingRequest<'_>) -> Result<()> {
        Ok(())
    }
    fn on_bind(
        &mut self,
        ctx: &mut DispatchCtx<'_>,
        object: ObjectId,
        interface: &str,
        version: u32,
    ) -> Result<()> {
        self.binds.push((interface.to_string(), version));
        // The bound object is live and emittable at hook time.
        let event = match interface {
            "ldp.core.shm" => "format",
            _ => "capabilities",
        };
        ctx.emit(object, event, vec![Value::Uint32(1)])
    }
    fn on_destroy(
        &mut self,
        _ctx: &mut DispatchCtx<'_>,
        _object: ObjectId,
        interface: &str,
    ) -> Result<()> {
        self.destroys.push(interface.to_string());
        Ok(())
    }
    fn on_wake(&mut self, _ctx: &mut DispatchCtx<'_>) -> Result<()> {
        self.wakes += 1;
        Ok(())
    }
}

#[test]
fn bind_destroy_wake_hooks_fire_in_order() {
    let state = Arc::new(Mutex::new(HookLog::default()));
    let shared = Arc::clone(&state);
    let server = TestServer::start_with("disp-hooks", common::standard_config(), move || {
        Shared(Arc::clone(&shared))
    });
    let mut c = ready(&server);

    // Bind shm: on_bind fires after `bound`, and the hook's event on the
    // live object streams right after the bound reply.
    c.bind(2, "ldp.core.shm", 1, 9);
    c.expect_bound();
    let back = c.recv();
    assert_eq!(back.object_id, 9);
    assert_eq!(back.opcode, common::ev("ldp.core.shm", "format"));
    let Value::Uint32(v) = back.args[0] else {
        panic!("format arg")
    };
    assert_eq!(v, 1);

    // A sync round-trip is itself a wake point (plus the messages above).
    let wakes_after_sync = {
        c.sync(7);
        state.lock().unwrap().wakes
    };

    // Destroy the shm object: on_destroy fires with its interface.
    c.destroy(9, 1);
    c.sync(8);

    // The wake of the final sync runs *after* its sync_done is on the
    // wire, so the counter converges asynchronously: spin to quiescence
    // instead of racing the session thread.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        {
            let log = state.lock().unwrap();
            if log.wakes >= 6 || std::time::Instant::now() > deadline {
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }

    let log = state.lock().unwrap();
    assert_eq!(log.binds, vec![("ldp.core.shm".to_string(), 1)]);
    assert_eq!(log.destroys, vec![("ldp.core.shm".to_string())]);
    // hello, get_registry, bind, sync(7), destroy, sync(8) — every
    // handled message woke the dispatcher once.
    assert_eq!(log.wakes, 6);
    assert!(log.wakes >= wakes_after_sync);
}
