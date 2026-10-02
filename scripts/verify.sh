#!/usr/bin/env bash
# verify.sh — the local gate mirroring CI. Run before every push.
#
# Steps: formatting, lint (clippy -D warnings), tests (including the
# spec/↔generated-code drift gate), docs, spec lint, drift check.
# Any failure aborts with a non-zero exit code.

set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

echo "==> cargo fmt --check"
cargo fmt --all --check

echo "==> cargo clippy"
cargo clippy --all-targets --all-features -- -D warnings

echo "==> cargo test (unit + integration + drift gate)"
cargo test --all --all-features

echo "==> cargo doc"
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --document-private-items

echo "==> spec lint"
python3 scripts/spec_lint.py

echo "==> dependency policy"
python3 scripts/dep_policy.py

echo "==> ldpc drift check"
cargo run -q -p ldpc -- gen --check
# The API stability contract's freeze gate (Phase 31): the frozen
# v1 surface must match the compiled spec byte-for-byte — a moved
# name, opcode, argument, or version breaks the build until the
# change is consciously re-frozen (`cargo run -p ldpc -- snapshot`).
cargo run -q -p ldpc -- snapshot --check

echo "==> release coherence"
python3 scripts/version_check.py

echo "==> OK: all verification gates passed"
