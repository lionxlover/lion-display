//! Keymap compilation and state through runtime-loaded xkbcommon.
//!
//! * [`sys`] — the audited FFI boundary: `dlopen("libxkbcommon.so.0")`,
//!   the symbol table, the sealed keymap descriptor. All `unsafe`
//!   in this tree lives there, each call with a `SAFETY` comment.
//! * [`api`] — the safe object graph: [`api::Context`] →
//!   [`api::Keymap`] → [`api::State`], RMLVO selection, v1
//!   serialization, modifier decomposition, keysyms.
//!
//! The headless contract: machines without libxkbcommon get a typed
//! [`sys::XkbError::LibraryLoad`] from [`sys::LibXkb::open`] — the
//! seat layer treats that as "no keymap yet", which is an honest
//! state, not a crash. The golden corpus runs against the real
//! library when present (CI machines carry it; the tests skip with a
//! visible message otherwise).

pub mod api;
pub mod sys;

pub use api::{Context, Keymap, ModsSnapshot, Rmlvo, State};
pub use sys::XkbError;

#[cfg(test)]
mod tests {
    use super::api::{Context, Keymap, Rmlvo, State};
    use super::sys::{get_seals, LibXkb, XkbError};
    use crate::codes::key;
    use std::io::Read;
    use std::os::fd::AsRawFd;
    use std::sync::Arc;

    /// The library this sandbox carries (absent machines skip).
    fn library() -> Option<Arc<LibXkb>> {
        LibXkb::open().ok().map(Arc::new)
    }

    fn default_keymap() -> Option<Keymap> {
        let lib = library()?;
        let ctx = Context::new(lib).ok()?;
        ctx.compile(&Rmlvo::default_selection()).ok()
    }

    fn default_state() -> Option<State> {
        default_keymap()?.state().ok()
    }

    #[test]
    fn default_keymap_compiles_and_serializes() {
        let Some(km) = default_keymap() else {
            eprintln!("SKIP: libxkbcommon not present");
            return;
        };
        let s = km.to_v1_string();
        assert!(!s.is_empty());
        assert!(
            s.starts_with("xkb_keymap"),
            "v1 header: {}",
            &s[..24.min(s.len())]
        );
        // The evdev range covers the keys we route.
        assert!(km.min_keycode() <= 8 + key::A);
        assert!(km.max_keycode() >= 8 + key::RIGHTMETA);
        // Modifier names include the canonical set.
        let mods = km.mod_names();
        assert!(mods.iter().any(|m| m == "Shift"), "mods: {mods:?}");
        assert!(mods.iter().any(|m| m == "Lock"), "mods: {mods:?}");
    }

    #[test]
    fn keymap_compilation_is_deterministic() {
        let Some(lib) = library() else {
            eprintln!("SKIP: libxkbcommon not present");
            return;
        };
        let ctx = Context::new(lib).expect("context");
        let a = ctx.compile(&Rmlvo::default_selection()).expect("km a");
        let b = ctx.compile(&Rmlvo::default_selection()).expect("km b");
        assert_eq!(a.to_v1_string(), b.to_v1_string());
    }

    #[test]
    fn shift_depresses_and_releases() {
        let Some(mut st) = default_state() else {
            eprintln!("SKIP: libxkbcommon not present");
            return;
        };
        let clean = st.mods();
        assert_eq!(clean.depressed, 0);
        // Press Shift_L.
        let changed = st.update_key(key::LEFTSHIFT, true);
        assert_ne!(changed, 0);
        let held = st.mods();
        assert_ne!(held.depressed, 0);
        // Release.
        st.update_key(key::LEFTSHIFT, false);
        let after = st.mods();
        assert_eq!(after.depressed, 0);
        assert_eq!(after.locked, clean.locked);
    }

    #[test]
    fn caps_lock_locks() {
        let Some(mut st) = default_state() else {
            eprintln!("SKIP: libxkbcommon not present");
            return;
        };
        // Press + release Caps Lock: the modifier stays locked.
        st.update_key(key::CAPSLOCK, true);
        st.update_key(key::CAPSLOCK, false);
        let locked = st.mods();
        assert_ne!(locked.locked, 0, "Caps Lock must lock");
        // One more cycle unlocks it.
        st.update_key(key::CAPSLOCK, true);
        st.update_key(key::CAPSLOCK, false);
        assert_eq!(st.mods().locked, 0);
    }

    #[test]
    fn keysyms_follow_modifiers() {
        let Some(mut st) = default_state() else {
            eprintln!("SKIP: libxkbcommon not present");
            return;
        };
        // Plain 'a'.
        assert_eq!(st.syms(key::A), vec![0x61]);
        // Shift held: 'A'.
        st.update_key(key::LEFTSHIFT, true);
        assert_eq!(st.syms(key::A), vec![0x41]);
        st.update_key(key::LEFTSHIFT, false);
        assert_eq!(st.syms(key::A), vec![0x61]);
    }

    #[test]
    fn serialization_round_trip() {
        let Some(lib) = library() else {
            eprintln!("SKIP: libxkbcommon not present");
            return;
        };
        let ctx = Context::new(lib).expect("context");
        let km = ctx.compile(&Rmlvo::default_selection()).expect("km");
        let text = km.to_v1_string();
        let re = ctx.from_string(&text).expect("recompile");
        // v1 serialization is not byte-stable across a recompile
        // (the library normalizes some sections); the honest round
        // trip is behavioral: same keysyms, and the recompiled map's
        // own serialization is a fixpoint.
        let text2 = re.to_v1_string();
        assert!(!text2.is_empty());
        let re2 = ctx.from_string(&text2).expect("recompile 2");
        assert_eq!(re2.to_v1_string(), text2, "serialization must fixpoint");
        let st = km.state().expect("state");
        let st2 = re.state().expect("state 2");
        let st3 = re2.state().expect("state 3");
        assert_eq!(st.syms(key::A), st2.syms(key::A));
        assert_eq!(st.syms(key::A), st3.syms(key::A));
        assert_eq!(st.mods(), st2.mods());
    }

    #[test]
    fn client_fd_is_sealed_and_byte_exact() {
        let Some(km) = default_keymap() else {
            eprintln!("SKIP: libxkbcommon not present");
            return;
        };
        let fd = km.client_fd().expect("memfd");
        // Read back through a duplicate.
        let mut file = std::fs::File::from(fd.try_clone().expect("dup"));
        let mut read_back = String::new();
        file.read_to_string(&mut read_back).expect("read");
        assert_eq!(read_back, km.to_v1_string());
        // The seals are set.
        let seals = get_seals(&fd).expect("seals");
        assert_eq!(
            seals,
            libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_WRITE
        );
        // The descriptor offset was rewound (read started at zero and
        // returned the full map).
        assert_eq!(read_back.len(), km.to_v1_string().len());
        assert!(fd.as_raw_fd() >= 0);
    }

    #[test]
    fn bogus_soname_fails_typed() {
        // The error plumbing, exercised without needing the real
        // library to be absent: dlopen a name that cannot exist.
        let err =
            unsafe { libc::dlopen(c"libxkbcommon-does-not-exist.so.0".as_ptr(), libc::RTLD_NOW) };
        assert!(err.is_null());
        // And the typed path reports the honest shape when the real
        // library is missing (machines that carry it take the Ok arm).
        match LibXkb::open() {
            Ok(_) => {}
            Err(XkbError::LibraryLoad(name)) => assert!(name.contains("xkbcommon")),
            Err(other) => panic!("unexpected {other:?}"),
        }
    }
}
