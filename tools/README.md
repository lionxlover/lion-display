# tools

## ldpc — the LDP protocol compiler

`ldpc` compiles the protocol's source of truth (`../spec/*.toml`, grammar:
`../docs/spec-format.md`) into everything the runtime and tooling need:

* **Rust schema tables** → `../crates/ldp-protocol/src/generated/`
  (static `ModuleSchema` tables, typed `#[non_exhaustive]` enums, bitset
  const modules, opcode constants, rustdoc from the spec `doc` fields),
* **introspection blob** → the `LDPS` binary format (`blob.bin`), the spec
  re-serialized for runtime tooling and the `registry.schema` service,
* **reference docs** → `../docs/reference/<module>.md`.

Pipeline: parse (serde, `deny_unknown_fields`) → validate (every rule of
spec-format §2, including cross-module import resolution and global
FQ-name uniqueness) → generate. Output is byte-stable, committed, and
drift-checked in CI: `cargo test` recompiles the spec and byte-compares.

```sh
cargo run -p ldpc -- check              # validate the spec set
cargo run -p ldpc -- gen                # regenerate all outputs
cargo run -p ldpc -- gen --check        # CI drift gate (exit 1 on drift)
```

Dependency note: `ldpc` uses `toml`/`serde` — a *tooling-only* dependency
justified under CONTRIBUTING.md rule 5; no runtime crate links it, and
ordinary builds of `ldp-protocol` never invoke the compiler.

Later phases add `ldp-tools` utilities here (ldp-info, ldp-debug,
ldp-validate, ldp-profiler, ldp-audit, ldp-input-debug — Phase 18).
