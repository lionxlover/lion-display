//! Introspection blob parsing.
//!
//! The blob is the spec re-serialized into a compact byte-packed format
//! (see `tools/ldpc` `blob_gen` for the layout) and embedded via
//! [`crate::BLOB`]. Runtime tooling (`ldp-info`, `ldp-debug`) and the
//! `registry.schema` service parse it back into owned schema structures
//! without linking the compiler. The parser is total: any truncation,
//! bad magic, wrong version, or overrun is an error — never a panic —
//! because blobs may come from peers or files.

use ldp_core::error::{ErrorCode, LdpError, Result};
use ldp_core::wire::ArgType;

use crate::schema::{BitsetDef, EnumDef, InterfaceSchema, ModuleSchema, OpSchema};

/// Blob magic (`LDPS`).
pub const MAGIC: &[u8; 4] = b"LDPS";

/// Blob format version this parser understands.
pub const FORMAT_VERSION: u16 = 1;

/// Owned mirror of [`EnumDef`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct OwnedEnumDef {
    /// Short name.
    pub name: String,
    /// Documentation.
    pub doc: String,
    /// `(name, wire value)` pairs sorted by value.
    pub values: Vec<(String, u32)>,
}

/// Owned mirror of [`BitsetDef`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct OwnedBitsetDef {
    /// Short name.
    pub name: String,
    /// Documentation.
    pub doc: String,
    /// `(name, bit index)` pairs sorted by index.
    pub bits: Vec<(String, u32)>,
}

/// Owned mirror of one operation's signature (raw tag numbers, no
/// `&'static` backing).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct OwnedOp {
    /// Operation name.
    pub name: String,
    /// Opcode, declaration order.
    pub opcode: u32,
    /// First version carrying the operation.
    pub since: u32,
    /// Documentation.
    pub doc: String,
    /// Reply event name (requests).
    pub reply: Option<String>,
    /// Argument signatures.
    pub args: Vec<OwnedArg>,
}

/// Owned mirror of one argument's signature.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct OwnedArg {
    /// Argument name.
    pub name: String,
    /// Wire type tag.
    pub ty: ArgType,
    /// Type qualifier, when present.
    pub of: Option<String>,
    /// Nullability (object arguments).
    pub nullable: bool,
    /// Documentation.
    pub doc: String,
}

/// Owned mirror of [`InterfaceSchema`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct OwnedInterface {
    /// Fully qualified name.
    pub name: String,
    /// Version range minimum.
    pub version_min: u32,
    /// Version range maximum.
    pub version_max: u32,
    /// Documentation.
    pub doc: String,
    /// Registry-advertised global.
    pub global: bool,
    /// Pre-bound object ID.
    pub builtin_id: Option<u32>,
    /// Request signatures.
    pub requests: Vec<OwnedOp>,
    /// Event signatures.
    pub events: Vec<OwnedOp>,
}

/// Owned mirror of [`ModuleSchema`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct OwnedModule {
    /// Fully qualified name.
    pub name: String,
    /// Version range minimum.
    pub version_min: u32,
    /// Version range maximum.
    pub version_max: u32,
    /// Documentation.
    pub doc: String,
    /// Interfaces.
    pub interfaces: Vec<OwnedInterface>,
    /// Enums.
    pub enums: Vec<OwnedEnumDef>,
    /// Bitsets.
    pub bitsets: Vec<OwnedBitsetDef>,
}

/// Parse a whole blob into owned modules.
///
/// # Errors
///
/// [`ErrorCode::MalformedMessage`] for bad magic, unsupported version,
/// truncation, non-UTF-8 strings, or unknown argument tags.
pub fn parse(blob: &[u8]) -> Result<Vec<OwnedModule>> {
    let mut c = Cursor { buf: blob, pos: 0 };
    let magic = c.take(4)?;
    if magic != MAGIC {
        return Err(LdpError::malformed(
            ErrorCode::MalformedMessage,
            "introspection blob magic mismatch",
        ));
    }
    let version = c.u16()?;
    if version != FORMAT_VERSION {
        return Err(LdpError::malformed(
            ErrorCode::MalformedMessage,
            format!("introspection blob version {version} unsupported"),
        ));
    }
    let module_count = c.u32()? as usize;
    let mut modules = Vec::with_capacity(module_count.min(c.remaining()));
    for _ in 0..module_count {
        modules.push(parse_module(&mut c)?);
    }
    if c.remaining() != 0 {
        return Err(LdpError::malformed(
            ErrorCode::MalformedMessage,
            "introspection blob has trailing bytes",
        ));
    }
    Ok(modules)
}

fn parse_module(c: &mut Cursor<'_>) -> Result<OwnedModule> {
    let name = c.str()?;
    let version_min = c.u32()?;
    let version_max = c.u32()?;
    let doc = c.str()?;
    let mut enums = Vec::new();
    for _ in 0..c.u32()? as usize {
        let ename = c.str()?;
        let edoc = c.str()?;
        let mut values = Vec::new();
        for _ in 0..c.u32()? as usize {
            values.push((c.str()?, c.u32()?));
        }
        enums.push(OwnedEnumDef {
            name: ename,
            doc: edoc,
            values,
        });
    }
    let mut bitsets = Vec::new();
    for _ in 0..c.u32()? as usize {
        let bname = c.str()?;
        let bdoc = c.str()?;
        let mut bits = Vec::new();
        for _ in 0..c.u32()? as usize {
            bits.push((c.str()?, c.u32()?));
        }
        bitsets.push(OwnedBitsetDef {
            name: bname,
            doc: bdoc,
            bits,
        });
    }
    let mut interfaces = Vec::new();
    for _ in 0..c.u32()? as usize {
        interfaces.push(parse_interface(c, version_min, version_max)?);
    }
    Ok(OwnedModule {
        name,
        version_min,
        version_max,
        doc,
        interfaces,
        enums,
        bitsets,
    })
}

fn parse_interface(c: &mut Cursor<'_>, vmin: u32, vmax: u32) -> Result<OwnedInterface> {
    let name = c.str()?;
    let doc = c.str()?;
    let global = c.u8()? != 0;
    let builtin_id = if c.u8()? != 0 { Some(c.u32()?) } else { None };
    // Interface version range follows the module's (v1 layout).
    let version_min = c.u32()?;
    let version_max = c.u32()?;
    let _ = (vmin, vmax);
    let mut requests = Vec::new();
    for _ in 0..c.u32()? as usize {
        requests.push(parse_op(c, true)?);
    }
    let mut events = Vec::new();
    for _ in 0..c.u32()? as usize {
        events.push(parse_op(c, false)?);
    }
    Ok(OwnedInterface {
        name,
        version_min,
        version_max,
        doc,
        global,
        builtin_id,
        requests,
        events,
    })
}

fn parse_op(c: &mut Cursor<'_>, is_request: bool) -> Result<OwnedOp> {
    let name = c.str()?;
    let opcode = c.u32()?;
    let since = c.u32()?;
    let reply = if c.u8()? != 0 { Some(c.str()?) } else { None };
    let doc = c.str()?;
    let mut args = Vec::new();
    for _ in 0..c.u32()? as usize {
        let aname = c.str()?;
        let tag = c.u8()?;
        let ty = ArgType::from_wire(tag).ok_or_else(|| {
            LdpError::malformed(
                ErrorCode::MalformedMessage,
                format!("blob argument carries unknown tag {tag:#04x}"),
            )
        })?;
        let of = if c.u8()? != 0 { Some(c.str()?) } else { None };
        let nullable = c.u8()? != 0;
        let adoc = c.str()?;
        args.push(OwnedArg {
            name: aname,
            ty,
            of,
            nullable,
            doc: adoc,
        });
    }
    if !is_request && reply.is_some() {
        return Err(LdpError::malformed(
            ErrorCode::MalformedMessage,
            "blob event carries a reply field",
        ));
    }
    Ok(OwnedOp {
        name,
        opcode,
        since,
        doc,
        reply,
        args,
    })
}

struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.remaining() < n {
            return Err(LdpError::malformed(
                ErrorCode::MalformedMessage,
                "introspection blob truncated",
            ));
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn str(&mut self) -> Result<String> {
        let len = self.u32()? as usize;
        if self.remaining() < len {
            return Err(LdpError::malformed(
                ErrorCode::MalformedMessage,
                "introspection blob string truncated",
            ));
        }
        let bytes = self.take(len)?;
        std::str::from_utf8(bytes).map(str::to_string).map_err(|_| {
            LdpError::malformed(
                ErrorCode::MalformedMessage,
                "introspection blob string is not UTF-8",
            )
        })
    }
}

impl From<&EnumDef> for OwnedEnumDef {
    fn from(e: &EnumDef) -> Self {
        OwnedEnumDef {
            name: e.name.into(),
            doc: e.doc.into(),
            values: e.values.iter().map(|&(n, v)| (n.into(), v)).collect(),
        }
    }
}

impl From<&BitsetDef> for OwnedBitsetDef {
    fn from(b: &BitsetDef) -> Self {
        OwnedBitsetDef {
            name: b.name.into(),
            doc: b.doc.into(),
            bits: b.bits.iter().map(|&(n, i)| (n.into(), i)).collect(),
        }
    }
}

impl From<&OpSchema> for OwnedOp {
    fn from(op: &OpSchema) -> Self {
        OwnedOp {
            name: op.name.into(),
            opcode: op.opcode,
            since: op.since,
            doc: op.doc.into(),
            reply: op.reply.map(str::to_string),
            args: op.args.iter().map(OwnedArg::from).collect(),
        }
    }
}

impl From<&crate::schema::ArgSchema> for OwnedArg {
    fn from(a: &crate::schema::ArgSchema) -> Self {
        OwnedArg {
            name: a.name.into(),
            ty: a.ty,
            of: a.of.map(str::to_string),
            nullable: a.nullable,
            doc: a.doc.into(),
        }
    }
}

impl From<&InterfaceSchema> for OwnedInterface {
    fn from(i: &InterfaceSchema) -> Self {
        OwnedInterface {
            name: i.name.into(),
            version_min: i.version_min,
            version_max: i.version_max,
            doc: i.doc.into(),
            global: i.global,
            builtin_id: i.builtin_id,
            requests: i.requests.iter().map(OwnedOp::from).collect(),
            events: i.events.iter().map(OwnedOp::from).collect(),
        }
    }
}

impl From<&ModuleSchema> for OwnedModule {
    fn from(m: &ModuleSchema) -> Self {
        OwnedModule {
            name: m.name.into(),
            version_min: m.version_min,
            version_max: m.version_max,
            doc: m.doc.into(),
            interfaces: m
                .interfaces
                .iter()
                .copied()
                .map(OwnedInterface::from)
                .collect(),
            enums: m.enums.iter().copied().map(OwnedEnumDef::from).collect(),
            bitsets: m
                .bitsets
                .iter()
                .copied()
                .map(OwnedBitsetDef::from)
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MODULES;

    #[test]
    fn committed_blob_parses_back_exactly() {
        let parsed = parse(crate::BLOB).unwrap();
        let from_statics: Vec<OwnedModule> = MODULES.iter().map(|&m| m.into()).collect();
        assert_eq!(parsed, from_statics);
        // Nine modules since Phase 22 (capture joined the spec).
        assert_eq!(parsed.len(), 9);
        let core = parsed.iter().find(|m| m.name == "ldp.core").unwrap();
        assert!(core.interfaces.iter().any(|i| i.builtin_id == Some(1)));
    }

    #[test]
    fn bad_magic_and_version_are_errors() {
        assert!(parse(&[]).is_err());
        assert!(parse(b"NOPE").is_err());
        let mut wrong_version = crate::BLOB.to_vec();
        wrong_version[4] = 9;
        assert!(parse(&wrong_version).is_err());
    }

    #[test]
    fn truncation_never_panics() {
        // Sampled truncation points (every 97th byte + boundaries): every
        // prefix must be an error, never a panic.
        let blob = crate::BLOB;
        let mut i = 0;
        while i < blob.len() {
            assert!(parse(&blob[..i]).is_err(), "prefix of {i} bytes must fail");
            i += 97;
        }
        assert!(parse(blob).is_ok());
    }

    #[test]
    fn trailing_bytes_are_rejected() {
        let mut padded = crate::BLOB.to_vec();
        padded.push(0);
        assert!(parse(&padded).is_err());
    }
}
