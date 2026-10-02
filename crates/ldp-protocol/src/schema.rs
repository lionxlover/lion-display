//! Schema table types.
//!
//! The shape of a compiled protocol as static data: every interface,
//! operation, and argument of `spec/*.toml` materializes here through the
//! `generated` module (ldpc output). The types are plain data — validation
//! logic lives in [`crate::validate`], lookup in [`crate::registry`].

use ldp_core::wire::ArgType;

/// Message direction: requests flow client→server, events server→client.
///
/// The wire envelope is direction-agnostic; direction qualifies *opcode
/// namespaces* and new_id ownership rules during signature validation.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Direction {
    /// Client-to-server operation.
    Request,
    /// Server-to-client operation.
    Event,
}

impl Direction {
    /// Stable name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Request => "request",
            Self::Event => "event",
        }
    }
}

/// One operation argument signature.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ArgSchema {
    /// Argument name (unique within the operation).
    pub name: &'static str,
    /// Wire type tag.
    pub ty: ArgType,
    /// Type qualifier: enum/bitset name, interface of an object/new_id, or
    /// the element type of an array. `None` for scalars and generic
    /// object references.
    pub of: Option<&'static str>,
    /// Whether wire object ID 0 means "null" (object arguments only).
    pub nullable: bool,
    /// Argument documentation (may be empty).
    pub doc: &'static str,
}

/// One request or event signature.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct OpSchema {
    /// Operation name.
    pub name: &'static str,
    /// Opcode: declaration order within the interface and kind, from 1.
    /// Frozen once released (`docs/spec-format.md` §4).
    pub opcode: u32,
    /// First module version carrying this operation.
    pub since: u32,
    /// Documentation.
    pub doc: &'static str,
    /// For requests: the correlated reply event's name.
    pub reply: Option<&'static str>,
    /// Ordered argument signatures.
    pub args: &'static [ArgSchema],
}

/// One interface type.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct InterfaceSchema {
    /// Fully qualified name (`ldp.core.surface`).
    pub name: &'static str,
    /// Minimum wire version (the module's advertised range).
    pub version_min: u32,
    /// Maximum wire version (the module's advertised range).
    pub version_max: u32,
    /// Documentation.
    pub doc: &'static str,
    /// Whether the registry advertises this interface as a global factory.
    pub global: bool,
    /// Pre-bound object ID (only `ldp.core.connection`, ID 1).
    pub builtin_id: Option<u32>,
    /// Request signatures, opcode order.
    pub requests: &'static [OpSchema],
    /// Event signatures, opcode order.
    pub events: &'static [OpSchema],
}

impl InterfaceSchema {
    /// Find an operation by opcode in the given direction's table.
    #[must_use]
    pub fn find_op(&self, dir: Direction, opcode: u32) -> Option<&'static OpSchema> {
        let ops = match dir {
            Direction::Request => self.requests,
            Direction::Event => self.events,
        };
        let mut i = 0;
        while i < ops.len() {
            if ops[i].opcode == opcode {
                return Some(&ops[i]);
            }
            i += 1;
        }
        None
    }

    /// Number of operations in the given direction.
    #[must_use]
    pub fn op_count(&self, dir: Direction) -> usize {
        match dir {
            Direction::Request => self.requests.len(),
            Direction::Event => self.events.len(),
        }
    }
}

/// A named enum with dense wire values `1..=N`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct EnumDef {
    /// Short name, unique within its module.
    pub name: &'static str,
    /// Documentation.
    pub doc: &'static str,
    /// `(name, wire value)` pairs sorted by value.
    pub values: &'static [(&'static str, u32)],
}

impl EnumDef {
    /// Whether `value` is a declared wire value.
    ///
    /// Unknown values are *not* wire errors (forward compatibility:
    /// receivers ignore them at the semantic layer); strict validation
    /// mode uses this check to reject them for tooling.
    #[must_use]
    pub fn contains(&self, value: u32) -> bool {
        let mut i = 0;
        while i < self.values.len() {
            if self.values[i].1 == value {
                return true;
            }
            i += 1;
        }
        false
    }
}

/// A named 128-bit flag set.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BitsetDef {
    /// Short name, unique within its module.
    pub name: &'static str,
    /// Documentation.
    pub doc: &'static str,
    /// `(name, bit index)` pairs sorted by index.
    pub bits: &'static [(&'static str, u32)],
}

impl BitsetDef {
    /// One past the highest declared bit index (0 when no bits).
    ///
    /// Bits at or above the width are undeclared: tolerated on the wire
    /// (forward compatibility), rejected by strict validation.
    #[must_use]
    pub fn width(&self) -> u32 {
        match self.bits.len() {
            0 => 0,
            n => self.bits[n - 1].1 + 1,
        }
    }
}

/// One protocol module.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ModuleSchema {
    /// Fully qualified module name (`ldp.core`).
    pub name: &'static str,
    /// Minimum wire version.
    pub version_min: u32,
    /// Maximum wire version.
    pub version_max: u32,
    /// Documentation.
    pub doc: &'static str,
    /// Interfaces, declaration order.
    pub interfaces: &'static [&'static InterfaceSchema],
    /// Enums, name order.
    pub enums: &'static [&'static EnumDef],
    /// Bitsets, name order.
    pub bitsets: &'static [&'static BitsetDef],
}

/// Parse a spec type spelling into its wire tag (array element `of`
/// values). The inverse mapping of [`ArgType::as_str`].
#[must_use]
pub fn arg_type_from_spec_name(name: &str) -> Option<ArgType> {
    match name {
        "int32" => Some(ArgType::Int32),
        "uint32" => Some(ArgType::Uint32),
        "int64" => Some(ArgType::Int64),
        "uint64" => Some(ArgType::Uint64),
        "float32" => Some(ArgType::Float32),
        "float64" => Some(ArgType::Float64),
        "bool" => Some(ArgType::Bool),
        "fd" => Some(ArgType::Fd),
        "rect" => Some(ArgType::Rect),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direction_names() {
        assert_eq!(Direction::Request.as_str(), "request");
        assert_eq!(Direction::Event.as_str(), "event");
    }

    #[test]
    fn spec_name_mapping_covers_array_elements() {
        for name in [
            "int32", "uint32", "int64", "uint64", "float32", "float64", "bool", "fd", "rect",
        ] {
            let t = arg_type_from_spec_name(name).unwrap();
            assert_eq!(t.as_str(), name);
        }
        // Non-element spellings are not array elements.
        assert!(arg_type_from_spec_name("string").is_none());
        assert!(arg_type_from_spec_name("enum").is_none());
        assert!(arg_type_from_spec_name("object").is_none());
        assert!(arg_type_from_spec_name("array").is_none());
        assert!(arg_type_from_spec_name("nope").is_none());
    }

    #[test]
    fn generated_registry_shape() {
        let reg = crate::REGISTRY;
        // Nine modules since Phase 22 (capture joined the spec).
        assert_eq!(reg.modules().len(), 9);
        let capture = reg.module("ldp.capture").unwrap();
        assert_eq!(capture.version_max, 1);
        assert!(reg.interface("ldp.capture.capture_manager").unwrap().global);
        let core = reg.module("ldp.core").unwrap();
        assert_eq!(core.version_min, 1);
        let connection = reg.interface("ldp.core.connection").unwrap();
        assert_eq!(connection.builtin_id, Some(1));
        assert!(!connection.global);
        // hello is request opcode 1 and replies with welcome (opcode 1 event).
        let hello = connection.find_op(Direction::Request, 1).unwrap();
        assert_eq!(hello.name, "hello");
        assert_eq!(hello.reply, Some("welcome"));
        assert_eq!(
            connection.find_op(Direction::Event, 1).unwrap().name,
            "welcome"
        );
        assert!(connection.find_op(Direction::Request, 9999).is_none());
    }

    #[test]
    fn enum_contains_and_bitset_width() {
        let reg = crate::REGISTRY;
        let ec = reg.resolve_enum("ldp.core", "error_code").unwrap();
        assert!(ec.contains(1) && ec.contains(15));
        assert!(!ec.contains(0) && !ec.contains(16));
        let caps = reg.resolve_bitset("ldp.core", "server_caps").unwrap();
        assert!(caps.width() >= 1);
    }
}
