//! Introspection blob generation (the spec, re-serialized).
//!
//! A compact byte-packed (not 8-aligned — it is an internal format, never
//! the wire) serialization of the whole spec set, embedded in
//! `ldp-protocol` via `include_bytes!` and parsed back by
//! `ldp_protocol::blob`. Runtime tooling (ldp-info, ldp-debug) and the
//! `registry.schema` service read it without touching TOML or the
//! compiler.
//!
//! Layout (all integers little-endian, `str` = `u32` length + UTF-8 bytes):
//!
//! ```text
//! magic  b"LDPS"   u16 version = 1   u32 module count
//! module: str name, u32 vmin, u32 vmax, str doc,
//!         u32 n_enums  * (str name, str doc, u32 n, (str, u32) * n)
//!         u32 n_bitset * (str name, str doc, u32 n, (str, u32) * n)
//!         u32 n_iface * (str fq name, str doc, u8 global, u8 has_builtin,
//!                        u32 builtin, u32 vmin, u32 vmax,
//!                        u32 n_req * op, u32 n_ev * op)
//! op:      str name, u32 opcode, u32 since, u8 has_reply, str reply,
//!          str doc, u32 n_args, arg * n
//! arg:     str name, u8 tag, u8 has_of, str of, u8 nullable, str doc
//! ```
//!
//! The format version bumps (never mutates in place) when the schema model
//! grows fields.

use crate::model::{Module, SpecSet};

/// Blob format magic.
pub const MAGIC: &[u8; 4] = b"LDPS";

/// Blob format version.
pub const FORMAT_VERSION: u16 = 1;

/// Serialize a validated spec set.
#[must_use]
pub fn generate_blob(set: &SpecSet) -> Vec<u8> {
    let mut w = Blob {
        out: Vec::with_capacity(4096),
    };
    w.out.extend_from_slice(MAGIC);
    w.u16(FORMAT_VERSION);
    w.u32(set.modules.len() as u32);
    for m in &set.modules {
        write_module(&mut w, m);
    }
    w.out
}

fn write_module(w: &mut Blob, m: &Module) {
    w.str(&m.name);
    w.u32(m.version_min);
    w.u32(m.version_max);
    w.str(&m.doc);
    w.u32(m.enums.len() as u32);
    for e in &m.enums {
        w.str(&e.name);
        w.str(&e.doc);
        w.u32(e.values.len() as u32);
        for (n, v) in &e.values {
            w.str(n);
            w.u32(*v);
        }
    }
    w.u32(m.bitsets.len() as u32);
    for b in &m.bitsets {
        w.str(&b.name);
        w.str(&b.doc);
        w.u32(b.bits.len() as u32);
        for (n, i) in &b.bits {
            w.str(n);
            w.u32(*i);
        }
    }
    w.u32(m.interfaces.len() as u32);
    for i in &m.interfaces {
        w.str(&format!("{}.{}", m.name, i.name));
        w.str(&i.doc);
        w.u8(u8::from(i.global));
        match i.builtin_id {
            Some(id) => {
                w.u8(1);
                w.u32(id);
            }
            None => w.u8(0),
        }
        w.u32(m.version_min);
        w.u32(m.version_max);
        for ops in [&i.requests, &i.events] {
            w.u32(ops.len() as u32);
            for (idx, op) in ops.iter().enumerate() {
                w.str(&op.name);
                w.u32(idx as u32 + 1);
                w.u32(op.since);
                match &op.reply {
                    Some(r) => {
                        w.u8(1);
                        w.str(r);
                    }
                    None => w.u8(0),
                }
                w.str(&op.doc);
                w.u32(op.args.len() as u32);
                for a in &op.args {
                    w.str(&a.name);
                    w.u8(a.ty.wire_tag());
                    match &a.of {
                        Some(x) => {
                            w.u8(1);
                            w.str(x);
                        }
                        None => w.u8(0),
                    }
                    w.u8(u8::from(a.nullable));
                    w.str(&a.doc);
                }
            }
        }
    }
}

// Imports are *not* serialized: they are compile-time linkage, fully
// resolved by validation, and carry no runtime information. The blob
// stores definitions; names inside `of` references resolve through the
// module tables the blob itself carries.

struct Blob {
    out: Vec<u8>,
}

impl Blob {
    fn u8(&mut self, v: u8) {
        self.out.push(v);
    }

    fn u16(&mut self, v: u16) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }

    fn u32(&mut self, v: u32) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }

    fn str(&mut self, s: &str) {
        self.u32(s.len() as u32);
        self.out.extend_from_slice(s.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ArgTypeSpec;

    #[test]
    fn header_is_magic_version_count() {
        let set = crate::testutil::fixture_set();
        let blob = generate_blob(&set);
        assert_eq!(&blob[0..4], MAGIC);
        assert_eq!(u16::from_le_bytes([blob[4], blob[5]]), FORMAT_VERSION);
        assert_eq!(u32::from_le_bytes([blob[6], blob[7], blob[8], blob[9]]), 1);
    }

    #[test]
    fn generation_is_deterministic() {
        let set = crate::testutil::fixture_set();
        assert_eq!(generate_blob(&set), generate_blob(&set));
    }

    #[test]
    fn wire_tags_cover_the_full_tag_range() {
        // Cross-check the tag set against every spec type exactly once.
        let tags: Vec<u8> = [
            "int32", "uint32", "int64", "uint64", "float32", "float64", "bool", "string", "ts",
            "rect", "object", "new_id", "fd", "array", "enum", "bitset",
        ]
        .iter()
        .map(|n| ArgTypeSpec::parse(n).unwrap().wire_tag())
        .collect();
        let mut sorted = tags.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted, (0x01..=0x10u8).collect::<Vec<_>>());
    }
}
