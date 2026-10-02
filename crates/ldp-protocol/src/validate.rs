//! Signature validation — stage 3 of `docs/protocol.md` §9.
//!
//! After a message is structurally decoded, [`check_signature`] proves it
//! matches its opcode's declared argument list for the bound interface
//! version: types, count, nullability, new_id ownership direction, and
//! (strict mode) enum/bitset domains. Only then does dispatch run
//! interface code (stage 4, the server's job).

use ldp_core::error::{ErrorCode, LdpError, Result};
use ldp_core::ids::ObjectId;
use ldp_core::wire::{ArgType, Value};

use crate::message::{Message, ValidationMode};
use crate::registry::SchemaRegistry;
use crate::schema::{arg_type_from_spec_name, Direction, InterfaceSchema, OpSchema};

/// Validate a decoded message against the schema and return the matched
/// operation signature.
///
/// `version` is the interface version the object was *bound* at
/// (negotiated in `[max(client_min, server_min), min(client_max,
/// server_max)]`); operations with `since > version` do not exist for
/// this object and are `invalid_opcode`.
///
/// # Errors
///
/// * [`ErrorCode::InvalidInterface`] — the interface name is unknown to
///   the registry,
/// * [`ErrorCode::InvalidOpcode`] — no such opcode for this direction and
///   version,
/// * [`ErrorCode::SignatureMismatch`] — argument count, types, or array
///   element types disagree,
/// * [`ErrorCode::InvalidObject`] — null object where the argument is not
///   nullable, or a new_id from the wrong ownership range,
/// * [`ErrorCode::OutOfRange`] — (strict mode) undeclared enum value or
///   bitset bits above the declared width,
/// * [`ErrorCode::FdMismatch`] — an fd index outside the message's table.
pub fn check_signature<'a>(
    registry: &'a SchemaRegistry<'_>,
    msg: &Message,
    interface: &str,
    dir: Direction,
    version: u32,
    mode: ValidationMode,
) -> Result<(&'a InterfaceSchema, &'a OpSchema)> {
    let Some(iface) = registry.interface(interface) else {
        return Err(LdpError::protocol(
            ErrorCode::InvalidInterface,
            None,
            format!("interface '{interface}' is not in the schema registry"),
        ));
    };
    let Some(op) = iface.find_op(dir, msg.opcode) else {
        return Err(LdpError::protocol(
            ErrorCode::InvalidOpcode,
            Some(ObjectId::from_wire(msg.object_id)),
            format!(
                "opcode {} is not a {} of '{}'",
                msg.opcode,
                dir.as_str(),
                interface
            ),
        ));
    };
    if op.since > version {
        return Err(LdpError::protocol(
            ErrorCode::InvalidOpcode,
            Some(ObjectId::from_wire(msg.object_id)),
            format!(
                "opcode '{}' requires version >= {}, object bound at {}",
                op.name, op.since, version
            ),
        ));
    }
    let module = registry
        .module_of_interface(interface)
        .ok_or(LdpError::Logic {
            what: "registry module_of_interface disagreed with interface lookup",
        })?;
    check_args(registry, msg, module.name, op, dir, mode)?;
    Ok((iface, op))
}

fn check_args(
    registry: &SchemaRegistry<'_>,
    msg: &Message,
    module: &str,
    op: &OpSchema,
    dir: Direction,
    mode: ValidationMode,
) -> Result<()> {
    if msg.args.len() != op.args.len() {
        return Err(LdpError::protocol(
            ErrorCode::SignatureMismatch,
            None,
            format!(
                "'{}' takes {} argument(s), message carries {}",
                op.name,
                op.args.len(),
                msg.args.len()
            ),
        ));
    }
    for (v, a) in msg.args.iter().zip(op.args.iter()) {
        check_one_arg(
            &ArgCtx {
                registry,
                module,
                op_name: op.name,
                dir,
                mode,
                fd_count: msg.required_fd_count(),
            },
            v,
            a,
        )?;
    }
    Ok(())
}

/// Per-message validation context shared by every argument check.
struct ArgCtx<'a> {
    registry: &'a SchemaRegistry<'a>,
    module: &'a str,
    op_name: &'a str,
    dir: Direction,
    mode: ValidationMode,
    fd_count: u32,
}

fn check_one_arg(ctx: &ArgCtx<'_>, v: &Value, a: &crate::schema::ArgSchema) -> Result<()> {
    let where_ = format!("{} arg '{}'", ctx.op_name, a.name);
    if v.arg_type() != a.ty {
        return Err(LdpError::protocol(
            ErrorCode::SignatureMismatch,
            None,
            format!(
                "{where_}: expected {}, found {}",
                a.ty.as_str(),
                v.arg_type().as_str()
            ),
        ));
    }
    match v {
        // `v.arg_type() == a.ty` above means each arm only fires for the
        // matching schema type.
        Value::Object(None) if !a.nullable => Err(LdpError::protocol(
            ErrorCode::InvalidObject,
            None,
            format!("{where_}: null object where the argument is not nullable"),
        )),
        Value::NewId(id) => check_new_id(*id, ctx.dir, &where_),
        Value::Enum(raw) if ctx.mode == ValidationMode::Strict => {
            check_enum_domain(*raw, a, ctx.registry, ctx.module, &where_)
        }
        Value::Bitset(set) if ctx.mode == ValidationMode::Strict => {
            check_bitset_domain(*set, a, ctx.registry, ctx.module, &where_)
        }
        Value::Array { element, .. } => check_array_element(*element, a, &where_),
        Value::Fd(index) if *index >= ctx.fd_count => Err(LdpError::malformed(
            ErrorCode::FdMismatch,
            format!("{where_}: fd index {index} outside the message's table"),
        )),
        _ => Ok(()),
    }
}

fn check_new_id(id: ObjectId, dir: Direction, where_: &str) -> Result<()> {
    let ok = match dir {
        Direction::Request => ObjectId::client(id.as_u32()).is_some(),
        Direction::Event => ObjectId::server(id.as_u32()).is_some(),
    };
    if ok {
        Ok(())
    } else {
        Err(LdpError::protocol(
            ErrorCode::InvalidObject,
            None,
            format!(
                "{where_}: new_id {:#010x} is outside the allowed range for {}s",
                id.as_u32(),
                dir.as_str()
            ),
        ))
    }
}

fn check_enum_domain(
    raw: u32,
    a: &crate::schema::ArgSchema,
    registry: &SchemaRegistry<'_>,
    module: &str,
    where_: &str,
) -> Result<()> {
    let Some(of) = a.of else {
        return Err(LdpError::Logic {
            what: "schema: enum argument without 'of'",
        });
    };
    let Some(def) = registry.resolve_enum(module, of) else {
        return Err(LdpError::Logic {
            what: "schema: enum 'of' does not resolve",
        });
    };
    if def.contains(raw) {
        Ok(())
    } else {
        Err(LdpError::protocol(
            ErrorCode::OutOfRange,
            None,
            format!("{where_}: enum value {raw} is not declared"),
        ))
    }
}

fn check_bitset_domain(
    set: ldp_core::bitset::Bitset128,
    a: &crate::schema::ArgSchema,
    registry: &SchemaRegistry<'_>,
    module: &str,
    where_: &str,
) -> Result<()> {
    let Some(of) = a.of else {
        return Err(LdpError::Logic {
            what: "schema: bitset argument without 'of'",
        });
    };
    let Some(def) = registry.resolve_bitset(module, of) else {
        return Err(LdpError::Logic {
            what: "schema: bitset 'of' does not resolve",
        });
    };
    if set == set.masked_to(def.width()) {
        Ok(())
    } else {
        Err(LdpError::protocol(
            ErrorCode::OutOfRange,
            None,
            format!(
                "{where_}: bits at or above width {} are undeclared",
                def.width()
            ),
        ))
    }
}

fn check_array_element(element: ArgType, a: &crate::schema::ArgSchema, where_: &str) -> Result<()> {
    let Some(of) = a.of else {
        return Err(LdpError::Logic {
            what: "schema: array argument without 'of'",
        });
    };
    let Some(want) = arg_type_from_spec_name(of) else {
        return Err(LdpError::Logic {
            what: "schema: array 'of' is not a scalar or rect",
        });
    };
    if element == want {
        Ok(())
    } else {
        Err(LdpError::protocol(
            ErrorCode::SignatureMismatch,
            None,
            format!(
                "{where_}: array element type {} does not match declared {}",
                element.as_str(),
                want.as_str()
            ),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::REGISTRY;
    use ldp_core::bitset::Bitset128;

    fn hello_msg() -> Message {
        Message::new(1, 1)
            .arg(Value::Uint32(1))
            .arg(Value::Bitset(Bitset128::EMPTY))
    }

    #[test]
    fn hello_signature_matches() {
        let m = hello_msg();
        let (iface, op) = check_signature(
            &REGISTRY,
            &m,
            "ldp.core.connection",
            Direction::Request,
            1,
            ValidationMode::Strict,
        )
        .unwrap();
        assert_eq!(iface.name, "ldp.core.connection");
        assert_eq!(op.name, "hello");
    }

    #[test]
    fn unknown_opcode_and_interface() {
        let m = Message::new(1, 999);
        let err = check_signature(
            &REGISTRY,
            &m,
            "ldp.core.connection",
            Direction::Request,
            1,
            ValidationMode::Tolerant,
        )
        .unwrap_err();
        assert_eq!(err.wire_code(), Some(ErrorCode::InvalidOpcode));
        let err = check_signature(
            &REGISTRY,
            &m,
            "ldp.nope.thing",
            Direction::Request,
            1,
            ValidationMode::Tolerant,
        )
        .unwrap_err();
        assert_eq!(err.wire_code(), Some(ErrorCode::InvalidInterface));
    }

    #[test]
    fn since_gating_rejects_below_bound_version() {
        let m = hello_msg();
        // Bound below the operation's since (v1): rejected.
        let err = check_signature(
            &REGISTRY,
            &m,
            "ldp.core.connection",
            Direction::Request,
            0,
            ValidationMode::Tolerant,
        )
        .unwrap_err();
        assert_eq!(err.wire_code(), Some(ErrorCode::InvalidOpcode));
    }

    #[test]
    fn count_and_type_mismatches() {
        let too_few = Message::new(1, 1);
        let err = check_signature(
            &REGISTRY,
            &too_few,
            "ldp.core.connection",
            Direction::Request,
            1,
            ValidationMode::Tolerant,
        )
        .unwrap_err();
        assert_eq!(err.wire_code(), Some(ErrorCode::SignatureMismatch));

        let wrong_type = Message::new(1, 1)
            .arg(Value::Int32(1))
            .arg(Value::Bitset(Bitset128::EMPTY));
        let err = check_signature(
            &REGISTRY,
            &wrong_type,
            "ldp.core.connection",
            Direction::Request,
            1,
            ValidationMode::Tolerant,
        )
        .unwrap_err();
        assert_eq!(err.wire_code(), Some(ErrorCode::SignatureMismatch));
    }

    #[test]
    fn enum_domain_strict_vs_tolerant() {
        // connection.error (event opcode 5) carries `code: enum<error_code>`.
        let unknown = Message::new(1, 5)
            .arg(Value::Enum(999))
            .arg(Value::Uint32(0))
            .arg(Value::String("x".into()));
        let err = check_signature(
            &REGISTRY,
            &unknown,
            "ldp.core.connection",
            Direction::Event,
            1,
            ValidationMode::Strict,
        )
        .unwrap_err();
        assert_eq!(err.wire_code(), Some(ErrorCode::OutOfRange));
        assert!(check_signature(
            &REGISTRY,
            &unknown,
            "ldp.core.connection",
            Direction::Event,
            1,
            ValidationMode::Tolerant
        )
        .is_ok());
    }

    #[test]
    fn bitset_width_strict_vs_tolerant() {
        // hello.options is bitset<connection_options>.
        let undeclared = Message::new(1, 1)
            .arg(Value::Uint32(1))
            .arg(Value::Bitset(Bitset128::single(127)));
        let err = check_signature(
            &REGISTRY,
            &undeclared,
            "ldp.core.connection",
            Direction::Request,
            1,
            ValidationMode::Strict,
        )
        .unwrap_err();
        assert_eq!(err.wire_code(), Some(ErrorCode::OutOfRange));
        assert!(check_signature(
            &REGISTRY,
            &undeclared,
            "ldp.core.connection",
            Direction::Request,
            1,
            ValidationMode::Tolerant
        )
        .is_ok());
    }

    #[test]
    fn new_id_ownership_direction() {
        // registry.bind (request opcode 1) takes a client-chosen new_id.
        let client = Message::new(2, 1)
            .arg(Value::String("ldp.core.output".into()))
            .arg(Value::Uint32(1))
            .arg(Value::NewId(ObjectId::client(9).unwrap()));
        assert!(check_signature(
            &REGISTRY,
            &client,
            "ldp.core.registry",
            Direction::Request,
            1,
            ValidationMode::Strict
        )
        .is_ok());
        // Server-range bits in a request new_id: rejected.
        let server = Message::new(2, 1)
            .arg(Value::String("ldp.core.output".into()))
            .arg(Value::Uint32(1))
            .arg(Value::NewId(ObjectId::server(0x8000_0009).unwrap()));
        let err = check_signature(
            &REGISTRY,
            &server,
            "ldp.core.registry",
            Direction::Request,
            1,
            ValidationMode::Tolerant,
        )
        .unwrap_err();
        assert_eq!(err.wire_code(), Some(ErrorCode::InvalidObject));
        // Zero new_id: rejected in both directions.
        let zero = Message::new(2, 1)
            .arg(Value::String("ldp.core.output".into()))
            .arg(Value::Uint32(1))
            .arg(Value::NewId(ObjectId::from_wire(0)));
        let err = check_signature(
            &REGISTRY,
            &zero,
            "ldp.core.registry",
            Direction::Request,
            1,
            ValidationMode::Tolerant,
        )
        .unwrap_err();
        assert_eq!(err.wire_code(), Some(ErrorCode::InvalidObject));
    }

    #[test]
    fn null_object_only_when_nullable() {
        // surface.attach (request opcode 1) takes a nullable buffer.
        let null = Message::new(3, 1).arg(Value::Object(None));
        assert!(check_signature(
            &REGISTRY,
            &null,
            "ldp.core.surface",
            Direction::Request,
            1,
            ValidationMode::Tolerant
        )
        .is_ok());
        // compositor.create_subsurface (request opcode 2) requires a
        // non-nullable `surface`.
        let bad = Message::new(3, 2)
            .arg(Value::Object(None))
            .arg(Value::Object(Some(ObjectId::client(5).unwrap())))
            .arg(Value::NewId(ObjectId::client(6).unwrap()));
        let err = check_signature(
            &REGISTRY,
            &bad,
            "ldp.core.compositor",
            Direction::Request,
            1,
            ValidationMode::Tolerant,
        )
        .unwrap_err();
        assert_eq!(err.wire_code(), Some(ErrorCode::InvalidObject));
    }
}
