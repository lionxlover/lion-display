#!/usr/bin/env bash
# gen-protocol.sh — regenerate everything derived from spec/*.toml.
#
# Writes crates/ldp-protocol/src/generated/ (Rust tables + blob.bin) and
# docs/reference/ (markdown). Unchanged files are left untouched, so
# incremental builds stay fast. Commit the result together with the spec
# change that motivated it.

set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

cargo run -q -p ldpc -- gen "$@"
