//! The GL command vocabulary — the object-safe seam the renderer talks
//! to and both backends implement.
//!
//! The altitude is deliberate: not raw GLES (the renderer logic would
//! drown in GL state, untestable without a GPU), not a scene-graph
//! (the renderer's damage/clipping decisions must stay *its own*, so
//! they are testable). Each method is one composite-pass concept the
//! renderer needs; [`crate::gles::mock::RefGles`] evaluates them with
//! the reference integer blend and the real backend (in `ldp-gpu`)
//! maps each onto the matching GLES 2.0 calls.
//!
//! Coordinate contract: **top-down pixels everywhere** — destination
//! rectangles, scissor rectangles, and readback rows. The GL origin
//! flip is an implementation detail of the real backend (it lives in
//! the pinned vertex shader), invisible above this seam.
//!
//! Color contract: clear colors are **premultiplied**; layer textures
//! are uploaded premultiplied, RGBA byte-ordered, row-major, stride =
//! `width * 4`. Blending inside a pass is premultiplied source-over
//! with the pass's opacity factor folded into the source.

use ldp_core::geometry::Rect;

/// A GL backend failure.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum GlesError {
    /// The backend could not be brought up (no GL stack, no context,
    /// no required extension). Carries the honest reason — the
    /// degradation report the user reads.
    Unavailable(String),
    /// A command referenced an unknown or destroyed handle.
    BadHandle(&'static str),
    /// The command stream violated the contract (sizes, pass state).
    Invalid(String),
    /// The backend rejected a command (shader compile failure, out of
    /// memory, GL error state). Carries the driver text when present.
    Failed(String),
}

impl std::fmt::Display for GlesError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(why) => write!(f, "GL backend unavailable: {why}"),
            Self::BadHandle(what) => write!(f, "unknown or destroyed {what}"),
            Self::Invalid(why) => write!(f, "invalid GL command stream: {why}"),
            Self::Failed(why) => write!(f, "GL command failed: {why}"),
        }
    }
}

impl std::error::Error for GlesError {}

/// A linked shader program handle (nonzero by construction).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct GlesProgram(u32);

impl GlesProgram {
    /// Wrap a raw handle; `0` is the null program and is rejected.
    ///
    /// # Errors
    /// [`GlesError::BadHandle`] for the null handle.
    pub fn new(raw: u32) -> Result<Self, GlesError> {
        if raw == 0 {
            return Err(GlesError::BadHandle("program handle (null)"));
        }
        Ok(Self(raw))
    }

    /// The raw handle (backend namespace).
    #[must_use]
    pub const fn raw(self) -> u32 {
        self.0
    }
}

/// A render-target handle: an offscreen framebuffer the compositor's
/// frames accumulate into (nonzero by construction).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct GlesTarget(u32);

impl GlesTarget {
    /// Wrap a raw handle; `0` is the null target and is rejected.
    ///
    /// # Errors
    /// [`GlesError::BadHandle`] for the null handle.
    pub fn new(raw: u32) -> Result<Self, GlesError> {
        if raw == 0 {
            return Err(GlesError::BadHandle("render-target handle (null)"));
        }
        Ok(Self(raw))
    }

    /// The raw handle (backend namespace).
    #[must_use]
    pub const fn raw(self) -> u32 {
        self.0
    }
}

/// A texture handle holding one layer's premultiplied RGBA pixels
/// (nonzero by construction).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct GlesTexture(u32);

impl GlesTexture {
    /// Wrap a raw handle; `0` is the null texture and is rejected.
    ///
    /// # Errors
    /// [`GlesError::BadHandle`] for the null handle.
    pub fn new(raw: u32) -> Result<Self, GlesError> {
        if raw == 0 {
            return Err(GlesError::BadHandle("texture handle (null)"));
        }
        Ok(Self(raw))
    }

    /// The raw handle (backend namespace).
    #[must_use]
    pub const fn raw(self) -> u32 {
        self.0
    }
}

/// The GL command vocabulary the composite pass runs on.
///
/// Implementations: the reference evaluator
/// ([`crate::gles::mock::RefGles`], the CI oracle) and the real
/// hardware backend (`ldp_gpu::gles::RealGles`, dlopen'd
/// EGL+GLESv2). The recorder
/// ([`crate::gles::mock::RecordingGles`]) wraps any implementation
/// and pins the command stream in tests.
///
/// State model: passes bracket every mutation. `begin_pass` binds a
/// target and installs a scissor (or clears it for the whole target);
/// `clear` and `draw_layer` act inside the pass; `end_pass` unbinds.
/// Uploads and program management may happen between passes. Handles
/// are owned: destroy frees them; use after destroy is a typed error.
///
/// The trait requires [`Send`]: a backend instance lives inside the
/// compositor's world, which crosses session threads under the world
/// mutex (the real backend documents its per-call context-migration
/// discipline).
pub trait GlesApi: Send {
    /// Link the composite program from the given shader sources.
    ///
    /// # Errors
    /// [`GlesError::Failed`] when compilation or linking fails (the
    /// driver's log text rides along).
    fn create_program(&mut self, vs: &str, fs: &str) -> Result<GlesProgram, GlesError>;

    /// Destroy a linked program.
    ///
    /// # Errors
    /// [`GlesError::BadHandle`] for an unknown program.
    fn destroy_program(&mut self, program: GlesProgram) -> Result<(), GlesError>;

    /// Create an offscreen render target `width` x `height`. Content
    /// persists across passes and frames (the damage-clip contract:
    /// undamaged pixels stay valid); initial content is opaque black.
    ///
    /// # Errors
    /// [`GlesError::Failed`] on allocation failure; [`GlesError::Invalid`]
    /// for a zero axis.
    fn create_target(&mut self, width: u32, height: u32) -> Result<GlesTarget, GlesError>;

    /// Destroy a render target.
    ///
    /// # Errors
    /// [`GlesError::BadHandle`] for an unknown target.
    fn destroy_target(&mut self, target: GlesTarget) -> Result<(), GlesError>;

    /// Upload one layer texture: `rgba` holds `width * height`
    /// premultiplied RGBA pixels, row-major, tightly packed.
    ///
    /// # Errors
    /// [`GlesError::Invalid`] when the payload size does not match the
    /// dimensions (or an axis is zero); [`GlesError::Failed`] on
    /// allocation failure.
    fn create_texture(
        &mut self,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) -> Result<GlesTexture, GlesError>;

    /// Destroy a texture.
    ///
    /// # Errors
    /// [`GlesError::BadHandle`] for an unknown texture.
    fn destroy_texture(&mut self, texture: GlesTexture) -> Result<(), GlesError>;

    /// Bind an imported EGLImage as a texture — the DMA-BUF
    /// zero-copy path (Phase 31): the pixels never cross the CPU;
    /// the image's own planes scan out through the sampler. The
    /// `image` value is the EGLImage handle (the EGL side's opaque
    /// pointer as u64 — the seam stays loose-typed at this trait,
    /// typed at the real backend).
    ///
    /// # Errors
    /// [`GlesError::Failed`] when the binding is rejected (a dead
    /// image, a missing extension); [`GlesError::Invalid`] on the
    /// reference evaluator — an imported image has no CPU-side words
    /// to blend, so the honest reference backend refuses it (the
    /// command stream and the real hardware carry the truth).
    fn create_texture_from_egl_image(&mut self, image: u64) -> Result<GlesTexture, GlesError>;

    /// Open a pass on `target`, clipping all `clear`/`draw_layer`
    /// writes to `scissor` intersected with the target bounds
    /// (`None` = the whole target).
    ///
    /// # Errors
    /// [`GlesError::BadHandle`] for an unknown target;
    /// [`GlesError::Invalid`] when a pass is already open or the
    /// scissor is empty/invalid (an empty scissor is a no-op pass the
    /// caller should have skipped — it names a logic bug).
    fn begin_pass(&mut self, target: GlesTarget, scissor: Option<Rect>) -> Result<(), GlesError>;

    /// Clear the pass's scissor region to the premultiplied color.
    ///
    /// # Errors
    /// [`GlesError::Invalid`] without an open pass.
    fn clear(&mut self, r: u8, g: u8, b: u8, a: u8) -> Result<(), GlesError>;

    /// Draw `texture` at `dst` (top-down pixels, clipped by the pass
    /// scissor and target bounds), blending premultiplied source-over
    /// with `opacity` (`0..=255`) scaling the source first.
    ///
    /// # Errors
    /// [`GlesError::BadHandle`] for an unknown texture;
    /// [`GlesError::Invalid`] without an open pass.
    fn draw_layer(&mut self, texture: GlesTexture, dst: Rect, opacity: u8)
        -> Result<(), GlesError>;

    /// Close the pass.
    ///
    /// # Errors
    /// [`GlesError::Invalid`] without an open pass.
    fn end_pass(&mut self) -> Result<(), GlesError>;

    /// Read a target back as top-down RGBA rows into `rgba_out`
    /// (length must be `width * height * 4` of the target's creation
    /// size). Does not require an open pass.
    ///
    /// # Errors
    /// [`GlesError::BadHandle`] for an unknown target;
    /// [`GlesError::Invalid`] on a size mismatch or a pass open on a
    /// different target.
    fn readback(&mut self, target: GlesTarget, rgba_out: &mut [u8]) -> Result<(), GlesError>;
}
