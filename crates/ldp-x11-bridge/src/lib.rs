//! # ldp-x11-bridge — the X11 compatibility proxy for LDP
//!
//! A pure-Rust X11 *server subset* (core protocol + BIG-REQUESTS +
//! MIT-SHM + the GC/drawable requests simple toolkits need) that
//! proxies X windows onto LDP toplevels with explicit-sync
//! translation. The crate is protocol machinery only: it opens no
//! sockets, maps no shared memory (the [`shm::ShmHost`] seam supplies
//! segment backing), reads no clock, and holds no `unsafe` — the
//! process binary owns the syscall layer per the `ldp-transport`
//! audit doctrine.
//!
//! Module map:
//!
//! * [`wire`] — byte order, request framing (BIG-REQUESTS included),
//!   the 32-byte reply/error/event envelopes;
//! * [`setup`] — the connection handshake both directions and the
//!   one-screen/one-visual setup reply;
//! * [`events`] — core event encoders (key/button/motion, exposure,
//!   structure, property, selection, ClientMessage);
//! * [`window`] — the window tree: geometry, stacking, exact
//!   visibility regions, gravity-retained backing stores;
//! * [`render`] — the software rasterizer (GX functions, plane masks,
//!   clipping, damage);
//! * [`shape`] — arc and polygon rasterization;
//! * [`gc`] — graphics-context state and the value-list codec;
//! * [`property`] — the ICCCM property store;
//! * [`atoms`] — the atom registry;
//! * [`shm`] — MIT-SHM segment state over the host seam;
//! * [`dispatch`] — the multi-client connection state machine and the
//!   driver-facing seams (input injection, root compositing, damage);
//! * [`ops`] / [`draw_ops`] — the per-opcode request handlers;
//! * [`driver`] — the LDP side: bridge-scope token check, the rootful
//!   toplevel, shm pool export, frame-driven damage commits, and the
//!   implicit-to-explicit sync translation.

#![forbid(unsafe_code)]

pub mod atoms;
pub mod dispatch;
pub mod draw_ops;
pub mod driver;
pub mod events;
pub mod gc;
pub mod ops;
pub mod property;
pub mod render;
pub mod rootless;
pub mod setup;
pub mod shape;
pub mod shm;
pub mod window;
pub mod wire;
