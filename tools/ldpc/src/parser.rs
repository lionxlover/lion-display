//! TOML text to `model::Module`.
//!
//! The raw mirror structs use `#[serde(deny_unknown_fields)]` everywhere so
//! unknown keys are *rejected*, not ignored (`docs/spec-format.md`: "anything
//! not listed is an error"). Semantic rules live in
//! [`validate`](crate::validate); this layer only guarantees shape.

use std::collections::BTreeMap;

use crate::model::{
    Arg, ArgTypeSpec, BitsetDef, EnumDef, Import, ImportKind, Interface, Module, Op,
};
use crate::Diagnostic;

/// Raw mirror of one module file (`docs/spec-format.md` §1).
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    module: RawModule,
    #[serde(rename = "import")]
    import: Option<Vec<RawImport>>,
    #[serde(rename = "enum")]
    enum_: Option<Vec<RawEnum>>,
    bitset: Option<Vec<RawBitset>>,
    interface: Option<Vec<RawInterface>>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawModule {
    name: String,
    version_min: i64,
    version_max: i64,
    doc: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawImport {
    interface: Option<String>,
    #[serde(rename = "enum")]
    enum_: Option<String>,
    bitset: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEnum {
    name: String,
    doc: String,
    values: BTreeMap<String, i64>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBitset {
    name: String,
    doc: String,
    bits: BTreeMap<String, i64>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawInterface {
    name: String,
    doc: String,
    global: Option<bool>,
    builtin_id: Option<i64>,
    #[serde(rename = "request")]
    request: Option<Vec<RawRequest>>,
    #[serde(rename = "event")]
    event: Option<Vec<RawEvent>>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRequest {
    name: String,
    since: Option<i64>,
    doc: String,
    args: Option<Vec<RawArg>>,
    reply: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEvent {
    name: String,
    since: Option<i64>,
    doc: String,
    args: Option<Vec<RawArg>>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawArg {
    name: String,
    #[serde(rename = "type")]
    ty: String,
    of: Option<String>,
    nullable: Option<bool>,
    doc: Option<String>,
}

/// Parse one module file's text. All diagnostics carry the file name.
pub(super) fn parse(file: &str, text: &str) -> Result<Module, Vec<Diagnostic>> {
    let raw: RawFile = toml::from_str(text).map_err(|e| {
        vec![Diagnostic {
            file: file.into(),
            path: "(toml)".into(),
            message: format!("TOML parse error: {e}"),
        }]
    })?;
    convert(file, raw)
}

fn convert(file: &str, raw: RawFile) -> Result<Module, Vec<Diagnostic>> {
    let mut diags = Vec::new();
    let imports = convert_imports(file, raw.import.unwrap_or_default(), &mut diags);
    let enums = convert_enums(file, raw.enum_.unwrap_or_default(), &mut diags);
    let bitsets = convert_bitsets(file, raw.bitset.unwrap_or_default(), &mut diags);
    let interfaces = convert_interfaces(file, raw.interface.unwrap_or_default(), &mut diags);
    let (version_min, version_max) = convert_version(file, &raw.module, &mut diags);
    if diags.is_empty() {
        Ok(Module {
            name: raw.module.name,
            version_min,
            version_max,
            doc: raw.module.doc.trim().to_string(),
            imports,
            enums,
            bitsets,
            interfaces,
        })
    } else {
        Err(diags)
    }
}

fn convert_version(file: &str, m: &RawModule, diags: &mut Vec<Diagnostic>) -> (u32, u32) {
    let pair = (
        u32::try_from(m.version_min).ok(),
        u32::try_from(m.version_max).ok(),
    );
    if let (Some(a), Some(b)) = pair {
        return (a, b);
    }
    diags.push(Diagnostic {
        file: file.into(),
        path: "[module]".into(),
        message: format!(
            "version range {}..{} is not representable as u32",
            m.version_min, m.version_max
        ),
    });
    (1, 1)
}

fn convert_imports(file: &str, raws: Vec<RawImport>, diags: &mut Vec<Diagnostic>) -> Vec<Import> {
    let mut imports = Vec::new();
    for imp in raws {
        let present: Vec<&str> = [
            imp.interface.as_deref().map(|_| "interface"),
            imp.enum_.as_deref().map(|_| "enum"),
            imp.bitset.as_deref().map(|_| "bitset"),
        ]
        .into_iter()
        .flatten()
        .collect();
        if present.len() != 1 {
            diags.push(Diagnostic {
                file: file.into(),
                path: "[[import]]".into(),
                message: format!(
                    "entry must set exactly one of interface/enum/bitset, got {present:?}"
                ),
            });
            continue;
        }
        let (kind, name) = if let Some(n) = imp.interface {
            (ImportKind::Interface, n)
        } else if let Some(n) = imp.enum_ {
            (ImportKind::Enum, n)
        } else {
            (ImportKind::Bitset, imp.bitset.unwrap_or_default())
        };
        imports.push(Import { kind, name });
    }
    imports
}

fn convert_enums(file: &str, raws: Vec<RawEnum>, diags: &mut Vec<Diagnostic>) -> Vec<EnumDef> {
    let mut enums = Vec::new();
    for e in raws {
        let mut values: Vec<(String, u32)> = Vec::new();
        for (vname, vnum) in e.values {
            match u32::try_from(vnum) {
                Ok(v) if vnum > 0 => values.push((vname, v)),
                _ => diags.push(Diagnostic {
                    file: file.into(),
                    path: format!("enum '{}'", e.name),
                    message: format!("value '{vname}' = {vnum} is not a positive u32"),
                }),
            }
        }
        values.sort_unstable_by_key(|&(_, v)| v);
        enums.push(EnumDef {
            name: e.name,
            doc: e.doc.trim().to_string(),
            values,
        });
    }
    enums
}

fn convert_bitsets(
    file: &str,
    raws: Vec<RawBitset>,
    diags: &mut Vec<Diagnostic>,
) -> Vec<BitsetDef> {
    let mut bitsets = Vec::new();
    for b in raws {
        let mut bits: Vec<(String, u32)> = Vec::new();
        for (bname, bidx) in b.bits {
            match u32::try_from(bidx) {
                Ok(i) if (0..=127).contains(&i) => bits.push((bname, i)),
                _ => diags.push(Diagnostic {
                    file: file.into(),
                    path: format!("bitset '{}'", b.name),
                    message: format!("bit '{bname}' index {bidx} outside 0..=127"),
                }),
            }
        }
        bits.sort_unstable_by_key(|&(_, i)| i);
        bitsets.push(BitsetDef {
            name: b.name,
            doc: b.doc.trim().to_string(),
            bits,
        });
    }
    bitsets
}

fn convert_interfaces(
    file: &str,
    raws: Vec<RawInterface>,
    diags: &mut Vec<Diagnostic>,
) -> Vec<Interface> {
    let mut interfaces = Vec::new();
    for iface in raws {
        let requests =
            convert_requests(file, &iface.name, iface.request.unwrap_or_default(), diags);
        let events = convert_events(file, &iface.name, iface.event.unwrap_or_default(), diags);
        let builtin_id = convert_builtin_id(file, &iface.name, iface.builtin_id, diags);
        interfaces.push(Interface {
            name: iface.name,
            doc: iface.doc.trim().to_string(),
            global: iface.global.unwrap_or(false),
            builtin_id,
            requests,
            events,
        });
    }
    interfaces
}

fn convert_builtin_id(
    file: &str,
    iface: &str,
    raw: Option<i64>,
    diags: &mut Vec<Diagnostic>,
) -> Option<u32> {
    let id = raw?;
    match u32::try_from(id) {
        Ok(v) if id >= 1 => Some(v),
        _ => {
            diags.push(Diagnostic {
                file: file.into(),
                path: format!("interface '{iface}'"),
                message: format!("builtin_id {id} is not a positive u32"),
            });
            None
        }
    }
}

fn since_of(file: &str, path: String, raw: i64, diags: &mut Vec<Diagnostic>) -> u32 {
    if (1..=u32::MAX as i64).contains(&raw) {
        raw as u32
    } else {
        diags.push(Diagnostic {
            file: file.into(),
            path,
            message: format!("since {raw} is not a valid version"),
        });
        1
    }
}

fn convert_requests(
    file: &str,
    iface: &str,
    raws: Vec<RawRequest>,
    diags: &mut Vec<Diagnostic>,
) -> Vec<Op> {
    raws.into_iter()
        .map(|r| {
            let path = format!("interface '{iface}'.request '{}'", r.name);
            let since = since_of(file, path.clone(), r.since.unwrap_or(1), diags);
            let args = convert_args(
                file,
                iface,
                &r.name,
                "request",
                r.args.unwrap_or_default(),
                diags,
            );
            Op {
                name: r.name,
                since,
                doc: r.doc.trim().to_string(),
                reply: r.reply,
                args,
            }
        })
        .collect()
}

fn convert_events(
    file: &str,
    iface: &str,
    raws: Vec<RawEvent>,
    diags: &mut Vec<Diagnostic>,
) -> Vec<Op> {
    raws.into_iter()
        .map(|e| {
            let path = format!("interface '{iface}'.event '{}'", e.name);
            let since = since_of(file, path, e.since.unwrap_or(1), diags);
            let args = convert_args(
                file,
                iface,
                &e.name,
                "event",
                e.args.unwrap_or_default(),
                diags,
            );
            Op {
                name: e.name,
                since,
                doc: e.doc.trim().to_string(),
                reply: None,
                args,
            }
        })
        .collect()
}

fn convert_args(
    file: &str,
    iface: &str,
    op: &str,
    kind: &str,
    raws: Vec<RawArg>,
    diags: &mut Vec<Diagnostic>,
) -> Vec<Arg> {
    raws.into_iter()
        .map(|a| {
            let path = format!("interface '{iface}'.{kind} '{op}' arg '{}'", a.name);
            let ty = if let Some(t) = ArgTypeSpec::parse(&a.ty) {
                t
            } else {
                diags.push(Diagnostic {
                    file: file.into(),
                    path,
                    message: format!("illegal argument type '{}'", a.ty),
                });
                ArgTypeSpec::Uint32
            };
            Arg {
                name: a.name,
                ty,
                of: a.of,
                nullable: a.nullable.unwrap_or(false),
                doc: a.doc.unwrap_or_default().trim().to_string(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r#"
[module]
name = "ldp.t"
version_min = 1
version_max = 1
doc = "Test module."

[[interface]]
name = "thing"
doc = "A thing."
[[interface.request]]
name = "poke"
doc = "Poke it."
args = [{ name = "n", type = "uint32" }]
[[interface.event]]
name = "poked"
doc = "It was poked."
args = [{ name = "state", type = "enum", of = "state" }]

[[enum]]
name = "state"
doc = "Thing state."
[enum.values]
off = 1
on = 2
"#;

    #[test]
    fn minimal_module_parses() {
        let m = parse("t", MINIMAL).expect("parses");
        assert_eq!(m.name, "ldp.t");
        assert_eq!(m.interfaces.len(), 1);
        assert_eq!(m.interfaces[0].requests[0].name, "poke");
        assert_eq!(m.interfaces[0].requests[0].args[0].ty, ArgTypeSpec::Uint32);
        assert_eq!(m.enums[0].values, vec![("off".into(), 1), ("on".into(), 2)]);
        assert!(m.imports.is_empty());
        assert_eq!(
            m.interfaces[0].events[0].args[0].of.as_deref(),
            Some("state")
        );
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let bad = MINIMAL.replace("doc = \"Poke it.\"", "doc = \"Poke it.\"\nbogus = 1");
        let diags = parse("t", &bad).unwrap_err();
        assert!(
            diags.iter().any(|d| d.message.contains("unknown field")),
            "{diags:?}"
        );
    }

    #[test]
    fn malformed_toml_is_a_diagnostic() {
        let diags = parse("t", "[module").unwrap_err();
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("TOML parse error"));
    }

    #[test]
    fn import_entries_split_by_kind() {
        let text = r#"
[module]
name = "ldp.t"
version_min = 1
version_max = 2
doc = "D."
[[import]]
enum = "ldp.core.transform"
[[import]]
interface = "ldp.core.output"
[[interface]]
name = "x"
doc = "X."
[[interface.event]]
name = "e"
doc = "E."
"#;
        let m = parse("t", text).unwrap();
        assert_eq!(m.version_max, 2);
        assert_eq!(m.imports.len(), 2);
        assert_eq!(m.imports[0].kind, ImportKind::Enum);
        assert_eq!(m.imports[1].name, "ldp.core.output");
        assert!(m.interfaces[0].requests.is_empty());
    }

    #[test]
    fn bad_values_and_indices_are_diagnostics() {
        let text = r#"
[module]
name = "ldp.t"
version_min = 1
version_max = 1
doc = "D."
[[enum]]
name = "e"
doc = "E."
[enum.values]
zero = 0
huge = 5000000000
[[bitset]]
name = "b"
doc = "B."
[bitset.bits]
neg = -1
big = 200
[[interface]]
name = "i"
doc = "I."
[[interface.event]]
name = "e2"
doc = "E."
"#;
        let diags = parse("t", text).unwrap_err();
        assert_eq!(diags.len(), 4, "{diags:?}");
        assert!(diags
            .iter()
            .all(|d| d.message.contains("is not a positive u32")
                || d.message.contains("outside 0..=127")));
    }
}
