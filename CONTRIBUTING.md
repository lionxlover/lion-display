# Contributing to LDP

Thank you for improving the Lion Display Protocol. This project has strict
engineering rules because it sits below every application on LionOS: a bug here
is a bug everywhere.

## Ground rules

1. **No placeholders.** Never merge stub functions, `unimplemented!()`,
   `todo!()`, or features hidden behind silent TODO comments. If something is
   out of scope, cut scope explicitly in the PR description and the roadmap.
   Missing functionality must be *visible* in `docs/roadmap.md`.
2. **Every module compiles and every public interface is tested.** CI enforces
   `cargo clippy --all-targets -- -D warnings` and `cargo test`.
3. **Safe Rust by default.** `unsafe` is allowed only where technically
   necessary (syscall wrappers, DMA-BUF/driver interop, SIMD). Every `unsafe`
   block carries a `// SAFETY:` comment stating the invariant. `ldp-core`,
   `ldp-protocol`, `ldp-shell`, `ldp-color` stay `#![forbid(unsafe_code)]`
   permanently.
4. **Small focused modules.** Target 200–400 lines per source file. Split by
   responsibility, never create grab-bag modules. One subsystem = one crate.
5. **Minimize dependencies.** Every new crate dependency requires justification
   in the PR. Prefer std, then mature widely-audited libraries; load native
   system libraries (EGL, DRM, xkbcommon) lazily via `dlopen` at runtime so
   headless CI never needs GPU drivers. No frameworks, no GTK/Qt in the core.
6. **The protocol core is toolkit-independent.** LTK and any UI toolkit live
   strictly above `ldp-client`. Nothing in `crates/` may depend on a UI toolkit.
7. **Docs are part of the API.** Every public item carries rustdoc with an
   example or a precise contract. Protocol changes update `spec/*.toml` and
   are regenerated via `./scripts/gen-protocol.sh` (or `cargo run -p ldpc --
   gen`) in the same PR; CI rejects drift between the spec and the committed
   generated code.
8. **Correctness and security first, optimization second.** Performance PRs
   must come with before/after numbers from the benchmark harness.

## Workflow

1. Fork / branch from `main`; one logical change per branch.
2. `./scripts/verify.sh` must pass locally before pushing.
3. PR description: what + why + how tested. Protocol changes must include spec
   diffs and the regenerated tables/blob/reference docs (run
   `./scripts/gen-protocol.sh`), plus conformance-test updates.
4. Review requires: code read by one maintainer; protocol/security changes
   need two (one must know the threat model in `docs/threat-model.md`).

## Code style

- Formatting: `cargo fmt` (config in `rustfmt.toml`). No manual formatting.
- Naming follows Rust API guidelines (`UpperCamelCase` types, `snake_case` fns).
- Error handling: return `Result` with the crate's error type; `panic!` is
  reserved for internal invariant violations (never on untrusted input).
- Tests live beside the code (`#[cfg(test)]`) plus integration tests in each
  crate's `tests/`; cross-crate suites go to the workspace `tests/` directory.
- Commits: imperative subject ≤72 chars, body explains rationale.

## Licensing

By contributing you agree your work is dual-licensed under MIT and
Apache-2.0 (see root `LICENSE-*`).
