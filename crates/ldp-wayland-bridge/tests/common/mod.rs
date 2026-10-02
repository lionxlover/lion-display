//! Shared Wayland-client scripting for the bridge's EC suites.
//!
//! A [`Script`] plays one foreign client: it owns the compositor-side
//! [`Client`] under test plus the in-memory pool segments its host
//! seam serves. Requests are built through the real wire encoder and
//! the pinned schema tables; events are decoded back with the same
//! tables, so every suite asserts actual bytes.

#![allow(dead_code, clippy::too_many_arguments)] // flat wire builders

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use ldp_wayland_bridge::dispatch::{Client, Host};
use ldp_wayland_bridge::protocol::{self, Interface};
use ldp_wayland_bridge::wire::{self, Message, Value};

/// The shared in-memory pool map (the host seam and the script both
/// hold handles).
type Pools = Rc<RefCell<BTreeMap<u32, Vec<u8>>>>;

/// The script's host: pools from the shared map, a fixed keymap.
#[derive(Clone)]
pub struct ScriptHost {
    pools: Pools,
}

impl Host for ScriptHost {
    fn read_pool(&mut self, pool: u32, offset: usize, len: usize) -> Option<Vec<u8>> {
        let map = self.pools.borrow();
        let bytes = map.get(&pool)?;
        let end = offset.checked_add(len)?;
        bytes.get(offset..end).map(<[u8]>::to_vec)
    }
    fn keymap_fd(&mut self) -> u32 {
        3
    }
    fn keymap(&mut self) -> Vec<u8> {
        b"xkb_keymap { };\0".to_vec()
    }
}

/// Build one request's wire bytes.
pub fn request(
    iface: &'static Interface,
    object_id: u32,
    opcode: u32,
    args: Vec<Value>,
) -> Vec<u8> {
    let schema = iface
        .requests
        .iter()
        .find(|m| m.opcode == opcode)
        .expect("request opcode exists");
    let msg = Message {
        object_id,
        opcode,
        args,
    };
    let (bytes, _fds) = wire::encode(&msg, schema.args);
    bytes
}

/// Decode one expected event at `at` in the output; returns the
/// message and the bytes consumed.
pub fn event_at(
    out: &[u8],
    at: usize,
    iface: &'static Interface,
    object: u32,
    opcode: u32,
) -> (Message, usize) {
    let schema = protocol::event(iface, opcode).expect("event opcode exists");
    let (msg, used, _) = wire::decode(&out[at..], object, opcode, schema.args)
        .unwrap_or_else(|e| panic!("decode {iface:?} op {opcode}: {e:?}"));
    (msg, used)
}

/// The header word at `at`.
pub fn header_at(out: &[u8], at: usize) -> (u32, u32) {
    let h = u32::from_le_bytes([out[at], out[at + 1], out[at + 2], out[at + 3]]);
    (h >> 8, h & 0xff)
}

/// A scripting client.
pub struct Script {
    /// The compositor-side client under test.
    pub client: Client,
    pools: Pools,
}

impl Default for Script {
    fn default() -> Self {
        Self::new()
    }
}

impl Script {
    /// A fresh script (the client's host serves this script's pools).
    pub fn new() -> Script {
        let pools: Pools = Rc::new(RefCell::new(BTreeMap::new()));
        let host = ScriptHost {
            pools: Rc::clone(&pools),
        };
        Script {
            client: Client::new(Box::new(host)),
            pools,
        }
    }

    /// The standard boot: `wl_display.get_registry` (id 2), returning
    /// the global replay.
    pub fn boot(&mut self) -> Vec<(u32, String, u32)> {
        let bytes = request(&protocol::WL_DISPLAY, 1, 1, vec![Value::NewId(2)]);
        self.client.feed(&bytes).unwrap();
        let (out, _) = self.client.take_output();
        let mut globals = Vec::new();
        let mut at = 0usize;
        for expected in 1..=u32::try_from(protocol::ALL_GLOBALS.len()).unwrap() {
            let (obj, op) = header_at(&out, at);
            assert_eq!((obj, op), (2, 0), "wl_registry.global replay");
            let (msg, used) = event_at(&out, at, &protocol::WL_REGISTRY, 2, 0);
            let Value::Uint(name) = msg.args[0] else {
                panic!("global name");
            };
            let Value::String(iface) = msg.args[1].clone() else {
                panic!("global interface");
            };
            let Value::Uint(version) = msg.args[2] else {
                panic!("global version");
            };
            assert_eq!(name, expected, "registry names run in order");
            globals.push((name, iface.to_string(), version));
            at += used;
        }
        assert_eq!(at, out.len(), "the replay is the whole output");
        globals
    }

    /// Bind one global (name → id).
    pub fn bind(&mut self, name: u32, id: u32) {
        let iface = protocol::ALL_GLOBALS
            .get(usize::try_from(name).unwrap() - 1)
            .unwrap();
        let bytes = request(
            &protocol::WL_REGISTRY,
            2,
            0,
            vec![
                Value::Uint(name),
                Value::String(iface.name.into()),
                Value::Uint(iface.version),
                Value::NewId(id),
            ],
        );
        self.client.feed(&bytes).unwrap();
        let _ = self.client.take_output();
    }

    /// The full standard setup: registry + the four binds.
    pub fn connect(&mut self) {
        self.boot();
        for (name, id) in [(1u32, 3u32), (2, 4), (3, 5), (4, 6)] {
            self.bind(name, id);
        }
    }

    /// Create a pool (id) with `size` zero bytes.
    pub fn create_pool(&mut self, id: u32, size: usize) {
        let bytes = request(
            &protocol::WL_SHM,
            4,
            0,
            vec![Value::Fd(0), Value::Int(size as i32), Value::NewId(id)],
        );
        self.client.feed(&bytes).unwrap();
        let _ = self.client.take_output();
        self.pools.borrow_mut().insert(id, vec![0u8; size]);
    }

    /// Create a buffer (id) from a pool.
    pub fn create_buffer(
        &mut self,
        id: u32,
        pool: u32,
        offset: i32,
        w: i32,
        h: i32,
        stride: i32,
        format: u32,
    ) {
        let bytes = request(
            &protocol::WL_SHM_POOL,
            pool,
            1,
            vec![
                Value::NewId(id),
                Value::Int(offset),
                Value::Int(w),
                Value::Int(h),
                Value::Int(stride),
                Value::Uint(format),
            ],
        );
        self.client.feed(&bytes).unwrap();
        let _ = self.client.take_output();
    }

    /// Create a surface (id).
    pub fn create_surface(&mut self, id: u32) {
        let bytes = request(&protocol::WL_COMPOSITOR, 3, 0, vec![Value::NewId(id)]);
        self.client.feed(&bytes).unwrap();
        let _ = self.client.take_output();
    }

    /// Attach + damage + commit in one batched feed.
    pub fn commit(&mut self, surface: u32, buffer: u32, damage: &[(i32, i32, i32, i32)]) {
        let mut all = request(
            &protocol::WL_SURFACE,
            surface,
            1,
            vec![Value::Object(buffer), Value::Int(0), Value::Int(0)],
        );
        for &(x, y, w, h) in damage {
            let d = request(
                &protocol::WL_SURFACE,
                surface,
                2,
                vec![Value::Int(x), Value::Int(y), Value::Int(w), Value::Int(h)],
            );
            all.extend_from_slice(&d);
        }
        let commit = request(&protocol::WL_SURFACE, surface, 6, vec![]);
        all.extend_from_slice(&commit);
        self.client.feed(&all).unwrap();
        let _ = self.client.take_output();
    }

    /// Fill a pool region with one xrgb pixel.
    pub fn fill(&mut self, pool: u32, offset: usize, w: usize, h: usize, pixel: [u8; 4]) {
        let mut map = self.pools.borrow_mut();
        let bytes = map.get_mut(&pool).expect("pool exists");
        for row in 0..h {
            for col in 0..w {
                let at = offset + (row * w + col) * 4;
                bytes[at..at + 4].copy_from_slice(&pixel);
            }
        }
    }
}
