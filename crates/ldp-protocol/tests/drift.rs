//! Drift gate (roadmap Phase 2 EC): recompile `spec/` with `ldpc` and
//! byte-compare the committed generated output. The spec is the source of
//! truth; if this test fails, run `cargo run -p ldpc -- gen` and commit.

use std::path::Path;

fn compile_specs() -> ldpc::SpecSet {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec");
    ldpc::compile_dir(&dir).unwrap_or_else(|diags| {
        panic!("repository specs must compile: {diags:?}");
    })
}

fn committed(name: &str) -> &'static str {
    match name {
        "mod.rs" => include_str!("../src/generated/mod.rs"),
        "a11y.rs" => include_str!("../src/generated/a11y.rs"),
        "color.rs" => include_str!("../src/generated/color.rs"),
        "core.rs" => include_str!("../src/generated/core.rs"),
        "data.rs" => include_str!("../src/generated/data.rs"),
        "input.rs" => include_str!("../src/generated/input.rs"),
        "security.rs" => include_str!("../src/generated/security.rs"),
        "session.rs" => include_str!("../src/generated/session.rs"),
        "shell.rs" => include_str!("../src/generated/shell.rs"),
        "capture.rs" => include_str!("../src/generated/capture.rs"),
        other => panic!("unexpected generated file {other}"),
    }
}

#[test]
fn committed_rust_tables_match_ldpc_output() {
    let set = compile_specs();
    let files = set.rust_files();
    assert_eq!(
        files.len(),
        10,
        "mod.rs + nine module files (capture joined in Phase 22)"
    );
    for (name, content) in &files {
        assert_eq!(
            committed(name),
            content,
            "{name} drifted from spec/ — regenerate with `cargo run -p ldpc -- gen`"
        );
    }
}

#[test]
fn committed_blob_matches_ldpc_output() {
    let set = compile_specs();
    let blob: &[u8] = include_bytes!("../src/generated/blob.bin");
    assert_eq!(
        blob,
        set.blob().as_slice(),
        "blob.bin drifted from spec/ — regenerate with `cargo run -p ldpc -- gen`"
    );
}

#[test]
fn blob_and_tables_describe_the_same_protocol() {
    // The blob parses into structures that must equal the static tables.
    let parsed = ldp_protocol::blob::parse(ldp_protocol::BLOB).unwrap();
    let from_statics: Vec<ldp_protocol::blob::OwnedModule> =
        ldp_protocol::MODULES.iter().map(|&m| m.into()).collect();
    assert_eq!(parsed, from_statics);
}

#[test]
fn schema_json_serving_is_deferred_by_design() {
    // `registry.schema` replies with a JSON string rendered from these
    // tables (Phase 4, ldp-server). Phase 2's contract is the data + the
    // blob; this test pins the decision so it is not silently forgotten.
    // The Phase 4 amendment added `connection.get_registry` (the registry
    // bootstrap path): 90 requests + 120 events across 33 interfaces.
    // Phase 22's capture module added one interface, one request, and
    // two events: 91 requests + 122 events across 34 interfaces.
    // Phase 45's material amendment added one toplevel request
    // (`set_material`, appended after the frozen opcodes — purely
    // additive on the wire): 92 requests + 122 events across 34
    // interfaces. Phase 47's semantic-scene amendment added the
    // toplevel triple set_semantic_role/set_security_class/
    // set_scene_profile (same additive doctrine): 95 requests + 122
    // events across 34 interfaces. Phase 50's operator's-hand
    // amendment added toplevel start_move/start_resize (same
    // additive doctrine, the resize_edge enum growing the set to
    // 37): 97 requests + 122 events across 34 interfaces. Phase 51's
    // view-switch amendment added the shell pair
    // switch_workspace/workspace_switched (the operator-side space
    // switching the ledger named — same additive doctrine, on the
    // shell global): 98 requests + 123 events across 34 interfaces.
    let set = compile_specs();
    let c = set.counts();
    assert_eq!(c.interfaces, 34);
    assert_eq!(c.requests + c.events, 98 + 123);
}

#[test]
fn frozen_surface_matches_the_compiled_spec() {
    // The stability contract's evidence (Phase 31): the committed
    // spec/surface-v1.json is byte-identical to the live compiled
    // surface — every module, interface, operation, opcode, since,
    // reply, and argument. A moved name breaks here exactly as it
    // breaks `ldpc snapshot --check` in verify.sh; additions surface
    // as conscious re-freezes, never silent drift.
    let set = compile_specs();
    let live = set.render_surface();
    let frozen = std::fs::read("../../spec/surface-v1.json").expect("the frozen surface");
    assert_eq!(
        live.as_bytes(),
        frozen.as_slice(),
        "the frozen v1 surface drifted from the compiled spec"
    );
}

#[test]
fn the_frozen_surface_covers_every_compiled_interface() {
    // The freeze is COMPLETE: every interface the registry serves
    // appears in the frozen surface (no interface can slip around
    // the contract).
    let set = compile_specs();
    let surface = set.render_surface();
    for module in &set.modules {
        for iface in &module.interfaces {
            let key = format!("\"{}\", \"global\"", iface.name);
            assert!(
                surface.contains(&key),
                "interface {} missing from the frozen surface",
                iface.name
            );
        }
    }
}
