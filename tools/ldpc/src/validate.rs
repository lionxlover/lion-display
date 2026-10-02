//! Semantic validation of parsed modules.
//!
//! Implements every invariant of `docs/spec-format.md` §2 — the same rules
//! `scripts/spec_lint.py` enforces in CI — plus the rules a *whole-set*
//! compiler can see and a file-by-file linter cannot: globally unique
//! fully-qualified interface names, import resolution across modules, and
//! name disjointness between enums, bitsets, and interfaces of one module
//! (a shared name would make `of = "…"` ambiguous).

use std::collections::{BTreeMap, BTreeSet};

use crate::model::{ArgTypeSpec, ImportKind, Module, SpecSet};
use crate::{names, Diagnostic};

/// Interfaces the `ldp.core` module must always define (bootstrap contract).
const CORE_REQUIRED_INTERFACES: &[&str] = &[
    "connection",
    "registry",
    "output",
    "surface",
    "buffer",
    "shm",
    "dmabuf",
];

/// Validate a whole parsed set. Returns all diagnostics (empty = valid).
pub(super) fn validate(modules: &[Module]) -> Vec<Diagnostic> {
    let mut v = Validator {
        by_name: BTreeMap::new(),
        diags: Vec::new(),
    };
    for m in modules {
        if v.by_name.insert(m.name.clone(), m).is_some() {
            v.diags.push(Diagnostic {
                file: m.name.clone(),
                path: "[module]".into(),
                message: format!("duplicate module name '{}'", m.name),
            });
        }
    }
    for m in modules {
        v.module(m);
    }
    v.diags
}

struct Validator<'a> {
    by_name: BTreeMap<String, &'a Module>,
    diags: Vec<Diagnostic>,
}

impl<'a> Validator<'a> {
    fn err(&mut self, file: &str, path: String, message: String) {
        self.diags.push(Diagnostic {
            file: file.into(),
            path,
            message,
        });
    }

    fn module(&mut self, m: &'a Module) {
        let file = &m.name;
        self.check_names(file, m);
        self.check_imports(file, m);
        self.check_enums(file, m);
        self.check_bitsets(file, m);
        self.check_interfaces(file, m);
        if m.name == "ldp.core" {
            let defined: BTreeSet<&str> = m.interfaces.iter().map(|i| i.name.as_str()).collect();
            for must in CORE_REQUIRED_INTERFACES {
                if !defined.contains(must) {
                    self.err(
                        file,
                        "[module]".into(),
                        format!("core module must define interface '{must}'"),
                    );
                }
            }
        }
    }

    /// snake_case names, non-empty docs, and pairwise disjoint names across
    /// the three importable kinds (ambiguity would break `of` resolution).
    fn check_names(&mut self, file: &str, m: &Module) {
        if m.doc.is_empty() {
            self.err(
                file,
                "[module]".into(),
                "module doc must be non-empty".into(),
            );
        }
        if !(1..=m.version_max).contains(&m.version_min) {
            self.err(
                file,
                "[module]".into(),
                format!(
                    "version range {}..{} invalid (need 1 <= min <= max)",
                    m.version_min, m.version_max
                ),
            );
        }
        let mut seen: BTreeSet<(u8, &str)> = BTreeSet::new();
        let mut kinds: Vec<(u8, &str)> = Vec::new();
        for e in &m.enums {
            kinds.push((0, &e.name));
        }
        for b in &m.bitsets {
            kinds.push((1, &b.name));
        }
        for i in &m.interfaces {
            kinds.push((2, &i.name));
        }
        for (kind, name) in &kinds {
            check_snake(self, file, name, "name");
            if !seen.insert((*kind, *name)) {
                self.err(
                    file,
                    "[module]".into(),
                    format!("duplicate name '{name}' within its kind"),
                );
            }
        }
        let plain: BTreeSet<&str> = kinds.iter().map(|(_, n)| *n).collect();
        if plain.len() != kinds.len() {
            self.err(
                file,
                "[module]".into(),
                "enum, bitset, and interface names must be pairwise disjoint ('of' resolution \
                 must be unambiguous)"
                    .into(),
            );
        }
    }

    fn check_imports(&mut self, file: &str, m: &Module) {
        let mut seen: BTreeSet<(ImportKind, &str)> = BTreeSet::new();
        for imp in &m.imports {
            if !imp.name.starts_with("ldp.") || !imp.name.contains('.') {
                self.err(
                    file,
                    "[[import]]".into(),
                    format!("'{}' must be a fully qualified ldp.* name", imp.name),
                );
                continue;
            }
            if !seen.insert((imp.kind, imp.name.as_str())) {
                self.err(
                    file,
                    "[[import]]".into(),
                    format!("duplicate import {} '{}'", imp.kind.as_str(), imp.name),
                );
            }
            let Some((target_mod, item)) = imp.name.rsplit_once('.') else {
                continue;
            };
            let Some(tm) = self.by_name.get(target_mod) else {
                self.err(
                    file,
                    "[[import]]".into(),
                    format!("'{}' imports from unknown module '{target_mod}'", imp.name),
                );
                continue;
            };
            let found = match imp.kind {
                ImportKind::Interface => tm.interfaces.iter().any(|i| i.name == item),
                ImportKind::Enum => tm.enums.iter().any(|e| e.name == item),
                ImportKind::Bitset => tm.bitsets.iter().any(|b| b.name == item),
            };
            if !found {
                self.err(
                    file,
                    "[[import]]".into(),
                    format!(
                        "'{}' does not name a(n) {} of module '{target_mod}'",
                        imp.name,
                        imp.kind.as_str()
                    ),
                );
            }
        }
    }

    fn check_enums(&mut self, file: &str, m: &Module) {
        for e in &m.enums {
            if e.doc.is_empty() {
                self.err(
                    file,
                    format!("enum '{}'", e.name),
                    "doc must be non-empty".into(),
                );
            }
            if e.values.is_empty() {
                self.err(
                    file,
                    format!("enum '{}'", e.name),
                    "values table must be non-empty".into(),
                );
                continue;
            }
            for (vname, _) in &e.values {
                check_snake(self, file, vname, "enum value name");
            }
            let dense = e
                .values
                .iter()
                .enumerate()
                .all(|(i, &(_, v))| v == i as u32 + 1);
            if !dense {
                self.err(
                    file,
                    format!("enum '{}'", e.name),
                    format!(
                        "values must be dense 1..={} (got {:?})",
                        e.values.len(),
                        e.values.iter().map(|&(_, v)| v).collect::<Vec<_>>()
                    ),
                );
            }
        }
    }

    fn check_bitsets(&mut self, file: &str, m: &Module) {
        for b in &m.bitsets {
            if b.doc.is_empty() {
                self.err(
                    file,
                    format!("bitset '{}'", b.name),
                    "doc must be non-empty".into(),
                );
            }
            if b.bits.is_empty() {
                self.err(
                    file,
                    format!("bitset '{}'", b.name),
                    "bits table must be non-empty".into(),
                );
            }
            for (bname, _) in &b.bits {
                check_snake(self, file, bname, "bit name");
            }
        }
    }

    fn check_interfaces(&mut self, file: &str, m: &Module) {
        for iface in &m.interfaces {
            let base = format!("interface '{}'", iface.name);
            if iface.doc.is_empty() {
                self.err(file, base.clone(), "doc must be non-empty".into());
            }
            if iface.requests.is_empty() && iface.events.is_empty() {
                self.err(
                    file,
                    base.clone(),
                    "interface declares no requests and no events".into(),
                );
            }
            match iface.builtin_id {
                None => {}
                Some(1) if m.name == "ldp.core" && iface.name == "connection" => {}
                Some(id) => self.err(
                    file,
                    base.clone(),
                    format!("builtin_id {id} is reserved to ldp.core.connection (ID 1) alone"),
                ),
            }
            let mut req_names: BTreeSet<&str> = BTreeSet::new();
            for op in &iface.requests {
                let path = format!("{base}.request '{}'", op.name);
                check_snake(self, file, &op.name, "request name");
                if !req_names.insert(&op.name) {
                    self.err(file, path.clone(), "duplicate request name".into());
                }
                self.check_op(file, m, &path, op, true, &iface.events);
            }
            let mut ev_names: BTreeSet<&str> = BTreeSet::new();
            for op in &iface.events {
                let path = format!("{base}.event '{}'", op.name);
                check_snake(self, file, &op.name, "event name");
                if !ev_names.insert(&op.name) {
                    self.err(file, path.clone(), "duplicate event name".into());
                }
                self.check_op(file, m, &path, op, false, &[]);
            }
        }
    }

    fn check_op(
        &mut self,
        file: &str,
        m: &Module,
        path: &str,
        op: &crate::model::Op,
        is_request: bool,
        events: &[crate::model::Op],
    ) {
        if op.doc.is_empty() {
            self.err(file, path.into(), "doc must be non-empty".into());
        }
        if !(m.version_min..=m.version_max).contains(&op.since) {
            self.err(
                file,
                path.into(),
                format!(
                    "since {} outside module version range {}..{}",
                    op.since, m.version_min, m.version_max
                ),
            );
        }
        if is_request {
            if let Some(reply) = &op.reply {
                if !events.iter().any(|e| &e.name == reply) {
                    self.err(
                        file,
                        path.into(),
                        format!("reply event '{reply}' is not declared by this interface"),
                    );
                }
            }
        }
        let mut arg_names: BTreeSet<&str> = BTreeSet::new();
        for arg in &op.args {
            let apath = format!("{path} arg '{}'", arg.name);
            check_snake(self, file, &arg.name, "arg name");
            if !arg_names.insert(&arg.name) {
                self.err(file, apath.clone(), "duplicate argument name".into());
            }
            if arg.nullable && arg.ty != ArgTypeSpec::Object {
                self.err(
                    file,
                    apath.clone(),
                    "'nullable' is only valid for object arguments".into(),
                );
            }
            self.check_of(file, m, &apath, arg);
        }
    }

    fn check_of(&mut self, file: &str, m: &Module, apath: &str, arg: &crate::model::Arg) {
        use ArgTypeSpec as T;
        let need_of = matches!(arg.ty, T::Enum | T::Bitset | T::Array);
        if need_of && arg.of.is_none() {
            self.err(
                file,
                apath.into(),
                format!("type '{}' requires 'of'", arg.ty.as_str()),
            );
            return;
        }
        let Some(of) = &arg.of else { return };
        match arg.ty {
            T::Enum => {
                if self.lookup(m, Lookup::Enum, of).is_none() {
                    self.err(
                        file,
                        apath.into(),
                        format!("enum '{of}' is neither defined nor imported by this module"),
                    );
                }
            }
            T::Bitset => {
                if self.lookup(m, Lookup::Bitset, of).is_none() {
                    self.err(
                        file,
                        apath.into(),
                        format!("bitset '{of}' is neither defined nor imported by this module"),
                    );
                }
            }
            T::Object | T::NewId => {
                if self.lookup(m, Lookup::Interface, of).is_none() {
                    self.err(
                        file,
                        apath.into(),
                        format!("interface '{of}' is neither defined nor imported by this module"),
                    );
                }
            }
            T::Array => match ArgTypeSpec::parse(of) {
                Some(elem) if elem.is_array_element() => {}
                _ => self.err(
                    file,
                    apath.into(),
                    format!("array element type '{of}' must be one of the scalar tags or rect"),
                ),
            },
            _ => self.err(
                file,
                apath.into(),
                format!("type '{}' must not carry 'of'", arg.ty.as_str()),
            ),
        }
    }

    /// Resolve an `of` reference: short name in this module, or fully
    /// qualified via the import list.
    fn lookup(&self, m: &Module, kind: Lookup, of: &str) -> Option<()> {
        let local = |name: &str| match kind {
            Lookup::Enum => m.enums.iter().any(|e| e.name == name),
            Lookup::Bitset => m.bitsets.iter().any(|b| b.name == name),
            Lookup::Interface => m.interfaces.iter().any(|i| i.name == name),
        };
        if !of.contains('.') {
            return local(of).then_some(());
        }
        let (target_mod, item) = of.rsplit_once('.')?;
        let imported = m.imports.iter().any(|imp| {
            imp.name == of
                && matches!(
                    (imp.kind, kind),
                    (ImportKind::Enum, Lookup::Enum)
                        | (ImportKind::Bitset, Lookup::Bitset)
                        | (ImportKind::Interface, Lookup::Interface)
                )
        });
        let tm = self.by_name.get(target_mod)?;
        let defined = match kind {
            Lookup::Enum => tm.enums.iter().any(|e| e.name == item),
            Lookup::Bitset => tm.bitsets.iter().any(|b| b.name == item),
            Lookup::Interface => tm.interfaces.iter().any(|i| i.name == item),
        };
        (imported && defined).then_some(())
    }
}

#[derive(Clone, Copy)]
enum Lookup {
    Enum,
    Bitset,
    Interface,
}

fn check_snake(v: &mut Validator<'_>, file: &str, name: &str, what: &str) {
    if !names::is_snake_name(name) {
        v.err(
            file,
            "[module]".into(),
            format!("{what} '{name}' must be snake_case ([a-z_][a-z0-9_]*)"),
        );
    }
    if names::is_unrepresentable(name) {
        v.err(
            file,
            "[module]".into(),
            format!("{what} '{name}' has no Rust identifier spelling"),
        );
    }
}

/// Validate that the set is compilable and return it name-sorted.
pub(super) fn finalize(modules: Vec<Module>) -> Result<SpecSet, Vec<Diagnostic>> {
    let diags = validate(&modules);
    if diags.is_empty() {
        let mut set = SpecSet { modules };
        set.modules.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(set)
    } else {
        Err(diags)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module(text: &str) -> Module {
        crate::parser::parse("m", text).expect("parse")
    }

    const BASE: &str = r#"
[module]
name = "ldp.m"
version_min = 1
version_max = 1
doc = "D."
[[interface]]
name = "thing"
doc = "A thing."
[[interface.request]]
name = "poke"
doc = "P."
args = [{ name = "n", type = "uint32" }]
[[interface.event]]
name = "poked"
doc = "E."
args = [{ name = "s", type = "enum", of = "st" }]
[[enum]]
name = "st"
doc = "State."
[enum.values]
a = 1
b = 2
"#;

    #[test]
    fn valid_module_passes() {
        let diags = validate(&[module(BASE)]);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn non_dense_enum_fails() {
        let bad = BASE.replace("b = 2", "b = 3");
        let diags = validate(&[module(&bad)]);
        assert!(
            diags.iter().any(|d| d.message.contains("dense")),
            "{diags:?}"
        );
    }

    #[test]
    fn nullable_on_scalar_fails() {
        let bad = BASE.replace(
            "args = [{ name = \"n\", type = \"uint32\" }]",
            "args = [{ name = \"n\", type = \"uint32\", nullable = true }]",
        );
        let diags = validate(&[module(&bad)]);
        assert!(
            diags
                .iter()
                .any(|d| d.message.contains("'nullable' is only valid for object")),
            "{diags:?}"
        );
    }

    #[test]
    fn builtin_id_outside_connection_fails() {
        let bad = BASE.replace(
            "name = \"thing\"\ndoc = \"A thing.\"",
            "name = \"thing\"\ndoc = \"A thing.\"\nbuiltin_id = 5",
        );
        let diags = validate(&[module(&bad)]);
        assert!(
            diags
                .iter()
                .any(|d| d.message.contains("reserved to ldp.core.connection")),
            "{diags:?}"
        );
    }

    #[test]
    fn cross_module_import_resolution() {
        let other = r#"
[module]
name = "ldp.other"
version_min = 1
version_max = 1
doc = "O."
[[import]]
enum = "ldp.m.st"
[[interface]]
name = "mirror"
doc = "M."
[[interface.event]]
name = "x"
doc = "X."
args = [{ name = "s", type = "enum", of = "ldp.m.st" }]
"#;
        let diags = validate(&[module(BASE), module(other)]);
        assert!(diags.is_empty(), "{diags:?}");

        let unresolved = r#"
[module]
name = "ldp.other"
version_min = 1
version_max = 1
doc = "O."
[[import]]
enum = "ldp.m.missing"
[[interface]]
name = "mirror"
doc = "M."
[[interface.event]]
name = "x"
doc = "X."
args = [{ name = "s", type = "enum", of = "ldp.m.missing" }]
"#;
        let diags = validate(&[module(BASE), module(unresolved)]);
        assert_eq!(diags.len(), 2, "{diags:?}");
        assert!(diags
            .iter()
            .any(|d| d.message.contains("does not name a(n) enum")));
        assert!(diags
            .iter()
            .any(|d| d.message.contains("neither defined nor imported")));
    }

    #[test]
    fn fq_interface_names_globally_unique() {
        let mut a = module(BASE);
        a.name = "ldp.a".into();
        let mut b = module(BASE);
        b.name = "ldp.b".into();
        // Same short name in different modules is fine.
        assert!(validate(&[a.clone(), b]).is_empty());
        // Two files claiming one module name is not.
        let diags = validate(&[a.clone(), a]);
        assert!(diags
            .iter()
            .any(|d| d.message.contains("duplicate module name")));
    }

    #[test]
    fn overlapping_names_across_kinds_fail() {
        let bad = BASE.replace(
            "[[enum]]\nname = \"st\"",
            "[[bitset]]\nname = \"st\"\ndoc = \"S.\"\n[bitset.bits]\nx = 0\n\n[[enum]]\nname = \"st\"",
        );
        let diags = validate(&[module(&bad)]);
        assert!(
            diags
                .iter()
                .any(|d| d.message.contains("pairwise disjoint")),
            "{diags:?}"
        );
    }
}
