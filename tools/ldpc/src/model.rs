//! The validated in-memory model of one protocol specification set.
//!
//! `parser` produces these structures from TOML text and
//! `validate` proves the invariants of
//! `docs/spec-format.md` §2 before any generator runs: generators may then
//! index and emit without defensive re-checking.

/// A whole compiled spec set (one entry per module file), name-sorted.
#[derive(Clone, Debug, PartialEq)]
pub struct SpecSet {
    /// Modules sorted by fully qualified name.
    pub modules: Vec<Module>,
}
/// Inventory of a compiled spec set (CI summaries, drift reports).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// Number of modules.
    pub modules: usize,
    /// Number of interfaces.
    pub interfaces: usize,
    /// Number of requests.
    pub requests: usize,
    /// Number of events.
    pub events: usize,
    /// Number of enums.
    pub enums: usize,
    /// Number of bitsets.
    pub bitsets: usize,
}

/// One protocol module (`spec/<stem>.toml`).
#[derive(Clone, Debug, PartialEq)]
pub struct Module {
    /// Fully qualified module name (`ldp.core`).
    pub name: String,
    /// Minimum wire version.
    pub version_min: u32,
    /// Maximum wire version.
    pub version_max: u32,
    /// Module documentation (spec `doc`, trimmed).
    pub doc: String,
    /// Cross-module references declared via `[[import]]`.
    pub imports: Vec<Import>,
    /// Named integer constants.
    pub enums: Vec<EnumDef>,
    /// Named 128-bit flag sets.
    pub bitsets: Vec<BitsetDef>,
    /// Object types.
    pub interfaces: Vec<Interface>,
}

/// A cross-module reference (one `[[import]]` entry).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Import {
    /// What is being imported.
    pub kind: ImportKind,
    /// Fully qualified target name (`ldp.core.transform`).
    pub name: String,
}

/// The importable spec item kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ImportKind {
    /// An interface type.
    Interface,
    /// A named enum.
    Enum,
    /// A named bitset.
    Bitset,
}

impl ImportKind {
    /// The TOML key for this kind (`interface` / `enum` / `bitset`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Interface => "interface",
            Self::Enum => "enum",
            Self::Bitset => "bitset",
        }
    }
}

/// A named enum with dense wire values `1..=N`.
#[derive(Clone, Debug, PartialEq)]
pub struct EnumDef {
    /// Short name, unique within (and disjoint across kinds in) the module.
    pub name: String,
    /// Documentation.
    pub doc: String,
    /// `(name, wire value)` pairs sorted by value.
    pub values: Vec<(String, u32)>,
}

impl EnumDef {
    /// Whether `value` is a declared wire value.
    #[must_use]
    pub fn contains(&self, value: u32) -> bool {
        self.values.iter().any(|&(_, v)| v == value)
    }
}

/// A named 128-bit flag set.
#[derive(Clone, Debug, PartialEq)]
pub struct BitsetDef {
    /// Short name, unique within (and disjoint across kinds in) the module.
    pub name: String,
    /// Documentation.
    pub doc: String,
    /// `(name, bit index)` pairs sorted by index.
    pub bits: Vec<(String, u32)>,
}

impl BitsetDef {
    /// One past the highest declared bit index (0 when no bits).
    #[must_use]
    pub fn width(&self) -> u32 {
        self.bits.last().map_or(0, |&(_, idx)| idx + 1)
    }
}

/// One interface type.
#[derive(Clone, Debug, PartialEq)]
pub struct Interface {
    /// Short name; the global key is `module.name + "." + name`.
    pub name: String,
    /// Documentation.
    pub doc: String,
    /// Whether the registry advertises this interface as a global factory.
    pub global: bool,
    /// Pre-bound object ID (only `ldp.core.connection`, ID 1).
    pub builtin_id: Option<u32>,
    /// Client-to-server operations, opcodes 1.. in declaration order.
    pub requests: Vec<Op>,
    /// Server-to-client operations, opcodes 1.. in declaration order.
    pub events: Vec<Op>,
}

/// One request or event.
#[derive(Clone, Debug, PartialEq)]
pub struct Op {
    /// Short name, unique within its kind in the interface.
    pub name: String,
    /// First module version carrying this operation.
    pub since: u32,
    /// Documentation.
    pub doc: String,
    /// For requests: the correlated reply event's name.
    pub reply: Option<String>,
    /// Ordered argument list.
    pub args: Vec<Arg>,
}

/// One operation argument.
#[derive(Clone, Debug, PartialEq)]
pub struct Arg {
    /// Argument name, unique within the operation.
    pub name: String,
    /// Argument type.
    pub ty: ArgTypeSpec,
    /// Type qualifier: enum/bitset name, interface for object/new_id,
    /// element type for arrays. `None` for scalars and generic objects.
    pub of: Option<String>,
    /// `object` arguments only: whether wire ID 0 means "null".
    pub nullable: bool,
    /// Per-argument documentation (may be empty).
    pub doc: String,
}

/// Spec argument types (`docs/spec-format.md` §3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArgTypeSpec {
    /// `int32`
    Int32,
    /// `uint32`
    Uint32,
    /// `int64`
    Int64,
    /// `uint64`
    Uint64,
    /// `float32`
    Float32,
    /// `float64`
    Float64,
    /// `bool`
    Bool,
    /// `string`
    String,
    /// `ts`
    Ts,
    /// `rect`
    Rect,
    /// `object`
    Object,
    /// `new_id`
    NewId,
    /// `fd`
    Fd,
    /// `array`
    Array,
    /// `enum`
    Enum,
    /// `bitset`
    Bitset,
}

impl ArgTypeSpec {
    /// Parse the spec spelling; `None` for unknown type names.
    #[must_use]
    pub fn parse(s: &str) -> Option<ArgTypeSpec> {
        match s {
            "int32" => Some(Self::Int32),
            "uint32" => Some(Self::Uint32),
            "int64" => Some(Self::Int64),
            "uint64" => Some(Self::Uint64),
            "float32" => Some(Self::Float32),
            "float64" => Some(Self::Float64),
            "bool" => Some(Self::Bool),
            "string" => Some(Self::String),
            "ts" => Some(Self::Ts),
            "rect" => Some(Self::Rect),
            "object" => Some(Self::Object),
            "new_id" => Some(Self::NewId),
            "fd" => Some(Self::Fd),
            "array" => Some(Self::Array),
            "enum" => Some(Self::Enum),
            "bitset" => Some(Self::Bitset),
            _ => None,
        }
    }

    /// Spec spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Int32 => "int32",
            Self::Uint32 => "uint32",
            Self::Int64 => "int64",
            Self::Uint64 => "uint64",
            Self::Float32 => "float32",
            Self::Float64 => "float64",
            Self::Bool => "bool",
            Self::String => "string",
            Self::Ts => "ts",
            Self::Rect => "rect",
            Self::Object => "object",
            Self::NewId => "new_id",
            Self::Fd => "fd",
            Self::Array => "array",
            Self::Enum => "enum",
            Self::Bitset => "bitset",
        }
    }

    /// Wire tag byte (matches `ldp_core::wire::ArgType::to_wire`).
    #[must_use]
    pub const fn wire_tag(self) -> u8 {
        match self {
            Self::Int32 => 0x01,
            Self::Uint32 => 0x02,
            Self::Int64 => 0x03,
            Self::Uint64 => 0x04,
            Self::Float32 => 0x05,
            Self::Float64 => 0x06,
            Self::Bool => 0x07,
            Self::String => 0x08,
            Self::Object => 0x09,
            Self::NewId => 0x0A,
            Self::Fd => 0x0B,
            Self::Array => 0x0C,
            Self::Rect => 0x0D,
            Self::Enum => 0x0E,
            Self::Bitset => 0x0F,
            Self::Ts => 0x10,
        }
    }

    /// Whether this type may appear as an array element (the set the wire
    /// `Primitive` model can represent: scalars and `rect`).
    #[must_use]
    pub const fn is_array_element(self) -> bool {
        matches!(
            self,
            Self::Int32
                | Self::Uint32
                | Self::Int64
                | Self::Uint64
                | Self::Float32
                | Self::Float64
                | Self::Bool
                | Self::Fd
                | Self::Rect
        )
    }
}

impl SpecSet {
    /// Aggregate counts for summaries and drift reports.
    #[must_use]
    pub fn counts(&self) -> Counts {
        let mut c = Counts {
            modules: self.modules.len(),
            ..Counts::default()
        };
        for m in &self.modules {
            c.enums += m.enums.len();
            c.bitsets += m.bitsets.len();
            c.interfaces += m.interfaces.len();
            c.requests += m.interfaces.iter().map(|i| i.requests.len()).sum::<usize>();
            c.events += m.interfaces.iter().map(|i| i.events.len()).sum::<usize>();
        }
        c
    }

    /// Look up a module by fully qualified name.
    #[must_use]
    pub fn module(&self, fq: &str) -> Option<&Module> {
        self.modules.iter().find(|m| m.name == fq)
    }

    /// Look up an interface by fully qualified name (`ldp.core.surface`).
    #[must_use]
    pub fn interface(&self, fq: &str) -> Option<(&Module, &Interface)> {
        let (mod_name, iface) = split_fq(fq)?;
        self.module(mod_name).and_then(|m| {
            m.interfaces
                .iter()
                .find(|i| i.name == iface)
                .map(|i| (m, i))
        })
    }

    /// Look up an enum: fully qualified, or short within `in_module`.
    #[must_use]
    pub fn resolve_enum(&self, in_module: &str, of: &str) -> Option<&EnumDef> {
        let (m, item) = if let Some((m, item)) = split_fq(of) {
            (self.module(m)?, item)
        } else {
            (self.module(in_module)?, of)
        };
        m.enums.iter().find(|e| e.name == item)
    }

    /// Look up a bitset: fully qualified, or short within `in_module`.
    #[must_use]
    pub fn resolve_bitset(&self, in_module: &str, of: &str) -> Option<&BitsetDef> {
        let (m, item) = if let Some((m, item)) = split_fq(of) {
            (self.module(m)?, item)
        } else {
            (self.module(in_module)?, of)
        };
        m.bitsets.iter().find(|b| b.name == item)
    }
}

/// Split `ldp.core.surface` into (`ldp.core`, `surface`); `None` when the
/// string is not dotted.
fn split_fq(fq: &str) -> Option<(&str, &str)> {
    let idx = fq.rfind('.')?;
    Some((&fq[..idx], &fq[idx + 1..]))
}
