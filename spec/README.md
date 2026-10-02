# LDP Protocol Specifications

The `spec/` directory is the **source of truth** for the LDP wire protocol.
Every interface, request, event, enum, and bitset is declared here in TOML
(grammar: `../docs/spec-format.md`) and compiled into type-safe Rust code by
`ldpc` (tools/ldpc — implemented in Phase 2). Tools and debuggers consume the
same files via the runtime introspection path, so the protocol never exists
twice. A human-diffable reference rendering of these files is generated into
`../docs/reference/`.

## Modules

v1 declared eight modules; Phase 22 (post-v0.1.0) grew the ninth —
`ldp.capture` — through the same pipeline, proving the spec-first
doctrine absorbs protocol growth. Post-freeze growth is additive by
discipline: Phase 45 appended `toplevel.set_material` and Phase 47
appended the semantic-scene triple (`set_semantic_role`,
`set_security_class`, `set_scene_profile`) *after* the frozen
opcodes — no existing opcode moves, the freeze is re-taken
consciously each time. Phase 50 appended the operator's hand the
same way (`toplevel.start_move`/`start_resize`, the `resize_edge`
enum growing the set to 37): 97 requests + 122 events today.

| File | Module | Scope |
|---|---|---|
| `core.toml` | `ldp.core` | connection, registry, output, compositor, surface, subsurface, buffer, shm, shm_pool, dmabuf, fence |
| `input.toml` | `ldp.input` | seat, pointer, keyboard, touch, tablet, gestures |
| `shell.toml` | `ldp.shell` | shell, toplevel, popup, dialog, spaces |
| `data.toml` | `ldp.data` | data_device_manager, data_device, data_source, data_offer |
| `color.toml` | `ldp.color` | color_manager, color_profile (ICC import) |
| `security.toml` | `ldp.security` | security (scopes, tokens), audit |
| `a11y.toml` | `ldp.a11y` | accessibility settings + provider bus, magnifier |
| `session.toml` | `ldp.session` | session lifecycle, inhibitors, display_config |
| `capture.toml` | `ldp.capture` | capture_manager (output frame snapshots; Phase 22) |

## Versioning policy

* Each module carries an independent wire version range `[version_min, version_max]`.
* Opcode numbers, argument lists, enum values, and bit assignments are **frozen
  once released**. Evolution appends new operations with `since` guards and new
  enum/bit values (unknown values must be ignored by receivers).
* Crate SemVer tracks the implementation; module versions track wire
  compatibility. The two never mix.
* Cross-module references (interface / enum / bitset) use fully qualified
  names via `[[import]]` and are validated by the linter.

## Checking & regenerating

```sh
python3 ../scripts/spec_lint.py        # structural pre-check (fast, runs in CI)
cargo run -p ldpc -- check             # full semantic validation
cargo run -p ldpc -- gen               # regenerate Rust tables + blob + reference docs
```

Generated output lives in `../crates/ldp-protocol/src/generated/` and
`../docs/reference/` and is committed; `cargo test` and CI fail on drift
between the spec and the committed output. Edit the TOML, regenerate,
commit both — never hand-edit generated files.
