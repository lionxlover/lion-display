//! End-to-end conformance: a real `ldp-protocol` message with FD
//! arguments travels over the transport and decodes on the far side.
//!
//! This pins the Phase 2 ↔ Phase 3 contract: the encoder derives
//! `fd_count` from the message's `fd` arguments, the transport carries
//! header + payload + the FD batch as one `sendmsg`, the framing reader
//! hands back bytes whose `fd_count` matches the ancillary array, and
//! `decode` + `check_signature` accept the result. FD indices in the
//! decoded message address the frame's FD table.

use ldp_core::error::ErrorCode;
use ldp_core::ids::ObjectId;
use ldp_core::limits::Limits;
use ldp_core::wire::Value;
use ldp_protocol::{check_signature, decode, Direction, Message, ValidationMode, REGISTRY};
use ldp_transport::{
    sys, FdList, FramedReader, FramedWriter, NoHooks, TransportListener, TransportStream, UnixAddr,
};
use std::os::fd::AsRawFd;

fn addr(tag: &str) -> UnixAddr {
    let name = format!("\u{0}ldp-e2e-{tag}-{}", std::process::id());
    UnixAddr::abstract_name(name.as_bytes()).unwrap()
}

/// `ldp.core.connection` has no fd-carrying request in v1, so this test
/// uses a hand-built message on the bootstrap object: two `fd` args
/// (indices 0 and 1) plus scalars. Stage 1–2 pass; stage 3 signature
/// checking then rejects the *hand-built* opcode — which is exactly the
/// layer split under test (transport says nothing about semantics).
#[test]
fn message_with_fd_arguments_crosses_the_wire() {
    let a = addr("with-fds");
    let listener = TransportListener::bind(&a, 4).unwrap();
    let mut client = TransportStream::connect(&a).unwrap();
    let (mut server, creds) = listener.accept().unwrap();
    assert_eq!(creds.pid, std::process::id() as i32);

    // A message whose fd_count the encoder derives automatically.
    let msg = Message::new(1, 1)
        .arg(Value::String("ldp.data.shm".into()))
        .arg(Value::Fd(0))
        .arg(Value::Fd(1))
        .arg(Value::Uint32(4096));
    let bytes = msg.encode(&Limits::DEFAULT).unwrap();
    assert_eq!(msg.required_fd_count(), 2);

    let mut fds = FdList::new();
    fds.push(sys::eventfd_owned().unwrap());
    fds.push(sys::eventfd_owned().unwrap());

    let mut writer = FramedWriter::new(Limits::DEFAULT, NoHooks);
    let mut reader = FramedReader::new(Limits::DEFAULT);
    writer.send_msg(&mut server, &bytes, &mut fds).unwrap();
    let frame = reader.recv_msg(&mut client).unwrap();

    // Stage 1 (transport): framing consistent, FD count exact.
    assert_eq!(frame.message_bytes(), &bytes[..]);
    assert_eq!(frame.fd_count(), 2);
    assert_eq!(frame.fds.len(), 2);
    // The received FDs are live and usable.
    for fd in frame.fds.iter() {
        assert!(ldp_transport::fd::validate(fd.as_raw_fd()));
    }

    // Stage 2 (codec): structural decode succeeds with the frame's count.
    let back = decode(
        frame.message_bytes(),
        frame.fd_count() as u32,
        &Limits::DEFAULT,
        ValidationMode::Strict,
    )
    .unwrap();
    assert_eq!(back, msg);
    assert_eq!(back.required_fd_count(), 2);
    // The fd arguments are indices into the frame's ancillary array.
    assert!(matches!(back.args[1], Value::Fd(0)));
    assert!(matches!(back.args[2], Value::Fd(1)));

    // Stage 3 (schema): the hand-built opcode 1 on `ldp.core.connection`
    // is `hello` — whose signature differs — so signature checking
    // rejects it. The transport correctly carried it anyway.
    let verdict = check_signature(
        &REGISTRY,
        &back,
        "ldp.core.connection",
        Direction::Request,
        1,
        ValidationMode::Strict,
    );
    assert!(
        verdict.is_err(),
        "hello's signature must not match this payload"
    );
    let code = verdict.unwrap_err();
    assert_eq!(
        code.wire_code(),
        Some(ErrorCode::SignatureMismatch),
        "stage 3 reports signature_mismatch: {code}"
    );
}

/// The same message round-trips with zero FDs when the arguments carry
/// none — the fd_count/header/ancillary invariants hold on the empty
/// path too.
#[test]
fn fd_free_message_crosses_the_wire() {
    let a = addr("fd-free");
    let listener = TransportListener::bind(&a, 4).unwrap();
    let mut client = TransportStream::connect(&a).unwrap();
    let (mut server, _) = listener.accept().unwrap();

    let msg = Message::new(1, 1)
        .arg(Value::Uint32(7))
        .arg(Value::Int32(-3))
        .arg(Value::Bool(true));
    let bytes = msg.encode(&Limits::DEFAULT).unwrap();
    assert_eq!(msg.required_fd_count(), 0);

    let mut writer = FramedWriter::new(Limits::DEFAULT, NoHooks);
    let mut reader = FramedReader::new(Limits::DEFAULT);
    writer
        .send_msg(&mut server, &bytes, &mut FdList::new())
        .unwrap();
    let frame = reader.recv_msg(&mut client).unwrap();
    assert_eq!(frame.fd_count(), 0);
    assert!(frame.fds.is_empty());
    assert_eq!(frame.message_bytes(), &bytes[..]);

    let back = decode(
        frame.message_bytes(),
        0,
        &Limits::DEFAULT,
        ValidationMode::Strict,
    )
    .unwrap();
    assert_eq!(back, msg);
}

/// A genuinely well-formed v1 request — `registry.bind` — crosses the
/// wire and passes all three validation stages.
#[test]
fn well_formed_bind_request_passes_all_stages() {
    let a = addr("bind-req");
    let listener = TransportListener::bind(&a, 4).unwrap();
    let mut client = TransportStream::connect(&a).unwrap();
    let (mut server, _) = listener.accept().unwrap();

    let msg = Message::new(2, 1)
        .arg(Value::String("ldp.core.registry".into()))
        .arg(Value::Uint32(1))
        .arg(Value::NewId(ObjectId::client(9).unwrap()));
    let bytes = msg.encode(&Limits::DEFAULT).unwrap();

    let mut writer = FramedWriter::new(Limits::DEFAULT, NoHooks);
    let mut reader = FramedReader::new(Limits::DEFAULT);
    writer
        .send_msg(&mut server, &bytes, &mut FdList::new())
        .unwrap();
    let frame = reader.recv_msg(&mut client).unwrap();

    let back = decode(
        frame.message_bytes(),
        frame.fd_count() as u32,
        &Limits::DEFAULT,
        ValidationMode::Strict,
    )
    .unwrap();
    assert_eq!(back, msg);
    let (_, op) = check_signature(
        &REGISTRY,
        &back,
        "ldp.core.registry",
        Direction::Request,
        1,
        ValidationMode::Strict,
    )
    .unwrap();
    assert_eq!(op.name, "bind");
}
