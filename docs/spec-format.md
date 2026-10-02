# The LDP Specification Format (TOML)

Status: **Phase 2 — normative, compiler-enforced.** One file per module under `spec/`,
file name = module suffix (`core.toml` → module `ldp.core`). `ldpc` (Phase 2)
is the reference compiler (implemented); `scripts/spec_lint.py` enforces the same structural
rules in CI today, so specs are continuously validated.

TOML was chosen for the protocol source of truth because it is human-diffable
in code review, trivially parseable from many languages (Rust, Python, C…),
and — unlike XML — needs no schema framework to stay readable. The grammar
below is deliberately small: anything not listed is an error (unknown keys
are rejected, not ignored).

---

## 1. File structure

```toml
[module]                      # exactly one per file
name = "ldp.core"
version_min = 1
version_max = 1
doc = "Core objects: connection, registry, outputs, surfaces, buffers."

[[import]]                    # optional: cross-module references
interface = "ldp.core.output" # importable kinds: interface, enum, bitset

[[import]]
enum = "ldp.core.transform"   # referenced as the fully qualified name

[[enum]]                      # 0..n named integer constants
name = "button_state"
doc = "Physical button state."
[enum.values]
released = 1
pressed = 2

[[bitset]]                    # 0..n named 128-bit flag sets
name = "seat_caps"
doc = "Input capabilities advertised by a seat."
[bitset.bits]
pointer = 0
keyboard = 1
touch = 2
tablet = 3
gesture = 4

[[interface]]                 # 1..n object types
name = "pointer"
doc = "Pointer device of a seat."
global = false                # true: advertised in the registry

[[interface.request]]         # 0..n, opcodes 1.. in declaration order
name = "frame"
since = 1
doc = "Request the next frame deadline for this surface."
reply = "frame_target"        # optional: event sent as direct reply
args = [
  { name = "cookie", type = "uint32" },
]

[[interface.event]]
name = "frame_target"
since = 1
doc = "The deadline the next commit must meet to land on the target frame."
args = [
  { name = "frame",    type = "uint64" },
  { name = "target",   type = "ts" },
  { name = "refresh",  type = "uint64" },
  { name = "budget",   type = "uint64" },
  { name = "policy",   type = "enum", of = "frame_policy" },
]
```

## 2. Reference rules

| Key | Type | Required | Rules |
|---|---|---|---|
| `[module].name` | string | ✓ | `ldp.<filename-stem>` exactly |
| `[module].version_min/_max` | int | ✓ | `1 ≤ min ≤ max`; the module's wire version range |
| `[module].doc` | string | ✓ | non-empty; rendered into rustdoc |
| `[[import]].interface/enum/bitset` | string | — | fully qualified `ldp.*` name; makes the item referenceable in `of` |
| `[[enum]].name` | string | ✓ | unique within module |
| `[[enum]].values` | table | ✓ | positive dense ints 1..=N (wire-safe, no zero) |
| `[[bitset]].name` | string | ✓ | unique within module |
| `[[bitset]].bits` | table | ✓ | bit indices 0..=127, unique |
| `[[interface]].name` | string | ✓ | unique within module; fully qualified as `<module>.<name>` |
| `[[interface]].global` | bool | — | registry-visible factory if true |
| `[[interface]].builtin_id` | int | — | pre-bound object ID (only `connection` uses it, ID 1) |
| `[[interface.request]] / [[interface.event]]` | table array | — | `name` unique within kind; `since` within module range; `args` ordered |
| `args[i].type` | string | ✓ | one of §3 |
| `args[i].doc` | string | — | per-argument documentation (recommended) |
| `args[i].of` | string | for `enum`/`bitset`/`array` | target enum/bitset/element type; for `object`/`new_id` optional — omitted means a generic object reference |
| `args[i].nullable` | bool | — | only valid for `object` (wire ID 0 = null) |
| `request.reply` | string | — | names a declared event of the same interface |

Additional invariants enforced by the linter/compiler:

* Opcode numbering = declaration order, per interface, requests and events
  counted separately from 1. Never renumber; new operations append.
* Every `enum`/`bitset`/`request`/`event`/`interface` carries a non-empty
  `doc`. The spec is the documentation of record.
* `object`/`new_id` arguments reference interfaces in the same module or an
  imported one; omitting `of` declares a *generic* object reference
  (validated semantically at runtime — e.g. `connection.destroy`). `new_id`
  appears only in requests (client-chosen ID) or in events that announce
  server-side objects.
* Cross-module enum/bitset references use the fully qualified name
  (`ldp.core.transform`) and require a matching `[[import]]` entry.
* Array element types are scalars (including `fd` — an index into the message
  FD table) or `rect`.
* A request with `reply` must reference a declared event; the reply event is
  otherwise a normal event.
* Modules may not redefine another module's interface names; the fully
  qualified name is the global key.

## 3. Argument types

Wire encodings are defined in `protocol.md` §4; the spec references them by
name:

| Spec type | Wire tag | `of` refers to |
|---|---|---|
| `int32` `uint32` `int64` `uint64` | scalars | — |
| `float32` `float64` | finite floats | — |
| `bool` | 0/1 | — |
| `string` | UTF-8, ≤ 4096 B | — |
| `ts` | u64 monotonic ns | — |
| `rect` | i32 x, i32 y, u32 w, u32 h | — |
| `object` | u32 ID | interface (nullable allowed) |
| `new_id` | u32 ID | interface |
| `fd` | index into FD table | — |
| `array` | typed elements | element type (scalar or `rect`) |
| `enum` | u32 | enum name |
| `bitset` | 128-bit | bitset name |

## 4. Versioning & evolution policy

1. Released opcode numbers, argument lists, enum values, and bit assignments
   are **frozen**. Evolution adds:
   * new requests/events with `since = v` (ignored by older peers, rejected
     if used against a lower bound version),
   * new enum values / capability bits (unknown values must be ignored
     gracefully by receivers — forward compatibility),
   * new interfaces and new modules.
2. `version_max` advances when new `since > 1` operations exist;
   `version_min` advances only when semantics genuinely require it (rare,
   release-noted).
3. A receiver MUST ignore unknown enum values and unknown capability bits;
   it MUST reject unknown message tags, flags, and argument-count mismatches
   (that's corruption, not evolution).
4. Protocol versions and crate SemVer are independent: the crate version
   tracks implementation API; the module version tracks wire compat.

## 5. What `ldpc` generates (the Phase 2 contract, implemented)

For every module spec, `ldpc` emits into `crates/ldp-protocol/src/generated/`
(committed output; `ldpc gen --check` and a `cargo test` drift gate fail
when it disagrees with `spec/`):

* `mod.rs` + `<module>.rs`: a static `ModuleSchema` table per module —
  fully qualified interface names, version ranges, request/event
  signatures as `&[OpSchema]` with `&[ArgSchema]` argument lists — plus
  typed `#[non_exhaustive]` enums (`from_wire`/`to_wire`), bitset const
  modules (`MASK`, per-bit consts), opcode constant modules
  (`<interface>::request::<OP>` / `<interface>::event::<OP>`), and
  rustdoc for every item, taken verbatim from the spec `doc` fields,
* `blob.bin`: the introspection blob — the spec re-serialized into the
  compact `LDPS` byte format parsed by `ldp_protocol::blob` (layout:
  magic `LDPS`, u16 format version, u32 module count, then
  length-prefixed strings and LE integers; imports are compile-time
  linkage and are not serialized),
* `docs/reference/<module>.md`: a human-diffable reference rendering.

`scripts/spec_lint.py` remains the fast structural pre-check; `ldpc`
adds the whole-set semantic rules (cross-module import resolution,
global FQ-name uniqueness, kind disjointness within a module).
