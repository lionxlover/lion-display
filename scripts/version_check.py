#!/usr/bin/env python3
"""Release-coherence guard (Phase 23, executable).

One version for the whole release, everywhere it is stated:

  * the workspace version in the root ``Cargo.toml``,
  * every member crate (``version.workspace`` inheritance, or an
    explicitly equal version),
  * the ``Cargo.lock`` entries of the in-tree packages,
  * the top ``debian/changelog`` entry (the base of any ``~phase``
    pre-release suffix must match),
  * the README milestone status line,
  * the CHANGELOG milestone heading,
  * the showcase scene's version mark.

A release can no longer be half-bumped: bumping ``Cargo.toml`` alone
would leave the debian changelog, the docs, or the scene mark behind,
and this gate fails the build naming every drifted file. Run after any
cargo command so the lockfile is fresh (``scripts/verify.sh`` runs it
last, after the drift gate's cargo invocation).

Exit status: 0 = coherent, 1 = drift (files named on stderr).
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# Workspace members: every ldp-* crate, the compositor binary, the
# tools, examples, and the test/fuzz harnesses (mirrors dep_policy.py).
INTERNAL_PREFIXES = (
    "ldp",
    "lion-",
    "hello-",
    "pointer-",
    "clip-",
    "showcase",
)


def workspace_version() -> str:
    """The single-point release version (``[workspace.package]``)."""
    text = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    block = re.search(r"\[workspace\.package\]\n(.*?)(?=\n\[|\Z)", text, re.S)
    if block is None:
        print("version check: [workspace.package] missing", file=sys.stderr)
        sys.exit(1)
    found = re.search(r"^version\s*=\s*\"([^\"]+)\"", block.group(1), re.M)
    if found is None:
        print("version check: workspace version missing", file=sys.stderr)
        sys.exit(1)
    return found.group(1)


def resolve_members() -> list[Path]:
    """Expand the workspace member globs exactly as cargo does."""
    text = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    members_block = re.search(r"members\s*=\s*\[(.*?)\]", text, re.S)
    if members_block is None:
        print("version check: members list missing", file=sys.stderr)
        sys.exit(1)
    members: list[Path] = []
    for entry in re.findall(r"\"([^\"]+)\"", members_block.group(1)):
        if entry.endswith("/*"):
            base = ROOT / entry[:-2]
            members.extend(
                sorted(p for p in base.iterdir() if (p / "Cargo.toml").is_file())
            )
        elif (ROOT / entry / "Cargo.toml").is_file():
            members.append(ROOT / entry)
    return members


def check_members(version: str, failures: list[str]) -> None:
    for member in resolve_members():
        rel = member.relative_to(ROOT)
        text = (member / "Cargo.toml").read_text(encoding="utf-8")
        if re.search(r"^version\.workspace\s*=\s*true\s*$", text, re.M) is not None:
            continue
        found = re.search(r"^version\s*=\s*\"([^\"]+)\"", text, re.M)
        if found is None or found.group(1) != version:
            failures.append(
                f"{rel}: neither version.workspace inheritance "
                f"nor an explicit version == {version}"
            )


def check_lock(version: str, failures: list[str]) -> None:
    lock = ROOT / "Cargo.lock"
    if not lock.is_file():
        failures.append("Cargo.lock: missing")
        return
    text = lock.read_text(encoding="utf-8")
    pinned: dict[str, str] = {}
    for stanza in text.split("[[package]]")[1:]:
        name = re.search(r"^name\s*=\s*\"([^\"]+)\"", stanza, re.M)
        ver = re.search(r"^version\s*=\s*\"([^\"]+)\"", stanza, re.M)
        if name and ver:
            pinned[name.group(1)] = ver.group(1)
    internal = {n: v for n, v in pinned.items() if n.startswith(INTERNAL_PREFIXES)}
    if not internal:
        failures.append("Cargo.lock: no in-tree packages found (stale lock?)")
        return
    for name, ver in sorted(internal.items()):
        if ver != version:
            failures.append(f"Cargo.lock: {name} pinned at {ver} (workspace {version})")


def check_debian(version: str, failures: list[str]) -> None:
    text = (ROOT / "debian/changelog").read_text(encoding="utf-8")
    found = re.search(r"^lion-display \(([^)]+)\)", text, re.M)
    if found is None:
        failures.append("debian/changelog: no versioned entry")
        return
    entry = found.group(1)
    base = entry.split(":")[-1].split("~")[0].split("+")[0]
    if base != version:
        failures.append(
            f"debian/changelog: top entry {entry} (base {base}) != workspace {version}"
        )


def check_docs(version: str, failures: list[str]) -> None:
    readme = (ROOT / "README.md").read_text(encoding="utf-8")
    if f"v{version} milestone" not in readme:
        failures.append(f"README.md: status line lacks 'v{version} milestone'")
    changelog = (ROOT / "CHANGELOG.md").read_text(encoding="utf-8")
    if f"## [{version}]" not in changelog:
        failures.append(f"CHANGELOG.md: milestone heading '## [{version}]' missing")
    scene = (ROOT / "examples/showcase/src/lib.rs").read_text(encoding="utf-8")
    if f'"v{version}"' not in scene:
        failures.append("examples/showcase/src/lib.rs: scene version mark drifted")


def main() -> int:
    version = workspace_version()
    failures: list[str] = []
    check_members(version, failures)
    check_lock(version, failures)
    check_debian(version, failures)
    check_docs(version, failures)
    if failures:
        print(f"release coherence BREAK (workspace {version}):")
        for failure in failures:
            print(f"  - {failure}")
        return 1
    print(
        f"release coherence OK: workspace {version} uniform across "
        "members, lock, debian, README, CHANGELOG, scene"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
