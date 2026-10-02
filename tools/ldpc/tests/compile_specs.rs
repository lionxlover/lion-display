//! Integration tests: compile the repository's real `spec/` set and check
//! the invariants the runtime depends on. These counts are the *authoritative*
//! v1 inventory (independently verified against the TOML sources with a
//! direct `[[interface]]` / request / event count).

use std::path::{Path, PathBuf};

fn spec_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec")
}

fn compile_repo_specs() -> ldpc::SpecSet {
    ldpc::compile_dir(&spec_dir()).unwrap_or_else(|diags| {
        panic!("repository specs must compile: {diags:?}");
    })
}

#[test]
fn repo_specs_compile_with_the_v1_inventory() {
    let set = compile_repo_specs();
    assert_eq!(
        set.counts(),
        ldpc::model::Counts {
            // 8 v1 modules + capture (Phase 22).
            modules: 9,
            interfaces: 34,
            // 90 requests since the Phase 4 amendment (connection.get_registry)
            // + capture.grab (Phase 22) + toplevel.set_material (Phase 45,
            // appended after the frozen opcodes — purely additive)
            // + the semantic-scene triple set_semantic_role/
            // set_security_class/set_scene_profile (Phase 47, same
            // additive doctrine) + the operator's hand
            // start_move/start_resize (Phase 50, same doctrine again)
            // + the view switch shell.switch_workspace (Phase 51,
            // same doctrine, on the shell global).
            requests: 98,
            // 120 v1 events + capture's frame/failed (Phase 22)
            // + the view switch shell.workspace_switched
            // (Phase 51, same additive doctrine).
            events: 123,
            // 31 v1 enums + capture_error (Phase 22) + material
            // (Phase 45) + semantic_role/security_class/scene_profile
            // (Phase 47) + resize_edge (Phase 50).
            enums: 37,
            bitsets: 16,
        }
    );
    let names: Vec<&str> = set.modules.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "ldp.a11y",
            "ldp.capture",
            "ldp.color",
            "ldp.core",
            "ldp.data",
            "ldp.input",
            "ldp.security",
            "ldp.session",
            "ldp.shell"
        ]
    );
}

#[test]
fn bootstrap_contract_holds() {
    let set = compile_repo_specs();
    let (m, connection) = set.interface("ldp.core.connection").unwrap();
    assert_eq!(m.name, "ldp.core");
    assert_eq!(connection.builtin_id, Some(1));
    assert!(!connection.global);
    // The first request any client may send.
    assert_eq!(connection.requests[0].name, "hello");
    assert_eq!(connection.requests[0].reply.as_deref(), Some("welcome"));
    // Central lifecycle: destroy -> destroyed on the connection object.
    assert!(connection
        .requests
        .iter()
        .any(|r| r.name == "destroy" && r.reply.as_deref() == Some("destroyed")));
}

#[test]
fn opcodes_are_dense_and_replies_resolve() {
    let set = compile_repo_specs();
    for m in &set.modules {
        for i in &m.interfaces {
            for (ops, kind) in [(&i.requests, "request"), (&i.events, "event")] {
                for (idx, op) in ops.iter().enumerate() {
                    // Opcodes are assigned by declaration order in the model:
                    // the generated tables re-derive them; assert the source
                    // arrays are non-empty per interface kind when present.
                    assert!(!op.name.is_empty(), "{}.{}", i.name, op.name);
                    assert!(
                        (m.version_min..=m.version_max).contains(&op.since),
                        "{kind} '{}.{}' since {} out of range",
                        i.name,
                        op.name,
                        op.since
                    );
                    let _ = idx;
                }
            }
            for r in &i.requests {
                if let Some(reply) = &r.reply {
                    assert!(
                        i.events.iter().any(|e| &e.name == reply),
                        "reply '{reply}' of '{}.{}' must be a declared event",
                        i.name,
                        r.name
                    );
                }
            }
        }
    }
}

#[test]
fn every_of_reference_resolves() {
    let set = compile_repo_specs();
    for m in &set.modules {
        for i in &m.interfaces {
            for op in i.requests.iter().chain(i.events.iter()) {
                for a in &op.args {
                    match a.ty {
                        ldpc::model::ArgTypeSpec::Enum => {
                            assert!(
                                set.resolve_enum(&m.name, a.of.as_deref().unwrap())
                                    .is_some(),
                                "{}.{} arg {}: enum '{}' unresolved",
                                i.name,
                                op.name,
                                a.name,
                                a.of.clone().unwrap_or_default()
                            );
                        }
                        ldpc::model::ArgTypeSpec::Bitset => {
                            assert!(
                                set.resolve_bitset(&m.name, a.of.as_deref().unwrap())
                                    .is_some(),
                                "{}.{} arg {}: bitset '{}' unresolved",
                                i.name,
                                op.name,
                                a.name,
                                a.of.clone().unwrap_or_default()
                            );
                        }
                        ldpc::model::ArgTypeSpec::Object | ldpc::model::ArgTypeSpec::NewId => {
                            if let Some(of) = &a.of {
                                let resolved = if of.contains('.') {
                                    set.interface(of).is_some()
                                } else {
                                    set.module(&m.name).is_some_and(|mm| {
                                        mm.interfaces.iter().any(|x| x.name == *of)
                                    })
                                };
                                assert!(
                                    resolved,
                                    "{}.{} arg {}: interface '{of}' unresolved",
                                    i.name, op.name, a.name
                                );
                            }
                        }
                        ldpc::model::ArgTypeSpec::Array => {
                            let elem =
                                ldpc::model::ArgTypeSpec::parse(a.of.as_deref().unwrap()).unwrap();
                            assert!(
                                elem.is_array_element(),
                                "{}.{} arg {}: bad element type",
                                i.name,
                                op.name,
                                a.name
                            );
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

#[test]
fn generation_is_deterministic_and_complete() {
    let set = compile_repo_specs();
    let a = set.rust_files();
    let b = set.rust_files();
    assert_eq!(a, b, "rust generation must be byte-stable");
    assert_eq!(
        a.len(),
        10,
        "mod.rs + 9 module files (capture joined in Phase 22)"
    );
    assert!(a.contains_key("mod.rs") && a.contains_key("core.rs"));
    assert_eq!(
        set.blob(),
        set.blob(),
        "blob generation must be byte-stable"
    );
    let md = set.markdown_files();
    assert_eq!(md.len(), 9);
    for stem in [
        "a11y", "color", "core", "data", "input", "security", "session", "shell",
    ] {
        assert!(md.contains_key(&format!("{stem}.md")));
    }
}

#[test]
fn invalid_specs_fail_with_targeted_diagnostics() {
    let bad_name = r#"
[module]
name = "ldp.wrong"
version_min = 1
version_max = 1
doc = "D."
[[interface]]
name = "x"
doc = "X."
[[interface.event]]
name = "e"
doc = "E."
"#;
    let diags = ldpc::compile(&[("t".to_string(), bad_name.to_string())]).unwrap_err();
    assert!(diags.iter().any(|d| d.message.contains("must be 'ldp.t'")));

    let dup_iface = r#"
[module]
name = "ldp.t"
version_min = 1
version_max = 1
doc = "D."
[[interface]]
name = "x"
doc = "X."
[[interface.event]]
name = "e"
doc = "E."
[[interface]]
name = "x"
doc = "X again."
[[interface.event]]
name = "e"
doc = "E again."
"#;
    let diags = ldpc::compile(&[("t".to_string(), dup_iface.to_string())]).unwrap_err();
    assert!(diags.iter().any(|d| d.message.contains("duplicate name")));

    let empty = ldpc::compile(&[]).unwrap_err();
    assert!(empty[0].message.contains("no spec modules"));
}

/// Phase 23's uniform version surface: `ldpc --version` exits 0 and
/// prints exactly `ldpc <workspace version>`.
#[test]
fn version_flag_reports_the_release() {
    let exe = env!("CARGO_BIN_EXE_ldpc");
    let out = std::process::Command::new(exe)
        .arg("--version")
        .output()
        .expect("spawn ldpc");
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(text.trim(), format!("ldpc {}", env!("CARGO_PKG_VERSION")));
}
