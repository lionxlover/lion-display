//! Renderer error taxonomy.
//!
//! Every failure is a contract violation by the *caller* (bad output
//! description, buffer data smaller than the validated geometry claims,
//! protocol misuse like submitting outside a frame); the pixel pipeline
//! itself cannot fail — out-of-range samples clamp rather than error, so a
//! frame always completes deterministically.

use core::fmt;
use ldp_core::buffer::FourCC;

/// Why a renderer operation was rejected.
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum RendererError {
    /// `begin_frame` output dimensions were zero on an axis.
    ZeroOutputSize,
    /// The output format is not one of the software framebuffer formats
    /// (32-bit XRGB/ARGB/XBGR/ABGR).
    UnsupportedOutputFormat(FourCC),
    /// The layer buffer's format is not a v1-supported fourcc.
    UnsupportedBufferFormat(FourCC),
    /// The backing store is smaller than the buffer geometry spans; the
    /// geometry was validated against a larger storage size than supplied.
    BufferDataTooSmall {
        /// Bytes the geometry's last plane row reaches.
        needed: u64,
        /// Bytes actually supplied.
        have: u64,
    },
    /// `submit`/`end_frame` called without an open `begin_frame`.
    NoFrameInProgress,
    /// The GL backend refused a command or could not be brought up
    /// (Phase 24): the typed [`GlesError`](crate::gles::api::GlesError)
    /// rides along — including the
    /// forced-`gl`-without-GL startup failure, which must not silently
    /// degrade.
    Gles(crate::gles::api::GlesError),
    /// The layer is outside the GL v1 path's representable subset
    /// (format family, transform, or scaling) — the caller composites
    /// such frames with the software backend instead of accepting a
    /// wrong render.
    UnsupportedLayer {
        /// What was outside the subset.
        detail: String,
    },
}

impl fmt::Display for RendererError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroOutputSize => f.write_str("output size is zero on an axis"),
            Self::UnsupportedOutputFormat(fmt4cc) => {
                write!(f, "unsupported output format {fmt4cc}")
            }
            Self::UnsupportedBufferFormat(fmt4cc) => {
                write!(f, "unsupported buffer format {fmt4cc}")
            }
            Self::BufferDataTooSmall { needed, have } => {
                write!(f, "buffer data too small: needs {needed} B, has {have} B")
            }
            Self::NoFrameInProgress => f.write_str("no frame in progress"),
            Self::Gles(e) => write!(f, "GL backend: {e}"),
            Self::UnsupportedLayer { detail } => {
                write!(f, "layer outside the GL v1 subset: {detail}")
            }
        }
    }
}

impl std::error::Error for RendererError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_name_the_culprit() {
        let e = RendererError::UnsupportedOutputFormat(FourCC::RGB565);
        assert!(e.to_string().contains("RG16"));
        let e = RendererError::BufferDataTooSmall {
            needed: 4096,
            have: 1024,
        };
        assert!(e.to_string().contains("4096"));
        assert_eq!(
            RendererError::NoFrameInProgress.to_string(),
            "no frame in progress"
        );
    }
}
