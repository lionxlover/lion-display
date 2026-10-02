//! The audited GLES symbol layer: context + entry points.
//!
//! One struct, [`GlesContext`], owns the whole hardware story: the
//! dlopen'd `libEGL.so.1` (through the existing [`crate::egl::sys`]
//! layer), the initialized display, an RGBA8888 pbuffer-capable
//! config, a GLES 2.0 context made current on the calling thread, and
//! the `gl*` function table resolved through `eglGetProcAddress` (the
//! GLES platform's own contract for core entry points). Dropping it
//! unbinds, destroys, and terminates in the correct order.
//!
//! Safety doctrine (the `ldp-transport`/`ldp-display` precedent):
//! every `unsafe` in the GLES tree lives in this module; each call
//! site carries a `SAFETY` comment; the renderer-facing layer
//! ([`super::api`]) stays `#![forbid(unsafe_code)]`.

use std::ffi::CStr;

use crate::egl::sys::{DisplayHandle, EglContext, EglError, EglSurface, LibEgl};

/// The GL layer's failures (context bring-up and symbol resolution).
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum GlesContextError {
    /// The EGL layer failed; carries its typed reason.
    Egl(EglError),
    /// `eglGetProcAddress` returned NULL for a core entry point
    /// (a broken GLES platform).
    MissingEntry {
        /// The entry point name.
        symbol: &'static str,
    },
}

impl core::fmt::Display for GlesContextError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Egl(e) => write!(f, "EGL: {e}"),
            Self::MissingEntry { symbol } => {
                write!(f, "eglGetProcAddress found no {symbol}")
            }
        }
    }
}

impl std::error::Error for GlesContextError {}

impl From<EglError> for GlesContextError {
    fn from(e: EglError) -> Self {
        Self::Egl(e)
    }
}

/// The GL function table. All entry points are core GLES 2.0.
#[allow(non_snake_case)]
#[derive(Clone, Copy)]
pub(crate) struct GlFns {
    pub(crate) glGetError: unsafe extern "C" fn() -> u32,
    /// `glEGLImageTargetTexture2DOES` — the EGLImage-to-texture
    /// binding (the DMA-BUF zero-copy path's one call; the 50th
    /// audited entry point).
    pub(crate) glEGLImageTargetTexture2DOES: unsafe extern "C" fn(u32, *const core::ffi::c_void),
    pub(crate) glActiveTexture: unsafe extern "C" fn(u32),
    pub(crate) glBindTexture: unsafe extern "C" fn(u32, u32),
    pub(crate) glDeleteTextures: unsafe extern "C" fn(i32, *const u32),
    pub(crate) glGenTextures: unsafe extern "C" fn(i32, *mut u32),
    pub(crate) glTexParameteri: unsafe extern "C" fn(u32, u32, i32),
    pub(crate) glTexImage2D:
        unsafe extern "C" fn(u32, i32, i32, i32, i32, i32, u32, u32, *const core::ffi::c_void),
    pub(crate) glCreateShader: unsafe extern "C" fn(u32) -> u32,
    pub(crate) glShaderSource: unsafe extern "C" fn(u32, i32, *const *const u8, *const i32),
    pub(crate) glCompileShader: unsafe extern "C" fn(u32),
    pub(crate) glGetShaderiv: unsafe extern "C" fn(u32, u32, *mut i32),
    pub(crate) glGetShaderInfoLog: unsafe extern "C" fn(u32, i32, *mut i32, *mut u8),
    pub(crate) glDeleteShader: unsafe extern "C" fn(u32),
    pub(crate) glCreateProgram: unsafe extern "C" fn() -> u32,
    pub(crate) glAttachShader: unsafe extern "C" fn(u32, u32),
    pub(crate) glLinkProgram: unsafe extern "C" fn(u32),
    pub(crate) glGetProgramiv: unsafe extern "C" fn(u32, u32, *mut i32),
    pub(crate) glGetProgramInfoLog: unsafe extern "C" fn(u32, i32, *mut i32, *mut u8),
    pub(crate) glDeleteProgram: unsafe extern "C" fn(u32),
    pub(crate) glUseProgram: unsafe extern "C" fn(u32),
    pub(crate) glBindAttribLocation: unsafe extern "C" fn(u32, u32, *const u8),
    pub(crate) glGetUniformLocation: unsafe extern "C" fn(u32, *const u8) -> i32,
    pub(crate) glUniform1f: unsafe extern "C" fn(i32, f32),
    pub(crate) glUniform1i: unsafe extern "C" fn(i32, i32),
    pub(crate) glUniform2f: unsafe extern "C" fn(i32, f32, f32),
    pub(crate) glUniform4f: unsafe extern "C" fn(i32, f32, f32, f32, f32),
    pub(crate) glGenFramebuffers: unsafe extern "C" fn(i32, *mut u32),
    pub(crate) glDeleteFramebuffers: unsafe extern "C" fn(i32, *const u32),
    pub(crate) glBindFramebuffer: unsafe extern "C" fn(u32, u32),
    pub(crate) glFramebufferTexture2D: unsafe extern "C" fn(u32, u32, u32, u32, i32),
    pub(crate) glCheckFramebufferStatus: unsafe extern "C" fn(u32) -> u32,
    pub(crate) glViewport: unsafe extern "C" fn(i32, i32, i32, i32),
    pub(crate) glScissor: unsafe extern "C" fn(i32, i32, i32, i32),
    pub(crate) glEnable: unsafe extern "C" fn(u32),
    pub(crate) glDisable: unsafe extern "C" fn(u32),
    pub(crate) glClearColor: unsafe extern "C" fn(f32, f32, f32, f32),
    pub(crate) glClear: unsafe extern "C" fn(u32),
    pub(crate) glBlendFunc: unsafe extern "C" fn(u32, u32),
    pub(crate) glBlendEquation: unsafe extern "C" fn(u32),
    pub(crate) glDrawArrays: unsafe extern "C" fn(u32, i32, i32),
    pub(crate) glReadPixels:
        unsafe extern "C" fn(i32, i32, i32, i32, u32, u32, *mut core::ffi::c_void),
    pub(crate) glGenBuffers: unsafe extern "C" fn(i32, *mut u32),
    pub(crate) glDeleteBuffers: unsafe extern "C" fn(i32, *const u32),
    pub(crate) glBindBuffer: unsafe extern "C" fn(u32, u32),
    pub(crate) glBufferData: unsafe extern "C" fn(u32, isize, *const core::ffi::c_void, u32),
    pub(crate) glVertexAttribPointer:
        unsafe extern "C" fn(u32, i32, u32, bool, i32, *const core::ffi::c_void),
    pub(crate) glEnableVertexAttribArray: unsafe extern "C" fn(u32),
    pub(crate) glDisableVertexAttribArray: unsafe extern "C" fn(u32),
    pub(crate) glGetString: unsafe extern "C" fn(u32) -> *const u8,
}

/// GLES 2.0 constants (the subset this backend uses).
pub mod gl {
    /// `GL_TEXTURE_2D`.
    pub const TEXTURE_2D: u32 = 0x0DE1;
    /// `GL_TEXTURE_MIN_FILTER`.
    pub const TEXTURE_MIN_FILTER: u32 = 0x2801;
    /// `GL_TEXTURE_MAG_FILTER`.
    pub const TEXTURE_MAG_FILTER: u32 = 0x2800;
    /// `GL_TEXTURE_WRAP_S`.
    pub const TEXTURE_WRAP_S: u32 = 0x2802;
    /// `GL_TEXTURE_WRAP_T`.
    pub const TEXTURE_WRAP_T: u32 = 0x2803;
    /// `GL_NEAREST` (min/mag filter value).
    pub const NEAREST: i32 = 0x2600;
    /// `GL_CLAMP_TO_EDGE`.
    pub const CLAMP_TO_EDGE: i32 = 0x812F;
    /// `GL_RGBA` (internal format and pixel format).
    pub const RGBA: i32 = 4;
    /// `GL_RGBA` (pixel-format enum value).
    pub const RGBA_ENUM: u32 = 0x1908;
    /// `GL_UNSIGNED_BYTE`.
    pub const UNSIGNED_BYTE: u32 = 0x1401;
    /// `GL_VERTEX_SHADER`.
    pub const VERTEX_SHADER: u32 = 0x8B31;
    /// `GL_FRAGMENT_SHADER`.
    pub const FRAGMENT_SHADER: u32 = 0x8B30;
    /// `GL_COMPILE_STATUS`.
    pub const COMPILE_STATUS: u32 = 0x8B81;
    /// `GL_LINK_STATUS`.
    pub const LINK_STATUS: u32 = 0x8B82;
    /// `GL_FRAMEBUFFER`.
    pub const FRAMEBUFFER: u32 = 0x8D40;
    /// `GL_COLOR_ATTACHMENT0`.
    pub const COLOR_ATTACHMENT0: u32 = 0x8CE0;
    /// `GL_FRAMEBUFFER_COMPLETE`.
    pub const FRAMEBUFFER_COMPLETE: u32 = 0x8CD5;
    /// `GL_TEXTURE0`.
    pub const TEXTURE0: u32 = 0x84C0;
    /// `GL_SCISSOR_TEST`.
    pub const SCISSOR_TEST: u32 = 0x0C11;
    /// `GL_BLEND`.
    pub const BLEND: u32 = 0x0BE2;
    /// `GL_ONE`.
    pub const ONE: u32 = 1;
    /// `GL_ONE_MINUS_SRC_ALPHA`.
    pub const ONE_MINUS_SRC_ALPHA: u32 = 0x0303;
    /// `GL_FUNC_ADD`.
    pub const FUNC_ADD: u32 = 0x8006;
    /// `GL_COLOR_BUFFER_BIT`.
    pub const COLOR_BUFFER_BIT: u32 = 0x4000;
    /// `GL_TRIANGLE_STRIP`.
    pub const TRIANGLE_STRIP: u32 = 0x0005;
    /// `GL_ARRAY_BUFFER`.
    pub const ARRAY_BUFFER: u32 = 0x8892;
    /// `GL_FLOAT`.
    pub const FLOAT: u32 = 0x1406;
    /// `GL_STATIC_DRAW`.
    pub const STATIC_DRAW: u32 = 0x88E4;
    /// `GL_STREAM_DRAW`.
    pub const STREAM_DRAW: u32 = 0x88E0;
    /// `GL_NO_ERROR`.
    pub const NO_ERROR: u32 = 0;
    /// `GL_VENDOR` (the `glGetString` name).
    pub const VENDOR: u32 = 0x1F00;
    /// `GL_RENDERER` (the `glGetString` name).
    pub const RENDERER: u32 = 0x1F01;
    /// `GL_VERSION` (the `glGetString` name).
    pub const VERSION: u32 = 0x1F02;
}

/// The names of every entry point, in table order (diagnostics and
/// the symbol-count integrity test).
const ENTRY_POINTS: &[&str] = &[
    "glGetError",
    "glActiveTexture",
    "glBindTexture",
    "glDeleteTextures",
    "glGenTextures",
    "glTexParameteri",
    "glTexImage2D",
    "glCreateShader",
    "glShaderSource",
    "glCompileShader",
    "glGetShaderiv",
    "glGetShaderInfoLog",
    "glDeleteShader",
    "glCreateProgram",
    "glAttachShader",
    "glLinkProgram",
    "glGetProgramiv",
    "glGetProgramInfoLog",
    "glDeleteProgram",
    "glUseProgram",
    "glBindAttribLocation",
    "glGetUniformLocation",
    "glUniform1f",
    "glUniform1i",
    "glUniform2f",
    "glUniform4f",
    "glGenFramebuffers",
    "glDeleteFramebuffers",
    "glBindFramebuffer",
    "glFramebufferTexture2D",
    "glCheckFramebufferStatus",
    "glViewport",
    "glScissor",
    "glEnable",
    "glDisable",
    "glClearColor",
    "glClear",
    "glBlendFunc",
    "glBlendEquation",
    "glDrawArrays",
    "glReadPixels",
    "glGenBuffers",
    "glDeleteBuffers",
    "glBindBuffer",
    "glBufferData",
    "glVertexAttribPointer",
    "glEnableVertexAttribArray",
    "glDisableVertexAttribArray",
    "glGetString",
    "glEGLImageTargetTexture2DOES",
];

/// How many entry points the table carries.
#[must_use]
pub fn entry_point_count() -> usize {
    ENTRY_POINTS.len()
}

/// A live GLES 2.0 context.
///
/// Construction: dlopen libEGL, bootstrap the display, bind the ES
/// API, choose the RGBA8888 pbuffer config, create a 1x1 pbuffer and
/// a GLES 2.0 context, make it current, resolve the `gl*` table. Any
/// step failing is typed — the honest headless outcome.
pub struct GlesContext {
    lib: LibEgl,
    display: DisplayHandle,
    surface: EglSurface,
    context: EglContext,
    fns: GlFns,
}

// The context is bound to the creating thread by EGL contract; the
// backend drives it single-threaded (the compositor's session world
// is one thread), so Send is accurate for the ownership move; Sync is
// not claimed.
unsafe impl Send for GlesContext {}

impl core::fmt::Debug for GlesContext {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("GlesContext")
            .field("display", &self.display.is_null())
            .field("entry_points", &ENTRY_POINTS.len())
            .finish_non_exhaustive()
    }
}

impl GlesContext {
    /// Bring the whole context up on the calling thread.
    ///
    /// # Errors
    /// [`GlesContextError::Egl`] for every EGL-side failure (absent
    /// library included); [`GlesContextError::MissingEntry`] for a
    /// broken GLES platform.
    pub fn new() -> Result<Self, GlesContextError> {
        let lib = LibEgl::open()?;
        let boot = lib.bootstrap()?;
        lib.bind_gles_api()?;
        let config = lib.choose_rgba_config(boot.display)?;
        let surface = lib.create_pbuffer(boot.display, config, 1, 1)?;
        let context = lib.create_gles2_context(boot.display, config)?;
        lib.make_current(boot.display, surface, surface, context)?;
        let fns = Self::resolve(&lib)?;
        Ok(Self {
            lib,
            display: boot.display,
            surface,
            context,
            fns,
        })
    }

    /// Resolve the `gl*` table through `eglGetProcAddress`.
    ///
    /// # Safety
    /// The library must be live; the transmuted prototypes must match
    /// the published GLES 2.0 ABI (they do — the table is fixed).
    #[allow(clippy::too_many_lines)] // one audited entry point per line is the shape that matters
    fn resolve(lib: &LibEgl) -> Result<GlFns, GlesContextError> {
        macro_rules! entry {
            ($field:ident, $sig:ty, $name:literal) => {{
                // SAFETY: the name literal is NUL-terminated via CStr;
                // eglGetProcAddress only reads it for the call.
                let ptr = unsafe {
                    lib.proc_address(
                        CStr::from_bytes_with_nul(concat!($name, "\0").as_bytes())
                            .map_err(|_| GlesContextError::MissingEntry { symbol: $name })?,
                    )
                };
                let Some(ptr) = ptr else {
                    return Err(GlesContextError::MissingEntry { symbol: $name });
                };
                // SAFETY: the entry point belongs to a live GLES
                // platform and the prototype matches the published ABI.
                unsafe { core::mem::transmute::<usize, $sig>(ptr) }
            }};
        }
        let fns = GlFns {
            glGetError: entry!(glGetError, unsafe extern "C" fn() -> u32, "glGetError"),
            glActiveTexture: entry!(
                glActiveTexture,
                unsafe extern "C" fn(u32),
                "glActiveTexture"
            ),
            glBindTexture: entry!(
                glBindTexture,
                unsafe extern "C" fn(u32, u32),
                "glBindTexture"
            ),
            glDeleteTextures: entry!(
                glDeleteTextures,
                unsafe extern "C" fn(i32, *const u32),
                "glDeleteTextures"
            ),
            glGenTextures: entry!(
                glGenTextures,
                unsafe extern "C" fn(i32, *mut u32),
                "glGenTextures"
            ),
            glTexParameteri: entry!(
                glTexParameteri,
                unsafe extern "C" fn(u32, u32, i32),
                "glTexParameteri"
            ),
            glTexImage2D: entry!(
                glTexImage2D,
                unsafe extern "C" fn(
                    u32,
                    i32,
                    i32,
                    i32,
                    i32,
                    i32,
                    u32,
                    u32,
                    *const core::ffi::c_void,
                ),
                "glTexImage2D"
            ),
            glCreateShader: entry!(
                glCreateShader,
                unsafe extern "C" fn(u32) -> u32,
                "glCreateShader"
            ),
            glShaderSource: entry!(
                glShaderSource,
                unsafe extern "C" fn(u32, i32, *const *const u8, *const i32),
                "glShaderSource"
            ),
            glCompileShader: entry!(
                glCompileShader,
                unsafe extern "C" fn(u32),
                "glCompileShader"
            ),
            glGetShaderiv: entry!(
                glGetShaderiv,
                unsafe extern "C" fn(u32, u32, *mut i32),
                "glGetShaderiv"
            ),
            glGetShaderInfoLog: entry!(
                glGetShaderInfoLog,
                unsafe extern "C" fn(u32, i32, *mut i32, *mut u8),
                "glGetShaderInfoLog"
            ),
            glDeleteShader: entry!(glDeleteShader, unsafe extern "C" fn(u32), "glDeleteShader"),
            glCreateProgram: entry!(
                glCreateProgram,
                unsafe extern "C" fn() -> u32,
                "glCreateProgram"
            ),
            glAttachShader: entry!(
                glAttachShader,
                unsafe extern "C" fn(u32, u32),
                "glAttachShader"
            ),
            glLinkProgram: entry!(glLinkProgram, unsafe extern "C" fn(u32), "glLinkProgram"),
            glGetProgramiv: entry!(
                glGetProgramiv,
                unsafe extern "C" fn(u32, u32, *mut i32),
                "glGetProgramiv"
            ),
            glGetProgramInfoLog: entry!(
                glGetProgramInfoLog,
                unsafe extern "C" fn(u32, i32, *mut i32, *mut u8),
                "glGetProgramInfoLog"
            ),
            glDeleteProgram: entry!(
                glDeleteProgram,
                unsafe extern "C" fn(u32),
                "glDeleteProgram"
            ),
            glUseProgram: entry!(glUseProgram, unsafe extern "C" fn(u32), "glUseProgram"),
            glBindAttribLocation: entry!(
                glBindAttribLocation,
                unsafe extern "C" fn(u32, u32, *const u8),
                "glBindAttribLocation"
            ),
            glGetUniformLocation: entry!(
                glGetUniformLocation,
                unsafe extern "C" fn(u32, *const u8) -> i32,
                "glGetUniformLocation"
            ),
            glUniform1f: entry!(glUniform1f, unsafe extern "C" fn(i32, f32), "glUniform1f"),
            glUniform1i: entry!(glUniform1i, unsafe extern "C" fn(i32, i32), "glUniform1i"),
            glUniform2f: entry!(
                glUniform2f,
                unsafe extern "C" fn(i32, f32, f32),
                "glUniform2f"
            ),
            glUniform4f: entry!(
                glUniform4f,
                unsafe extern "C" fn(i32, f32, f32, f32, f32),
                "glUniform4f"
            ),
            glGenFramebuffers: entry!(
                glGenFramebuffers,
                unsafe extern "C" fn(i32, *mut u32),
                "glGenFramebuffers"
            ),
            glDeleteFramebuffers: entry!(
                glDeleteFramebuffers,
                unsafe extern "C" fn(i32, *const u32),
                "glDeleteFramebuffers"
            ),
            glBindFramebuffer: entry!(
                glBindFramebuffer,
                unsafe extern "C" fn(u32, u32),
                "glBindFramebuffer"
            ),
            glFramebufferTexture2D: entry!(
                glFramebufferTexture2D,
                unsafe extern "C" fn(u32, u32, u32, u32, i32),
                "glFramebufferTexture2D"
            ),
            glCheckFramebufferStatus: entry!(
                glCheckFramebufferStatus,
                unsafe extern "C" fn(u32) -> u32,
                "glCheckFramebufferStatus"
            ),
            glViewport: entry!(
                glViewport,
                unsafe extern "C" fn(i32, i32, i32, i32),
                "glViewport"
            ),
            glScissor: entry!(
                glScissor,
                unsafe extern "C" fn(i32, i32, i32, i32),
                "glScissor"
            ),
            glEnable: entry!(glEnable, unsafe extern "C" fn(u32), "glEnable"),
            glDisable: entry!(glDisable, unsafe extern "C" fn(u32), "glDisable"),
            glClearColor: entry!(
                glClearColor,
                unsafe extern "C" fn(f32, f32, f32, f32),
                "glClearColor"
            ),
            glClear: entry!(glClear, unsafe extern "C" fn(u32), "glClear"),
            glBlendFunc: entry!(glBlendFunc, unsafe extern "C" fn(u32, u32), "glBlendFunc"),
            glBlendEquation: entry!(
                glBlendEquation,
                unsafe extern "C" fn(u32),
                "glBlendEquation"
            ),
            glDrawArrays: entry!(
                glDrawArrays,
                unsafe extern "C" fn(u32, i32, i32),
                "glDrawArrays"
            ),
            glReadPixels: entry!(
                glReadPixels,
                unsafe extern "C" fn(i32, i32, i32, i32, u32, u32, *mut core::ffi::c_void),
                "glReadPixels"
            ),
            glGenBuffers: entry!(
                glGenBuffers,
                unsafe extern "C" fn(i32, *mut u32),
                "glGenBuffers"
            ),
            glDeleteBuffers: entry!(
                glDeleteBuffers,
                unsafe extern "C" fn(i32, *const u32),
                "glDeleteBuffers"
            ),
            glBindBuffer: entry!(glBindBuffer, unsafe extern "C" fn(u32, u32), "glBindBuffer"),
            glBufferData: entry!(
                glBufferData,
                unsafe extern "C" fn(u32, isize, *const core::ffi::c_void, u32),
                "glBufferData"
            ),
            glVertexAttribPointer: entry!(
                glVertexAttribPointer,
                unsafe extern "C" fn(u32, i32, u32, bool, i32, *const core::ffi::c_void),
                "glVertexAttribPointer"
            ),
            glEnableVertexAttribArray: entry!(
                glEnableVertexAttribArray,
                unsafe extern "C" fn(u32),
                "glEnableVertexAttribArray"
            ),
            glDisableVertexAttribArray: entry!(
                glDisableVertexAttribArray,
                unsafe extern "C" fn(u32),
                "glDisableVertexAttribArray"
            ),
            glGetString: entry!(
                glGetString,
                unsafe extern "C" fn(u32) -> *const u8,
                "glGetString"
            ),
            glEGLImageTargetTexture2DOES: entry!(
                glEGLImageTargetTexture2DOES,
                unsafe extern "C" fn(u32, *const core::ffi::c_void),
                "glEGLImageTargetTexture2DOES"
            ),
        };
        Ok(fns)
    }

    /// The function table (crate-internal: the api layer's calls).
    #[must_use]
    pub(crate) const fn fns(&self) -> &GlFns {
        &self.fns
    }

    /// The EGL library handle (teardown paths in the api layer).
    #[must_use]
    pub(crate) const fn lib(&self) -> &LibEgl {
        &self.lib
    }

    /// The display handle.
    #[must_use]
    pub(crate) const fn display(&self) -> DisplayHandle {
        self.display
    }

    /// The pbuffer surface handle (the api layer's current-binding).
    #[must_use]
    pub(crate) const fn surface(&self) -> EglSurface {
        self.surface
    }

    /// The GLES context handle.
    #[must_use]
    pub(crate) const fn context(&self) -> EglContext {
        self.context
    }

    /// Query one `glGetString` name (vendor, renderer, version) as a
    /// lossless Rust string; empty when the platform answers NULL.
    ///
    /// Core GLES 2.0 entry point, valid on any current context.
    ///
    /// # Panics
    /// Never: a NULL answer maps to the empty string (the honest
    /// "the platform said nothing" — classification treats it as
    /// unknown, never as an error).
    #[must_use]
    pub fn string_name(&self, name: u32) -> String {
        // SAFETY: the entry point is a core GLES 2.0 symbol resolved
        // against a live, current context; the returned pointer, when
        // non-NULL, is a NUL-terminated static string owned by the GL
        // platform for the context's lifetime (the GLES 2.0 spec's
        // own contract for glGetString), and it is only read here.
        let ptr = unsafe { (self.fns.glGetString)(name) };
        if ptr.is_null() {
            return String::new();
        }
        // SAFETY: the pointer is NUL-terminated platform-owned memory
        // (the contract above); CStr::from_ptr only walks it to the
        // terminator, and the bytes are copied out before return.
        unsafe { CStr::from_ptr(ptr.cast()) }
            .to_string_lossy()
            .into_owned()
    }
}

impl Drop for GlesContext {
    fn drop(&mut self) {
        // Unbind before destroying: EGL requires the context not be
        // current on any thread at destruction. Failure here is not
        // actionable (teardown on a best-effort basis, by policy).
        let _ = self.lib.make_current(
            self.display,
            EglSurface::null(),
            EglSurface::null(),
            EglContext::null(),
        );
        let _ = self.lib.destroy_context(self.display, self.context);
        let _ = self.lib.destroy_surface(self.display, self.surface);
        let _ = self.lib.terminate(self.display);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_table_is_sized() {
        // 50 core entry points, all distinct (48 at Phase 24, the
        // `glGetString`, the Phase 30 identity read).
        // glGetString at Phase 30, glEGLImageTargetTexture2DOES at
        // Phase 31 — the zero-copy binding).
        assert_eq!(entry_point_count(), 50);
        let mut names = ENTRY_POINTS.to_vec();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), ENTRY_POINTS.len());
    }
}
