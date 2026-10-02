//! The decode-driven object table both relay sides maintain.
//!
//! The relay is transparent at the byte level — messages forward raw
//! and unmodified — but descriptors are *state*, not bytes, and state
//! needs interpretation. This module supplies exactly as much protocol
//! understanding as the FD relay vocabulary requires, resolved against
//! the same [`ldp_protocol`] schema registry the server itself uses:
//!
//! * the **object table**: raw wire id → interface, learned from every
//!   `new_id` argument the connection ever carries (both directions —
//!   client-chosen ids and the one server-chosen id in
//!   `data_device.data_offer`), seeded with the connection bootstrap
//!   object (id 1),
//! * the **pool/buffer geometry**: `shm.create_pool` registers a pool,
//!   `shm_pool.create_buffer` carves an `(offset, stride × height)`
//!   window, `surface.attach` + `surface.commit` select the window
//!   whose bytes must be fresh when the commit lands,
//! * the **FD dispositions**: which descriptor class each
//!   descriptor-bearing operation carries — pools, snapshots (ICC
//!   profiles, keymaps), streams (clipboard pipes), fences (eventfd
//!   release fences), and the explicitly unsupported GPU classes.
//!
//! # Posture
//!
//! Interpretation failures are never silent. A message the tracker
//! cannot classify but that carries descriptors yields
//! [`FdDisposition::Unsupported`] — the session ends with a diagnostic
//! — because relaying such a message *without* its descriptor state
//! would deliver silently-wrong pixels. A tracker that lags the
//! protocol it forwards is a bug, and the relay makes it visible.

#![forbid(unsafe_code)]

use std::collections::HashMap;

use ldp_core::error::{ErrorCode, LdpError, Result};
use ldp_core::wire::Value;
use ldp_protocol::{schema::OpSchema, Direction, Message, REGISTRY};

/// The bootstrap object every connection owns (`docs/protocol.md` §6).
pub const CONNECTION_OBJECT: u32 = 1;

/// How one descriptor-bearing message's FDs must be relayed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FdSemantics {
    /// `shm.create_pool`: ship the whole pool content now; track it
    /// for per-commit updates. `size` is the pool's declared byte
    /// size.
    Pool {
        /// Relay-assigned id, stable for the pool's lifetime.
        relay_id: u32,
        /// Declared pool size (bytes).
        size: u64,
    },
    /// Read-once file content (ICC profile, keymap): ship the whole
    /// content now; no later mutation tracking.
    Snapshot {
        /// Relay-assigned id.
        relay_id: u32,
    },
    /// A pipe (clipboard-class transfer): substitute a local pipe on
    /// the far side and stream the bytes as they flow.
    Stream {
        /// Relay-assigned id.
        relay_id: u32,
    },
    /// An eventfd fence (`buffer.release`): ship the counter snapshot.
    Fence,
}

/// What the tracker concluded about a message's descriptors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FdDisposition {
    /// The message carries no descriptors the relay must interpret.
    None,
    /// Per-referenced-descriptor semantics, ascending fd index.
    Interpret(Vec<FdSemantics>),
    /// A descriptor class the remote layer does not relay; the session
    /// must end with a diagnostic (never a silent wrong delivery).
    Unsupported(&'static str),
}

/// State the relay must ship *before* forwarding this message.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ShipBefore {
    /// Nothing — forward as-is.
    Nothing,
    /// A commit references an attached buffer: its pool window must be
    /// fresh before the commit message lands.
    PoolUpdate {
        /// The pool's relay id.
        relay_id: u32,
        /// Window offset in the pool (bytes).
        offset: u64,
        /// Window length (bytes).
        len: u64,
    },
    /// A pool grew: the far side must `ftruncate` before the resize
    /// message lands.
    PoolResize {
        /// The pool's relay id.
        relay_id: u32,
        /// The new size (bytes; pools never shrink).
        new_size: u64,
    },
}

/// What the tracker planned for one observed message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    /// Descriptor handling.
    pub disposition: FdDisposition,
    /// Pre-ship state (pool updates feeding a commit, pool growth).
    pub ship_before: ShipBefore,
}

impl Plan {
    /// A plan that forwards without interpretation.
    #[must_use]
    pub fn opaque() -> Plan {
        Plan {
            disposition: FdDisposition::None,
            ship_before: ShipBefore::Nothing,
        }
    }
}

/// One tracked shm pool.
#[derive(Clone, Copy, Debug)]
struct PoolInfo {
    relay_id: u32,
    size: u64,
}

/// One tracked buffer window into a pool.
#[derive(Clone, Copy, Debug)]
struct BufferInfo {
    pool: u32,
    offset: u64,
    len: u64,
}

/// The object table: interfaces, pools, buffers, and surface attaches,
/// learned from the live message stream.
#[derive(Debug)]
pub struct Tracker {
    interfaces: HashMap<u32, &'static str>,
    pools: HashMap<u32, PoolInfo>,
    buffers: HashMap<u32, BufferInfo>,
    attached: HashMap<u32, u32>,
    next_relay: u32,
}

impl Default for Tracker {
    fn default() -> Self {
        Self::new()
    }
}

impl Tracker {
    /// A fresh tracker: the connection bootstrap object pre-registered.
    #[must_use]
    pub fn new() -> Tracker {
        let connection = REGISTRY
            .interface("ldp.core.connection")
            .map(|i| i.name)
            .unwrap_or("ldp.core.connection");
        let mut interfaces = HashMap::new();
        interfaces.insert(CONNECTION_OBJECT, connection);
        Tracker {
            interfaces,
            pools: HashMap::new(),
            buffers: HashMap::new(),
            attached: HashMap::new(),
            next_relay: 1,
        }
    }

    /// The interface a raw wire id resolved to, if any.
    #[must_use]
    pub fn interface_of(&self, raw: u32) -> Option<&'static str> {
        self.interfaces.get(&raw).copied()
    }

    /// The relay id a pool object id resolved to, if any.
    #[must_use]
    pub fn pool_relay_id(&self, pool_object: u32) -> Option<u32> {
        self.pools.get(&pool_object).map(|p| p.relay_id)
    }

    /// The tracked size of a pool (bytes), if any.
    #[must_use]
    pub fn pool_size(&self, pool_object: u32) -> Option<u64> {
        self.pools.get(&pool_object).map(|p| p.size)
    }

    /// Observe one decoded message; update the table and plan the
    /// descriptor work.
    ///
    /// # Errors
    /// [`LdpError::Malformed`] for hard invariant violations — a
    /// `surface.commit` whose attached buffer resolves to no tracked
    /// pool would relay silently-stale pixels, so it is refused; the
    /// caller ends the session. Everything merely *unclassifiable*
    /// returns [`FdDisposition::Unsupported`] instead (also fatal for
    /// the session, but diagnosable).
    pub fn observe(&mut self, msg: &Message, dir: Direction) -> Result<Plan> {
        let Some(interface) = self.interfaces.get(&msg.object_id).copied() else {
            return Ok(self.plan_or_unsupported(msg, "unknown target object"));
        };
        let Some((_schema, op)) = REGISTRY.find_op(interface, dir, msg.opcode) else {
            return Ok(self.plan_or_unsupported(msg, "unknown opcode for interface"));
        };
        // Signature guard: interpret positionally only when the arg
        // count matches the schema — the server's stage-3 check rejects
        // anything else; the relay refuses to guess.
        if msg.args.len() != op.args.len() {
            return Ok(self.plan_or_unsupported(msg, "signature mismatch"));
        }
        self.register_new_ids(interface, op, msg);
        self.interpret(interface, op.name, dir, msg)
    }

    /// Handle a message the tracker cannot resolve: opaque if it
    /// carries no descriptors, unsupported otherwise.
    fn plan_or_unsupported(&self, msg: &Message, what: &'static str) -> Plan {
        if fd_indices(msg).is_empty() {
            Plan::opaque()
        } else {
            Plan {
                disposition: FdDisposition::Unsupported(what),
                ship_before: ShipBefore::Nothing,
            }
        }
    }

    /// Record every object the message creates (both directions):
    /// `new_id` arguments per the schema, `registry.bind` dynamic.
    fn register_new_ids(&mut self, interface: &'static str, op: &OpSchema, msg: &Message) {
        let dynamic_bind = interface == "ldp.core.registry" && op.name == "bind";
        for (schema_arg, value) in op.args.iter().zip(&msg.args) {
            if schema_arg.ty != ldp_core::wire::ArgType::NewId {
                continue;
            }
            let Value::NewId(id) = value else { continue };
            let fq: Option<String> = if dynamic_bind {
                msg.args.iter().find_map(|a| match a {
                    Value::String(s) => Some(s.to_string()),
                    _ => None,
                })
            } else {
                schema_arg.of.map(|short| {
                    if short.contains('.') {
                        short.to_string()
                    } else {
                        format!(
                            "{}.{short}",
                            REGISTRY
                                .module_of_interface(interface)
                                .map(|m| m.name)
                                .unwrap_or_default()
                        )
                    }
                })
            };
            if let Some(fq) = fq {
                if let Some(name) = REGISTRY.interface(&fq).map(|i| i.name) {
                    self.interfaces.insert(id.as_u32(), name);
                }
            }
        }
    }

    /// The per-operation interpretation table. Every arm that returns
    /// `FdDisposition::Interpret` must cover exactly the message's
    /// referenced descriptor indices (the relay asserts this).
    fn interpret(
        &mut self,
        interface: &'static str,
        op: &str,
        dir: Direction,
        msg: &Message,
    ) -> Result<Plan> {
        let ship_before = match (interface, op, dir) {
            ("ldp.core.shm", "create_pool", Direction::Request) => {
                let size = int_arg(&msg.args[1]).unwrap_or(0).max(0) as u64;
                let relay_id = self.allocate_relay();
                if let Some(Value::NewId(id)) = msg.args.get(2) {
                    self.pools.insert(id.as_u32(), PoolInfo { relay_id, size });
                }
                return Ok(Plan {
                    disposition: FdDisposition::Interpret(vec![FdSemantics::Pool {
                        relay_id,
                        size,
                    }]),
                    ship_before: ShipBefore::Nothing,
                });
            }
            ("ldp.core.shm_pool", "resize", Direction::Request) => {
                let new_size = int_arg(&msg.args[0]).unwrap_or(0).max(0) as u64;
                let Some(pool) = self.pools.get(&msg.object_id) else {
                    return Err(self.invariant("resize of an untracked pool"));
                };
                let relay_id = pool.relay_id;
                let grown = pool.size.max(new_size);
                if let Some(pool) = self.pools.get_mut(&msg.object_id) {
                    pool.size = grown;
                }
                ShipBefore::PoolResize {
                    relay_id,
                    new_size: grown,
                }
            }
            ("ldp.core.shm_pool", "create_buffer", Direction::Request) => {
                let offset = int_arg(&msg.args[0]).unwrap_or(0).max(0) as u64;
                let height = int_arg(&msg.args[2]).unwrap_or(0).max(0) as u64;
                let stride = int_arg(&msg.args[3]).unwrap_or(0).max(0) as u64;
                if let Some(Value::NewId(id)) = msg.args.get(5) {
                    self.buffers.insert(
                        id.as_u32(),
                        BufferInfo {
                            pool: msg.object_id,
                            offset,
                            len: stride.saturating_mul(height),
                        },
                    );
                }
                ShipBefore::Nothing
            }
            ("ldp.core.surface", "attach", Direction::Request) => {
                match msg.args.first() {
                    Some(Value::Object(Some(buffer))) => {
                        self.attached.insert(msg.object_id, buffer.as_u32());
                    }
                    _ => {
                        self.attached.remove(&msg.object_id);
                    }
                }
                ShipBefore::Nothing
            }
            ("ldp.core.surface", "commit", Direction::Request) => {
                return self.plan_commit(msg);
            }
            ("ldp.core.connection", "destroy", Direction::Request) => {
                if let Some(Value::Uint32(raw)) = msg.args.first() {
                    self.forget(*raw);
                }
                ShipBefore::Nothing
            }
            ("ldp.color.color_manager", "import_icc", Direction::Request)
            | ("ldp.input.keyboard", "keymap", Direction::Event)
            | ("ldp.capture.capture_manager", "frame", Direction::Event) => {
                let relay_id = self.allocate_relay();
                return Ok(Plan {
                    disposition: FdDisposition::Interpret(vec![FdSemantics::Snapshot { relay_id }]),
                    ship_before: ShipBefore::Nothing,
                });
            }
            ("ldp.data.data_offer", "receive", Direction::Request)
            | ("ldp.data.data_source", "send", Direction::Event) => {
                let relay_id = self.allocate_relay();
                return Ok(Plan {
                    disposition: FdDisposition::Interpret(vec![FdSemantics::Stream { relay_id }]),
                    ship_before: ShipBefore::Nothing,
                });
            }
            ("ldp.core.buffer", "release", Direction::Event) => {
                return Ok(Plan {
                    disposition: FdDisposition::Interpret(vec![FdSemantics::Fence]),
                    ship_before: ShipBefore::Nothing,
                });
            }
            ("ldp.core.dmabuf", "create", Direction::Request) => {
                return Ok(Plan {
                    disposition: FdDisposition::Unsupported(
                        "dmabuf descriptors (GPU) are not relayable",
                    ),
                    ship_before: ShipBefore::Nothing,
                });
            }
            ("ldp.core.fence", "import_sync_file", Direction::Request)
            | ("ldp.core.fence", "import_syncobj", Direction::Request) => {
                return Ok(Plan {
                    disposition: FdDisposition::Unsupported(
                        "explicit-sync descriptors are not relayable",
                    ),
                    ship_before: ShipBefore::Nothing,
                });
            }
            _ => ShipBefore::Nothing,
        };
        Ok(Plan {
            disposition: self.plan_or_unsupported_msg(msg),
            ship_before,
        })
    }

    /// `surface.commit`: the attached buffer's window must be fresh
    /// when the commit lands. A commit whose attach the tracker saw
    /// but cannot resolve to a pool is a hard invariant violation
    /// (silently-stale pixels otherwise).
    fn plan_commit(&mut self, msg: &Message) -> Result<Plan> {
        let Some(&buffer) = self.attached.get(&msg.object_id) else {
            return Ok(Plan::opaque());
        };
        let Some(info) = self.buffers.get(&buffer) else {
            // An attach of an object the tracker never saw created:
            // impossible on a healthy stream (every buffer comes from a
            // create_buffer this relay forwarded).
            return Err(
                self.invariant("commit references an attached buffer that was never tracked")
            );
        };
        let Some(pool) = self.pools.get(&info.pool) else {
            return Err(self.invariant("commit references a buffer whose pool is gone"));
        };
        let relay_id = pool.relay_id;
        let (offset, len) = (info.offset, info.len);
        Ok(Plan {
            disposition: FdDisposition::None,
            ship_before: ShipBefore::PoolUpdate {
                relay_id,
                offset,
                len,
            },
        })
    }

    /// Forget a destroyed object across every table.
    fn forget(&mut self, raw: u32) {
        self.interfaces.remove(&raw);
        self.pools.remove(&raw);
        self.buffers.remove(&raw);
        self.attached.remove(&raw);
        self.attached.retain(|_, &mut b| b != raw);
    }

    /// Allocate the next relay id (1-based; 0 is never assigned).
    fn allocate_relay(&mut self) -> u32 {
        let id = self.next_relay;
        self.next_relay += 1;
        id
    }

    fn invariant(&self, what: &'static str) -> LdpError {
        LdpError::malformed(
            ErrorCode::MalformedMessage,
            format!("ldp-remote tracker: {what}"),
        )
    }

    fn plan_or_unsupported_msg(&self, msg: &Message) -> FdDisposition {
        if fd_indices(msg).is_empty() {
            FdDisposition::None
        } else {
            FdDisposition::Unsupported("descriptors on an uninterpreted operation")
        }
    }
}

/// The fd indices a message references, ascending and deduplicated.
fn fd_indices(msg: &Message) -> Vec<u32> {
    let mut out: Vec<u32> = msg
        .args
        .iter()
        .filter_map(|a| match a {
            Value::Fd(i) => Some(*i),
            _ => None,
        })
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Read an integer argument across width variants.
fn int_arg(v: &Value) -> Option<i64> {
    match v {
        Value::Int32(x) => Some(i64::from(*x)),
        Value::Uint32(x) => Some(i64::from(*x)),
        Value::Int64(x) => Some(*x),
        Value::Uint64(x) if *x <= i64::MAX as u64 => Some(*x as i64),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ldp_core::ids::ObjectId;

    fn msg(object: u32, opcode: u32, args: Vec<Value>) -> Message {
        let mut m = Message::new(object, opcode);
        m.args = args;
        m
    }

    /// Resolve an opcode by interface + operation *name* (robust to
    /// declaration-order changes; the numbers are frozen on the wire,
    /// but tests should not hardcode them).
    fn op_by_name(iface: &str, dir: Direction, name: &str) -> u32 {
        let schema = REGISTRY.interface(iface).expect("interface");
        let ops = match dir {
            Direction::Request => schema.requests,
            Direction::Event => schema.events,
        };
        ops.iter()
            .find(|op| op.name == name)
            .unwrap_or_else(|| panic!("{iface} has no {dir:?} op '{name}'"))
            .opcode
    }

    fn id(n: u32) -> ObjectId {
        ObjectId::client(n).expect("client id")
    }

    /// hello → get_registry → bind(shm) → create_pool, with the opcode
    /// numbers resolved from the registry (the spec's declaration
    /// order; frozen once released).
    fn bootstrap_pool(tracker: &mut Tracker, pool_object: u32) -> u32 {
        // connection.get_registry
        let opcode = op_by_name("ldp.core.connection", Direction::Request, "get_registry");
        let registry_object = 9;
        tracker
            .observe(
                &msg(
                    CONNECTION_OBJECT,
                    opcode,
                    vec![Value::Uint32(1), Value::NewId(id(registry_object))],
                ),
                Direction::Request,
            )
            .expect("get_registry");
        assert_eq!(
            tracker.interface_of(registry_object),
            Some("ldp.core.registry")
        );

        // registry.bind("ldp.core.shm")
        let bind_opcode = op_by_name("ldp.core.registry", Direction::Request, "bind");
        tracker
            .observe(
                &msg(
                    registry_object,
                    bind_opcode,
                    vec![
                        Value::String("ldp.core.shm".into()),
                        Value::Uint32(1),
                        Value::NewId(id(10)),
                    ],
                ),
                Direction::Request,
            )
            .expect("bind");
        assert_eq!(tracker.interface_of(10), Some("ldp.core.shm"));

        // shm.create_pool(fd, size, new_id)
        let pool_opcode = op_by_name("ldp.core.shm", Direction::Request, "create_pool");
        let plan = tracker
            .observe(
                &msg(
                    10,
                    pool_opcode,
                    vec![
                        Value::Fd(0),
                        Value::Int64(4096),
                        Value::NewId(id(pool_object)),
                    ],
                ),
                Direction::Request,
            )
            .expect("create_pool");
        match plan.disposition {
            FdDisposition::Interpret(ref semantics) => match semantics[0] {
                FdSemantics::Pool { relay_id, size } => {
                    assert_eq!(size, 4096);
                    assert_eq!(tracker.pool_relay_id(pool_object), Some(relay_id));
                    relay_id
                }
                _ => panic!("expected pool semantics"),
            },
            other => panic!("expected interpret, got {other:?}"),
        }
    }

    #[test]
    fn bootstrap_resolves_dynamic_and_schema_new_ids() {
        let mut tracker = Tracker::new();
        assert_eq!(
            tracker.interface_of(CONNECTION_OBJECT),
            Some("ldp.core.connection")
        );
        bootstrap_pool(&mut tracker, 11);
        assert_eq!(tracker.interface_of(11), Some("ldp.core.shm_pool"));
    }

    #[test]
    fn buffer_geometry_and_commit_plan() {
        let mut tracker = Tracker::new();
        bootstrap_pool(&mut tracker, 11);

        // shm_pool.create_buffer(offset, w, h, stride, format, id)
        let buffer_opcode = op_by_name("ldp.core.shm_pool", Direction::Request, "create_buffer");
        tracker
            .observe(
                &msg(
                    11,
                    buffer_opcode,
                    vec![
                        Value::Int32(64),
                        Value::Int32(2),
                        Value::Int32(2),
                        Value::Int32(8),
                        Value::Uint32(0x3432_5258),
                        Value::NewId(id(12)),
                    ],
                ),
                Direction::Request,
            )
            .expect("create_buffer");
        assert_eq!(tracker.interface_of(12), Some("ldp.core.buffer"));

        // surface attach + commit via a bound surface
        let bind_opcode = op_by_name("ldp.core.registry", Direction::Request, "bind");
        tracker
            .observe(
                &msg(
                    9,
                    bind_opcode,
                    vec![
                        Value::String("ldp.core.compositor".into()),
                        Value::Uint32(1),
                        Value::NewId(id(20)),
                    ],
                ),
                Direction::Request,
            )
            .expect("bind compositor");
        let create_surface =
            op_by_name("ldp.core.compositor", Direction::Request, "create_surface");
        tracker
            .observe(
                &msg(20, create_surface, vec![Value::NewId(id(21))]),
                Direction::Request,
            )
            .expect("create_surface");
        assert_eq!(tracker.interface_of(21), Some("ldp.core.surface"));

        let attach = op_by_name("ldp.core.surface", Direction::Request, "attach");
        tracker
            .observe(
                &msg(21, attach, vec![Value::Object(Some(id(12)))]),
                Direction::Request,
            )
            .expect("attach");

        let commit = op_by_name("ldp.core.surface", Direction::Request, "commit");
        let plan = tracker
            .observe(
                &msg(21, commit, vec![Value::Uint32(0xCAFE)]),
                Direction::Request,
            )
            .expect("commit");
        assert_eq!(
            plan.ship_before,
            ShipBefore::PoolUpdate {
                relay_id: 1,
                offset: 64,
                len: 16,
            }
        );
    }

    #[test]
    fn resize_plans_growth_and_never_shrinks() {
        let mut tracker = Tracker::new();
        let relay_id = bootstrap_pool(&mut tracker, 11);
        let resize = op_by_name("ldp.core.shm_pool", Direction::Request, "resize");
        let plan = tracker
            .observe(
                &msg(11, resize, vec![Value::Int64(8192)]),
                Direction::Request,
            )
            .expect("resize");
        assert_eq!(
            plan.ship_before,
            ShipBefore::PoolResize {
                relay_id,
                new_size: 8192,
            }
        );
        assert_eq!(tracker.pool_size(11), Some(8192));

        // A "shrink" attempt keeps the tracked maximum.
        let plan = tracker
            .observe(
                &msg(11, resize, vec![Value::Int64(1024)]),
                Direction::Request,
            )
            .expect("resize again");
        assert_eq!(
            plan.ship_before,
            ShipBefore::PoolResize {
                relay_id,
                new_size: 8192,
            }
        );
    }

    #[test]
    fn destroy_forgets_across_tables() {
        let mut tracker = Tracker::new();
        bootstrap_pool(&mut tracker, 11);
        let destroy = op_by_name("ldp.core.connection", Direction::Request, "destroy");
        // connection.destroy(object_id, cookie).
        tracker
            .observe(
                &msg(
                    CONNECTION_OBJECT,
                    destroy,
                    vec![Value::Uint32(11), Value::Uint32(0)],
                ),
                Direction::Request,
            )
            .expect("destroy");
        assert_eq!(tracker.pool_relay_id(11), None);
        assert_eq!(tracker.interface_of(11), None);
    }

    #[test]
    fn gpu_descriptors_are_explicitly_unsupported() {
        let mut tracker = Tracker::new();
        // Materialize the registry first (get_registry), then bind
        // dmabuf through it.
        let get_registry = op_by_name("ldp.core.connection", Direction::Request, "get_registry");
        let registry_object = 9;
        tracker
            .observe(
                &msg(
                    CONNECTION_OBJECT,
                    get_registry,
                    vec![Value::Uint32(1), Value::NewId(id(registry_object))],
                ),
                Direction::Request,
            )
            .expect("get_registry");
        let bind_opcode = op_by_name("ldp.core.registry", Direction::Request, "bind");
        tracker
            .observe(
                &msg(
                    registry_object,
                    bind_opcode,
                    vec![
                        Value::String("ldp.core.dmabuf".into()),
                        Value::Uint32(1),
                        Value::NewId(id(30)),
                    ],
                ),
                Direction::Request,
            )
            .expect("bind");
        assert_eq!(tracker.interface_of(30), Some("ldp.core.dmabuf"));

        let create = op_by_name("ldp.core.dmabuf", Direction::Request, "create");
        // Signature: the full arg set is longer; the arg-count guard
        // routes arity mismatches to unsupported too — the outcome that
        // matters is *refusal*, never silent forwarding.
        let plan = tracker
            .observe(&msg(30, create, vec![Value::Fd(0)]), Direction::Request)
            .expect("observe");
        assert!(matches!(plan.disposition, FdDisposition::Unsupported(_)));
    }

    #[test]
    fn release_event_plans_a_fence() {
        let mut tracker = Tracker::new();
        let release = op_by_name("ldp.core.buffer", Direction::Event, "release");
        // The buffer object 12 was never tracked, so the tracker falls
        // back to unknown-target — with an fd present that must be
        // Unsupported (never silently forwarded without state).
        let plan = tracker
            .observe(&msg(12, release, vec![Value::Fd(0)]), Direction::Event)
            .expect("observe");
        assert!(matches!(plan.disposition, FdDisposition::Unsupported(_)));

        // Track the buffer first (via a bound shm → pool → buffer
        // chain), then the event plans a fence.
        let mut tracker = Tracker::new();
        bootstrap_pool(&mut tracker, 11);
        let buffer_opcode = op_by_name("ldp.core.shm_pool", Direction::Request, "create_buffer");
        tracker
            .observe(
                &msg(
                    11,
                    buffer_opcode,
                    vec![
                        Value::Int32(0),
                        Value::Int32(1),
                        Value::Int32(1),
                        Value::Int32(4),
                        Value::Uint32(0),
                        Value::NewId(id(12)),
                    ],
                ),
                Direction::Request,
            )
            .expect("create_buffer");
        let plan = tracker
            .observe(&msg(12, release, vec![Value::Fd(0)]), Direction::Event)
            .expect("release");
        assert_eq!(
            plan.disposition,
            FdDisposition::Interpret(vec![FdSemantics::Fence])
        );
    }
}
