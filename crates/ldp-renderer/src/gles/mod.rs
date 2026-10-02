//! The GL rendering backend — hardware acceleration behind the same
//! [`Renderer`](crate::Renderer) contract.
//!
//! The macOS doctrine, stated as code: rendering runs on the GPU when
//! the machine has a GL stack and falls back to the software backend
//! when it does not — *by default*, never as a silent surprise. The
//! selection policy ([`select`]) is pure and pinned by tests; the GL
//! command vocabulary ([`api`]) is an object-safe seam so the whole
//! renderer is CI-provable without a GPU; the reference evaluator
//! ([`mock::RefGles`]) executes the command stream with the exact
//! integer blend rule the software renderer uses, so the Phase 24
//! exit criteria can demand **byte-equality** between the GL path and
//! the reference path over randomized corpora — and through the whole
//! compositor session (the `binaries/lion-compositor` hardware suite).
//!
//! Scope of the v1 GL path, stated the way the maturity matrix states
//! it: the 32-bit RGB family (XRGB/ARGB/XBGR/ABGR8888), 1:1 placement,
//! `Transform::Normal`, uniform per-layer opacity, sRGB SDR
//! same-description blending. Outside that subset the renderer fails
//! typed ([`RendererError::UnsupportedLayer`](crate::errors::RendererError::UnsupportedLayer))
//! rather than rendering
//! something wrong — the caller composites those frames with the
//! software backend (the compositor's mixed-mode fallback, documented
//! in the user guide). YUV shader uploads, scaled sampling, and the
//! DMA-BUF zero-copy import path are the next GL milestones
//! (`docs/roadmap.md`).
//!
//! Real-hardware note: on a physical GPU the fixed-function blend runs
//! in float and may differ from the integer reference by ±1 LSB; the
//! *reference* evaluator is the conformance oracle, the real backend
//! is the fast path. The command stream both execute is identical.

pub mod api;
pub mod mock;
pub mod renderer;
pub mod select;

pub use api::{GlesApi, GlesError, GlesProgram, GlesTarget, GlesTexture};
pub use mock::{GlesCmd, RecordingGles, RefGles};
pub use renderer::GlesRenderer;
pub use select::{RendererBackend, RendererChoice, RendererDecision};

/// The composite pass vertex shader (GLES 2.0).
///
/// `a_pos` is unit quad space; `u_rect` is the destination in
/// top-down pixels; the Y-flip (GL's origin is bottom-left) lives
/// here so the API contract stays top-down everywhere else. Pinned
/// byte-for-byte by `tests/gles_stream.rs`.
pub const VERTEX_SRC: &str = "attribute vec2 a_pos;
attribute vec2 a_uv;
uniform vec4 u_rect;
uniform vec2 u_size;
varying vec2 v_uv;
void main() {
    vec2 px = vec2(u_rect.x + a_pos.x * u_rect.z, u_rect.y + a_pos.y * u_rect.w);
    vec2 ndc = vec2(px.x / u_size.x, px.y / u_size.y) * 2.0 - 1.0;
    gl_Position = vec4(ndc.x, -ndc.y, 0.0, 1.0);
    v_uv = a_uv;
}
";

/// The composite pass fragment shader (GLES 2.0).
///
/// Textures arrive premultiplied and RGBA-ordered (the renderer
/// swizzles on upload); the opacity uniform scales every channel —
/// the premultiplied `over` blend is set by the API's pass state.
pub const FRAGMENT_SRC: &str = "precision mediump float;
uniform sampler2D u_tex;
uniform float u_opacity;
varying vec2 v_uv;
void main() {
    gl_FragColor = texture2D(u_tex, v_uv) * u_opacity;
}
";
