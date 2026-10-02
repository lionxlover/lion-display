//! Built-in semantics for the two bootstrap interfaces.
//!
//! `ldp.core.connection` (handshake, sync, central lifecycle, keepalive,
//! registry bootstrap) and `ldp.core.registry` (bind, introspection) are
//! implemented by the session core itself — every other interface is a
//! [`crate::dispatch::Dispatcher`] concern. All handlers run *after*
//! stage-3 validation, so argument shapes are guaranteed; handlers only
//! decide state and semantics.

use ldp_core::error::{ErrorCode, LdpError, Result};
use ldp_core::ids::ObjectId;
use ldp_core::wire::Value;
use ldp_protocol::schema::OpSchema;
use ldp_protocol::Message;

use crate::audit::AuditRecord;
use crate::config::connection_options;
use crate::dispatch::{DispatchCtx, Dispatcher};
use crate::object::{ObjectEntry, ObjectKind};
use crate::schema_json;
use crate::session::{ClientSession, CONNECTION_INTERFACE, REGISTRY_INTERFACE};

/// Handle a request on a live connection object (the bootstrap object).
pub(crate) fn handle_connection_request(
    session: &mut ClientSession,
    msg: &Message,
    op: &OpSchema,
    fds: &mut ldp_transport::FdList,
    dispatcher: &mut dyn Dispatcher,
) -> Result<()> {
    match op.name {
        "hello" => handle_hello(session, msg),
        "sync" => session.send_event(
            ObjectId::CONNECTION,
            CONNECTION_INTERFACE,
            "sync_done",
            vec![msg.args[0].clone()],
        ),
        "ping" => session.send_event(
            ObjectId::CONNECTION,
            CONNECTION_INTERFACE,
            "pong",
            vec![msg.args[0].clone()],
        ),
        "destroy" => handle_destroy(session, msg, fds, dispatcher),
        "get_registry" => handle_get_registry(session, msg, fds, dispatcher),
        _ => Err(LdpError::Logic {
            what: "schema/connection drift: unknown connection request",
        }),
    }
}

/// `connection.hello(protocol_release, options)` — the handshake.
fn handle_hello(session: &mut ClientSession, msg: &Message) -> Result<()> {
    if session.handshake_done() {
        return Err(LdpError::protocol(
            ErrorCode::InvalidState,
            Some(ObjectId::CONNECTION),
            "connection.hello arrived twice",
        ));
    }
    let Value::Uint32(release) = msg.args[0] else {
        return Err(LdpError::Logic {
            what: "hello: argument 0 is not the uint32 the schema declares",
        });
    };
    let Value::Bitset(options) = msg.args[1] else {
        return Err(LdpError::Logic {
            what: "hello: argument 1 is not the bitset the schema declares",
        });
    };
    // The server may silently drop any requested option bit.
    let granted = options.intersect(session.config().options_allowed);
    if granted.test(connection_options::LARGE_MESSAGES) {
        session.enable_large_messages();
    }
    session.complete_handshake(release, granted);
    session.audit(AuditRecord::Handshake {
        client: session.client_id(),
        release,
        options: granted,
    });
    let sandbox = session.config().sandbox_verdict(session.creds());
    session.send_event(
        ObjectId::CONNECTION,
        CONNECTION_INTERFACE,
        "welcome",
        vec![
            Value::Uint32(session.config().protocol_release),
            Value::Bitset(session.config().caps),
            Value::Uint32(session.client_id().as_u32()),
            Value::String(String::new().into()),
            Value::Enum(sandbox.to_wire()),
        ],
    )
}

/// `connection.destroy(object_id, cookie)` — the one lifecycle path.
fn handle_destroy(
    session: &mut ClientSession,
    msg: &Message,
    fds: &mut ldp_transport::FdList,
    dispatcher: &mut dyn Dispatcher,
) -> Result<()> {
    let Value::Uint32(raw) = msg.args[0] else {
        return Err(LdpError::Logic {
            what: "destroy: argument 0 is not the uint32 the schema declares",
        });
    };
    let Value::Uint32(cookie) = msg.args[1] else {
        return Err(LdpError::Logic {
            what: "destroy: argument 1 is not the uint32 the schema declares",
        });
    };
    if raw == 0 {
        return Err(LdpError::protocol(
            ErrorCode::InvalidObject,
            None,
            "destroy: object ID 0 is not an object",
        ));
    }
    let id = ObjectId::from_wire(raw);
    let entry = match session.store().lookup(id) {
        crate::object::Lookup::NotFound => {
            return Err(LdpError::protocol(
                ErrorCode::InvalidObject,
                Some(id),
                "destroy: object ID was never bound on this connection",
            ));
        }
        crate::object::Lookup::Stale(_) => {
            return Err(LdpError::protocol(
                ErrorCode::InvalidState,
                Some(id),
                "destroy: object already destroyed (double destroy)",
            ));
        }
        crate::object::Lookup::Live(entry) => *entry,
    };
    if entry.kind == ObjectKind::Connection {
        return Err(LdpError::protocol(
            ErrorCode::InvalidState,
            Some(id),
            "destroy: the bootstrap connection object cannot be destroyed",
        ));
    }
    let removed = session.store_mut().remove(id)?;
    session.audit(AuditRecord::Destroy {
        client: session.client_id(),
        interface: removed.interface.into(),
        object: id,
    });
    session.send_event(
        ObjectId::CONNECTION,
        CONNECTION_INTERFACE,
        "destroyed",
        vec![Value::Uint32(raw), Value::Uint32(cookie)],
    )?;
    // The dispatcher's shadow resources for this object (a surface, a
    // pool, a buffer) go away now — the object is already dead in the
    // store, so the hook must not emit on it.
    let interface = removed.interface;
    let mut ctx = DispatchCtx::new(session, fds);
    dispatcher.on_destroy(&mut ctx, id, interface)
}

/// `connection.get_registry(version, new_id)` — the registry bootstrap.
fn handle_get_registry(
    session: &mut ClientSession,
    msg: &Message,
    fds: &mut ldp_transport::FdList,
    dispatcher: &mut dyn Dispatcher,
) -> Result<()> {
    let Value::Uint32(version) = msg.args[0] else {
        return Err(LdpError::Logic {
            what: "get_registry: argument 0 is not the uint32 the schema declares",
        });
    };
    let Value::NewId(new_id) = msg.args[1] else {
        return Err(LdpError::Logic {
            what: "get_registry: argument 1 is not the new_id the schema declares",
        });
    };
    let iface = ldp_protocol::REGISTRY
        .interface(REGISTRY_INTERFACE)
        .ok_or(LdpError::Logic {
            what: "registry interface missing from compiled schema",
        })?;
    if version < iface.version_min || version > iface.version_max {
        return Err(LdpError::protocol(
            ErrorCode::UnsupportedVersion,
            Some(new_id),
            format!(
                "get_registry: version {version} outside the advertised range {}..{}",
                iface.version_min, iface.version_max
            ),
        ));
    }
    if session.store().live_count() >= u64::from(session.limits().client_objects) {
        return Err(LdpError::Limit {
            kind: ldp_core::error::LimitKind::ClientObjects,
            value: session.store().live_count(),
        });
    }
    session.store_mut().insert_client(
        new_id,
        ObjectEntry {
            interface: iface.name,
            version,
            kind: ObjectKind::Registry,
        },
    )?;
    session.audit(AuditRecord::Bind {
        client: session.client_id(),
        interface: iface.name.into(),
        version,
        object: new_id,
    });
    // The global replay: one event per advertised global, on the new
    // object (the handshake diagram of `docs/protocol.md` §6). The
    // registry itself is replayed first (implicitly advertised).
    for global in session.config().replay_list() {
        let g = ldp_protocol::REGISTRY
            .interface(&global)
            .ok_or(LdpError::Logic {
                what: "advertised global missing from compiled schema",
            })?;
        session.send_event(
            new_id,
            REGISTRY_INTERFACE,
            "global",
            vec![
                Value::String(g.name.into()),
                Value::Uint32(g.version_min),
                Value::Uint32(g.version_max),
            ],
        )?;
    }
    // The dynamic-globals seam (Phase 44): the dispatcher learns the
    // live registry object — the target for every later
    // `global`/`global_remove` fan-out. The hook fires after the
    // replay, so anything the dispatcher queues rides *behind* the
    // advertisement burst on the same FIFO.
    let mut ctx = DispatchCtx::new(session, fds);
    dispatcher.on_registry(&mut ctx, new_id)
}

/// Handle a request on a live registry object.
pub(crate) fn handle_registry_request(
    session: &mut ClientSession,
    msg: &Message,
    op: &OpSchema,
    registry_object: ObjectId,
    fds: &mut ldp_transport::FdList,
    dispatcher: &mut dyn Dispatcher,
) -> Result<()> {
    match op.name {
        "bind" => handle_bind(session, msg, registry_object, fds, dispatcher),
        "introspect" => handle_introspect(session, msg, registry_object),
        _ => Err(LdpError::Logic {
            what: "schema/registry drift: unknown registry request",
        }),
    }
}

/// `registry.bind(interface, version, new_id)`.
fn handle_bind(
    session: &mut ClientSession,
    msg: &Message,
    registry_object: ObjectId,
    fds: &mut ldp_transport::FdList,
    dispatcher: &mut dyn Dispatcher,
) -> Result<()> {
    let Value::String(interface) = &msg.args[0] else {
        return Err(LdpError::Logic {
            what: "bind: argument 0 is not the string the schema declares",
        });
    };
    let interface: &str = interface;
    let Value::Uint32(version) = msg.args[1] else {
        return Err(LdpError::Logic {
            what: "bind: argument 1 is not the uint32 the schema declares",
        });
    };
    let Value::NewId(new_id) = msg.args[2] else {
        return Err(LdpError::Logic {
            what: "bind: argument 2 is not the new_id the schema declares",
        });
    };

    let Some(iface) = ldp_protocol::REGISTRY.interface(interface) else {
        return Err(LdpError::protocol(
            ErrorCode::InvalidInterface,
            Some(new_id),
            format!("bind: interface '{interface}' is not in the schema registry"),
        ));
    };
    let advertised = session.config().advertises(interface);
    if !advertised {
        return Err(LdpError::protocol(
            ErrorCode::InvalidInterface,
            Some(new_id),
            format!("bind: interface '{interface}' is not advertised by this server"),
        ));
    }
    if version < iface.version_min || version > iface.version_max {
        return Err(LdpError::protocol(
            ErrorCode::UnsupportedVersion,
            Some(new_id),
            format!(
                "bind: version {version} outside the advertised range {}..{} of '{interface}'",
                iface.version_min, iface.version_max
            ),
        ));
    }
    if session.store().live_count() >= u64::from(session.limits().client_objects) {
        return Err(LdpError::Limit {
            kind: ldp_core::error::LimitKind::ClientObjects,
            value: session.store().live_count(),
        });
    }
    let kind = if interface == REGISTRY_INTERFACE {
        ObjectKind::Registry
    } else {
        ObjectKind::Global
    };
    session.store_mut().insert_client(
        new_id,
        ObjectEntry {
            interface: iface.name,
            version,
            kind,
        },
    )?;
    session.audit(AuditRecord::Bind {
        client: session.client_id(),
        interface: iface.name.into(),
        version,
        object: new_id,
    });
    session.send_event(
        registry_object,
        REGISTRY_INTERFACE,
        "bound",
        vec![Value::String(interface.into()), Value::Uint32(version)],
    )?;
    // Globals with initial events (output cascades, shm formats) act
    // after the `bound` reply so the client observes bound-then-cascade.
    let name: &'static str = iface.name;
    let mut ctx = DispatchCtx::new(session, fds);
    dispatcher.on_bind(&mut ctx, new_id, name, version)
}

/// `registry.introspect(interface)` — requires the granted option; the
/// empty name streams one `schema` event per interface of the whole
/// protocol (each payload fits the string limit by construction).
fn handle_introspect(
    session: &mut ClientSession,
    msg: &Message,
    registry_object: ObjectId,
) -> Result<()> {
    let Value::String(interface) = &msg.args[0] else {
        return Err(LdpError::Logic {
            what: "introspect: argument 0 is not the string the schema declares",
        });
    };
    let interface: &str = interface;
    if !session.options().test(connection_options::INTROSPECTION) {
        session.audit(AuditRecord::Denied {
            client: session.client_id(),
            request: "registry.introspect",
        });
        return Err(LdpError::protocol(
            ErrorCode::Unauthorized,
            Some(registry_object),
            "introspect: the introspection option was not granted",
        ));
    }
    if interface.is_empty() {
        for (module, iface) in schema_json::all_interfaces() {
            let json = schema_json::render_interface(module, iface);
            session.send_event(
                registry_object,
                REGISTRY_INTERFACE,
                "schema",
                vec![Value::String(iface.name.into()), Value::String(json.into())],
            )?;
        }
        return Ok(());
    }
    let Some((module, iface)) = schema_json::all_interfaces().find(|(_, i)| i.name == interface)
    else {
        return Err(LdpError::protocol(
            ErrorCode::InvalidInterface,
            Some(registry_object),
            format!("introspect: interface '{interface}' is not in the schema registry"),
        ));
    };
    let json = schema_json::render_interface(module, iface);
    session.send_event(
        registry_object,
        REGISTRY_INTERFACE,
        "schema",
        vec![Value::String(iface.name.into()), Value::String(json.into())],
    )
}
