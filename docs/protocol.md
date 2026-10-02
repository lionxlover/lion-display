# The LDP Wire Protocol

Status: **Phase 2 — normative and implemented.** The machine-readable source
of truth is `spec/*.toml`; this document defines the framing, encoding
rules, and semantics that the TOML files reference. `ldpc` compiles the
specs into `ldp-protocol`, whose codec and test suite pin every rule in
this file to exact bytes and exact error codes.

---

## 1. Transport basis

* One `AF_UNIX` `SOCK_STREAM` socket per client connection.
* One message = one `sendmsg` (header + payload + optional SCM_RIGHTS FDs).
* All multi-byte fields are **little-endian**.
* All integers, including the header, are aligned to 8 bytes; total message
  length is always a multiple of 8.
* Clock: `ts` values are unsigned nanoseconds on `CLOCK_MONOTONIC`.

## 2. Message envelope

```
offset  size  field            meaning
──────  ────  ───────────────  ──────────────────────────────────────────
0       4     payload_words   payload length in 8-byte words (u32)
4       4     object_id       target object (u32; §3)
8       4     opcode          request or event opcode (u32; §5)
12      2     flags           u16 bitfield (§2.1)
14      2     fd_count        number of FDs in the ancillary array (u16)
16      8·N   payload         N = payload_words tagged arguments (§4)
```

Invariants, checked before any allocation or dispatch:

1. `16 + 8 · payload_words ≤ MAX_MESSAGE_BYTES` (1 MiB default, negotiable up
   in `hello`/`welcome`).
2. `fd_count ≤ MAX_FDS_PER_MESSAGE` (64) and equals the number of FDs actually
   received in the SCM_RIGHTS array — a mismatch is fatal
   (`fd_mismatch`).
3. All reserved flag bits are zero — unknown flags are fatal, so extensions
   are always negotiated, never guessed.
4. The receiver closes every FD of a message rejected by any validation rule.

### 2.1 Flags

| Bit | Name | Meaning |
|---|---|---|
| 0 | `URGENT` | hints the receiver's scheduler to dispatch first (input-class events) |
| 1 | `HAS_REPLY` | request expects a correlated reply event (informational) |
| 2–15 | reserved | must be zero |

### 2.2 Direction

Requests flow client→server; events flow server→client. Direction is a
property of the socket side, not the wire — the same envelope is used both
ways. Opcodes are namespaced per direction per interface (§5).

## 3. Object identity

An object ID (`u32`) names one protocol object within one connection:

* **bit 31 set** → server-allocated ID. The client must never send such an ID
  in a `new_id` argument.
* **bit 31 clear** → client-allocated ID, chosen by the client when creating
  objects (the `new_id` argument of a request). Valid client IDs are
  1 .. 2³¹−1; 0 is never a valid object ID (it encodes "no object" in
  nullable object arguments).

### 3.1 Generations

The server maintains a generation counter per object-ID slot (the full
wire ID: client- and server-allocated IDs occupy disjoint slots). When an
object is destroyed and its ID is later re-allocated on the same connection,
the generation increments. The wire format carries only the 32-bit ID —
so the protection splits into two layers, both implemented by the server:

* **Wire references** resolve to the slot's *current* occupant (that is
  how legal ID reuse works); a reference to a destroyed-but-unreused slot
  is `stale_object`, and a never-used slot is `invalid_object`.
* **Server-side stored references** — a compositor object keeping a
  handle to another object — are stored as (id, generation) pairs; a
  handle outliving its target resolves to `stale_object` instead of
  being misdelivered to whatever occupies the ID now. This is the
  misdelivery protection of the recycling discipline.

This makes ID recycling safe by construction; generations are connection
state, never wire data.

### 3.2 Object lifecycle

* Creation: a request with a `new_id` argument (client-chosen ID) or a
  server-side factory (server-chosen ID in the event that announces it).
* Destruction: **always** `connection.destroy(object_id, cookie)` →
  `connection.destroyed(object_id, cookie)`. Interfaces never declare their
  own destructors: one lifecycle path, uniform validation (typed aliasing,
  generation check, double-destroy check), one audit hook. Server-initiated
  teardown uses `connection.revoked(object_id, reason)` (e.g. the object's
  interface ceased to exist, or a capability was revoked).
* After a `destroy` request, further references to the destroyed object
  are protocol errors: `stale_object` for a destroyed-but-unreused slot,
  `invalid_object` for a slot that was never bound. A wire reference to a
  slot that has since been *reused* addresses its current occupant (the
  recycling discipline of §3.1); only server-side stored references can
  observe the reuse as a generation mismatch. Double `destroy` of a dead
  slot is `invalid_state`.

### 3.3 Bootstrap object

Object ID **1** is pre-bound to the `ldp.core connection` interface —
the only pre-bound object on any connection, and the only object that
exists before the handshake. ID 1 lives in the client-allocated range but
is **reserved**: client allocations of ID 1 are invalid
(`invalid_object`), and the server never announces it as a new object.
The first client message on any connection must be `connection.hello`.

## 4. Argument encoding

Every argument is **tagged**: one byte of type tag, then the value, then
zero padding to the next multiple of 8 — the whole *unit* is always a
whole number of 8-byte words. Tagged encoding lets a receiver validate a
message structurally *before* knowing its opcode signature —
schema-independent validation is a deliberate defense-in-depth property
(§9).

Unit layout (normative; `ldp-protocol` is the reference implementation
and its tests pin these bytes exactly):

| Tag | Type | Layout after the tag byte |
|---|---|---|
| `0x01` | `int32` | i32 (two's complement, LE) + 3 B pad — 8 B total |
| `0x02` | `uint32` | u32 LE + 3 B pad — 8 B total |
| `0x03` | `int64` | i64 LE + 7 B pad — 16 B total |
| `0x04` | `uint64` | u64 LE + 7 B pad — 16 B total |
| `0x05` | `float32` | f32 bits LE + 3 B pad; **must be finite** — NaN/inf is a protocol error |
| `0x06` | `float64` | f64 bits LE + 7 B pad; finite only |
| `0x07` | `bool` | 1 byte, value 0 or 1 — anything else is invalid; + 6 B pad |
| `0x08` | `string` | u32 byte length, bytes, NUL terminator, zero-pad to 8; must be valid UTF-8; length ≤ the negotiated string limit |
| `0x09` | `object` | u32 ID + 3 B pad; 0 = null when the argument is nullable |
| `0x0A` | `new_id` | u32 ID chosen by sender (client→server only) + 3 B pad |
| `0x0B` | `fd` | u32 index into the message's ancillary FD table + 3 B pad; must be `< fd_count` |
| `0x0C` | `array` | u32 element tag, u32 count, 7 B pad (16 B header), then each element as a full tagged unit of the declared type |
| `0x0D` | `rect` | i32 x, i32 y, u32 width, u32 height + 7 B pad — 24 B total |
| `0x0E` | `enum` | u32 + 3 B pad; unknown values are structurally valid (see below) |
| `0x0F` | `bitset` | 4 × u32 LE words = the 128-bit set + 7 B pad — 24 B total |
| `0x10` | `ts` | u64 monotonic nanoseconds + 7 B pad — 16 B total |

Notes:

* Padding is zero on the wire; receivers must not require it (but the
  encoder always emits zeros, and strict tooling mode rejects nonzero
  padding).
* Arrays: the element tag is declared once in the header, every element
  repeats it as its own tag byte, and each element is padded like a
  top-level unit. Elements must be scalars or `rect` (the set
  `ArgType::is_array_element` defines: int32, uint32, int64, uint64,
  float32, float64, bool, fd, rect). Even an empty array carries its
  element type. Region arguments are `array<rect>`. Strings are
  NUL-terminated for C-bridge comfort; the declared length excludes the
  NUL.
* FD values never appear inline — `fd` arguments are *indices* into the
  SCM_RIGHTS array of the same message, and the header's `fd_count` must
  equal the array length. This prevents FD/argument desynchronization, a
  classic wire bug class.
* **Forward compatibility (enums, bitsets):** unknown enum values and
  undeclared bitset bits are structurally valid on the wire — receivers
  MUST ignore what they do not know at the semantic layer (the evolution
  policy of `docs/spec-format.md` §4). Strict validation mode (tooling,
  conformance suites, fuzzing) rejects them with `out_of_range`.
  Corrupt tags, flags, argument counts, and structural violations are
  never tolerated.

### 4.1 Worked example

`surface.damage(x=10, y=20, width=100, height=50)` (opcode 4, object
0x8000_0100, no flags, no FDs) encodes as 48 bytes — a 16-byte header
plus four one-word units (32 payload bytes, `payload_words = 4`):

```text
04000000 00010080 04000000 0000 0000    header
01 0a000000 00000000                     int32 10
01 14000000 00000000                     int32 20
02 64000000 00000000                     uint32 100
02 32000000 00000000                     uint32 50
```

(The spec's `damage` actually takes an `array<rect>`; the scalar form is
shown for encoding clarity. `ldp-protocol`'s test suite pins these bytes
exactly.)

## 5. Interfaces, opcodes, versions

* An **interface** is a named, versioned set of requests and events. Names are
  module-qualified: `ldp.core.surface`, `ldp.shell.toplevel`.
* Opcodes are assigned per interface, **contiguous from 1**, in declaration
  order, separately for requests and events. Once released, opcode numbers
  and their argument lists are frozen; evolution adds *new* opcodes with
  `since = v` guards (matching the module's version range).
* A **module** is the versioning and distribution unit (`ldp.core`,
  `ldp.shell`, …). The registry advertises every interface with a version
  *range* `[min, max]`. A client binds at any version in
  `[max(client_min, server_min), min(client_max, server_max)]`; the empty
  intersection means "not available" (bind fails with `unsupported_version`).
* **Capabilities.** Capability discovery is explicit: bound objects emit
  dedicated capability events (`output.capabilities`, `dmabuf.feedback`, …)
  and the registry serves introspection blobs on demand. Structural change ⇒
  new opcode or new version; behavioral variation within a version ⇒
  capability bits. This split keeps "which features can I use?" answerable
  without version-number folklore.

## 6. Handshake

```
client                                   server
  connection.hello(protocol_release,    ──▶
                   options)
                                         ◀── connection.welcome(protocol_release, caps,
                                                                 client_id, app_id, sandbox)
  connection.get_registry(version,      ──▶
                          new_id)
                                         ◀── registry.global(...)  ×N   (on new_id)
```

* `hello.protocol_release` — the client's LDP release number, for
  diagnostics. `hello.options` is a `connection_options` bitset; the server
  may silently drop any bit (the granted set is observable through what
  works: `introspection` enables `registry.introspect`, `large_messages`
  raises the message ceiling to 64 MiB for the connection).
* `get_registry` — materializes the per-connection registry object (the
  bootstrap path: the registry cannot be bound before it exists). The
  server then replays one `registry.global` per advertised global on the
  new object. The registry is also itself a global: further registries may
  be bound with `registry.bind`.
* `welcome.sandbox` — the server's verdict on the peer (from `SO_PEERCRED`
  and, in later phases, manifest verification), before any global is
  advertised. `welcome.app_id` is the verified application identity, empty
  until a security broker exists.
* Any message before `hello`, or a second `hello`, is fatal
  (`invalid_state`).
* Round-trip primitive: `connection.sync(cookie)` → `connection.sync_done(cookie)`
  after all prior events have been delivered.

## 7. Event ordering & coalescing

Events are strictly FIFO per connection. Under backpressure the server may
coalesce per class *before* sending (never after): latest-wins for
configuration and presentation-class events; input events are never dropped
or reordered. Coalescing is observable only under pressure, and the
`presented`/`frame_target` pair is always self-consistent when coalesced.

## 8. Errors

Errors are **fatal to the connection** and always delivered as
`connection.error(code, object_id, message)` followed by socket close. The
taxonomy is global (stable numeric codes, shared by all modules):

| Code | Name | Meaning |
|---|---|---|
| 1 | `invalid_object` | object ID not bound on this connection |
| 2 | `stale_object` | generation mismatch (destroyed-and-reused ID) |
| 3 | `invalid_interface` | object exists but is not of the required interface |
| 4 | `invalid_opcode` | opcode not defined for the object's interface version |
| 5 | `signature_mismatch` | argument types do not match the opcode signature |
| 6 | `malformed_message` | framing, alignment, or tag violation |
| 7 | `fd_mismatch` | FD count/index inconsistency |
| 8 | `invalid_string` | non-UTF-8 or malformed string encoding (over-length is `limit_exceeded`) |
| 9 | `limit_exceeded` | message/array/string/object/FD/resource ceiling |
| 10 | `unauthorized` | capability check failed (audit record emitted) |
| 11 | `invalid_state` | request illegal in the object's current state |
| 12 | `out_of_range` | value outside the declared domain |
| 13 | `unsupported_version` | version negotiation failed |
| 14 | `invalid_buffer` | buffer/geometry/format/modifier rejected |
| 15 | `server_error` | internal failure (client should reconnect) |

Denial (authorization) and invalidity (protocol) are distinct: `unauthorized`
never leaks why, and is always audited.

## 9. Validation pipeline

Every inbound message passes, in order, each stage independent of the next:

1. **Framing** (transport): size, alignment, fd_count consistency.
2. **Structural decode** (ldp-protocol): tags well-formed, lengths bounded,
   strings UTF-8, floats finite, bools 0/1, enum/bitset ranges.
3. **Signature check**: decoded argument types match the opcode's signature
   for the object's bound interface version (`since` gates included).
4. **Semantic check** (per-interface code): state machines, geometry
   domains, ownership of referenced objects, capabilities.

A message rejected at stage 1–3 costs only bounded work — no allocation
proportional to untrusted sizes happens before all four length fields are
validated. This ordering is a security requirement (see `threat-model.md` §4).

## 10. Limits (defaults)

| Limit | Value |
|---|---|
| message size | 1 MiB (negotiable to 64 MiB for large transfers in `hello.options`) |
| FDs per message | 64 |
| open FDs per client | 256 |
| string length | 4096 B |
| live objects per client | 2²⁰ |
| region rects per damage/region call | 4096 |
| queued event bytes per client | 2 MiB (then coalescing/backpressure engages) |
| buffer bytes per client | policy (default 512 MiB) |

## 11. Zero-copy & sync conventions

* Buffers travel as FDs once (`shm_pool` fd, `dmabuf` plane fds), then by
  object reference.
* Acquire fences: `surface.commit` may carry an optional fence FD (sync-file
  or syncobj dmabuf point via `ldp.fence`); the server waits before reading.
* Release fences: `buffer.release` carries a fence FD the server signals
  after last read; reusing a buffer without waiting is a client bug the
  server defends against by fencing.
* Neither side ever spins: waits are `poll()`/`eventfd`-integrated in the
  event loops.

## 12. Determinism rules

* No NaN/Inf, no uninitialized padding, no time-of-day values (monotonic
  only), no hash-ordered iteration in observable behavior (event order is
  insertion order).
* The scheduler's decisions are a pure function of (event history, clock
  samples, policy) — replayable and tested.

## 13. Wire-level example session

```
C→S  connection.hello(protocol_release=1, options=0)
S→C  connection.welcome(protocol_release=1, caps=..., client_id=7,
                         app_id="", sandbox=unconfined)
C→S  connection.get_registry(version=1, new_id=2)
S→C  registry.global(interface="ldp.core.output", min=1, max=1)     # on object 2
S→C  registry.global(interface="ldp.shell", min=1, max=1)
C→S  registry.bind(interface="ldp.shell", version=1, new_id=3)
S→C  registry.bound(interface="ldp.shell", version=1)               # on object 2
C→S  shell.get_toplevel(surface=4, new_id=5)
S→C  toplevel.configure(serial=1, states=ACTIVATED, size=1280x800, insets=...)
C→S  toplevel.ack_configure(serial=1)
C→S  surface.attach(buffer=6)
C→S  surface.damage(rects=[(0,0,1280,800)])
C→S  surface.commit(cookie=1)
S→C  surface.frame_target(frame=1, target_ts=..., refresh_ns=16_666_666,
                         budget_ns=..., policy=depth1)
S→C  surface.presented(frame=1, ts=..., flags=VBLANK, refresh_ns=16_666_666)
```

(The argument spellings above are illustrative; `spec/*.toml` is normative.)
