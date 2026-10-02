# The API stability contract

*Phase 31. This document is the promise `docs/comparison.md`'s
"API stability for apps" row stands on: what freezes, what may grow,
and which gate enforces it.*

## The promise

**A client written against LDP's v1 surface keeps working, release
after release.** The mechanism is not trust — it is a frozen artifact
in the repository and a gate that fails the build when it drifts:

* `spec/surface-v1.json` — the complete client-facing surface: every
  module, interface, operation, opcode, `since` version, reply
  correlation, and argument (name, type, qualifier, nullability).
  Byte-deterministic, docs excluded (stability is about shape, not
  prose).
* `cargo run -p ldpc -- snapshot --check` — the freeze gate, run by
  `scripts/verify.sh` on every commit and mirrored by a unit test in
  `crates/ldp-protocol/tests/drift.rs`. Any difference — a renamed
  operation, a renumbered opcode, a reordered argument, a changed
  type, a moved version bound — fails the build.

## What may change, and how

| Change | Legal? | The gate's response |
|---|---|---|
| New operation (appended, new `since`) | Yes — the evolution policy | Names the addition; re-freeze consciously |
| New interface / module | Yes | Same |
| New enum value / bitset bit | Yes — receivers must ignore unknowns (forward-compat wire rule) | Same (the surface carries the enum tables' presence) |
| Renaming anything | **No** | Build fails |
| Renumbering / reordering opcodes | **No** (opcodes are declaration order, frozen at first release) | Build fails |
| Changing an argument's type or order | **No** | Build fails |
| Removing anything | **No** (deprecation is a *process*, see below) | Build fails |
| Advancing `version_max` | Yes (with new `since`-guarded ops) | Re-freeze |
| Advancing `version_min` | Only with a full deprecation cycle | Re-freeze; the cycle is the review |

A legal addition is made in three steps, all visible in one commit:
edit the spec, regenerate (`cargo run -p ldpc -- gen`), and re-freeze
(`cargo run -p ldpc -- snapshot`). The re-frozen diff in review is
the *entire* compatibility decision — nothing else may move in it.

## Deprecation (the removal process)

Removal never happens in one release. A deprecation:

1. **Mark** — the docs name the operation deprecated; the surface
   keeps it (a deprecated operation is still a working operation).
2. **Wait** — at least two minor releases, so every client author
   sees the notice.
3. **Bound** — the operation's removal advances `version_min`; old
   clients that still bind below the new floor get the typed
   `unsupported_version` handshake failure (never a mystery break).
4. **Remove** — the spec drops it, the surface re-freezes, and the
   review shows exactly one line leaving.

No operation has yet been deprecated (the protocol is young); the
process is published *before* it is needed, because a stability
contract written after the first break is a eulogy.

## Why clients can trust it more than inertia

X11's legendary stability was never written down — it emerged from
the X Consortium's restraint, and its one real break (X11R7's module
split) still shipped compatibility shims. This contract is the
*mechanical* form of that restraint: the surface is an artifact, the
gate is a diff, and the review is the decision. A client author can
diff two releases' `surface-v1.json` files and know — byte-exactly —
what changed underneath them.

## The old-client proof

The wire rules carry the runtime half:

* Version pinning at bind (`registry.bind(interface, version, id)`)
  with the compiled range checked — an old client binding v1 of an
  interface whose `version_max` has since advanced still binds, and
  every operation with `since > 1` is invisible to it (the typed
  `invalid_opcode` rejection if it somehow sends one).
* Unknown enum values and undeclared bitset bits are *tolerated* by
  receiving validators (`ValidationMode::Tolerant`, the default) — a
  new server may send a new enum value to an old client and the old
  client's stage-3 validation passes (the semantic layer is told to
  ignore).
* Unknown argument *tags* and reserved flag bits are fatal — the
  corruption-versus-evolution split (corruption is never tolerated).

The `since`-gate unit tests (`crates/ldp-protocol/src/validate.rs`)
and the tolerant-mode corpus pin all three behaviors; the freeze gate
pins the surface they defend.
