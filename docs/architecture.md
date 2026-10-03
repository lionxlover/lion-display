# LDP Architecture

Status: **Phase 1 baseline — normative for all later phases.**
This document is the complete architecture of the Lion Display Protocol and its
server ecosystem. It is deliberately independent of Wayland's architecture;
where a concept coincides (there are only so many ways to push a buffer to a
scanout engine), the differing LDP design rationale is called out explicitly.

Companion documents: `protocol.md` (wire format & object model),
`spec-format.md` (TOML grammar), `threat-model.md` (security),
`roadmap.md` (20-phase delivery plan).

---

## 1. System overview

LDP is one protocol, one reference server, and a set of optional bridges:

```
 LionOS desktop session (systemd --user)
 ┌────────────────────────────────────────────────────────────────────┐
 │ lion-compositor (the LDP server)              uid = compositor     │
 │  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌───────────┐ ┌─────────┐ │
 │  │ ldp-shell│ │ldp-input │ │ldp-clipbrd│ │ldp-color  │ │ldp-pwr  │ │
 │  │ spaces   │ │ seats    │ │ offers   │ │ HDR/VRR   │ │ session │ │
 │  ├──────────┴─┴──────────┴─┴───────────┴─┴───────────┴─┴─────────┤ │
 │  │ ldp-compositor: scene graph · damage · deadline frame scheduler│ │
 │  ├───────────────────────────────────────────────────────────────┤ │
 │  │ ldp-renderer (GL primary / software) · ldp-gpu (dmabuf,fences) │ │
 │  ├───────────────────────────────────────────────────────────────┤ │
 │  │ ldp-display: DRM/KMS atomic · hotplug · backlight              │ │
 │  ├───────────────────────────────────────────────────────────────┤ │
 │  │ ldp-server: object store · dispatch · capability enforcement   │ │
 │  ├───────────────────────────────────────────────────────────────┤ │
 │  │ ldp-transport: unix sockets, SCM_RIGHTS, SO_PEERCRED           │ │
 │  └───────────────────────────────────────────────────────────────┘ │
 │        ▲ LDP socket          ▲ LDP socket         ▲ LDP socket     │
 └────────┼─────────────────────┼────────────────────┼────────────────┘
   native app │           ldp-wayland-bridge │   ldp-x11-bridge │
   (ldp-client, any lang)   (Wayland clients)    (X11 clients)
```

Design decisions, in order of importance:

1. **The server owns the clock.** Every output has a frame timeline derived
   from KMS page-flip timestamps. Clients are told deadlines; they never guess.
2. **Everything is a fenced buffer exchange.** Surfaces submit DMA-BUF or
   shared-memory buffers together with explicit acquire/release fences.
3. **The protocol is data-driven.** All interfaces are declared in
   `spec/*.toml`, compiled to Rust by `ldpc`; runtime introspection makes
   debuggers protocol-complete without recompilation.
4. **Privilege is a token, not a uid check.** Capability tokens are granted by
   a broker after manifest verification or user escalation, enforced per
   request, and logged per decision.
5. **Compatibility lives outside the core.** Bridges are ordinary LDP clients
   with additional ambient authority (they receive a bridge scope token);
   nothing in `ldp-*` core crates knows Wayland or X11 exists.

## 2. Process, threading and memory model

### 2.1 Processes

| Process | uid | Role |
|---|---|---|
| `lion-compositor` | dedicated `compositor` uid | LDP server, DRM-Master via logind `TakeDevice` |
| `ldp-session-broker` | own uid, privileged helper | manifest verification, capability minting, escalation prompts |
| `ldp-wayland-bridge` / `ldp-x11-bridge` | user uid | compatibility proxies |
| applications | user uid | LDP clients |

The broker is the *only* privileged authority; the compositor itself runs
unprivileged w.r.t. files and asks logind for device access. Screenshot and
recording features in the compositor are gated on broker-minted tokens, so a
compromised compositor still cannot silently capture the screen without a
traceable grant.

### 2.2 Threading model (lion-compositor)

The compositor is multi-threaded with **one thread per responsibility**, never
per-client:

| Thread | Owns | Blocking on |
|---|---|---|
| `main` | event loop, protocol dispatch, scene graph mutations | epoll on client sockets + timerfd |
| `render-N` (per GPU) | GL/Vulkan command submission | fence waits (poll, never spin) |
| `input` | evdev devices (libevdev-compatible ioctls) | epoll on `/dev/input/*` |
| `hotplug` | udev monitor socket | udev events |
| `broker` | connection to `ldp-session-broker` | broker socket |

Data handoff is via lock-free MPSC queues with bounded capacity (input events
never dropped; protocol events may be coalesced per class — §10.4). The scene
graph uses a **double-buffered state snapshot**: the main thread mutates the
"next" state while the render thread reads an immutable "current" state,
swapped atomically at commit boundaries. No locks are held across GPU calls.

*As built (Phase 10, the vertical slice):* the first real assembly is
deliberately simpler — one session thread per client (the phase-4 server
model) and a single world lock serializing render and dispatch. The
determinism doctrine does the heavy lifting instead of threads: the mock
clock advances only at protocol wake points, so one `pump` per handled
message renders, flips, and lands presentation feedback to quiescence.
The MPSC/epoll event loop, the render threads, and the double-buffered
scene swap arrive with the input and GPU phases, where their consumers
exist.

### 2.3 Memory & FD ownership

* Every FD has exactly one owner; ownership transfers are explicit in the API
  (`OwnedFd` in / `OwnedFd` out). The transport layer closes any FDs belonging
  to a malformed or rejected message immediately (FD-bomb defense).
* Buffers are zero-copy: client-shared memory and DMA-BUFs are mapped directly
  into the renderer; the compositor never copies pixel data except when a
  repack is unavoidable (format conversion for a scanout plane).
* Per-client resource ceilings (object count, buffer bytes, FD count) are
  enforced in `ldp-server` before allocation; exceeding a ceiling is a
  protocol error, not an OOM.

## 3. Crate map (23 crates)

| Crate | Layer | Responsibility | Phase |
|---|---|---|---|
| `ldp-core` | foundation | IDs, versions, errors, wire value model, geometry/damage algebra, timing types, color/HDR/buffer types, capability bitsets, tokens, limits | **1** |
| `ldp-protocol` | foundation | wire codec, schema registry, generated interface tables (`ldpc` output), message validation | 2 |
| `ldp-transport` | foundation | unix sockets, framing, SCM_RIGHTS FD passing, SO_PEERCRED credentials, limits, backpressure | 3 |
| `ldp-client` | library | client connection, proxies, dispatch, event queues, typed API for core interfaces | 5 |
| `ldp-server` | library | accept loop, per-client sessions, generational object store, dispatch, capability gates | 4 |
| `ldp-compositor` | server core | scene graph, surface tree, damage propagation, deadline frame scheduler, animations, presentation feedback | 6–7 |
| `ldp-renderer` | server core | `Renderer` trait; GL (EGL) primary backend, software backend (headless CI + fallback), overlay plane assignment | 8 |
| `ldp-gpu` | server core | GPU discovery, render-node selection, DMA-BUF import/export, fence/syncobj plumbing, EGL context management | 9 |
| `ldp-display` | server core | DRM/KMS atomic modesetting, connector/CRTC/plane management, hotplug, VRR props, DPMS, backlight | 9 |
| `ldp-input` | server core | evdev backend, pointer acceleration, gesture state machines, tablet transforms, xkbcommon keymap loading | 11 |
| `ldp-seat` | server core | seats, device assignment, per-seat focus stacks, multi-seat isolation | 11 |
| `ldp-shell` | server core | toplevel/popup/dialog state machines, workspaces ("spaces"), stacking order, SSD geometry | 12 |
| `ldp-clipboard` | server core | data sources/offers, MIME negotiation, streaming transfers, primary selection, DnD state machine | 13 |
| `ldp-color` | server core | color-space math, transfer functions, gamut mapping, tone mapping (BT.2390), ICC import | 14 |
| `ldp-hdr` | server core | HDR metadata handling, luminance adaptation, static/dynamic HDR policy | 14 |
| `ldp-vrr` | server core | adaptive-sync policy engine, refresh windows, tearing policy, latency optimizer | 15 |
| `ldp-security` | server core + broker | capability token minting/verification, manifest policy engine, audit log | 16 |
| `ldp-accessibility` | server core | a11y event bus, screen-reader bridge, magnifier, settings broadcast | 16 |
| `ldp-power` | server core | idle state machine, DPMS, backlight policy, suspend/resume, inhibitors | 16 |
| `ldp-session` | server core | logind integration, VT handling, session activation, lock screen | 16 |
| `ldp-wayland-bridge` | compat | Wayland server subset, proxies windows into LDP | 17 |
| `ldp-x11-bridge` | compat | X11 server subset (core protocol + SHM), proxies windows into LDP | 17 |
| `ldp-tools` | tooling | shared CLI framework for the tool suite | 18 |
| `ldp-test` | tooling | conformance harness, fuzz corpus generation, mock clients | 19 |

`ldp-tools` and `ldp-test` are libraries; the executables (`lion-compositor`,
`ldp-info`, `ldp-debug`, `ldp-validate`, `ldp-profiler`, `ldp-audit`,
`ldp-input-debug`, `ldpc`) live in `binaries/` and `tools/`.

## 4. Core protocol architecture

See `protocol.md` for the full wire-level contract. Architecture-level
properties:

* **Object model.** Per-connection object namespace; IDs with bit 31 set are
  server-allocated, otherwise client-allocated. The server-side table is
  keyed by the full wire ID (the two allocation ranges are disjoint), every
  slot carries a generation counter, and the protection is two-layered
  (`protocol.md` §3.1): wire references resolve to the slot's current
  occupant — so legal ID reuse just works — while server-side *stored*
  references (a subsurface's parent, a buffer's pool) carry their
  generation and resolve to `stale_object` instead of being misdelivered
  (this class of bug is endemic in ID-recycling protocols; LDP makes it
  structurally impossible).
* **Central lifecycle.** Destruction is a `connection.destroy(id, cookie)`
  request answered by `connection.destroyed(id, cookie)` — one code path, one
  validation rule, one audit point; interfaces never declare per-object
  destructors. The registry bootstrap is the one deliberate exception to
  "objects come from `registry.bind`": `connection.get_registry` materializes
  the registry (the mechanism that bootstraps binding itself), after which
  the server replays one `registry.global` per advertised global on it.
* **Version + capability negotiation.** Interfaces are versioned *ranges*. The
  registry advertises `(interface, min_version, max_version)`; binding pins a
  version. Behavioral capability discovery happens on the bound objects via
  dedicated capability events (`output.capabilities`, `dmabuf.feedback`,
  `color_manager.capabilities`, …) plus the runtime introspection blob —
  avoiding Wayland's version-number feature creep ("is `wl_surface` v6
  enough for this?").
* **Module system.** Interfaces group into modules (`ldp.core`, `ldp.shell`,
  `ldp.input`, `ldp.data`, `ldp.color`, `ldp.security`, `ldp.a11y`,
  `ldp.session`). A client binds only what it uses; unbound modules cost one
  registry entry, zero objects, zero events.
* **Determinism rules.** No NaN on the wire; floats are finite or the message
  is invalid. Timestamps are `ts` (u64 ns, CLOCK_MONOTONIC) values. Strings
  are UTF-8 with hard length caps. All arrays are length-prefixed and bounded
  by the message-size limit.

## 5. Client system

`ldp-server` (Phase 4) implements the connection/session engine the client
system talks to; `ldp-client` (Phase 5) is a zero-dependency-safe library
(plus optional typed helpers):

* **Session engine.** One `ClientSession` per accepted connection owns the
  framing reader/writer, the negotiated limits, and the generational object
  store. Its per-message pipeline is exactly `protocol.md` §9: framing →
  structural decode → **object resolution before signature check** (the
  target's interface and pinned version select the signature — the
  type-confusion defense) → dispatch. The session core implements
  `ldp.core.connection` and `ldp.core.registry` itself; every other
  interface is a `Dispatcher` implementation — the compositor (Phase 6) is
  *a* dispatcher, not a fork of the session core. Dispatchers receive
  validated requests plus a context for emitting events, creating and
  revoking objects, and taking ownership of message FDs; untaken FDs close
  on return.
* **Connection lifecycle (client half, Phase 5):** `Connection::connect`
  performs the `hello`/`welcome` handshake under a `ClientConfig` (release,
  `connection_options` bits, handshake timeout); nothing may precede
  `welcome`. A client requesting `large_messages` widens its own framing to
  64 MiB at request time — the granted set is observable through what works.
  The blocking driving model (v1) is `pump_one` → `dispatch_budget`, with
  `roundtrip` as the sync barrier; event loops flip the stream to
  nonblocking after the handshake and poll the raw descriptor themselves.
  Client-side object IDs are handed out monotonically (no free list — reuse
  is what the generation discipline exists to guard); exhaustion is a hard
  error. Every event `new_id` argument binds a proxy automatically, so
  server-announced objects (`data_offer`, later factories) are usable
  immediately.
* **Identity:** the server reads `SO_PEERCRED` (pid/uid) *before* any protocol
  data is processed. The `welcome` event returns a security context: sandbox
  flavor, manifest scopes (if a verified manifest exists), and the client ID
  used in audit records.
* **Threading.** Phase 4 serves one session thread per connection
  (blocking I/O, panic isolation, detached with a live-count ceiling);
  the Phase 6 compositor main loop embeds `ClientSession::step` under
  `epoll` instead — the session API was shaped for exactly that. The
  client is one connection, one thread (v1): handlers receive events
  only, never a connection handle, so dispatch re-entrancy is a compile
  error rather than a deadlock.
* **Event dispatch (client half):** the inbound pipeline mirrors the
  server's §9 order with the direction flipped — proxy resolution *before*
  the signature check, so a server that targets events at objects the
  client never held is rejected locally. Events flow to a single
  `EventHandler` in dispatch order, after the connection's own
  bookkeeping (sync/pong cookies, proxy removal on `destroyed`/`revoked`).
  Five classes (input / data / control / configuration / presentation)
  have distinct queue lanes so a burst of `frame_target` events can never
  delay pointer motion beyond one dispatch cycle; control-class events
  are barriers dispatched only after every earlier-arrived event, which
  is exactly the `sync_done` contract. Interfaces absent from the
  compiled schema classify as control — the conservative default that
  cannot break ordering invariants.
* **Reconnect:** an LDP connection is stateful end to end — every proxy
  and pinned version dies with the socket — so reconnecting is a session
  *rebuild*: the `Reconnector` loop (connect → handshake → application
  `Rebuild` callback → retry) runs under bounded exponential backoff with
  deterministic jitter.
* **Crash handling:** client crashes are detected via socket `EPOLLHUP`/`EPIPE`
  plus pidfd in the server; the server reclaims all client objects, releases
  buffers, cancels grabs, and continues. A crashing client can never wedge the
  server: no lock is held while calling into client code, because the server
  never calls into client code — it only writes to sockets. Symmetrically, a
  server that dies leaves the client with a classified `DisconnectKind`
  (clean between messages, crash mid-message, transport failure), and every
  later operation on the connection reports it.

## 6. Transport

* **Socket:** one `AF_UNIX` stream socket per client at
  `$XDG_RUNTIME_DIR/ldp-<session>/ldp.sock` (plus an announcement symlink for
  discovery); `SO_PEERCRED` at accept time.
* **Framing:** 16-byte header (payload length in 8-byte words, object id,
  opcode, flags, FD count) + 8-byte-aligned payload. One `sendmsg` per message
  with SCM_RIGHTS FDs riding the same call — FD count is cross-checked between
  the header and the ancillary array; mismatch = malformed.
* **Tagged arguments:** every argument carries a type tag, enabling full
  schema-independent validation before dispatch (defense in depth vs.
  schema-confusion attacks).
* **Limits:** per-message size (1 MiB), FDs per message (64), queued bytes,
  FD table ceiling per client — all enforced pre-allocation.
* **Backpressure:** writes are non-blocking; a full socket parks the client's
  event queue (coalescible classes collapse; input events block new input
  processing for that client — the client is wedging itself, never the server).
  The transport's half (`ldp-transport`, Phase 3): the writer's local queue is
  bounded by the queued-event-byte limit plus a queued-FD budget, overflow
  rejects the send (the server's cue to coalesce or drop), and edge-triggered
  hooks report congestion / drain / recovery. On blocking streams the writer
  issues one `sendmsg` per call — large messages proceed by interleaved
  flush-and-read, never by parking a thread inside the kernel.
* **Credentials & FD hygiene:** received FDs are validated with `fcntl(F_GETFD)`
  before ownership transfer; the transport closes FDs of rejected messages.
  `unsafe` code lives only in the audited `sys` module (syscall wrappers);
  every other `ldp-transport` module forbids it outright.

## 7. Surface system

* `ldp.surface` — the core drawing target. State (buffer, transform, scale,
  input/opaque regions, color description, HDR metadata, presentation mode)
  is accumulated in a pending state and applied **atomically** on `commit`.
* `ldp.buffer` — immutable pixel storage created from `ldp.shm` pools or
  `ldp.dmabuf` plane sets. Release is fenced: `release(fd)` delivers a
  sync-file the client must wait for before reusing the buffer.
* **Damage:** `damage(rects[])` in surface coordinates or `damage_buffer` in
  buffer coordinates; the compositor unions it into the per-surface damage
  region, propagates through the tree (subsurface above/below relationships),
  and subtracts occlusion to produce per-plane damage.
* **Subsurfaces:** `ldp.subsurface` with sync (default: commit-atomic with
  parent) and async modes, stacking controls, and position setting.
* **Frame timing:** `surface.frame()` returns a frame ID; the server later
  emits `frame_target` (deadline + refresh + budget) and `presented`
  (timestamps + flags + estimator state) or `frame_dropped(reason)`. This is
  the *deadline-aware* core (§10).
* **The semantic layer (Phase 47):** a surface is more than its buffer.
  Over the shell it may claim what it *is* (`toplevel.set_semantic_role`:
  window, dialog, tooltip, overlay, lock), what it may *expose*
  (`set_security_class`: normal, private, protected, system — the capture
  path's ladder), and how the machine spends its frame budget on it
  (`set_scene_profile`: desktop, creative, gaming — the scheduler's
  per-surface admission floor). The claims are policy, carried in the
  scene's side-maps and enforced at the seams where they have teeth
  (§10.7); the server's own invariants stand above them (the lock role
  floors the security class; ephemeral roles never animate in).

## 8. Window/shell system

* **As built (Phase 12):** `ldp-shell` is pure window-management policy
  over ldp-core + the compiled schema (no clock reads, no I/O, no
  unsafe; the server binary is the integrator that maps
  `WindowKey` identities onto compositor scene nodes). The
  configure/ack commit is exact — one live proposal per object, latest
  serial wins, stale acks rejected, the next commit realizes the
  acked proposal — with a wrapping, reservation-skipping serial clock
  (`resume_from` seeds crash recovery past the pre-crash watermark).
  Gravity follows the LDP spec text (the popup *grows* from the anchor
  point along the gravity vector); the constraint pipeline is slide
  (capped at the anchor's opposite edge) → flip (axis-mirrored,
  accepted only when it fits) → resize (extent clamp), each gated by
  its permission bit. State flags are intents; fullscreen derives the
  full output with zero insets, maximized the workspace area minus
  ceil-scaled SSD insets, both clamped to hint pairs that saturate
  rather than contradict. Mixed-DPI: metrics are logical, insets
  ceil-cover per output (< 1 px error), and output migration
  re-proposes with fresh serials. Spaces track sticky-or-assigned
  visibility per seat; the stacking layer files dialogs above their
  parents, gates focus modally, and records activation *attribution*
  (bounded MRU log) for a11y and audit; bulk reflow is deterministic
  (BTreeMap-ordered rebuild). See §10 for the scene-graph side and
  §14 for the seat-side focus split.

`ldp-shell` implements the `ldp.shell` module:

* **Toplevels** (`ldp.toplevel`, the states arm served as of Phase
  49): states (maximized/fullscreen/minimized/activated/sticky),
  title, app-id, min/max sizes, workspace assignment — every request
  the frozen interface declares is live. Configuration is a two-phase
  commit: server sends `configure` with a serial; the client
  `ack_configure`s and its next commit realizes the state (the
  buffer it attaches is the size answer; the position completes the
  placement). This avoids the flicker races of one-shot resize
  protocols. The **geometry verbs** propose through the machine —
  maximize fills the workspace area minus the insets the decoration
  mode reserves, fullscreen covers the whole output with zero insets
  and the output pinned; the un-verbs return to the **restore
  point**, the position the window held when server geometry began.
  The **visibility verbs** apply immediately (the shell's own visual
  decision): a minimized or off-space window renders nowhere, takes
  no input, parks its frame requests (App Nap's seam), and leaves
  its outputs over the wire; `workspace_changed` reports the clamped
  actual. The mint's initial configure is the machine's own first
  proposal under the *seat's interaction clock* (the data family's
  `set_selection` reference) — the machine adopts the serial and
  continues the domain itself; the handshake carries the insets the
  decoration mode reserves (CSD's system hit zone, the `ssd`
  doctrine). `ack_configure` is strict: a non-live serial is the
  protocol error the contract names — with one bounded softening
  (Phase 50): the interactive resize supersedes proposals at the
  operator's hand speed, so the last 8 superseded *drag* serials stay
  acknowledgeable (the grace window — the client acking the configure
  it actually saw is never at fault); every verb proposal clears the
  window, the strict doctrine whole for them.
* **The operator's hand** (`toplevel.start_move`/`start_resize`,
  Phase 50 — the interactive drag): the client asks the *compositor*
  to drive the geometry under the seat's freshness gate. A **move**
  is server truth — every pointer motion batch re-anchors the window
  at the input pump's cadence (no proposal, no configure: the client
  never learns positions it cannot use), the keep band holding 48
  logical px of the title grip reachable. A **resize** rides the
  same two-phase commit the verbs do: the edge algebra (engaged edges
  follow, opposite corner anchors) proposes at the pointer's cadence
  with the `resizing` state riding, and the position realizes
  together with the client's committed buffer (never tearing). A
  geometry-stated window dragged by its title **demotes** — the
  floating size restores under the pointer's proportional grip, the
  shrink realizing at the client's cadence. The input pump owns the
  drag's heartbeat (§14's own seam — the input pump advances it); the
  deaths sweep the grip.
* **Decorations** (the SSD pass served as of Phase 52): server-side by
  default — and now *drawn*, matching the macOS-inspired visual identity
  of LionOS. The configure insets were always the reservation; the chrome
  pass paints what they reserve: the title bar (28 logical px), the
  border ring (1), and the close affordance (a warm capsule carrying a
  white ×) — CPU ink on the dock's own model (no framebuffer, the honest
  `NoFb` demotion: a visible band pins the frame to the composite arm),
  cached per frame *shape* (stateless ink — geometry is its only input).
  One geometry answer — the *applied* configure's insets grown around
  the content — feeds the render layer, the damage ledger, and the
  input pump's ring hit test. The close affordance's release asks the
  client to close (`toplevel.close`, the frozen event's first sender):
  the press arms consumed (the client never learns a press on pixels it
  does not own; the band's press focuses its own window), the release
  inside the same button fires, a drag away cancels — the caption
  doctrine every desktop serves; the client destroys the toplevel "when
  ready" (the server never force-kills). A maximized SSD window's frame
  fills the workspace area (the content inset by the applied chrome —
  every Phase 49 CSD pin unchanged). Clients may opt for client-side
  decorations at creation; the shell then applies the LionOS decoration
  *metrics* (shadow margins, hit zones) so CSD apps still land on the
  system grid — and the server draws nothing for them (their buffer's
  top is their own chrome).
* **Popups** (`ldp.popup`): anchor/gravity geometry with slide/flip constraint
  solving, pointer grabs with implicit dismissal, menus/submenus.
* **Dialogs** (`ldp.dialog`, served as of Phase 48): transient windows
  attached to a toplevel, centered server-side over the parent's content
  area at first attach (`Dialog::center_over`, clamped into the usable
  area), with the machine's own two-phase size commit (a stale
  `ack_configure` serial is the protocol error the contract names).
  **Modal** dialogs gate input delivery to their parent's whole tree —
  the routing view filters the parent root and its popups while the
  dialog is mapped, and the dialog's mapping commit retargets the
  parent's keyboard focus to itself; modeless dialogs gate nothing. A
  dialog cannot outlive its parent: the parent's death closes it (the
  sheet-dies-with-window doctrine). A surface may take at most one
  role — the second claim is refused by name.
* **Spaces** (workspaces, served as of Phase 49): per-seat ordered
  lists; toplevels may be sticky or moved between spaces (`set_workspace`
  clamps into the count and reports the actual through
  `workspace_changed`; the shell bind answers `workspace_count` —
  four by default, `--workspaces N`). (macOS-style "Spaces"
  semantics were chosen deliberately for LionOS identity. The
  operator-side *switching* — which space the seat views — is the
  taskbar-era roadmap line: the wire has no request.)
* **Stacking & focus:** a global stacking tree (spaces → toplevels →
  (popups | dialogs | subsurfaces)); per-seat focus with activation
  attribution (the *cause* of focus changes is recorded for a11y and audit).

## 9. GPU & rendering architecture

### 9.1 Backends

| Backend | Status | Role |
|---|---|---|
| software (scalar Rust, format-complete) | **implemented (Phase 8)** | CI, headless, DRM-less fallback, the pixel oracle |
| EGL/OpenGL ES 3.x | next (Phase 9) | full compositing path on Mesa/AMD/Intel/NVIDIA |
| Vulkan | planned post-v1 via the same `Renderer` trait | explicit-sync-native, compute compositing |

The `Renderer` trait (`ldp-renderer`) is the contract: `begin_frame(output,
damage)` → `submit(back-to-front layers)` → `end_frame()` → statistics.
Each layer carries a `BufferView` (data + validated geometry + stable
identity for the future GL texture cache), the output-space destination
(scale folded in), the buffer transform, its color description and an
opacity factor. The GL backend will add fences and plane handoff through
the same shape.

The software backend is a damage-clipped **scanline compositor**. Per layer
the renderer resolves one of four pixel paths:

* **copy** — 1:1 untransformed opaque X-family buffer onto the same output
  format: a row word-copy with forced alpha (the 4K single-surface EC lives
  here: 3–4 ms median on the 2-core CI box, gate < 8 ms),
* **opaque** — no-alpha formats at full opacity: sample and write, never
  reading the destination,
* **alpha** — premultiplied integer `over` in the target's *encoded* space
  (pixman/GL-fast-path equivalence: fixed-point `mul255` with round-half-up,
  bit-stable everywhere, no libm),
* **pipeline** — cross-description composites decode to scene-linear, blend
  linear, re-encode.

Sampling is **format-complete**: all eleven v1 fourccs (7 RGB + 4 YUV),
full and studio ranges, P010 10-bit, with the YCbCr matrix *derived from
the surface's primaries chromaticities* (one principled path for BT.709 /
DCI-P3 / BT.2020) and integer chroma pivots (128 / 512) so neutral
gray/black/white anchors decode byte-exactly. All eight buffer transforms
have exact integer 1:1 inverses; scaled layers use the continuous
pixel-center inverse with nearest sampling. Writes land exactly on damaged
pixels: per row, the destination is intersected with every damage rect and
the intervals are sorted and merged, so overlapping damage never
double-blends and undamaged framebuffer content survives partial
re-submission.

The **color pipeline** (Phase 8's hook, refined by `ldp-color` in Phase 14)
resolves per layer: range decode → transfer decode (sRGB, gamma 2.2/2.8,
PQ/ST 2084, HLG) → primaries conversion (XYZ-derived 3×3) →
reference-white anchor (PQ's absolute nits renormalized so source white
lands on output white; BT.2408-style anchoring) → clamp. Identical
descriptions skip all of it. Direct-scanout eligibility
(`scanout_candidate`) is the scheduler's cheap pre-filter: fullscreen,
untransformed, output-sized, opaque region covering the destination,
identical format and color description.

Channel order follows the DRM fourcc convention (named order is the u32
field order, memory little-endian — byte-identical to `wl_shm`, which the
bridges require), and the framebuffer stores premultiplied alpha. The
golden-image suite pins reference frames for every core format byte-exactly
(RGB family) and to ±1 LSB with definitional anchors (YUV, where derived
coefficients meet rounded standard constants); transfer curves carry
tolerance anchors because only they touch libm `powf`.

The GL backend renders through one pipeline per blend configuration with a
single Y-flipped pass into KMS framebuffers, reusing the same sampling,
color-pipeline and scanout-eligibility contracts.

### 9.2 GPU device management

`ldp-gpu` discovers render nodes (`/dev/dri/renderD*`), matches them to KMS
card nodes via `DRM_IOCTL_VERSION` + PCI bus IDs, opens render nodes
**unprivileged** for client-side buffer allocation (libEGL on the render node)
and the card node as DRM-Master (via logind) for modesetting. Multi-GPU: one
`render-N` thread per GPU; buffers imported cross-device via DMA-BUF with
explicit fences; the copy path is selected only when import is impossible.

*As built (Phase 9):* discovery runs `drmGetDevices2` behind a two-symbol
dlopen layer; the selection policy is a pure function over the catalog —
the render node of the first device that has one, the primary node only as
a DRM-Master fallback. DMA-BUF imports carry `DmaBufDescriptor`s validated
against the format's plane algebra (explicit modifiers only) and encoded
into the exact `EGL_EXT_image_dma_buf_import(_modifiers)` attribute list.
PCI-bus matching and the multi-GPU render threads land with the Phase 10
compositor, where the consumers exist.

### 9.3 Synchronization

Explicit sync is the only sanctioned handoff: client commits carry an acquire
fence (sync-file FD or a syncobj DMA-BUF + point); the compositor releases
buffers with a release fence after last use. Implicit DMA-BUF dependency
tracking is treated as a compat-only path for foreign (Wayland/X11) clients
inside the bridges, never on the LDP wire.

*As built (Phase 9):* the model lives in `ldp-gpu::sync` — one `Fence` type
across the three provenances (owned sync-file descriptors, timeline points,
and `ldp-display`'s mock tokens, which map by value so the crates share no
dependency edge), AND-merges ready when their latest member is, and the
object-safe `SyncDriver` trait with a virtual-clock mock. The EGL side
resolves `eglDupNativeFenceFDANDROID`/`eglWaitSync` through
`eglGetProcAddress` at load; their wait paths activate in Phase 10.

### 9.4 Hardware composition

*As built (Phase 34, v0.10.1):* the plane engine
(`ldp-planes` + the compositor's `plane_session`) grades every layer of the
frame's stack for plane eligibility — a kernel framebuffer behind it (the
import walk: pool fd → `drmPrimeFDToHandle` → `AddFB2WithModifiers`, cached
per buffer with honest refusals), 1:1 untransformed in-bounds placement at
full opacity in the output's own color, unstyled (corners/shadows/frost
exist only in the render path), and a (format, modifier) some plane's
`IN_FORMATS` takes. The split-point solver then maps the eligible *suffix*
of the stack onto the CRTC's planes (primary + overlays, zpos-monotone,
backtracking over per-layer capability sets, overlay-budget-fitted), with
the composite *prefix* rendering into the canvas on the primary beneath
them — a fullscreen opaque client takes the **zero-composite** shape (its
own framebuffer scans out, the renderer emits no pass). Each frame's atomic
commit carries the whole plane state: assignments on with zpos, retired
overlays off, `PAGE_FLIP_EVENT|NONBLOCK` as always. Every composite layer
carries a named demotion (the ledger: `NoFb`, `Styled`, `Scaled`,
`Transformed`, `ColorMismatch`, `OutOfBounds`, `Translucent`,
`NeedsBackdrop`, `FormatUnsupported`, `BelowSplit`, `BottomNotCovering`).
The scheduler's deadline grid, release gates, and presentation feedback are
unchanged — the flip the planes ride is the same flip. Full detail:
`docs/gpu-compositing.md`.

### 9.5 The Liquid material language

*As built (Phase 27, deepened by Phase 40 / v0.10.7):* the *system* owns
the materials (the macOS rule) — clients draw content, the compositor adds
the look, and nothing in the protocol carries it. A `LayerStyle` is what
the system adds around a layer's pixels: an antialiased rounded-corner
radius, a soft shadow (a blurred silhouette, memoized as *imagery* per
(params, geometry)), a frosted backdrop (the region under a translucent
layer blurred and tinted before the layer draws over it), and — since
Phase 40 — the **edge light**, the luminous 1-px hairline traced inside
the rounded silhouette (the coverage difference against the silhouette's
own 1-px Minkowski erosion, so the ring follows the corners on the same
antialiased curve the ink clips with; drawn *after* the ink, memoized like
the shadow in `EdgeMemo`).

The frost's saturation dial spans the **vibrant domain** (`u16 0..=510`):
`0..=255` desaturates towards BT.601 luma (255 = identity, the Phase 27
semantics byte-frozen), `256..=510` *boosts* each channel away from its
luma by `(saturation − 255) / 255` of its own luma-distance —
integer-exact, rounded half away from zero — the acrylic/vibrant distance
the giants' backdrop blur carries. The vibrant order is blur → boost →
veil, every rounding named and oracle-pinned.

The vocabulary is the **material family** (`ldp_renderer::Material`):
Panel (an opaque window), Sheet (a translucent surface), Menu (a popup —
the vibrant frost plus the hairline), VibrantDark (the control-center
glass), Chrome (the dock — frost, shadow cleared, the gentler hairline).
The compositor's policy resolves every surface through the family; the
legacy pair is the Phase 27 policy verbatim, and `Minimal` serves every
material plain (the library default's bytes frozen). The menu's shadow
parameters equal the sheet's at every tier — the damage-equivalence
constraint: the repaint spread is a function of the shadow alone, so the
damage expansion and the layer assembly compute identical effect rects
whatever each knows about the surface's role.

Both backends draw the identical material bytes: the software path
writes the memoized words through the shared damage-clipped
`apply_material` (the hairline as a post-ink stage), and the GL stream
draws the same words as textured quads (the hairline as the after-ink
quad) — byte-equality by construction, pinned by the randomized
cross-backend corpus and hand-computed golden oracles.

## 10. Compositor architecture

### 10.1 Scene graph

`ldp-compositor` (Phase 6) authors the tree as a flat surface map plus
per-parent back-to-front stacking lists: `roots → subsurface → …` (the
`output → space → (toplevel|dialog|popup)` levels arrive with the shell in
Phase 7; the scene graph treats roots uniformly). Children render above
their parents; later siblings above earlier ones. Surface state (buffer,
transform, scale, input/opaque regions, color, HDR metadata, presentation
mode) accumulates in a pending state and applies **atomically** on commit,
with the subsurface sync cascade: sync-mode subsurface commits *stash*
(merged over an earlier stash) and release when an ancestor applies;
desync subsurfaces and roots apply immediately and flush their sync
descendants. `set_position` rides the same cascade. The render path never
touches the authoring tree: `SurfaceTree::snapshot()` captures an immutable
`Arc`-shared `FrameSnapshot` at frame boundaries — structural sharing keeps
captures cheap, old snapshots stay alive and unchanged across later
mutations (mutation-storm-tested), and the back-to-front draw list is the
exact reverse of the damage engine's front-to-back flatten.

*As built (Phase 10):* `binaries/lion-compositor` drives exactly this
path end to end — the dispatcher splices `attach` into the pending
state, `commit` runs the cascade and feeds the scheduler, the pump's
render pass takes the changes, runs the damage engine, snapshots, and
composites through the reference renderer with a damage-scoped
background clear, and the double-buffered scanout chain delivers the
frame through `PAGE_FLIP_EVENT` atomic flips whose landing timestamps
become `presented` events. A pristine root converts to the subsurface
role by remove-and-reinsert (no state exists to lose); mapped
surfaces refuse the conversion.

### 10.2 Damage tracking

Damage is defined by a **normative per-pixel fold model**: a composited
pixel is a bottom-to-top fold over the covering surfaces where translucent
surfaces contribute and an opaque surface (per its `set_opaque_region`
guarantee) wipes everything below it; a pixel must be repainted iff its
fold value changed. `DamageEngine::compute` realizes that exactly through
four rule families over the `ldp-core` region algebra:

* **content** — accumulated surface-local damage (explicit rects; a
  different buffer with no explicit damage damages everything),
  translated through the node chain and minus the current occlusion;
* **coverage** — old-only cells through the previous frame's occlusion,
  new-only cells through the current one; a *move* (offset change)
  additionally repaints the interior (every interior pixel now shows a
  shifted content cell) minus the cells occluded in both frames, while
  a geometry-only change needs just the symmetric difference;
* **opaque flips** — the old ∆ new opaque footprint, only where a
  surface below can be revealed or hidden;
* **restack pairs** — subtree-footprint intersections of the mover and
  the crossed siblings (a restack moves whole subtrees), flushed when
  the front-to-back walk enters the front-most participant's subtree so
  the subtracted occlusion is exactly the opaque set above the whole
  crossed range.

Removed subtrees are the coverage rule with an empty new side. The
compositor maintains *three* damage classes — repaint damage (compositor
rendering), scanout damage (the repaint subset inside direct-scanout
candidates: fully-visible, untransformed, fully-opaque root surfaces
entirely on the output), and per-surface presentation damage (the visible
content change clients are told about) — all clipped to output bounds.
A randomized corpus (5 seeds × 80 frames of create/commit/move/restack/
destroy churn) proves exact per-pixel equality against an independent
reference implementation of the fold model, every frame.

### 10.3 Deadline frame scheduler (the heart of LDP)

Per output, the scheduler (`ldp-compositor::scheduler::FrameScheduler`,
roadmap Phase 7) is an event-driven state machine over timestamped inputs
— flip observations, client `frame` registrations, commits, visibility and
mode changes, park/resume. It never reads a clock itself:

```
vblank k-1 ──▶ flip IRQ (ts) ──▶ schedule(k) ──▶ render window opens
     render window: [ts, deadline(k))
 deadline(k) = predicted vblank(k) − submit_cost − flip_latency
 commit deadline hit? ─── yes ──▶ atomic commit targeting vblank(k)
      └── no (miss) ──▶ escalate: drop to vblank(k+1); frame_dropped event
                             with reason so clients can adapt
```

* **Prediction** rides the `ldp-core` first-order PLL: predicted vblank =
  last flip ts + refresh, with an integrated correction that converges to
  the *per-period drift* (a panel truly running slower than its mode pulls
  the whole grid onto the true period — the correction doubles as the
  drift term for multi-vblank extrapolation, so deep targets stay exact at
  lock). Discontinuities (stalls, modesets) re-anchor hard. The `presented`
  feedback carries the measured interval between the last two plausible
  flips, not the nominal mode.
* **The frame contract:** one live registration per surface
  (`frame(frame_id)` → `frame_target`, carrying target vblank, deadline,
  refresh, budget, mode). A commit strictly before the deadline (or before
  `deadline + vrr_window` under adaptive mode) satisfies it and binds it
  to that vblank; every registration terminates in exactly one event —
  `presented`, or `frame_dropped` with reason
  (`deadline_missed`/`superseded`/`surface_hidden`/`output_off`;
  `throttled` is reserved for the Phase 15 VRR policy engine). Late
  content still becomes live state silently — the client is told, and
  adapts.
* **Pipeline depth is policy:** `latency = depth × refresh` (default depth
  1; immediate mode = "commit at ready" with tearing marked in the
  `presented` flags — the KMS immediate-flip path arrives with the display
  backend in Phase 9).
* **The per-surface budget floor (Phase 47):** the registration walk's
  admission predicate is `max(config.min_commit_lead_ns,
  profile.budget_ns())` — the surface's claimed scene profile floors the
  minimum remaining commit time a deadline must offer before the walk
  retargets the next vblank. `Gaming` floors at 1 ms (the tightest
  makeable admission — latency through reliability: a gaming client that
  cannot make the imminent flip gets the makeable one, because a missed
  frame is a full period of latency); `Creative` floors at 4 ms (stable
  pacing over single-frame latency — the editor's doctrine); `Desktop`
  adds nothing — the operator's configured policy *is* the desktop
  doctrine, byte-identical with the pre-Phase-47 scheduler. A live
  registration keeps the contract it was answered with (the `set_mode`
  doctrine); the replay vocabulary records the change (`SchedInput::
  SetProfile`, codec tag 8, backward-compatible).
* **Escalation ladder:** a miss streak widens the deadline
  (`extra_lead = min(streak/escalate_after, max_extra_lead)` extra
  vblanks), so systematically slow clients are handed deadlines they can
  meet; de-escalation is hysteretic (only after `deescalate_hits`
  consecutive presentations), so a client sitting between two pipeline
  depths settles instead of oscillating.
* **Under VRR (§14)** the deadline widens into a window `[deadline,
  deadline + vrr_window]`: commits inside the window still make the
  target frame. The refresh-frequency *selection* (commit-at-ready
  self-refresh) is the Phase 15 policy engine's job; the scheduler's
  window semantics and their tests are already in place. Phase 39
  completes the picture twice over: the **presentation clock** learns
  the probed window (the measured-interval validity band *is* the
  panel's `[min, max]` — the LFC fast end and the stretched slow end
  are both honest single-period landings, so `presented.refresh`
  reports the cadence the panel actually ran instead of falling back
  to the nominal step whenever a landing left the legacy
  half..1.5×band), and an **adaptive-mode** surface under a fully
  probed window targets the panel's next refresh *opportunity* — the
  earliest in-window landing, `last_landing + lead × min` floored at
  the request — rather than a nominal grid cell the event-driven
  panel may already sit behind. The verdict fires at the flip that
  carries the content (`presented_at` = the honest landing), and the
  deadlines pace at the fast end while the client keeps up: the frame
  pacing follows the content, which is the point of adaptive sync.
* **Determinism is structural:** no clock reads, integer arithmetic
  only, ordered per-surface iteration, every input carrying its
  timestamp. The replay harness (`ldp-compositor::replay`) records a
  session — config, nominal refresh, input stream, and the outputs they
  produced — into a self-describing checksummed binary artifact and
  replays it to the identical event vector; `ldp-debug` (Phase 18) will
  consume exactly this format.

### 10.4 Event coalescing classes

The emission path between compositor and clients
(`ldp-compositor::coalesce::CoalescingQueue`) enforces the policy table
under dispatch pressure:

| Class | Policy under pressure |
|---|---|
| input | never dropped, never reordered (backpressure) |
| position state (`pointer.motion`, `pointer.frame`) | the latest value replaces the still-pending one *in its slot* (the oldest pending position); the discrete barrier seals it |
| drag state (Phase 50: the live resize's `configure`s) | the freshest target replaces the still-pending one in its slot — the grip is position state of the toplevel object; the final proposal is plain input, never dropped |
| presentation (`frame_target`, `presented`) | coalesce to latest per surface/kind |
| configuration (`configure`, output events) | coalesce to latest per key |
| data (offers) | never dropped (small, rare; backpressure) |

Coalescing replaces an older still-queued item with the newer one carrying
the same key — the surviving value keeps the *latest* arrival position, so
cross-class ordering stays arrival order. Per-frame terminal signals
(`frame_dropped`) are unkeyed and never replaced. When a coalescing class
itself hits capacity the oldest queued item of that class is evicted:
under pressure, stale presentation/configuration state is expendable,
stale input never is. Control messages are deliberately absent — they are
barriers in the dispatch lanes (§9), not queue content.

**Position state** (Phase 39) is the class with different replacement
geometry: the survivor overwrites the *oldest* pending slot in place —
its order relative to every other queued event never moves. A 1000 Hz
device parking sixteen samples between display frames delivers one
`pointer.motion` carrying the freshest coordinates (absolute samples are
replaceable by construction — the X11 `MotionNotify` doctrine), every
`relative_motion` delta (the delta stream is plain input — full
fidelity for raw-input consumers), and one `frame` terminator: a
client draining at frame cadence reads *one* wake per frame whatever
the device's rate. Discrete events (buttons, enters, leaves, axes)
seal the pending sample at push time — the discrete event's context is
the position it rode with, never a fresher one (the X11
flush-before-button doctrine, expressed as a freeze). The served
outbox (`lion-compositor::outbox`) is this table's live consumer: the
class is declared where the semantics are known (the emission sites),
and the unclassified default keeps the exact pre-Phase-39 FIFO.

**The drag's own row** (Phase 50): the interactive resize's live
`configure`s are the grip's position state — a pointer-paced flood
collapses to the latest target per display-frame wake (the 1000 Hz
doctrine, applied to the window's size instead of the pointer's
coordinates), while the machine's grace window keeps every
*delivered* superseded serial acknowledgeable (§8): the two layers
carry the same cadence honestly, one from the delivery side, one
from the acknowledgment side. The release's final proposal is plain
input — never dropped, never reordered after the freshest
intermediate.


### 10.5 Animations & transitions

Animations are *sampler functions of the frame clock* evaluated by the
compositor during the render pass, never timer-driven repaint loops.

**The transitions catalog** (`ldp-compositor::transitions`, Phase 47) is
the system-level motion vocabulary: nine kinds — window open/close,
workspace change, app switch, fullscreen, display connect/disconnect,
lock/unlock — each a `TransitionSpec` naming its spring curve after the
macOS motion grammar (critically damped for state changes, gently bouncy
for the playful moves). A [`Transition`] is one live instance: a spring
integrated in fixed substeps at whole-millisecond driver timestamps, so
the curve is a pure function of the timestamp sequence — bit-reproducible,
the scheduler's determinism doctrine applied to choreography.

The **v1 driver** serves the window-open fade. The mapping commit (the
first commit that maps the surface) begins it; the pump advances every
live transition once per wake at the driver's clock (the dock's own
advance doctrine); the damage pass claims each live transition's surface
rectangle (the vacate rule — a fade moves every pixel under it); and the
render path reads the host's opacity at layer assembly — the *one*
opacity truth feeding both the plane solver's facts (a translucent layer
composites; there is no v1 plane alpha) and the renderer's layer. A
settled transition is *removed* at the advance that settled it: the
settled frame renders at the default opacity (exactly 1.0),
**byte-identical with the never-animated one** — every pixel oracle the
equivalence corpora pin keeps its meaning. The serve loop's poll cadence
self-wakes a desktop whose clients all sleep (`pump_animations` under the
reserved `ClientId::SERVER` identity — the system wake parks every
emission in its owner's outbox, exactly as a client wake would route it).

**The close fade** (Phase 48) is the catalog's second driver: a window
leaving the desktop — destroyed by the client, or unmapped by a detach
commit — fades out over an **owned ghost**: a row-tight copy of its last
committed raster, its frozen style/color/transform, and its **z slot**
(the ghost inserts at the window's own render-order index — a fading
window never jumps above the windows that were above it). The ghost is a
compositor-owned layer in the dock's own pattern (owned ink, a borrowed
view, `NoFb` facts — never a plane offload), and its removal claims are
the host's own (`GhostHost::vacated`): a ghost is nobody's surface, no
client frame economy re-renders its region, so the settle frame's
repaint must be claimed by the advance that removed it — or the ghost's
last ink stays on the canvas while the hardware planes re-assign around
the hole. The settled desktop is the plain post-destroy bytes (the A/B
byte oracle); the same eligibility rules as the open hold (popups
dismiss instantly, ephemeral roles leave no ghost, the library default
is off).

The library default is **off** (`--transitions` opts the choreography in)
— the byte-exactness doctrine; popups and surfaces claiming ephemeral
roles (tooltip, overlay) appear instantly (the menu doctrine: a menu that
fades in has already failed its user). The geometry springs (the slide,
the genie) ship their curves in the catalog; their drivers ride the
render-thread roadmap line. The workspace/app-switch/fullscreen/display/
lock kinds are the named line behind them: the states machines
(`ldp-shell`'s `Spaces`/`Toplevel`) are served (Phase 49) — the
*drivers* that would animate a workspace switch or a fullscreen
engagement are the render-thread line's own. A states-arm-hidden
window (minimized, off-space) leaves no ghost when it dies: it had no
ink on the canvas to fade.

### 10.6 Idle & power hooks

`ldp-power` idle stages (active → dimming → dpms-off → suspend-hint) feed the
scheduler: on dimming, animations pause but the timeline stays armed; on
dpms-off, the scheduler parks until resume.

### 10.7 The semantic scene (Phase 47)

The architecture's own identity: **a display server that manages a
semantic, security-aware, GPU-optimized scene rather than merely
compositing application buffers.** Three claim families ride the shell
protocol (appended after the frozen opcodes, the Phase 45 additive
doctrine), carried in the scene's side-maps, and enforced at the seams
where each has teeth:

* **Semantic role** (`set_semantic_role`) — what the surface *is*: the
  transitions catalog's eligibility (ephemeral roles appear instantly)
  and the security floor (the lock role floors the class at
  `protected`, whatever the client claims beneath).
* **Security class** (`set_security_class`) — what the surface may
  *expose*: the capture path's ladder (§16.6). `Normal`/`Private`
  capture; `Protected`/`System` redact — the compositor is the
  enforcement point *because* it controls what becomes visible.
* **Scene profile** (`set_scene_profile`) — how the machine spends its
  frame budget on the surface: the scheduler's per-surface admission
  floor (§10.3), fed under the same lock as the claim lands (the wire
  claim and the slot's enforcement — two truths, one breath).

**The adaptive scheduler's priority ladder** — input latency, the
active application, animation, video, the cursor, compositor effects,
background applications — is not one knob; it is the name for
mechanisms that exist, one per rung, each honest:

| rung | the mechanism that enforces it |
|---|---|
| input latency | the deadline contract itself (`target_vblank − submit_cost − flip_latency`) and `PresentationMode::Immediate` for tearing-tolerant surfaces |
| active application | the per-surface budget floor (this phase): a `Gaming`/`Creative` surface holds admission rights the background does not |
| animation | the transitions catalog: compositor-owned motion claims repaint at the pump's cadence |
| video | the VRR window and the refresh-opportunity targeting (Phase 39) |
| the cursor | the reserved cursor plane in every output's inventory |
| compositor effects | the Liquid tier resolution — the quality budget the server owns |
| background applications | the occlusion quiescing (Phase 45): a fully-occluded surface's frame requests park unanswered |

**The composition decision engine** (`ldp-planes::decision`) makes the
frame's path a first-class vocabulary: `CompositionPath` — `DirectScanout`
(the client's buffer is the panel's), `HardwareOverlay` (the
fullscreen-video shape on a format-refusing primary), `SplitComposition`
(canvas prefix, overlay suffix — the Windows MPO underlay),
`FullComposition` (the canvas is the frame) — and the `DecisionRecord`
carries the shape counts with the demotion ledger's first *cause*
(BelowSplit entries are consequences, not causes; the record says so).
The same classification the frame loop has always acted on, now named
for the operator, the tools, and the tests to read.

The LionOS display architecture's ten pillars map onto this build as:
the one graphics authority (§1, the single `World` under one lock);
scene-first composition (§10.1–10.2 — the tree, damage, occlusion);
semantic surfaces (this section); the adaptive frame scheduler (§10.3
+ the ladder above); partial-scene rendering with the decision engine
(this section + §9.4); creative modes (the scene profiles); the
security-aware compositor (§16); the native multi-display scene (§19 —
one global scene, per-output delivery state); compositor-owned
transitions (§10.5); and LDP itself — the protocol whose nine modules
carry surface, scene, input, frame, workspace, display, color,
security, a11y, clipboard, and capture as first-class vocabulary.

## 11. Display (DRM/KMS)

`ldp-display` wraps atomic modesetting:

* **Objects:** connectors (with EDID identity: make/model/serial), CRTCs,
  planes (primary/overlay/cursor with format/modifier caps), encoders,
  modes (including preferred + panel self-refresh modes).
* **Atomic commits:** full pipeline state in one `DRM_MODE_ATOMIC` call;
  TEST_ONLY commits pre-validate plane assignments (§9.4); page-flip events
  timestamp the timeline (§10.3).
* **Hotplug:** udev monitor → reprobe connectors → registry add/remove events
  + workspace reflow. Modeset changes are seamless (double-buffered KMS state
  committed at vblank boundaries only).
* **Per-output properties:** VRR enable, max bpc, HDR metadata (HDR1
  InfoFrame via `Colorspace`/`HDR_OUTPUT_METADATA` props), color pipeline
  (degamma/CTM/gamma) for fixed-function color management.
* **Backlight & DPMS:** sysfs backlight with clamped ramps; DPMS through
  atomic active property, coordinated with `ldp-power`.

*As built (Phase 9):* all of the above exists behind the object-safe
`KmsBackend` trait with two interchangeable implementations — the
deterministic `MockDevice` (the headless-CI vehicle: injected-clock
timelines, kernel-check-order commit validation, hotplug injection) and
the `dlopen("libdrm.so.2")` `DrmBackend` (declarative requests with
name-addressed properties, typed 20-reason rejections, per-CRTC
heap-stable `OUT_FENCE_PTR` slots). udev hotplug implements the
`HotplugSource` seam over `dlopen("libudev.so.1")`; `Reprobe` computes
the topology diffs the registry layer will consume. Backlight ramps are
monotone integer interpolations with round-half-away-from-zero; raw
panels clamp at the platform floor. The gamma/CTM/HDR property surfaces
ride the generic blob/property machinery — their typed models are
Phase 12 color work.

**The mode foundry (Phase 42, v0.10.9).** The "every display size"
row's named remainder — the giants' decades of EDID/timing quirk
coverage — answered with arithmetic instead of a quirk table. Four
pieces, each pure and each pinned:

* **`timing.rs`** — the VESA CVT reduced-blanking synthesis
  (`cvt_rb(SynthRequest) -> Mode`): the digital-panel doctrine (the
  160-pixel compact blank, the 3-line vertical front porch, the
  4-line sync, the 460 µs minimum vertical blanking period solved
  *exactly in integers* — `vblank ≥ ceil(460·f·vactive/(10⁹ −
  460·f))`, floored at the structural minimum). The clock doctrine
  is exactness: `ceil(htotal·vtotal·refresh/10⁶)` in integer
  kilohertz, never *under* the asked refresh (a slower mode steals
  frame deadlines; the VESA tool's 0.25 MHz grid under-serves —
  `cvt -r` lands 1080p RB60 at 59.93 Hz, the foundry at
  60.000087). The canonical anchor is pinned: the 2080×1111 totals,
  bit-exact. Degenerate asks (zero axes, axes beyond the 16-bit
  mode wire, refresh past the solve's denominator) are typed
  refusals, never truncations.
* **`edid.rs`'s timing half** — the detailed timing descriptors
  decoded to [`Mode`]s (the first DTD is the sink's preferred
  timing, marked like the kernel marks it; the DTD wire's own 10 kHz
  clock granularity is stated, not hidden), and the monitor range
  limits parsed (the declared pixel-clock ceiling). The audit
  (`edid::audit`) names the three classes a deterministic rig can
  prove — a rotten block, a preferred-timing lie, a declared
  ceiling the enumerated list exceeds — each mapping to its quirk
  row's escape.
* **The pour's two gates** — the connector's EDID range limits gate
  the pour before it is ever proposed (`SynthError::ClockCeiling`
  names the declaration; never a silent clamp), and the display
  engine validates the `USERDEF` mode again at commit time against
  its declared synthesis envelope (the mock's gate mirrors the real
  driver's atomic check — `ModeType::USERDEF` is the kernel's own
  vocabulary for operator-minted timings, the `drmModeAddMode`
  lineage). The pour then joins the *protocol face's* advertised
  mode list (`OutputGlobal::adopt_mode` — `xrandr --addmode`'s
  story, protocol-side): the KMS list itself never learns it — the
  mode stays a user-defined timing on the wire, never laundered
  into a sink-offered one.
* **The pour rides every selection** — bring-up (single and
  multi-output, each connector pouring its own timing under its own
  ceiling) *and* every hotplug migration (a refusing connector falls
  back to the unsized doctrine — the sized doctrine's honesty,
  verbatim). The CLI grammar is `--synth WxH[@HZ]` (refresh
  defaults to 60), mutually exclusive with `--resolution` (the
  offered list vs the pour — two different asks).

The honest remainder, named: the GTF and CVT standard-blanking
families (legacy analog sinks) and CVT-RBv2's 80-pixel blank (the
DisplayPort deep-color era) are roadmap lines; every digital panel
the mock inventory models serves RB timings.

## 12. Color & HDR

* **Model:** every surface carries a `ColorDescription` (primaries, transfer,
  range, mastering luminances) created from parameters or an ICC profile (v2
  and v4 matrix/TRC profiles parsed by `ldp-color`; parametric-first). Every
  output advertises its color pipeline and HDR capabilities.
* **Pipeline:** per-surface → scene-graph blending in linear light (scRGB
  float internally) → per-output tone map (BT.2390 rolloff for SDR targets;
  inverse-PQ → panel PQ for HDR targets) → gamut map (perceptual clipping
  with soft-knee) → fixed-function KMS LUTs or shader tail.
* **HDR metadata:** static HDR10 (mastering display, max CLL/FALL) carried
  per surface and forwarded to the output InfoFrame; dynamic (ST 2094) is a
  capability-gated extension path. `ldp-hdr` computes the luminance
  adaptation (SDR-in-HDR canvas level, sdr_nits reference) matching the
  LionOS appearance pipeline.
* **Precision rule:** all wire color values are f32; internal compositing is
  fp16 minimum; PQ encodes via the exact integer 10/12-bit ramps (no float
  rounding drift) — tables generated and tested in `ldp-color`.

**As built (Phase 14).** `ldp-color` is the canonical math crate: the six
transfer curves (f32 pairs bit-identical to the renderer's Phase 8 hook —
a cross-consistency suite pins them together forever, plus the f64 PQ
reference every table and the BT.2390 EETF evaluate through), the
XYZ-derived primaries matrices (anchored against the published IEC/ITU
values), the exact integer PQ ramps (`decode(encode(l))` returns the same
integer code with zero float drift — the f32 curve pair is documented as
the approximation, the table as the authority), luma-preserving gamut
mapping (a hard-clip mode with the bit-exact in-gamut identity fast path,
and the soft-knee perceptual mode: chroma-ratio smoothstep compression,
continuous across the gamut edge, monotone), the BT.2390-structured EETF
(PQ-domain normalization, 75% knee, a C¹ Hermite rolloff whose end slopes
satisfy the Fritsch–Carlson monotonicity condition — below a 3:1
PQ-normalized compression ratio, where any slope-1-at-knee cubic is
provably non-monotone, a smoothstep branch takes over), and the ICC
import: v2/v4 matrix-TRC profiles parsed to an f64-exact model (PCS D50
values de-adapted through the profile's own `chad` inverse, or the
Bradford D50→D65 inverse for v2), resolved to the closest parametric
description (the `color_profile.description` contract), and re-serialized
by a canonical deterministic writer (byte-stable round-trips — the
exit-criterion fixture engine). The output tail
(`OutputTransform`) composes tone-map scale → reference-white
normalization → gamut map → transfer encode, with absolute-nits inputs
(HDR chains natively; SDR chains anchored by `ldp-hdr`'s canvas first —
never a hidden constant).

`ldp-hdr` is the policy layer above it: HDR10 static-metadata validation
and normalization (CTA 0-unknown sentinels preserved, FALL clamped to
CLL, the PQ ceiling enforced), the CTA-861.3 HDMI HDR Static Metadata
InfoFrame codec (30-byte packet, checksum self-verifying, parse-back
round-trips), the luminance adaptation (the SDR-in-HDR canvas level —
BT.2408's 203-nit default — monotone in input and canvas, the
exit-criterion property; plus BT.2100's peak-dependent HLG system gamma
`1.2 + 0.42·log₁₀(Lw/1000)` with the published anchors pinned), and the
output-mode decision table (SDR / PQ / HLG from output caps + the
surface stack, PQ winning over HLG as the interchange EOTF) with
dwell-based hysteresis so the panel mode never flaps. Known-answer
vectors (independent spec transcriptions + published SMPTE/ITU anchors)
cover every formula; luminance-adaptation monotonicity is fuzz-proven
over randomized corpora.

## 13. VRR

`ldp-vrr` owns the policy: off / deadline (window mode) / always (lowest
achievable latency, still tear-free). It bounds the refresh window around
the panel's min/max, applies flip-slip avoidance (never two flips closer
than the minimum refresh interval), and interfaces with the deadline
scheduler (§10.3). Tearing is a separate opt-in (async flip) never implied
by VRR. The policy engine is pure, deterministic, and table-driven over
(panel caps, surface presentation modes, battery state, user preference).

* **As built (Phase 15):** the crate is six modules over `ldp-core` +
  `ldp-compositor` (with `ldp-display` as a dev-dependency for the
  panel-model integration suites). `caps` validates the output's
  support — the `[min, max]` panel window (millihertz wire conversions
  with the rate↔period inversion handled) plus the `vrr_caps` bits
  (seamless, fixed_rate) — and enforces the kernel's structural
  invariant that the nominal mode period sits inside the window.
  `policy` is the decision table: one pure function over policy ×
  support × adaptive demand × battery saver, every row carrying a
  machine-checkable rationale; it produces the effective window and
  the scheduler widening — `max − nominal` under deadline (exactly
  the stretch a late commit may consume: a commit inside the widened
  window still presents, at its own stretched flip), the full
  `max − min` span under always. `window` owns the arithmetic,
  including `select_flip`, the flip-slip avoidance rule
  `clamp(max(ready, last + min), last + min, last + max)` — the same
  formula the mock KMS timeline implements, pinned against it by the
  integration suite. `refresh` is the `RefreshSelector`: the per-output
  latency optimizer whose rulings (`FlipAt` / `Defer` / `Late`) turn
  ready timestamps into flip slots — sequencing off the one
  outstanding flip like the KMS queue — with the fixed-rate fallback:
  below-minimum-rate frames under deadline policy defer onto the
  nominal grid with a retry hint, surfacing as the reserved
  `frame_dropped(throttled)` reason. `tearing` is the gate —
  immediate-mode surface ∧ async-flip capability ∧ session
  permission — deliberately blind to VRR state. `engine` composes
  them per output: state changes emit `WindowApplied` (the embedder
  programs `VRR_ENABLED` and the bounds through the next atomic
  commit; the initial state is queued at construction), deferrals
  emit `Throttled` with the retry grid point, and
  `scheduler_config()` installs the live widening into the
  `SchedulerConfig`, so the deadline scheduler and the panel agree on
  the same window. The async-flip seam itself landed in `ldp-display`
  as `CommitFlags::PAGE_FLIP_ASYNC`. Exit criteria: the exhaustive
  36-row table conformance walk; scheduler+VRR golden timelines
  including the mock-KMS panel loop (rulings match the device's flip
  completions to the nanosecond; `presented` feedback carries the
  measured VRR interval); and tearing proven isolated from VRR at the
  gate, scheduler, and KMS-seam levels (byte-identical immediate-path
  event vectors with and without the window).

* **The quirk ledger (Phase 41, v0.10.8):** the comparison row's own
  named remainder — "the multi-year driver quirk table, not the
  mechanism" — answered with the structure the decades fill. Three
  pieces, one doctrine: the operator tells the system the truth
  about their panel, the system clamps *its own scheduling* to that
  truth, and the hardware's own claim is never rewritten. **The
  honest floor** (`quirk.rs`): `apply_floor` clamps the advertised
  window's stretch ceiling to `1e9/floor_hz` (the slip floor stays
  the panel's own — the operator narrows the stretch they do not
  trust, never the panel's capability); the compositor runs the pass
  at bring-up *before any consumer*, so the scheduler's widened
  deadline, the `output.vrr` advertisement, and the LFC cadence all
  read the effective window, while the mock's device-side timeline
  keeps the advertised bounds — the override lives in the display
  stack above the driver, exactly where the giants' tables live.
  `FloorOutcome` (passthrough / below-advertised / clamped with both
  nanosecond bounds) is the audit trail the selftest's `vrr:` line
  prints per output; `AboveNominal` (the floor exceeds the mode's
  own rate — the fixed grid would leave the window) refuses boot
  rather than silently ignoring a lie. The grammar is `--scale`'s
  (`--vrr-floor 57` blankets; `57,0` is per-output with the `0`
  passthrough; extras reuse the last entry). **The LFC cadence and
  latch** (`refresh.rs`): `lfc_interval` computes the giants'
  low-framerate-compensation arithmetic — content at period `P`
  below the window bridges on `k = ceil(P/max)` repeats at `P/k`
  apart, phase-locked so every content flip lands exactly on a
  repeat slot (judder-free by construction); a window too narrow to
  bridge declines (`P/k < min`) and the miss ruling stands. The
  latch is the anti-flap dwell: engaged by the first missed window,
  held through boundary-hugging flips (within `min` of the ceiling —
  *not* recovery), cleared only by a comfortable landing; content
  hovering at the boundary rides warmed repeats instead of
  alternating stretch/repeat. **The sibling escape** (the
  compositor's bring-up): a mixed desktop — one output with a
  window, one without — under `--vrr-uniform` collapses to one
  uniform fixed sync across the seam (no output arms, no widening,
  the collapse only biting when the seam is real); per-output VRR —
  the modern per-display doctrine — stays the default. The quirk
  table accrues the rows' names (`vrr-floor-flicker`,
  `vrr-sibling-flicker`, five deep now, each with symptom,
  detection, escape, and cost — a row is a public commitment).

## 14. Input & seats

* **As built (Phase 11):** `ldp-input` implements the evdev wire codec
  (24-byte `input_event`, `SYN_REPORT` framing with drop/protocol-A
  flagging), the device model (`EVIOCGBIT` capability bitmaps, ABS axis
  ranges, hint-first classification), the multitouch slot machine
  (kernel protocol B with tracking IDs and drop resynchronization),
  the unit-honest normalizer (0..1 axes, mm from resolution, fuzz
  dead-zones on pressure, tilt hundredths-of-degree → radians,
  autorepeat filtering — repeat is modeled server-side), the two-tier
  smooth-curve pointer acceleration (latency-first: one event of
  history, C¹-continuous at both tier boundaries, monotone and
  bounded by property tests), the touchpad gesture machines (swipe /
  pinch / hold with uniform ending semantics, two-finger scroll
  emulation, scroll→pinch conversion on divergence, single-finger
  pointer strokes), key repeat schedule arithmetic, keymap compilation
  through `dlopen("libxkbcommon.so.0")` (RMLVO, v1 serialization with
  the caller-owned string freed, modifier decomposition with the
  header-verified state-component constants, sealed-memfd client
  descriptors), and the `/dev/input` backend with typed headless
  results (empty enumeration, `NotEvdev` for non-evdev descriptors —
  exercised against a memfd in CI). `ldp-seat` implements seat
  management (udev-tag device assignment with duplicate refusal,
  capability recompute on every change), subpixel input-region hit
  testing with the three focus domains (pointer follows hit tests,
  keyboard is shell-driven, touch is grab-driven), the grab model
  (implicit while buttons hold, explicit popup grabs, capability-gated
  keyboard grabs, dismissal on surface death), and the per-seat router
  whose golden corpus proves the full pipeline: byte-literal evdev
  traces → wire-encoded protocol events.
* **Backend:** raw evdev via `/dev/input/*` (opened through logind device
  grants), ABS/MSC/SYN decoding, multitouch slots, pressure/tilt/tool
  normalization for tablets, LED state writeback. libinput compatibility is
  a *consumption* guarantee (same event semantics), not a dependency.
* **Processing:** per-seat pointer acceleration (two-tier smooth curve,
  latency-first), wheel/touchpad gesture state machines (swipe/pinch/hold),
  button/scroll emulation, keymap via xkbcommon (loaded from the shared
  keymap FD handed to clients verbatim), key repeat modeled server-side.
* **Routing:** focus is a per-seat stack; pointer focus follows the input
  region test (rect regions with subpixel coordinates), keyboard focus is
  shell-driven, touch focus is grab-driven. Grabs: pointer (implicit during
  buttons, explicit via popup), keyboard (rare, capability-gated).
* **Injection:** synthetic input (automation, remote control, a11y) is
  *not* a core path — it requires the `input_inject` capability token and is
  audited per injection batch.
* **Multi-seat:** seats are registry globals; devices are assigned by udev
  seat tags; two seats never share focus, clipboard, or spaces. Each seat
  gets its own keyboard map, pointer, and data device.
* **The operator's hand (Phase 50):** the input pump is the interactive
  drag's heartbeat. `toplevel.start_move`/`start_resize` (§8) mint the
  grip under the seat's freshness serial; every motion batch that
  follows advances it — the move's geometry applies at the pump's
  cadence (server truth, `set_position_now`, the R2 damage rule
  repainting both ends), the resize's edge-algebra target proposes
  when the clamped size changed (§10.4's drag row carries the
  delivery cadence, §8's grace window the acknowledgment cadence; the
  drawn chrome's grip rides the same pump — the band's press consumed
  before the router ever sees it, its release firing the frozen
  `toplevel.close`, §8's SSD pass) —
  and the button's release retires the grip (the resize's final
  proposal, the move's silence). The pump reads the router's own
  freshest position — the same position state the routed motion
  serves, the window's travel exactly the pointer's travel, whatever
  the accel curve did with the batch. The drag dies with its window,
  its role object, its client, or a superseding grip (the death
  sweeps; one hand).

## 15. Clipboard & data exchange

`ldp-clipboard` implements `ldp.data`: data sources (MIME offer lists),
per-seat data devices (selection, primary selection, DnD), offers with
action negotiation (copy/move/ask), and streaming transfers over pipe FDs
(the server never buffers payloads larger than a page — clients stream). The
selection is bound to seat focus; clipboard *read* is manifest- or
escalation-gated (§16). DnD is a state machine (enter/motion/drop/leave)
driven by the seat's pointer, with the drag icon as a special surface type.

**As built (Phase 13).** MIME strings validate to an RFC 6838 subset
before any storage, canonicalize (lowercased tokens, sorted
parameters), and match with `text/plain` charset folding: the
ASCII-compatible family (`utf-8`/`utf8`/`us-ascii`/`ansi_x3.4-1968`)
compares equal, everything else is exact. The seat holds two unified
slots — clipboard and primary — with serial-bound setting, ownership
eviction (the previous owner is `cancelled`), owner-only null clears,
and publication to every non-owner device client (owners never see
their own content announced). DnD action negotiation: the drag starts
with all actions available, the entered receiver narrows with
`data_offer.set_actions`, the server re-announces the current set to
the source (`data_source.actions`), and at drop the compositor picks
copy over move over ask from the narrowed set — announcing the
singleton to the source (its last `actions` event carries the
negotiated action; `dnd_finished` takes no argument) — while an empty
narrowed set cancels the drag outright. One offer per drag, reused
across leave/re-enter; every abort path funnels through a single
cancel choreography (`cancelled` + `leave` + offer death exactly
once). Transfers are *statefully* pumped: one page per pump, a
would-blocking sink keeps its pending tail (a stateless step provably
loses bytes), the transfer registry admits under the client FD budget
with check-then-commit receives (a rejected receive leaves no
trace), and the ordering is gate → validate → admit → `send`. The
permission split (§16): reading clipboard/primary data requires the
`clipboard_read` scope; *setting* the selection needs no scope
(ownership is not a read); DnD drop receives are user-intent
authorized. The manager is pure policy over opaque client/seat/source
keys returning routed event batches; the single audited libc seam is
`pipe` (pipe2/fcntl/read/write — the `ldp-input` sys precedent).

## 16. Security architecture

Layered, deny-by-default (full adversary model in `threat-model.md`) —
delivered as `crates/ldp-security` (as-built):

1. **Manifest baseline.** Apps ship a manifest (`manifest.rs`: app_id
   charset, requested `ScopeSet`, sandbox flavor, canonical form +
   SHA-256 manifest hash). The broker verifies it at launch; the
   baseline it derives is exactly the matrix's manifest-class scopes —
   prompt-class and prompt-only scopes never baseline.
2. **The permission matrix** (`matrix.rs`): every threat-model §3 row
   as an `Operation` with its requirement class (Free / Manifest /
   ManifestAndPrompt / Prompt / Impossible). `decide(op, manifest,
   effective)` answers Allow / Prompt / Deny; `manifest_baselines`
   computes the connect-time baseline. The 18-row × 3-state exhaustive
   walk is the Phase 16 exit criterion.
3. **Runtime escalation** (`broker.rs`): the `SessionBroker` is the
   only grant authority — manifest gate, per-app rate limiter, the
   brokered user prompt through the `PromptDecider` seam (TCC-style;
   tests script it), TTL'd or session-bound token minting. Every
   decision is audited.
4. **Tokens** (`grant.rs`): 256-bit tokens from the `TokenSeed`
   entropy seam (deterministic splitmix64 in tests, OS randomness in
   deployment), stored in the grant table with app binding, expiry
   (boundary-exact), revocation, and per-connection submission
   bookkeeping. Validation walks the whole table with constant-time
   byte compares: forgery, cross-app replay, and revoked resubmission
   all deny with `invalid_token`; same-app resubmission (reconnect) is
   legal by design.
5. **Audit** (`chain.rs`): SHA-256 in pure Rust (`sha256.rs`, NIST
   vectors pinned), hash-chained records (seq, ts, client, app, scope,
   action, detail), bounded ring with a window anchor, `verify` that
   recomputes the retained window and reports the first break, class
   filtering (bridge folds into the captures class), and JSONL lines
   for the Phase 18 `ldp-audit` reader. Tampering with any field,
   reordering, truncating, or forging records fails verification.

The server core *enforces* (scope checks before argument validation,
`unauthorized` + audit on denial) but never *decides* grants; the
broker is hostable in its own process without touching this logic.

6. **The capture-path enforcement (Phase 47, the security-aware
   compositor):** every surface carries a security class — the default
   `Normal`, or a claim (`toplevel.set_security_class`) mapped onto the
   ladder. `Protected` and `System` (and any surface whose semantic role
   is `lock` — the floor applies whatever the claim says) **redact out
   of every client-visible capture frame**: the rectangle paints opaque
   black in the `capture_manager.grab` output while the display itself
   keeps scanning the content — the user's own eyes see their window,
   the screenshot does not. The compositor is the right enforcement
   point precisely because it owns the visible composition; the class
   maps onto the security module's `Scope::ProtectedSurface` (the
   "render into protected (HDCP-class) content" scope the wire has
   carried since v1). The `Private` tier captures locally and
   classifies for the screen-share negotiation — the broker-era line,
   honestly documented, never a silent stub. The proof is the A/B/A
   byte-equality in `semantic_session.rs`: claim → black; clear → the
   pixel-exact bytes restored.

## 17. Accessibility

`crates/ldp-accessibility` (as-built):

* **Settings** (`settings.rs`): the feature-toggle bitset (the exact
  `a11y_settings` wire bits — bit 2 unassigned by the spec), text
  scale (Q8, 1.0x–4.0x) and cursor size (1–256 px) with range
  validation, snapshot diffing, and the `SettingsBus` broadcast —
  every bound client receives the same snapshots; late binders join at
  the current state; queues coalesce to the newest under bound
  (settings are state, not events).
* **The event bus** (`bus.rs`): providers (the `a11y_provider`
  objects) push announce/focus/caret/state events; assistive
  technologies subscribe through the `a11y_control` scope gate
  (deny-by-default). Delivery is a stable merge in global-sequence
  order — per-provider FIFO is preserved exactly, attribution is
  push-time (a later focus change never rewrites an earlier
  announcement's), and per-provider queue overflow drops the oldest
  and counts it (a flooding provider cannot block the bus). The
  5,000-event LCG corpus with interleaved dispatches pins the
  ordering contract (the Phase 16 exit criterion).
* **Magnifier** (`magnifier.rs`): lens/content geometry with Q8 scale
  (1x–16x, 0 = keep), follow selectivity (focus/caret/pointer/center —
  unfollowed sources never move the lens), lens clamped inside the
  output, zoom steps at 4/3 and 3/4 with clamping.

Keyboard access *processing* stays in ldp-input (the settings model
carries only the enabled state); magnifier *rendering* is the
compositor's output pass (this crate owns control state and geometry).

## 18. Power & session integration

`crates/ldp-power` + `crates/ldp-session` (as-built):

* **Idle stages** (`ldp-power::idle`): the active → dimmed → off →
  suspend ladder with logind's delay semantics — `blur`/`display`/
  `idle` inhibitors each hold their transition; release fires it at the
  next tick; activity resets with wake events; long gaps cascade
  through overdue stages. Emits data events (`IdleEvent`) the host
  maps to session events, DPMS, and backlight actions.
* **Backlight** (`ldp-power::backlight`): one ramp path for the dim
  stage and the slider — 16 steps at 20 ms, monotone convergence with
  exact landing, mid-ramp retargets continue from the current level.
* **Suspend/resume** (`ldp-power::suspend`): the sleeping/resumed pair
  with sleep kinds and the suspended-duration report; `ReAnchor::
  grid_align` computes the first phase-preserving vblank boundary
  after the wake — feeding it as the first post-resume flip to the
  compositor's frame clock re-anchors the PLL with zero phase error
  (proven against the real `FrameClock` at 144/90/60/40 Hz across
  sleep depths — the Phase 16 exit criterion).
* **D-Bus minimal client** (`ldp-session::dbus`/`conn`): the wire
  codec implemented from the specification (no external binding) —
  message framing, header `a(yv)` with fields 1–9, the full basic +
  variant/array/struct/dict marshaling with exact alignment,
  bounds-checked parsing that never panics; golden bytes pin the
  canonical Hello and TakeControl forms. The connection state machine
  (serial allocation, reply matching, EXTERNAL auth preamble) rides
  the `DbusTransport` seam.
* **The logind session** (`ldp-session::logind`): TakeControl /
  TakeDevice (fd + inactive), PauseDevice (pause/force/gone reasons,
  ack owed only for `pause`), ResumeDevice with the fresh fd,
  Lock/Unlock, PrepareForSleep — all as typed events; the device lease
  table tracks fds across pauses.
* **VT switching** (`ldp-session::vt`): pause → release DRM master +
  ack → inactive → resume → reacquire device → active, with stray
  events ignored by state.
* **Lock screen** (`ldp-session::lock`): Locking (surface-mapping
  window with an optional force-blank deadline for broken lockers) →
  Locked (the focus gate admits only lock surfaces) → Unlocked; only
  the lock surface may request unlock (anything else is refused);
  logind's administrative Unlock signal tears it down from outside.
* **Inhibitors** (`ldp-session::inhibit`): the cookie registry —
  per-connection cookies whose union drives the effective mask;
  connection teardown releases everything they held. The bit contract
  is pinned against ldp-power's mask by a cross-crate conformance
  test, and the registry's output drives the idle machine end-to-end.

## 19. Multi-monitor

Outputs are independent timelines (per-CRTC schedulers) with a global
monotonic anchor for cross-output animation sync. Layout (positions,
rotation, per-output scale as rationals — never a global mixed-DPI lie),
per-output refresh, per-output HDR state and per-output color pipelines are
first-class. Hotplug reflows spaces by output affinity; windows keep their
relative position within a space when an output disappears (they migrate,
never die). Cursor crosses outputs through the KMS cursor plane handoff —
pointer motion is never blocked on output transitions.

## 20. Compatibility bridges

Both bridges are separate user-session processes, delivered as pure protocol
machinery (sockets, mapped segments and the keymap fd are process-binary
seams; neither crate reads a clock or holds `unsafe`):

* `ldp-x11-bridge` — a pure-Rust X11 *server* subset: the full core request
  table (window tree with exact visibility/expose diffs and gravity-retained
  backing stores, properties, atoms, selections, focus and pointer routing
  with crossing details, the whole GC/drawable vocabulary over a
  GX/plane-mask/clip rasterizer with arcs and polygons), BIG-REQUESTS
  framing, and MIT-SHM over a host seam. One screen, one TrueColor visual —
  the exact shape of LDP's XRGB8888, so window stores need no conversion.
  The rootful driver composites the window tree into the root store on
  every structural or drawing change and re-attaches it as one LDP
  toplevel's buffer with exact damage; rootless per-window mapping is the
  stretch seam (`Server::top_level_windows`).
* `ldp-wayland-bridge` — a pure-Rust Wayland *compositor* subset: the wire
  codec (24-bit object ids + 8-bit opcodes, NUL-terminated padded strings,
  ancillary fd slots), the registry replay over the four globals it serves
  (compositor, shm, seat, xdg_wm_base), double-buffered `wl_surface`
  commits under the xdg configure/ack handshake, seats with the
  monotonic-serial discipline and the frame-grouped pointer dialect, and
  xdg-shell popups whose positioners translate one-to-one onto LDP's own
  anchor/gravity/constraint vocabulary (the superset — the mapping is
  total). Each committed foreign surface exports as one LDP surface +
  toplevel with exact damage.

Bridges hold a `bridge` scope token (audited) permitting: buffer import
without per-buffer grants, synthetic input for foreign clients, and window
management on behalf of their children. The token gate is a seam
(`driver::TokenCheck`) the process wires to its broker connection; the
suites wire it to the Phase 16 grant table with a 32-case single-bit
forgery corpus, cross-app denials, and revocation.

**Synchronization doctrine.** Foreign protocols carry no fences: the
client's request stream *is* the synchronization. Both bridges process
every request to completion before emitting an LDP `commit`, so the commit
is an explicit readiness statement by construction — the LDP wire never
sees an implicit dependency. Foreign DMA-BUF (the future main-loop path)
attaches through the same host seam with fences minted there.

The LDP core contains zero compatibility code — verified by an
architectural lint (`ldp-wayland-bridge`'s `tests/architecture.rs`): no
core crate source or manifest references Wayland/X11 vocabulary, and no
core crate depends on a bridge.

*As built (Phase 17):* 11 modules + 5 EC suites in the X bridge (the
xeyes/xclock-class lifecycles byte-for-byte with pixel assertions, the
BIG-REQ/SHM transfer corpus, the full error-code corpus, the driver's
message stream and token conformance) and 6 modules + 6 EC suites in the
Wayland bridge (the weston-terminal lifecycle — map, keymap, type, resize,
close — the wire golden bytes plus the malformed corpus, the popup/positioner
translation table, the serial discipline, the driver's export stream and
token conformance, and the architectural lint).

## 21. Developer tools

`ldp-info` (protocol/server introspection: versions, outputs, seats, caps),
`ldp-debug` (live message tracer with schema-aware decoding via runtime
introspection), `ldp-validate` (spec + message conformance checker),
`ldp-profiler` (frame timeline visualization: deadlines, flips, misses,
render/submit costs), `ldp-audit` (security log reader/verifier),
`ldp-input-debug` (device + event stream inspector). All tools talk the same
protocol (debug scopes are capability-gated like everything else).

## 22. Protocol compiler — `ldpc`

`ldpc` parses `spec/*.toml`, validates (same rules as `scripts/spec_lint.py`
plus type-level checks), and generates: Rust schema tables for
`ldp-protocol` (opcode→signature maps, typed enums/bitsets, doc comments),
introspection blobs for runtime tooling, and markdown protocol reference
docs. Generated code is committed for reviewability; CI re-runs `ldpc` and
fails on drift.

## 23. Testing architecture

| Layer | Suites |
|---|---|
| unit | every crate, next to code |
| protocol conformance | generated from spec: round-trip encode/decode of every message, malformed-byte rejection, version matrix |
| transport | FD passing, credential checks, malformed frames, FD bombs, slow-client backpressure |
| compositor | damage engine corpus-verified against a per-pixel reference; scheduler determinism replay tests |
| renderer | golden-image pixel suites: all 11 formats vs independent decoders, hand-computed blends, forward-referenced transforms, color-pipeline anchors; 4K single-surface perf EC (release, 2-core) |
| input | recorded evdev event streams → expected protocol events (golden traces) |
| security | permission matrix (all scopes × all states), token forgery, revocation, audit integrity |
| integration | full server + mock clients in namespaces: crash (SIGKILL mid-message), stress (N clients × M surfaces), hotplug simulation |
| fuzz | seeded deterministic mutators over codec/transport/dispatch; runs in CI time budget |
| performance | benchmark harness with JSON output; gates: sub-µs message decode, sub-ms 4K composition, allocation-free steady-state dispatch |

## 24. Future directions (explicitly out of v1)

VR/AR (pose-tracked surfaces, multi-layer distorting composition — the
surface/color model already carries the needed extensibility), Vulkan
renderer, compute composition, offload rendering to a second GPU,
remote LDP (stream transport with the same message layer), AI-assisted
upscaling in the color pipeline. All attach as *modules or Renderer
backends*, never as core surgery.
