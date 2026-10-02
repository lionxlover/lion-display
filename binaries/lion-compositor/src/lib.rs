//! # lion-compositor — the Phase 10 vertical slice, grown into the
//! real scanout service
//!
//! The example compositor of the LionOS display stack: one binary that
//! wires the whole LDP pipeline end to end —
//!
//! ```text
//! client window → attach/commit → surface tree → damage engine
//!   → renderer (GL by default, software fallback) → scanout chain
//!   → atomic page flip (mock KMS in CI, real DRM on hardware)
//!   → presentation feedback (frame_target / presented)
//! ```
//!
//! and serves it over the real protocol: an [`ldp_server::Server`] on
//! an abstract-namespace Unix socket, one session thread per client,
//! every window-system decision inside one
//! [`CompositorDispatcher`] — the
//! "compositor is a dispatcher" doctrine of `docs/architecture.md`.
//!
//! ## The serve loop (Phase 25) and live re-arrangement (Phase 26)
//!
//! [`server::Compositor::drm`] drives the *real* pipeline: DRM-Master
//! rights, kernel dumb buffers mapped for CPU composition, the applied
//! atomic enable, then [`server::Compositor::serve_kms`] — poll the
//! listening socket and the DRM fd, land page flips, accept clients,
//! and on Ctrl-C tear the display down honestly (disable commit,
//! objects released, master dropped). The same choreography runs on
//! the mock device in CI — [`frame_loop`] is backend-independent over
//! [`ldp_display::driver::DisplayDriver`], and the mapped-store
//! equivalence suite proves the real delivery path byte-equal.
//!
//! When the topology moves under a serving compositor — a cable
//! pulls, a dock lands, a sink re-negotiates — the serve loop's
//! hotplug arm answers: [`World::rearrange`](crate::rearrange::Rearrange)
//! re-probes the
//! whole topology and moves the served pipeline (monitor swap), goes
//! honestly dark (last display gone — the protocol keeps serving,
//! frame requests park with `output_off`), or relights (the first
//! display returns — the desktop paints again, no session lost).
//! Clients feel it through their output objects: revoked, re-bound,
//! the fresh cascade.
//!
//! ## The headless time doctrine
//!
//! The mock device's injected clock is the only clock. It advances
//! only inside [`World::pump`](frame_loop) at *wake points* — every
//! successfully handled inbound message pumps the world, rendering
//! pending damage and advancing the clock exactly to pending flip
//! landings, never speculatively. The sequence of protocol events is
//! therefore a pure function of the message stream: byte-reproducible,
//! CI-friendly, and identical to a wall-time loop's decisions.
//!
//! ## Buffer exchange
//!
//! `shm.create_pool` maps the received descriptor read-only
//! (the audited [`sys`] layer); `shm_pool.create_buffer` validates the
//! window eagerly; `surface.attach` + `surface.commit` splice it into
//! the tree's pending state; the render pass builds
//! [`BufferView`](ldp_renderer::BufferView)s from the live mappings.
//! Superseded buffers get a `buffer.release` event whose fence is an
//! already-signalled **eventfd** in headless operation — the
//! documented stand-in for the kernel sync-file the DRM path hands out
//! (readability means "the compositor is done reading").
//!
//! ## Scope (honest limits)
//!
//! * One output at a time (the first connected connector), one
//!   primary plane, composition through the hardware-first renderer
//!   selection (GL when the machine has a GL stack, software
//!   otherwise) into a double-buffered XRGB scanout chain — CPU-mapped
//!   dumb buffers on real hardware, shadow buffers on the mock. The
//!   served output *follows the topology* live (Phase 26): swaps,
//!   darks, and relights — but driving several outputs at once (the
//!   multi-CRTC layout) is the next display milestone.
//! * Root surfaces map at the layout origin — the positioning shell
//!   is Phase 12.
//! * The dynamic-global story (registry `global_remove` + re-advert)
//!   is broker-era work; today the output global stays advertised and
//!   object revocation is the live-migration signal (documented in
//!   `docs/protocol.md` and the Phase 26 roadmap entry).
//!
//! # Safety
//!
//! Every `unsafe` lives in the audited [`sys`] module (mmap, memfd,
//! eventfd, shutdown signals — each call carrying a `SAFETY` comment,
//! the `ldp-transport` precedent); every other module is
//! `#![forbid(unsafe_code)]`, so the crate root cannot forbid.

pub mod dispatch;
pub mod drm_probe;
pub mod dump;
pub mod frame_loop;
pub mod input;
pub mod outbox;
pub mod output;
pub mod plane_session;
pub mod power;
pub mod rearrange;
pub mod renderer;
pub mod scene;
pub mod server;
pub mod shell;
pub mod shm;
pub mod sys;

use ldp_core::bitset::Bitset128;

pub use dispatch::CompositorDispatcher;
pub use frame_loop::FrameError;
pub use outbox::{OutboxEntry, Outboxes};
pub use output::OutputGlobal;
pub use rearrange::{Rearrange, Served};
pub use renderer::CompositorRenderer;
pub use scene::{DrmLease, ObjectKey, Route, ScanoutChain, Scene, Shared, World};
pub use server::{Compositor, CompositorConfig};

/// A bitset from its low wire word (bits 0..=31).
pub(crate) fn low_bits(v: u32) -> Bitset128 {
    Bitset128::from_words([v, 0, 0, 0])
}
