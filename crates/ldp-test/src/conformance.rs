//! The spec-walking conformance runner.
//!
//! The spec is the oracle: [`ldp_protocol::REGISTRY`] is the compiled
//! form of `spec/*.toml`, and this module *derives* the conformance
//! suite from it instead of enumerating cases by hand. For every
//! interface, direction, and opcode it:
//!
//! 1. synthesizes a canonical argument list from the operation's
//!    signature (declared enum values, in-width bitsets, ownership-
//!    correct new_ids, FD indices backed by the message's own table),
//! 2. proves the full round-trip —
//!    [`Message::encode`] → [`decode`] (strict) → argument equality →
//!    [`check_signature`] (strict) resolving back to the same
//!    interface and operation,
//! 3. sweeps the per-type boundary matrix over every argument
//!    position: the valid extremes must survive the round-trip
//!    unchanged, and the deliberately-invalid shapes must be rejected
//!    at the *documented* stage with the *documented* wire error
//!    (null objects on non-nullable arguments, undeclared enum values,
//!    over-width bitsets, out-of-range new_ids, FD indices outside the
//!    ancillary table, over-limit strings).
//!
//! A failure anywhere is collected into the report with a fully
//! qualified location — the suite's own test asserts the report is
//! empty, so any spec/codec disagreement fails CI with the exact
//! operation and argument named.

use ldp_core::error::ErrorCode;
use ldp_core::geometry::Rect;
use ldp_core::ids::{ObjectId, SERVER_OBJECT_FLAG};
use ldp_core::limits::Limits;
use ldp_core::wire::{ArgType, Primitive, Value};
use ldp_protocol::schema::{ArgSchema, Direction, InterfaceSchema, ModuleSchema, OpSchema};
use ldp_protocol::{check_signature, decode, Message, SchemaRegistry, ValidationMode};

/// Aggregate outcome of one full registry walk.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConformanceReport {
    /// Modules walked.
    pub modules: usize,
    /// Interfaces walked.
    pub interfaces: usize,
    /// Operations walked (requests + events).
    pub operations: usize,
    /// Messages that proved the encode/decode/validate round-trip.
    pub round_trips: usize,
    /// Deliberately-invalid shapes rejected with the expected error.
    pub rejections: usize,
    /// Every disagreement between spec and codec, fully located.
    pub failures: Vec<ConformanceFailure>,
}

/// One located conformance failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConformanceFailure {
    /// Fully qualified location
    /// (`"<interface> <direction> <op> arg '<name>'"`).
    pub location: String,
    /// What disagreed.
    pub detail: String,
}

impl ConformanceReport {
    /// Whether the walk found no disagreement.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.failures.is_empty()
    }
}

/// Walk every module/interface/operation of `registry` and prove the
/// codec against the spec.
///
/// `seed` addresses the boundary sweep's non-deterministic-looking
/// choices (there are none today — the matrix is exhaustive — but the
/// parameter keeps the runner seed-addressed like every other LDP
/// harness, and future sweeps may use it).
#[must_use]
pub fn run_registry_conformance(registry: &SchemaRegistry<'_>, _seed: u64) -> ConformanceReport {
    let mut report = ConformanceReport::default();
    for module in registry.modules() {
        report.modules += 1;
        for iface in module.interfaces {
            report.interfaces += 1;
            walk_interface(registry, module, iface, &mut report);
        }
    }
    report
}

fn walk_interface(
    registry: &SchemaRegistry<'_>,
    module: &ModuleSchema,
    iface: &InterfaceSchema,
    report: &mut ConformanceReport,
) {
    let directions = [
        (Direction::Request, iface.requests),
        (Direction::Event, iface.events),
    ];
    for (dir, ops) in directions {
        for op in ops {
            report.operations += 1;
            walk_op(registry, module, iface, dir, op, report);
        }
    }
}

fn walk_op(
    registry: &SchemaRegistry<'_>,
    module: &ModuleSchema,
    iface: &InterfaceSchema,
    dir: Direction,
    op: &OpSchema,
    report: &mut ConformanceReport,
) {
    let where_ = format!("{} {} {}", iface.name, dir.as_str(), op.name);

    // 1. Canonical argument synthesis from the signature.
    let mut args = Vec::with_capacity(op.args.len());
    for (index, a) in op.args.iter().enumerate() {
        match canonical_arg(registry, module.name, a, dir, index) {
            Ok(v) => args.push(v),
            Err(e) => report.failures.push(ConformanceFailure {
                location: format!("{where_} arg '{}'", a.name),
                detail: e,
            }),
        }
    }
    if report
        .failures
        .iter()
        .any(|f| f.location.starts_with(&where_) && f.location.contains("arg '"))
    {
        // Synthesis already failed for this op: the boundary sweep
        // would only repeat the same schema-level complaint.
        return;
    }

    // 2. Canonical round-trip + strict signature resolution.
    let msg = build_message(dir, op.opcode, args.clone());
    match round_trip(registry, iface, dir, op, &msg) {
        Ok(()) => report.round_trips += 1,
        Err(e) => report.failures.push(ConformanceFailure {
            location: where_.clone(),
            detail: e,
        }),
    }

    // 3. Boundary matrix over every argument position.
    for (index, a) in op.args.iter().enumerate() {
        for case in boundary_cases(registry, module.name, a, dir) {
            let saved = args[index].clone();
            let mut variant = args.clone();
            variant[index] = case.value;
            let location = format!("{where_} arg '{}'", a.name);
            match case.expect {
                Expect::Valid => {
                    let msg = build_message(dir, op.opcode, variant);
                    match round_trip(registry, iface, dir, op, &msg) {
                        Ok(()) => report.round_trips += 1,
                        Err(e) => report.failures.push(ConformanceFailure {
                            location,
                            detail: format!("valid boundary '{}' rejected: {e}", case.label),
                        }),
                    }
                }
                Expect::Invalid(code) => {
                    let msg = build_message(dir, op.opcode, variant);
                    match expect_rejection(registry, iface, dir, op, &msg, code) {
                        Ok(()) => report.rejections += 1,
                        Err(e) => report.failures.push(ConformanceFailure {
                            location,
                            detail: format!("invalid boundary '{}': {e}", case.label),
                        }),
                    }
                }
                Expect::EncodeLimit => {
                    // Over-limit strings must fail at the encoder
                    // before any byte reaches the wire.
                    let msg = build_message(dir, op.opcode, variant);
                    match msg.encode(&Limits::DEFAULT) {
                        Err(ldp_core::error::LdpError::Limit { .. }) => {
                            report.rejections += 1;
                        }
                        Err(e) => report.failures.push(ConformanceFailure {
                            location,
                            detail: format!(
                                "over-limit string '{}' failed with the wrong error: {e}",
                                case.label
                            ),
                        }),
                        Ok(_) => report.failures.push(ConformanceFailure {
                            location,
                            detail: format!(
                                "over-limit string '{}' encoded successfully (limit ignored)",
                                case.label
                            ),
                        }),
                    }
                }
            }
            args[index] = saved;
        }
    }
}

/// Build the message for `dir` with an ownership-correct target id
/// (requests address server-side objects, events address client-side
/// proxies; the target id itself is not signature-validated — the
/// object store owns that check at dispatch — but representative
/// ranges keep the suite honest).
fn build_message(dir: Direction, opcode: u32, args: Vec<Value>) -> Message {
    let target = match dir {
        Direction::Request => ldp_core::ids::CONNECTION_OBJECT_ID,
        Direction::Event => SERVER_OBJECT_FLAG | 1,
    };
    let mut msg = Message::new(target, opcode);
    for v in args {
        msg = msg.arg(v);
    }
    msg
}

fn round_trip(
    registry: &SchemaRegistry<'_>,
    iface: &InterfaceSchema,
    dir: Direction,
    op: &OpSchema,
    msg: &Message,
) -> Result<(), String> {
    let limits = Limits::DEFAULT;
    let fd_count = msg.required_fd_count();
    let bytes = msg
        .encode(&limits)
        .map_err(|e| format!("encode failed: {e}"))?;
    let back = decode(&bytes, fd_count, &limits, ValidationMode::Strict)
        .map_err(|e| format!("strict decode failed: {e}"))?;
    if &back != msg {
        return Err(format!(
            "round-trip changed the message: sent {msg:?}, got {back:?}"
        ));
    }
    let (resolved_iface, resolved_op) = check_signature(
        registry,
        &back,
        iface.name,
        dir,
        iface.version_max,
        ValidationMode::Strict,
    )
    .map_err(|e| format!("strict signature check failed: {e}"))?;
    if resolved_iface.name != iface.name || resolved_op.name != op.name {
        return Err(format!(
            "signature resolved to '{}.{}', expected '{}.{}'",
            resolved_iface.name, resolved_op.name, iface.name, op.name
        ));
    }
    Ok(())
}

fn expect_rejection(
    registry: &SchemaRegistry<'_>,
    iface: &InterfaceSchema,
    dir: Direction,
    op: &OpSchema,
    msg: &Message,
    want: ErrorCode,
) -> Result<(), String> {
    let limits = Limits::DEFAULT;
    let bytes = msg
        .encode(&limits)
        .map_err(|e| format!("encode of an invalid shape failed: {e}"))?;
    let fd_count = msg.required_fd_count();
    // The codec is structural: the invalid enum/bitset/object shapes
    // decode fine and must be rejected by the *signature* stage; FD
    // index shapes are rejected by the decoder against the declared
    // table size.
    if let Ok(back) = decode(&bytes, fd_count, &limits, ValidationMode::Strict) {
        let err = check_signature(
            registry,
            &back,
            iface.name,
            dir,
            iface.version_max,
            ValidationMode::Strict,
        )
        .err();
        match err {
            Some(e) if e.wire_code() == Some(want) => Ok(()),
            Some(e) => {
                let got = e.wire_code();
                let iface_name = iface.name;
                let dir_name = dir.as_str();
                let op_name = op.name;
                Err(format!(
                    "rejected with {got:?}, expected {want:?} ({iface_name} {dir_name} {op_name})"
                ))
            }
            None => {
                let iface_name = iface.name;
                let dir_name = dir.as_str();
                let op_name = op.name;
                Err(format!(
                    "accepted by strict signature validation ({iface_name} {dir_name} {op_name})"
                ))
            }
        }
    } else {
        // Decode-time rejection is the FD-index path: confirm the code.
        let err = decode(&bytes, fd_count, &limits, ValidationMode::Strict)
            .expect_err("checked above")
            .wire_code();
        if err == Some(want) {
            Ok(())
        } else {
            Err(format!("decoder rejected with {err:?}, expected {want:?}"))
        }
    }
}

// ---- canonical value synthesis --------------------------------------

/// Synthesize the canonical valid value for one argument schema.
fn canonical_arg(
    registry: &SchemaRegistry<'_>,
    module: &str,
    a: &ArgSchema,
    dir: Direction,
    index: usize,
) -> Result<Value, String> {
    match a.ty {
        ArgType::Int32 => Ok(Value::Int32(1)),
        ArgType::Uint32 => Ok(Value::Uint32(1)),
        ArgType::Int64 => Ok(Value::Int64(1)),
        ArgType::Uint64 => Ok(Value::Uint64(1)),
        ArgType::Float32 => Ok(Value::Float32(0.5)),
        ArgType::Float64 => Ok(Value::Float64(0.25)),
        ArgType::Bool => Ok(Value::Bool(true)),
        ArgType::Ts => Ok(Value::Ts(1_000_000)),
        ArgType::String => Ok(Value::String("conformance".into())),
        ArgType::Rect => Ok(Value::Rect(Rect::new(1, 2, 3, 4))),
        ArgType::Object => {
            let id = match dir {
                Direction::Request => ObjectId::client(0x0100 + index as u32),
                Direction::Event => ObjectId::server(SERVER_OBJECT_FLAG | 0x0100),
            }
            .expect("canonical object id stays in range");
            Ok(Value::Object(Some(id)))
        }
        ArgType::NewId => {
            let id = match dir {
                Direction::Request => ObjectId::client(0x0200 + index as u32),
                Direction::Event => ObjectId::server(SERVER_OBJECT_FLAG | 0x0200),
            }
            .expect("canonical new_id stays in range");
            Ok(Value::NewId(id))
        }
        ArgType::Fd => Ok(Value::Fd(0)),
        ArgType::Enum => {
            let of =
                a.of.ok_or_else(|| "enum argument without 'of'".to_string())?;
            let def = registry
                .resolve_enum(module, of)
                .ok_or_else(|| format!("enum '{of}' does not resolve"))?;
            let value = def
                .values
                .first()
                .map(|(_, v)| *v)
                .ok_or_else(|| format!("enum '{of}' declares no values"))?;
            Ok(Value::Enum(value))
        }
        ArgType::Bitset => {
            let of =
                a.of.ok_or_else(|| "bitset argument without 'of'".to_string())?;
            let def = registry
                .resolve_bitset(module, of)
                .ok_or_else(|| format!("bitset '{of}' does not resolve"))?;
            let bit = def
                .bits
                .first()
                .map(|(_, b)| *b)
                .ok_or_else(|| format!("bitset '{of}' declares no bits"))?;
            Ok(Value::Bitset(ldp_core::bitset::Bitset128::single(bit)))
        }
        ArgType::Array => {
            let of =
                a.of.ok_or_else(|| "array argument without 'of'".to_string())?;
            let element = ldp_protocol::schema::arg_type_from_spec_name(of)
                .ok_or_else(|| format!("array element '{of}' is not an element type"))?;
            let items = match element {
                ArgType::Int32 => vec![Primitive::Int32(-1), Primitive::Int32(1)],
                ArgType::Uint32 => vec![Primitive::Uint32(0), Primitive::Uint32(1)],
                ArgType::Int64 => vec![Primitive::Int64(-2), Primitive::Int64(2)],
                ArgType::Uint64 => vec![Primitive::Uint64(0), Primitive::Uint64(2)],
                ArgType::Float32 => vec![Primitive::Float32(0.5), Primitive::Float32(-0.5)],
                ArgType::Float64 => vec![Primitive::Float64(0.25), Primitive::Float64(-0.25)],
                ArgType::Bool => vec![Primitive::Bool(false), Primitive::Bool(true)],
                ArgType::Fd => vec![Primitive::Fd(0), Primitive::Fd(1)],
                ArgType::Rect => vec![
                    Primitive::Rect(Rect::new(0, 0, 1, 1)),
                    Primitive::Rect(Rect::new(-1, -1, 2, 2)),
                ],
                _ => return Err(format!("unsupported element type {element:?}")),
            };
            Value::array(element, items).map_err(|e| format!("array synthesis: {e}"))
        }
        _ => Err("argument type outside the codec vocabulary".into()),
    }
}

// ---- boundary matrix -------------------------------------------------

/// What one boundary case must do.
#[derive(Clone, Copy, Debug)]
enum Expect {
    /// Survive the full round-trip unchanged.
    Valid,
    /// Be rejected with this wire error code.
    Invalid(ErrorCode),
    /// Fail at the encoder with a limit error.
    EncodeLimit,
}

/// One boundary case for one argument position.
struct BoundaryCase {
    label: &'static str,
    value: Value,
    expect: Expect,
}

/// The per-type boundary matrix for one argument schema: scalar
/// shapes first, then the domain-checked shapes (objects, new_ids,
/// FDs, enums, bitsets, arrays).
fn boundary_cases(
    registry: &SchemaRegistry<'_>,
    module: &str,
    a: &ArgSchema,
    dir: Direction,
) -> Vec<BoundaryCase> {
    let mut cases = scalar_boundary_cases(a);
    cases.extend(domain_boundary_cases(registry, module, a, dir));
    cases
}

// Exhaustive per-type tables: each arm is 1–4 push statements and the
// match mirrors `ArgType` exactly — splitting it would scatter one
// table across helpers, so the length is accepted deliberately.
#[allow(clippy::too_many_lines)]
fn scalar_boundary_cases(a: &ArgSchema) -> Vec<BoundaryCase> {
    let mut cases = Vec::new();
    match a.ty {
        ArgType::Int32 => {
            cases.push(BoundaryCase {
                label: "min",
                value: Value::Int32(i32::MIN),
                expect: Expect::Valid,
            });
            cases.push(BoundaryCase {
                label: "zero",
                value: Value::Int32(0),
                expect: Expect::Valid,
            });
            cases.push(BoundaryCase {
                label: "max",
                value: Value::Int32(i32::MAX),
                expect: Expect::Valid,
            });
        }
        ArgType::Uint32 => {
            cases.push(BoundaryCase {
                label: "zero",
                value: Value::Uint32(0),
                expect: Expect::Valid,
            });
            cases.push(BoundaryCase {
                label: "max",
                value: Value::Uint32(u32::MAX),
                expect: Expect::Valid,
            });
        }
        ArgType::Int64 => {
            cases.push(BoundaryCase {
                label: "min",
                value: Value::Int64(i64::MIN),
                expect: Expect::Valid,
            });
            cases.push(BoundaryCase {
                label: "max",
                value: Value::Int64(i64::MAX),
                expect: Expect::Valid,
            });
        }
        ArgType::Uint64 => {
            cases.push(BoundaryCase {
                label: "zero",
                value: Value::Uint64(0),
                expect: Expect::Valid,
            });
            cases.push(BoundaryCase {
                label: "max",
                value: Value::Uint64(u64::MAX),
                expect: Expect::Valid,
            });
        }
        ArgType::Float32 => {
            cases.push(BoundaryCase {
                label: "smallest",
                value: Value::Float32(f32::MIN_POSITIVE),
                expect: Expect::Valid,
            });
            cases.push(BoundaryCase {
                label: "largest",
                value: Value::Float32(f32::MAX),
                expect: Expect::Valid,
            });
            cases.push(BoundaryCase {
                label: "lowest",
                value: Value::Float32(-f32::MAX),
                expect: Expect::Valid,
            });
        }
        ArgType::Float64 => {
            cases.push(BoundaryCase {
                label: "smallest",
                value: Value::Float64(f64::MIN_POSITIVE),
                expect: Expect::Valid,
            });
            cases.push(BoundaryCase {
                label: "largest",
                value: Value::Float64(f64::MAX),
                expect: Expect::Valid,
            });
        }
        ArgType::Bool => {
            cases.push(BoundaryCase {
                label: "false",
                value: Value::Bool(false),
                expect: Expect::Valid,
            });
        }
        ArgType::Ts => {
            cases.push(BoundaryCase {
                label: "zero",
                value: Value::Ts(0),
                expect: Expect::Valid,
            });
            cases.push(BoundaryCase {
                label: "max",
                value: Value::Ts(u64::MAX),
                expect: Expect::Valid,
            });
        }
        ArgType::String => {
            cases.push(BoundaryCase {
                label: "empty",
                value: Value::String("".into()),
                expect: Expect::Valid,
            });
            let exact = "x".repeat(Limits::DEFAULT.string_bytes as usize);
            cases.push(BoundaryCase {
                label: "at-limit",
                value: Value::String(exact.into()),
                expect: Expect::Valid,
            });
            let over = "x".repeat(Limits::DEFAULT.string_bytes as usize + 1);
            cases.push(BoundaryCase {
                label: "one-past-limit",
                value: Value::String(over.into()),
                expect: Expect::EncodeLimit,
            });
        }
        ArgType::Rect => {
            cases.push(BoundaryCase {
                label: "empty",
                value: Value::Rect(Rect::new(0, 0, 0, 0)),
                expect: Expect::Valid,
            });
            cases.push(BoundaryCase {
                label: "min-origin",
                value: Value::Rect(Rect::new(i32::MIN, i32::MIN, 1, 1)),
                expect: Expect::Valid,
            });
            cases.push(BoundaryCase {
                label: "max-extent",
                value: Value::Rect(Rect::new(i32::MAX, i32::MAX, u32::MAX, u32::MAX)),
                expect: Expect::Valid,
            });
        }
        // Domain-checked shapes (objects, new_ids, FDs, enums,
        // bitsets, arrays) are filled in by domain_boundary_cases;
        // every other type falls through with no scalar boundaries.
        _ => {}
    }
    cases
}

// Same doctrine as `scalar_boundary_cases`: the match is the table.
#[allow(clippy::too_many_lines)]
fn domain_boundary_cases(
    registry: &SchemaRegistry<'_>,
    module: &str,
    a: &ArgSchema,
    dir: Direction,
) -> Vec<BoundaryCase> {
    let mut cases = Vec::new();
    match a.ty {
        ArgType::Object => {
            if a.nullable {
                cases.push(BoundaryCase {
                    label: "null",
                    value: Value::Object(None),
                    expect: Expect::Valid,
                });
            } else {
                cases.push(BoundaryCase {
                    label: "null",
                    value: Value::Object(None),
                    expect: Expect::Invalid(ErrorCode::InvalidObject),
                });
            }
            cases.push(BoundaryCase {
                label: "wire-zero-raw",
                value: Value::Object(Some(ObjectId::from_wire(1))),
                expect: Expect::Valid,
            });
        }
        ArgType::NewId => {
            let (wrong, right): (Value, Value) = match dir {
                Direction::Request => (
                    Value::NewId(ObjectId::from_wire(SERVER_OBJECT_FLAG | 1)),
                    Value::NewId(
                        ObjectId::client(u32::MAX ^ SERVER_OBJECT_FLAG).expect("client max"),
                    ),
                ),
                Direction::Event => (
                    Value::NewId(ObjectId::from_wire(1)),
                    Value::NewId(ObjectId::server(u32::MAX).expect("server max")),
                ),
            };
            cases.push(BoundaryCase {
                label: "wrong-ownership",
                value: wrong,
                expect: Expect::Invalid(ErrorCode::InvalidObject),
            });
            cases.push(BoundaryCase {
                label: "ownership-max",
                value: right,
                expect: Expect::Valid,
            });
        }
        ArgType::Fd => {
            // Fd(index) is structurally valid whenever index < the
            // message's own FD table (required_fd_count derives from
            // the args). The rejection shape needs a *declared* table
            // smaller than the reference — exercised below via the
            // dedicated decoder path.
            cases.push(BoundaryCase {
                label: "index-zero",
                value: Value::Fd(0),
                expect: Expect::Valid,
            });
            cases.push(BoundaryCase {
                label: "index-large",
                value: Value::Fd(60),
                expect: Expect::Valid,
            });
        }
        ArgType::Enum => {
            if let Some(of) = a.of {
                if let Some(def) = registry.resolve_enum(module, of) {
                    if let Some((_, last)) = def.values.iter().last().copied() {
                        cases.push(BoundaryCase {
                            label: "declared-last",
                            value: Value::Enum(last),
                            expect: Expect::Valid,
                        });
                    }
                    if !def.contains(0) {
                        cases.push(BoundaryCase {
                            label: "undeclared-zero",
                            value: Value::Enum(0),
                            expect: Expect::Invalid(ErrorCode::OutOfRange),
                        });
                    }
                    cases.push(BoundaryCase {
                        label: "undeclared-max",
                        value: Value::Enum(u32::MAX),
                        expect: Expect::Invalid(ErrorCode::OutOfRange),
                    });
                }
            }
        }
        ArgType::Bitset => {
            if let Some(of) = a.of {
                if let Some(def) = registry.resolve_bitset(module, of) {
                    let width = def.width();
                    cases.push(BoundaryCase {
                        label: "empty",
                        value: Value::Bitset(ldp_core::bitset::Bitset128::EMPTY),
                        expect: Expect::Valid,
                    });
                    if width > 0 && width < 128 {
                        cases.push(BoundaryCase {
                            label: "bit-at-width",
                            value: Value::Bitset(ldp_core::bitset::Bitset128::single(width)),
                            expect: Expect::Invalid(ErrorCode::OutOfRange),
                        });
                    }
                    if width < 128 {
                        let full = ldp_core::bitset::Bitset128::from_words([
                            u32::MAX,
                            u32::MAX,
                            u32::MAX,
                            u32::MAX,
                        ]);
                        cases.push(BoundaryCase {
                            label: "all-128-bits",
                            value: Value::Bitset(full),
                            expect: Expect::Invalid(ErrorCode::OutOfRange),
                        });
                    }
                }
            }
        }
        ArgType::Array => {
            if let Some(of) = a.of {
                if let Some(element) = ldp_protocol::schema::arg_type_from_spec_name(of) {
                    let empty = Value::array(element, Vec::<Primitive>::new())
                        .expect("empty arrays are always representable");
                    cases.push(BoundaryCase {
                        label: "empty",
                        value: empty,
                        expect: Expect::Valid,
                    });
                }
            }
        }
        _ => {}
    }
    cases
}

// ---- rng-driven message synthesis (fuzzer corpus) --------------------

/// Synthesize one *random but valid* argument value for `a` — the
/// fuzzer-corpus counterpart of the canonical synthesis above: values
/// stay inside the schema's declared domain (declared enum values,
/// in-width bitsets, ownership-correct ids) so every generated message
/// is a legitimate wire citizen the codec must round-trip.
fn random_valid_arg(
    rng: &mut crate::rng::SplitMix64,
    registry: &SchemaRegistry<'_>,
    module: &str,
    a: &ArgSchema,
    dir: Direction,
) -> Value {
    match a.ty {
        ArgType::Int32 => {
            let jitter = (rng.next_u32() as i32) / 7;
            Value::Int32(*rng.pick(&[0i32, 1, -1, i32::MIN, i32::MAX, jitter]))
        }
        ArgType::Uint32 => {
            let jitter = rng.next_u32() / 11;
            Value::Uint32(*rng.pick(&[0u32, 1, u32::MAX, jitter]))
        }
        ArgType::Int64 => Value::Int64(*rng.pick(&[0i64, 1, -1, i64::MIN, i64::MAX])),
        ArgType::Uint64 => {
            let jitter = rng.next_u64() / 13;
            Value::Uint64(*rng.pick(&[0u64, 1, u64::MAX, jitter]))
        }
        ArgType::Float32 => Value::Float32(rng.next_f64() as f32),
        ArgType::Float64 => Value::Float64(rng.next_f64()),
        ArgType::Bool => Value::Bool(rng.next_bool()),
        ArgType::Ts => Value::Ts(rng.next_u64()),
        ArgType::String => {
            const ALPHABET: &[u8] = b"ldpconformance-0123456789";
            let len = rng.below(48);
            let s: String = (0..len)
                .map(|_| ALPHABET[rng.below(ALPHABET.len())] as char)
                .collect();
            Value::String(s.into())
        }
        ArgType::Rect => Value::Rect(Rect::new(
            rng.below(4096) as i32,
            rng.below(2160) as i32,
            rng.below(1024) as u32,
            rng.below(1024) as u32,
        )),
        ArgType::Object => {
            let id = match dir {
                Direction::Request => ObjectId::client(0x0100 + rng.below(0x100) as u32),
                Direction::Event => ObjectId::server(SERVER_OBJECT_FLAG | 0x0100),
            }
            .expect("in-range object id");
            if a.nullable && rng.next_bool() {
                Value::Object(None)
            } else {
                Value::Object(Some(id))
            }
        }
        ArgType::NewId => {
            let id = match dir {
                Direction::Request => ObjectId::client(0x0200 + rng.below(0x100) as u32),
                Direction::Event => ObjectId::server(SERVER_OBJECT_FLAG | 0x0200),
            }
            .expect("in-range new_id");
            Value::NewId(id)
        }
        ArgType::Fd => Value::Fd(0),
        ArgType::Enum => {
            let declared =
                a.of.and_then(|of| registry.resolve_enum(module, of))
                    .and_then(|def| def.values.first().map(|(_, v)| *v))
                    .unwrap_or(1);
            Value::Enum(declared)
        }
        ArgType::Bitset => {
            let width =
                a.of.and_then(|of| registry.resolve_bitset(module, of))
                    .map_or(0, ldp_protocol::schema::BitsetDef::width);
            let mut set = ldp_core::bitset::Bitset128::EMPTY;
            for bit in 0..width.min(64) {
                if rng.next_bool() {
                    set = set.with(bit);
                }
            }
            Value::Bitset(set)
        }
        ArgType::Array => {
            let element =
                a.of.and_then(ldp_protocol::schema::arg_type_from_spec_name)
                    .unwrap_or(ArgType::Uint32);
            let count = rng.below(8);
            let items: Vec<Primitive> = (0..count)
                .map(|_| match element {
                    ArgType::Int32 => Primitive::Int32(rng.next_u32() as i32),
                    ArgType::Uint32 => Primitive::Uint32(rng.next_u32()),
                    ArgType::Int64 => Primitive::Int64(rng.next_u64() as i64),
                    ArgType::Uint64 => Primitive::Uint64(rng.next_u64()),
                    ArgType::Float32 => Primitive::Float32(rng.next_f64() as f32),
                    ArgType::Float64 => Primitive::Float64(rng.next_f64()),
                    ArgType::Bool => Primitive::Bool(rng.next_bool()),
                    ArgType::Fd => Primitive::Fd(0),
                    _ => Primitive::Rect(Rect::new(0, 0, 1, 1)),
                })
                .collect();
            Value::array(element, items).unwrap_or(Value::Uint32(0))
        }
        _ => Value::Uint32(0),
    }
}

/// Synthesize one random *valid* message for
/// `(interface, direction, opcode)` straight from the compiled spec.
///
/// The fuzzer-corpus generator: every produced message satisfies its
/// signature (types, declared domains, id ownership), so the codec
/// round-trip invariant applies to it. Returns `None` when the
/// interface or opcode does not exist in `registry`.
#[must_use]
pub fn spec_message(
    rng: &mut crate::rng::SplitMix64,
    registry: &SchemaRegistry<'_>,
    interface: &str,
    dir: Direction,
    opcode: u32,
) -> Option<Message> {
    let iface = registry.interface(interface)?;
    let op = iface.find_op(dir, opcode)?;
    let module = registry.module_of_interface(interface)?.name;
    let mut msg = Message::new(
        match dir {
            Direction::Request => ldp_core::ids::CONNECTION_OBJECT_ID,
            Direction::Event => SERVER_OBJECT_FLAG | 1,
        },
        opcode,
    );
    for a in op.args {
        msg = msg.arg(random_valid_arg(rng, registry, module, a, dir));
    }
    Some(msg)
}

/// The dedicated FD-index rejection proof: a message referencing
/// `index` decoded against a *declared* table of exactly `index`
/// entries must fail with [`ErrorCode::FdMismatch`] — the ancillary
/// table is the truth, the argument must point inside it.
///
/// Returns the number of shapes proven.
///
/// # Panics
///
/// Panics when any shape is accepted or rejected with the wrong
/// error — that is exactly the failure this proof exists to catch.
#[must_use]
pub fn fd_boundary_proof() -> usize {
    let limits = Limits::DEFAULT;
    let mut proven = 0;
    for index in [1u32, 2, 7, 31] {
        let msg = Message::new(1, 1).arg(Value::Fd(index));
        let bytes = msg.encode(&limits).expect("fd arg encodes");
        let err = decode(&bytes, index, &limits, ValidationMode::Tolerant)
            .expect_err("index == table size must fail");
        assert_eq!(err.wire_code(), Some(ErrorCode::FdMismatch));
        // And one slot larger is fine again.
        decode(&bytes, index + 1, &limits, ValidationMode::Tolerant)
            .expect("index < table size must pass");
        proven += 2;
    }
    proven
}

#[cfg(test)]
mod tests {
    use super::*;
    use ldp_protocol::REGISTRY;

    #[test]
    fn the_full_spec_walks_clean() {
        let report = run_registry_conformance(&REGISTRY, 0);
        assert!(
            report.is_clean(),
            "conformance failures:\n{:#?}",
            report.failures
        );
        // Coverage floors — the suite must actually walk the spec, not
        // trivially succeed on an empty registry.
        assert_eq!(report.modules, 9); // 8 v1 modules + capture (Phase 22)
        assert!(report.interfaces >= 33, "interfaces: {}", report.interfaces);
        assert!(
            report.operations >= 214, // 210 v1 ops + capture's three (Phase 22) + set_material (Phase 45)
            "operations: {}",
            report.operations
        );
        assert!(
            report.round_trips >= 600,
            "round trips: {}",
            report.round_trips
        );
        assert!(
            report.rejections >= 200,
            "rejections: {}",
            report.rejections
        );
    }

    #[test]
    fn fd_boundary_shapes_reject_at_the_decoder() {
        assert!(fd_boundary_proof() >= 8);
    }

    #[test]
    fn seed_does_not_change_the_exhaustive_walk() {
        let a = run_registry_conformance(&REGISTRY, 0);
        let b = run_registry_conformance(&REGISTRY, u64::MAX);
        assert_eq!(a, b);
    }
}
