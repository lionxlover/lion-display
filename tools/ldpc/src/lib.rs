//! # ldpc — the LDP protocol compiler
//!
//! Compiles `spec/*.toml` (grammar: `docs/spec-format.md`) into everything
//! the runtime needs:
//!
//! * **Rust schema tables** — [`rust_gen`] emits `mod.rs` + one file per
//!   module for `crates/ldp-protocol/src/generated/` (static
//!   [`ModuleSchema`](model::SpecSet) tables, typed enums, bitset consts,
//!   opcode constants, rustdoc from spec `doc` fields),
//! * **introspection blobs** — [`blob_gen`] re-serializes the spec into the
//!   compact `LDPS` format parsed by `ldp_protocol::blob`,
//! * **reference docs** — [`md_gen`] renders `docs/reference/<module>.md`.
//!
//! Pipeline: parse (`parser`, shape-strict) → validate (`validate`,
//! every invariant of spec-format §2, cross-module) → generate. Output is
//! byte-stable, so CI regenerates and fails on drift: the committed
//! generated code and the spec cannot disagree.
//!
//! ```no_run
//! let spec = ldpc::compile_dir(std::path::Path::new("spec")).unwrap();
//! for name in spec.rust_files().keys() {
//!     println!("generated: {name}");
//! }
//! ```
//!
//! Dependency policy (CONTRIBUTING.md rule 5): `ldpc` is tooling; it may
//! depend on `toml`/`serde` because no *runtime* crate ever depends on it —
//! generated output is committed.

pub mod blob_gen;
pub mod md_gen;
pub mod model;
pub mod names;
pub mod rust_gen;

mod parser;
mod validate;

use std::fmt;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

pub use model::SpecSet;

/// One validation finding, rendered as `<file>: <path>: <message>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    /// File (module name or stem) the finding came from.
    pub file: String,
    /// Location path inside the module, e.g. `interface 'surface'.request 'damage'`.
    pub path: String,
    /// What is wrong.
    pub message: String,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}: {}", self.file, self.path, self.message)
    }
}

/// Parse and validate a whole spec set. `sources` is
/// `(file stem, TOML text)` pairs; one pair per module file.
///
/// # Errors
///
/// Every violation found across all files, in file order. Empty `sources`
/// is an error too: a spec set with no modules is not a protocol.
pub fn compile(sources: &[(String, String)]) -> Result<SpecSet, Vec<Diagnostic>> {
    if sources.is_empty() {
        return Err(vec![Diagnostic {
            file: "(spec)".into(),
            path: "-".into(),
            message: "no spec modules given".into(),
        }]);
    }
    let mut modules = Vec::with_capacity(sources.len());
    let mut diags = Vec::new();
    for (stem, text) in sources {
        match parser::parse(stem, text) {
            Ok(m) => {
                let expected = format!("ldp.{stem}");
                if m.name != expected {
                    diags.push(Diagnostic {
                        file: stem.clone(),
                        path: "[module]".into(),
                        message: format!("module name must be '{expected}', got '{}'", m.name),
                    });
                }
                modules.push(m);
            }
            Err(ds) => diags.extend(ds),
        }
    }
    if !diags.is_empty() {
        return Err(diags);
    }
    validate::finalize(modules)
}

/// Read `dir/*.toml` and compile. Files are processed sorted by stem.
///
/// # Errors
///
/// I/O failures reading the directory, or the full validation diagnostic
/// list from [`compile`].
pub fn compile_dir(dir: &Path) -> Result<SpecSet, Vec<Diagnostic>> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| {
            vec![Diagnostic {
                file: dir.display().to_string(),
                path: "(io)".into(),
                message: format!("cannot read spec directory: {e}"),
            }]
        })?
        .filter_map(|ent| ent.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect();
    paths.sort();
    let mut sources = Vec::with_capacity(paths.len());
    for p in &paths {
        let stem = p
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        let text = std::fs::read_to_string(p).map_err(|e| {
            vec![Diagnostic {
                file: stem.clone(),
                path: "(io)".into(),
                message: format!("cannot read {}: {e}", p.display()),
            }]
        })?;
        sources.push((stem, text));
    }
    compile(&sources)
}

impl SpecSet {
    /// Generate every output file: Rust sources (`mod.rs`, `<stem>.rs`),
    /// keyed by file name for the generated directory.
    #[must_use]
    pub fn rust_files(&self) -> std::collections::BTreeMap<String, String> {
        rust_gen::generate_files(self)
    }

    /// Generate the introspection blob.
    #[must_use]
    pub fn blob(&self) -> Vec<u8> {
        blob_gen::generate_blob(self)
    }

    /// Generate the reference docs, keyed by file name.
    #[must_use]
    pub fn markdown_files(&self) -> std::collections::BTreeMap<String, String> {
        md_gen::generate_files(self)
    }

    /// Render the frozen API *surface* (Phase 31's stability
    /// contract): every module, interface, operation, opcode, `since`,
    /// reply, and argument — the complete client-facing shape, docs
    /// excluded. Deterministic (module name order, declaration order),
    /// hand-encoded JSON (no serializer dependency; the dep policy
    /// stays closed).
    ///
    /// The `snapshot` command writes this to `spec/surface-v1.json`;
    /// `--check` fails on any difference — a frozen name, opcode,
    /// argument, or version that moved breaks the build until the
    /// change is consciously re-frozen. Additions are legal (the
    /// append-only evolution policy) but visible: the gate's diff
    /// names every one.
    #[must_use]
    pub fn render_surface(&self) -> String {
        let mut out = String::from("{\n  \"surface\": \"v1\",\n  \"modules\": [\n");
        for (i, module) in self.modules.iter().enumerate() {
            if i > 0 {
                out.push_str(",\n");
            }
            let _ = write!(
                out,
                "    {{\"name\": \"{}\", \"version_min\": {}, \"version_max\": {}, \"interfaces\": [",
                json_str(&module.name), module.version_min, module.version_max
            );
            for (j, iface) in module.interfaces.iter().enumerate() {
                if j > 0 {
                    out.push_str(", ");
                }
                let global = if iface.global { "true" } else { "false" };
                let builtin = match iface.builtin_id {
                    Some(id) => format!(", \"builtin_id\": {id}"),
                    None => String::new(),
                };
                let _ = write!(
                    out,
                    "{{\"name\": \"{}\", \"global\": {}{}, \"requests\": [{}], \"events\": [{}]}}",
                    json_str(&iface.name),
                    global,
                    builtin,
                    ops_json(&iface.requests),
                    ops_json(&iface.events),
                );
            }
            out.push_str("]}");
        }
        out.push_str("\n  ]\n}\n");
        out
    }
}

/// One interface's operations, JSON-encoded (opcodes are declaration
/// order + 1 — the wire's frozen numbering).
fn ops_json(ops: &[model::Op]) -> String {
    ops.iter()
        .enumerate()
        .map(|(i, op)| {
            let reply = match &op.reply {
                Some(r) => format!(", \"reply\": \"{}\"", json_str(r)),
                None => String::new(),
            };
            let args: Vec<String> = op
                .args
                .iter()
                .map(|a| {
                    let of = match &a.of {
                        Some(o) => format!(", \"of\": \"{}\"", json_str(o)),
                        None => String::new(),
                    };
                    let nullable = if a.nullable {
                        ", \"nullable\": true"
                    } else {
                        ""
                    };
                    format!(
                        "{{\"name\": \"{}\", \"ty\": \"{}\"{}{}}}",
                        json_str(&a.name),
                        a.ty.as_str(),
                        of,
                        nullable
                    )
                })
                .collect();
            format!(
                "{{\"name\": \"{}\", \"opcode\": {}, \"since\": {}{}, \"args\": [{}]}}",
                json_str(&op.name),
                i + 1,
                op.since,
                reply,
                args.join(", ")
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// JSON string escaping (the surface's names are identifiers; the
/// escaping is completeness, not expectation).
fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod testutil {
    //! Shared fixture: a minimal but complete spec set used by the
    //! generator unit tests.

    use crate::model::SpecSet;

    /// Fixture set with module `ldp.t`: enum `st`, bitset `caps`,
    /// interface `thing` (request `poke` replying to event `poked`).
    pub(crate) fn fixture_set() -> SpecSet {
        const TOML: &str = r#"
[module]
name = "ldp.t"
version_min = 1
version_max = 2
doc = "Fixture module."

[[enum]]
name = "st"
doc = "State."
[enum.values]
off = 1
on = 2

[[bitset]]
name = "caps"
doc = "Caps."
[bitset.bits]
fast = 0
slow = 3

[[interface]]
name = "thing"
doc = "A thing."
[[interface.request]]
name = "poke"
doc = "Poke it."
reply = "poked"
args = [{ name = "n", type = "uint32" }]
[[interface.event]]
name = "poked"
doc = "It was poked."
args = [{ name = "s", type = "enum", of = "st" }]
"#;
        crate::compile(&[("t".to_string(), TOML.to_string())]).expect("fixture compiles")
    }
}
