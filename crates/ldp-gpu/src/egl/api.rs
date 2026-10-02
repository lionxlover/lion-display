//! The EGL API seam — bootstrap state machine and the import path.
//!
//! [`EglApi`] is the object-safe contract: load/initialize, parse the
//! extension string, gate the DMA-BUF import path on
//! `EGL_EXT_image_dma_buf_import(_modifiers)`, import and destroy
//! images. Two implementations:
//!
//! * [`RealEgl`] — the `dlopen("libEGL.so.1")` path from
//!   [`super::sys`]. On machines without a GL stack the *load itself*
//!   fails, typed — the honest headless outcome, CI-tested here.
//! * [`MockEgl`] — a context over CPU buffers that imports by
//!   **decoding the attribute list** the real path would encode (the
//!   codec round trip runs on every import), then composites with the
//!   reference blend math so the Phase 9 exit criteria can demand
//!   byte-equality against `ldp-renderer`'s software output.
//!
//! The bootstrap state machine is shared vocabulary:
//! `Unloaded → DisplayOpen → Initialized → Capable`: each step's
//! failure is typed, and the capability gate is exactly the extension
//! pair the import path needs — configurable on the mock both ways,
//! because the *absence* of an extension is a path worth testing.

#![forbid(unsafe_code)]

use crate::dmabuf::{DmaBufDescriptor, EglAttrib};
use crate::egl::sys::{DisplayHandle, EglError, LibEgl};
use crate::sync::SyncDriver;
use ldp_core::buffer::FourCC;

/// The extension names the import path gates on.
pub const EXT_DMABUF_IMPORT: &str = "EGL_EXT_image_dma_buf_import";
/// The modifiers companion.
pub const EXT_DMABUF_MODIFIERS: &str = "EGL_EXT_image_dma_buf_import_modifiers";
/// The native fence sync extension.
pub const EXT_NATIVE_FENCE: &str = "EGL_ANDROID_native_fence_sync";

/// The EGL version a display initialized with.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct EglVersion {
    /// Major.
    pub major: i32,
    /// Minor.
    pub minor: i32,
}

/// What bootstrap proved about the display.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Capabilities {
    /// The initialized version, when bootstrap reached initialize.
    pub version: Option<EglVersion>,
    /// `EGL_EXT_image_dma_buf_import` present.
    pub dmabuf_import: bool,
    /// The modifiers companion present.
    pub dmabuf_import_modifiers: bool,
    /// `EGL_ANDROID_native_fence_sync` present.
    pub native_fence_sync: bool,
}

impl Capabilities {
    /// Whether DMA-BUF import can run at all.
    #[must_use]
    pub const fn can_import(&self) -> bool {
        self.dmabuf_import
    }

    /// Whether imports may carry explicit modifiers.
    #[must_use]
    pub const fn can_import_modifiers(&self) -> bool {
        self.dmabuf_import && self.dmabuf_import_modifiers
    }
}

/// An imported image handle. The payload is backend-defined (the real
/// backend stores the `EGLImageKHR` pointer; the mock stores its
/// import-table index) and opaque to every caller.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ImageHandle(pub u64);

/// The EGL API contract.
pub trait EglApi {
    /// Run the bootstrap state machine: display, initialize,
    /// extension parse. Idempotent.
    ///
    /// # Errors
    /// Typed [`EglError`]s at whichever step fails.
    fn bootstrap(&mut self) -> Result<Capabilities, EglError>;

    /// The parsed extension names (empty before bootstrap).
    fn extensions(&self) -> &[String];

    /// Whether one extension is present.
    fn supports(&self, name: &str) -> bool {
        self.extensions().iter().any(|e| e == name)
    }

    /// Import a DMA-BUF image.
    ///
    /// # Errors
    /// [`EglError::CallFailed`] when the capability gate is closed or
    /// the import is rejected (malformed descriptors fail earlier, at
    /// construction).
    fn import_image(&mut self, descriptor: &DmaBufDescriptor) -> Result<ImageHandle, EglError>;

    /// Destroy an imported image.
    ///
    /// # Errors
    /// [`EglError::CallFailed`] for an unknown handle.
    fn destroy_image(&mut self, image: ImageHandle) -> Result<(), EglError>;
}

/// The real `libEGL.so.1` backend.
pub struct RealEgl {
    lib: Option<LibEgl>,
    display: DisplayHandle,
    extension_list: Vec<String>,
    caps: Capabilities,
}

impl core::fmt::Debug for RealEgl {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RealEgl")
            .field("loaded", &self.lib.is_some())
            .field("capabilities", &self.caps)
            .finish_non_exhaustive()
    }
}

impl RealEgl {
    /// A backend in the `Unloaded` state; the first `bootstrap` loads.
    #[must_use]
    pub const fn unloaded() -> Self {
        Self {
            lib: None,
            display: DisplayHandle::null(),
            extension_list: Vec::new(),
            caps: Capabilities {
                version: None,
                dmabuf_import: false,
                dmabuf_import_modifiers: false,
                native_fence_sync: false,
            },
        }
    }
}

impl Default for RealEgl {
    fn default() -> Self {
        Self::unloaded()
    }
}

impl Drop for RealEgl {
    fn drop(&mut self) {
        // Terminate before the library handle closes — the wrapper is
        // safe, the ordering is the ownership discipline.
        if let Some(lib) = self.lib.take() {
            if !self.display.is_null() {
                // A failed terminate during teardown is not actionable;
                // the descriptor close below still releases the rest.
                let _ = lib.terminate(self.display);
            }
        }
    }
}

impl EglApi for RealEgl {
    fn bootstrap(&mut self) -> Result<Capabilities, EglError> {
        if self.caps.version.is_some() {
            return Ok(self.caps.clone());
        }
        // State 1: load.
        let lib = LibEgl::open()?;
        // State 2-3: display + initialize + extensions.
        let boot = lib.bootstrap()?;
        self.display = boot.display;
        boot.extensions.clone_into(&mut self.extension_list);
        let (major, minor) = boot.version;
        let caps = Capabilities {
            version: Some(EglVersion { major, minor }),
            dmabuf_import: self.extension_list.iter().any(|e| e == EXT_DMABUF_IMPORT),
            dmabuf_import_modifiers: self
                .extension_list
                .iter()
                .any(|e| e == EXT_DMABUF_MODIFIERS),
            native_fence_sync: self.extension_list.iter().any(|e| e == EXT_NATIVE_FENCE),
        };
        self.caps = caps.clone();
        self.lib = Some(lib);
        Ok(caps)
    }

    fn extensions(&self) -> &[String] {
        &self.extension_list
    }

    fn import_image(&mut self, descriptor: &DmaBufDescriptor) -> Result<ImageHandle, EglError> {
        let lib = self.lib.as_ref().ok_or(EglError::CallFailed {
            while_doing: "import before bootstrap",
            code: super::sys::egl::NOT_INITIALIZED,
        })?;
        if !self.caps.can_import_modifiers() {
            // The attribute list always carries modifier pairs, so the
            // modifiers extension is on the required path — mirroring
            // EGL, where modifier attribs without the extension are an
            // error even for LINEAR.
            return Err(EglError::CallFailed {
                while_doing: "import without the modifiers extension",
                code: super::sys::egl::BAD_ACCESS,
            });
        }
        // The attribute list crosses as EGLint pairs terminated by
        // EGL_NONE — exactly the encoding dmabuf.rs owns.
        let attribs = descriptor.attribs();
        let mut flat: Vec<i32> = Vec::with_capacity(attribs.len() * 2);
        for (name, value) in &attribs {
            flat.push(*name as i32);
            flat.push(i32::try_from(*value).map_err(|_| EglError::CallFailed {
                while_doing: "attribute value overflows EGLint",
                code: super::sys::egl::BAD_PARAMETER,
            })?);
        }
        flat.push(super::sys::egl::NONE);
        let image = lib.create_dmabuf_image(self.display, &flat)?;
        Ok(ImageHandle(u64::try_from(image).map_err(|_| {
            EglError::CallFailed {
                while_doing: "image handle overflows the u64 space",
                code: super::sys::egl::BAD_PARAMETER,
            }
        })?))
    }

    fn destroy_image(&mut self, image: ImageHandle) -> Result<(), EglError> {
        let lib = self.lib.as_ref().ok_or(EglError::CallFailed {
            while_doing: "destroy before bootstrap",
            code: super::sys::egl::NOT_INITIALIZED,
        })?;
        if image.0 == 0 {
            return Err(EglError::CallFailed {
                while_doing: "destroy the null image",
                code: super::sys::egl::BAD_PARAMETER,
            });
        }
        let raw = usize::try_from(image.0).map_err(|_| EglError::CallFailed {
            while_doing: "image handle does not fit the pointer space",
            code: super::sys::egl::BAD_PARAMETER,
        })?;
        lib.destroy_image(self.display, raw)
    }
}

/// A registered CPU buffer backing a mock dma-buf fd.
#[derive(Clone, Debug)]
struct MockBuffer {
    geometry: ldp_core::buffer::BufferGeometry,
    data: std::rc::Rc<Vec<u8>>,
}

/// The mock EGL context: imports by decoding the attribute list, and
/// composites with the reference blend math.
///
/// One integer fd namespace backs the mock; [`MockEgl::register`]
/// binds an fd to a CPU buffer (the "allocator" seam), and imports
/// resolve fds through it — an unknown fd is a typed error, exactly
/// like an invalid dma-buf on real hardware.
pub struct MockEgl {
    extensions: Vec<String>,
    caps: Option<Capabilities>,
    buffers: std::collections::BTreeMap<i32, MockBuffer>,
    imported: Vec<Option<DmaBufDescriptor>>,
}

impl core::fmt::Debug for MockEgl {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("MockEgl")
            .field("extensions", &self.extensions)
            .field("registered", &self.buffers.len())
            .field("imported", &self.imported.len())
            .finish_non_exhaustive()
    }
}

impl MockEgl {
    /// A mock context advertising the given extension names.
    #[must_use]
    pub fn with_extensions(extensions: &[&str]) -> Self {
        Self {
            extensions: extensions.iter().map(|s| (*s).to_owned()).collect(),
            caps: None,
            buffers: std::collections::BTreeMap::new(),
            imported: Vec::new(),
        }
    }

    /// A mock context with the full import stack present.
    #[must_use]
    pub fn capable() -> Self {
        Self::with_extensions(&[EXT_DMABUF_IMPORT, EXT_DMABUF_MODIFIERS, EXT_NATIVE_FENCE])
    }

    /// Bind a mock fd to a CPU buffer.
    pub fn register(
        &mut self,
        fd: i32,
        geometry: ldp_core::buffer::BufferGeometry,
        data: std::rc::Rc<Vec<u8>>,
    ) {
        self.buffers.insert(fd, MockBuffer { geometry, data });
    }

    /// The descriptor a handle imported (test introspection); `None`
    /// for destroyed or unknown handles.
    #[must_use]
    pub fn descriptor_of(&self, image: ImageHandle) -> Option<&DmaBufDescriptor> {
        self.imported.get(image.0 as usize)?.as_ref()
    }

    /// Composite `layers` (back-to-front) into a fresh ARGB8888 or
    /// XRGB8888 framebuffer of `width` x `height`, gated on `acquire`.
    ///
    /// The blend math is the reference renderer's — premultiplied
    /// source-over with the shared `mul255` rounding — so byte-equality
    /// with `ldp-renderer` is a genuine cross-implementation oracle,
    /// not a tautology. The representable subset: 1:1 placement, no
    /// transforms, ARGB8888/XRGB8888 layers over an ARGB8888/XRGB8888
    /// target, uniform per-layer opacity.
    ///
    /// # Errors
    /// [`EglError::CallFailed`] when the acquire fence is pending (the
    /// fenced-draw contract: retry after the driver clock advances),
    /// when a layer references an unknown image, or when geometry
    /// steps outside the representable subset.
    pub fn draw_and_readback(
        &mut self,
        width: u32,
        height: u32,
        format: FourCC,
        layers: &[(ImageHandle, i32, i32, u8)],
        acquire: &crate::sync::Fence,
        driver: &crate::sync::MockSyncDriver,
    ) -> Result<Vec<u8>, EglError> {
        if !driver
            .state(acquire)
            .map_err(|_| EglError::CallFailed {
                while_doing: "acquire fence lookup",
                code: super::sys::egl::BAD_PARAMETER,
            })?
            .is_signaled()
        {
            return Err(EglError::CallFailed {
                while_doing: "fenced draw with a pending acquire fence",
                code: super::sys::egl::BAD_ACCESS,
            });
        }
        let reject = |what: &'static str| EglError::CallFailed {
            while_doing: what,
            code: super::sys::egl::BAD_PARAMETER,
        };
        let four = width
            .checked_mul(height)
            .ok_or(reject("output overflows"))?;
        // Fresh framebuffers start opaque black — the reference
        // renderer's own fresh-frame contract.
        let mut fb = vec![0xFF00_0000u32; four as usize];
        // Back-to-front: each layer blends over the current target.
        for (handle, dx, dy, opacity) in layers {
            let descriptor = self
                .imported
                .get(handle.0 as usize)
                .and_then(Option::as_ref)
                .ok_or(reject("unknown image"))?;
            let buffer = self
                .buffers
                .get(&descriptor.planes[0].fd)
                .ok_or(reject("image fd not registered"))?;
            if descriptor.format != FourCC::ARGB8888 && descriptor.format != FourCC::XRGB8888 {
                return Err(reject("layer format outside the representable subset"));
            }
            if descriptor.width == 0 || descriptor.height == 0 {
                return Err(reject("empty layer"));
            }
            if *dx < 0 || *dy < 0 {
                return Err(reject("layer placement overflows the target"));
            }
            let origin_x = u32::try_from(*dx).map_err(|_| reject("placement overflows"))?;
            let origin_y = u32::try_from(*dy).map_err(|_| reject("placement overflows"))?;
            if origin_x
                .checked_add(descriptor.width)
                .map_or(true, |end| end > width)
                || origin_y
                    .checked_add(descriptor.height)
                    .map_or(true, |end| end > height)
            {
                return Err(reject("layer placement overflows the target"));
            }
            let stride = descriptor.planes[0].stride;
            let plane = buffer.geometry.planes()[0];
            for row in 0..descriptor.height {
                let src_line = plane.offset as usize + stride as usize * row as usize;
                let dst_y = origin_y + row;
                for col in 0..descriptor.width {
                    let s = src_line + col as usize * 4;
                    if s + 4 > buffer.data.len() {
                        return Err(reject("layer buffer is short"));
                    }
                    let word = u32::from_ne_bytes([
                        buffer.data[s],
                        buffer.data[s + 1],
                        buffer.data[s + 2],
                        buffer.data[s + 3],
                    ]);
                    // The X byte of X-family buffers is garbage by
                    // definition (the pattern stores alpha there as a
                    // trap); samplers must ignore it.
                    let word = if descriptor.format == FourCC::XRGB8888 {
                        word | 0xFF00_0000
                    } else {
                        word
                    };
                    let dst_x = origin_x + col;
                    let di = (dst_y * width + dst_x) as usize;
                    fb[di] = blend_word(word, fb[di], *opacity);
                }
            }
        }
        // Pack the target words into the requested output format.
        let mut out = Vec::with_capacity(fb.len() * 4);
        for word in fb {
            match format {
                FourCC::ARGB8888 => out.extend_from_slice(&word.to_ne_bytes()),
                FourCC::XRGB8888 => out.extend_from_slice(&(word | 0xFF00_0000).to_ne_bytes()),
                _ => return Err(reject("target format outside the representable subset")),
            }
        }
        Ok(out)
    }
}

/// The reference blend: premultiplied source-over with the opacity
/// folded into the source alpha — `a2 = mul255(sa, q)`,
/// `inv = 255 - a2` — and sums clamped to the result alpha, exactly
/// `ldp-renderer`'s composite rule (the one every path there shares).
fn blend_word(src: u32, dst: u32, opacity: u8) -> u32 {
    let unpack = |w: u32| {
        [
            w & 0xFF,
            (w >> 8) & 0xFF,
            (w >> 16) & 0xFF,
            (w >> 24) & 0xFF,
        ]
    };
    let mul255 = |v: u32, a: u32| -> u32 { (v * a + 127) / 255 };
    let px = unpack(src);
    let d = unpack(dst);
    let q = u32::from(opacity);
    let a2 = mul255(px[3], q);
    let inv = 255 - a2;
    let oa = a2 + mul255(d[3], inv);
    let chan = |i: usize| (mul255(px[i], q) + mul255(d[i], inv)).min(oa);
    (oa << 24) | (chan(2) << 16) | (chan(1) << 8) | chan(0)
}

impl EglApi for MockEgl {
    fn bootstrap(&mut self) -> Result<Capabilities, EglError> {
        if self.caps.is_some() {
            return Ok(self.caps.clone().expect("checked above"));
        }
        let caps = Capabilities {
            version: Some(EglVersion { major: 1, minor: 5 }),
            dmabuf_import: self.extensions.iter().any(|e| e == EXT_DMABUF_IMPORT),
            dmabuf_import_modifiers: self.extensions.iter().any(|e| e == EXT_DMABUF_MODIFIERS),
            native_fence_sync: self.extensions.iter().any(|e| e == EXT_NATIVE_FENCE),
        };
        self.caps = Some(caps.clone());
        Ok(caps)
    }

    fn extensions(&self) -> &[String] {
        &self.extensions
    }

    fn import_image(&mut self, descriptor: &DmaBufDescriptor) -> Result<ImageHandle, EglError> {
        let caps = self.caps.clone().unwrap_or_default();
        if !caps.can_import_modifiers() {
            return Err(EglError::CallFailed {
                while_doing: "import without the full modifiers stack",
                code: super::sys::egl::BAD_ACCESS,
            });
        }
        // Import by decoding the encoded attribute list: the codec
        // round trip is the import on the mock.
        let attribs: Vec<EglAttrib> = descriptor.attribs();
        let decoded = DmaBufDescriptor::decode(&attribs).ok_or(EglError::CallFailed {
            while_doing: "attribute round trip",
            code: super::sys::egl::BAD_PARAMETER,
        })?;
        if decoded != *descriptor {
            return Err(EglError::CallFailed {
                while_doing: "attribute round trip diverged",
                code: super::sys::egl::BAD_PARAMETER,
            });
        }
        // The fd must resolve against the registered allocator.
        for plane in &decoded.planes {
            if !self.buffers.contains_key(&plane.fd) {
                return Err(EglError::CallFailed {
                    while_doing: "import of an unregistered dma-buf fd",
                    code: super::sys::egl::BAD_PARAMETER,
                });
            }
        }
        self.imported.push(Some(decoded));
        let index = self.imported.len() - 1;
        Ok(ImageHandle(u64::try_from(index).map_err(|_| {
            EglError::CallFailed {
                while_doing: "image count overflows the handle space",
                code: super::sys::egl::BAD_PARAMETER,
            }
        })?))
    }

    fn destroy_image(&mut self, image: ImageHandle) -> Result<(), EglError> {
        let index = image.0 as usize;
        let slot = self.imported.get_mut(index).ok_or(EglError::CallFailed {
            while_doing: "destroy an unknown image",
            code: super::sys::egl::BAD_PARAMETER,
        })?;
        if slot.is_none() {
            return Err(EglError::CallFailed {
                while_doing: "destroy an already-destroyed image",
                code: super::sys::egl::BAD_PARAMETER,
            });
        }
        *slot = None;
        Ok(())
    }
}
