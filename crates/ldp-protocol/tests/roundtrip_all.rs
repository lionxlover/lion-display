//! Schema-driven round-trip: EVERY operation of EVERY interface in the
//! compiled spec set is instantiated with signature-valid arguments,
//! encoded, decoded, compared, and signature-checked in both validation
//! modes (roadmap Phase 2 EC: "codec round-trips all messages").

use ldp_core::bitset::Bitset128;
use ldp_core::geometry::Rect;
use ldp_core::ids::ObjectId;
use ldp_core::limits::Limits;
use ldp_core::wire::{ArgType, Primitive, Value};
use ldp_protocol::{
    check_signature, decode, ArgSchema, Direction, Message, SchemaRegistry, ValidationMode,
    MODULES, REGISTRY,
};

/// Deterministic value synthesis for one argument schema.
fn synth(registry: &SchemaRegistry<'_>, module: &str, a: &ArgSchema, dir: Direction) -> Value {
    match a.ty {
        ArgType::Int32 => Value::Int32(-7),
        ArgType::Uint32 => Value::Uint32(7),
        ArgType::Int64 => Value::Int64(-9),
        ArgType::Uint64 => Value::Uint64(9),
        ArgType::Float32 => Value::float32(0.5).unwrap(),
        ArgType::Float64 => Value::float64(0.25).unwrap(),
        ArgType::Bool => Value::Bool(true),
        ArgType::String => Value::String("ldp".into()),
        ArgType::Ts => Value::Ts(1_234_567_890),
        ArgType::Rect => Value::Rect(Rect::new(1, 2, 3, 4)),
        ArgType::Object => Value::Object(Some(ObjectId::client(0x20).unwrap())),
        ArgType::NewId => match dir {
            Direction::Request => Value::NewId(ObjectId::client(3).unwrap()),
            Direction::Event => Value::NewId(ObjectId::server(0x8000_0003).unwrap()),
        },
        ArgType::Fd => Value::Fd(0),
        ArgType::Enum => {
            let def = registry
                .resolve_enum(module, a.of.expect("enum args carry 'of'"))
                .expect("enum 'of' resolves");
            Value::Enum(def.values[0].1)
        }
        ArgType::Bitset => {
            let def = registry
                .resolve_bitset(module, a.of.expect("bitset args carry 'of'"))
                .expect("bitset 'of' resolves");
            Value::Bitset(Bitset128::single(def.bits[0].1))
        }
        ArgType::Array => {
            let elem = ldp_protocol::schema::arg_type_from_spec_name(a.of.expect("array 'of'"))
                .expect("array element type");
            let one = match elem {
                ArgType::Int32 => Primitive::Int32(-1),
                ArgType::Uint32 => Primitive::Uint32(1),
                ArgType::Int64 => Primitive::Int64(-2),
                ArgType::Uint64 => Primitive::Uint64(2),
                ArgType::Float32 => Primitive::Float32(0.5),
                ArgType::Float64 => Primitive::Float64(0.5),
                ArgType::Bool => Primitive::Bool(false),
                ArgType::Fd => Primitive::Fd(0),
                ArgType::Rect => Primitive::Rect(Rect::new(0, 0, 5, 5)),
                _ => unreachable!("element types are primitives"),
            };
            Value::array(elem, vec![one]).unwrap()
        }
        _ => unreachable!("synthesis covers every wire type"),
    }
}

#[test]
fn every_operation_of_every_interface_round_trips() {
    let limits = Limits::DEFAULT;
    let mut total_ops = 0usize;
    let mut fd_msgs = 0usize;

    for &module in MODULES {
        for iface in module.interfaces {
            for (ops, dir) in [
                (iface.requests, Direction::Request),
                (iface.events, Direction::Event),
            ] {
                for op in ops {
                    total_ops += 1;
                    let mut m = Message::new(0x1000, op.opcode);
                    if dir == Direction::Request && op.reply.is_some() {
                        m = m.reply();
                    }
                    for a in op.args {
                        m = m.arg(synth(&REGISTRY, module.name, a, dir));
                    }
                    if m.required_fd_count() > 0 {
                        fd_msgs += 1;
                    }
                    let bytes = m.encode(&limits).expect("encode");
                    let fd_count = m.required_fd_count();
                    let back =
                        decode(&bytes, fd_count, &limits, ValidationMode::Strict).expect("decode");
                    assert_eq!(back, m, "{}.{} did not round-trip", iface.name, op.name);

                    for mode in [ValidationMode::Tolerant, ValidationMode::Strict] {
                        let (_, matched) = check_signature(
                            &REGISTRY,
                            &back,
                            iface.name,
                            dir,
                            iface.version_max,
                            mode,
                        )
                        .unwrap_or_else(|e| {
                            panic!(
                                "{}.{} failed signature check in {mode:?}: {e}",
                                iface.name, op.name
                            )
                        });
                        assert_eq!(matched.name, op.name);
                    }
                }
            }
        }
    }

    // The v1 inventory: 98 requests + 123 events across 34 interfaces
    // (the Phase 4 amendment added `connection.get_registry`; Phase 22's
    // capture module added grab/frame/failed; Phase 45's material
    // amendment added `toplevel.set_material`, appended after the
    // frozen opcodes — purely additive on the wire; Phase 47's
    // semantic-scene amendment added the toplevel triple
    // set_semantic_role/set_security_class/set_scene_profile, the
    // same additive doctrine; Phase 50's operator's-hand amendment
    // added toplevel start_move/start_resize, the same doctrine
    // again; Phase 51's view-switch amendment added the shell pair
    // switch_workspace/workspace_switched, on the shell global).
    assert_eq!(total_ops, 98 + 123);
    assert!(fd_msgs > 0, "fd-carrying operations must be exercised");
}

#[test]
fn opcode_tables_are_dense_and_frozen() {
    for &module in MODULES {
        for iface in module.interfaces {
            for (ops, kind) in [(iface.requests, "requests"), (iface.events, "events")] {
                for (idx, op) in ops.iter().enumerate() {
                    assert_eq!(
                        op.opcode,
                        idx as u32 + 1,
                        "{} {} opcode numbering must be dense from 1",
                        iface.name,
                        kind
                    );
                }
            }
            // Every reply names a declared event of the same interface.
            for r in iface.requests {
                if let Some(reply) = r.reply {
                    assert!(
                        iface.events.iter().any(|e| e.name == reply),
                        "{}: reply '{reply}' must be a declared event",
                        iface.name
                    );
                }
            }
        }
    }
}

#[test]
fn empty_messages_are_legal_and_exact() {
    // Find zero-arg operations in the spec set and round-trip them.
    let mut found = 0;
    'outer: for &module in MODULES {
        for iface in module.interfaces {
            for op in iface.events.iter().chain(iface.requests.iter()) {
                if op.args.is_empty() {
                    let m = Message::new(0x8000_0001, op.opcode);
                    let bytes = m.encode(&Limits::DEFAULT).unwrap();
                    assert_eq!(bytes.len(), 16);
                    let back = decode(&bytes, 0, &Limits::DEFAULT, ValidationMode::Strict).unwrap();
                    assert_eq!(back, m);
                    found += 1;
                    if found >= 3 {
                        break 'outer;
                    }
                }
            }
        }
    }
    assert!(found >= 1, "spec v1 should contain zero-arg operations");
}

#[test]
fn worked_example_matches_the_documented_bytes() {
    // docs/protocol.md §4.1: four scalar units after a 16-byte header.
    let m = Message::new(0x8000_0100, 4)
        .arg(Value::Int32(10))
        .arg(Value::Int32(20))
        .arg(Value::Uint32(100))
        .arg(Value::Uint32(50));
    let bytes = m.encode(&Limits::DEFAULT).unwrap();
    assert_eq!(
        bytes,
        vec![
            0x04, 0x00, 0x00, 0x00, // payload_words = 4
            0x00, 0x01, 0x00, 0x80, // object_id = 0x80000100
            0x04, 0x00, 0x00, 0x00, // opcode = 4
            0x00, 0x00, // flags
            0x00, 0x00, // fd_count
            0x01, 0x0A, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // int32 10
            0x01, 0x14, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // int32 20
            0x02, 0x64, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // uint32 100
            0x02, 0x32, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // uint32 50
        ]
    );
}
