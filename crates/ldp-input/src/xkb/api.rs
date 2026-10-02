//! The safe keymap API: compile, serialize, state, modifier
//! decomposition, keysyms, and the sealed client descriptor.
//!
//! Ownership follows the C library's object graph: a [`Context`]
//! creates [`Keymap`]s (which hold their own context reference, so
//! the Rust `Context` may drop after construction), a [`Keymap`]
//! creates [`State`]s (which hold their own keymap reference). Every
//! Rust wrapper unrefs exactly once on drop.
//!
//! Keycodes crossing this API are **evdev** keycodes; the +8 xkbcommon
//! offset is applied here and nowhere else.
//!
//! Keymap determinism: compiling the same RMLVO twice yields the same
//! v1 string on the same library version — the property the golden
//! corpus relies on (tested); the descriptor handed to clients is the
//! same string, sealed (see [`Keymap::client_fd`]).

#![forbid(unsafe_code)]

use std::ffi::CString;
use std::os::fd::OwnedFd;
use std::sync::Arc;

use super::sys::{sealed_memfd, LibXkb, RuleNames, XkbError};

/// RMLVO keymap selection.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Rmlvo {
    /// The rules file ("evdev"); `None` = library default.
    pub rules: Option<String>,
    /// The model ("pc105"); `None` = default.
    pub model: Option<String>,
    /// The layout ("us"); `None` = default.
    pub layout: Option<String>,
    /// The layout variant; `None` = none.
    pub variant: Option<String>,
    /// XKB options string; `None` = none.
    pub options: Option<String>,
}

impl Rmlvo {
    /// The all-defaults selection.
    #[must_use]
    pub fn default_selection() -> Rmlvo {
        Rmlvo::default()
    }

    /// Build the C struct (with owned backing CStrings).
    fn to_c(&self) -> (Vec<CString>, RuleNames) {
        let owned: Vec<CString> = [
            &self.rules,
            &self.model,
            &self.layout,
            &self.variant,
            &self.options,
        ]
        .iter()
        .map(|o| match o {
            Some(s) => CString::new(s.as_str()).unwrap_or_default(),
            None => CString::default(),
        })
        .collect();
        let names = RuleNames {
            rules: owned[0].as_ptr(),
            model: owned[1].as_ptr(),
            layout: owned[2].as_ptr(),
            variant: owned[3].as_ptr(),
            options: owned[4].as_ptr(),
        };
        (owned, names)
    }
}

/// An xkbcommon context.
#[derive(Debug)]
pub struct Context {
    lib: Arc<LibXkb>,
    ctx: *mut std::ffi::c_void,
}

impl Context {
    /// Create a context from a loaded library.
    ///
    /// # Errors
    /// [`XkbError::LibraryLoad`] path is already resolved by
    /// [`super::sys::LibXkb::open`]; here only allocation failure can
    /// surface, reported as [`XkbError::KeymapCompile`] (the library
    /// reports no distinct code; OOM in C is terminal anyway).
    pub fn new(lib: Arc<LibXkb>) -> Result<Context, XkbError> {
        let ctx = lib.context_new().ok_or(XkbError::KeymapCompile)?;
        Ok(Context { lib, ctx })
    }

    /// Compile a keymap from RMLVO names.
    ///
    /// # Errors
    /// [`XkbError::KeymapCompile`] when the selection is invalid or
    /// the compile fails.
    pub fn compile(&self, rmlvo: &Rmlvo) -> Result<Keymap, XkbError> {
        let (_backing, names) = rmlvo.to_c();
        let km = self
            .lib
            .keymap_new_from_names(self.ctx, &names)
            .ok_or(XkbError::KeymapCompile)?;
        Ok(Keymap {
            lib: Arc::clone(&self.lib),
            km,
        })
    }

    /// Compile a keymap from a v1 text serialization.
    ///
    /// # Errors
    /// [`XkbError::KeymapCompile`] when the text is not a valid v1
    /// keymap.
    pub fn from_string(&self, text: &str) -> Result<Keymap, XkbError> {
        let c = CString::new(text).map_err(|_| XkbError::KeymapCompile)?;
        let km = self
            .lib
            .keymap_new_from_string(self.ctx, &c)
            .ok_or(XkbError::KeymapCompile)?;
        Ok(Keymap {
            lib: Arc::clone(&self.lib),
            km,
        })
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        self.lib.context_unref(self.ctx);
    }
}

/// A compiled keymap.
#[derive(Debug)]
pub struct Keymap {
    lib: Arc<LibXkb>,
    km: *mut std::ffi::c_void,
}

impl Keymap {
    /// The v1 text serialization (what clients receive).
    #[must_use]
    pub fn to_v1_string(&self) -> String {
        self.lib.keymap_get_as_string(self.km).unwrap_or_default()
    }

    /// The modifier names, in index order (diagnostics and mask
    /// decomposition).
    #[must_use]
    pub fn mod_names(&self) -> Vec<String> {
        let n = self.lib.keymap_num_mods(self.km);
        (0..n)
            .filter_map(|i| self.lib.keymap_mod_get_name(self.km, i))
            .collect()
    }

    /// The minimum xkb keycode.
    #[must_use]
    pub fn min_keycode(&self) -> u32 {
        self.lib.keymap_min_keycode(self.km)
    }

    /// The maximum xkb keycode.
    #[must_use]
    pub fn max_keycode(&self) -> u32 {
        self.lib.keymap_max_keycode(self.km)
    }

    /// The sealed read-only descriptor carrying the v1 keymap string.
    ///
    /// # Errors
    /// [`XkbError::Io`] for any descriptor-plumbing failure.
    pub fn client_fd(&self) -> Result<OwnedFd, XkbError> {
        sealed_memfd("ldp-keymap", self.to_v1_string().as_bytes())
    }

    /// Create a state tracker over this keymap.
    ///
    /// # Errors
    /// [`XkbError::KeymapCompile`] on allocation failure only.
    pub fn state(&self) -> Result<State, XkbError> {
        let st = self.lib.state_new(self.km).ok_or(XkbError::KeymapCompile)?;
        Ok(State {
            lib: Arc::clone(&self.lib),
            st,
        })
    }
}

impl Drop for Keymap {
    fn drop(&mut self) {
        self.lib.keymap_unref(self.km);
    }
}

/// The modifier state snapshot (the wire's `modifiers` event).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct ModsSnapshot {
    /// Depressed modifiers (physically held).
    pub depressed: u32,
    /// Latched modifiers (one-shot, released by next non-modifier key).
    pub latched: u32,
    /// Locked modifiers (toggled: Caps Lock, Num Lock).
    pub locked: u32,
    /// The effective layout group.
    pub group: u32,
}

/// A keymap state tracker: modifier and layout state per keycode
/// transition.
#[derive(Debug)]
pub struct State {
    lib: Arc<LibXkb>,
    st: *mut std::ffi::c_void,
}

impl State {
    /// Apply an evdev key transition; returns the changed state
    /// components (the library's bit mask — nonzero means the
    /// modifier or layout state changed and a `modifiers` event is
    /// due).
    pub fn update_key(&mut self, evdev_keycode: u32, down: bool) -> u32 {
        self.lib
            .state_update_key(self.st, evdev_keycode + super::sys::xkb::EVDEV_OFFSET, down)
    }

    /// The current modifier snapshot.
    #[must_use]
    pub fn mods(&self) -> ModsSnapshot {
        ModsSnapshot {
            depressed: self
                .lib
                .state_serialize_mods(self.st, super::sys::xkb::STATE_MODS_DEPRESSED),
            latched: self
                .lib
                .state_serialize_mods(self.st, super::sys::xkb::STATE_MODS_LATCHED),
            locked: self
                .lib
                .state_serialize_mods(self.st, super::sys::xkb::STATE_MODS_LOCKED),
            group: self.lib.state_serialize_layout(self.st),
        }
    }

    /// The keysyms an evdev keycode produces under the current state.
    #[must_use]
    pub fn syms(&self, evdev_keycode: u32) -> Vec<u32> {
        self.lib
            .state_key_get_syms(self.st, evdev_keycode + super::sys::xkb::EVDEV_OFFSET)
    }
}

impl Drop for State {
    fn drop(&mut self) {
        self.lib.state_unref(self.st);
    }
}
