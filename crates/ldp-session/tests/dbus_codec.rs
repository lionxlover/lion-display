//! The D-Bus codec conformance suite: golden literal bytes (hand-built
//! from the specification's marshalling rules), marshal→parse
//! round-trips across every supported type, and a malformed-input
//! corpus that must fail with typed errors, never panics.

use ldp_session::dbus::{parse_message, DbusError, DbusMessage, DbusValue, MessageType};

fn msg(kind: MessageType, serial: u32) -> DbusMessage {
    DbusMessage {
        kind,
        flags: 0,
        serial,
        path: None,
        interface: None,
        member: None,
        error_name: None,
        reply_serial: None,
        destination: None,
        sender: None,
        signature: String::new(),
        body: Vec::new(),
        unix_fds: 0,
    }
}

fn s(v: &str) -> String {
    v.to_owned()
}

// ---------------------------------------------------------------------------
// Golden bytes: the bus Hello call, hand-marshaled from the spec.
// ---------------------------------------------------------------------------

#[test]
fn golden_hello_method_call() {
    // Expected bytes assembled field-by-field from the specification's
    // marshalling rules (ascending field-code order; computed string
    // lengths; alignment hand-rolled here so the codec's shortcuts
    // cannot echo back):
    //   'l' 01 00 01 | body 0 | serial 1 | array len
    //   a(yv): PATH(1) "o" | MEMBER(3) "s" | DESTINATION(6) "s"
    //   then the message padded to 8.
    let path = "/org/freedesktop/DBus";
    let dest = "org.freedesktop.DBus";
    let mut array: Vec<u8> = Vec::new();
    // PATH
    array.push(1);
    array.extend_from_slice(&[1, b'o', 0]);
    array.extend_from_slice(&(path.len() as u32).to_le_bytes());
    array.extend_from_slice(path.as_bytes());
    array.push(0);
    while array.len() % 8 != 0 {
        array.push(0);
    }
    // MEMBER
    array.push(3);
    array.extend_from_slice(&[1, b's', 0]);
    array.extend_from_slice(&5u32.to_le_bytes());
    array.extend_from_slice(b"Hello\0");
    while array.len() % 8 != 0 {
        array.push(0);
    }
    // DESTINATION
    array.push(6);
    array.extend_from_slice(&[1, b's', 0]);
    array.extend_from_slice(&(dest.len() as u32).to_le_bytes());
    array.extend_from_slice(dest.as_bytes());
    array.push(0);

    let mut want: Vec<u8> = Vec::new();
    want.extend_from_slice(&[b'l', 1, 0, 1]);
    want.extend_from_slice(&0u32.to_le_bytes());
    want.extend_from_slice(&1u32.to_le_bytes());
    want.extend_from_slice(&(array.len() as u32).to_le_bytes());
    want.extend_from_slice(&array);
    while want.len() % 8 != 0 {
        want.push(0);
    }

    let mut m = msg(MessageType::MethodCall, 1);
    m.path = Some(s(path));
    m.destination = Some(s(dest));
    m.member = Some(s("Hello"));
    let got = m.marshal();
    assert_eq!(got, want, "Hello bytes drifted from the spec");
    assert_eq!(got.len() % 8, 0);

    let back = parse_message(&got).unwrap();
    assert_eq!(back.kind, MessageType::MethodCall);
    assert_eq!(back.path.as_deref(), Some(path));
    assert_eq!(back.destination.as_deref(), Some(dest));
    assert_eq!(back.member.as_deref(), Some("Hello"));
    assert!(back.body.is_empty());
}

#[test]
fn golden_takecontrol_call_with_body() {
    // TakeControl(false): PATH / IFACE / MEMBER / DEST / SIGNATURE("b")
    // fields, one boolean body padded to 8.
    let path = "/org/freedesktop/login1/session/c2";
    let dest = "org.freedesktop.login1";
    let iface = "org.freedesktop.login1.Session";
    let mut array: Vec<u8> = Vec::new();
    for (code, sigty, val) in [
        (1u8, b'o', path.as_bytes()),
        (2, b's', iface.as_bytes()),
        (3, b's', b"TakeControl"),
        (6, b's', dest.as_bytes()),
    ] {
        array.push(code);
        array.extend_from_slice(&[1, sigty, 0]);
        array.extend_from_slice(&(val.len() as u32).to_le_bytes());
        array.extend_from_slice(val);
        array.push(0);
        while array.len() % 8 != 0 {
            array.push(0);
        }
    }
    // SIGNATURE field: variant sig "g" carrying "b".
    array.push(8);
    array.extend_from_slice(&[1, b'g', 0]);
    array.extend_from_slice(&[1, b'b', 0]);

    let mut want: Vec<u8> = Vec::new();
    want.extend_from_slice(&[b'l', 1, 0, 1]);
    want.extend_from_slice(&8u32.to_le_bytes()); // 4 body + 4 pad
    want.extend_from_slice(&7u32.to_le_bytes()); // serial
    want.extend_from_slice(&(array.len() as u32).to_le_bytes());
    want.extend_from_slice(&array);
    while want.len() % 8 != 0 {
        want.push(0);
    }
    want.extend_from_slice(&0u32.to_le_bytes()); // bool false
    want.extend_from_slice(&[0; 4]); // body pad to 8

    let mut m = msg(MessageType::MethodCall, 7);
    m.path = Some(s(path));
    m.destination = Some(s(dest));
    m.interface = Some(s(iface));
    m.member = Some(s("TakeControl"));
    m.signature = s("b");
    m.body = vec![DbusValue::Bool(false)];
    assert_eq!(m.marshal(), want);

    let back = parse_message(&m.marshal()).unwrap();
    assert_eq!(back.body, vec![DbusValue::Bool(false)]);
    assert_eq!(back.signature, "b");
}

// ---------------------------------------------------------------------------
// Round-trips across every type.
// ---------------------------------------------------------------------------

#[test]
fn round_trip_every_type() {
    let cases: Vec<DbusValue> = vec![
        DbusValue::Byte(7),
        DbusValue::Bool(true),
        DbusValue::I16(-3),
        DbusValue::U16(0xBEEF),
        DbusValue::I32(-100_000),
        DbusValue::U32(0xDEAD_BEEF),
        DbusValue::I64(-5_000_000_000),
        DbusValue::U64(0xDEAD_BEEF_CAFE),
        DbusValue::Double(1.5),
        DbusValue::Str("héllo".into()),
        DbusValue::Path("/org/a/b".into()),
        DbusValue::Signature("sa{sv}".into()),
        DbusValue::Fd(3),
        DbusValue::Variant(Box::new(DbusValue::Str("v".into()))),
        DbusValue::Array(vec![
            DbusValue::U32(1),
            DbusValue::U32(2),
            DbusValue::U32(3),
        ]),
        DbusValue::Struct(vec![DbusValue::Str("k".into()), DbusValue::I32(9)]),
        DbusValue::Dict(vec![(
            DbusValue::Str("key".into()),
            DbusValue::Variant(Box::new(DbusValue::Bool(true))),
        )]),
    ];
    for value in cases {
        let mut m = msg(MessageType::MethodCall, 5);
        m.signature = value.signature();
        m.body = vec![value.clone()];
        let bytes = m.marshal();
        let back = parse_message(&bytes).unwrap();
        assert_eq!(back.body, vec![value.clone()], "round trip of {value:?}");
        assert_eq!(back.signature, value.signature());
    }
}

#[test]
fn round_trip_multi_value_bodies() {
    // TakeDevice-style "uu", PauseDevice-style "uuss", sleep "b".
    let bodies: Vec<Vec<DbusValue>> = vec![
        vec![DbusValue::U32(226), DbusValue::U32(0)],
        vec![
            DbusValue::U32(226),
            DbusValue::U32(0),
            DbusValue::Str("pause".into()),
            DbusValue::Str("vt".into()),
        ],
        vec![DbusValue::Bool(true)],
        vec![DbusValue::U32(226), DbusValue::U32(0), DbusValue::Fd(2)],
    ];
    for body in bodies {
        let mut m = msg(MessageType::Signal, 9);
        m.signature = body.iter().map(DbusValue::signature).collect();
        m.body = body.clone();
        let back = parse_message(&m.marshal()).unwrap();
        assert_eq!(back.body, body);
    }
}

#[test]
fn reply_serial_and_error_name_round_trip() {
    let mut m = msg(MessageType::Error, 44);
    m.reply_serial = Some(7);
    m.error_name = Some(s("org.freedesktop.DBus.Error.AccessDenied"));
    m.signature = s("s");
    m.body = vec![DbusValue::Str("not allowed".into())];
    let back = parse_message(&m.marshal()).unwrap();
    assert_eq!(back.kind, MessageType::Error);
    assert_eq!(back.reply_serial, Some(7));
    assert_eq!(
        back.error_name.as_deref(),
        Some("org.freedesktop.DBus.Error.AccessDenied")
    );
    assert_eq!(back.body[0], DbusValue::Str("not allowed".into()));
}

#[test]
fn unix_fd_count_field_round_trips() {
    let mut m = msg(MessageType::MethodReturn, 21);
    m.reply_serial = Some(20);
    m.signature = s("hb");
    m.body = vec![DbusValue::Fd(0), DbusValue::Bool(true)];
    m.unix_fds = 1;
    let back = parse_message(&m.marshal()).unwrap();
    assert_eq!(back.unix_fds, 1);
    assert_eq!(back.body[0], DbusValue::Fd(0));
}

// ---------------------------------------------------------------------------
// Malformed corpus: typed failures, never panics.
// ---------------------------------------------------------------------------

#[test]
fn malformed_corpus_fails_typed() {
    let good = baseline_bytes();
    let cases: Vec<(Vec<u8>, DbusError)> = vec![
        (Vec::new(), DbusError::TooShort),
        (good[..10].to_vec(), DbusError::TooShort),
        (with_byte(&good, 0, b'B'), DbusError::BadEndianness),
        (with_byte(&good, 3, 2), DbusError::BadVersion),
        (with_byte(&good, 1, 9), DbusError::BadType),
        (with_word(&good, 12, 0xFFFF_FFFF), DbusError::Overlong),
        (with_word(&good, 4, 0xFFFF_FFFF), DbusError::Overlong),
        (with_word(&good, 8, 0), DbusError::BadType), // serial 0
    ];
    for (bytes, want) in cases {
        assert_eq!(parse_message(&bytes), Err(want), "case {bytes:02x?}");
    }
    // Truncation at every prefix length of a good message with a body.
    let mut m = msg(MessageType::MethodCall, 3);
    m.path = Some(s("/a"));
    m.signature = s("uu");
    m.body = vec![DbusValue::U32(1), DbusValue::U32(2)];
    let bytes = m.marshal();
    for cut in 1..bytes.len() {
        assert!(
            parse_message(&bytes[..cut]).is_err(),
            "truncation at {cut} parsed clean"
        );
    }
}

/// A minimal but valid METHOD_CALL with one header field.
fn baseline_bytes() -> Vec<u8> {
    let mut m = msg(MessageType::MethodCall, 3);
    m.path = Some(s("/a"));
    m.marshal()
}

fn with_byte(bytes: &[u8], idx: usize, v: u8) -> Vec<u8> {
    let mut out = bytes.to_vec();
    out[idx] = v;
    out
}

fn with_word(bytes: &[u8], idx: usize, v: u32) -> Vec<u8> {
    let mut out = bytes.to_vec();
    out[idx..idx + 4].copy_from_slice(&v.to_le_bytes());
    out
}

#[test]
fn object_paths_are_validated() {
    let mut m = msg(MessageType::MethodCall, 3);
    m.path = Some(s("/bad path"));
    // The marshal writes it; the parse rejects it.
    assert_eq!(parse_message(&m.marshal()), Err(DbusError::InvalidString));
    let mut m2 = msg(MessageType::MethodCall, 3);
    m2.path = Some(s("no-leading-slash"));
    assert_eq!(parse_message(&m2.marshal()), Err(DbusError::InvalidString));
}

#[test]
fn signature_of_values() {
    assert_eq!(DbusValue::U32(0).signature(), "u");
    assert_eq!(DbusValue::Fd(0).signature(), "h");
    assert_eq!(DbusValue::Str(String::new()).signature(), "s");
    // A variant's signature character is just "v" — the inner type
    // rides on the wire, not in the outer signature.
    assert_eq!(
        DbusValue::Variant(Box::new(DbusValue::Bool(true))).signature(),
        "v"
    );
    assert_eq!(
        DbusValue::Struct(vec![DbusValue::U32(0), DbusValue::Str(String::new())]).signature(),
        "(us)"
    );
    assert_eq!(
        DbusValue::Dict(vec![(
            DbusValue::Str(String::new()),
            DbusValue::Variant(Box::new(DbusValue::I32(0)))
        )])
        .signature(),
        "a{s:v}"
    );
    assert_eq!(DbusValue::Array(vec![DbusValue::U16(0)]).signature(), "aq");
}
