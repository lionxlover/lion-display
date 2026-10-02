//! [`RealGles`] — the [`GlesApi`] implementation over a real EGL+GLES
//! context.
//!
//! Threading contract, stated once: the compositor's world mutex
//! serializes every renderer call, but the calling *thread* varies
//! (each session pumps under the lock). An EGL context can only be
//! current on one thread at a time, so this backend re-binds the
//! context on every API call — `eglBindAPI` + `eglMakeCurrent`, work,
//! `eglMakeCurrent(null)` — which is race-free under the caller's
//! serialization and costs microseconds against a 16 ms frame budget.
//! A dedicated render thread with one long-lived binding is the next
//! GL milestone (`docs/roadmap.md`).
//!
//! Real-hardware honesty: GL blending runs in float and may differ
//! from the integer reference evaluator by ±1 LSB; the command stream
//! is identical (pinned by `ldp-renderer`'s stream goldens against
//! the recorder), the reference evaluator is the conformance oracle.

// Safety doctrine: unlike the pure layers, this module is one of the
// crate's audited call-site layers (with `sys`): every `unsafe` block
// below is a raw GLES entry-point call carrying its `SAFETY` note,
// mirroring the `SAFETY`-commented blocks the transport/display
// crates keep in their own `sys` seams.

use std::collections::BTreeMap;

use ldp_core::geometry::Rect;
use ldp_renderer::gles::api::{GlesApi, GlesError, GlesProgram, GlesTarget, GlesTexture};

use super::sys::{gl, GlesContext};

/// One linked program with its uniform locations.
#[derive(Clone)]
struct ProgramState {
    raw: u32,
    rect: i32,
    size: i32,
    opacity: i32,
    tex: i32,
}

/// One render target: the FBO's GL name plus its extent.
#[derive(Clone, Copy)]
struct TargetState {
    /// The GL framebuffer name (what glBindFramebuffer takes).
    fbo: u32,
    width: u32,
    height: u32,
}

/// One texture: the GL name (the extent was validated against the
/// payload at creation; the API contract carries it per call).
#[derive(Clone, Copy)]
struct TextureState {
    /// The GL texture name (what glBindTexture takes).
    gl_name: u32,
}

/// The pass state: which FBO is bound and its extent.
struct PassState {
    width: u32,
    height: u32,
}

/// The real hardware backend.
pub struct RealGles {
    ctx: GlesContext,
    programs: BTreeMap<u32, ProgramState>,
    targets: BTreeMap<u32, TargetState>,
    textures: BTreeMap<u32, TextureState>,
    pass: Option<PassState>,
    next_program: u32,
    next_texture: u32,
    next_target: u32,
    /// The unit-quad VBO (pos + uv, 4 vertices, static).
    quad: u32,
}

impl core::fmt::Debug for RealGles {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RealGles")
            .field("programs", &self.programs.len())
            .field("targets", &self.targets.len())
            .field("textures", &self.textures.len())
            .field("pass_open", &self.pass.is_some())
            .finish_non_exhaustive()
    }
}

impl RealGles {
    /// Bring the hardware backend up: dlopen, display, context, the
    /// entry-point table, and the shared unit-quad VBO.
    ///
    /// # Errors
    /// [`GlesError::Unavailable`] on every bring-up failure — the
    /// typed honest outcome the selection policy consumes (no GL
    /// stack included).
    pub fn new() -> Result<Self, GlesError> {
        let ctx = GlesContext::new().map_err(|e| GlesError::Unavailable(e.to_string()))?;
        let mut backend = Self {
            ctx,
            programs: BTreeMap::new(),
            targets: BTreeMap::new(),
            textures: BTreeMap::new(),
            pass: None,
            next_program: 1,
            next_texture: 1,
            next_target: 1,
            quad: 0,
        };
        // The unit quad: (pos.xy, uv.xy) per vertex, triangle strip.
        let quad: [f32; 16] = [
            0.0, 0.0, 0.0, 0.0, // bottom-left in unit space
            1.0, 0.0, 1.0, 0.0, // bottom-right
            0.0, 1.0, 0.0, 1.0, // top-left
            1.0, 1.0, 1.0, 1.0, // top-right
        ];
        let vbo = backend.with_current(|f| {
            let mut vbo: u32 = 0;
            // SAFETY: valid out-pointer; the context is current.
            unsafe { (f.glGenBuffers)(1, &mut vbo) };
            // SAFETY: vbo fresh; the quad borrow is live for the call.
            unsafe {
                (f.glBindBuffer)(gl::ARRAY_BUFFER, vbo);
                (f.glBufferData)(
                    gl::ARRAY_BUFFER,
                    quad.len() as isize,
                    quad.as_ptr().cast::<core::ffi::c_void>(),
                    gl::STATIC_DRAW,
                );
                (f.glBindBuffer)(gl::ARRAY_BUFFER, 0);
            }
            Ok(vbo)
        })?;
        backend.quad = backend.check_error("unit-quad VBO upload", vbo)?;
        Ok(backend)
    }

    /// The context's identity: the vendor/renderer/version strings
    /// plus the derived [`crate::classify::GpuClass`] (the desktop
    /// matrix doctrine — what kind of GPU answers behind this
    /// context).
    ///
    /// Pure reads of core GLES 2.0 state on an already-live context;
    /// a platform that answers NULL for any string gets the empty
    /// spelling, and an empty renderer classifies as `Unknown`
    /// (never a silent software degrade).
    #[must_use]
    pub fn identity(&self) -> crate::classify::GlIdentity {
        let vendor = self.ctx.string_name(gl::VENDOR);
        let renderer = self.ctx.string_name(gl::RENDERER);
        let version = self.ctx.string_name(gl::VERSION);
        crate::classify::GlIdentity::from_strings(&vendor, &renderer, &version)
    }

    /// Run `f` with the GL context current on this thread (the
    /// migration discipline above); unbinds afterwards.
    fn with_current<T>(
        &self,
        f: impl FnOnce(&super::sys::GlFns) -> Result<T, GlesError>,
    ) -> Result<T, GlesError> {
        let lib = self.ctx.lib();
        let display = self.ctx.display();
        // The rendering API is thread-local; bind it here.
        lib.bind_gles_api()
            .map_err(|e| GlesError::Unavailable(e.to_string()))?;
        // SAFETY: the surface/context handles are owned by
        // self.ctx and live for the duration; the caller serializes
        // all access (the world mutex).
        lib.make_current(
            display,
            self.ctx.surface(),
            self.ctx.surface(),
            self.ctx.context(),
        )
        .map_err(|e| GlesError::Failed(format!("eglMakeCurrent: {e}")))?;
        let out = f(self.ctx.fns());
        // SAFETY: unbinding with null handles; always attempted.
        let _ = lib.make_current(
            display,
            crate::egl::sys::EglSurface::null(),
            crate::egl::sys::EglSurface::null(),
            crate::egl::sys::EglContext::null(),
        );
        out
    }

    /// Drain and classify the GL error state.
    fn check_error<T>(&self, while_doing: &'static str, value: T) -> Result<T, GlesError> {
        // SAFETY: pure query; the context is current.
        let code = unsafe { (self.ctx.fns().glGetError)() };
        if code == gl::NO_ERROR {
            return Ok(value);
        }
        let mut first = code;
        // SAFETY: drain the error queue (bounded by spec: at most a
        // few sticky flags).
        for _ in 0..8 {
            let next = unsafe { (self.ctx.fns().glGetError)() };
            if next == gl::NO_ERROR {
                break;
            }
            first = next;
        }
        Err(GlesError::Failed(format!(
            "{while_doing} raised GL error {first:#06x}"
        )))
    }

    /// Compile one shader; returns its handle or the driver's log.
    fn compile_shader(f: &super::sys::GlFns, kind: u32, src: &str) -> Result<u32, GlesError> {
        // SAFETY: pure create; the context is current.
        let shader = unsafe { (f.glCreateShader)(kind) };
        if shader == 0 {
            return Err(GlesError::Failed("glCreateShader returned 0".into()));
        }
        let ptr = src.as_ptr();
        let len = i32::try_from(src.len())
            .map_err(|_| GlesError::Invalid("shader source overflows the GLsizei space".into()))?;
        // SAFETY: `src` outlives the call (the caller's borrow);
        // explicit length, no NUL requirement.
        unsafe { (f.glShaderSource)(shader, 1, [ptr].as_ptr(), [len].as_ptr()) };
        // SAFETY: the shader handle is fresh.
        unsafe { (f.glCompileShader)(shader) };
        let mut status: i32 = 0;
        // SAFETY: valid out-pointer.
        unsafe { (f.glGetShaderiv)(shader, gl::COMPILE_STATUS, &mut status) };
        if status == 0 {
            let mut log = [0u8; 1024];
            let mut written: i32 = 0;
            // SAFETY: the log buffer is a valid 1 KiB borrow.
            unsafe {
                (f.glGetShaderInfoLog)(shader, log.len() as i32, &mut written, log.as_mut_ptr());
            };
            let text = String::from_utf8_lossy(&log[..written.max(0) as usize]).into_owned();
            // SAFETY: teardown of the failed shader.
            unsafe { (f.glDeleteShader)(shader) };
            return Err(GlesError::Failed(format!("shader compile failed: {text}")));
        }
        Ok(shader)
    }
}

impl Drop for RealGles {
    fn drop(&mut self) {
        // Best-effort teardown under the migration discipline; the
        // GlesContext's own Drop destroys the context itself.
        let programs: Vec<u32> = self.programs.values().map(|p| p.raw).collect();
        let targets: Vec<u32> = self.targets.values().map(|t| t.fbo).collect();
        let textures: Vec<u32> = self.textures.values().map(|t| t.gl_name).collect();
        let quad = self.quad;
        let _ = self.with_current(|f| {
            for p in programs {
                // SAFETY: owned program names collected above.
                unsafe { (f.glDeleteProgram)(p) };
            }
            for t in targets {
                // SAFETY: owned FBO names collected above.
                unsafe { (f.glDeleteFramebuffers)(1, &t) };
            }
            for t in textures {
                // SAFETY: owned texture names collected above.
                unsafe { (f.glDeleteTextures)(1, &t) };
            }
            // SAFETY: the shared quad VBO.
            unsafe { (f.glDeleteBuffers)(1, &quad) };
            Ok(())
        });
    }
}

impl Default for RealGles {
    /// Panics without hardware: the backend is fallible by nature and
    /// `default` exists only for generic contexts. No caller in this
    /// workspace uses it; the API's users go through [`RealGles::new`].
    ///
    /// # Panics
    ///
    /// When the machine has no GL stack (the typed [`RealGles::new`]
    /// failure is the designed path instead).
    fn default() -> Self {
        Self::new().expect("RealGles::default requires a GL stack")
    }
}

impl GlesApi for RealGles {
    #[allow(clippy::too_many_lines)] // one audited GL step per line is the shape that matters
    fn create_program(&mut self, vs: &str, fs: &str) -> Result<GlesProgram, GlesError> {
        if self.pass.is_some() {
            return Err(GlesError::Invalid("create_program with a pass open".into()));
        }
        let raw_program = self.with_current(|f| {
            let vsh = Self::compile_shader(f, gl::VERTEX_SHADER, vs)?;
            let fsh = Self::compile_shader(f, gl::FRAGMENT_SHADER, fs)?;
            // SAFETY: pure create.
            let program = unsafe { (f.glCreateProgram)() };
            if program == 0 {
                return Err(GlesError::Failed("glCreateProgram returned 0".into()));
            }
            // SAFETY: both handles are fresh and owned here.
            unsafe {
                (f.glAttachShader)(program, vsh);
                (f.glAttachShader)(program, fsh);
            }
            // Bind the attribute slots before linking (a_pos=0, a_uv=1)
            // so every program shares one VBO layout.
            // SAFETY: NUL-terminated literals; program is fresh.
            unsafe {
                (f.glBindAttribLocation)(program, 0, c"a_pos".as_ptr().cast::<u8>());
                (f.glBindAttribLocation)(program, 1, c"a_uv".as_ptr().cast::<u8>());
            }
            // SAFETY: program is fresh.
            unsafe { (f.glLinkProgram)(program) };
            // The shaders are spent once linked.
            // SAFETY: teardown of the now-attached shaders.
            unsafe {
                (f.glDeleteShader)(vsh);
                (f.glDeleteShader)(fsh);
            }
            let mut status: i32 = 0;
            // SAFETY: valid out-pointer.
            unsafe { (f.glGetProgramiv)(program, gl::LINK_STATUS, &mut status) };
            if status == 0 {
                let mut log = [0u8; 1024];
                let mut written: i32 = 0;
                // SAFETY: valid log borrow.
                unsafe {
                    (f.glGetProgramInfoLog)(
                        program,
                        log.len() as i32,
                        &mut written,
                        log.as_mut_ptr(),
                    );
                };
                let text = String::from_utf8_lossy(&log[..written.max(0) as usize]).into_owned();
                // SAFETY: teardown of the failed program.
                unsafe { (f.glDeleteProgram)(program) };
                return Err(GlesError::Failed(format!("program link failed: {text}")));
            }
            // SAFETY: NUL-terminated literals; program linked.
            let rect =
                unsafe { (f.glGetUniformLocation)(program, c"u_rect".as_ptr().cast::<u8>()) };
            let size =
                unsafe { (f.glGetUniformLocation)(program, c"u_size".as_ptr().cast::<u8>()) };
            let opacity =
                unsafe { (f.glGetUniformLocation)(program, c"u_opacity".as_ptr().cast::<u8>()) };
            let tex = unsafe { (f.glGetUniformLocation)(program, c"u_tex".as_ptr().cast::<u8>()) };
            if rect < 0 || size < 0 || opacity < 0 || tex < 0 {
                // SAFETY: teardown of the mismatched program.
                unsafe { (f.glDeleteProgram)(program) };
                return Err(GlesError::Failed(
                    "the composite program lacks a required uniform".into(),
                ));
            }
            Ok(ProgramState {
                raw: program,
                rect,
                size,
                opacity,
                tex,
            })
        })?;
        let raw = self.next_program;
        self.next_program += 1;
        self.programs.insert(raw, raw_program);
        GlesProgram::new(raw).map_err(|e| GlesError::Invalid(e.to_string()))
    }

    fn destroy_program(&mut self, program: GlesProgram) -> Result<(), GlesError> {
        let Some(state) = self.programs.remove(&program.raw()) else {
            return Err(GlesError::BadHandle("program handle"));
        };
        self.with_current(|f| {
            // SAFETY: the handle is owned and was just removed.
            unsafe { (f.glDeleteProgram)(state.raw) };
            Ok(())
        })?;
        self.check_error("glDeleteProgram", ())
    }

    fn create_target(&mut self, width: u32, height: u32) -> Result<GlesTarget, GlesError> {
        if width == 0 || height == 0 {
            return Err(GlesError::Invalid("render target with a zero axis".into()));
        }
        if self.pass.is_some() {
            return Err(GlesError::Invalid("create_target with a pass open".into()));
        }
        let fbo = self.with_current(|f| {
            let mut tex: u32 = 0;
            let mut fbo: u32 = 0;
            // SAFETY: valid out-pointers.
            unsafe {
                (f.glGenTextures)(1, &mut tex);
                (f.glGenFramebuffers)(1, &mut fbo);
                (f.glBindTexture)(gl::TEXTURE_2D, tex);
                (f.glTexParameteri)(gl::TEXTURE_2D, gl::TEXTURE_MIN_FILTER, gl::NEAREST);
                (f.glTexParameteri)(gl::TEXTURE_2D, gl::TEXTURE_MAG_FILTER, gl::NEAREST);
                (f.glTexParameteri)(gl::TEXTURE_2D, gl::TEXTURE_WRAP_S, gl::CLAMP_TO_EDGE);
                (f.glTexParameteri)(gl::TEXTURE_2D, gl::TEXTURE_WRAP_T, gl::CLAMP_TO_EDGE);
                // NULL data: a zero-filled texture, cleared below.
                (f.glTexImage2D)(
                    gl::TEXTURE_2D,
                    0,
                    gl::RGBA,
                    width as i32,
                    height as i32,
                    0,
                    gl::RGBA_ENUM,
                    gl::UNSIGNED_BYTE,
                    core::ptr::null(),
                );
                (f.glBindFramebuffer)(gl::FRAMEBUFFER, fbo);
                (f.glFramebufferTexture2D)(
                    gl::FRAMEBUFFER,
                    gl::COLOR_ATTACHMENT0,
                    gl::TEXTURE_2D,
                    tex,
                    0,
                );
            }
            let status = {
                // SAFETY: pure query.
                unsafe { (f.glCheckFramebufferStatus)(gl::FRAMEBUFFER) }
            };
            if status != gl::FRAMEBUFFER_COMPLETE {
                // SAFETY: teardown of the incomplete objects.
                unsafe {
                    (f.glBindFramebuffer)(gl::FRAMEBUFFER, 0);
                    (f.glDeleteFramebuffers)(1, &fbo);
                    (f.glDeleteTextures)(1, &tex);
                }
                return Err(GlesError::Failed(format!(
                    "render target incomplete (status {status:#06x})"
                )));
            }
            // The contract: fresh targets start opaque black.
            // SAFETY: the FBO is bound; a plain clear.
            unsafe {
                (f.glDisable)(gl::SCISSOR_TEST);
                (f.glClearColor)(0.0, 0.0, 0.0, 1.0);
                (f.glClear)(gl::COLOR_BUFFER_BIT);
                (f.glBindFramebuffer)(gl::FRAMEBUFFER, 0);
            }
            Ok(fbo)
        })?;
        self.check_error("render target creation", ())?;
        let raw = self.next_target;
        self.next_target += 1;
        self.targets.insert(raw, TargetState { fbo, width, height });
        GlesTarget::new(raw).map_err(|e| GlesError::Invalid(e.to_string()))
    }

    fn destroy_target(&mut self, target: GlesTarget) -> Result<(), GlesError> {
        if self.pass.is_some() {
            // The pass state does not pin the target handle (the FBO
            // is only bound during passes); destroying while a pass is
            // open is rejected to mirror the reference evaluator.
            return Err(GlesError::Invalid("destroy_target with a pass open".into()));
        }
        let Some(state) = self.targets.remove(&target.raw()) else {
            return Err(GlesError::BadHandle("render-target handle"));
        };
        let fbo = state.fbo;
        self.with_current(|f| {
            // SAFETY: the FBO handle is owned and was just removed.
            unsafe { (f.glDeleteFramebuffers)(1, &fbo) };
            Ok(())
        })?;
        self.check_error("glDeleteFramebuffers", ())
    }

    fn create_texture(
        &mut self,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) -> Result<GlesTexture, GlesError> {
        if width == 0 || height == 0 {
            return Err(GlesError::Invalid("texture with a zero axis".into()));
        }
        let need = usize::try_from(width.saturating_mul(height).saturating_mul(4))
            .map_err(|_| GlesError::Invalid("texture extent overflows".into()))?;
        if rgba.len() != need {
            return Err(GlesError::Invalid(format!(
                "texture payload is {} bytes, expected {need}",
                rgba.len()
            )));
        }
        let gl_name = self.with_current(|f| {
            let mut tex: u32 = 0;
            // SAFETY: valid out-pointer.
            unsafe { (f.glGenTextures)(1, &mut tex) };
            // SAFETY: the handle is fresh; the payload borrow is live.
            unsafe {
                (f.glBindTexture)(gl::TEXTURE_2D, tex);
                (f.glTexParameteri)(gl::TEXTURE_2D, gl::TEXTURE_MIN_FILTER, gl::NEAREST);
                (f.glTexParameteri)(gl::TEXTURE_2D, gl::TEXTURE_MAG_FILTER, gl::NEAREST);
                (f.glTexParameteri)(gl::TEXTURE_2D, gl::TEXTURE_WRAP_S, gl::CLAMP_TO_EDGE);
                (f.glTexParameteri)(gl::TEXTURE_2D, gl::TEXTURE_WRAP_T, gl::CLAMP_TO_EDGE);
                (f.glTexImage2D)(
                    gl::TEXTURE_2D,
                    0,
                    gl::RGBA,
                    width as i32,
                    height as i32,
                    0,
                    gl::RGBA_ENUM,
                    gl::UNSIGNED_BYTE,
                    rgba.as_ptr().cast::<core::ffi::c_void>(),
                );
            }
            Ok(tex)
        })?;
        self.check_error("texture upload", ())?;
        let raw = self.next_texture;
        self.next_texture += 1;
        self.textures.insert(raw, TextureState { gl_name });
        GlesTexture::new(raw).map_err(|e| GlesError::Invalid(e.to_string()))
    }

    fn create_texture_from_egl_image(&mut self, image: u64) -> Result<GlesTexture, GlesError> {
        // The DMA-BUF zero-copy path (Phase 31): one glGenTextures,
        // the NEAREST/CLAMP parameters (the sampler contract the
        // CPU-upload path sets), and the single
        // glEGLImageTargetTexture2DOES call that binds the imported
        // image — the pixels never cross the CPU.
        let gl_name = self.with_current(|f| {
            let mut tex: u32 = 0;
            // SAFETY: valid out-pointer.
            unsafe { (f.glGenTextures)(1, &mut tex) };
            // SAFETY: the handle is fresh; the image value is the
            // EGLImage pointer the EGL side minted (the same u64 the
            // import returned).
            unsafe {
                (f.glBindTexture)(gl::TEXTURE_2D, tex);
                (f.glTexParameteri)(gl::TEXTURE_2D, gl::TEXTURE_MIN_FILTER, gl::NEAREST);
                (f.glTexParameteri)(gl::TEXTURE_2D, gl::TEXTURE_MAG_FILTER, gl::NEAREST);
                (f.glTexParameteri)(gl::TEXTURE_2D, gl::TEXTURE_WRAP_S, gl::CLAMP_TO_EDGE);
                (f.glTexParameteri)(gl::TEXTURE_2D, gl::TEXTURE_WRAP_T, gl::CLAMP_TO_EDGE);
                (f.glEGLImageTargetTexture2DOES)(
                    gl::TEXTURE_2D,
                    usize::try_from(image)
                        .map_err(|_| GlesError::Invalid("EGLImage handle overflows".into()))?
                        as *const core::ffi::c_void,
                );
            }
            Ok(tex)
        })?;
        self.check_error("EGLImage texture bind", ())?;
        let raw = self.next_texture;
        self.next_texture += 1;
        self.textures.insert(raw, TextureState { gl_name });
        GlesTexture::new(raw).map_err(|e| GlesError::Invalid(e.to_string()))
    }

    fn destroy_texture(&mut self, texture: GlesTexture) -> Result<(), GlesError> {
        let Some(state) = self.textures.remove(&texture.raw()) else {
            return Err(GlesError::BadHandle("texture handle"));
        };
        let raw = state.gl_name;
        self.with_current(|f| {
            // SAFETY: the handle is owned and was just removed.
            unsafe { (f.glDeleteTextures)(1, &raw) };
            Ok(())
        })?;
        self.check_error("glDeleteTextures", ())
    }

    fn begin_pass(&mut self, target: GlesTarget, scissor: Option<Rect>) -> Result<(), GlesError> {
        if self.pass.is_some() {
            return Err(GlesError::Invalid(
                "begin_pass with a pass already open".into(),
            ));
        }
        let Some(state) = self.targets.get(&target.raw()).copied() else {
            return Err(GlesError::BadHandle("render-target handle"));
        };
        let (w, h) = (state.width, state.height);
        // Clip the scissor to the target (the API contract).
        let clip = match scissor {
            Some(r) => clip_to(w, h, r),
            None => None,
        };
        let fbo = state.fbo;
        self.with_current(|f| {
            // SAFETY: the FBO is owned; binding is stateless.
            unsafe { (f.glBindFramebuffer)(gl::FRAMEBUFFER, fbo) };
            // SAFETY: plain state.
            unsafe { (f.glViewport)(0, 0, w as i32, h as i32) };
            match clip {
                Some((x, y, cw, ch)) => {
                    // Top-down rect to GL's bottom-left origin.
                    let gy = i32::try_from(h).unwrap_or(0) - y - ch;
                    // SAFETY: plain state.
                    unsafe {
                        (f.glScissor)(x, gy, cw, ch);
                        (f.glEnable)(gl::SCISSOR_TEST);
                    }
                }
                None => {
                    // SAFETY: plain state.
                    unsafe { (f.glDisable)(gl::SCISSOR_TEST) }
                }
            }
            Ok(())
        })?;
        self.check_error("begin_pass", ())?;
        self.pass = Some(PassState {
            width: w,
            height: h,
        });
        Ok(())
    }

    #[allow(clippy::many_single_char_names)] // the GlesApi contract's own parameter names
    fn clear(&mut self, r: u8, g: u8, b: u8, a: u8) -> Result<(), GlesError> {
        if self.pass.is_none() {
            return Err(GlesError::Invalid("clear without an open pass".into()));
        }
        let (rf, gf, bf, af) = (
            f32::from(r) / 255.0,
            f32::from(g) / 255.0,
            f32::from(b) / 255.0,
            f32::from(a) / 255.0,
        );
        self.with_current(|f| {
            // SAFETY: plain state + a scissored clear (GL semantics).
            unsafe {
                (f.glClearColor)(rf, gf, bf, af);
                (f.glClear)(gl::COLOR_BUFFER_BIT);
            }
            Ok(())
        })?;
        self.check_error("clear", ())
    }

    fn draw_layer(
        &mut self,
        texture: GlesTexture,
        dst: Rect,
        opacity: u8,
    ) -> Result<(), GlesError> {
        let Some(pass) = self.pass.as_ref() else {
            return Err(GlesError::Invalid("draw_layer without an open pass".into()));
        };
        let Some(tex_state) = self.textures.get(&texture.raw()).copied() else {
            return Err(GlesError::BadHandle("texture handle"));
        };
        // The program: the renderer links exactly one; use the first.
        // (GlesRenderer links one program and reuses it; drawing
        // without any program is a stream bug.)
        let Some(program) = self.programs.values().next().cloned() else {
            return Err(GlesError::Invalid(
                "draw_layer before any program was linked".into(),
            ));
        };
        let (target_w, target_h) = (pass.width, pass.height);
        let program_raw = program.raw;
        let (rect_loc, size_loc, opacity_loc, tex_loc) =
            (program.rect, program.size, program.opacity, program.tex);
        let quad = self.quad;
        let (dx, dy, dw, dh) = (dst.x as f32, dst.y as f32, dst.w as f32, dst.h as f32);
        let q = f32::from(opacity) / 255.0;
        let tex_raw = tex_state.gl_name;
        self.with_current(|f| {
            // SAFETY: plain state on a live program.
            unsafe { (f.glUseProgram)(program_raw) };
            // SAFETY: locations validated at link; plain uniforms.
            unsafe {
                (f.glUniform4f)(rect_loc, dx, dy, dw, dh);
                (f.glUniform2f)(size_loc, target_w as f32, target_h as f32);
                (f.glUniform1f)(opacity_loc, q);
                (f.glUniform1i)(tex_loc, 0);
                (f.glActiveTexture)(gl::TEXTURE0);
                (f.glBindTexture)(gl::TEXTURE_2D, tex_raw);
            }
            // The quad VBO: two attribs of two floats each.
            // SAFETY: the VBO is owned; the stride/offset are the
            // pinned layout (a_pos=0, a_uv=1).
            unsafe {
                (f.glBindBuffer)(gl::ARRAY_BUFFER, quad);
                (f.glEnableVertexAttribArray)(0);
                (f.glEnableVertexAttribArray)(1);
                (f.glVertexAttribPointer)(0, 2, gl::FLOAT, false, 16, core::ptr::null());
                (f.glVertexAttribPointer)(
                    1,
                    2,
                    gl::FLOAT,
                    false,
                    16,
                    8usize as *const core::ffi::c_void,
                );
                // Premultiplied source-over.
                (f.glEnable)(gl::BLEND);
                (f.glBlendEquation)(gl::FUNC_ADD);
                (f.glBlendFunc)(gl::ONE, gl::ONE_MINUS_SRC_ALPHA);
                (f.glDrawArrays)(gl::TRIANGLE_STRIP, 0, 4);
                (f.glDisableVertexAttribArray)(0);
                (f.glDisableVertexAttribArray)(1);
                (f.glBindBuffer)(gl::ARRAY_BUFFER, 0);
                (f.glDisable)(gl::BLEND);
            }
            Ok(())
        })?;
        self.check_error("draw_layer", ())
    }

    fn end_pass(&mut self) -> Result<(), GlesError> {
        if self.pass.take().is_none() {
            return Err(GlesError::Invalid("end_pass without an open pass".into()));
        }
        self.with_current(|f| {
            // SAFETY: unbinding the FBO and the scissor.
            unsafe {
                (f.glBindFramebuffer)(gl::FRAMEBUFFER, 0);
                (f.glDisable)(gl::SCISSOR_TEST);
            }
            Ok(())
        })?;
        self.check_error("end_pass", ())
    }

    fn readback(&mut self, target: GlesTarget, rgba_out: &mut [u8]) -> Result<(), GlesError> {
        if self.pass.is_some() {
            return Err(GlesError::Invalid(
                "readback with a pass open (close it first)".into(),
            ));
        }
        let Some(state) = self.targets.get(&target.raw()).copied() else {
            return Err(GlesError::BadHandle("render-target handle"));
        };
        let need = (state.width as usize) * (state.height as usize) * 4;
        if rgba_out.len() != need {
            return Err(GlesError::Invalid(format!(
                "readback buffer is {} bytes, the target holds {need}",
                rgba_out.len()
            )));
        }
        let fbo = state.fbo;
        let (w, h) = (
            i32::try_from(state.width).unwrap_or(0),
            i32::try_from(state.height).unwrap_or(0),
        );
        self.with_current(|f| {
            // SAFETY: the FBO is owned; the output borrow is live and
            // exactly target-sized.
            unsafe {
                (f.glBindFramebuffer)(gl::FRAMEBUFFER, fbo);
                (f.glDisable)(gl::SCISSOR_TEST);
                (f.glReadPixels)(
                    0,
                    0,
                    w,
                    h,
                    gl::RGBA_ENUM,
                    gl::UNSIGNED_BYTE,
                    rgba_out.as_mut_ptr().cast::<core::ffi::c_void>(),
                );
                (f.glBindFramebuffer)(gl::FRAMEBUFFER, 0);
            }
            Ok(())
        })?;
        self.check_error("glReadPixels", ())?;
        // glReadPixels returns bottom-up rows; the API is top-down.
        flip_rows(rgba_out, state.width as usize);
        Ok(())
    }
}

/// Clip a top-down rect to `width` x `height`, as `(x, y, w, h)`
/// GL-style bottom-left values (`None` when empty or no scissor).
fn clip_to(width: u32, height: u32, rect: Rect) -> Option<(i32, i32, i32, i32)> {
    let x0 = rect.x.max(0);
    let y0 = rect.y.max(0);
    let x1 = rect
        .x
        .saturating_add(i32::try_from(rect.w).unwrap_or(0))
        .min(i32::try_from(width).unwrap_or(i32::MAX));
    let y1 = rect
        .y
        .saturating_add(i32::try_from(rect.h).unwrap_or(0))
        .min(i32::try_from(height).unwrap_or(i32::MAX));
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some((x0, y0, x1 - x0, y1 - y0))
}

/// Reverse row order in place (bottom-up GL readback → top-down).
fn flip_rows(rgba: &mut [u8], width: usize) {
    if width == 0 {
        return;
    }
    let stride = width * 4;
    let rows = rgba.len() / stride;
    for r in 0..rows / 2 {
        let top = r * stride;
        let bottom = (rows - 1 - r) * stride;
        for i in 0..stride {
            rgba.swap(top + i, bottom + i);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_to_matches_the_reference_semantics() {
        use ldp_core::geometry::Rect;
        // Inside.
        assert_eq!(clip_to(8, 8, Rect::new(1, 2, 3, 4)), Some((1, 2, 3, 4)));
        // Hanging off the left/top.
        assert_eq!(clip_to(8, 8, Rect::new(-2, -2, 4, 4)), Some((0, 0, 2, 2)));
        // Hanging off the right/bottom.
        assert_eq!(clip_to(8, 8, Rect::new(6, 6, 8, 8)), Some((6, 6, 2, 2)));
        // Fully outside.
        assert_eq!(clip_to(8, 8, Rect::new(9, 9, 2, 2)), None);
        // Zero-size.
        assert_eq!(clip_to(8, 8, Rect::new(3, 3, 0, 4)), None);
    }

    #[test]
    fn row_flip_is_an_involution() {
        let mut rows: Vec<u8> = (0..24u8).collect();
        let before = rows.clone();
        flip_rows(&mut rows, 2); // 3 rows of 8 bytes.
        flip_rows(&mut rows, 2);
        assert_eq!(rows, before);
        // And a single flip reverses row order.
        let mut rows: Vec<u8> = (0..16u8).collect();
        flip_rows(&mut rows, 2);
        assert_eq!(rows[0], 8);
        assert_eq!(rows[8], 0);
    }

    #[test]
    fn honest_headless_construction_fails_typed() {
        // On CI (no GL stack) this must fail with Unavailable — the
        // branch the renderer-selection policy consumes. On a GL
        // machine it succeeds and the test is a no-op smoke.
        match RealGles::new() {
            Ok(backend) => {
                // Hardware present: prove the identity round-trips.
                assert!(!format!("{backend:?}").is_empty());
            }
            Err(GlesError::Unavailable(reason)) => {
                assert!(!reason.is_empty(), "the reason must be carried");
            }
            Err(other) => panic!("unexpected RealGles failure: {other:?}"),
        }
    }
}
