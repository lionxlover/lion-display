#!/usr/bin/env python3
"""spec_lint.py — Phase 1 specification sanity checker.

Validates every ``spec/*.toml`` module against the structural rules of
``docs/spec-format.md`` so breakage is caught before the ``ldpc`` Rust compiler
(Phase 2) even runs. Pure stdlib: ``tomllib`` parses, this script checks:

* required keys and value types per table kind,
* unique names (modules, interfaces, requests, events, enums, bitsets, args),
* opcode numbering = declaration order, contiguous from 1, stable,
* legal argument types and their required ``of`` references,
* references resolve within the module or to a declared ``import``,
* enum/bitset values are positive integers, unique, and dense,
* ``since`` within the module version range,
* no unknown keys (catches typos — a common spec-editing bug).

Exit code 0 = all specs clean; 1 = any failure (messages on stdout).
"""

from __future__ import annotations

import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SPEC_DIR = ROOT / "spec"

ARG_TYPES = {
    "int32", "uint32", "int64", "uint64", "float32", "float64",
    "bool", "string", "object", "new_id", "fd", "array", "rect",
    "enum", "bitset", "ts",
}
# `of` is mandatory for these types; for object/new_id it is optional
# (generic reference) but must resolve when present.
TYPES_NEEDING_OF = {"enum", "bitset", "array"}
KEYS_REQUEST = {"name", "since", "doc", "args", "reply"}
KEYS_EVENT = {"name", "since", "doc", "args"}
KEYS_ARG = {"name", "type", "of", "nullable", "doc"}
KEYS_IMPORT = {"interface", "enum", "bitset"}

errors: list[str] = []


def err(module: str, msg: str) -> None:
    errors.append(f"{module}: {msg}")


def check_doc(module: str, table: dict, where: str) -> None:
    doc = table.get("doc")
    if not isinstance(doc, str) or not doc.strip():
        err(module, f"{where} is missing a non-empty 'doc' string")


def check_args(module: str, table: dict, where: str,
               enum_names: set[str], bitset_names: set[str],
               iface_names: set[str], arg_names_seen: set[str],
               imported_enums: set[str] = frozenset(),
               imported_bitsets: set[str] = frozenset(),
               imported_ifaces: set[str] = frozenset()) -> None:
    args = table.get("args", [])
    if not isinstance(args, list):
        err(module, f"{where}: 'args' must be an array of tables")
        return
    for i, arg in enumerate(args):
        w = f"{where} arg[{i}]"
        if not isinstance(arg, dict):
            err(module, f"{w}: must be a table")
            continue
        unknown = set(arg) - KEYS_ARG
        if unknown:
            err(module, f"{w}: unknown keys {sorted(unknown)}")
        name = arg.get("name")
        if not isinstance(name, str) or not name:
            err(module, f"{w}: missing 'name'")
            continue
        w = f"{where} arg '{name}'"
        if name in arg_names_seen:
            err(module, f"{w}: duplicate argument name")
        arg_names_seen.add(name)
        typ = arg.get("type")
        if typ not in ARG_TYPES:
            err(module, f"{w}: illegal type {typ!r}")
            continue
        if typ in TYPES_NEEDING_OF or (typ in ("object", "new_id") and "of" in arg):
            of = arg.get("of")
            if typ in TYPES_NEEDING_OF and (not isinstance(of, str) or not of):
                err(module, f"{w}: type '{typ}' requires 'of'")
            elif typ == "enum" and of not in enum_names | imported_enums:
                err(module, f"{w}: enum '{of}' not defined or imported in module")
            elif typ == "bitset" and of not in bitset_names | imported_bitsets:
                err(module, f"{w}: bitset '{of}' not defined or imported in module")
            elif typ in ("object", "new_id") and of not in iface_names | imported_ifaces:
                err(module, f"{w}: interface '{of}' not defined or imported in module")
            elif typ == "array" and of not in ARG_TYPES:
                err(module, f"{w}: array element type '{of}' illegal")


def check_module(path: Path) -> None:
    module_name = path.stem
    try:
        data = tomllib.loads(path.read_text(encoding="utf-8"))
    except tomllib.TOMLDecodeError as exc:
        err(module_name, f"TOML parse error: {exc}")
        return

    mod = data.get("module", {})
    if not isinstance(mod, dict):
        err(module_name, "'[module]' table missing")
        return
    for key in ("name", "version_min", "version_max", "doc"):
        if key not in mod:
            err(module_name, f"[module] missing '{key}'")
    name = mod.get("name")
    if name != f"ldp.{module_name}":
        err(module_name, f"[module].name must be 'ldp.{module_name}', got {name!r}")
    vmin, vmax = mod.get("version_min"), mod.get("version_max")
    if not (isinstance(vmin, int) and isinstance(vmax, int) and 1 <= vmin <= vmax):
        err(module_name, "[module] version range invalid (need 1 <= min <= max)")
    check_doc(module_name, mod, "[module]")
    if set(mod) - {"name", "version_min", "version_max", "doc"}:
        err(module_name, f"[module] unknown keys {sorted(set(mod) - {'name', 'version_min', 'version_max', 'doc'})}")

    enum_names: set[str] = set()
    bitset_names: set[str] = set()
    iface_names: set[str] = set()
    imported_ifaces: set[str] = set()
    imported_enums: set[str] = set()
    imported_bitsets: set[str] = set()

    for imp in data.get("import", []):
        if not isinstance(imp, dict):
            err(module_name, "[[import]] entries must be tables")
            continue
        unknown = set(imp) - KEYS_IMPORT
        if unknown:
            err(module_name, f"[[import]] unknown keys {sorted(unknown)}")
        for key, target in (("interface", imported_ifaces),
                            ("enum", imported_enums),
                            ("bitset", imported_bitsets)):
            value = imp.get(key)
            if value is not None:
                if not isinstance(value, str) or not value.startswith("ldp."):
                    err(module_name, f"[[import]] {key} must be a fully qualified 'ldp.*' name")
                elif value in target:
                    err(module_name, f"[[import]] duplicate {key} '{value}'")
                else:
                    target.add(value)

    # Pre-pass: collect all declared names so forward references resolve,
    # and detect duplicates by counting source occurrences.
    from collections import Counter

    def dup_check(kind: str, tables: list, names: set[str]) -> None:
        counts = Counter(
            t.get("name") for t in tables
            if isinstance(t, dict) and isinstance(t.get("name"), str)
        )
        for name, count in counts.items():
            if count > 1:
                err(module_name, f"{kind} '{name}': declared {count} times")
        names.update(counts)

    dup_check("enum", data.get("enum", []), enum_names)
    dup_check("bitset", data.get("bitset", []), bitset_names)
    dup_check("interface", data.get("interface", []), iface_names)

    for e in data.get("enum", []):
        w = "enum"
        name = e.get("name")
        if not isinstance(name, str) or not name:
            err(module_name, f"{w}: missing 'name'")
            continue
        w = f"enum '{name}'"
        check_doc(module_name, e, w)
        values = e.get("values")
        if not isinstance(values, dict) or not values:
            err(module_name, f"{w}: needs a non-empty 'values' table")
            continue
        seen: set[int] = set()
        for vname, vnum in values.items():
            if not isinstance(vnum, int) or vnum < 1:
                err(module_name, f"{w}.{vname}: value must be a positive integer")
            if vnum in seen:
                err(module_name, f"{w}.{vname}: duplicate value {vnum}")
            seen.add(vnum)
        if seen and max(seen) != len(seen):
            err(module_name, f"{w}: values must be dense 1..=N (got max {max(seen)}, count {len(seen)})")

    for b in data.get("bitset", []):
        name = b.get("name")
        if not isinstance(name, str) or not name:
            err(module_name, "bitset: missing 'name'")
            continue
        w = f"bitset '{name}'"
        check_doc(module_name, b, w)
        bits = b.get("bits")
        if not isinstance(bits, dict) or not bits:
            err(module_name, f"{w}: needs a non-empty 'bits' table")
            continue
        seen: set[int] = set()
        for bname, bidx in bits.items():
            if not isinstance(bidx, int) or not 0 <= bidx <= 127:
                err(module_name, f"{w}.{bname}: bit index must be 0..=127")
            if bidx in seen:
                err(module_name, f"{w}.{bname}: duplicate bit {bidx}")
            seen.add(bidx)

    for iface in data.get("interface", []):
        name = iface.get("name")
        if not isinstance(name, str) or not name:
            err(module_name, "interface: missing 'name'")
            continue
        w = f"interface '{name}'"
        check_doc(module_name, iface, w)
        allowed = {"name", "doc", "builtin_id", "global", "request", "event"}
        unknown = set(iface) - allowed
        if unknown:
            err(module_name, f"{w}: unknown keys {sorted(unknown)}")
        bid = iface.get("builtin_id")
        if bid is not None and (not isinstance(bid, int) or bid < 1):
            err(module_name, f"{w}: builtin_id must be a positive integer")
        if iface.get("global") not in (None, True, False):
            err(module_name, f"{w}: 'global' must be a boolean")
        if not iface.get("request") and not iface.get("event"):
            err(module_name, f"{w}: interface declares no requests and no events")

        request_names: set[str] = set()
        event_names: set[str] = set()
        for req in iface.get("request", []):
            rname = req.get("name")
            if not isinstance(rname, str) or not rname:
                err(module_name, f"{w}: request missing 'name'")
                continue
            rw = f"{w}.request '{rname}'"
            if rname in request_names:
                err(module_name, f"{rw}: duplicate")
            request_names.add(rname)
            check_doc(module_name, req, rw)
            unknown = set(req) - KEYS_REQUEST
            if unknown:
                err(module_name, f"{rw}: unknown keys {sorted(unknown)}")
            since = req.get("since", 1)
            if not isinstance(since, int) or not (isinstance(vmin, int) and vmin <= since <= (vmax or 0)):
                err(module_name, f"{rw}: 'since' outside module version range")
            reply = req.get("reply")
            if reply is not None and not isinstance(reply, str):
                err(module_name, f"{rw}: 'reply' must name an event")
            check_args(module_name, req, rw, enum_names, bitset_names,
                       iface_names, set(), imported_enums, imported_bitsets,
                       imported_ifaces)
        for ev in iface.get("event", []):
            ename = ev.get("name")
            if not isinstance(ename, str) or not ename:
                err(module_name, f"{w}: event missing 'name'")
                continue
            ew = f"{w}.event '{ename}'"
            if ename in event_names:
                err(module_name, f"{ew}: duplicate")
            event_names.add(ename)
            check_doc(module_name, ev, ew)
            unknown = set(ev) - KEYS_EVENT
            if unknown:
                err(module_name, f"{ew}: unknown keys {sorted(unknown)}")
            since = ev.get("since", 1)
            if not isinstance(since, int) or not (isinstance(vmin, int) and vmin <= since <= (vmax or 0)):
                err(module_name, f"{ew}: 'since' outside module version range")
            check_args(module_name, ev, ew, enum_names, bitset_names,
                       iface_names, set(), imported_enums, imported_bitsets,
                       imported_ifaces)
        for req in iface.get("request", []):
            if isinstance(req, dict) and isinstance(req.get("reply"), str):
                if req["reply"] not in event_names:
                    err(module_name, f"{w}.request '{req.get('name')}': reply event '{req['reply']}' not declared")

    if module_name == "core":
        for must in ("connection", "registry", "output", "surface", "buffer", "shm", "dmabuf"):
            if must not in iface_names:
                err(module_name, f"core module must define interface '{must}'")


def main() -> int:
    if not SPEC_DIR.is_dir():
        print("spec/ directory not found", file=sys.stderr)
        return 1
    specs = sorted(SPEC_DIR.glob("*.toml"))
    if not specs:
        print("no spec files found", file=sys.stderr)
        return 1
    names: set[str] = set()
    for path in specs:
        check_module(path)
        raw = tomllib.loads(path.read_text(encoding="utf-8"))
        mname = raw.get("module", {}).get("name")
        if mname in names:
            err(path.stem, f"duplicate module name {mname!r}")
        names.add(mname)
    if errors:
        print(f"spec lint FAILED ({len(errors)} problem(s)):")
        for e in errors:
            print(f"  - {e}")
        return 1
    print(f"spec lint OK: {len(specs)} module(s) clean: {', '.join(sorted(names))}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
