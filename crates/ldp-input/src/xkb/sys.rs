//! The libxkbcommon runtime layer — dlopen, symbols, opaque types.
//!
//! This is the crate's **audit boundary**: every `unsafe` in the xkb
//! tree lives here with per-call `SAFETY` comments. The library is
//! loaded at runtime (`dlopen("libxkbcommon.so.0")`); headless
//! machines without it get a typed [`XkbError::LibraryLoad`], never a
//! link-time dependency. Every xkbcommon type this crate touches is
//! an opaque pointer (`xkb_context`, `xkb_keymap`, `xkb_state`) or a
//! plain `u32`/`i32` — the symbol table below is the whole ABI
//! surface consumed.
//!
//! The same layer owns the keymap descriptor: a sealed memfd holding
//! the v1 keymap string, created write-once and handed to clients
//! read-only (the wayland keymap protocol shape).

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::io;
use std::os::fd::{FromRawFd, OwnedFd};

/// SONAME.
const LIBXKBCOMMON: &str = "libxkbcommon.so.0";

/// xkbcommon ABI constants (xkbcommon.h, stable since 0.2).
pub mod xkb {
    /// `XKB_CONTEXT_NO_FLAGS`.
    pub const CONTEXT_NO_FLAGS: u32 = 0;
    /// `XKB_KEYMAP_COMPILE_NO_FLAGS`.
    pub const KEYMAP_COMPILE_NO_FLAGS: u32 = 0;
    /// `XKB_KEYMAP_FORMAT_TEXT_V1`.
    pub const KEYMAP_FORMAT_TEXT_V1: u32 = 1;
    /// `XKB_KEY_UP`.
    pub const KEY_UP: u32 = 0;
    /// `XKB_KEY_DOWN`.
    pub const KEY_DOWN: u32 = 1;
    /// `XKB_STATE_MODS_DEPRESSED` (verified against the 1.7.0
    /// header: the mods components occupy the low nibble).
    pub const STATE_MODS_DEPRESSED: u32 = 1 << 0;
    /// `XKB_STATE_MODS_LATCHED`.
    pub const STATE_MODS_LATCHED: u32 = 1 << 1;
    /// `XKB_STATE_MODS_LOCKED`.
    pub const STATE_MODS_LOCKED: u32 = 1 << 2;
    /// `XKB_STATE_MODS_EFFECTIVE`.
    pub const STATE_MODS_EFFECTIVE: u32 = 1 << 3;
    /// `XKB_STATE_LAYOUT_EFFECTIVE`.
    pub const STATE_LAYOUT_EFFECTIVE: u32 = 1 << 7;
    /// The evdev→xkb keycode offset (kernel keycodes start at 8 in
    /// xkbcommon's numbering).
    pub const EVDEV_OFFSET: u32 = 8;
}

/// `struct xkb_rule_names` (RMLVO).
#[repr(C)]
pub struct RuleNames {
    /// The rules file ("evdev", or NULL for the default).
    pub rules: *const c_char,
    /// The model ("pc105", or NULL).
    pub model: *const c_char,
    /// The layout ("us", "de", or NULL).
    pub layout: *const c_char,
    /// The layout variant, or NULL.
    pub variant: *const c_char,
    /// XKB options, or NULL.
    pub options: *const c_char,
}

/// Errors of the FFI layer.
#[derive(Debug)]
pub enum XkbError {
    /// `dlopen` failed (library absent).
    LibraryLoad(String),
    /// A required symbol is missing from the loaded library.
    SymbolMissing(&'static str),
    /// A keymap failed to compile.
    KeymapCompile,
    /// Descriptor plumbing failed.
    Io(io::Error),
}

impl std::fmt::Display for XkbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            XkbError::LibraryLoad(name) => write!(f, "cannot load {name}"),
            XkbError::SymbolMissing(sym) => write!(f, "symbol {sym} missing"),
            XkbError::KeymapCompile => write!(f, "keymap compilation failed"),
            XkbError::Io(e) => write!(f, "io: {e}"),
        }
    }
}

impl std::error::Error for XkbError {}

/// The loaded library: handle plus the resolved symbol table.
///
/// Function-pointer types are declared inline; every entry mirrors
/// the C prototype exactly.
#[allow(non_snake_case)]
pub struct LibXkb {
    handle: *mut c_void,
    pub(crate) xkb_context_new: unsafe extern "C" fn(u32) -> *mut c_void,
    pub(crate) xkb_context_unref: unsafe extern "C" fn(*mut c_void),
    pub(crate) xkb_keymap_new_from_names:
        unsafe extern "C" fn(*mut c_void, *const RuleNames, u32) -> *mut c_void,
    pub(crate) xkb_keymap_new_from_string:
        unsafe extern "C" fn(*mut c_void, *const c_char, u32, u32) -> *mut c_void,
    pub(crate) xkb_keymap_unref: unsafe extern "C" fn(*mut c_void),
    pub(crate) xkb_keymap_get_as_string: unsafe extern "C" fn(*mut c_void, u32) -> *mut c_char,
    pub(crate) xkb_state_new: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    pub(crate) xkb_state_unref: unsafe extern "C" fn(*mut c_void),
    pub(crate) xkb_state_update_key: unsafe extern "C" fn(*mut c_void, u32, u32) -> u32,
    pub(crate) xkb_state_serialize_mods: unsafe extern "C" fn(*mut c_void, u32) -> u32,
    pub(crate) xkb_state_serialize_layout: unsafe extern "C" fn(*mut c_void, u32) -> u32,
    pub(crate) xkb_state_key_get_syms:
        unsafe extern "C" fn(*mut c_void, u32, *mut *const u32) -> c_int,
    pub(crate) xkb_keymap_num_mods: unsafe extern "C" fn(*mut c_void) -> u32,
    pub(crate) xkb_keymap_mod_get_name: unsafe extern "C" fn(*mut c_void, u32) -> *const c_char,
    pub(crate) xkb_keymap_min_keycode: unsafe extern "C" fn(*mut c_void) -> u32,
    pub(crate) xkb_keymap_max_keycode: unsafe extern "C" fn(*mut c_void) -> u32,
}

// The raw handle is Send/Sync-safe: dlopen handles are refcounted by
// the loader and the library is thread-safe (xkbcommon is documented
// as such for these entry points; all objects created from one handle
// stay on the owning thread here).
unsafe impl Send for LibXkb {}
unsafe impl Sync for LibXkb {}

// The compiled keymap state is Send-safe under the &mut discipline:
// the C state carries no thread affinity and no thread-local storage,
// and this crate only mutates it through `&mut self` (never shared
// across threads while borrowed). Embedders that park it behind a
// mutex (the compositor's world lock) hold that discipline by
// construction; moving the handle between threads is sound.
unsafe impl Send for crate::xkb::api::State {}

/// Resolve one symbol or fail typed.
macro_rules! sym {
    ($handle:expr, $name:literal, $ty:ty) => {{
        // SAFETY: the symbol name is NUL-terminated; dlsym only reads it.
        let ptr = unsafe { libc::dlsym($handle, concat!($name, "\0").as_ptr().cast::<c_char>()) };
        if ptr.is_null() {
            return Err(XkbError::SymbolMissing($name));
        }
        // SAFETY: the transmute is sound because $ty is exactly the C
        // prototype of $name (declared inline above).
        unsafe { core::mem::transmute::<*mut c_void, $ty>(ptr) }
    }};
}

impl LibXkb {
    /// `dlopen("libxkbcommon.so.0")` and resolve the symbol table.
    ///
    /// # Errors
    /// [`XkbError::LibraryLoad`] when the library is absent;
    /// [`XkbError::SymbolMissing`] for a partial symbol table.
    pub fn open() -> Result<LibXkb, XkbError> {
        let cname =
            CString::new(LIBXKBCOMMON).map_err(|_| XkbError::LibraryLoad(LIBXKBCOMMON.into()))?;
        // SAFETY: the literal is NUL-terminated; dlopen only reads it.
        let handle = unsafe { libc::dlopen(cname.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
        let handle = match handle {
            null if null.is_null() => {
                return Err(XkbError::LibraryLoad(LIBXKBCOMMON.into()));
            }
            h => h,
        };
        Ok(LibXkb {
            handle,
            xkb_context_new: sym!(
                handle,
                "xkb_context_new",
                unsafe extern "C" fn(u32) -> *mut c_void
            ),
            xkb_context_unref: sym!(
                handle,
                "xkb_context_unref",
                unsafe extern "C" fn(*mut c_void)
            ),
            xkb_keymap_new_from_names: sym!(
                handle,
                "xkb_keymap_new_from_names",
                unsafe extern "C" fn(*mut c_void, *const RuleNames, u32) -> *mut c_void
            ),
            xkb_keymap_new_from_string: sym!(
                handle,
                "xkb_keymap_new_from_string",
                unsafe extern "C" fn(*mut c_void, *const c_char, u32, u32) -> *mut c_void
            ),
            xkb_keymap_unref: sym!(
                handle,
                "xkb_keymap_unref",
                unsafe extern "C" fn(*mut c_void)
            ),
            xkb_keymap_get_as_string: sym!(
                handle,
                "xkb_keymap_get_as_string",
                unsafe extern "C" fn(*mut c_void, u32) -> *mut c_char
            ),
            xkb_state_new: sym!(
                handle,
                "xkb_state_new",
                unsafe extern "C" fn(*mut c_void) -> *mut c_void
            ),
            xkb_state_unref: sym!(handle, "xkb_state_unref", unsafe extern "C" fn(*mut c_void)),
            xkb_state_update_key: sym!(
                handle,
                "xkb_state_update_key",
                unsafe extern "C" fn(*mut c_void, u32, u32) -> u32
            ),
            xkb_state_serialize_mods: sym!(
                handle,
                "xkb_state_serialize_mods",
                unsafe extern "C" fn(*mut c_void, u32) -> u32
            ),
            xkb_state_serialize_layout: sym!(
                handle,
                "xkb_state_serialize_layout",
                unsafe extern "C" fn(*mut c_void, u32) -> u32
            ),
            xkb_state_key_get_syms: sym!(
                handle,
                "xkb_state_key_get_syms",
                unsafe extern "C" fn(*mut c_void, u32, *mut *const u32) -> c_int
            ),
            xkb_keymap_num_mods: sym!(
                handle,
                "xkb_keymap_num_mods",
                unsafe extern "C" fn(*mut c_void) -> u32
            ),
            xkb_keymap_mod_get_name: sym!(
                handle,
                "xkb_keymap_mod_get_name",
                unsafe extern "C" fn(*mut c_void, u32) -> *const c_char
            ),
            xkb_keymap_min_keycode: sym!(
                handle,
                "xkb_keymap_min_keycode",
                unsafe extern "C" fn(*mut c_void) -> u32
            ),
            xkb_keymap_max_keycode: sym!(
                handle,
                "xkb_keymap_max_keycode",
                unsafe extern "C" fn(*mut c_void) -> u32
            ),
        })
    }

    /// The raw handle (diagnostics).
    #[must_use]
    pub fn is_open(&self) -> bool {
        !self.handle.is_null()
    }
}

impl LibXkb {
    /// `xkb_context_new` (NULL-checked).
    #[must_use]
    pub(crate) fn context_new(&self) -> Option<*mut c_void> {
        // SAFETY: flags are a defined constant; NULL return means
        // allocation failure, surfaced as None.
        let p = unsafe { (self.xkb_context_new)(xkb::CONTEXT_NO_FLAGS) };
        (!p.is_null()).then_some(p)
    }

    /// `xkb_context_unref`.
    pub(crate) fn context_unref(&self, ctx: *mut c_void) {
        // SAFETY: ctx came from context_new on this handle.
        unsafe { (self.xkb_context_unref)(ctx) };
    }

    /// `xkb_keymap_new_from_names` (NULL-checked).
    #[must_use]
    pub(crate) fn keymap_new_from_names(
        &self,
        ctx: *mut c_void,
        names: &RuleNames,
    ) -> Option<*mut c_void> {
        // SAFETY: ctx is a live context of this handle; names is a
        // repr(C) struct of NUL-terminated pointers valid for the call.
        let p =
            unsafe { (self.xkb_keymap_new_from_names)(ctx, names, xkb::KEYMAP_COMPILE_NO_FLAGS) };
        (!p.is_null()).then_some(p)
    }

    /// `xkb_keymap_new_from_string` (NULL-checked).
    #[must_use]
    pub(crate) fn keymap_new_from_string(&self, ctx: *mut c_void, s: &CStr) -> Option<*mut c_void> {
        // SAFETY: ctx live; s NUL-terminated for the call duration;
        // format and flags are defined constants (v1 / no flags).
        let p = unsafe {
            (self.xkb_keymap_new_from_string)(
                ctx,
                s.as_ptr(),
                xkb::KEYMAP_FORMAT_TEXT_V1,
                xkb::KEYMAP_COMPILE_NO_FLAGS,
            )
        };
        (!p.is_null()).then_some(p)
    }

    /// `xkb_keymap_unref`.
    pub(crate) fn keymap_unref(&self, km: *mut c_void) {
        // SAFETY: km came from a keymap constructor on this handle.
        unsafe { (self.xkb_keymap_unref)(km) };
    }

    /// `xkb_keymap_get_as_string` (v1), copied into a `String` and
    /// freed (the library hands ownership to the caller).
    #[must_use]
    pub(crate) fn keymap_get_as_string(&self, km: *mut c_void) -> Option<String> {
        // SAFETY: km live; the returned pointer is a caller-owned
        // NUL-terminated allocation per the documented contract.
        let p = unsafe { (self.xkb_keymap_get_as_string)(km, xkb::KEYMAP_FORMAT_TEXT_V1) };
        if p.is_null() {
            return None;
        }
        // SAFETY: p is a valid NUL-terminated string; copied before free.
        let out = Some(unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned());
        // SAFETY: p came from the library's allocator (malloc), freed
        // exactly once here.
        unsafe { libc::free(p.cast::<c_void>()) };
        out
    }

    /// `xkb_state_new` (NULL-checked).
    #[must_use]
    pub(crate) fn state_new(&self, km: *mut c_void) -> Option<*mut c_void> {
        // SAFETY: km is a live keymap; NULL means allocation failure.
        let p = unsafe { (self.xkb_state_new)(km) };
        (!p.is_null()).then_some(p)
    }

    /// `xkb_state_unref`.
    pub(crate) fn state_unref(&self, st: *mut c_void) {
        // SAFETY: st came from state_new on this handle.
        unsafe { (self.xkb_state_unref)(st) };
    }

    /// `xkb_state_update_key`; returns the changed state components.
    pub(crate) fn state_update_key(&self, st: *mut c_void, keycode: u32, down: bool) -> u32 {
        let dir = if down { xkb::KEY_DOWN } else { xkb::KEY_UP };
        // SAFETY: st live; keycode/direction are plain values.
        unsafe { (self.xkb_state_update_key)(st, keycode, dir) }
    }

    /// `xkb_state_serialize_mods` for one component.
    pub(crate) fn state_serialize_mods(&self, st: *mut c_void, component: u32) -> u32 {
        // SAFETY: st live; component is a defined constant.
        unsafe { (self.xkb_state_serialize_mods)(st, component) }
    }

    /// `xkb_state_serialize_layout` (effective group).
    pub(crate) fn state_serialize_layout(&self, st: *mut c_void) -> u32 {
        // SAFETY: st live; component constant.
        unsafe { (self.xkb_state_serialize_layout)(st, xkb::STATE_LAYOUT_EFFECTIVE) }
    }

    /// `xkb_state_key_get_syms`, copied.
    #[must_use]
    pub(crate) fn state_key_get_syms(&self, st: *mut c_void, keycode: u32) -> Vec<u32> {
        let mut syms: *const u32 = std::ptr::null();
        // SAFETY: st live; syms_out is a valid out-pointer for the call.
        let n = unsafe { (self.xkb_state_key_get_syms)(st, keycode, &mut syms) };
        if n <= 0 || syms.is_null() {
            return Vec::new();
        }
        // SAFETY: the library guarantees n valid keysyms at syms for
        // the duration of the call window; we copy immediately.
        unsafe { std::slice::from_raw_parts(syms, n as usize) }.to_vec()
    }

    /// `xkb_keymap_num_mods`.
    pub(crate) fn keymap_num_mods(&self, km: *mut c_void) -> u32 {
        // SAFETY: km live.
        unsafe { (self.xkb_keymap_num_mods)(km) }
    }

    /// `xkb_keymap_mod_get_name`, copied.
    #[must_use]
    pub(crate) fn keymap_mod_get_name(&self, km: *mut c_void, idx: u32) -> Option<String> {
        // SAFETY: km live; idx < num_mods enforced by the caller.
        let p = unsafe { (self.xkb_keymap_mod_get_name)(km, idx) };
        // SAFETY: p is NULL or a valid string owned by the keymap.
        unsafe { cstr_to_string(p) }
    }

    /// `xkb_keymap_min_keycode`.
    pub(crate) fn keymap_min_keycode(&self, km: *mut c_void) -> u32 {
        // SAFETY: km live.
        unsafe { (self.xkb_keymap_min_keycode)(km) }
    }

    /// `xkb_keymap_max_keycode`.
    pub(crate) fn keymap_max_keycode(&self, km: *mut c_void) -> u32 {
        // SAFETY: km live.
        unsafe { (self.xkb_keymap_max_keycode)(km) }
    }
}

impl std::fmt::Debug for LibXkb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LibXkb")
            .field("handle", &self.handle)
            .field("loaded", &self.is_open())
            .finish_non_exhaustive()
    }
}

impl Drop for LibXkb {
    fn drop(&mut self) {
        // SAFETY: handle came from dlopen and is dropped exactly once.
        unsafe { libc::dlclose(self.handle) };
    }
}

/// A sealed read-only memfd carrying `bytes`.
///
/// The write-once keymap descriptor handed to clients: created with
/// `memfd_create(MFD_CLOEXEC)`, sized exactly, written, rewound to
/// offset zero, then sealed (`SHRINK|GROW|WRITE`) so a hostile client
/// can neither extend nor rewrite the mapping its siblings share.
///
/// # Errors
/// [`XkbError::Io`] for any syscall failure.
pub fn sealed_memfd(name: &str, bytes: &[u8]) -> Result<OwnedFd, XkbError> {
    use std::io::Write;
    use std::os::fd::FromRawFd;
    use std::os::unix::io::AsRawFd;

    let cname = CString::new(name)
        .map_err(|e| XkbError::Io(io::Error::new(io::ErrorKind::InvalidInput, e)))?;
    // SAFETY: NUL-terminated name; return checked before adoption.
    // MFD_ALLOW_SEALING is required for F_ADD_SEALS to be permitted.
    let ret =
        unsafe { libc::memfd_create(cname.as_ptr(), libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING) };
    if ret < 0 {
        return Err(XkbError::Io(io::Error::last_os_error()));
    }
    // SAFETY: freshly created, unowned descriptor.
    let file = unsafe { OwnedFd::from_raw_fd(ret as i32) };
    // SAFETY: raw fd of an OwnedFd we still own; libc ftruncate only
    // sees the number.
    let trunc = unsafe { libc::ftruncate(file.as_raw_fd(), bytes.len() as libc::off_t) };
    if trunc != 0 {
        return Err(XkbError::Io(io::Error::last_os_error()));
    }
    {
        let mut w = std::fs::File::from(try_clone_fd(&file)?);
        w.write_all(bytes).map_err(XkbError::Io)?;
        w.flush().map_err(XkbError::Io)?;
    }
    // SAFETY: fd owned; lseek to zero so read()-style consumers start
    // at the keymap's beginning.
    let seek = unsafe { libc::lseek(file.as_raw_fd(), 0, libc::SEEK_SET) };
    if seek < 0 {
        return Err(XkbError::Io(io::Error::last_os_error()));
    }
    // SAFETY: fd owned, no writable mappings exist, seals are pure
    // descriptor state.
    let seals = libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_WRITE;
    let fcntl_ret = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_ADD_SEALS, seals) };
    if fcntl_ret != 0 {
        return Err(XkbError::Io(io::Error::last_os_error()));
    }
    Ok(file)
}

/// Duplicate an `OwnedFd` (the memfd writer needs its own copy).
fn try_clone_fd(fd: &OwnedFd) -> Result<OwnedFd, XkbError> {
    use std::os::unix::io::AsRawFd;
    // SAFETY: dup on a live, owned fd; result checked before adoption.
    let ret = unsafe { libc::dup(fd.as_raw_fd()) };
    if ret < 0 {
        return Err(XkbError::Io(io::Error::last_os_error()));
    }
    // SAFETY: freshly duplicated, unowned descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(ret as i32) })
}

/// The seal bits currently set on a descriptor (diagnostics/tests).
///
/// # Errors
/// [`XkbError::Io`] when the descriptor does not support seals.
pub fn get_seals(fd: &OwnedFd) -> Result<i32, XkbError> {
    use std::os::unix::io::AsRawFd;
    // SAFETY: fd owned; F_GET_SEALS is a pure query.
    let ret = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GET_SEALS) };
    if ret < 0 {
        return Err(XkbError::Io(io::Error::last_os_error()));
    }
    Ok(ret)
}

/// Read a C string returned by the library (statically allocated by
/// the keymap's lifetime — copied immediately).
///
/// # Safety
/// The pointer must be NULL or a valid NUL-terminated string owned by
/// a live xkbcommon object for the duration of this call.
pub(crate) unsafe fn cstr_to_string(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: caller guarantees validity; CStr only reads.
    Some(
        unsafe { CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned(),
    )
}
