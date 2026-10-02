//! The compositor's renderer: the hardware-first selection (the
//! macOS doctrine) and the two backends behind one type.
//!
//! [`CompositorRenderer`] wraps the software reference backend and
//! the GL backend ([`GlesRenderer`]) behind the shared
//! [`Renderer`] contract plus the readout
//! seam the frame loop copies into the scanout chain. Selection runs
//! through [`ldp_renderer::gles::select::resolve`] composed with the
//! Phase 30 GPU-class layer in [`build_renderer`]: **Auto** (the
//! default) probes the real EGL+GLES context
//! ([`ldp_gpu::gles::RealGles`]), reads its identity
//! (`glGetString` vendor/renderer/version — [`ldp_gpu::GlIdentity`]),
//! and takes the GL backend when a real GPU answers — *refusing* a
//! CPU-rasterizer context (llvmpipe and friends) in favor of the
//! specialized software backend, reason carried in the report line;
//! a missing GL stack falls back to software the same honest way; a
//! forced `gl` without hardware is a hard, typed startup failure —
//! never a silent CPU path.
//!
//! Every frame the compositor produces — window-management redraws
//! (map/unmap/restack), client animation frames, damage repaints,
//! and the capture path — flows through this seam, so "hardware
//! acceleration by default" covers the whole render pipeline, not a
//! special case.

#![forbid(unsafe_code)]

use ldp_renderer::gles::api::GlesApi;
use ldp_renderer::gles::{GlesRenderer, RendererChoice, RendererDecision};
use ldp_renderer::{Renderer, SoftwareRenderer};

/// The compositor's rendering backend: software or GL, one type.
pub enum CompositorRenderer {
    /// The reference CPU compositor.
    Software(SoftwareRenderer),
    /// The GL backend over any [`GlesApi`] implementation — the real
    /// EGL+GLES context on hardware, the reference evaluator in the
    /// equivalence suites.
    Gles(GlesRenderer),
}

impl core::fmt::Debug for CompositorRenderer {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Software(_) => f.write_str("CompositorRenderer::Software"),
            Self::Gles(_) => f.write_str("CompositorRenderer::Gles"),
        }
    }
}

impl CompositorRenderer {
    /// The pure software backend.
    #[must_use]
    pub fn software() -> Self {
        Self::Software(SoftwareRenderer::new())
    }

    /// The GL backend over an injected API (tests, benches).
    #[must_use]
    pub fn gles(api: Box<dyn GlesApi>) -> Self {
        Self::Gles(GlesRenderer::new(api))
    }

    /// The backend's name (startup reports, selftest lines).
    #[must_use]
    pub fn backend(&self) -> &'static str {
        match self {
            Self::Software(_) => "software",
            Self::Gles(_) => "gles",
        }
    }

    /// The last completed frame's pixels: premultiplied ARGB words,
    /// top-down — the scanout copy source for either backend.
    #[must_use]
    pub fn readout(&self) -> std::borrow::Cow<'_, [u32]> {
        match self {
            Self::Software(r) => std::borrow::Cow::Borrowed(r.readout()),
            Self::Gles(r) => std::borrow::Cow::Borrowed(r.readout()),
        }
    }
}

impl Renderer for CompositorRenderer {
    fn backend(&self) -> &'static str {
        self.backend()
    }

    fn begin_frame(
        &mut self,
        output: &ldp_renderer::OutputDesc,
        damage: &ldp_core::geometry::Region,
    ) -> Result<(), ldp_renderer::RendererError> {
        match self {
            Self::Software(r) => r.begin_frame(output, damage),
            Self::Gles(r) => r.begin_frame(output, damage),
        }
    }

    fn submit(
        &mut self,
        layers: &[ldp_renderer::SurfaceLayer<'_>],
    ) -> Result<ldp_renderer::RenderStats, ldp_renderer::RendererError> {
        match self {
            Self::Software(r) => r.submit(layers),
            Self::Gles(r) => r.submit(layers),
        }
    }

    #[allow(clippy::many_single_char_names)] // the Renderer contract's own parameter names
    fn clear_damage(
        &mut self,
        r: u8,
        g: u8,
        b: u8,
        a: u8,
    ) -> Result<(), ldp_renderer::RendererError> {
        match self {
            Self::Software(sw) => sw.clear_damage(r, g, b, a),
            Self::Gles(gl) => gl.clear_damage(r, g, b, a),
        }
    }

    fn end_frame(&mut self) -> Result<ldp_renderer::CompletedFrame, ldp_renderer::RendererError> {
        match self {
            Self::Software(r) => r.end_frame(),
            Self::Gles(r) => r.end_frame(),
        }
    }
}

/// Build the renderer per the operator's choice, hardware first.
///
/// `inject` (tests) bypasses the probe entirely. The returned
/// decision's report line is what the startup prints — the honest
/// "renderer: gles (hardware: …)" or "renderer: software (GL
/// unavailable: …)".
///
/// The Phase 30 doctrine on top of the pinned policy: the probe
/// reports the context's *identity* (vendor/renderer/version strings
/// plus the derived GPU class), and `Auto` refuses a **CPU-rasterizer
/// GL context** — when the "hardware" is `llvmpipe` (Mesa's software
/// driver; the no-graphics-card machine, or the failed-driver
/// fallback), the specialized software backend is the faster CPU path
/// (it blends words directly instead of emulating a GL state
/// machine), so Auto takes it *with the reason in the report line*.
/// `--renderer gl` still forces the GL path over any context — the
/// operator is the final authority, always.
///
/// # Errors
/// A forced `--renderer gl` on a machine without GL: the typed
/// bring-up failure, surfaced as an I/O-class
/// [`LdpError`](ldp_core::error::LdpError) with the
/// reason (no silent fallback, ever).
///
/// # Panics
///
/// Only on the internal invariant that a `gles` decision follows a
/// green probe (a logic bug, not an environment state).
pub fn build_renderer(
    choice: RendererChoice,
    inject: Option<Box<dyn GlesApi>>,
) -> Result<(CompositorRenderer, RendererDecision), ldp_core::error::LdpError> {
    use ldp_renderer::gles::select::resolve;
    // The injected backend short-circuits the probe (the equivalence
    // suites drive the whole compositor through the reference
    // evaluator this way).
    if let Some(api) = inject {
        return Ok((
            CompositorRenderer::gles(api),
            RendererDecision {
                backend: ldp_renderer::gles::RendererBackend::Gles,
                report: "gles (injected backend)".to_owned(),
            },
        ));
    }
    // The hardware probe: a real EGL + GLES 2.0 context or the typed
    // reason it could not come up.
    let probe = ldp_gpu::gles::RealGles::new();
    let decision = match &probe {
        Ok(backend) => gpu_aware_decision(choice, &backend.identity()),
        Err(e) => resolve(choice, Err(&e.to_string())),
    }
    .map_err(|e| {
        ldp_core::error::LdpError::Io(std::sync::Arc::new(std::io::Error::other(format!(
            "renderer bring-up failed: {e}"
        ))))
    })?;
    let renderer = match decision.backend {
        ldp_renderer::gles::RendererBackend::Gles => {
            // The probe proved the context; move it into the renderer.
            let backend = probe.expect("the decision said gles after a green probe");
            CompositorRenderer::Gles(GlesRenderer::new(Box::new(backend)))
        }
        ldp_renderer::gles::RendererBackend::Software => {
            // A CPU-rasterizer context is dropped here (its `Drop`
            // tears the EGL state down honestly) — the specialized
            // software backend owns the CPU instead.
            CompositorRenderer::software()
        }
    };
    Ok((renderer, decision))
}

/// The class-aware selection policy (pure — every cell is unit-pinned).
///
/// Composes the pinned [`ldp_renderer::gles::select::resolve`] matrix
/// with the one Phase 30 rule:
/// `Auto` + a software-rasterizer GL context → the software backend,
/// reason carried. Everything else defers to the pinned doctrine.
fn gpu_aware_decision(
    choice: RendererChoice,
    identity: &ldp_gpu::GlIdentity,
) -> Result<RendererDecision, ldp_renderer::RendererError> {
    use ldp_gpu::GpuClass;
    use ldp_renderer::gles::select::resolve;
    // Auto refuses CPU-GL: the specialized backend is the faster CPU.
    if choice == RendererChoice::Auto && identity.class == GpuClass::SoftwareRasterizer {
        return Ok(RendererDecision {
            backend: ldp_renderer::gles::RendererBackend::Software,
            report: format!(
                "software (GL is a CPU rasterizer: {}; the specialized software backend is faster)",
                identity.summary()
            ),
        });
    }
    // Hardware (or forced): the pinned doctrine, with the identity in
    // the report — "EGL + GLES 2.0 context up, discrete GPU: AMD …".
    let summary = format!("EGL + GLES 2.0 context up, {}", identity.summary());
    resolve(choice, Ok(&summary))
}

#[cfg(test)]
mod tests {
    use super::gpu_aware_decision;
    use ldp_gpu::{GlIdentity, GpuClass};
    use ldp_renderer::gles::{RendererBackend, RendererChoice};

    fn id(renderer: &str) -> GlIdentity {
        GlIdentity::from_strings("vendor", renderer, "OpenGL ES 2.0")
    }

    #[test]
    fn auto_prefers_real_hardware_with_the_identity_named() {
        let d = gpu_aware_decision(RendererChoice::Auto, &id("AMD Radeon RX 7900 XTX")).unwrap();
        assert_eq!(d.backend, RendererBackend::Gles);
        assert_eq!(
            d.report,
            "gles (hardware: EGL + GLES 2.0 context up, discrete GPU: AMD Radeon RX 7900 XTX)"
        );
    }

    #[test]
    fn auto_refuses_cpu_rasterizer_gl_with_the_reason() {
        let d = gpu_aware_decision(
            RendererChoice::Auto,
            &id("llvmpipe (LLVM 17.0.6, 256 bits)"),
        )
        .unwrap();
        assert_eq!(d.backend, RendererBackend::Software);
        assert_eq!(
            d.report,
            "software (GL is a CPU rasterizer: software rasterizer (no GPU): \
             llvmpipe (LLVM 17.0.6, 256 bits); the specialized software backend is faster)"
        );
    }

    #[test]
    fn forced_gl_takes_the_gl_path_even_over_a_cpu_rasterizer() {
        // The operator is the final authority: forced GL over
        // llvmpipe is honest (and the report names what it is).
        let d = gpu_aware_decision(RendererChoice::Gles, &id("llvmpipe (LLVM 17.0.6)")).unwrap();
        assert_eq!(d.backend, RendererBackend::Gles);
        assert_eq!(
            d.report,
            "gles (forced; EGL + GLES 2.0 context up, \
             software rasterizer (no GPU): llvmpipe (LLVM 17.0.6))"
        );
    }

    #[test]
    fn unknown_and_virtual_gpus_stay_hardware() {
        // The honest default: an unrecognized or virtual GPU is still
        // a GPU — never a silent software degrade.
        for renderer in ["", "Some Future Silicon", "virgl (Virtio-GPU Venus)"] {
            let d = gpu_aware_decision(RendererChoice::Auto, &id(renderer)).unwrap();
            assert_eq!(d.backend, RendererBackend::Gles, "renderer {renderer:?}");
        }
        assert_eq!(id("").class, GpuClass::Unknown);
    }

    #[test]
    fn forced_software_ignores_the_identity_entirely() {
        let d =
            gpu_aware_decision(RendererChoice::Software, &id("AMD Radeon RX 7900 XTX")).unwrap();
        assert_eq!(d.backend, RendererBackend::Software);
        assert_eq!(d.report, "software (forced)");
    }
}
