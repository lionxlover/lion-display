//! Window properties: the ICCCM string/atom store.
//!
//! Each window carries a set of `(property atom, type atom, format,
//! data)` entries. `ChangeProperty` supports Replace/Append/Prepend
//! with the format (8/16/32) fixed per property — a change with a
//! different format or type than the existing property is a
//! `BadMatch`. `GetProperty` windows through `long-offset` /
//! `long-length` in 4-byte units and reports `bytes-after`; the
//! `delete` flag clears the property after reading it. The store caps
//! per-property and per-window sizes so a client cannot balloon the
//! bridge's memory (the LDP `Limits` doctrine applied at the bridge
//! boundary).

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

/// Maximum bytes of property data per window (1 MiB).
pub const MAX_WINDOW_PROPERTY_BYTES: usize = 1024 * 1024;

/// One stored property: type atom + format + raw units.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PropValue {
    /// Type atom (STRING, CARDINAL, ATOM, WM_STATE...).
    pub type_atom: u32,
    /// 8, 16 or 32 — the unit width the client wrote.
    pub format: u8,
    /// Raw bytes in wire order (network byte order for 16/32 units is
    /// *not* applied: the bridge stores the bytes as the client sent
    /// them in the connection's order and returns them in that order).
    pub data: Vec<u8>,
}

/// A property change request, decoded and validated.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum PropMode {
    /// Discard the old value.
    Replace,
    /// Append units to the old value.
    Append,
    /// Prepend units to the old value.
    Prepend,
}

/// Errors the property store produces; the dispatcher maps them onto
/// X error events.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PropError {
    /// The property exists with a different format or type.
    Mismatch,
    /// The window's property budget is exhausted.
    Limit,
    /// Format not in {8, 16, 32}.
    BadFormat(u8),
}

/// The per-window property map.
#[derive(Clone, Debug, Default)]
pub struct PropertyStore {
    props: BTreeMap<u32, PropValue>,
    total: usize,
}

impl PropertyStore {
    /// Empty store.
    #[must_use]
    pub const fn new() -> PropertyStore {
        PropertyStore {
            props: BTreeMap::new(),
            total: 0,
        }
    }

    /// Read one property (clone).
    #[must_use]
    pub fn get(&self, atom: u32) -> Option<&PropValue> {
        self.props.get(&atom)
    }

    /// Whether the window has the property at all.
    #[must_use]
    pub fn contains(&self, atom: u32) -> bool {
        self.props.contains_key(&atom)
    }

    /// Delete one property; true when it existed.
    pub fn delete(&mut self, atom: u32) -> bool {
        match self.props.remove(&atom) {
            Some(v) => {
                self.total = self.total.saturating_sub(v.data.len());
                true
            }
            None => false,
        }
    }

    /// List property atoms in id order.
    #[must_use]
    pub fn list(&self) -> Vec<u32> {
        self.props.keys().copied().collect()
    }

    /// Apply `ChangeProperty`.
    ///
    /// # Errors
    /// [`PropError`] per the store's contract above.
    pub fn change(
        &mut self,
        atom: u32,
        type_atom: u32,
        format: u8,
        mode: PropMode,
        data: &[u8],
    ) -> Result<(), PropError> {
        if !matches!(format, 8 | 16 | 32) {
            return Err(PropError::BadFormat(format));
        }
        let entry = self.props.get(&atom);
        if let Some(existing) = entry {
            if existing.format != format || existing.type_atom != type_atom {
                return Err(PropError::Mismatch);
            }
        }
        let existing_len = entry.map_or(0, |e| e.data.len());
        let merged: Vec<u8> = match (entry, mode) {
            (Some(existing), PropMode::Append) => {
                let mut v = existing.data.clone();
                v.extend_from_slice(data);
                v
            }
            (Some(existing), PropMode::Prepend) => {
                let mut v = data.to_vec();
                v.extend_from_slice(&existing.data);
                v
            }
            _ => data.to_vec(),
        };
        let budget = MAX_WINDOW_PROPERTY_BYTES - (self.total - existing_len);
        if merged.len() > budget {
            return Err(PropError::Limit);
        }
        if let Some(old) = self.props.get(&atom) {
            self.total -= old.data.len();
        }
        self.total += merged.len();
        self.props.insert(
            atom,
            PropValue {
                type_atom,
                format,
                data: merged,
            },
        );
        Ok(())
    }

    /// The `GetProperty` read: 4-byte-unit window into the data.
    ///
    /// Returns `(type_atom, format, bytes_after, chunk)`. An absent
    /// property answers `(None, 0, 0, empty)`.
    #[must_use]
    pub fn read(
        &self,
        atom: u32,
        long_offset: u32,
        long_length: u32,
    ) -> (Option<u32>, u8, u32, Vec<u8>) {
        let Some(v) = self.props.get(&atom) else {
            return (None, 0, 0, Vec::new());
        };
        let off = long_offset as usize * 4;
        if off > v.data.len() {
            // Out-of-range offset: empty value, everything after.
            return (Some(v.type_atom), v.format, v.data.len() as u32, Vec::new());
        }
        let len = if long_length == 0 {
            v.data.len() - off
        } else {
            (long_length as usize * 4).min(v.data.len() - off)
        };
        let chunk = v.data[off..off + len].to_vec();
        let after = (v.data.len() - off - len) as u32;
        (Some(v.type_atom), v.format, after, chunk)
    }

    /// Total stored bytes (budget observability).
    #[must_use]
    pub fn total_bytes(&self) -> usize {
        self.total
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_with_name() -> PropertyStore {
        let mut s = PropertyStore::new();
        s.change(33, 25, 8, PropMode::Replace, b"xclock").unwrap();
        s
    }

    #[test]
    fn replace_read_delete_round_trip() {
        let mut s = store_with_name();
        let (ty, fmt, after, chunk) = s.read(33, 0, 0);
        assert_eq!(ty, Some(25));
        assert_eq!(fmt, 8);
        assert_eq!(after, 0);
        assert_eq!(chunk, b"xclock");
        assert!(s.contains(33));
        assert!(s.delete(33));
        assert!(!s.delete(33));
        let (ty, fmt, after, chunk) = s.read(33, 0, 0);
        assert_eq!((ty.is_none(), fmt, after, chunk.len()), (true, 0, 0, 0));
    }

    #[test]
    fn append_and_prepend_keep_format() {
        let mut s = PropertyStore::new();
        s.change(1, 6, 32, PropMode::Replace, &[1, 0, 0, 0])
            .unwrap();
        s.change(1, 6, 32, PropMode::Append, &[2, 0, 0, 0]).unwrap();
        let (_, _, _, chunk) = s.read(1, 0, 0);
        assert_eq!(chunk, [1, 0, 0, 0, 2, 0, 0, 0]);
        s.change(1, 6, 32, PropMode::Prepend, &[3, 0, 0, 0])
            .unwrap();
        let (_, _, after, chunk) = s.read(1, 0, 0);
        assert_eq!(chunk, [3, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0]);
        assert_eq!(after, 0);
    }

    #[test]
    fn type_or_format_change_is_mismatch() {
        let mut s = store_with_name();
        assert_eq!(
            s.change(33, 6, 8, PropMode::Replace, b"x"),
            Err(PropError::Mismatch)
        );
        assert_eq!(
            s.change(33, 25, 16, PropMode::Replace, &[0; 2]),
            Err(PropError::Mismatch)
        );
        // A fresh property with format 16 is fine.
        assert!(s.change(34, 25, 16, PropMode::Replace, &[0; 2]).is_ok());
    }

    #[test]
    fn bad_format_rejected() {
        let mut s = PropertyStore::new();
        assert_eq!(
            s.change(1, 6, 24, PropMode::Replace, &[0]),
            Err(PropError::BadFormat(24))
        );
    }

    #[test]
    fn read_windows_and_reports_bytes_after() {
        let mut s = PropertyStore::new();
        s.change(1, 6, 32, PropMode::Replace, &[0xab; 16]).unwrap();
        // Read 2 longs starting at long 1.
        let (_, _, after, chunk) = s.read(1, 1, 2);
        assert_eq!(chunk.len(), 8);
        assert_eq!(chunk, &[0xab; 8]);
        assert_eq!(after, 4); // 16 - 4 - 8
                              // Out-of-range offset reads empty.
        let (_, _, after, chunk) = s.read(1, 99, 4);
        assert!(chunk.is_empty());
        assert_eq!(after, 16);
    }

    #[test]
    fn budget_is_enforced() {
        let mut s = PropertyStore::new();
        let big = vec![0u8; MAX_WINDOW_PROPERTY_BYTES + 1];
        assert_eq!(
            s.change(1, 25, 8, PropMode::Replace, &big),
            Err(PropError::Limit)
        );
        // Two halves each fit; together they exceed the budget by one.
        let half = vec![0u8; MAX_WINDOW_PROPERTY_BYTES / 2];
        assert!(s.change(1, 25, 8, PropMode::Replace, &half).is_ok());
        let too_much = vec![0u8; MAX_WINDOW_PROPERTY_BYTES / 2 + 1];
        assert_eq!(
            s.change(1, 25, 8, PropMode::Append, &too_much),
            Err(PropError::Limit)
        );
        assert_eq!(s.total_bytes(), MAX_WINDOW_PROPERTY_BYTES / 2);
    }

    #[test]
    fn list_is_sorted_by_atom() {
        let mut s = store_with_name();
        s.change(5, 25, 8, PropMode::Replace, b"z").unwrap();
        s.change(2, 25, 8, PropMode::Replace, b"a").unwrap();
        assert_eq!(s.list(), vec![2, 5, 33]);
    }
}
