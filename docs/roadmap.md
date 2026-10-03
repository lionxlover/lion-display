# LDP Delivery Roadmap — 52 Phases

Each phase lands as one reviewable, buildable, tested chunk (delivered to the
user as `phase<N>.zip`; the final phase also produces the all-in-one bundle).
Exit criteria are hard: a phase is not complete until every box is green.

Legend: **EC** = exit criteria.

---

## Phase 52 — The drawn chrome *(delivered — see CHANGELOG 0.20.0)*

The gap-filling program's second release (the ledger's next named
line, one gap at a time): the **SSD chrome pass** — the drawn title
bar, border ring, and close affordance every server-decorated window
wears — and the **server-initiated close event** the frozen
`toplevel.close` has awaited since v1. Zero wire movement: the event
is already frozen; this phase gives it its first sender.

* **The drawn band**: the compositor paints the chrome its configure
  insets always reserved — the title bar (28 logical px), the border
  ring (1), and the close affordance (a warm capsule carrying a
  white ×) — as CPU ink on the dock's own model (no framebuffer, the
  honest `NoFb` demotion: a visible band pins the frame to the
  composite arm), cached per frame *shape* (stateless ink: geometry
  is its only input, same-shaped windows share one raster).
  CSD windows draw nothing (their buffer's top is their own chrome),
  fullscreen covers everything (zero insets, no band), and a window
  that never applied its insets draws nothing (the two-phase commit
  owns the reservation).
* **The geometry truth**: one answer — `chrome_geometry` — feeds the
  render pass (the layer), the damage ledger (the claims), and the
  input pump (the ring hit test); the applied configure is the
  insets' source, the primary's scale the paint's.
* **The close ask**: the drawn button's press *arms* (consumed — a
  client never learns a press on pixels it does not own; the band's
  press focuses its own window, the activated bit riding the
  proposal), its release inside the same button *fires* — the frozen
  `toplevel.close` emitted over the real socket, parked in the
  outbox exactly like every routed event — and a drag away cancels
  (the caption doctrine every desktop serves). The client destroys
  the toplevel "when ready" (the frozen doc's own words): the server
  never force-kills, the ignoring client keeps its window.
* **The claims ledger**: the damage engine knows only protocol
  damage; the band is the compositor's own ink around it. The ledger
  diffs every serving band's frame rect against the last-claimed one
  — a band that moved, resized, hid, or died claims its old rect, a
  band that came back or newly serves claims its new one (the R2
  rule's chrome sibling; minimize hides the chrome with the ink,
  unminimize restores the exact bytes).
* **The geometry regime**: a maximized SSD window's *frame* fills
  the workspace area (the content inset by the applied chrome — the
  band on-screen at every state; every Phase 49 CSD pin unchanged:
  a client-decorated buffer is its own whole footprint).
* **EC**: 6 `ldp-shell` geometry tests + 6 painter/palette unit
  tests + 8 `chrome_session` real-socket tests (the close narrative
  with the client's answer, the drag-away cancel and re-arm, the
  ignoring client, the band press, the CSD territory, the fullscreen
  cover, the pixel oracle with the minimize/unminimize byte
  restoration, the maximized frame fill) + every prior suite green
  (2,241 → 2,258); the freeze gates untouched (zero wire surface
  moved); all tiers green.
* **The honest remainders** (named, not hidden): the chrome-aware
  *placement* (the cascade places the buffer; a window parked at
  the usable origin wears its top band above the visible area until
  it engages geometry), the title *glyph* (the bar draws its shape;
  a font rasterizer is a feature of its own), the Liquid
  chrome-material dressing for the band (flat ink is this phase's
  look), the chrome *drag* (the drawn title bar is not yet a move
  grip — `start_move` remains the client's ask; the server-side
  caption drag is the follow-on), the chrome *ghost* (a dying
  window's content fades, its band leaves with the route), and the
  window-menu family (the band's right press).

---

## Phase 1 — Foundations (this deliverable)
Architecture + protocol design + repository.
* **EC:** `docs/` complete (architecture, protocol, spec-format,
  threat-model, roadmap) · `spec/` complete for v1 (8 modules) passing
  `scripts/spec_lint.py` · `ldp-core` implemented: IDs/versions/errors/wire
  values/geometry+damage algebra/timing/color+HDR types/buffer
  formats/capability bitsets/tokens/limits · `cargo fmt/clippy/test/doc`
  green, zero dependencies, zero `unsafe`.

## Phase 2 — Protocol compiler & wire codec
`tools/ldpc` + `ldp-protocol`.
* ldpc: TOML → validated model → Rust schema tables + enums/bitsets +
  introspection blobs + markdown reference docs.
* `ldp-protocol`: message encoder/decoder (header + tagged args + FD table),
  full validation pipeline stages 1–3, round-trip tests generated from spec.
* **EC:** `ldp-protocol` exposes every `spec/` interface as data; codec
  round-trips all messages; malformed corpus rejected at the right stage;
  committed generated code matches `ldpc` output in CI.

## Phase 3 — Transport
`ldp-transport`.
* Unix listener/connector; `sendmsg`/`recvmsg` with SCM_RIGHTS; SO_PEERCRED;
  FD hygiene (validate, close-on-reject); message framing reader/writer
  with limits; backpressure hooks; documented `unsafe` syscall blocks only.
* **EC:** FD passing round-trips 10k messages; malformed frames rejected;
  FD-bomb and slow-peer tests pass; zero leaks (FD count asserted).

## Phase 4 — Server core
`ldp-server`.
* Accept loop; per-client session state; generational object store; typed
  dispatch; error emission; resource ceilings; client-crash reclamation;
  audit hooks (interface points, no broker yet).
* **EC:** mock clients (raw codec) can hello/welcome/bind/create/destroy;
  stale-ID and type-confusion attacks rejected; 1k clients × 1k objects
  steady-state without growth; kill -9 mid-message leaves server healthy.

## Phase 5 — Client library
`ldp-client`.
* Connection + handshake; proxy objects; typed event dispatch with class
  lanes; sync/roundtrip; disconnect/reconnect helpers.
* **EC:** a test client completes a full session against `ldp-server`;
  event class latency property test (input never starved by presentation
  floods) passes.

## Phase 6 — Compositor core
`ldp-compositor` (scene graph, damage).
* Surface tree, atomic state commits, damage propagation with occlusion
  subtraction, region algebra integration, stacking/focus data structures.
* **EC:** damage equals reference model on randomized scene-graph corpus;
  occlusion tests; snapshot/immutability invariants hold under mutation.

## Phase 7 — Deadline frame scheduler
`ldp-compositor` (scheduler) + presentation events.
* PLL vblank predictor; deadline computation; depth policy; missed-frame
  escalation; presentation feedback emission; determinism replay harness.
* **EC:** simulated-timeline conformance suite (golden schedules) passes;
  deadline hit-rate property under jitter; event coalescing classes obeyed.

## Phase 8 — Renderer (software + GL skeleton)
`ldp-renderer`.
* `Renderer` trait; software backend (format-complete blending, transforms,
  color pipeline hooks); golden-image pixel suite in CI.
* **EC:** 4K single-surface composition < 8 ms (release build, 2-core CI);
  pixel-exact reference frames for all core formats.

## Phase 9 — Display & GPU *(delivered — see CHANGELOG 0.1.0-phase9)*
`ldp-display` + `ldp-gpu`.
* DRM/KMS atomic backend (dlopen libdrm): connectors/CRTCs/planes, modes,
  page-flip timestamps, hotplug (udev), VRR props, backlight, DPMS.
* GPU discovery, render-node EGL context (dlopen libEGL), DMA-BUF
  import/export, syncobj/sync-file plumbing.
* **EC:** headless KMS mock passes atomic-commit golden tests; EGL backend
  renders the software suite's frames bit-comparably (where representable);
  VRR enable/disable verified against mock props.

## Phase 10 — Example compositor (vertical slice) *(delivered — see CHANGELOG 0.1.0-phase10)*
`binaries/lion-compositor` headless + DRM modes.
* End-to-end: client window → commit → composite → present (headless
  snapshot mode + real DRM when present).
* **EC:** `tests/` integration suite: full session incl. buffer exchange and
  presentation events green in CI (headless).

## Phase 11 — Input & seats *(delivered — see CHANGELOG 0.1.0-phase11)*
`ldp-input` + `ldp-seat`.
* evdev backend; pointer acceleration; gesture machines; tablet transforms;
  xkbcommon keymaps (dlopen); seat/focus stacks; grabs.
* **EC:** golden evdev traces → protocol events; accel curves property
  tests; multi-seat isolation tests.

## Phase 12 — Shell *(delivered — see CHANGELOG 0.1.0-phase12)*
`ldp-shell`.
* Toplevel/popup/dialog machines; configure/ack protocol; SSD geometry;
  spaces; stacking; focus policy.
* **EC:** state-machine conformance from `spec/shell.toml`; fuzzed configure
  sequences never wedge the shell; SSD insets correct on mixed-DPI.

## Phase 13 — Clipboard & DnD *(delivered — see CHANGELOG 0.1.0-phase13)*
`ldp-clipboard`.
* Sources/offers/MIME negotiation; streaming; primary selection; DnD FSM.
* **EC:** transfer fuzz (random MIME/size/pattern) never deadlocks;
  100 MB stream via pipe under limits; permission gates enforced.

## Phase 14 — Color & HDR *(delivered — see CHANGELOG 0.1.0-phase14)*
`ldp-color` + `ldp-hdr`.
* Transfer functions (sRGB/PQ/HLG), matrices, gamut mapping, BT.2390 tone
  mapping, ICC (matrix/TRC v2+v4) import, HDR metadata plumbing.
* **EC:** known-answer vector tests (SMPTE/ITU references) pass within
  tolerance; ICC parse round-trip on fixture profiles; luminance adaptation
  monotonicity properties.

## Phase 15 — VRR *(delivered — see CHANGELOG 0.1.0-phase15)*
`ldp-vrr`.
* Policy engine (off/on/always), refresh windows, flip-slip avoidance,
  latency optimizer; integration with scheduler + KMS props.
* **EC:** policy table conformance; scheduler+VRR golden timelines; tearing
  opt-in verified isolated from VRR.

## Phase 16 — Security, a11y, power, session *(delivered — see CHANGELOG 0.1.0-phase16)*
`ldp-security`, `ldp-accessibility`, `ldp-power`, `ldp-session` + broker.
* Capability tokens, manifest policy, escalation prompts, audit chain;
  a11y bus + magnifier + settings; idle/DPMS/backlight; logind session
  (D-Bus minimal client), VT switching, lock screen.
* **EC:** permission matrix fully tested (all rows × deny/prompt/grant);
  token forgery/replay fails; audit chain verifies; suspend/resume
  re-anchoring test; a11y event ordering property.

## Phase 17 — Compatibility bridges (delivered)
`ldp-wayland-bridge` + `ldp-x11-bridge`.
* X11 server subset (core + BIG-REQ + SHM + GC/drawable ops); Wayland
  compositor subset (core + xdg-shell); both proxy into LDP; bridge scope
  tokens; implicit→explicit sync translation.
* **EC (met):** xeyes/xclock-class clients run via the X bridge in CI
  (Xvfb-free) — `tests/xeyes.rs`, `tests/xclock.rs`; weston-terminal-class
  Wayland clients map, type, and resize — `tests/weston_terminal.rs`; core
  architectural lint: no Wayland/X11 references in core crates —
  `ldp-wayland-bridge/tests/architecture.rs`, green.

## Phase 18 — Tools & examples *(delivered — see CHANGELOG 0.1.0-phase18)*
`ldp-tools` + binaries + examples.
* ldp-info, ldp-debug (live tracer), ldp-validate, ldp-profiler,
  ldp-audit, ldp-input-debug; examples: hello-ldp, pointer-paint,
  clip-client.
* **EC (met):** every tool runs against the Phase 10 compositor in CI
  (compiled binaries over the real socket — `tests/tools_live.rs`,
  including the dead-socket fast-fail); examples pass their scripted
  scenarios (`hello-ldp`: one committed frame, presentation verdict,
  pixel-exact scanout; `pointer-paint`: evdev → real normalizer →
  pixel-exact stroke; `clip-client`: the full in-process
  offer/accept/receive negotiation with a real pipe, plus the live
  honest-degradation report).

## Phase 19 — Test, fuzz, stress, benchmarks *(delivered — see CHANGELOG 0.1.0-phase19)*
`ldp-test`, `fuzz/`, `tests/`, benchmark harness.
* Conformance suites from spec; deterministic fuzzers (codec/transport/
  dispatch/X11/Wayland codecs); crash/stress/hotplug simulation;
  performance + security benchmarks with JSON reports.
* **EC (met):** fuzzers run clean for CI budget — the five targets run
  under one serialized gate with a counting panic hook across every
  thread, both accept and reject paths exercised, and process-wide FD
  stability asserted (`fuzz/tests/fuzz_ci.rs`); the stress gate ran
  the full 15 minutes — 32 clients, 30,056 sessions (4,909 abrupt
  disconnects reclaimed), 104,918 committed frame cycles, 129,819
  frames, canary-healthy, scene drained, FD baseline restored
  (numbers in `docs/benchmarks.md`); the benchmark report is
  committed to `docs/benchmarks.md` (release tables + JSON schema).
  Two real bugs the harnesses flushed out on their first runs, both
  fixed with regression suites: the Wayland bridge's length-bomb
  DoS (unbounded input buffering on a ~2 GiB length claim — now
  capped at one LDP large frame, `tests/length_bomb.rs`) and the
  compositor's post-mortem outbox leak (a crashed client's pending
  buffer release re-creating its outbox queue and leaking an eventfd —
  `Scene::drop_client` now clears pending releases).

## Phase 20 — Release v0.1.0 *(delivered — see CHANGELOG 0.1.0)*
Docs, packaging, polish.
* Full API docs; user/admin guide; Debian packaging + systemd units;
  milestone release; all-in-one bundle `lion-display-v0.1.0.zip`.
* **EC (met):** the Debian packages (`lion-compositor` +
  `ldp-tools`, debhelper-free rules under `Rules-Requires-Root: no`)
  build with `dpkg-buildpackage -us -uc -b` and install cleanly in a
  fresh root with real `dpkg` inside a user namespace, with every
  installed binary smoke-tested and the systemd unit passing
  `systemd-analyze --root=... verify` (`scripts/build-deb.sh
  --verify`, green for this release); `./scripts/verify.sh` green
  (fmt, clippy -D warnings, 1,576 tests, rustdoc -D warnings, spec
  lint, no drift — also re-proven from a fresh extraction of the
  bundle); CHANGELOG 0.1.0 + this roadmap entry + the maturity
  matrix (`docs/maturity.md`) written; user guide
  (`docs/user-guide.md`) and admin guide (`docs/admin-guide.md`)
  shipped, also packaged as `/usr/share/doc` documentation; final
  bundle `lion-display-v0.1.0.zip` delivered.

---

## Phase 51 — The focus and the view *(delivered — see CHANGELOG 0.19.0)*

The states arm's follow-ons, part one — the gap-filling program's
first release (the ledger's own discipline: one gap at a time, one
phase, one version). Two of the three named follow-ons land; the
third defers deliberately with its honest reason.

* **The focus truth**: the `activated` state bit (frozen at index 3
  since v1 — the machine's flag, the fuzz invariant, the dialog
  conformance story all waiting) rides real proposals. The one
  retarget seam drives it: the press sets it (click-to-focus), the
  departure clears it (a dialog's handoff, an unmap commit, a
  death), a non-toplevel holder takes no bit but the old toplevel
  still clears. The proposal is `propose_state` — the same
  derivation as the verbs', but the replaced proposal parks in the
  drag's grace window (a focus move the client never asked for
  never punishes a frame-cadence client — the `propose_drag`
  doctrine extended to the state seams).
* **The promotion doctrine**: the focus holder's departure
  promotes the frontmost remaining window (the topmost toplevel or
  dialog; popups are pointer-grab surfaces). One seam —
  `promote_focus` — behind the hidden discipline, the unmap
  discipline, the death sweeps, and the view switch's own arm.
* **The view switch**: `shell.switch_workspace(index)` + the
  `workspace_switched` report (one request + one event, appended
  additively after the frozen opcodes — the Phase 45/47/50
  doctrine, the freeze re-taken consciously: 98 requests + 123
  events, 37 enums). The clamp's honest answer reaches every shell
  binder (the asker directly, the others' outboxes); the
  visibility sweep is the set_workspace verb's own machinery at
  desktop scale (dialogs follow their parents — the sheet moves
  with its window); the keys land on the frontmost window *homed
  on the newly-viewed space* (sticky windows show everywhere but
  the switch's keys belong to the space's own windows; an empty
  space falls back to the frontmost visible); the hidden spaces'
  frame requests park through the occlusion quiescer; a switch to
  the viewed space is a full no-op.
* **EC**: 1 machine test + 4 `focus_session` real-socket tests
  (the press narrative, the dying holder's promotion, the
  view-switch narrative with the ink flip/sticky/parking/focus
  both directions, the clamp + broadcast + no-op) + the dialog
  gate's return grown its keyboard truth + the drag narratives
  grown the press's activation proposal; the freeze gate re-taken
  with the count pins updated; all tiers green.
* **The honest deferral**: the server-initiated `close` event (the
  third follow-on) rides Phase 52 with the SSD chrome pass — the
  drawn title-bar close button is its honest trigger, and it
  deserves its own version (the slowly-slowly doctrine the program
  named).

## Phase 50 — The operator's hand *(delivered — see CHANGELOG 0.18.0)*

The interactive move/resize vocabulary the spec never grew, served
whole — the gap ledger's own named line, the one feature every
desktop competitor serves (DWM's caption drag, WindowServer's title
drag, the Wayland compositors' `xdg_toplevel.move`/`resize`). Two
requests appended additively after the frozen opcodes (`start_move`,
`start_resize` with the `resize_edge` enum — the Phase 45/47 doctrine,
the freeze re-taken consciously: 97 requests + 122 events, 37
enums), the `resizing` state bit riding real proposals for the first
time since v1 froze it at index 5.

* **The move**: server-truth geometry at the input pump's cadence —
  every motion batch re-anchors the window (`set_position_now`, the
  R2 damage rule repainting both ends), zero configures (the client
  never learns positions it cannot use), the keep band holding 48
  logical px of the title grip reachable (the router's desktop
  clamp the first boundary, the keep band the second). The release
  ends the grip; the geometry stands.
* **The resize**: the edge algebra through the real two-phase
  commit — the engaged edges follow the drag's delta, the opposite
  corner anchors, the client's min/max grammar clamps, the
  `resizing` state rides every live proposal, and the position and
  the committed buffer realize together (never tearing: a left/top
  grip moves the origin at the realize). The release's final
  proposal clears the state. Refused on a geometry-stated window
  (the states own the size).
* **The demotion**: a geometry-stated window dragged by its title
  releases the states, restores the floating size the engagement
  displaced (captured at the verb, the tree's realized truth; the
  workspace's two-thirds the never-floated fallback), and detaches
  *now* under the pointer's proportional grip — the Windows 11 /
  macOS doctrine, the shrink realizing at the client's cadence.
* **The grace window**: pointer-paced supersession never punishes a
  frame-cadence client — the last 8 superseded drag serials stay
  acknowledgeable (the acked size realizes with the client's own
  committed buffer), every verb proposal clears the window (the
  Phase 49 strict-ack doctrine preserved, pinned). The delivery
  side carries the same doctrine: the drag's live configures are
  outbox position state (the §10.4 freshest-wins, a new kind tag),
  the final proposal never dropped.
* **One hand**: a fresh grip supersedes; the engaging verbs end a
  drag on the dragged window; the deaths (window, role object,
  client) sweep the grip away.
* **EC**: 5 machine tests + 5 host/algebra unit tests + 8
  `drag_session` real-socket tests (the move narrative with the
  pixel oracle and the zero-configure proof; the keep band; the
  resize narrative with the realize and the final proposal; the
  left-edge origin move; the graceful ack; the demotion; the typed
  refusals; the death sweep); the freeze gate re-taken consciously
  (97+122, 37 enums) with the count pins updated; all tiers green.

## Phase 49 — The states arm *(delivered — see CHANGELOG 0.17.0)*

The last honestly-consumed toplevel vocabulary, served: the frozen
17-request `ldp.shell.toplevel` surface, every request live, zero wire
surface moved (the schema from v1; `ldp-shell`'s `Toplevel` and `Spaces`
machines built and tested all along — this phase is the server wiring
they waited for, exactly the roadmap line that named them).

* **The host** (`ToplevelHost`, the DialogHost doctrine): every live
  toplevel object carries its `Toplevel` machine plus the two serving
  truths the machine does not own — the client's `get_toplevel`
  decoration choice (SSD or CSD: every proposal's insets derive from
  it — the wire argument was previously discarded) and the *restore
  point*, the position the window held when it first engaged a
  server-geometry state (the un-verbs return there; a chain of
  geometry verbs restores to the one point where server geometry
  began). The machine grows exactly one method: `propose_at` — the
  mint's handshake proposal under the *seat's interaction clock* (the
  data family's `set_selection` reference, Phase 32's doctrine
  untouched), the machine adopting the serial as its watermark so its
  own later proposals continue the domain without collision.
* **The handshake, upgraded honestly**: the initial configure is now
  the machine's own first proposal (previously hand-emitted) —
  serial 1 from the seat clock, empty states, the client's own size,
  and the *insets the decoration mode reserves* (CSD's system hit
  zone — the `ssd` module's own doctrine, so CSD apps land on the
  system grid; the previous all-zero insets were the Phase 31
  simplification). The ack of the handshake is accepted: the strict
  two-phase includes the mint — a well-behaved client that acks its
  first configure is never punished. The shell bind answers
  `workspace_count` (the spec's "after binding" — the shell global's
  first bind-time event): four spaces, the desktop doctrine
  (`--workspaces N` opts another count in; zero promotes to one, the
  machine's own rule).
* **The geometry verbs** (`maximize`/`unmaximize`,
  `fullscreen(output|null)`/`unfullscreen`): the intent lands on the
  machine, the proposal derives under the live policy (the usable
  area, the primary's logical size and scale, the LionOS metrics,
  the decoration, the home space) — maximize fills the workspace
  area minus the insets, fullscreen covers the whole output with
  zero insets and the output pinned (the client's bound object,
  validated by name; null resolves to the primary's). The client
  acks, and its next commit realizes: the buffer it attaches is the
  size answer, the position completes the placement
  (`set_position_now` — the migration arm's doctrine, the damage
  engine's R2 rule repaints both ends). Fullscreen precedes
  maximized (the machine's invariant, now observable over the wire:
  both flags ride the states, the geometry is fullscreen's).
* **The visibility verbs** (`minimize`/`unminimize`,
  `set_workspace`, `set_sticky`): the shell's own visual decision,
  applied immediately — a hidden window (minimized, or homed on a
  space the seat is not viewing; sticky windows show on every one)
  *renders nowhere* (the grade walk skips it, the vacate claims
  repaint the ink it left — the ghost host's own doctrine, now the
  hidden set's), *takes no input* (the routing view filters it, a
  hidden focus holds no keys — the leave rides the focus retarget),
  *parks its frame requests* (the visibility pass clears its output
  set — App Nap's own seam, the Phase 45 machinery reused exactly),
  and *leaves its outputs over the wire* (enter/leave_output, the
  release-gate truth). `workspace_changed` reports the clamped
  actual; the configure's workspace field carries it. Unminimize
  restores the exact bytes (the A/B/A byte oracle).
* **The hints and the handshake's teeth**: `set_title`/`set_app_id`
  land on the machine (the bounded-string doctrine's own error
  codes — the client's validator refuses the over-length before the
  wire, the server refuses the embedded NUL), the size pair
  saturates (a min above a max clamps to it — consistent by
  construction), and `ack_configure` is strict: a serial that is
  not the live proposal is `invalid_state` (the dialog's doctrine,
  now the toplevel's — superseding proposals kill the previous
  serial, so stale acks are detectable).
* **The death sweep**: the toplevel object's destroy drops the host
  entry and releases the scene's role mapping (the surface may
  outlive the role, roleless — a re-mint replaces both); the
  surface's death drops its entries, its spaces membership, and its
  hidden-set membership (a destroyed workspace resident leaves no
  residue).
* **EC:** 6 `ToplevelHost` unit tests + 3 `propose_at` machine tests
  (the handshake adoption, the domain continuation, the
  two-machines seeding shape) + 10 `states_session` tests over the
  real socket (the handshake under the seat serial with the CSD
  insets and the accepted ack; the maximize narrative with the
  cascade restore point, the derived size, the position realize,
  and the fill's pixel oracle; the unmaximize restore; the
  fullscreen cover with the output pin and the dock-band doctrine;
  the precedence; the stale-ack refusal; the minimize narrative
  with the frame park, the leave/enter truth, and the A/B/A byte
  oracle; the workspace move/clamp/report; the sticky doctrine; the
  hints with the error codes and the saturation); all tiers green,
  the freeze gate untouched (zero wire surface moved).

## Phase 48 — The knock and the goodbye *(delivered — see CHANGELOG 0.16.0)*

The frozen shell surface's two honest refusals, closed as mechanism —
the **dialog** a window's flow asks with, and the **close fade** a
window leaves with. Zero wire surface moved: `get_dialog`, the
`ldp.shell.dialog` interface, and the `dialog_modality` enum were
frozen in the v1 spec from the start, their policy machines
(`ldp-shell`'s `Dialog`) built and tested for years, waiting for the
server that finally drives them.

* **The dialog arm** (`shell.get_dialog` served): the surface takes
  the transient role attached to a parent *toplevel* (the window,
  not the object — a re-minted toplevel keeps its dialogs), the
  machine carries the modality and its own serial clock (the
  dialog's two-phase commit validates against the machine, not the
  seat's interaction clock — the acks must reference their own
  proposals). The initial configure proposes the client's own size
  (0x0, the xdg doctrine); the **centered placement** lands at the
  dialog's first attach — `Dialog::center_over` (the parent's
  content center, clamped into the usable area), riding the pending
  queue so the dialog never lands origin-then-jumps. A surface may
  take at most one role (the spec's clause, now enforced by name:
  toplevel, popup, dialog, subsurface each refuse the second
  claim); `set_title` serves the bounded-string doctrine (the
  NUL-embedded title is `invalid_string`, the over-length the
  client's own budget rejects before the wire).
* **The modal gate** (the spec's own clause): a *mapped* modal
  dialog gates input delivery to its parent's whole tree — the
  routing view filters the parent root and the popups rooted under
  it (the router routes roots; the gate is the membership the scene
  supplies, the router never guesses), the gated keyboard focus
  holds no keys (the dialog's mapping commit retargets the parent's
  focus to the dialog — leave + enter over the real wire), and the
  gate lifts the moment the dialog closes or dies. Modeless dialogs
  gate nothing — the control half of the same session.
* **The sheet-dies-with-window doctrine**: a dialog cannot outlive
  its parent — the parent surface's death closes every dialog it
  owns (`dialog.close`, freshly closed only, idempotent never a
  double event); the dialog's own surface death closes its machine
  the same way. The entry itself stays until the object destroy
  (the popup host's done-but-alive doctrine: a close-but-alive
  dialog keeps answering `ack_configure`).
* **The window-close fade** (the transitions catalog's close arm):
  a window leaving the desktop — destroyed, or unmapped by a detach
  commit — fades out under the catalog's close spring over an
  **owned ghost**: a row-tight copy of its last committed raster
  (the pool's mapping is the client's to reuse the moment the
  release lands — the copy is the only honest carrier), its frozen
  style, color, transform, and z slot. The ghost renders as a
  compositor-owned layer (the dock's own pattern: owned ink, a
  borrowed view, `NoFb` facts — never a plane offload), inserted at
  the window's own render-order index (a fading window never jumps
  above the windows that were above it — the z-fidelity proof pins
  it under a covering window). The damage pass claims the ghost's
  rect plus its frozen style's effect rect every frame, and — the
  load-bearing detail the z-fidelity test caught — **the vacated
  rects the settle drains**: a ghost is nobody's surface (no client
  frame economy re-renders its region), so the removal claim is the
  host's own (`GhostHost::vacated`), or the ghost's last ink stays
  on the canvas forever while the planes above it re-assign around
  the hole. The settled desktop is the plain post-destroy bytes —
  the A/B byte oracle against a transitions-off bench. Popups and
  ephemeral roles leave no ghost (instant dismissal is their
  contract); with transitions off (the library default) the destroy
  behaves exactly as it always has.
* **EC:** 6 `DialogHost` unit tests (the membership truth, the gate
  pairs, the close sweeps, the done-but-alive doctrine) + 7
  `dialog_session` tests over the real socket (the mint/ack/center
  pixel oracle, the second-role refusal, the stale-ack refusal, the
  NUL title, the modal gate's full narrative — motion gated, dialog
  self-input, the focus handoff, the gate's return — the
  parent-death close, the modeless control) + 5 `ghost_session`
  tests over the real socket (the destroy fade with the A/B
  byte oracle, the unmap fade, the off-by-default plain destroy, the
  z fidelity under a covering window, the popup/tooltip
  instant-dismiss); all tiers green, the freeze gate untouched
  (zero wire surface moved).

## Phase 47 — The semantic scene *(delivered — see CHANGELOG 0.15.0)*

The LionOS display architecture's own identity, closed as mechanism:
**a display server that manages a semantic, security-aware,
GPU-optimized scene rather than merely compositing application
buffers.** Four subsystems, each with real teeth and a real-wire
proof — none of them a Wayland compositor's vocabulary.

* **Semantic surfaces** (the identity claim): a surface may claim
  what it *is* (`toplevel.set_semantic_role` — window, dialog,
  tooltip, overlay, lock), what it may *expose*
  (`toplevel.set_security_class` — normal, private, protected,
  system), and how the machine spends its frame budget on it
  (`toplevel.set_scene_profile` — desktop, creative, gaming). The
  requests append after the frozen opcodes (the Phase 45 doctrine;
  the freeze consciously re-taken, 95 requests + 122 events, 36
  enums); `ldp-shell` grew the typed triple; the scene carries the
  claims in the `material_requests` pattern (one side-map, dying
  with the surface). The server keeps its own invariants: the
  ephemeral roles never animate in, and the **lock role floors the
  security class at protected** — the one claim a client cannot
  talk its way below.
* **The adaptive frame scheduler's budget floor** (the per-surface
  doctrine): the profile floors the registration walk's admission
  predicate — `max(config.min_commit_lead_ns, profile.budget_ns())`
  — `gaming` at 1 ms (the tightest makeable admission; latency
  through reliability), `creative` at 4 ms (stable pacing over
  single-frame latency, the editor's doctrine), `desktop` adds
  nothing (the operator's configured policy *is* the desktop
  doctrine — byte-identical with the pre-Phase-47 scheduler, every
  golden pinned). The `set_mode` contract doctrine applies (a live
  registration keeps the contract it was answered with); the
  replay vocabulary records `SetProfile` (codec tag 8,
  backward-compatible); the priority ladder each rung names is the
  module docs' enforced-mechanism table.
* **The composition decision engine** (the path as a first-class
  subsystem): `ldp-planes` grew the `CompositionPath` vocabulary —
  **direct scanout** (the client's buffer is the panel's), hardware
  **overlay** (the fullscreen-video shape on a format-refusing
  primary), **split** composition (canvas prefix, overlay suffix —
  the Windows MPO underlay), **full** composition (the canvas is
  the frame) — plus the `DecisionRecord`: the shape counts and the
  demotion ledger's first *cause* (BelowSplit entries are
  consequences, not causes — the record says so). Golden tests pin
  each path against canonical stacks.
* **The compositor-owned transitions** (the system-level motion):
  the catalog (`ldp-compositor::transitions`) names nine kinds —
  window open/close, workspace change, app switch, fullscreen,
  display connect/disconnect, lock/unlock — each with its spring
  curve after the macOS motion grammar (critically damped state,
  gently bouncy play). The v1 driver serves the **window-open
  fade**: the mapping commit begins it, the pump advances it at
  the driver's clock (the dock's own doctrine), the serve loop's
  poll cadence self-wakes a desktop whose clients all sleep
  (`pump_animations` under the reserved `ClientId::SERVER`
  identity — every emission parks in its owner's outbox), and the
  settled frame is **byte-identical with the never-animated one**
  (settle is removal; the terminal value is exact) — every pixel
  oracle the equivalence corpora pin keeps its meaning. The
  library default is off (`--transitions` opts the choreography
  in); popups and ephemeral roles appear instantly (the menu
  doctrine).
* **EC:** 6 semantics + 8 transitions + 6 scheduler-profile tests
  (the budget-floor walk pinned per profile, the replay harness
  round-trip, the identity proof) + 7 decision tests (the four
  paths + the cause ledger) + 7 session tests
  (`semantic_session.rs`: the A/B/A capture redaction proof, the
  lock floor, the profile's two-truths oracle, the role carry;
  `transitions_session.rs`: the fade's first-frame near-black
  start, the settle-to-exact-bytes oracle, the instant tooltip,
  the off-by-default identity); the freeze re-taken consciously
  (95 + 122, 36); all tiers green.

## Phase 46 — The foundry completes *(delivered — see CHANGELOG 0.14.0)*

The mode foundry's own named remainder, closed whole: every timing
family the VESA standards define — from the 1996 analog era to the
DisplayPort deep-color one — poured in exact integer arithmetic,
spec-faithful to the letter (the VESA CVT 1.2 and GTF 1.1 documents
themselves, fetched and followed formula by formula).

* **The family grammar** (`--synth WxH[@HZ][:family]`, default `rb` —
  Phase 42's pour unchanged, its pinned report strings
  byte-identical): `rb2` (reduced blanking v2, CVT 1.2 §3.4.3 — the
  80-pixel blank with 8/32/40 porches, the fixed 8-line vertical sync
  of Errata E2, the fixed 6-line back porch, and **1-pixel
  horizontal precision** so the 1366-class widths pour exactly), `rb2v`
  (RB2 with §3.4.3 item-1's 1000/1001 video-optimized multiplier —
  the 59.94 Hz class, geometry bit-identical, only the clock moves:
  the spec's own guarantee), `cvt` (standard "CRT" blanking, §5.3 —
  the GTF duty-cycle machinery, the 550 µs sync + back porch,
  Table 3-2's aspect-mapped vertical sync: 4:3 → 4, 16:9 → 5, 16:10 →
  6, the 1280×1024/1280×768 special cases → 7, non-standard → 10),
  and `gtf` (the 1999 generalized timing formula — the 1-line front
  porch, the 3-line sync, `ROUND` semantics where CVT rounds down,
  and the period refinement that collapses to `1/(rate × vtotal)`
  exactly in integers).
* **The exactness doctrine, every family**: integer arithmetic
  throughout (no floats), the clock `ceil`-ed to integer kilohertz
  never under the ask (finer than the spec's own 0.25/0.001 MHz
  grids — the divergence documented), u16-wire honesty, and each
  family's own degenerate refusals (GTF's back-porch and
  duty-collapse bands refused where CVT's 20% floor saves its own
  family — the distinction pinned by test).
* **The pour is the same pour**: every family rides the foundry's
  two existing gates (the EDID range-limits ceiling; the engine's
  `USERDEF` commit validation), joins the protocol face's advertised
  mode list, and migrates with the re-pour doctrine.
* **EC:** 29 foundry tests (hand-derived spec anchors per family:
  1080p60 pours 2080×1111 / 2000×1111 / 2576×1120 / 2576×1118 — four
  distinct rasters, the clock economy ordered RB2 < RB < GTF < CVT;
  the classic 800×500 VGA raster; the GTF 1344×795 classic; family
  distinctness, dispatch, never-under sweeps, degenerate bands), the
  serve-level family gate test, and the session-level proofs
  (`every_family_pours_end_to_end` — the engine's commit gate passes
  the analog raster, a bound client hears the pour, the GTF desktop
  presents onto it with pixel truth; the video-optimized rate serves
  at 59 940 mHz); all tiers green; the freeze gate untouched (the
  foundry is arithmetic, not protocol).

## Phase 45 — The nap, the claim, and the rebuild *(delivered — see CHANGELOG 0.13.0)*

Four features the giants own and this repository did not — each a
named line in the comparison documents, each closed with mechanism,
honest semantics, and a real-wire proof.

* **Occlusion quiescing — the App-Nap doctrine** (the macOS power
  story's structural advantage, earned): the damage pass's visible
  map is the authority — a mapped surface resolved onto **zero**
  outputs hides its scheduler slot at the pass (change-driven, the
  route's `quiesced` flag); the hidden slot's live registration dies
  with `SurfaceHidden`, its later `frame` requests **park** (no
  deadline for a client the server cannot show — the parked-output
  doctrine applied per surface), flips never answer them, and the
  unhide is the answer point. The scheduler contract and one golden
  were consciously re-pinned; the proof is
  `binaries/lion-compositor/tests/occlusion_quiesce.rs`.
* **The per-surface material request** (`toplevel.set_material`,
  the NSVisualEffectView doctrine): the client claims its window's
  Liquid material; `default` clears; the request is appended after
  the frozen opcodes (purely additive — the freeze consciously
  re-taken, 92 requests + 122 events); `ldp-shell` grew the typed
  `Material` vocabulary; `surface_style` consults the claim at both
  call sites; the server keeps the fullscreen-plain and popup-menu
  invariants and the tier quality budget. The proof is the A/B/A
  byte-equality in `material_session.rs` — VibrantDark's first call
  site.
* **The session-rebuild supervisor** (`lion-supervisor`, the DWM
  doctrine): the pinned socket, the crash → backoff → rebuild
  choreography, the epoch, the graceful stop (the audited
  `sys::terminate_child`), the pid file, the rebuild cap. The proof
  is `tests/rebuild_session.rs` over real processes: `kill -9` the
  compositor, the same abstract name serves again, a reconnecting
  client presents again, the operator's SIGTERM ends the session
  cleanly. The failure-stage verdict's named line, served.
* **The axis-metadata coalescing** (the input row's named
  remainder): `touch.shape`/`touch.orientation` as per-contact
  position state of their own kinds (`POSITION_SHAPE`/
  `POSITION_ORIENTATION`) — the freshest ellipse and angle per
  contact per display frame. Proven in `input_session.rs`.
* **EC:** all tiers green (2,126 tests); the freeze gate re-taken
  consciously (additive only, opcode tables dense, no opcode
  moved); byte-exact oracles intact; clippy/fmt/doc gates green.

## Phase 44 — The quiet frame and the architecture answer *(delivered — see CHANGELOG 0.12.0)*

The damage engine's steady-state economy, measured A/B against
the pristine v0.11.0 engine; the fuzz gate's own soak mode finding
and fixing a latent harness panic; and the pipeline-depth
architecture comparison against Windows' and macOS' display stacks.

* **The quiet frame** (five guarded fast paths in the damage walk):
  the below-unions table materializes only at opaque-flip nodes (one
  backward pass, O(n) adds, a clone per flip — the old table was one
  deep region clone per surface per frame); the R2 coverage algebra
  runs only for surfaces whose bounds changed; the R3 flip algebra
  only for surfaces whose opaque footprint changed; the
  fully-visible check is a one-rect comparison; the pass records
  skip quiescent unchanged surfaces — with the damage-consumption
  and occlusion-relatch terms load-bearing and each pinned by a
  regression test (the corpus oracle standing over all of it).
* **The A/B** (full-size release run, 7-run medians, identical
  input): quiet 64-window damage pass 94,720 → 44,609 ns/op
  (−52.9%); one-moving-window load 126,906 → 77,199 (−39.2%);
  two new `ldp-bench` rows committed with the numbers.
* **The fuzz-soak hardening**: `LDP_FUZZ_SCALE=8` (the gate's own
  soak mode) found the transport fuzzer panicking in
  `chunk_stream` on a one-byte mutated stream (the interior-cut
  draw against a zero — want-chunks is always ≥ 2); the empty-source
  splice sibling in `mutate_bytes` fixed with it; the gate clean at
  20× scale; two mutator-edge regressions committed.
* **The architecture answer**: `docs/comparison-windows-macos.md`
  (new) — ten pipeline stages, Windows' DWM/WDDM/DXGI/
  DirectComposition and macOS' WindowServer/CoreAnimation/IOSurface/
  Metal walked beside the LDP stack, an honest ledger, and a
  stage-verdict table pricing every gap with the roadmap line that
  closes it.
* **EC:** all tiers green (2,121 tests); the freeze gate
  (`ldpc snapshot --check`) untouched; byte-exact oracles intact;
  clippy/fmt/doc gates green.

## Phase 43 — The contact doctrine and the living registry *(delivered — see CHANGELOG 0.11.0)*

The input-latency row's named remainder and the protocol-design row's
named future line, closed together, plus the deepest hardening pass
since the crash corpus.

* **The per-contact coalescer**: `CoalesceKey` grows a `sub`
  discriminator (the contact/tool id); the emission sites classify
  touch and tablet streams per contact and per axis — two-finger
  floods collapse to two motions (each finger's own freshest), pen
  streams to one motion + one pressure + one tilt per frame, the
  discrete barrier seals a contact's own stream, `touch.cancel`
  seals nothing (the down that opens the next sequence carries its
  own seal).
* **The living registry**: `Dispatcher::on_registry` (default
  no-op) tracks each session's fan-out targets; the dark state
  withdraws the output global (revocations before the zero-arg
  `global_remove` barrier — the spec's own order) and the relight
  re-advertises it; a withdrawn bind answers `interface_removed`;
  the ldp-client `GlobalRemove` decoder is drift-fixed to the
  frozen zero-arg shape.
* **The hardening wave**: the subsurface depth cap (64, typed
  refusal), the cross-client pending-attach fix, saturating
  occlusion offsets, the union-found disjoint-rect merge (the
  O(n³) restart loop closed), the bounded damage accumulation, the
  replay codec allocation guard.
* **The delivery-path economy** (A/B-measured: peak RSS −22.6% on
  the identical bring-up and client): the transport writer's
  unsent-front cursor, the frame delivery by borrow, the
  allocation-stable display model, branch-free plane blends.
* **EC:** all tiers green (2,116 tests); the freeze gate
  (`ldpc snapshot --check`) untouched; byte-exact oracles intact;
  clippy/fmt/doc gates green.

## Phase 42 — The mode foundry *(delivered — see CHANGELOG 0.10.9)*
The display-size field's answer: the row's own named remainder —
"the giants keep their 90s on decades of EDID/timing quirk coverage
— that depth cannot be simulated" — answered with the timing
machine itself, arithmetic instead of a quirk table.
* The VESA CVT reduced-blanking synthesis (`ldp-display`'s
  `timing.rs`, `--synth WxH[@HZ]`): the digital-panel doctrine —
  the 160-pixel compact blank, the 3-line vertical front porch,
  the 4-line sync, the 460 µs minimum vertical blanking period
  solved *exactly in integers* (no floats — the bit-reproducibility
  doctrine), the pixel clock ceil-targeted so the realized refresh
  is never under the ask. The canonical anchor pinned: `cvt -r
  1920 1080 60`'s 2080×1111 totals, bit-exact. Degenerate asks are
  typed refusals, never truncations.
* The pour is a **user-defined mode** (the kernel's own vocabulary,
  the `drmModeAddMode` lineage): the display engine validates it at
  commit against its declared synthesis envelope (the mock's gate
  mirrors the real driver's atomic check), and the protocol face
  *adopts* it — the pour joins the advertised mode list, flagged
  current (`xrandr --addmode`'s story, protocol-side).
* The EDID parse grows its timing half: the detailed timing
  descriptors (the first is the sink's preferred timing, marked
  like the kernel marks it; the DTD wire's 10 kHz clock granularity
  stated, not hidden) and the monitor range limits — the declared
  pixel-clock ceiling gating every pour with the typed refusal
  naming it (never a silent clamp). The pour rides bring-up
  (single- and multi-output, per-connector ceilings) *and* every
  hotplug migration (a refusing connector falls back to the unsized
  doctrine — the sized doctrine's honesty, verbatim).
* The EDID audit (`edid::audit`) names the three classes a
  deterministic rig can prove — a rotten block, a preferred-timing
  lie, a declared ceiling the list exceeds — and the quirk table
  accrues the timing trio (`edid-rotten`, `edid-preferred-lie`,
  `pixel-clock-ceiling` — eight rows now, a `Timing` quirk class,
  each row's detection the audit's own named finding).
* **EC (met):** `binaries/lion-compositor/tests/synth_session.rs`
  — five proofs over the real socket (the pour serves a size the
  list lacks, pixel-exact at 2560×1440; the pour serves a refresh
  the list lacks, 1080p at 144 Hz; the above-ceiling refusal names
  the declaration; the audit diagnoses the stale firmware by name;
  the migration re-pours then falls back past the ceiling);
  `ldp-display`'s suites grown by twenty-three (the timing
  arithmetic's hand-derived oracles, the DTD/range round trips, the
  audit fixtures, the synth selectors); `verify.sh` green (2,116
  tests) in-tree.
* The honest remainder, named: the GTF and CVT standard-blanking
  families (legacy analog sinks) and CVT-RBv2's 80-pixel blank stay
  roadmap lines — every digital panel the mock inventory models
  serves RB timings.

---

## Phase 41 — The quirk ledger *(delivered — see CHANGELOG 0.10.8)*
The VRR field's answer: the comparison row's own named remainder —
"the multi-year driver quirk table, not the mechanism" — answered
with the ledger's structure the decades fill, the first rows tabled
as mechanism-grade operator escapes.
* The honest floor (`--vrr-floor N`, per-output CSV — the `--scale`
  grammar's mirror): panels whose advertised VRR range flickers at
  the bottom get the operator's honest minimum refresh rate clamped
  into every window *consumer* — the scheduler's widened deadline,
  the `output.vrr` advertisement, the LFC cadence — while the
  device's own claim stands untouched (the override lives in the
  display stack above the driver, exactly where the giants' tables
  live). `ldp-vrr::quirk` owns the clamp (`FloorOutcome`'s audit
  trail; `AboveNominal`/`AboveRange` name the operator's mistake); an
  unservable floor refuses boot rather than silently ignoring a lie.
* The LFC cadence and the anti-flap latch (`ldp-vrr::refresh`):
  content slower than the window bridges on repeats locked to the
  content's own phase (`k = ceil(P/max)` — judder-free by
  construction, the giants' low-framerate-compensation arithmetic);
  the latch engages on the first missed window, holds through
  boundary-hugging flips, clears only on comfortable recovery — the
  stretch/repeat flap the driver tables name rides warmed repeats.
* The sibling escape (`--vrr-uniform`): a mixed desktop — one VRR
  panel, one fixed — collapses to one uniform fixed sync across the
  seam for the platforms whose cross-CRTC clock coupling flickers
  the fixed sibling. Per-output VRR stays the default (the modern
  per-display doctrine); the collapse only bites when the desktop
  actually spans the seam.
* The quirk table accrues its fourth and fifth rows
  (`vrr-floor-flicker`, `vrr-sibling-flicker` — symptom, detection,
  escape, cost, each a stable public commitment), and the selftest's
  VRR line prints the floor pass's audit trail per output.
* **EC (met):** `binaries/lion-compositor/tests/vrr_floor_session.rs`
  — six proofs over the real socket against the mock panel's own
  advertised 48-144 Hz window (the clamp on the wire event, the
  narrowed widening, the passthrough entry, the refusal, the
  collapse's disarming with the contrast armed beside it, the two
  composed); ldp-vrr's own suites grown by fifteen (the floor's
  cross-product and the LFC cadence table); `verify.sh` green
  (2,084 tests) in-tree and from a fresh extraction of the bundle.

---

## Phase 40 — The deep material *(delivered — see CHANGELOG 0.10.7)*
The visual-quality field's answer: the materials distance to the
giants closed — the vibrant domain, the edge light, and the material
family.
* The vibrant domain: `BackdropParams::saturation` extends to
  `u16` `0..=510` — below 255 the Phase 27 desaturation stands
  byte-identically, above it each channel extends *away* from its
  luma by `(saturation − 255) / 255` of its own luma-distance
  (integer-exact, rounded half away from zero) — the acrylic/vibrant
  distance where Windows' acrylic and macOS's vibrant materials live.
  Named materials `vibrant_light` (the menu glass, saturation 383)
  and `vibrant_dark` (the control-center glass); the vibrant order
  documented and pinned: blur, boost, veil.
* The edge light: `LayerStyle::edge_light` — the luminous 1-px
  hairline traced inside the rounded silhouette (the coverage
  difference against the silhouette's own 1-px Minkowski erosion, so
  the ring follows the corners on the same antialiased curve the ink
  clips with), drawn *after the ink* in both backends — software
  through the shared damage-clipped `apply_material`, GL as the
  stream's after-ink textured quad of the identical memoized words
  (byte-equal by construction). The ring is *imagery* like the
  shadow: `EdgeMemo` serves it per (dest, radius, params).
* The material family: `ldp_renderer::Material` — Panel, Sheet,
  Menu, VibrantDark, Chrome — one named vocabulary the desktop
  speaks. The compositor routes through it: a popup wears the menu
  glass (the vibrant frost plus the hairline — backdrop blur
  *everywhere* the giants mean it), the dock wears chrome (the
  Phase 28 shadow-cleared doctrine plus the hairline), the legacy
  pair verbatim (Panel ≡ the opaque policy, Sheet ≡ the translucent
  policy, byte-equal at every tier; Minimal serves every material
  plain — the library default's bytes frozen). The menu's shadow
  parameters equal the sheet's at every tier — the
  damage-equivalence constraint keeping the repaint spreads honest.
* **EC (met):** `binaries/lion-compositor/tests/material_session.rs`
  — the menu glass end to end over the real socket (the interior is
  the vibrant frost hand-computed, the straight edges carry the
  hairline, the wallpaper untouched, the steady state keeps the
  bytes); the golden boost/ring oracles; the randomized
  cross-backend equivalence corpus grown into the vibrant domain
  with hairlines; `verify.sh` green (2,063 tests) in-tree and from
  a fresh extraction of the bundle.

---

## Phase 39 — The fresh frame *(delivered — see CHANGELOG 0.10.6)*
The input-latency field's answer: the giants' end-to-end tuning —
event coalescing — served, and the VRR-aware presentation clock.
* The position-state class (§10.4's table grows a row):
  `EventClass::InputState` in `ldp-compositor::coalesce` — the
  replacement geometry that differs from every other class (the
  survivor overwrites the *oldest* pending slot in place, so a
  discrete event is never pre-empted by a newer sample), and
  `CoalescingQueue::seal` — the discrete-input barrier (a click's
  context is the sample it rode with; the X11 flush-before-button
  doctrine expressed as a freeze).
* The served outbox becomes the §10.4 emission path (the doctrine
  was library-only since Phase 7; the live `Outboxes` was a bare
  FIFO): per-client `CoalescingQueue`s, the class declared at the
  emission sites — the pointer's absolute `motion` and its `frame`
  terminator are position state (a 1000 Hz device parks sixteen
  samples between display frames; a frame-cadenced client reads ONE
  `motion` carrying the freshest coordinates), `relative_motion` is
  plain input (every delta delivers — full fidelity for raw-input
  consumers), the discrete vocabulary seals, the presentation
  feedback coalesces per surface and kind, and the unclassified
  default keeps the exact pre-Phase-39 FIFO (nothing coalesces
  unless an emission site says so).
* The VRR-aware presentation clock: `FrameClock::arm_vrr` — the
  measured-interval validity band becomes the panel's own probed
  `[min, max]` window (the LFC fast end and the stretched slow end
  are both honest single-period landings; the legacy half..1.5×
  band rejected the fast end as a duplicate), and the adaptive
  opportunity target (`sched_build_adaptive`) — an adaptive-mode
  surface under a fully probed window targets the panel's next
  refresh opportunity (the earliest in-window landing) instead of a
  nominal grid cell the event-driven panel may already sit behind:
  the verdict fires at the flip that carries the content, the
  deadlines pace at the fast end while the client keeps up (the
  frame pacing follows the content — adaptive sync's whole point),
  and an unprobed window keeps the nominal walk (the honest
  degradation). Wired from `--vrr` through `SchedulerConfig.vrr_min_ns`
  (the bring-up carries the window's min side alongside the Phase 31
  max-side widening).
* **EC (met):** `binaries/lion-compositor/tests/latency_session.rs`
  — five proofs over the real socket (the flood: sixteen parked
  batches deliver one motion with the freshest coordinates and
  every delta; the barrier: the click rides between its own sample
  and the fresher one; the VRR fast end: every `presented` carries
  the panel's actual 6.944 ms cadence; the fixed-grid contrast; the
  budget: the flood rides the next frame within one nominal
  period), the doctrine suites in `coalesce.rs`/`outbox.rs` and the
  predictor/scheduler unit tests (the opportunity traces, the
  window band, the degradation gates); `verify.sh` green (2,050
  tests) in-tree and from a fresh extraction of the bundle.

---

## Phase 38 — The honest peak *(delivered — see CHANGELOG 0.10.5)*
The HDR field's answer: the panel's real peak and the content's own
mastering metadata finally reach the ink.
* The negotiation vocabulary (`ldp-hdr::negotiation`, new): the
  panel's *effective* peak (`PanelPeak` — the advertisement clamped
  by `--hdr-peak`, the min), the negotiated canvas ceiling
  (`negotiated_ceiling` — the stack's brightest content clamped to
  the panel: dimmer content negotiates to itself, brighter maps
  down, unknown keeps the panel), and the per-surface mastering
  refinement (`layer_mastering` — the declared HDR10 bounds when
  known, the description's own otherwise, the CTA sentinels never
  fabricating). The fold records `World::negotiated` (the comment
  that promised "recorded for the render's tone-mapping ceiling"
  since the v0.10.1 era finally true), and the PQ canvas carries the
  ceiling as its `luminance_max`.
* The luminance tail (`ldp-renderer`): `TonePolicy` — the per-layer
  directive (`Pass`/`Clip`/`Eetf`) the compositor concludes from the
  negotiated ceiling and the layer's declared mastering bounds;
  `ColorPipeline::with_tone` bakes it into the linear domain (the
  BT.2390-structured knee in the renderer's f32 doctrine, hue
  preserved by construction). A tail-bearing layer takes the
  scene-linear path whatever its description says (the encoded-space
  copies must not bypass the rolloff); the GL v1 path refuses a
  tail-bearing layer typed (the honest boundary — the frame routes
  to the software arm).
* The operator's honesty knob (`--hdr-peak N`, with `--hdr`): the
  panel-peak truth for the bloated-peak quirk class — the
  advertisement (`hdr_caps`), the canvas ceiling, and every HDR
  layer's tone policy carry the *effective* peak, both directions
  (what clients can rely on is what the render pass delivers). The
  quirk table accrues its first row (`hdr-peak-bloat`, class
  *advertising*); the selftest prints the HDR line.
* **EC (met):** `binaries/lion-compositor/tests/hdr_session.rs` —
  nine proofs over the real socket (the four Phase 38 additions: the
  negotiated ink pinned to the independent f64 model at ±3 LSB, the
  dimmer content's own level, the operator cap reaching both the
  advertisement and the ink, the reply side); the renderer's golden
  suite — the clip's exact ceiling code, the EETF's published
  anchors (saturation, the monotone sweep, the identity segment),
  `Pass`'s byte-identity, the GL refusal; `verify.sh` green (2,025
  tests) in-tree and from a fresh extraction of the bundle.

---

## Phase 37 — One desktop, every arrangement *(delivered — see CHANGELOG 0.10.4)*
The multi-monitor field's answer: mirror mode, per-output scales,
and the quirk table's mechanism.
* The mirror doctrine (`--outputs mirror`): every served display at
  the layout origin — the overlap itself the protocol's clone
  signal (the vocabulary X11 and the Wayland compositors serve for
  clone mode). The desktop is the *primary's* bounds (a union would
  inflate the desktop to the largest display); each display renders
  the same stack at its own mode — a smaller display crops, a larger
  letterboxes in the desktop's own painted background (the
  full-bounds seed paints every display whole from its first flip,
  bring-up and hotplug alike). The dock renders on every mirrored
  display (the desktop's own chrome); the release gates span the
  mirror (a buffer both displays scan out releases after both flip);
  a display joining mid-session joins the mirror, never an
  extension. The library default stays `Extended` — every Phase 31
  byte intact.
* The per-output scale doctrine (`--scale F1,F2,…`): output *i*
  advertises factor *i* (Q8.8 over the cascade), extras reusing the
  last entry; a hotplug newcomer takes the last entry too (the
  stretch rule — the operator's list never runs out). The shell's
  logical canvas resolves on the primary's own factor; a lone factor
  keeps the Phase 31 doctrine byte-identical.
* The quirk table (`ldp-display::quirks`): the display ecosystem's
  long tail as a mechanism — every row named, classified, carrying
  its symptom, its detection, its operator escape, and its cost. A
  quirk is only tabled when the system can route around it
  deterministically. Ships with the two honest rows (`psr-flicker`
  → `--no-psr`; `vrr-flicker` → serve without `--vrr`); the
  selftest prints the table's report line, the admin guide the
  table proper. The per-vendor *rows* accrue with hardware — the
  giants' decades are the honest remainder.
* Fixed (the seal run's own bug hunt): the renderers' per-output
  canvas parking filed under the layout *origin* — an identity that
  stops being unique the moment two displays overlap (which is
  exactly what a mirror is): the 720p display restored the 1080p
  canvas it had just parked, truncated into the wrong shape (the
  crop proof's per-pixel equality caught it; canvases now file
  under their full geometry, and same-size mirrors *share* one
  canvas — the mirror's own meaning); and a hotplug newcomer reset
  its scale to identity, losing the operator's `--scale` mid-session
  (the stretch rule fixes it).
* **EC (met):** `binaries/lion-compositor/tests/mirror_session.rs`
  — seven proofs over the real socket: the origin bring-up, the
  identical-pixels byte-equality (both same-size displays' scanouts
  word-for-word equal), the mixed-size per-pixel crop (every row of
  the 720p display equals the primary's first 1280 columns), the
  topology motion (unplug → `OutputRemoved`, plug → `OutputAdded`
  at the origin, the newcomer painting whole, the mirror continuing
  byte-equal), the per-output scale cascades (256 then 512), the
  stretch rule (and the bug it fixes), and the mirrored release
  gates; `multi_output.rs` — the Phase 31 exit criteria unchanged
  and green; `verify.sh` green (2,013 tests) in-tree and from a
  fresh extraction of the bundle.

---

## Phase 36 — The rootless door *(delivered — see CHANGELOG 0.10.3)*
Every X11 window becomes a first-class LDP window; the popup role is
served, not just specified.
* The compositor serves `ldp.shell.popup` end to end: `get_popup`
  mints `ldp-shell`'s anchor/gravity machine (the parent validated
  as a live surface of the connection, null = the output itself —
  the bar-menu doctrine), the placement proposal rides the surface's
  first attach (the buffer's size is the solver's input; the
  position rides the pending queue — lands placed, never
  origin-then-jump; popups never take the toplevel cascade),
  `popup.configure` carries the parent-relative placement,
  `ack_configure`/`reposition` (the live move via `set_position_now`)
  /`dismiss`→`done` answer the client vocabulary, and the
  parent-death sweep dismisses every child. `grab` enforces the
  serial-freshness doctrine; `get_dialog` is honestly refused (the
  window-management roadmap line). The `PopupHost` is the policy
  home in the compositor's shell module.
* `ldp-x11-bridge::rootless` — the per-window export engine: every
  mapped root-child X window rides its own LDP surface (toplevel for
  the managed, popup anchored at the OR window's own screen rect for
  the override-redirect), the **subtree composite**
  (`Server::window_frame` + `WindowTree::region_within`) painting
  each export's descendants in the export's own frame, per-window
  damage (`Server::take_top_damage`), one high-water pool with
  per-window buffer offsets, and the two-truth coordinate doctrine:
  the X geometry stays the X clients' truth, the LDP placement is
  the screen's truth, input bridging them by construction
  (surface-local + the X window's own origin = the X root
  coordinates). The root window is not exported (the
  XQuartz-rootless doctrine).
* `lion-bridge`: the rootless mode is the X11 default;
  `--x11-rootful` is the whole-screen escape. The quiescent
  keepalive (`connection.sync` per idle turn) polls the
  compositor's parked cross-client events out — a reactive bridge
  would never see its input otherwise. The link's watches are
  idempotent and carry the popup vocabulary.
* Fixed (the seal run's own bug hunt): the bridge's pointer `enter`
  coordinates lifted the surface object as the x (every enter at
  (0,0) — a latent v0.10.0 bug, caught by the rootless session's
  coordinate proof); the X face's spontaneous output parked until
  the foreign client's next request (the flush now runs every
  turn); the rootless pool's growth re-zeroed the mirror while only
  fresh windows re-committed (the elders went black — grow-only
  now, caught as a one-in-two flake by the session suite).
* **EC (met):** `bridge_x11_rootless_session.rs` — three X windows
  at fictional X geometries, all pixel-exact on the scanout through
  a real Unix socket (the cascade, the popup anchor, and the damage
  isolation), the pointer motion over the display position arriving
  at the X client with the X geometry's root coordinates, and the
  unmap teardown with the neighbors standing;
  `binaries/lion-compositor/tests/popup_session.rs` — the native
  popup role end to end (solve at attach, ack, the live reposition
  move, dismiss, the parent-death sweep); the rootful regression
  (`--x11-rootful`) unchanged and green; `verify.sh` green (2,001
  tests) in-tree and from a fresh extraction of the bundle.

---

## Phase 32 — The data family over the wire *(delivered — see CHANGELOG 0.10.0)*
The clipboard the compatibility bridges need, served end to end
through the real protocol.
* `ldp.data.data_device_manager` advertised and served: devices
  mint per seat (`get_data_device` — the seat argument validated as
  a live seat binding), sources offer MIME types, and
  `set_selection`/`set_primary_selection` install a source on the
  seat's slot. The publication routes to every other device client
  on the seat through the outbox — the `data_offer` announcement
  arriving as a real receiver-held object (the store's new
  `insert_server_at`/`create_announced` path: the server-chosen id
  crosses sessions as a create-first instruction), the MIME
  enumeration, and the `selection` event.
* The transfer: `accept` narrows (the source sees `target`),
  `receive(mime, fd)` hands the pipe's write end across the request,
  the permission gate runs (`ClipboardRead` in the same-user
  baseline), and the source's `send` event rides the descriptor
  back — the payload byte-exact in the receiver's read end.
* The seat's interaction serial clock: one monotonic
  `World::next_seat_serial`, drawn by every serial-bearing delivery
  (today the shell's configure; the evdev path joins the same
  clock). A stale serial is the fatal `invalid_state` — the
  freshness doctrine, now observable over the wire.
* The five device mints (`get_pointer`/`get_keyboard`/`get_touch`/
  `get_tablet`/`get_gestures`) at their true interfaces.
* The compatibility door lands in-tree: `binaries/lion-bridge` —
  Wayland clients become LDP windows one-for-one (`--wayland PATH`),
  an X11 screen becomes one rootful LDP window (`--x11 PATH`), one
  single-threaded poll engine over every descriptor, every foreign
  session behind its own LDP identity and a bridge-scope token.
* Fixed: the shell's initial `configure` carried 4 of the 10
  arguments the schema declares — a wire violation any validating
  client would reject; found by this phase's end-to-end suite (the
  first client ever to bind the shell).
* **EC (met):** `data_session.rs` — the full choreography (the
  selection, the announcement as a requestable object, the
  enumeration, the selection event), the byte-exact pipe transfer,
  the freshness doctrine (a stale serial is fatally
  `invalid_state`), and the teardown truth (destroying the source
  announces `selection(null)`); the manager's reverse lookups and
  `offer_destroy` unit-pinned in `ldp-clipboard`; `clip-client
  --live` reports the served family; the bridge sessions — a
  Wayland client's pixels and an X11 client's pixels reach the
  compositor's scanout (`bridge_{wayland,x11}_session.rs`, the
  faceless refusal, the bootstrap probes); the bridge binary ships
  in the lion-compositor deb; `verify.sh` green.

---

## Phase 31 — The served stack *(delivered — see CHANGELOG 0.10.0)*
The compositor stops being a renderer with a protocol attached:
several displays at once, the fractional-scale truth, the HDR
pipeline, adaptive sync, the idle ladder, and the frozen v1 API
contract.
* The multi-output doctrine (`--outputs multi`): every pipeline the
  allocator finds serves — one per CRTC/plane, one logical desktop
  left-to-right; per-output bind round-robin; per-output visibility
  (a spanning window enters both); topology motion mid-session
  (`OutputRemoved`, promotion, extension, mixed sizes); spanning
  release gates; the library default stays single, byte-identical.
* The fractional scale doctrine (`--scale F`): the factor
  advertises, the shell resolves on the logical canvas, placement
  lands physical, clients learn at bind and `enter_output`.
* The HDR pipeline (`--hdr`): PQ caps over `output.hdr_caps`, the
  mode controller's dwell hysteresis, scene-linear blending with
  the PQ tail (SDR at BT.2408 reference white).
* Adaptive sync (`--vrr`): `VRR_ENABLED` on every capable flip; the
  scheduler's commit window stretched to the CRTC's range.
* The idle ladder (`--idle MS`): Dimmed at the timeout, Off at
  twice it (DPMS, parked scheduler, `output_off` deaths), wake on
  activity.
* The API stability freeze gate: `ldpc snapshot --check` — the v1
  surface pinned byte-for-byte in CI.
* The zero-copy EGLImage arm: a layer imported as an EGLImage never
  touches a CPU payload — the command stream binds the image as the
  texture directly; the command-stream goldens
  (`tests/gles_stream.rs`) pin exactly which passes the GL renderer
  emits (damage passes, skipped layers, scissors, readbacks, the
  shader sources).
* **EC (met):** `multi_output.rs` (15 tests), `scale_session.rs`,
  `hdr_session.rs`, `vrr_session.rs`, `power_input_session.rs`,
  `gles_stream.rs`; every prior oracle untouched (the defaults keep
  the plain doctrines); `verify.sh` green.

---

## Phase 30 — The desktop matrix *(delivered — see CHANGELOG 0.9.0)*
Every display size, every graphics card, and the machines with none:
the compositor meets the machine it boots on, and the honest world
comparison is a committed document.
* GPU-class capability probing: the EGL+GLES probe reads the
  context's identity (`glGetString` vendor/renderer/version — core
  GLES 2.0, the new audited FFI entry) into `GlIdentity`, and
  `GpuClass` — Discrete / Integrated / Virtual / SoftwareRasterizer /
  Unknown — is derived from the renderer string by pure table-driven
  logic. The honest default: an unrecognized GPU is `Unknown` and
  counts as hardware (never a silent degrade).
* The llvmpipe doctrine: **Auto refuses a CPU-rasterizer GL
  context** — the specialized software backend (word blending, no GL
  state machine) is the faster CPU path; the reason rides the
  report line. `--renderer gl` still forces GL over any context
  (the operator is the final authority). The startup line names the
  device: `renderer: gles (hardware: …, discrete GPU: AMD Radeon …)`.
* The sized output doctrine (`--resolution WxH`): the bring-up and
  every hotplug migration prefer a mode of exactly the forced size
  (preferred flag, then the highest refresh among matches); a size
  nothing offers is the typed failure listing every size the panel
  does offer — the operator's menu, never a silent nearest-neighbor
  guess; a migration to a display without the size falls back to the
  unsized doctrine and the world keeps the preference.
  `OutputGlobal::select_size` re-points the protocol face at the
  forced mode. `Mode::panel_4k60` joins the fixtures.
* The desktop matrix benchmark rows: the Liquid desktop frame
  (wallpaper, four rounded-and-shadowed windows, one frosted dock
  bar) at 1920×1080 / 2560×1440 / 3440×1440 (21:9 ultrawide) /
  3840×2160, at the High tier and at the tier the no-GPU doctrine
  picks, steady state plus the cold first frame at the two ends.
  Measured (5-run medians): High steady 7.31 / 11.49 / 15.11 /
  24.67 ms — 60 Hz holds at High through the ultrawide; the
  doctrine's tier 5.96 / 7.95 / 10.96 / 18.99 ms. The first run
  caught the 4K thrash (194 ms — the shadow memo's fixed 6 Mi-word
  budget sat under a 4K working set of ~6.5 Mi); the output-scaled
  budget fixed it (7.9×, same bytes, the flat-counter oracle).
  The full bring-up's peak RSS: **19 MiB**.
* `docs/comparison.md`: twenty fields scored out of 100 against
  Wayland, X11/Xorg, macOS WindowServer, and Windows DWM — every
  score reasoned, every gap named with its roadmap line, the totals
  read honestly (the engineering fields lead; the adoption fields
  are priced without sentiment).
* **EC (met):** the classification table exhaustively unit-pinned
  (every known renderer string, the software-wins-over-everything
  rule, the Unknown default); the class-aware policy matrix
  (Auto+llvmpipe refused with the reason, forced-GL-over-llvmpipe
  honored, unknown/virtual stay hardware); the sized selection
  (the best exact match, the offered-menu failure, the no-display
  arm); `resolution_session.rs` — the forced size that is offered
  serves it (the world, the flagged current mode, the scanout
  pixels), the honest miss fails with the menu, the migration
  honors the forced size and falls back honestly without dying;
  the 4K thrash oracles (the budget-scaling semantics; the scaled
  budget keeping a large working set resident where the floor
  evicts; the flat `shadow_rebuilds()` counter across a 4K desktop's
  steady frames); every prior pixel oracle untouched (the library
  defaults keep the plain doctrines); `verify.sh` green at 1,839
  tests.

## Phase 29 — The steady-state pass *(delivered — see CHANGELOG 0.8.0)*
The low-end doctrine made real: the full Liquid scene at 60 Hz on a
weak CPU, byte-identical to Phase 28's output.
* The material memos: `MaterialCache` (a shadow built once per
  (params, destination, ink radius) — the macOS rule that shadows are
  *imagery*; linear scan, LRU under a 6 Mi-word budget, the honest
  retained-scratch recompute past it) and `FrostMemo` (the frost
  material rebuilt only when the backdrop's own words changed — a
  full comparison, no hashing; the steady state skips the blur
  entirely). Pure memoization: `submit` stays a pure function of
  (framebuffer, damage, layers).
* The corner-arc bands (`corner_band_xs`): pixel centers sit exactly
  half a pixel from every straight edge, so fractional coverage only
  ever comes from the arcs — 99.6% of a phone window's pixels never
  run the SDF's `sqrt` again. Tight bands for sanitized radii,
  conservatively whole for the degenerate capsule radii the public
  API admits; `merged()` walks them disjointly.
* The box blur rebuilt: edge-free interior loops, a 16-column strip
  walk for the vertical pass (contiguous row slices instead of a
  cache line per row per column), the window mean as an exact 48-bit
  reciprocal multiply (exhaustively proven for the sanitized domain),
  and a retained-scratch entry point.
* Word-level styled paths: the frost snapshot is a row word-copy
  (the canonical word *is* the ARGB-family output word), material
  application writes opaque pixels as words and skips the holed
  ink's transparent runs by scan, styled layers' full rows take the
  plain compositing path (word copies included, styled statistics
  unchanged), the copy paths broadened to the same format family,
  and the 1:1 blend loop monomorphized per source format.
* The GL backend on the same memos: the shadow's texture bytes
  convert once per unique shadow, one retained readback buffer per
  output size, the frost served from its memo.
* **EC (met):** the phone-frame benchmark — a 1080×2340 output, a
  plain wallpaper, four rounded-and-shadowed app cards, one frosted
  translucent panel, full damage every frame — measured
  **178.6 → 8.70 ms median steady state (20.5×)** and
  **178.0 → 64.8 ms first frame (2.75×)**, same machine, with the
  output byte-identical (the styled golden suites, the GL/software
  equivalence corpus, and the new steady-state transparency oracle —
  a warm renderer's frames byte- and stat-equal to a cold render's);
  the Phase 27 kernels restated verbatim as reference oracles and
  cross-checked over randomized shapes including the degenerate
  radii; every other benchmark row unchanged within run noise;
  `verify.sh` green at 1,816 tests.

## Phase 28 — The positioning shell *(delivered — see CHANGELOG 0.7.0)*
Where the system puts windows, and the dock it keeps for itself —
the phone's home-screen layer. The system owns the screen (macOS's
rule): clients draw content, the shell places it, the dock reserves
the edge.
* `ldp-shell::layout` — the pure placement engine: `DeviceClass`
  (portrait → phone, landscape → desktop — shape, not a device
  probe), `Layout::resolve` (classify, dock, carve; the thickness
  clamped against half the edge), `DockConfig`, the policies — Fill
  (the phone: apps anchor at the usable origin, overflow runs *under*
  the dock, occluded never destroyed — the iOS keyboard doctrine),
  Cascade (the desktop: diagonal steps caught at the usable edge,
  oversized windows anchored), Center (dialogs), `clamp_into`, and
  the report line. No clocks, no scene types: arithmetic only.
* The compositor's placement arm: a root's *first attach* places it
  (the buffer's size is the input; the position rides the pending
  queue and applies with the very commit that maps the surface — a
  window lands placed, never origin-then-jump); migrations re-place
  every mapped root immediately through
  `SurfaceTree::set_position_now` (the damage engine's R2 rule reads
  live positions, so the migration frame itself carries the move).
* The system dock: the phone's home bar — a frosted bar the
  compositor owns (a haze plus a row of app pills, ARGB ink at a
  stable buffer identity), rendered above every client layer through
  Phase 27's styled-layer machinery (the frost snapshots the
  composed backdrop; the corner coverage folds at upload — zero
  growth of the render seam), reserving its thickness out of the
  usable area, rising from below the edge on a critically damped
  spring integrated at render cadence from the driver's timestamps
  (bit-reproducible; a quiescent compositor holds its last
  integrated offset). The repaint doctrine: the vacate rule while
  rising (old and new placements), the frost's claim when damage
  intersects the dock's rect (the clamp-edge blur samples only
  within it), plain ink at Minimal. The direct-scanout candidacy
  subtracts the dock's reservation (chrome above client content is
  never a scanout candidate's truth).
* The doctrine switch: `--dock auto|off` (the CLI defaults to auto;
  the **library default stays off** — placement keeps creation
  positions and no chrome draws, so every Phase 26/27 pixel oracle
  stays byte-exact, the same contract as the effects tier) and
  `--dock-thickness`; the honest third startup line ("phone layout,
  dock 84 px at the bottom").
* **EC (met):** `shell_session.rs` — the desktop cascade pixel-exact
  against reference math written out in the test (the window one
  step in, the origin staying the wallpaper, the shadow strip, the
  frost material / haze / pill hand values, the corner coverage
  fold, the mid-rise dynamic), the phone re-anchor on a portrait
  swap (the whole doctrine re-resolving mid-session: the class, the
  usable carve, the dock re-formed at the new width, the roots
  re-anchored, the wallpaper running *under* the dock), the legacy
  regression gate (dock off keeps the Phase 27 pixels verbatim), the
  plain Minimal dock, the report lines, and the whole shell session
  **byte-equal under the injected reference GL backend**; plus the
  layout engine's unit suite, the immediate-move damage rule, and
  the shell module's ink/rise suite.

---

## Phase 27 — The Liquid visual engine *(delivered — see CHANGELOG 0.6.0)*
The material language of the reference desktops, on Linux, built for
low-end devices — the phone milestone. The system owns the materials
(macOS's rule): clients draw content, the compositor dresses every
surface.
* `ldp-renderer::style` — `LayerStyle` (corners, shadow, frost) with
  a plain default (Phase 26 layers render byte-identically), and
  `EffectTier` — the low-end doctrine: Minimal / Low / Medium / High
  trade blur passes for reach (`--effects auto` resolves GL → High,
  software on a phone-sized output → Medium, software on a desktop
  output → Low, the headless CI path → Minimal unless asked).
* `ldp-renderer::effects` — the pure kernels: the IEEE-exact
  rounded-rect SDF coverage, the separable premultiplied box blur
  (clamp edge for frost, transparent edge for the shadow's spread),
  the ink-holed shadow material (a translucent pane never sees its
  own shadow), the frost material (blur + desaturation + tint veil),
  and the crate's one shared integer `over` — the composite path,
  the GL reference evaluator, and the material draws all run it.
* The styled render paths: software composites the corner coverage
  into the source alpha; the GL stream goes **layer-major** for
  styled frames (the frost readback snapshots the same backdrop state
  the software save sees), folds the coverage at texture upload, and
  rides the shadow/frost materials as plain texture draws — zero
  growth of the command seam, byte-equality held (the corpus pins
  full, partial, and multi-rect damage, second frames included).
* `ldp-compositor::spring` — the critically-damped spring, fixed
  4 ms substeps, bit-reproducible: the motion vocabulary (the
  showcase's panel pills ride it through the presentation loop).
* The compositor integration: `--effects`, the tier report line, the
  per-surface policy (opaque → corners + shadow; translucent → the
  frost too; the fullscreen wallpaper undressed), and the damage
  expansion to effect rects (a mapped window's shadow paints in the
  map frame). Fixed on the way: the GL stream's latent
  overlapping-damage double-composite.
* **EC (met):** `styled_session.rs` — the frosted dock pixel-exact
  against reference math written out in the test (the veil, the
  desaturation, the ink, the hard shadow strip, all hand-derived),
  the plain-path regression gate (the default config keeps Phase 26
  pixels), the tier resolution/report, and the whole styled session
  **byte-equal under the injected reference GL backend**; the
  renderer's `golden_effects.rs` (hand oracles) and
  `gles_styled.rs` (the randomized styled corpus); the spring's
  physics suite; the showcase's glass-panel session.

---

## Phase 26 — Live output re-arrangement *(delivered — see CHANGELOG 0.5.0)*
The display milestone the serve loop pointed at: the served pipeline
*follows the topology* — no restart, no session lost.
* `lion_compositor::rearrange` — `World::rearrange`: re-probe the
  whole topology (one hotplug signal, every consequence), re-ask
  `serve::select_pipeline`, compare with the live pipeline. Four
  transitions: `Spurious` (the re-probe picked what is already
  served — stability over preference), `Migrated` (monitor swap or
  mode renegotiation), `Dark` (the last display gone), `Relit` (the
  first display back). The *pipeline comparison* decides, not the
  status diff — a sink can re-negotiate its mode list without any
  status change.
* The migration choreography is Phase 25's vocabulary replayed:
  applied disable, scanout objects released (framebuffers removed,
  DRM dumb buffers destroyed, mappings dropped), fresh chain in the
  configured store kind, applied enable, bring-up flip latching
  `FrameScheduler::reanchor` (the new output's nominal), full-scene
  re-render. The dark state is typed (`Option<OutputGlobal>`): the
  scheduler parks (`output_off` drops, deferred requests), pending
  fences flush, commits keep committing, grabs answer
  `failed(no_output)` — the capture_error vocabulary's second value
  (the spec's second growth, through the full ldpc pipeline).
* The client story: output objects are revoked
  (`capability_revoked`) through the outbox wake-point delivery; a
  re-bind delivers the fresh cascade. Uniform on purpose: a mode
  renegotiation and a monitor swap are the same event to a client.
* **EC (met):** `hotplug_rearrange.rs` — THE swap test (the served
  connector dies mid-session; the migration's applied state asserted
  on the mock; the client revoked, re-bound, presenting again —
  pixel-exact scanout at the new geometry), the dark state (honest
  device state, the session still serving, fences flushed, deferred
  frame requests, refused grabs), the relight (the deferred request
  answered on the new timeline, the desktop painting again),
  stability, re-anchoring (a 144 Hz swap answers at the new
  nominal), mode renegotiation, and bind-while-dark;
  `verify.sh` green at 1,727 tests.

---

## Phase 25 — The real-KMS serve loop *(delivered — see CHANGELOG 0.4.0)*
The display milestone the rehearsal pointed at: serving the protocol
*from* real hardware, with the same choreography CI proves on the
mock.
* `ldp-display::driver` — `DisplayDriver`: `KmsBackend` plus the
  serve loop's time-and-wait surface. The mock's `wait_events`
  advances the deterministic clock exactly to the next due event
  (the headless doctrine, unchanged); the real driver polls the DRM
  fd. `now()` unifies the mock clock and CLOCK_MONOTONIC.
* `ldp-display::serve` — the shared choreography: `select_pipeline`,
  the applied `enable`/`disable` commits (the rehearsal request
  minus TEST_ONLY, and its teardown mirror), the pitch-honoring
  `write_frame_rows` delivery into mapped scanout.
* The mapped dumb-buffer layer: `DRM_IOCTL_MODE_MAP_DUMB` + `mmap`
  (request number pinned against the published ABI — the pin test
  caught a size-field arithmetic slip in the expected constant),
  `DumbMapping` with drop-unmap, `anon_mapping` (the CI vehicle),
  `poll_readable`, `monotonic_now`.
* `lion-compositor --mode drm`: master, dumb buffers, FBs, mappings
  (zeroed opaque black), applied enable, bring-up flip latched; then
  `serve_kms` — poll the listening socket and the DRM fd, land flips,
  accept clients; SIGINT/SIGTERM unwind through teardown (disable,
  release, destroy, drop master). `--probe` keeps the TEST_ONLY
  rehearsal; nodeless machines fail typed and loud.
* **EC (met):** the mapped-store equivalence suite — the identical
  two-frame client session through the shadow oracle and through the
  real `mmap` delivery path, byte-equal over the whole scanout
  (`kms_serve.rs`); teardown's applied disable asserted against the
  mock's live state; the serve loop's device service drains hotplug
  without moving the deterministic clock; the binary's `--mode drm`
  and `--probe` fail honestly on nodeless machines (stderr carries
  the reason); `verify.sh` green at 1,715 tests.

---

## Phase 24 — Hardware acceleration by default *(delivered — see CHANGELOG 0.3.0)*
The macOS doctrine: GPU compositing whenever the machine has a GL
stack, software fallback honestly reported, everything through one
render-pipeline contract.
* `ldp-renderer::gles`: the `GlesApi` object-safe command seam, the
  `GlesRenderer` (damage-driven passes, layer-skip decisions, pinned
  GLSL composite shaders), the reference evaluator `RefGles` (the
  exact integer blend rule — the byte-equality oracle), the
  `RecordingGles` stream recorder, and the selection policy — Auto
  is hardware-first; forced `gl` without hardware fails typed; forced
  software ignores the machine.
* `ldp-gpu::gles`: `GlesContext` (dlopen'd libEGL, RGBA8888 pbuffer
  config, GLES 2.0 context, the 48-entry `gl*` table via
  `eglGetProcAddress`) and `RealGles` over it, with per-command
  context migration under the world mutex; typed unavailability on
  headless machines.
* `lion-compositor`: `--renderer auto|gl|software` (default auto) —
  the whole render pipeline (window management redraws, animation
  frames, damage repaints, capture) flows through the selected
  backend; the startup report carries the honest one-liner.
* The DRM hardware rehearsal in `--mode drm`: `drmSetMaster`, dumb
  scanout buffers (`DRM_IOCTL_MODE_CREATE_DUMB` — the request
  numbers pinned against the published ABI), framebuffer
  registration, and the full atomic enable commit under `TEST_ONLY`
  — validated by the kernel, never applied — then unwound and
  reported. The always-on real-KMS serve loop is the next display
  milestone (Phase 25).
* **EC (met):** the randomized GL/software equivalence corpus
  (byte-equal, both frames of every case, damage persistence
  included) and the command-stream goldens; the full compositor
  session byte-equal under the injected GL backend
  (`tests/hardware_renderer.rs`); forced-gl-without-hardware fails
  typed; `verify.sh` green at 1,696 tests; the honest-degradation
  branches CI-pinned (no GL stack on CI, no DRM on CI).

---

## Phase 23 — Release v0.2.0 *(delivered — see CHANGELOG 0.2.0)*
The milestone seal: one version, one story.
* Workspace version 0.1.0 → 0.2.0 (every member inherits; the
  showcase scene carries the mark) and `--version` / `-V` on every
  shipped binary — the eight operator tools, `ldp-bench`, `ldpc`,
  `ldp-remote-gateway`, `lion-compositor`, and the four examples —
  each pinned by its package's live tests against
  `CARGO_PKG_VERSION`.
* `scripts/version_check.py`: the release-coherence gate (workspace
  == every manifest == the in-tree `Cargo.lock` pins == the debian
  changelog base == the README milestone == the CHANGELOG heading ==
  the scene mark), wired into `verify.sh`.
* Debian: the `0.2.0` entry rolling up the `~phase21`/`~phase22`
  pre-releases; control descriptions raised to the v0.2.0 census; the
  fresh-root install gate green for the release — and hardened on its
  first run: the verify stage now pins the checked pair to the
  current changelog version (the old `dist/` glob once let stale
  pre-release debs unpack over the release pair) and asserts every
  installed binary's `--version` equals it.
* Docs raised to the milestone: README status/matrix/installation,
  this roadmap, the maturity matrix (row 23 + the new always-on
  gate), the user/admin guides.
* **EC (met):** `verify.sh` green at 1,672 tests with the new gate
  included; every binary's `--version` asserted in CI; the Debian
  packages build and install through the fresh-root gate; the
  all-in-one bundle `lion-display-v0.2.0.zip` delivered and
  re-proven by a fresh-extraction verify run.

---

## Phase 22 — Capture, PNG, and the next performance tier *(delivered — see CHANGELOG 0.2.0-phase22)*
The protocol's first growth + the screenshot story + renderer perf.
* `spec/capture.toml` (the `ldp.capture` module: `grab` → read-once
  `frame` snapshots) through the full ldpc pipeline; the compositor's
  scanout copied under the render lock and shipped as a memfd; the
  remote tracker's snapshot arm relays captures whole.
* `crates/ldp-png`: the zero-dependency deterministic PNG encoder
  (adaptive filters + fixed-Huffman deflate) with an independent
  decoder proving the round trip.
* `ldp-grab`: the seventh operator tool (PNG/PPM capture, multi-frame).
* Renderer: the `CopyPremul` word-copy path (7.4x on opaque ARGB
  composites) and the per-frame damage row index (O(intervals) per
  layer-row; ~12% on the desktop shape).
* Future-proofing: the MSRV CI job (1.75 tested, not declared) and the
  dependency policy guard (`scripts/dep_policy.py`).
* **EC (met):** the capture end-to-end suite — pixel-exact grabs
  against the scanout oracle, remote grabs through both gateways
  pixel-exact, repeated-grab stability, FD hygiene across grab-heavy
  lifecycles (`binaries/lion-compositor/tests/capture.rs`,
  `crates/ldp-remote/tests/remote_loopback.rs`); ldp-grab runs live in
  CI against the in-process compositor (`tools_live.rs`); ldp-png's
  round-trip suite decodes with an independent inflate; `verify.sh`
  green (fmt, clippy -D warnings, 1,700+ tests, rustdoc -D warnings,
  spec lint, dep policy, no ldpc drift).

---

## Phase 21 — Remote transport *(delivered — see CHANGELOG 0.2.0-phase21)*
`crates/ldp-remote` + `ldp-remote-gateway`.
* The v0.2 "remote transport" direction: an edge gateway (local
  `AF_UNIX` clients in) and a hub gateway (authenticated TCP in,
  compositor's socket out) carrying whole LDP sessions across hosts;
  the envelope wire with negotiated caps and length-bomb-proof
  validation; the FD relay vocabulary (per-commit pool windows,
  resize, eventfd fences, snapshots, streaming pipes); keepalive with
  dead-peer teardown; GPU descriptors refused explicitly.
* **EC (met):** the showcase example — unmodified, full client
  library, ping-pong pools, four animated frames with presentation
  verdicts and release fences — completes over loopback TCP through
  both gateways with the compositor's scanout **pixel-exact** against
  the deterministic render (`crates/ldp-remote/tests/
  remote_loopback.rs`, `showcase_over_the_wire_is_pixel_exact`);
  concurrent remote clients each present (`two_concurrent_remote_
  clients_both_present`); a wrong token fast-fails the client and
  leaks no session; a 2 GiB length claim is rejected after exactly one
  header read with no session spawned; a HELLO-then-silent peer is
  torn down within the keepalive deadline; three full session
  lifecycles restore the process FD count to baseline; 42 tests
  green (36 unit + 6 end-to-end); `Region::subtract` ~2× faster
  (373.9 → 189.0 ns median, same machine before/after) with the
  renderer and remote suites added to `ldp-bench` and
  `docs/benchmarks.md`.

---

### Milestone releases

| Tag | After phase | Meaning |
|---|---|---|
| v0.1.0 | 20 | bootstrapped ecosystem: protocol + server + compositor + bridges + tools, feature-complete per spec v1 subsets |
| v0.2.0 | 23 | milestone sealed: remote transport (Phase 21) + capture/PNG/perf (Phase 22) + release coherence (Phase 23) |
| v0.3.0 | 24 | hardware acceleration by default: the GL renderer (selection policy + FFI + byte-equal oracle) and the DRM scanout rehearsal |
| v0.4.0 | 25 | the real-KMS serve loop: mapped dumb scanout, applied atomic modeset, page flips on the DRM fd, LDP clients on the socket, clean teardown; serve choreography CI-proven byte-equal on the mock |
| v0.5.0 | 26 | live output re-arrangement: the served pipeline follows the topology — swaps migrate mid-session, the last display gone leaves the compositor honestly dark while the protocol keeps serving, the first re-plug paints the desktop again; clients feel it through revoked output objects and fresh cascades |
| v0.6.0 | 27 | the Liquid visual engine: the macOS-class material language (rounded corners, soft shadows, frosted glass) with the low-end quality tiers and the spring motion — byte-equal across software and GL, the system owning the materials |
| v0.7.0 | 28 | the positioning shell: the layout doctrine from the output's shape (phone fill, desktop cascade), placement at first attach, migration re-layout, and the frosted system dock at the screen edge rising on a spring — the system owning the screen |
| v0.8.0 | 29 | the steady-state pass: the full Liquid scene at 60 Hz on a weak CPU — memoized shadow and frost materials, band-limited corner arcs, the rebuilt box blur — byte-identical, proven by reference oracles and the steady-state transparency gate |
| v0.9.0 | 30 | the desktop matrix: GPU-class capability probing with the llvmpipe doctrine, the sized output (`--resolution`), the benchmarked desktop matrix (FHD → QHD → ultrawide → 4K), and the honest world comparison (`docs/comparison.md`) |
| v0.10.0 | 32 | the served stack: the multi-output doctrine, the fractional scale, the HDR pipeline, adaptive sync, the idle ladder, the frozen v1 API contract — the clipboard over the real wire (the data family the bridges translate foreign clipboards onto), the in-tree `lion-bridge` compatibility door (Wayland windows one-for-one, the rootful X11 screen), and the zero-copy EGLImage arm with the command-stream goldens |
| v0.10.1 | 34 | the hardware compositing path: the plane-assignment engine (`ldp-planes`) with the split-point doctrine and the named demotion ledger, the zero-composite frame (a fullscreen client's own framebuffer scans out — zero render passes), the underlay split, the buffer-to-framebuffer import walk (`drmPrimeFDToHandle` + `AddFB2WithModifiers`), YUV/scaled GL uploads byte-equal to software, dma-buf feedback tranches, and the display-model oracle — plus the evdev input path (Phase 33, landed in-tree post-v0.10.0 and changelogged here) |
| v0.10.2 | 35 | the sleeping panel: panel self-refresh (the per-output machine with entry hysteresis and named exits, the frozen timeline, the one-nominal rescan), the GPU clock governor (asymmetric hysteresis over the landed-flip load), the energy ledger (the documented cost model and the CI-reproducible static-scene ratio), the serve-loop idle-ladder fix, and `--no-psr` |
| v0.10.3 | 36 | the rootless door: every X11 window a first-class LDP window (the rootless driver's per-window export, the subtree composite, the two-truth coordinate doctrine, `--x11-rootful` as the whole-screen escape), the popup role served end to end (`get_popup`'s anchor/gravity machine, solve-at-attach, the parent-death sweep), the quiescent keepalive, and three real bug fixes from the seal run's own hunt |
| v0.10.4 | 37 | one desktop, every arrangement: the mirror doctrine (`--outputs mirror` — every display on the same desktop, the per-pixel crop for mixed modes, the clone signal in the overlapping origins), the per-output scale doctrine (`--scale F1,F2,…` with the stretch rule), the named-quirk table (the mechanism with the two honest rows), and two real bug fixes from the seal run's own hunt (the origin-keyed canvas parking, the hotplug scale reset) |
| v0.10.5 | 38 | the honest peak: the panel negotiation (the effective peak, the negotiated canvas ceiling, the per-surface mastering refinement — `ldp-hdr::negotiation`), the luminance tail (every HDR layer's ink rides the BT.2390-structured knee from its declared mastering range onto the negotiated ceiling — the system's rolloff replaces the panel's hard clip), `--hdr-peak` (the bloated-peak quirk's honesty knob, the advertisement and the ink both carrying the effective peak), and the quirk table's first accrued row (`hdr-peak-bloat`) |
| v0.10.6 | 39 | the fresh frame: the position-state coalescing in the served emission path (§10.4's table grows the row — one wake per display frame, the freshest coordinates, the delta stream intact, the discrete barrier sealing the click's own sample), and the VRR-aware presentation clock (the measured band is the panel's own window — the LFC fast end honestly reported; the adaptive opportunity target — the verdict fires at the content's own flip and the pacing follows the content) |
| v0.10.7 | 40 | the deep material: the vibrant domain (the frost's saturation dial extends past the backdrop's own chroma — the acrylic/vibrant distance, integer-exact), the edge light (the luminous 1-px hairline traced inside every rounded silhouette, drawn after the ink in both backends — memoized imagery like the shadow), and the material family (Panel/Sheet/Menu/VibrantDark/Chrome — menus and popups in glass, the dock in chrome, the legacy pair verbatim, Minimal plain) |
| v0.10.8 | 41 | the quirk ledger: the VRR field's answer — the honest floor (`--vrr-floor`, per-output, the advertisement the panel cannot sustain clamped out of every scheduling consumer), the LFC cadence and anti-flap latch (phase-aligned repeats, `k = ceil(P/max)`, the boundary flap warmed away), the mixed-desktop escape (`--vrr-uniform`), and five quirk rows tabled |
| v0.10.9 | 42 | the mode foundry: the display-size field's answer — VESA CVT reduced-blanking synthesis for any size (`--synth WxH@Hz`, integers-only, the clock never under the ask), the pour as a user-defined mode over the whole wire (engine-validated at commit, adopted by the protocol face), the EDID timing parse (the preferred timing, the range-limits ceiling), the bring-up EDID audit, and the timing quirk trio (eight rows tabled) |
| v0.11.0 | 43 | the contact doctrine and the living registry: per-contact touch/tablet position-state coalescing (two fingers collapse to two motions, each its own freshest sample; the tablet's per-tool axes are position state of their own kind; the discrete barrier seals a contact's own stream) and dynamic globals (the dark state withdraws the output global — revocations before the `global_remove` barrier, the relight re-advertises — through a default-no-op `Dispatcher::on_registry` seam, zero wire-surface change), plus the hardening wave and the delivery-path economy (peak RSS −22.6% A/B) |
| v0.12.0 | 44 | the quiet frame and the architecture answer: the damage engine's steady-state pass halves (quiet 64-window pass −52.9%, one-moving-window −39.2%, A/B against the pristine v0.11.0 engine) via five guarded fast paths with equivalence proofs; the fuzz gate's soak mode finds and the release fixes the one-byte `chunk_stream` panic and its empty-source splice sibling (clean at 20× scale); and `docs/comparison-windows-macos.md` — the pipeline-depth architecture comparison against the DWM/WDDM and WindowServer/CoreAnimation stacks |
| v0.13.0 | 45 | the nap, the claim, and the rebuild: occlusion quiescing (the App-Nap doctrine — parked frame requests for fully-occluded windows, `surface_hidden` terminations, the reveal's answer), the per-surface material request (`toplevel.set_material`, appended after the frozen opcodes, the freeze re-taken consciously), the session-rebuild supervisor (`lion-supervisor` — the DWM doctrine, `kill -9` proven over real processes), and the touch/tablet axis-metadata coalescing |
| v0.14.0 | 46 | the foundry completes: every VESA timing family poured in exact integer arithmetic — CVT-RB2 (`:rb2`, the 80-pixel blank, the 8-line sync, 1-pixel horizontal precision), the video-optimized 1000/1001 rate (`:rb2v`, the 59.94 Hz class), CVT standard CRT blanking (`:cvt`, the GTF duty-cycle machinery, Table 3-2's aspect-mapped sync), and GTF (`:gtf`, the 1999 formula) — spec-faithful to the letter, through the same two honest gates |
| v0.15.0 | 47 | the semantic scene: the LionOS display architecture's own identity — semantic surfaces over the wire (`set_semantic_role`/`set_security_class`/`set_scene_profile`, appended additively, the freeze re-taken), the security-aware capture (protected/system surfaces redact out of every client-visible frame while the display keeps showing them, the lock role's floor), the adaptive frame scheduler's per-surface budget floor (gaming 1 ms, creative 4 ms, desktop the operator's own), the composition decision engine (`CompositionPath` + the `DecisionRecord`'s named cause), and the compositor-owned transitions catalog (nine kinds, macOS-grammar springs, the window-open fade settling to byte-exact plain ink, `--transitions`) |
| v0.16.0 | 48 | the knock and the goodbye: the dialog arm served (`get_dialog` + the frozen `ldp.shell.dialog` interface — the machine's own serial clock, the centered placement at attach, the strict two-phase ack, the one-role rule enforced by name), the modal gate (a mapped modal dialog gates its parent's whole tree — the routing filter, the keyboard focus handoff, the modeless control), the sheet-dies-with-window doctrine (the parent's death closes its dialogs), and the window-close fade (the owned ghost: a row-tight copy of the dying window's last raster at its own z slot, the catalog's close spring, the vacated-rect removal claims, the A/B byte oracle) — zero wire surface moved, the freeze gate untouched |
| v0.17.0 | 49 | the states arm: the frozen 17-request `ldp.shell.toplevel` vocabulary served whole — the geometry verbs (maximize fills the workspace minus insets, fullscreen covers the output with the pin; ack then commit realizes the placement, the restore point returns on the un-verb), the visibility verbs (minimize hides immediately — no render, no input, parked frame requests (App Nap's seam), leave_output; set_workspace moves and reports the clamped actual; sticky shows everywhere), the hints (bounded strings, saturating sizes), the strict ack, the machine-owned handshake under the seat's interaction clock (the data gate's serial, `propose_at`), the CSD insets honest, `workspace_count` at bind, `--workspaces N` — zero wire surface moved |
| v0.19.0 | 51 | the focus and the view: the states arm’s follow-ons, part one — the frozen `activated` bit riding real proposals through every keyboard transition (the grace-parking `propose_state` never punishing a drag-draining client), the promotion doctrine (the frontmost window takes the keys when the holder leaves — hidden, unmapped, dead), and `shell.switch_workspace` served whole (the taskbar’s line: the clamp-honest `workspace_switched` broadcast to every binder, the visibility sweep with dialogs following their parents, the space-preferred focus, the hidden spaces’ frame parking, the no-op silence) — one request + one event appended additively, the freeze re-taken (98+123, 37 enums) |
| v0.20.0 | 52 | the drawn chrome: the SSD band served whole — the title bar, the border ring, and the close affordance painted as CPU ink on the dock's own model (cached per frame shape, the honest `NoFb` demotion), the frozen `toplevel.close` event's first sender (the drawn button's press arms, its release inside the same affordance fires, a drag away cancels — the caption doctrine; the ignoring client keeps its window, the server never force-kills), the claims ledger (the R2 rule's chrome sibling: a band that moved, resized, hid, or died claims its rect — minimize hides the chrome with the ink, unminimize restores the exact bytes), and the chrome-aware geometry regime (a maximized SSD window's *frame* fills the workspace area, the content inset by the applied band) — zero wire movement: the event was frozen since v1, waiting for its sender; 2,241 → 2,258 tests |
| v0.18.0 | 50 | the operator's hand: the interactive move/resize vocabulary the spec never grew, served whole — `start_move` (server-truth geometry at the pump's cadence, zero configures, the 48-px keep band), `start_resize` with the eight-edge grip (the edge algebra through the real two-phase commit, the `resizing` state riding live proposals, the position and the committed buffer realizing together, the final proposal clearing the state), the demotion (dragging a maximized window restores the floating size under the pointer's proportional grip), and the grace window (pointer-paced supersession never punishes a frame-cadence client — 8 superseded drag serials stay acknowledgeable, the verbs' strict doctrine preserved) — appended additively, the freeze re-taken (97+122, 37 enums) |
| next | future | the dma-buf negotiation surface (client pools over the wire, the feedback tranches' carrier — the frozen `ldp.core.dmabuf` interface awaits its honest `create` arm), the dock's dumb-buffer plane delivery, a dedicated render thread (the single-mutex doctrine is load-bearing for the byte-exactness oracles; the pipelining line stays open), the Vulkan renderer on the proven seam, the real-panel end-to-end latency lab (the giants' 90s ride years of measured hardware tuning; the architecture and the CI budgets are ours), the quirk table's per-vendor rows (the mechanism shipped in Phase 37; the rows themselves accrue with hardware — the giants' decades are the honest remainder), the foundry's RBv3/OVT arms (CEA-861-H/I's successors to the blanking story — Phase 46 poured every family the VESA standards define; the CTA's own remain), the transitions catalog's remaining drivers (the workspace/app-switch/fullscreen/display/lock kinds ship their curves; the close fade's ghost shipped in Phase 48 — the geometry slide and genie ride the render-thread line), the drawn chrome's own follow-ons (the server-side caption drag — the band as a move grip, `start_move`'s machinery at the pump's cadence; the title glyph — a font rasterizer of its own; the Liquid chrome-material dressing; the chrome-aware placement; the chrome ghost), the drag vocabulary's own follow-ons (press-issued serials on the wire — the device-event serial line; edge snapping — the snap-preview geometry; the live rubber-band between client commits — the render-thread line with the other geometry drivers), the private tier's screen-share negotiation (the broker-era line — `ldp.security`'s requests and the SessionBroker host in-process), per-surface home-output schedulers (the pacing grid is the primary's), multi-user remote identity (broker-era), TLS-class transport security |

### Standing rule

No phase may contain stub code. If scope must shrink, the roadmap gains a
visible line item — missing functionality is always *documented*, never
hidden.
