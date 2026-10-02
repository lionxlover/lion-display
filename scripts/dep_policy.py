#!/usr/bin/env python3
"""Dependency policy guard (CONTRIBUTING.md rule 5, executable).

The runtime dependency policy: the shipped crates depend only on
in-tree LDP crates plus `libc` (the audited Linux ABI seam), and the
tools may additionally use `ldpc`'s spec front-end (toml/serde). This
script reads Cargo.lock and fails on any other entry, so a stray
`cargo add` can never ride into a release unnoticed.

Exit status: 0 = policy holds, 1 = policy break (names printed).
"""

from __future__ import annotations

import sys
from pathlib import Path

# The audited external set: libc (the syscall seam), the toml/serde
# front-end ldpc compiles spec/ with (tools only, never shipped
# runtime), and their transitive closure as of the lockfile.
ALLOWED = {
    "libc",
    "toml",
    "toml_datetime",
    "toml_edit",
    "toml_parser",
    "toml_writer",
    "serde",
    "serde_core",
    "serde_derive",
    "serde_spanned",
    "winnow",
    "indexmap",
    "hashbrown",
    "equivalent",
    "allocator-api2",
    "proc-macro2",
    "quote",
    "syn",
    "unicode-ident",
}

# Workspace members: every ldp-* crate, the compositor binary, the
# tools, examples, and the test/fuzz harnesses.
INTERNAL_PREFIXES = (
    "ldp",
    "lion-",
    "hello-",
    "pointer-",
    "clip-",
    "showcase",
)


def lockfile_names(path: Path) -> set[str]:
    names: set[str] = set()
    for line in path.read_text(encoding="utf-8").splitlines():
        if line.startswith("name = "):
            names.add(line.split('"')[1])
    return names


def main() -> int:
    root = Path(__file__).resolve().parent.parent
    lock = root / "Cargo.lock"
    if not lock.is_file():
        print(f"dep policy: {lock} not found", file=sys.stderr)
        return 1
    names = lockfile_names(lock)
    internal = {n for n in names if n.startswith(INTERNAL_PREFIXES)}
    unexpected = sorted(names - ALLOWED - internal)
    if unexpected:
        print("dependency policy BREAK: unexpected Cargo.lock entries:")
        for name in unexpected:
            print(f"  - {name}")
        print("(see CONTRIBUTING.md rule 5; extend ALLOWED only by deliberate policy)")
        return 1
    print(
        f"dependency policy OK: {len(names)} lockfile entries, "
        f"{len(internal)} in-tree, {len(names) - len(internal)} allowed external"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
