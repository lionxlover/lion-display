//! The renderer selection policy — the macOS doctrine as pure logic.
//!
//! One decision, three inputs, no surprises:
//!
//! * **Auto** (the default): hardware first. A working GL context
//!   selects the GL backend; a missing GL stack selects software and
//!   *carries the reason* — the honest degradation the operator reads.
//! * **Gles** (forced): the GL backend or a typed failure. No silent
//!   fallback — an operator who pins `--renderer gl` wants the
//!   failure, not a quiet CPU path.
//! * **Software** (forced): the reference backend, always available.
//!
//! The policy is deliberately separate from the probing: the caller
//! tries to bring the GL context up (`ldp_gpu::gles::RealGles::new()`
//! on a real machine, the reference evaluator in tests) and hands the
//! outcome to [`resolve`], which is exhaustively pinned by tests —
//! every cell of the choice × hardware matrix.

use crate::errors::RendererError;

/// Which backend the operator asked for.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum RendererChoice {
    /// Hardware first, software fallback with the reason carried
    /// (the default — the macOS doctrine).
    #[default]
    Auto,
    /// The GL backend or a typed failure; never a silent fallback.
    Gles,
    /// The reference software backend.
    Software,
}

impl RendererChoice {
    /// Parse the CLI spelling (`--renderer VALUE`).
    ///
    /// # Errors
    /// [`crate::errors::RendererError::UnsupportedLayer`]-free: this
    /// returns `Err(String)` naming the bad value and the accepted
    /// set — the argv layer's error shape.
    pub fn parse(text: &str) -> Result<Self, String> {
        match text {
            "auto" => Ok(Self::Auto),
            "gl" | "gles" => Ok(Self::Gles),
            "software" | "sw" => Ok(Self::Software),
            other => Err(format!("unknown renderer '{other}' (auto|gl|software)")),
        }
    }

    /// The canonical CLI spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Gles => "gl",
            Self::Software => "software",
        }
    }
}

/// Which backend a decision landed on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RendererBackend {
    /// GPU compositing through the GLES seam.
    Gles,
    /// The reference CPU compositor.
    Software,
}

impl RendererBackend {
    /// The backend's name (log lines, selftest reports).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Gles => "gles",
            Self::Software => "software",
        }
    }
}

/// A resolved selection: the backend plus the one-line report the
/// operator reads at startup.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RendererDecision {
    /// The backend to build.
    pub backend: RendererBackend,
    /// The report line (always present; "gles (hardware)" or
    /// "software (GL unavailable: …)" and friends).
    pub report: String,
}

/// Resolve a choice against the GL probe outcome.
///
/// `hardware` carries the GL context's bring-up result: `Ok(summary)`
/// when a context came up (the summary names what it proved — e.g.
/// "EGL 1.5, GLES 2.0 context"), `Err(reason)` when the machine has
/// no usable GL stack.
///
/// # Errors
///
/// [`RendererError::Gles`] **only** for a forced `Gles` choice on a
/// machine without GL — the one case that must not silently degrade.
pub fn resolve(
    choice: RendererChoice,
    hardware: Result<&str, &str>,
) -> Result<RendererDecision, RendererError> {
    match (choice, hardware) {
        (RendererChoice::Auto, Ok(summary)) => Ok(RendererDecision {
            backend: RendererBackend::Gles,
            report: format!("gles (hardware: {summary})"),
        }),
        (RendererChoice::Auto, Err(reason)) => Ok(RendererDecision {
            backend: RendererBackend::Software,
            report: format!("software (GL unavailable: {reason})"),
        }),
        (RendererChoice::Gles, Ok(summary)) => Ok(RendererDecision {
            backend: RendererBackend::Gles,
            report: format!("gles (forced; {summary})"),
        }),
        (RendererChoice::Gles, Err(reason)) => Err(RendererError::Gles(
            crate::gles::api::GlesError::Unavailable(reason.to_owned()),
        )),
        (RendererChoice::Software, _) => Ok(RendererDecision {
            backend: RendererBackend::Software,
            report: "software (forced)".to_owned(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_choice_hardware_cell_is_pinned() {
        // Auto prefers hardware.
        let d = resolve(RendererChoice::Auto, Ok("EGL 1.5, GLES 2.0")).unwrap();
        assert_eq!(d.backend, RendererBackend::Gles);
        assert_eq!(d.report, "gles (hardware: EGL 1.5, GLES 2.0)");
        // Auto degrades with the reason carried.
        let d = resolve(RendererChoice::Auto, Err("no libEGL")).unwrap();
        assert_eq!(d.backend, RendererBackend::Software);
        assert_eq!(d.report, "software (GL unavailable: no libEGL)");
        // Forced GL works when hardware exists.
        let d = resolve(RendererChoice::Gles, Ok("ctx up")).unwrap();
        assert_eq!(d.backend, RendererBackend::Gles);
        assert_eq!(d.report, "gles (forced; ctx up)");
        // Forced GL never silently degrades.
        let err = resolve(RendererChoice::Gles, Err("no libEGL")).unwrap_err();
        assert!(matches!(err, RendererError::Gles(_)));
        // Forced software ignores hardware entirely.
        let d = resolve(RendererChoice::Software, Ok("ctx up")).unwrap();
        assert_eq!(d.backend, RendererBackend::Software);
        assert_eq!(d.report, "software (forced)");
    }

    #[test]
    fn cli_spellings_round_trip() {
        for (text, want) in [
            ("auto", RendererChoice::Auto),
            ("gl", RendererChoice::Gles),
            ("gles", RendererChoice::Gles),
            ("software", RendererChoice::Software),
            ("sw", RendererChoice::Software),
        ] {
            assert_eq!(RendererChoice::parse(text).unwrap(), want);
        }
        let err = RendererChoice::parse("vulkan").unwrap_err();
        assert!(err.contains("vulkan") && err.contains("auto|gl|software"));
        // The default is the doctrine: hardware first.
        assert_eq!(RendererChoice::default(), RendererChoice::Auto);
    }
}
