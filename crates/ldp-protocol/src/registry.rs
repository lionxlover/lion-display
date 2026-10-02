//! Schema lookup over the compiled module tables.
//!
//! [`SchemaRegistry`] answers the questions dispatch asks: which module
//! and interface does a fully qualified name denote, which operation is
//! `(interface, direction, opcode)`, and which enum/bitset does an `of`
//! reference resolve to (short names resolve within the referencing
//! module, dotted names globally — matching the spec's reference rules).

use crate::schema::{
    ArgSchema, BitsetDef, Direction, EnumDef, InterfaceSchema, ModuleSchema, OpSchema,
};

/// Static schema lookup table.
///
/// Built from the `generated` module's [`MODULES`](crate::MODULES) tables;
/// tests may build smaller registries from their own statics.
#[derive(Clone, Copy, Debug)]
pub struct SchemaRegistry<'a> {
    modules: &'a [&'a ModuleSchema],
}

impl<'a> SchemaRegistry<'a> {
    /// Wrap a slice of module schemas (sorted by name is conventional).
    #[must_use]
    pub const fn new(modules: &'a [&'a ModuleSchema]) -> SchemaRegistry<'a> {
        SchemaRegistry { modules }
    }

    /// Every module in the registry.
    #[must_use]
    pub const fn modules(&self) -> &'a [&'a ModuleSchema] {
        self.modules
    }

    /// Look up a module by fully qualified name.
    #[must_use]
    pub fn module(&self, name: &str) -> Option<&'a ModuleSchema> {
        self.modules.iter().copied().find(|m| m.name == name)
    }

    /// Look up an interface by fully qualified name (`ldp.core.surface`).
    ///
    /// Interface schema names are fully qualified (ldpc emits them that
    /// way), so this is a direct match — the module is still used for
    /// `of`-reference resolution.
    #[must_use]
    pub fn interface(&self, fq: &str) -> Option<&'a InterfaceSchema> {
        self.modules
            .iter()
            .flat_map(|m| m.interfaces.iter())
            .copied()
            .find(|i| i.name == fq)
    }

    /// The module that defines the interface with this fully qualified name.
    #[must_use]
    pub fn module_of_interface(&self, fq: &str) -> Option<&'a ModuleSchema> {
        self.modules
            .iter()
            .copied()
            .find(|m| m.interfaces.iter().any(|i| i.name == fq))
    }

    /// Resolve an enum `of` reference: short name in `in_module`, or a
    /// fully qualified `ldp.<module>.<name>` globally.
    #[must_use]
    pub fn resolve_enum(&self, in_module: &str, of: &str) -> Option<&'a EnumDef> {
        let (module, item) = split_ref(in_module, of)?;
        self.module(module)?
            .enums
            .iter()
            .copied()
            .find(|e| e.name == item)
    }

    /// Resolve a bitset `of` reference (same rules as [`Self::resolve_enum`]).
    #[must_use]
    pub fn resolve_bitset(&self, in_module: &str, of: &str) -> Option<&'a BitsetDef> {
        let (module, item) = split_ref(in_module, of)?;
        self.module(module)?
            .bitsets
            .iter()
            .copied()
            .find(|b| b.name == item)
    }

    /// Find an operation by interface, direction, and opcode.
    #[must_use]
    pub fn find_op(
        &self,
        interface: &str,
        dir: Direction,
        opcode: u32,
    ) -> Option<(&'a InterfaceSchema, &'a OpSchema)> {
        let iface = self.interface(interface)?;
        iface.find_op(dir, opcode).map(|op| (iface, op))
    }

    /// Find an argument schema by interface, direction, opcode, and arg name.
    #[must_use]
    pub fn find_arg(
        &self,
        interface: &str,
        dir: Direction,
        opcode: u32,
        arg: &str,
    ) -> Option<&'a ArgSchema> {
        self.find_op(interface, dir, opcode)?
            .1
            .args
            .iter()
            .find(|a| a.name == arg)
    }
}

/// Split `ldp.core.surface` into (`ldp.core`, `surface`).
fn split_fq(fq: &str) -> Option<(&str, &str)> {
    let idx = fq.rfind('.')?;
    Some((&fq[..idx], &fq[idx + 1..]))
}

/// Resolve a reference spelling: dotted means fully qualified, bare means
/// local to `in_module`.
fn split_ref<'m>(in_module: &'m str, of: &'m str) -> Option<(&'m str, &'m str)> {
    if of.contains('.') {
        split_fq(of)
    } else {
        Some((in_module, of))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::REGISTRY;

    #[test]
    fn module_and_interface_lookup() {
        // Nine modules since Phase 22 (capture joined the spec).
        assert_eq!(REGISTRY.modules().len(), 9);
        assert!(REGISTRY.module("ldp.capture").is_some());
        assert!(REGISTRY.module("ldp.shell").is_some());
        assert!(REGISTRY.module("ldp.nothing").is_none());
        assert!(REGISTRY.interface("ldp.core.surface").is_some());
        assert!(REGISTRY.interface("ldp.core.not_there").is_none());
        assert!(REGISTRY.interface("surface").is_none()); // bare names are not interfaces
        let m = REGISTRY.module_of_interface("ldp.input.seat").unwrap();
        assert_eq!(m.name, "ldp.input");
        assert!(REGISTRY.module_of_interface("ldp.core.nope").is_none());
    }

    #[test]
    fn short_and_fq_of_resolution() {
        // Short name within the referencing module.
        let e = REGISTRY.resolve_enum("ldp.core", "transform").unwrap();
        assert_eq!(e.name, "transform");
        // Fully qualified cross-module reference (session imports it).
        let e = REGISTRY
            .resolve_enum("ldp.session", "ldp.core.transform")
            .unwrap();
        assert_eq!(e.name, "transform");
        assert!(REGISTRY.resolve_enum("ldp.shell", "transform").is_none());
        let b = REGISTRY.resolve_bitset("ldp.core", "output_caps").unwrap();
        assert_eq!(b.name, "output_caps");
        assert!(REGISTRY
            .resolve_bitset("ldp.shell", "output_caps")
            .is_none());
    }

    #[test]
    fn find_op_and_arg() {
        let (iface, op) = REGISTRY
            .find_op("ldp.core.surface", Direction::Request, 2)
            .unwrap();
        assert_eq!(iface.name, "ldp.core.surface");
        assert_eq!(op.name, "damage");
        let arg = REGISTRY
            .find_arg("ldp.core.surface", Direction::Request, 2, "rects")
            .unwrap();
        assert_eq!(arg.of, Some("rect"));
        assert!(REGISTRY
            .find_op("ldp.core.surface", Direction::Request, 999)
            .is_none());
        assert!(REGISTRY
            .find_arg("ldp.core.surface", Direction::Request, 2, "nope")
            .is_none());
    }

    #[test]
    fn every_interface_finds_its_module() {
        for m in REGISTRY.modules() {
            for i in m.interfaces {
                assert_eq!(
                    REGISTRY.module_of_interface(i.name).map(|mm| mm.name),
                    Some(m.name),
                    "module_of_interface failed for {}",
                    i.name
                );
            }
        }
    }
}
