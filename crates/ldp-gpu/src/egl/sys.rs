//! The libEGL runtime layer — dlopen, symbols, opaque types.
//!
//! The crate's second **audit boundary**: every `unsafe` in the EGL
//! tree lives here with per-call `SAFETY` comments. EGL's public ABI
//! is unusually friendly to this treatment: every interesting type is
//! either an opaque pointer (`EGLDisplay`, `EGLImageKHR`, `EGLSyncKHR`)
//! or a plain `u32`/`i32`, so no `repr(C)` struct walking is needed at
//! all — the symbol table below is the whole ABI surface this crate
//! consumes.
//!
//! Loading is failure-tolerant by design: headless machines without
//! GL stacks simply lack `libEGL.so.1`, and [`LibEgl::open`] reports
//! that as a typed [`EglError::LibraryLoad`] — the honest unavailable
//! path, exercised in CI. The two symbols the fence path wants
//! (`eglDupNativeFenceFD`, `eglWaitSync`) are resolved through
//! `eglGetProcAddress` at bootstrap the way the extensions require,
//! with their absence equally typed.

use std::ffi::{c_char, c_int, c_void, CStr};

/// SONAME.
const LIBEGL: &str = "libEGL.so.1";

/// EGL constants the layer is pinned against (egl.h/eglext.h ABI).
pub mod egl {
    /// `EGL_SUCCESS`.
    pub const SUCCESS: u32 = 0x3000;
    /// `EGL_NOT_INITIALIZED`.
    pub const NOT_INITIALIZED: u32 = 0x3001;
    /// `EGL_BAD_ACCESS`.
    pub const BAD_ACCESS: u32 = 0x3002;
    /// `EGL_BAD_PARAMETER`.
    pub const BAD_PARAMETER: u32 = 0x300C;
    /// `EGL_DEFAULT_DISPLAY` (NULL).
    pub const DEFAULT_DISPLAY: *mut core::ffi::c_void = core::ptr::null_mut();
    /// `EGL_EXTENSIONS`.
    pub const EXTENSIONS: u32 = 0x3055;
    /// `EGL_NONE` — the attribute list terminator.
    pub const NONE: i32 = 0x3058;
    /// `EGL_LINUX_DMA_BUF_EXT` — the image target for imports.
    pub const LINUX_DMA_BUF: u32 = 0x3270;
    /// `EGL_SYNC_NATIVE_FENCE_ANDROID` — the fence sync type.
    pub const SYNC_NATIVE_FENCE: u32 = 0x3144;
    /// `EGL_NO_NATIVE_FENCE_FD_ANDROID` — "no fd yet".
    pub const NO_NATIVE_FENCE_FD: i32 = -1;
    /// `EGL_FOREVER` — wait without timeout.
    pub const FOREVER: u64 = 0xFFFF_FFFF_FFFF_FFFF;
    /// `EGL_CONDITION_SIGNALED` — wait succeeded.
    pub const CONDITION_SIGNALED: u32 = 0x30F2;
    /// `EGL_TIMEOUT_EXPIRED` — wait gave up.
    pub const TIMEOUT_EXPIRED: u32 = 0x30F5;

    // ---- the Phase 24 context-creation constants -------------------
    /// `EGL_SURFACE_TYPE` (config attrib).
    pub const SURFACE_TYPE: i32 = 0x3030;
    /// `EGL_PBUFFER_BIT`.
    pub const PBUFFER_BIT: i32 = 0x0004;
    /// `EGL_RENDERABLE_TYPE` (config attrib).
    pub const RENDERABLE_TYPE: i32 = 0x3040;
    /// `EGL_OPENGL_ES2_BIT` (renderable type value).
    pub const OPENGL_ES2_BIT: i32 = 0x0004;
    /// `EGL_RED_SIZE` (config attrib).
    pub const RED_SIZE: i32 = 0x3024;
    /// `EGL_GREEN_SIZE` (config attrib).
    pub const GREEN_SIZE: i32 = 0x3023;
    /// `EGL_BLUE_SIZE` (config attrib).
    pub const BLUE_SIZE: i32 = 0x3022;
    /// `EGL_ALPHA_SIZE` (config attrib).
    pub const ALPHA_SIZE: i32 = 0x3021;
    /// `EGL_DEPTH_SIZE` (config attrib).
    pub const DEPTH_SIZE: i32 = 0x3025;
    /// `EGL_STENCIL_SIZE` (config attrib).
    pub const STENCIL_SIZE: i32 = 0x3026;
    /// `EGL_WIDTH` (pbuffer attrib).
    pub const PBUFFER_WIDTH: i32 = 0x3057;
    /// `EGL_HEIGHT` (pbuffer attrib).
    pub const PBUFFER_HEIGHT: i32 = 0x3056;
    /// `EGL_CONTEXT_CLIENT_VERSION` (context attrib).
    pub const CONTEXT_CLIENT_VERSION: i32 = 0x3098;
    /// `EGL_OPENGL_ES_API` (eglBindAPI value).
    pub const OPENGL_ES_API: u32 = 0x30A0;
}

/// An initialized EGL display, opaque.
///
/// Constructed only by [`LibEgl::bootstrap`]; the raw pointer never
/// crosses a safe function signature.
#[derive(Clone, Copy, Debug)]
pub struct DisplayHandle {
    raw: *mut c_void,
}

/// An EGLConfig handle (from `eglChooseConfig`).
#[derive(Clone, Copy)]
pub struct EglConfig {
    /// The raw handle (backend namespace).
    pub(crate) raw: *mut c_void,
}

impl EglConfig {
    /// The null config (no match).
    #[must_use]
    pub const fn null() -> Self {
        Self {
            raw: core::ptr::null_mut(),
        }
    }

    /// Whether this is the null handle (non-const: `is_null` gained
    /// const stability only in 1.84, above the workspace MSRV).
    #[must_use]
    pub fn is_null(self) -> bool {
        self.raw.is_null()
    }
}

/// An EGLSurface handle (pbuffer surfaces here).
#[derive(Clone, Copy)]
pub struct EglSurface {
    /// The raw handle (backend namespace).
    pub(crate) raw: *mut c_void,
}

impl EglSurface {
    /// The null surface (unbind).
    #[must_use]
    pub const fn null() -> Self {
        Self {
            raw: core::ptr::null_mut(),
        }
    }

    /// Whether this is the null handle (non-const: `is_null` gained
    /// const stability only in 1.84, above the workspace MSRV).
    #[must_use]
    pub fn is_null(self) -> bool {
        self.raw.is_null()
    }
}

/// An EGLContext handle.
#[derive(Clone, Copy)]
pub struct EglContext {
    /// The raw handle (backend namespace).
    pub(crate) raw: *mut c_void,
}

impl EglContext {
    /// The null context (unbind).
    #[must_use]
    pub const fn null() -> Self {
        Self {
            raw: core::ptr::null_mut(),
        }
    }

    /// Whether this is the null handle (non-const: `is_null` gained
    /// const stability only in 1.84, above the workspace MSRV).
    #[must_use]
    pub fn is_null(self) -> bool {
        self.raw.is_null()
    }
}

/// A completed bootstrap: the live display, its parsed extension
/// names, and the EGL version it initialized with.
#[derive(Clone, Debug)]
pub struct Bootstrap {
    /// The initialized display.
    pub display: DisplayHandle,
    /// The parsed `EGL_EXTENSIONS` entries.
    pub extensions: Vec<String>,
    /// The `(major, minor)` version.
    pub version: (i32, i32),
}

impl DisplayHandle {
    /// The null handle (no display yet).
    #[must_use]
    pub const fn null() -> Self {
        Self {
            raw: core::ptr::null_mut(),
        }
    }

    /// Whether no display is bound.
    #[must_use]
    pub fn is_null(self) -> bool {
        self.raw.is_null()
    }
}

/// The resolved function table.
#[derive(Clone, Copy)]
struct EglFns {
    get_display: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    initialize: unsafe extern "C" fn(*mut c_void, *mut c_int, *mut c_int) -> u32,
    terminate: unsafe extern "C" fn(*mut c_void) -> u32,
    query_string: unsafe extern "C" fn(*mut c_void, u32) -> *const c_char,
    get_proc_address: unsafe extern "C" fn(*const c_char) -> *mut c_void,
    create_image:
        unsafe extern "C" fn(*mut c_void, *mut c_void, u32, usize, *const i32) -> *mut c_void,
    destroy_image: unsafe extern "C" fn(*mut c_void, *mut c_void) -> u32,
    bind_api: unsafe extern "C" fn(u32) -> u32,
    choose_config:
        unsafe extern "C" fn(*mut c_void, *const i32, *mut *mut c_void, i32, *mut i32) -> u32,
    create_pbuffer_surface:
        unsafe extern "C" fn(*mut c_void, *mut c_void, *const i32) -> *mut c_void,
    create_context:
        unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, *const i32) -> *mut c_void,
    make_current: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, *mut c_void) -> u32,
    destroy_context: unsafe extern "C" fn(*mut c_void, *mut c_void) -> u32,
    destroy_surface: unsafe extern "C" fn(*mut c_void, *mut c_void) -> u32,
    /// `eglDupNativeFenceFDANDROID` (extension entry point).
    dup_native_fence_fd: Option<unsafe extern "C" fn(*mut c_void, *mut c_void) -> c_int>,
    /// `eglWaitSync` (1.5 core, resolved per the extension contract).
    wait_sync: Option<unsafe extern "C" fn(*mut c_void, *mut c_void, u32) -> u32>,
}

/// The loaded library.
pub struct LibEgl {
    handle: *mut c_void,
    fns: EglFns,
}

// dlopen handles are loader-refcounted and thread-safe.
unsafe impl Send for LibEgl {}
unsafe impl Sync for LibEgl {}

impl core::fmt::Debug for LibEgl {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Handle and table are opaque; the name is the identity.
        f.debug_struct("LibEgl")
            .field("library", &LIBEGL)
            .finish_non_exhaustive()
    }
}

/// EGL layer failures.
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum EglError {
    /// `libEGL.so.1` could not be loaded (headless machine).
    LibraryLoad {
        /// The SONAME that failed.
        library: &'static str,
    },
    /// A required core symbol is missing (ABI too old).
    MissingSymbol {
        /// The SONAME being searched.
        library: &'static str,
        /// The missing symbol.
        symbol: &'static str,
    },
    /// An extension entry point the fence path wants is absent.
    MissingExtension {
        /// The entry point name.
        symbol: &'static str,
    },
    /// `eglGetDisplay` returned `EGL_NO_DISPLAY`.
    NoDisplay,
    /// `eglInitialize` failed with the given EGL error code.
    InitializeFailed {
        /// The EGL error code.
        code: u32,
    },
    /// An EGL call failed with the given error code.
    CallFailed {
        /// What was being attempted.
        while_doing: &'static str,
        /// The EGL error code.
        code: u32,
    },
}

impl core::fmt::Display for EglError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::LibraryLoad { library } => write!(f, "cannot load {library}"),
            Self::MissingSymbol { library, symbol } => {
                write!(f, "{library} is missing symbol {symbol}")
            }
            Self::MissingExtension { symbol } => write!(f, "extension entry point {symbol} absent"),
            Self::NoDisplay => write!(f, "eglGetDisplay returned no display"),
            Self::InitializeFailed { code } => {
                write!(f, "eglInitialize failed with EGL error {code:#06x}")
            }
            Self::CallFailed { while_doing, code } => {
                write!(f, "egl error while {while_doing}: {code:#06x}")
            }
        }
    }
}

impl std::error::Error for EglError {}

impl LibEgl {
    /// `dlopen("libEGL.so.1")` and resolve the core table.
    ///
    /// # Errors
    /// [`EglError::LibraryLoad`] without a GL stack;
    /// [`EglError::MissingSymbol`] naming the first gap.
    pub fn open() -> Result<Self, EglError> {
        // SAFETY: the literal is NUL-terminated; dlopen only reads it
        // for the duration of the call.
        let handle =
            unsafe { libc::dlopen(c"libEGL.so.1".as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
        if handle.is_null() {
            return Err(EglError::LibraryLoad { library: LIBEGL });
        }
        // SAFETY: handle is live; each prototype matches the published
        // EGL 1.4/1.5 ABI. The two Android fence entry points come from
        // eglGetProcAddress per the extension contract — their absence
        // is a typed MissingExtension, not a load failure.
        let fns = unsafe { Self::resolve(handle)? };
        Ok(Self { handle, fns })
    }

    /// Resolve the symbol table, naming the first missing core symbol.
    ///
    /// # Safety
    /// `handle` must be a live dlopen handle; the transmuted prototypes
    /// must match the published EGL ABI.
    #[allow(clippy::too_many_lines)] // one entry point per line is the audited shape
    unsafe fn resolve(handle: *mut c_void) -> Result<EglFns, EglError> {
        macro_rules! sym {
            ($field:ident, $sig:ty, $name:literal) => {{
                let ptr = libc::dlsym(handle, concat!($name, "\0").as_ptr().cast::<c_char>());
                if ptr.is_null() {
                    return Err(EglError::MissingSymbol {
                        library: LIBEGL,
                        symbol: $name,
                    });
                }
                core::mem::transmute::<*mut c_void, $sig>(ptr)
            }};
        }
        let get_display: unsafe extern "C" fn(*mut c_void) -> *mut c_void = sym!(
            get_display,
            unsafe extern "C" fn(*mut c_void) -> *mut c_void,
            "eglGetDisplay"
        );
        let initialize: unsafe extern "C" fn(*mut c_void, *mut c_int, *mut c_int) -> u32 = sym!(
            initialize,
            unsafe extern "C" fn(*mut c_void, *mut c_int, *mut c_int) -> u32,
            "eglInitialize"
        );
        let terminate: unsafe extern "C" fn(*mut c_void) -> u32 = sym!(
            terminate,
            unsafe extern "C" fn(*mut c_void) -> u32,
            "eglTerminate"
        );
        let query_string: unsafe extern "C" fn(*mut c_void, u32) -> *const c_char = sym!(
            query_string,
            unsafe extern "C" fn(*mut c_void, u32) -> *const c_char,
            "eglQueryString"
        );
        let get_proc_address: unsafe extern "C" fn(*const c_char) -> *mut c_void = sym!(
            get_proc_address,
            unsafe extern "C" fn(*const c_char) -> *mut c_void,
            "eglGetProcAddress"
        );
        let create_image: unsafe extern "C" fn(
            *mut c_void,
            *mut c_void,
            u32,
            usize,
            *const i32,
        ) -> *mut c_void = sym!(
            create_image,
            unsafe extern "C" fn(*mut c_void, *mut c_void, u32, usize, *const i32) -> *mut c_void,
            "eglCreateImage"
        );
        let destroy_image: unsafe extern "C" fn(*mut c_void, *mut c_void) -> u32 = sym!(
            destroy_image,
            unsafe extern "C" fn(*mut c_void, *mut c_void) -> u32,
            "eglDestroyImage"
        );
        // Extension entry points: resolved by name through
        // eglGetProcAddress; absence is typed, not fatal to the load.
        // (Fence-sync *creation* resolves the same way in Phase 10's
        // compositor, when the wait paths exist to drive it.)
        let dup_native_fence_fd = unsafe {
            let ptr = get_proc_address(c"eglDupNativeFenceFDANDROID".as_ptr());
            if ptr.is_null() {
                None
            } else {
                Some(core::mem::transmute::<
                    *mut c_void,
                    unsafe extern "C" fn(*mut c_void, *mut c_void) -> c_int,
                >(ptr))
            }
        };
        let wait_sync = unsafe {
            let ptr = get_proc_address(c"eglWaitSync".as_ptr());
            if ptr.is_null() {
                None
            } else {
                Some(core::mem::transmute::<
                    *mut c_void,
                    unsafe extern "C" fn(*mut c_void, *mut c_void, u32) -> u32,
                >(ptr))
            }
        };
        let bind_api: unsafe extern "C" fn(u32) -> u32 =
            sym!(bind_api, unsafe extern "C" fn(u32) -> u32, "eglBindAPI");
        let choose_config: unsafe extern "C" fn(
            *mut c_void,
            *const i32,
            *mut *mut c_void,
            i32,
            *mut i32,
        ) -> u32 = sym!(
            choose_config,
            unsafe extern "C" fn(*mut c_void, *const i32, *mut *mut c_void, i32, *mut i32) -> u32,
            "eglChooseConfig"
        );
        let create_pbuffer_surface: unsafe extern "C" fn(
            *mut c_void,
            *mut c_void,
            *const i32,
        ) -> *mut c_void = sym!(
            create_pbuffer_surface,
            unsafe extern "C" fn(*mut c_void, *mut c_void, *const i32) -> *mut c_void,
            "eglCreatePbufferSurface"
        );
        let create_context: unsafe extern "C" fn(
            *mut c_void,
            *mut c_void,
            *mut c_void,
            *const i32,
        ) -> *mut c_void = sym!(
            create_context,
            unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, *const i32) -> *mut c_void,
            "eglCreateContext"
        );
        let make_current: unsafe extern "C" fn(
            *mut c_void,
            *mut c_void,
            *mut c_void,
            *mut c_void,
        ) -> u32 = sym!(
            make_current,
            unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, *mut c_void) -> u32,
            "eglMakeCurrent"
        );
        let destroy_context: unsafe extern "C" fn(*mut c_void, *mut c_void) -> u32 = sym!(
            destroy_context,
            unsafe extern "C" fn(*mut c_void, *mut c_void) -> u32,
            "eglDestroyContext"
        );
        let destroy_surface: unsafe extern "C" fn(*mut c_void, *mut c_void) -> u32 = sym!(
            destroy_surface,
            unsafe extern "C" fn(*mut c_void, *mut c_void) -> u32,
            "eglDestroySurface"
        );
        Ok(EglFns {
            get_display,
            initialize,
            terminate,
            query_string,
            get_proc_address,
            create_image,
            destroy_image,
            bind_api,
            choose_config,
            create_pbuffer_surface,
            create_context,
            make_current,
            destroy_context,
            destroy_surface,
            dup_native_fence_fd,
            wait_sync,
        })
    }

    /// The raw table's bootstrap: default display + initialize +
    /// extension string.
    ///
    /// # Errors
    /// [`EglError::NoDisplay`], [`EglError::InitializeFailed`].
    pub fn bootstrap(&self) -> Result<Bootstrap, EglError> {
        // SAFETY: NULL selects the platform default display; the
        // version out-pointers are valid locals for the call.
        let display = unsafe { (self.fns.get_display)(egl::DEFAULT_DISPLAY) };
        if display.is_null() {
            return Err(EglError::NoDisplay);
        }
        let mut major: c_int = 0;
        let mut minor: c_int = 0;
        // SAFETY: display is live from get_display; the version
        // out-pointers are valid locals.
        let ok = unsafe { (self.fns.initialize)(display, &mut major, &mut minor) };
        if ok != egl::SUCCESS {
            return Err(EglError::InitializeFailed { code: ok });
        }
        let handle = DisplayHandle { raw: display };
        let extensions = self.query_extensions(handle)?;
        Ok(Bootstrap {
            display: handle,
            extensions,
            version: (major, minor),
        })
    }

    /// Create a DMA-BUF image from a flat `EGLint` attribute list
    /// (already `EGL_NONE`-terminated by the caller).
    ///
    /// # Errors
    /// [`EglError::CallFailed`] when the create returns
    /// `EGL_NO_IMAGE_KHR`.
    ///
    /// # Safety contract (caller)
    /// `display` must be an initialized display from this library's
    /// bootstrap; the attribute list must be a valid borrow for the
    /// call.
    pub fn create_dmabuf_image(
        &self,
        display: DisplayHandle,
        attribs: &[i32],
    ) -> Result<usize, EglError> {
        // SAFETY: caller contract above; the target and context-less
        // import are the extension's documented shape.
        let image = unsafe {
            (self.fns.create_image)(
                display.raw,
                core::ptr::null_mut(),
                egl::LINUX_DMA_BUF,
                0,
                attribs.as_ptr(),
            )
        };
        if image.is_null() {
            return Err(EglError::CallFailed {
                while_doing: "eglCreateImage",
                code: egl::BAD_PARAMETER,
            });
        }
        Ok(image as usize)
    }

    /// Destroy an image created by this library.
    ///
    /// # Errors
    /// [`EglError::CallFailed`] with the EGL error code.
    ///
    /// # Safety contract (caller)
    /// `display` must be initialized; `image` must have come from this
    /// display's create path and not been destroyed yet.
    pub fn destroy_image(&self, display: DisplayHandle, image: usize) -> Result<(), EglError> {
        // SAFETY: caller contract above.
        let ok = unsafe { (self.fns.destroy_image)(display.raw, image as *mut c_void) };
        if ok != egl::SUCCESS {
            return Err(EglError::CallFailed {
                while_doing: "eglDestroyImage",
                code: ok,
            });
        }
        Ok(())
    }

    /// Tear a bootstrapped display down (`eglTerminate`).
    ///
    /// # Errors
    /// [`EglError::CallFailed`] with the EGL error code.
    ///
    /// # Safety contract (caller)
    /// `display` must have come from this library's bootstrap; after
    /// the call no images or syncs from it may be used.
    pub fn terminate(&self, display: DisplayHandle) -> Result<(), EglError> {
        // SAFETY: caller contract above; terminate is idempotent.
        let ok = unsafe { (self.fns.terminate)(display.raw) };
        if ok != egl::SUCCESS {
            return Err(EglError::CallFailed {
                while_doing: "eglTerminate",
                code: ok,
            });
        }
        Ok(())
    }

    /// Bind the OpenGL ES API for this thread (`eglBindAPI`).
    ///
    /// # Errors
    /// [`EglError::CallFailed`] when the bind fails.
    pub fn bind_gles_api(&self) -> Result<(), EglError> {
        // SAFETY: a plain EGLenum; no pointers cross.
        let ok = unsafe { (self.fns.bind_api)(egl::OPENGL_ES_API) };
        if ok != egl::SUCCESS {
            return Err(EglError::CallFailed {
                while_doing: "eglBindAPI(OPENGL_ES_API)",
                code: ok,
            });
        }
        Ok(())
    }

    /// Choose an RGBA8888 pbuffer-capable GLES2-renderable config
    /// (`eglChooseConfig` with the composite pass's fixed attribs).
    /// Returns the first matching config handle.
    ///
    /// # Errors
    /// [`EglError::CallFailed`] when no config matches.
    pub fn choose_rgba_config(&self, display: DisplayHandle) -> Result<EglConfig, EglError> {
        let attribs: [i32; 13] = [
            egl::SURFACE_TYPE,
            egl::PBUFFER_BIT,
            egl::RENDERABLE_TYPE,
            egl::OPENGL_ES2_BIT,
            egl::RED_SIZE,
            8,
            egl::GREEN_SIZE,
            8,
            egl::BLUE_SIZE,
            8,
            egl::ALPHA_SIZE,
            8,
            egl::NONE,
        ];
        let mut config: *mut c_void = core::ptr::null_mut();
        let mut count: c_int = 0;
        // SAFETY: display is initialized; attribs is a fixed
        // EGL_NONE-terminated borrow; config/count are valid locals.
        let ok = unsafe {
            (self.fns.choose_config)(display.raw, attribs.as_ptr(), &mut config, 1, &mut count)
        };
        if ok != egl::SUCCESS || count < 1 || config.is_null() {
            return Err(EglError::CallFailed {
                while_doing: "eglChooseConfig (RGBA8888 pbuffer GLES2)",
                code: ok,
            });
        }
        Ok(EglConfig { raw: config })
    }

    /// Create a `width` x `height` pbuffer surface on `config`
    /// (`eglCreatePbufferSurface`).
    ///
    /// # Errors
    /// [`EglError::CallFailed`] when creation fails.
    pub fn create_pbuffer(
        &self,
        display: DisplayHandle,
        config: EglConfig,
        width: i32,
        height: i32,
    ) -> Result<EglSurface, EglError> {
        let attribs: [i32; 5] = [
            egl::PBUFFER_WIDTH,
            width,
            egl::PBUFFER_HEIGHT,
            height,
            egl::NONE,
        ];
        // SAFETY: display/config are live; attribs is a fixed
        // EGL_NONE-terminated borrow.
        let surface =
            unsafe { (self.fns.create_pbuffer_surface)(display.raw, config.raw, attribs.as_ptr()) };
        if surface.is_null() {
            return Err(EglError::CallFailed {
                while_doing: "eglCreatePbufferSurface",
                code: 0x300C,
            });
        }
        Ok(EglSurface { raw: surface })
    }

    /// Create a GLES 2.0 context on `config` (`eglCreateContext` with
    /// `EGL_CONTEXT_CLIENT_VERSION = 2`).
    ///
    /// # Errors
    /// [`EglError::CallFailed`] when creation fails.
    pub fn create_gles2_context(
        &self,
        display: DisplayHandle,
        config: EglConfig,
    ) -> Result<EglContext, EglError> {
        let attribs: [i32; 3] = [egl::CONTEXT_CLIENT_VERSION, 2, egl::NONE];
        // SAFETY: display/config live; no share context; attribs is a
        // fixed EGL_NONE-terminated borrow.
        let ctx = unsafe {
            (self.fns.create_context)(
                display.raw,
                config.raw,
                core::ptr::null_mut(),
                attribs.as_ptr(),
            )
        };
        if ctx.is_null() {
            return Err(EglError::CallFailed {
                while_doing: "eglCreateContext (GLES2)",
                code: 0x300C,
            });
        }
        Ok(EglContext { raw: ctx })
    }

    /// Make `ctx` current with `draw`/`read` on this thread
    /// (`eglMakeCurrent`). Null handles release the current context.
    ///
    /// # Errors
    /// [`EglError::CallFailed`] when the bind fails.
    pub fn make_current(
        &self,
        display: DisplayHandle,
        draw: EglSurface,
        read: EglSurface,
        ctx: EglContext,
    ) -> Result<(), EglError> {
        // SAFETY: display live; surface/context handles are owned by
        // this library's context layer (null = unbind).
        let ok = unsafe { (self.fns.make_current)(display.raw, draw.raw, read.raw, ctx.raw) };
        if ok != egl::SUCCESS {
            return Err(EglError::CallFailed {
                while_doing: "eglMakeCurrent",
                code: ok,
            });
        }
        Ok(())
    }

    /// Destroy a context (`eglDestroyContext`).
    ///
    /// # Errors
    /// [`EglError::CallFailed`] on failure.
    pub fn destroy_context(&self, display: DisplayHandle, ctx: EglContext) -> Result<(), EglError> {
        // SAFETY: ctx is live and owned by the caller.
        let ok = unsafe { (self.fns.destroy_context)(display.raw, ctx.raw) };
        if ok != egl::SUCCESS {
            return Err(EglError::CallFailed {
                while_doing: "eglDestroyContext",
                code: ok,
            });
        }
        Ok(())
    }

    /// Destroy a surface (`eglDestroySurface`).
    ///
    /// # Errors
    /// [`EglError::CallFailed`] on failure.
    pub fn destroy_surface(
        &self,
        display: DisplayHandle,
        surface: EglSurface,
    ) -> Result<(), EglError> {
        // SAFETY: surface is live and owned by the caller.
        let ok = unsafe { (self.fns.destroy_surface)(display.raw, surface.raw) };
        if ok != egl::SUCCESS {
            return Err(EglError::CallFailed {
                while_doing: "eglDestroySurface",
                code: ok,
            });
        }
        Ok(())
    }

    /// Resolve one extension entry point by name
    /// (`eglGetProcAddress`) — the extension contract's own lookup
    /// mechanism. Returns the raw address; callers transmute to the
    /// published prototype at their own audited call sites.
    ///
    /// # Safety
    /// The returned address, if any, belongs to a live EGL library;
    /// transmuting it to a prototype that does not match the symbol's
    /// documented signature is UB — Phase 10's audited fence path owns
    /// that responsibility.
    #[must_use]
    pub unsafe fn proc_address(&self, name: &CStr) -> Option<usize> {
        // SAFETY: `name` is NUL-terminated; eglGetProcAddress only
        // reads it for the duration of the call.
        let ptr = unsafe { (self.fns.get_proc_address)(name.as_ptr()) };
        if ptr.is_null() {
            None
        } else {
            Some(ptr as usize)
        }
    }

    /// Which fence entry points resolved at load time:
    /// `(eglDupNativeFenceFDANDROID, eglWaitSync)`.
    ///
    /// Absence is a typed [`EglError::MissingExtension`] when the
    /// Phase 10 fence path wants them; this accessor is the
    /// capability probe.
    #[must_use]
    pub fn fence_entry_points(&self) -> (bool, bool) {
        (
            self.fns.dup_native_fence_fd.is_some(),
            self.fns.wait_sync.is_some(),
        )
    }

    /// Parse the display's extension list.
    ///
    /// # Errors
    /// [`EglError::CallFailed`] when the query returns NULL.
    pub fn query_extensions(&self, display: DisplayHandle) -> Result<Vec<String>, EglError> {
        // SAFETY: display is initialized; the returned pointer is a
        // static NUL-terminated string owned by EGL, valid until
        // terminate — copied out here.
        let raw = unsafe { (self.fns.query_string)(display.raw, egl::EXTENSIONS) };
        if raw.is_null() {
            return Ok(Vec::new());
        }
        // SAFETY: EGL returns NUL-terminated strings.
        let list = unsafe { CStr::from_ptr(raw) }
            .to_string_lossy()
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        Ok(list)
    }
}

impl Drop for LibEgl {
    fn drop(&mut self) {
        // SAFETY: handle came from dlopen and is dropped exactly once.
        unsafe { libc::dlclose(self.handle) };
    }
}
