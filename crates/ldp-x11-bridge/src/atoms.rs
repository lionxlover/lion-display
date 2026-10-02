//! Atom registry: name-to-XID interning and the predefined table.
//!
//! Atoms are 32-bit protocol-level handles for property names and
//! types. Atoms 1..=64 are predefined by the core protocol and never
//! interned; the bridge assigns dynamic atoms monotonically from 69
//! upward (real servers use 65..=68 for a few rarely used font
//! properties — the Phase 17 subset leaves that range reserved).
//!
//! Atom 0 is `None` and never a valid lookup.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

/// The first dynamically assigned atom.
pub const FIRST_DYNAMIC: u32 = 69;

/// The predefined atoms (index = atom - 1). Names are wire-stable.
pub const PREDEFINED: [&str; 64] = [
    "PRIMARY",
    "SECONDARY",
    "ARC",
    "ATOM",
    "BITMAP",
    "CARDINAL",
    "COLORMAP",
    "CURSOR",
    "CUT_BUFFER0",
    "CUT_BUFFER1",
    "CUT_BUFFER2",
    "CUT_BUFFER3",
    "CUT_BUFFER4",
    "CUT_BUFFER5",
    "CUT_BUFFER6",
    "CUT_BUFFER7",
    "DRAWABLE",
    "FONT",
    "INTEGER",
    "PIXMAP",
    "POINT",
    "RECTANGLE",
    "RGB_COLOR_MAP",
    "RGB_PIXEL",
    "STRING",
    "VISUALID",
    "WINDOW",
    "WM_COMMAND",
    "WM_HINTS",
    "WM_CLIENT_MACHINE",
    "WM_ICON_NAME",
    "WM_ICON_SIZE",
    "WM_NAME",
    "WM_NORMAL_HINTS",
    "WM_SIZE_HINTS",
    "WM_ZOOM_HINTS",
    "MIN_SPACE",
    "NORM_SPACE",
    "MAX_SPACE",
    "END_SPACE",
    "SUPERSCRIPT_X",
    "SUPERSCRIPT_Y",
    "SUBSCRIPT_X",
    "SUBSCRIPT_Y",
    "UNDERLINE_POSITION",
    "UNDERLINE_THICKNESS",
    "STRIKEOUT_ASCENT",
    "STRIKEOUT_DESCENT",
    "ITALIC_ANGLE",
    "X_HEIGHT",
    "QUAD_WIDTH",
    "CAP_HEIGHT",
    "POINT_SIZE",
    "RESOLUTION",
    "COPYRIGHT",
    "NOTICE",
    "FONT_NAME",
    "FAMILY_NAME",
    "WEIGHT_NAME",
    "SLANT",
    "SETWIDTH_NAME",
    "ADD_STYLE_NAME",
    "PIXEL_SIZE",
    "AVERAGE_WIDTH",
];

/// Interning table: dynamic names in both directions, predefined as a
/// static lookup. Lookup by id of a predefined atom answers from the
/// table without allocation.
#[derive(Clone, Debug, Default)]
pub struct AtomTable {
    next: u32,
    /// Dynamic atom -> name.
    forward: BTreeMap<u32, String>,
    /// Name -> dynamic atom (predefined names never enter here).
    reverse: BTreeMap<String, u32>,
}

impl AtomTable {
    /// A fresh table; dynamic interning starts at [`FIRST_DYNAMIC`].
    #[must_use]
    pub const fn new() -> AtomTable {
        AtomTable {
            next: FIRST_DYNAMIC,
            forward: BTreeMap::new(),
            reverse: BTreeMap::new(),
        }
    }

    /// The `InternAtom` request: returns the existing atom or mints one.
    ///
    /// `only_if_exists` on an unknown name is the classic `BadName`
    /// path; the caller maps `None` to the error event.
    #[must_use]
    pub fn intern(&mut self, name: &[u8], only_if_exists: bool) -> Option<u32> {
        if let Some(pre) = predefined_atom(name) {
            return Some(pre);
        }
        let key = String::from_utf8_lossy(name).into_owned();
        if let Some(existing) = self.reverse.get(&key) {
            return Some(*existing);
        }
        if only_if_exists {
            return None;
        }
        let atom = self.next;
        self.next += 1;
        self.forward.insert(atom, key.clone());
        self.reverse.insert(key, atom);
        Some(atom)
    }

    /// Atom -> name (`GetAtomName`); `None` for atom 0, predefined
    /// ids outside 1..=64, and never-interned dynamics.
    #[must_use]
    pub fn name(&self, atom: u32) -> Option<&str> {
        if (1..=64).contains(&atom) {
            return Some(PREDEFINED[(atom - 1) as usize]);
        }
        self.forward.get(&atom).map(String::as_str)
    }

    /// Whether `atom` names a known (predefined or interned) atom.
    #[must_use]
    pub fn exists(&self, atom: u32) -> bool {
        (1..=64).contains(&atom) || self.forward.contains_key(&atom)
    }

    /// Name → atom without interning (`None` when the name is unknown).
    /// The server side uses this to probe ICCCM properties it cares
    /// about without consuming atom ids.
    #[must_use]
    pub fn lookup(&self, name: &str) -> Option<u32> {
        if let Some(pre) = predefined_atom(name.as_bytes()) {
            return Some(pre);
        }
        self.reverse.get(name).copied()
    }
}

/// Look up a predefined atom by name.
#[must_use]
pub fn predefined_atom(name: &[u8]) -> Option<u32> {
    let s = std::str::from_utf8(name).ok()?;
    PREDEFINED
        .iter()
        .position(|p| *p == s)
        .map(|i| i as u32 + 1)
}

/// `WM_PROTOCOLS` — the ICCCM property listing protocol atoms the
/// client participates in (`WM_DELETE_WINDOW`, `WM_TAKE_FOCUS`...).
pub const WM_PROTOCOLS_NAME: &str = "WM_PROTOCOLS";
/// `WM_DELETE_WINDOW` — the close-request protocol atom.
pub const WM_DELETE_WINDOW_NAME: &str = "WM_DELETE_WINDOW";
/// `WM_NAME` — the window title property.
pub const WM_NAME: &str = "WM_NAME";
/// `WM_NORMAL_HINTS` — the size-hints property.
pub const WM_NORMAL_HINTS: &str = "WM_NORMAL_HINTS";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn predefined_table_has_no_gaps_or_duplicates() {
        let mut sorted: Vec<&str> = PREDEFINED.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), PREDEFINED.len());
        assert_eq!(PREDEFINED[0], "PRIMARY");
        assert_eq!(PREDEFINED[32], "WM_NAME");
        assert_eq!(PREDEFINED[63], "AVERAGE_WIDTH");
    }

    #[test]
    fn predefined_intern_is_idempotent() {
        let mut t = AtomTable::new();
        assert_eq!(t.intern(b"WM_NAME", false), Some(33));
        assert_eq!(t.intern(b"WM_NAME", true), Some(33));
        assert_eq!(t.intern(b"CARDINAL", true), Some(6));
        assert_eq!(t.name(33), Some("WM_NAME"));
        assert_eq!(t.name(6), Some("CARDINAL"));
    }

    #[test]
    fn dynamic_interning_starts_at_69() {
        let mut t = AtomTable::new();
        let a = t.intern(b"WM_PROTOCOLS", false).unwrap();
        assert_eq!(a, FIRST_DYNAMIC);
        let b = t.intern(b"WM_DELETE_WINDOW", false).unwrap();
        assert_eq!(b, FIRST_DYNAMIC + 1);
        // Both directions round-trip.
        assert_eq!(t.name(a), Some("WM_PROTOCOLS"));
        assert_eq!(t.name(b), Some("WM_DELETE_WINDOW"));
        // Re-intern returns the same atom.
        assert_eq!(t.intern(b"WM_PROTOCOLS", false), Some(a));
    }

    #[test]
    fn only_if_exists_on_unknown_is_none() {
        let mut t = AtomTable::new();
        assert_eq!(t.intern(b"_NET_WM_STATE_fresh", true), None);
        // The failed intern must not have consumed an atom.
        let a = t.intern(b"_NET_WM_STATE_fresh", false).unwrap();
        assert_eq!(a, FIRST_DYNAMIC);
    }

    #[test]
    fn bad_atoms_lookup_to_none() {
        let t = AtomTable::new();
        assert_eq!(t.name(0), None);
        assert_eq!(t.name(65), None);
        assert_eq!(t.name(500), None);
        assert!(!t.exists(0));
        assert!(t.exists(1));
    }
}
