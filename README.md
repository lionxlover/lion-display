# LDP — Lion Display Protocol

**A modern, first-principles display protocol and server ecosystem for LionOS.**

LDP is the native graphics and input stack of [LionOS](https://lionos.example) — a
macOS- and Cutefish-OS-inspired desktop Linux distribution. It is an independent
alternative to Wayland/X11: a new wire protocol, a new object model, a new
scheduling architecture, and a new security model, written in safe Rust and
designed for deterministic, low-latency, high-refresh-rate compositing on
60/120/144/240 Hz+ displays.

LDP is **not** a Wayland fork, rename, or re-implementation. Existing Wayland and
X11 applications run through optional, out-of-tree bridge processes
(`ldp-wayland-bridge`, `ldp-x11-bridge`) that never contaminate the LDP core.

```
┌──────────────────────────────────────────────────────────────────┐
│  Applications:  native (LTK or any language) · Wayland · X11     │
├────────────────┬─────────────────────┬────────────────────────────┤
│  native client │ ldp-wayland-bridge  │ ldp-x11-bridge             │
│  (ldp-client)  │ (optional process)  │ (optional process)         │
├────────────────┴─────────────────────┴────────────────────────────┤
│                     LDP wire protocol (ldp-protocol)             │
├──────────────────────────────────────────────────────────────────┤
│  lion-compositor: shell · seats · input · clipboard · color/HDR  │
│  · VRR policy · capability security · a11y · power/session       │
├──────────────────────────────────────────────────────────────────┤
│  renderer (GL / software) · gpu (DMA-BUF, fences) · display      │
│  (DRM/KMS atomic) · transport (Unix sockets + FD passing)        │
├──────────────────────────────────────────────────────────────────┤
│  Linux: logind · evdev · DRM/KMS · DMA-BUF · sysfs · systemd     │
└──────────────────────────────────────────────────────────────────┘
```

## Design pillars

1. **Deterministic frame scheduling.** The compositor owns per-output frame
   timelines. Clients do not ask "may I draw?" — the server *pushes presentation
   deadlines* (`frame_target` events: target timestamp, refresh interval, latency
   budget). Clients schedule rendering against a deadline, the compositor commits
   against the vblank. Result: stable pipeline depth, no guess-driven latency.
2. **Explicit synchronization from day one.** Buffer handoff is fenced (sync-file /
   syncobj DMA-BUF), never implicit-and-undocumented. No GPU race conditions by
   construction; multi-GPU safe.
3. **Generational object identity.** Object IDs are validated against per-slot
   generations. Use-after-destroy on the wire is a *typed protocol error*
   (`stale_object`), never a misdelivered message.
4. **Capability security, deny-by-default.** Screenshots, screen recording, input
   injection, global shortcuts, display configuration and friends are gated behind
   unforgeable 256-bit access tokens minted by a broker. Baseline permissions come
   from a signed application manifest; the most sensitive operations escalate
   through a user-visible prompt (layered model). Every grant and denial is audited.
5. **First-class color.** Every surface carries a color description; every output
   advertises its color pipeline. PQ/HLG HDR, per-output tone mapping, ICC import
   and fractional scaling are core concepts, not bolt-on extensions.
6. **One protocol, many modules.** The core is small; shell, input, data exchange,
   color, security, accessibility and session form versioned modules described in
   reviewable TOML specifications and compiled into type-safe Rust code by `ldpc`.

## Feature matrix (target v1.0)

| Area | Highlights |
|---|---|
| Rendering | **the per-surface material request (v0.13.0): `toplevel.set_material` — the client claims its window's Liquid material over the wire, the freeze re-taken additively**; **EGL/OpenGL ES hardware compositing by default (v0.3.0)** with honest software fallback, **the Liquid visual engine (v0.6.0): rounded corners, soft shadows, frosted glass — tiered for low-end devices, byte-equal across backends**, **the positioning shell (v0.7.0): phone/desktop layout doctrine, cascade placement, the frosted system dock at the screen edge rising on a spring**, **the steady-state pass (v0.8.0): memoized materials and band-limited corners — the full Liquid scene at 60 Hz on a weak CPU, byte-identical**, Vulkan-ready `Renderer` trait, hardware overlay planes, direct scanout |
| Display | **real-KMS serving on hardware (v0.4.0) that follows the topology (v0.5.0): mapped dumb scanout, applied atomic modeset, page flips on the DRM fd, live monitor swaps without losing a session, the honest dark state, clean teardown**; DRM/KMS atomic modesetting, multi-monitor, mixed-DPI, mixed-HDR, hotplug, VRR, tearing opt-in |
| Color/HDR | sRGB/DisplayP3/BT.2020, PQ/HLG, HDR10 static metadata, BT.2390 tone mapping, ICC profiles, per-output pipelines |
| Input | keyboard (xkb), pointer (relative/absolute), touch, stylus/tablet (pressure/tilt), gestures, grabs, multi-seat; **per-contact, per-axis position-state coalescing (v0.11/v0.13): every touch and tablet axis — motion, pressure, tilt, shape, orientation — collapses to its contact's freshest sample per display frame** |
| Shell | **the positioning shell (v0.7.0): the layout doctrine from the output's shape (phone fill / desktop cascade / dialog center), placement at first attach, migration re-layout, the system dock reserving the usable area**; **the semantic-scene triple (v0.15.0): a toplevel claims its role (window/dialog/tooltip/overlay/lock), its exposure class, and its scene profile — appended additively, the freeze re-taken**; **the dialog arm (v0.16.0): `get_dialog` served — centered placement at attach, the machine's own serial clock, the modal gate, the sheet-dies-with-window doctrine**; **the states arm (v0.17.0): the 17-request toplevel vocabulary served whole — the geometry verbs (maximize/fullscreen through the real two-phase commit, the restore point), the visibility verbs (minimize/set_workspace/set_sticky with App-Nap parking and byte-exact restoration), the hints, the strict ack**; **the operator's hand (v0.18.0): `toplevel.start_move`/`start_resize` served — the title-bar drag (server-truth geometry at the input pump's cadence, the 48-px keep band, zero configures), the eight-edge grip (the edge algebra through the real two-phase commit, the `resizing` state, the position and the committed buffer realizing together), the maximized-window demotion under the pointer's proportional grip, and the grace window that keeps pointer-paced supersession from ever punishing a frame-cadence client**; **the focus and the view (v0.19.0): the `activated` state bit riding real proposals (the press sets it, the departure clears it, the frontmost window takes the keys when the holder leaves — one grace-parking proposal per transition, never punishing a client draining a drag's serials) and `shell.switch_workspace` served — the view switch (the taskbar's line): the clamp's honest reply `workspace_switched` to every binder, the visibility flip, the space-preferred focus, the hidden spaces' frame parking**;**the drawn chrome (v0.20.0): the SSD pass — the title bar, border ring, and close affordance painted as compositor-owned ink around every server-decorated window, and the frozen `toplevel.close`'s first sender (press arms, release fires, drag-away cancels — the caption doctrine)**; SSD-first decorations, toplevels/popups/dialogs, fullscreen/maximize/minimize, workspaces (4 by default, `--workspaces N`), focus/stacking |
| Data | clipboard + primary selection, MIME offers, streaming transfers, drag-and-drop |
| Security | capability tokens, manifest baseline, runtime escalation, audit log, **the security-aware capture (v0.15.0): `set_security_class(protected/system)` redacts the surface out of every client-visible capture frame while the display keeps showing it, the lock role's floor included** |
| Session | logind integration, idle/suspend/resume, DPMS, backlight, inhibitors, lock screen; **the session-rebuild supervisor (v0.13.0): `lion-supervisor` — the DWM crash-recovery doctrine, the pinned socket re-bound after a killed compositor, clients reconnect and the desktop survives** |
| A11y | screen-reader events, magnification, keyboard nav, settings broadcast |
| Compat | `ldp-wayland-bridge`, `ldp-x11-bridge` (separate processes, real subsets) |
| Tools | `ldpc`, `ldp-info`, `ldp-debug`, `ldp-validate`, `ldp-profiler`, `ldp-audit`, `ldp-input-debug`, `ldp-grab`, `ldp-bench`, `ldp-remote-gateway` |
| Remote | edge/hub TCP relay (`ldp-remote`), bearer-token auth, keepalive, the FD vocabulary — pool windows, fences, snapshots, pipes — pixel-exact over the wire |
| Capture | `ldp.capture` grab → read-once frames, memfd scanout copies, the deterministic zero-dependency PNG encoder (`ldp-png`), `ldp-grab`; **security redaction (v0.15.0): protected/system surfaces paint black in the captured frame — the user sees their window, the screenshot does not** |

## Repository layout

```
lion-display/
├── Cargo.toml            # workspace: 36 members added phase by phase
├── debian/               # Debian source package (lion-compositor + ldp-tools)
├── docs/                 # architecture, protocol, threat model, roadmap,
│   │                     # benchmarks, user/admin guides, maturity matrix
│   └── reference/        # ldpc-generated per-module protocol reference
├── spec/                 # the LDP protocol specification (TOML, source of truth)
├── crates/
│   ├── ldp-core/         # Phase 1: shared foundation types
│   ├── ldp-protocol/     # Phase 2: wire codec + generated schema tables
│   ├── ldp-transport/    # Phase 3: AF_UNIX transport, SCM_RIGHTS, framing
│   ├── ldp-server/       # Phase 4: sessions, object store, dispatch, audit
│   ├── ldp-client/       # Phase 5: connection, proxies, class lanes, reconnect
│   ├── ldp-compositor/   # Phases 6–7: surface tree, damage engine,
│   │                     # deadline frame scheduler, coalescing, replay
│   ├── ldp-renderer/     # Phase 8: Renderer contract + software backend
│       └── …             # gpu, display, … (Phases 9–16)
├── ldp-remote/          # Phase 21: the TCP relay (edge + hub gateways)
├── ldp-png/             # Phase 22: the zero-dependency PNG encoder
├── binaries/             # lion-compositor, bridge executables (later phases)
├── examples/             # hello-ldp, pointer-paint, clip-client, showcase
├── tests/                # stress / crash / hotplug gates (Phase 19)
├── fuzz/                 # five deterministic fuzz targets (Phase 19)
├── tools/
│   └── ldpc/             # Phase 2: the protocol compiler (spec → Rust/blob/docs)
├── assets/               # test fixtures, icons, EDID samples
├── packaging/            # systemd unit + packaging notes (debian/ at root)
└── scripts/              # verify.sh, build-deb.sh, gen-protocol.sh, package-phase.sh
```

Crates are added only when they are real: this repository never contains stub or
placeholder implementations. See `docs/roadmap.md` for the 32-phase build plan
and the exit criteria of each phase.

## Status

**v0.20.0 milestone** (Phases 1–52). The 20-phase bootstrap, the
post-milestone growth phases, the hardware-acceleration doctrine,
**the real-KMS serve loop**, **live output re-arrangement**, **the
Liquid visual engine**, **the positioning shell**, **the
steady-state pass**, **the desktop matrix**, **the served stack**,
**the semantic scene** (v0.15.0: a surface claims what it *is*, what
it may *expose*, and how the machine spends its frame budget on it —
`set_semantic_role`/`set_security_class`/`set_scene_profile` over the
wire; protected surfaces redact out of captures while the display
keeps showing them; the gaming/creative budget floors pace the
deadline walk; the compositor-owned window choreography fades windows
in on springs, `--transitions`) — **the knock and the goodbye**
(v0.16.0: the frozen dialog surface served — `get_dialog` with the
machine's own two-phase serial clock, the server-side centering at
first attach, the modal gate over the parent's whole tree, the
sheet-dies-with-window doctrine; and the window-close fade — the
dying window's owned raster fading out at its own z slot, the settle
byte-exact, zero wire surface moved) — **the states arm**
(v0.17.0: the frozen 17-request toplevel vocabulary served whole —
maximize/fullscreen as the real two-phase commit with the restore
point on the un-verb; minimize/set_workspace/set_sticky applied
immediately with App-Nap frame parking, leave/enter_output, and
byte-exact restoration; the machine-owned handshake under the seat's
interaction clock with the CSD insets honest; `workspace_count` at
bind and `--workspaces N`; the strict `ack_configure`; zero wire
surface moved) — **the operator's hand** (v0.18.0: the interactive
move/resize vocabulary the spec never grew, served whole —
`toplevel.start_move`/`start_resize` appended additively with the
`resize_edge` enum, the freeze re-taken; the move's server-truth
geometry at the pump's cadence with the keep band; the resize's
edge algebra through the two-phase commit with the `resizing`
state and the never-tearing realize; the maximized-window
demotion under the pointer's proportional grip; the grace window
bounding pointer-paced supersession honestly) — **the focus and the
view** (v0.19.0: the frozen `activated` bit riding real proposals
through every focus transition, and the view switch
`shell.switch_workspace`/`workspace_switched` appended additively —
the freeze re-taken, 98 requests + 123 events) —
— **the drawn chrome**
(v0.20.0: the SSD band served — the title bar, the border ring, and
the close affordance painted as CPU ink on the dock's own model,
cached per frame shape; the frozen `toplevel.close` event's first
sender: the drawn button's press arms, its release fires, a drag
away cancels; the claims ledger (the R2 rule's chrome sibling —
minimize hides the chrome with the ink, unminimize restores the
exact bytes); the chrome-aware maximize (the frame fills the
workspace area); zero wire movement) —
several displays at once (`--outputs multi`: one logical desktop,
per-output visibility, topology motion mid-session, spanning
release gates), the fractional-scale truth (`--scale F`: the
logical canvas, physical placement, the `preferred_scale` hints),
the HDR pipeline (`--hdr`: PQ caps over `output.hdr_caps`, the mode
controller's hysteresis, scene-linear blending with the PQ tail),
adaptive sync (`--vrr`: `VRR_ENABLED` on every capable flip, the
scheduler's window stretched to the CRTC's range), the idle ladder
(`--idle MS`: DPMS blanking, parked scheduler, wake on activity),
the **frozen v1 API contract** (`ldpc snapshot --check` — the spec
surface pinned byte-for-byte, drift breaks the build), and **the
clipboard over the real wire** (`ldp.data.data_device_manager`
served: devices, sources, offers as real receiver-held objects,
selection publication, accept/receive through real pipes
byte-exact, honest teardown — the family the bridges translate
foreign clipboards onto) — and, since v0.10.3, **the rootless
door**: every top-level X11 window rides its own LDP surface (the
shell places them, the Liquid materials dress them,
override-redirect windows ride the popup role's anchor/gravity
machine, input bridges the two coordinate truths — `--x11-rootful`
keeps the whole-screen escape) — and, since v0.10.4, **one desktop,
every arrangement**: `--outputs mirror` places every display on the
same desktop (the overlapping origins are the protocol's clone
signal, each display cropping to its own mode — proven per-pixel),
`--scale 1,2` serves one factor per output (the stretch rule carries
the doctrine across hotplugs), and the named-quirk table ships its
mechanism (every row with its symptom, its detection, and its
operator escape) — and, since v0.10.5, **the honest peak**: the HDR
panel negotiation — every HDR layer's declared mastering metadata
rides the BT.2390-structured knee onto the *negotiated* ceiling
(the stack's brightest content clamped to the panel's effective
peak), `--hdr-peak N` caps a bloated advertisement (the quirk
table's first accrued row), and `hdr_caps` replies with what the
render pass will actually deliver — and, since v0.10.6, **the fresh
frame**: the emission path serves the giants' latency tuning —
position-state coalescing (a 1000 Hz pointer parks sixteen samples
between display frames; a frame-cadenced client reads ONE `motion`
with the freshest coordinates, every `relative_motion` delta intact,
the discrete barrier sealing the click's own sample) and the
VRR-aware presentation clock (an adaptive surface's `presented`
reports the cadence the panel actually ran — the LFC fast end at
6.944 ms honestly — and the verdict fires at the flip that carries
the content, the pacing following the content) — and, since v0.10.7,
**the deep material**: the glass stops being one flavor — the
frost's saturation dial extends *past* the backdrop's own chroma
(`0..=510`, the acrylic/vibrant distance, integer-exact), every
material traces the luminous 1-px hairline inside its rounded
silhouette (the stroke that makes glass read as glass — memoized
imagery like the shadow, byte-equal across backends by
construction), and the desktop speaks one named family (panel,
sheet, menu, vibrant-dark, chrome): menus and popups wear the
vibrant menu glass, the dock wears chrome with the hairline, and
`--effects` still tiers the whole language for the weakest device.
Since v0.13, **the client claims its own window's material** —
`toplevel.set_material` over the wire (the NSVisualEffectView
doctrine, the request appended after the frozen opcodes: purely
additive), `default` clearing the claim, and the server keeping
its fullscreen and popup invariants plus the tier quality budget.
Since v0.13, **occlusion quiescing** — the App-Nap doctrine the
damage pass drives: a fully-occluded window's frame requests park
(no deadline for pixels that cannot reach the panel), its live
registration dies with `surface_hidden`, and the reveal answers at
the moment it can present; the occluded client stops spending the
power on frames nobody can see.
And since v0.10.8, **the quirk ledger**: the VRR row's own named
remainder — the giants' multi-year driver quirk tables — answered
with the structure the decades fill: the honest floor
(`--vrr-floor N`, per-output, the advertisement the panel cannot
sustain clamped out of every scheduling decision), the LFC cadence
and anti-flap latch (slow content bridges on phase-aligned repeats,
the boundary flap warmed away), and the mixed-desktop escape
(`--vrr-uniform` — one fixed clock across the seam for the
platforms whose cross-CRTC coupling flickers the fixed sibling);
five quirk rows tabled, each with its symptom, detection, escape,
and honest cost. And since v0.10.9, **the mode foundry**: the
"every display size" row's own named remainder — the giants'
decades of EDID/timing quirk coverage — answered with arithmetic
instead of a quirk table: `--synth WxH@Hz` pours a VESA CVT
reduced-blanking timing for any size (the custom-resolution machine
`xrandr --newmode` and Windows' custom-resolution form run on),
integers-only and never under the asked refresh; the EDID parse
grows its timing half (the detailed timing descriptors — the
preferred mode — and the range-limits envelope, the pixel-clock
ceiling gating every pour); the pour rides the wire as a
**user-defined mode** (the kernel's own vocabulary, validated by
the display engine at commit); it joins the protocol face's
advertised list (`xrandr --addmode`'s story, protocol-side); the
migration re-pours on the next display; and the quirk table
accrues the timing trio — `edid-rotten`, `edid-preferred-lie`,
`pixel-clock-ceiling` — eight rows tabled, each with its escape.
And since v0.14.0, **the foundry completes**: every timing family
the VESA standards define, one grammar (`--synth
WxH[@Hz][:family]`, default `rb`) — `rb2` (CVT reduced blanking
v2: the 80-pixel deep-color-era blank, the fixed 8-line sync,
**1-pixel horizontal precision** so 1366-class widths pour
exactly), `rb2v` (RB2 with the 1000/1001 video-optimized
multiplier — the 59.94 Hz class, geometry identical, only the
clock moves), `cvt` (standard CRT blanking: the GTF duty-cycle
machinery, the aspect-mapped vertical sync of Table 3-2), and
`gtf` (the 1999 generalized timing formula) — poured in exact
integer arithmetic, spec-faithful to the letter (the VESA CVT 1.2
and GTF 1.1 documents themselves fetched and followed formula by
formula), through the same two honest gates. And since v0.15.0, **the
semantic scene**: a surface is more than its buffer — the client
claims what it *is* (`set_semantic_role`), what it may *expose*
(`set_security_class` — protected surfaces redact out of every
client-visible capture while the display keeps showing them, the
lock role's floor included), and how the machine spends its frame
budget on it (`set_scene_profile` — the gaming 1 ms / creative
4 ms admission floors on the deadline walk); the composition
decision engine names every frame's path (direct scanout, hardware
overlay, split, full) with its auditable cause; and the
compositor-owned choreography (`--transitions`) fades windows in
on macOS-grammar springs — settling, byte-exact, to the plain ink.
And since v0.11.0, **the contact doctrine
and the living registry**: the fresh-frame doctrine extends to every
contact-carrying stream — a 120 Hz touchscreen parking sixteen
two-finger batches between display frames delivers TWO motions
(each finger's own freshest sample in its own slot, one frame
terminator, the downs intact), the tablet's per-tool axes are
position state of their own kind (a 240 Hz pen collapses to one
motion, one pressure, one tilt per frame), and the discrete barrier
seals a contact's own stream (a finger's down never freezes another
finger's sample) — while the registry becomes the display set's
changelog: the dark state *withdraws* the output global (every live
bind revoked first, then the `global_remove` barrier on every live
registry) and the relight re-advertises it, the ordering the spec's
own sentence always demanded. The release also lands the deepest
hardening pass since the crash corpus (the subsurface depth cap,
the cross-client attach fix, saturating offset accumulation, the
union-found damage merge, the bounded damage accumulation, the
replay allocation guard, the `GlobalRemove` decoder drift fix) and
the delivery-path economy (the transport writer's O(1) unsent-front
cursor, the frame delivery by borrow, the allocation-stable display
model, branch-free plane blends). And since v0.12.0, **the quiet
frame and the architecture answer**: the damage engine's
steady-state pass halves on a static desktop (the quiet 64-window
pass **−52.9%**, the one-moving-window load **−39.2%**, A/B-measured
against the pristine v0.11.0 engine — five guarded fast paths, every
equivalence pinned by regression tests and the per-pixel corpus),
the fuzz gate's own soak mode found and the release fixes a latent
harness panic (the one-byte `chunk_stream` source and its
empty-source splice sibling — the gate now clean at 20× scale),
and [`docs/comparison-windows-macos.md`](docs/comparison-windows-macos.md)
answers the field's biggest question at pipeline depth: ten stages
from the client's pixels to the lit panel, Windows' DWM/WDDM/DXGI
stack and macOS' WindowServer/CoreAnimation/IOSurface/Metal stack
walked beside this one, an honest ledger of what only time buys and
what only this repository has, every gap priced with its roadmap
line. The compositor
meets the machine it boots on: the GL probe reads the context's
identity and classifies the GPU (discrete / integrated / virtual /
software rasterizer / unknown — the startup line names the device,
`auto` refuses a CPU-rasterizer GL context because the specialized
software backend is the faster CPU path), `--resolution WxH` forces
any offered mode (the exact size or the honest failure listing the
panel's menu — and hotplug migrations honor the preference), and
the desktop matrix is benchmarked end to end: the full Liquid
desktop frame at 1080p / 1440p / 21:9 ultrawide / 4K — 1080p High
7.31 ms (137 Hz-capable), ultrawide High 15.1 ms (66 Hz — 60 Hz
holds at High through the 21:9), 4K High 24.7 ms after the budget
fix the matrix itself caught (the shadow memo's fixed 6 Mi-word
cap thrashed at 4K — 194 ms; scaled to the output, 7.9× faster,
same bytes), every size under the no-GPU doctrine's tier in budget,
69 MiB peak-RSS default bring-up (dual-1080p mock, dock rendered;
60 MiB dock-off — re-measured fresh by the deep-test audit, the
stale 19 MiB Phase-30 figure corrected) —
with the honest world comparison committed as
[`docs/comparison.md`](docs/comparison.md). The phone-frame
benchmark measured 178.6 ms per frame at v0.7.0, it now
measures 8.70 ms steady state (20.5×, byte-identical) through
memoized shadow and frost materials, band-limited corner arcs, and
a rebuilt box blur. The positioning shell — where the
system puts windows and the dock it keeps for itself: the output's
shape picks the doctrine (a portrait output is a phone — apps anchor
above the dock; a landscape output is a desktop — windows cascade),
the system dock frosts at the screen edge and rises on a spring, and
migrations re-place the whole desktop onto the new screen —
`--mode drm` drives real hardware (mapped dumb scanout, applied
atomic modeset, page flips on the DRM fd, LDP clients on the socket,
clean teardown) *and follows the topology*: a monitor swap migrates
the served pipeline mid-session, the last display going away leaves
the compositor honestly dark while the protocol keeps serving, and
the first re-plug paints the desktop again — no session lost; GPU
compositing by default, honest software fallback, one coherent
version across every surface asserted by the release-coherence gate:

* **Phase 1 — foundations:** complete architecture documentation
  (`docs/architecture.md`), the v1 protocol specification (`spec/*.toml`,
  8 modules), the TOML spec grammar, the threat model, the 20-phase
  roadmap, and `ldp-core` (object identity, versions, error taxonomy,
  wire value model, geometry/damage algebra, frame timing, color/HDR
  types, buffer formats, capability bitsets, access tokens, limits) —
  zero dependencies, `#![forbid(unsafe_code)]`.
* **Phase 2:** `tools/ldpc`, the protocol compiler
  (TOML → validated model → Rust schema tables, typed enums/bitsets,
  opcode constants, introspection blob, reference docs — committed
  output, CI drift gate), and `crates/ldp-protocol`, the wire codec
  (16-byte envelope, tagged argument units, validation stages 1–3,
  schema registry) with every v1 operation round-trip
  tested and a malformed-message corpus rejected at the right stage.
* **Phase 3:** `crates/ldp-transport`, the AF_UNIX
  transport: listener/connect with `SO_PEERCRED` credentials, framed
  message reader/writer with `SCM_RIGHTS` FD passing (header count
  cross-checked, every FD of a rejected message closed), send-side
  local validation, backpressure hooks with hard queue bounds, and
  FD hygiene asserted by process-wide leak counts — 10k FD-passing
  round-trips, an FD-bomb corpus, and slow-peer tests all green.
* **Phase 4:** `crates/ldp-server`, the session core:
  accept loop with credentials and the client ceiling, per-client
  sessions running the full §9 pipeline (framing → decode → object
  resolution before signature check → dispatch), the generational
  object store, built-in `connection`/`registry` semantics
  (hello/welcome, `get_registry` bootstrap, bind, the central
  destroy/destroyed lifecycle, sync, introspection with compact JSON),
  the `Dispatcher` seam for every other interface (with FD hand-off,
  object creation and revocation through `DispatchCtx`), audit hooks,
  and crash reclamation. Exit criteria green: mock clients complete
  full sessions over the raw codec; stale-ID and type-confusion
  attacks rejected; 1,000,000 bind/destroy round-trips across 1,000
  clients with flat FD and session counts; a `kill -9` mid-frame peer
  is reclaimed while the server keeps serving others.
* **Phase 5:** `crates/ldp-client`, the client library:
  `Connection` with the `hello`/`welcome` handshake and the blocking
  driving model (`pump_one` / `dispatch_budget` / `roundtrip`), the
  proxy map with destroy-window states and automatic proxies for
  server-announced objects, schema-checked sends plus the typed
  factory flow (`create_object`), and the **class-lane scheduler** —
  five dispatch lanes where input outranks presentation and control
  events are barriers against the whole past, so presentation floods
  can never starve pointer motion. Reconnect is a session rebuild:
  bounded exponential backoff driving an application `Rebuild`
  callback. Exit criteria green: a test client completes a full
  session against a live `ldp-server` (handshake, registry bootstrap,
  global replay, bind, factories, sync, ping, destroy, clean
  disconnect — plus the failure paths), and the latency property
  tests hold: one input event jumps a 100,000-event presentation
  backlog; randomized and live floods never starve input.
* **Phase 6:** `crates/ldp-compositor`, the scene graph
  and damage engine: the `SurfaceTree` with nested subsurface roles,
  validated stacking orders, and the atomic commit cascade
  (sync-stash / root-flush semantics); the **damage engine** built on
  the normative per-pixel fold model (translucent surfaces contribute,
  opaque surfaces wipe below) with four exact rule families — content,
  coverage (with move-vs-geometry split), opaque flips, and
  subtree-scoped restack pairs — computing repaint, scanout, and
  per-surface presentation damage with occlusion subtraction through
  the `ldp-core` region algebra. Plus the MRU/keyboard `FocusStack` and
  immutable `Arc`-shared per-frame snapshots for the render path.
  Exit criteria green: a randomized corpus (5 seeds × 80 frames of
  create/commit/move/restack/destroy churn) matches an independent
  per-cell reference implementation of the fold model **exactly**, on
  both repaint and presentation damage, every frame; occlusion and
  snapshot-immutability tests pass under a 120-round mutation storm.
* **Phase 7:** the scheduler half of `ldp-compositor`: the
  per-output `FrameScheduler` (event-driven, timestamped inputs, no
  clock reads — replayable by construction) implementing the frame
  contract — one live registration per surface, deadline =
  `predicted vblank − submit_cost − flip_latency` for the
  `depth + extra_lead`-th vblank ahead, commits satisfying the window
  bind to it, and exactly one terminal event per registration
  (`presented` / `frame_dropped` with `deadline_missed`,
  `superseded`, `surface_hidden`, `output_off`). The vblank PLL
  extrapolates through the effective step so deadlines stay exact at
  lock under real period drift; a miss-streak escalation ladder hands
  slow clients deadlines they can meet (hysteretic de-escalation);
  immediate mode presents torn; adaptive mode widens the deadline into
  a window. Emissions flow through the §10.4 class-aware
  `CoalescingQueue` (input never dropped/reordered, presentation and
  configuration coalesce to latest per key, data never dropped), and
  every session records to a checksummed binary artifact that replays
  to the identical event vector. Exit criteria green: 15 golden
  timeline scenarios pin exact emission vectors; 8 hit-rate properties
  under jitter hold across seeds (PLL lock under drift, slow-client
  convergence, stall re-anchor, adaptive-vs-control, multi-client
  fairness); determinism replay verified for every property scenario.
* **Phase 8:** `crates/ldp-renderer`: the backend-agnostic
  `Renderer` contract (begin_frame(output, damage) → submit(back-to-front
  layers) → end_frame → statistics) and the reference **software backend**
  — a damage-clipped scanline compositor with four resolved pixel paths
  (row word-copy, write-only opaque, bit-stable integer premultiplied
  `over` in encoded space, scene-linear pipeline for cross-description
  composites). Format-complete sampling of all eleven v1 fourccs (full and
  studio ranges, P010 10-bit, YCbCr matrices derived from the primaries'
  chromaticities with integer chroma pivots), all eight buffer transforms
  with exact integer 1:1 inverses plus a continuous nearest-sample scaled
  mapping, per-row merged damage spans (overlapping damage never
  double-blends, undamaged pixels survive partial re-submission), and the
  Phase-14-facing color pipeline: range → transfer (sRGB, gamma 2.2/2.8,
  PQ, HLG) → XYZ-derived primaries conversion → reference-white anchor.
  `scanout_candidate` is the scheduler's direct-scanout pre-filter.
  Exit criteria green: golden pixel suites pin all 11 formats against
  independently-written decoders (RGB family byte-exact, YUV ±1 LSB with
  definitional gray/black/white/red anchors), hand-computed blend and
  damage-clipping vectors, forward-referenced transform frames, and
  color-pipeline anchor models; the 4K single-surface EC composites in
  ~3.5–3.9 ms median (release, 2-core CI) against the < 8 ms gate.

* **Phase 9:** `crates/ldp-display` + `crates/ldp-gpu` —
  the hardware path. `ldp-display` wraps atomic modesetting behind the
  object-safe `KmsBackend` trait with two interchangeable
  implementations: `MockDevice` (a deterministic in-memory DRM —
  EDID-carrying connectors, a VRR-capable CRTC, planes with
  `IN_FORMATS` capability blobs, kernel-grade commit validation, and
  injected-clock scanout timelines where VRR flips land at
  `max(commit, last+min)` and idle panels stretch to max) and
  `DrmBackend` over `dlopen("libdrm.so.2")` with the full audited
  symbol table. Commits are declarative — property-name addressing,
  backend-minted blobs, a 20-reason typed rejection taxonomy in the
  kernel's own check order — and `OUT_FENCE_PTR` comes back as an owned
  sync-file descriptor. The periphery ships too: EDID identity
  parsing/synthesis, the exact `drmModeModeInfo` blob codec (59.94 Hz
  stays distinguishable from 60), the `IN_FORMATS` codec, sysfs
  backlight policy with monotone integer ramps, and udev hotplug
  monitoring. `ldp-gpu` adds render-node discovery with a deterministic
  selection policy, DMA-BUF descriptors with the exact
  EGL attribute codec (round trip included), the fence model (sync
  files, timeline points, mock tokens, AND-merges, one injected clock),
  and the EGL bootstrap state machine — the real `libEGL.so.1` path
  failing typed on GL-less machines, the mock importing by decoding the
  real attribute encoding. Exit criteria green: atomic-commit goldens
  and VRR proofs on the mock (byte-reproducible event traces), the
  honest headless outcomes asserted for the real layers, and the
  bit-comparable harness — the same layer list through the reference
  software renderer and the mock EGL path agreeing byte-for-byte.

* **Phase 10:** `binaries/lion-compositor` — the vertical
  slice. One binary wires the whole stack end to end: an
  `ldp-server` on an abstract-namespace socket (one session thread per
  client), every window-system decision inside one
  `CompositorDispatcher` — the "compositor is a dispatcher" doctrine —
  over a single world lock holding the scene, the device, the
  renderer, and the double-buffered scanout chain. The headless time
  doctrine keeps the whole system deterministic: the mock KMS clock
  advances only at protocol wake points (`Dispatcher::on_wake` fires
  after every handled message), the pump renders pending damage,
  submits `PAGE_FLIP_EVENT|NONBLOCK` atomic flips, and advances the
  clock *exactly* to pending flip landings — never speculatively — so
  the served event stream is a pure function of the message sequence.
  Buffer exchange is real: `shm.create_pool` mmaps the received
  descriptor read-only (the audited sys layer), buffers validate
  eagerly, `attach`/`commit` splice them into the tree's pending
  state, and superseded buffers get a `buffer.release` fence (a
  signalled eventfd in headless — the documented stand-in for the
  kernel sync-file the DRM path will hand out). The output global
  replays its full bind cascade (geometry/EDID identity, mode list,
  VRR window, name) through the new `on_bind` hook, and `on_destroy`
  tears scene resources down through the central lifecycle. Exit
  criteria green: the full-session integration suite drives a real
  client through handshake → registry → globals → shm pool → buffer →
  surface → `frame_target` (deadline on the exact 60 Hz grid) →
  commit → `presented` (vblank-exact timestamps) → buffer release →
  clean teardown; pixel-truth suites pin scanout bytes for subsurface
  stacking, premultiplied blending, incremental damage, and
  detach/destroy repaints; presentation suites pin the deadline
  contract, late-commit drops, superseded registrations, and release
  fences; lifecycle suites pin fatal validation codes, pool growth,
  and two-client isolation. 757 workspace tests green.

* **Phase 11:** `crates/ldp-input` + `crates/ldp-seat` —
  input & seats. `ldp-input` owns everything between `/dev/input`
  bytes and normalized events: the 24-byte evdev codec with
  `SYN_REPORT` framing, the device model with kernel protocol-B
  multitouch slots, the unit-honest normalizer (0..1 axes,
  millimeters from axis resolution, fuzz dead-zones, tilt and wheel
  conversions, autorepeat filtering — repeat is modeled server-side),
  the two-tier smooth-curve pointer acceleration (latency-first,
  property-tested: monotone, bounded, C¹ at both tier joins,
  direction-preserving), the touchpad gesture machines (swipe / pinch
  / hold with uniform ending semantics, two-finger scroll emulation
  that converts to a pinch when fingers diverge, single-finger
  pointer strokes), key repeat schedule arithmetic, keymap
  compilation through `dlopen("libxkbcommon.so.0")` with sealed-memfd
  client descriptors and header-verified state constants, and the
  `/dev/input` backend whose headless results are typed and honest
  (empty enumeration, `NotEvdev` — exercised against a memfd in CI).
  `ldp-seat` turns those events into the `ldp.input` wire vocabulary:
  `SeatManager` with udev-tag device assignment and capability
  recompute, subpixel input-region hit testing with the three focus
  domains (pointer follows hit tests, keyboard is shell-driven, touch
  is grab-driven), the grab model (implicit while buttons hold,
  explicit popup grabs, capability-gated keyboard grabs, dismissal on
  surface death), and the per-seat router. The exit criteria are the
  golden corpus — byte-literal evdev traces → wire-encoded protocol
  events across eleven scenarios (mouse, keyboard + modifiers,
  touchscreen, touchpad scroll and swipe, tablet proximity/axes) with
  codec round-trips and byte-stability — plus the acceleration
  property suite and the multi-seat isolation suite (interleaved
  input, independent positions/focus/keymaps, nothing ever crosses).
  860 workspace tests green.

* **Phase 12 — this phase:** `crates/ldp-shell` — the shell layer:
  window roles, the configure/ack two-phase commit, server-side
  decoration geometry, spaces, stacking and focus policy. The
  toplevel machine carries the six spec state flags as *intents*
  (maximize/fullscreen/minimize/activated/sticky/resizing — minimize
  clears activation, activation clears minimize) and derives
  proposals from policy inputs: fullscreen covers the whole output
  with zero insets, maximized fills the workspace area minus the SSD
  insets, both clamped to the client's own size hints — which
  saturate when the client sends contradictory min/max pairs (the
  hint pair stays consistent by construction). The two-phase commit
  is exact: at most one live proposal, a new configure supersedes (and
  kills the serial of) the previous one, an ack must reference the
  live serial (stale acks are typed errors), the next commit realizes
  the acked proposal and nothing else. The serial clock is wrapping
  monotonic with reservation skipping plus a crash-recovery
  `resume_from` watermark, so stale pre-crash acks can never collide
  post-restart. The popup machine implements anchor/gravity placement
  (gravity = the direction the popup grows from the anchor point) and
  the slide/flip/resize constraint pipeline — slide caps at the
  anchor rect's opposite edge (never detaches), flip mirrors
  anchor+gravity on the still-offending axis and only accepts a
  fitting mirror, resize clamps the overflowing extent. The dialog
  machine adds modality gating and policy centering over the parent's
  content area. Spaces are macOS-style: per-seat active space,
  sticky-or-assigned visibility, count-shrink reflow that reports
  every moved window. The stacking layer keeps per-space back-to-front
  order with dialogs above their parents, modal gating on focus, MRU
  activation with *attribution* (the cause of every focus change is
  recorded for a11y and audit), and deterministic reflow when spaces
  change. SSD geometry is authored logical and ceil-scaled per output
  — mixed-DPI is first-class: migrating a toplevel between outputs at
  different scales re-derives its insets and forces a fresh serial.
  The exit criteria: schema conformance (every request/event of
  `spec/shell.toml` has a typed handle, checked in *both* directions
  against the compiled schema), golden lifecycles whose every emitted
  message passes the encode → decode → strict-signature gauntlet
  byte-stably, 20k-step fuzzed configure sequences over a six-window
  fleet that never wedge the shell (invariants asserted after every
  step), popup-solver property suites against an independent reference
  model, and the mixed-DPI inset suite across scales 1×–3× including
  a fuzzed Q8.8 scale table. 955 workspace tests green.

* **Phase 13:** `crates/ldp-clipboard` — the data
  exchange layer: sources/offers, MIME negotiation, streaming
  transfers over pipes, primary selection, and the drag-and-drop
  state machine. MIME types are validated to an RFC 6838 subset
  *before* storage anywhere (the validation-before-allocation
  doctrine), canonicalized (lowercased tokens, sorted parameters),
  and matched with `text/plain` charset folding — the
  ASCII-compatible family (`utf-8`/`utf8`/`us-ascii`/latin `ascii`
  aliases) compares equal, other encodings stay distinct, and
  preference-ordered negotiation picks the first want the offer list
  satisfies. The seat carries two unified slots (clipboard +
  primary selection): setting requires a serial that references the
  seat's input record, ownership eviction cancels the previous owner,
  null clears are owner-only, and publication goes to every non-owner
  device client (the owner never sees its own content announced).
  The DnD negotiation is pinned: the drag starts with all actions;
  the entered receiver narrows via `set_actions`; the server
  re-announces the narrowing to the source (`data_source.actions`);
  at drop the compositor picks copy over move over ask from the
  narrowed set, announces the singleton to the source (its last
  `actions` event carries the negotiated action — `dnd_finished`
  takes no argument), and a declined (empty) narrowed set cancels the
  drag. One offer per drag, reused across leave/re-enter; every abort
  path (release, origin/icon death, source death, teardown) funnels
  through one cancel choreography so `cancelled` + `leave` + offer
  death happen exactly once. Transfers stream: the server never
  buffers more than a page — the pump is a *stateful* page-sized
  engine (a sink that would-blocks mid-chunk keeps its pending tail;
  the fuzz proved a stateless step loses bytes), the registry admits
  under the client FD budget with check-then-commit receives
  (rejections leave no trace), and gate → validate → admit → `send`
  ordering. The permission model: *reading* clipboard/primary data
  requires the `clipboard_read` scope (manifest baseline or escalated
  token — deny-by-default with audit records), *setting* the
  selection needs no scope (ownership is not a read), and DnD drop
  receives are user-intent-authorized (the drop is the grant). The
  exit criteria: schema conformance in both directions against the
  compiled `ldp.data` schema, golden lifecycles through the
  encode → decode → strict-signature gauntlet, a 2,000-case transfer
  fuzz (random MIME/size/pattern over adversarial stalling/failing
  halves) that never deadlocks and loses no bytes, a 30k-step DnD
  transition fuzz that never wedges, the 100 MB stream through real
  pipes under limits (byte-exact streaming checksums, one-page
  working set, FD-table leak assertion), and the full permission
  matrix including no-trace denials. 1,048 workspace tests green.
* **Phase 14:** `crates/ldp-color` +
  `crates/ldp-hdr` — the color pipeline. `ldp-color` is the canonical
  math: the six transfer curves as f32 encode/decode pairs (bit-identical
  to the renderer's Phase 8 hook, pinned together by a cross-consistency
  suite) plus the single f64 PQ reference every table and tone curve
  evaluates through; the XYZ-derived primaries matrices anchored against
  the published IEC/ITU values (sRGB, BT.2020, DCI-P3, the BT.709↔BT.2020
  conversions, the Bradford D50↔D65 adaptation with its published matrix);
  the exact integer PQ ramps (10/12-bit, generic 8..16) whose
  `decode(encode(l))` returns the same integer code with zero float
  rounding drift — the architecture's no-drift rule realized as a table
  (the f32 curve pair is documented as the approximation, the table as
  the authority); luma-preserving gamut mapping in two modes (hard clip
  with the bit-exact in-gamut identity fast path; a soft-knee perceptual
  mode compressing the chroma ratio through a smoothstep shoulder —
  continuous across the gamut edge, monotone, always in-gamut); the
  BT.2390-structured EETF (PQ-domain normalization, 75% knee, luminance
  identity below the knee, a C¹ Hermite rolloff whose end slopes satisfy
  the Fritsch–Carlson monotonicity condition, switching to a smoothstep
  branch when the display is compressed past 3:1 in PQ terms — where any
  slope-1-at-knee cubic is provably non-monotone; master peak lands
  exactly on display peak); ICC v2/v4 matrix-TRC import (header/tag-table
  validation with typed rejections, curveType tables and parametricCurve
  types 0–4 with their piecewise inverses, `chad`-exact D50→D65
  de-adaptation, resolution to the closest parametric description per the
  `color_profile.description` contract) plus a canonical deterministic
  writer whose parse↔write round-trips are byte-identical; and the
  per-output tail (`OutputTransform`) composing tone-map scale →
  reference-white normalization → gamut map → transfer encode over
  absolute-nits inputs. `ldp-hdr` is the policy layer: HDR10 static
  metadata validation/normalization (CTA unknown sentinels, FALL ≤ CLL,
  the PQ ceiling), the CTA-861.3 HDMI HDR Static Metadata InfoFrame codec
  (30-byte packet with self-verifying checksum and parse-back
  round-trips), luminance adaptation (the SDR-in-HDR canvas level —
  BT.2408's 203-nit default — monotone in input and canvas, and
  BT.2100's peak-dependent HLG system gamma with the published anchors),
  and the output-mode decision table (SDR/PQ/HLG, PQ winning as the
  interchange EOTF) with dwell hysteresis so the panel mode never flaps.
  The exit criteria: the KAVT suite — every formula checked against an
  independently transcribed f64 reference plus published SMPTE/ITU
  anchors (PQ 100 nits ≈ 0.508 / 203 ≈ 0.5806 / the 7.309559e-7 black
  signal, HLG 0.75 → 0.264898, the sRGB 0.5 anchors, the luma
  coefficient pairs, the published matrices, the BT.2390 structural
  anchors with the 203-nit → mid-80s landing); ICC parse round-trips on
  three fixture profiles (sRGB-class v2 table TRC, Display-P3-class v4
  parametric + chad, gamma-2.2 v2) exact in model and bytes, with
  resolution fitting the right descriptions and the de-adapted matrices
  recovering the standard primaries; and luminance-adaptation
  monotonicity fuzzed over randomized corpora, end to end through the
  output tail. 1,128 workspace tests green.
* **Phase 15:** `crates/ldp-vrr` — the adaptive-sync
  policy engine. The decision table (off / deadline / always × panel
  support × adaptive demand × battery saver, each row with a
  machine-checkable rationale) picks the effective window and hands
  the deadline scheduler exactly the widening the panel can honor:
  `max − nominal` under deadline policy (a commit past the plain
  deadline but inside the window still presents — the window-late
  save — at its own stretched flip, tear-free), the full `max − min`
  span under always (commit-at-ready). The selector owns the flip
  times: flip-slip avoidance clamps every flip into
  `[last + min, last + max]` (never two flips closer than the minimum
  refresh interval), below-minimum-rate frames under deadline policy
  hit the fixed-rate fallback and defer onto the nominal grid — the
  reserved `throttled` drop reason, with a retry hint — and without
  the fallback they catch up at readiness. Tearing is a separate
  opt-in (immediate-mode surface + async-flip capability + session
  permission) proven independent of VRR at gate, scheduler, and KMS
  seam levels; the async-flip path lands as
  `CommitFlags::PAGE_FLIP_ASYNC` in `ldp-display`. The exit criteria: the
  exhaustive 36-row policy-table conformance walk; scheduler+VRR
  golden timelines including mock-KMS panel integration (engine
  rulings match the device's flip completions to the nanosecond;
  `presented` feedback carries the measured VRR interval); and the
  tearing/VRR isolation matrix plus a 2,000-case randomized corpus.
  1,198 workspace tests green.
* **Phase 16 —** `crates/ldp-security`, `crates/ldp-accessibility`, `crates/ldp-power`, `crates/ldp-session` — security,
  accessibility, power, and session management:
  Security: the permission matrix (every threat-model §3 row as an
  executable requirement class; the 18-row × 3-state exhaustive walk is
  the exit criterion), app manifests with canonical SHA-256 hashing,
  the session broker — the only grant authority — with manifest gates,
  per-app rate limiting, brokered user prompts (the `PromptDecider`
  seam), and TTL'd 256-bit token minting through the injectable
  entropy seam; the grant table validates submissions with
  constant-time byte compares and app binding (forgery, cross-app
  replay, and revoked resubmission deny; reconnect resubmission is
  legal); the audit chain hash-links every decision through pure-Rust
  SHA-256 (NIST-pinned) with window-anchored ring verification,
  tamper/reorder/truncate detection, class filtering, and JSONL lines.
  A11y: the settings broadcast (coalescing queues), the
  provider/subscriber event bus with global-seq stable-merge ordering,
  push-time focus attribution, `a11y_control` gating, and bounded
  per-provider backpressure (the 5,000-event corpus pins the ordering
  property), plus the magnifier geometry (follow selectivity, lens
  clamping, Q8 zoom steps). Power: the inhibitor-aware idle ladder
  with logind delay semantics, the monotone backlight ramp, and
  suspend/resume re-anchoring — `grid_align` computes the first
  phase-preserving vblank boundary after the wake, proven against the
  real compositor frame clock at 144/90/60/40 Hz across sleep depths
  (post-resume predictions land exactly one nominal interval past the
  re-anchor, never in the past). Session: the D-Bus wire codec
  implemented from the specification (golden Hello/TakeControl bytes,
  full alignment discipline, bounds-checked parsing), the connection
  state machine over the `DbusTransport` seam, the logind session
  machine (TakeControl/TakeDevice, pause/force/gone, ResumeDevice,
  Lock/Unlock, PrepareForSleep), VT-switch choreography, the lock
  screen with the only-lock-surfaces focus gate, and the inhibitor
  cookie registry cross-checked against the power layer's mask.
  1,328 workspace tests green.
* **Phase 17:** `crates/ldp-x11-bridge`,
  `crates/ldp-wayland-bridge` — the compatibility proxies. X11: the
  pure-Rust server subset (the full core request table over a window
  tree with exact visibility/expose diffs and gravity-retained
  backing stores, properties/atoms/selections, focus and pointer
  routing with crossing details, the whole GC/drawable vocabulary
  with GX/plane-mask/clip rasterization, arcs and polygons),
  BIG-REQUESTS framing, MIT-SHM over a host seam, and the rootful LDP
  driver that composites into one toplevel with exact damage —
  xeyes/xclock-class lifecycles run byte-for-byte in CI, Xvfb-free.
  Wayland: the wire codec, registry replay, double-buffered
  wl_surface commits under the xdg configure/ack handshake, seats
  with the monotonic-serial discipline, and xdg-shell popups whose
  positioners translate one-to-one onto LDP's own placement
  vocabulary — weston-terminal-class clients map, type, and resize.
  Both drivers gate on a real bridge-scope token (32-case forgery
  corpus, cross-app denial) through the broker seam, and translate
  foreign implicit sync into LDP's explicit-commit readiness
  statement. The core architectural lint (no Wayland/X11 vocabulary
  in any core crate) is green.
  1,466 workspace tests green.
* **Phase 18 — this phase:** `tools/ldp-tools` + the example
  applications — the operator toolchain. One library crate, six
  binaries: `ldp-info` (protocol + live session + DRM/GPU probe, the
  honest-degradation doctrine), `ldp-debug` (live event tracer with
  generated presentation traffic + scheduler-recording replay),
  `ldp-validate` (spec-set compilation + live served-schema
  cross-check), `ldp-profiler` (live frame-pipeline latency /
  deadline hit-rate measurement + LDP7REC replay), `ldp-audit`
  (hash-chained audit-log reader/verifier with operator checkpoints
  — deletion, reordering, truncation, and field edits each caught by
  the right layer), and `ldp-input-debug` (evdev dump analysis:
  raw → SYN_REPORT frames → normalized events, with device-spec
  inference). The shared core: the `ToolSession` driver (the Phase 10
  testbench promoted to a public library), socket discovery
  (`--socket`/`LDP_SOCKET`, abstract namespace), wire-value
  formatting, and the audited `sys` seam (memfd + fence reads —
  every `unsafe` SAFETY-commented). Examples: `hello-ldp` (the
  smallest complete client — connect, one committed frame, the
  presentation verdict, pixel-checked in scanout), `pointer-paint`
  (evdev trace → real Phase 11 normalizer → per-motion pixels in a
  committed surface — the stroke lands pixel-exact), and
  `clip-client` (live availability report + the full in-process
  offer/accept/receive negotiation through the real Phase 13
  manager, payload crossing a real pipe). The exit criterion — every
  tool runs against the Phase 10 compositor in CI — holds as
  compiled binaries over the real socket (7 integration scenarios
  including the dead-socket fast-fail). Two latent bugs the tools
  flushed out, both fixed: the compositor's abstract-socket bind
  doubled the namespace NUL marker (external connectors could never
  dial in), and the session pool sizing starved ping-pong buffers.
  1,537 workspace tests green.
* **Phase 19:** `crates/ldp-test`, `fuzz/`, `tests/`, `ldp-bench` —
  the test, fuzz, stress, and benchmark layer. The deterministic
  support library (seed-addressed SplitMix64 rng, byte/envelope
  mutators, the spec-walking conformance runner — canonical
  synthesis, strict round-trip proof, and the per-type boundary
  matrix over every one of the 210 compiled operations — plus the
  JSON writer and the benchmark harness). Five deterministic fuzz
  targets (codec, transport over real socketpairs with SCM_RIGHTS
  and FD-stability proof, dispatch against a real accept loop, X11
  framing in both endiannesses, Wayland pinned-schema decode) under
  one serialized CI gate with a counting panic hook; `LDP_FUZZ_SCALE`
  for soak runs. The cross-crate integration gates over the real
  in-process compositor: the stress gate (32 clients, mixed
  workloads, crash-and-reconnect — compressed in CI,
  `LDP_STRESS_FULL=1` runs the 15-minute gate: 30,056 sessions,
  4,909 abrupt disconnects reclaimed, 129,819 frames, FD baseline
  restored), the crash corpus (eight protocol cut points plus raw
  mid-frame and seeded churn), and the hotplug simulation.
  `docs/benchmarks.md`: the committed release-build report with the
  JSON schema. Two real bugs flushed out on the harnesses' first
  runs, both fixed with regression suites: the Wayland bridge's
  length-bomb DoS (unbounded input buffering on a ~2 GiB length
  claim — capped at one LDP large frame) and the compositor's
  post-mortem outbox leak (a crashed client's pending buffer release
  re-creating its outbox queue and leaking an eventfd). 1,576
  workspace tests green.
* **Phase 20 — release:** the v0.1.0 milestone. Debian packaging
  (`debian/`: source `lion-display`, binary `lion-compositor` +
  `ldp-tools`, debhelper-free rules under `Rules-Requires-Root: no`),
  the hardened systemd unit, `scripts/build-deb.sh` with the
  fresh-root dpkg install gate, the user guide
  (`docs/user-guide.md`), the admin guide (`docs/admin-guide.md`),
  the maturity matrix (`docs/maturity.md`), and the all-in-one
  bundle `lion-display-v0.1.0.zip`.
* **Phase 21 — remote transport:** `crates/ldp-remote` +
  the `ldp-remote-gateway` binary — LDP sessions across hosts. An
  edge gateway accepts ordinary local clients (unmodified
  `ldp-client`/tools/examples dial it like a compositor) and bridges
  each session over authenticated, keepalive-monitored TCP to a hub
  near the compositor; the envelope wire validates every length
  before allocating (length-bomb-proof), and the FD relay vocabulary
  carries the protocol's descriptor state as bytes: per-commit pool
  windows (scanout stays **pixel-exact** over the wire — proven by
  running the showcase example through both gateways), pool growth,
  eventfd fences, keymap/ICC snapshots, and streamed clipboard pipes;
  GPU descriptors are refused explicitly, never silently wrong.
  Alongside: `Region::subtract` ~2× faster, permanent renderer +
  remote benchmark rows, and the whole workspace green under the
  current stable toolchain (Rust 1.98, MSRV 1.75). 1,623 tests.
* **Phase 22 — capture, PNG, and the next performance tier:** the
  protocol's first *growth* — the ninth module `ldp.capture`
  (`spec/capture.toml`: `grab` → read-once `frame(fd, width, height)`
  snapshots) landing through the full ldpc pipeline and relaying whole
  through the remote gateways (captures over the wire are
  **pixel-exact**, proven in the loopback suite); the zero-dependency
  deterministic **`ldp-png`** encoder (adaptive filters +
  fixed-Huffman deflate, round-trip proven by an independent decoder);
  the seventh operator tool **`ldp-grab`** (PNG/PPM screenshots,
  multi-frame sequences); and two renderer optimizations with
  permanent benchmark rows — the `CopyPremul` word-copy path for
  opaque ARGB composites (**7.4×** on the 4K shape) and the per-frame
  damage row index (O(intervals) per layer-row). CI gained the MSRV
  job (1.75 tested, not declared) and the dependency policy guard.
  1,700+ tests.
* **Phase 23 — release:** the v0.2.0 milestone. One coherent version
  across all 36 workspace members (`--version` / `-V` on every
  shipped binary — compositor, eight tools, `ldp-bench`, `ldpc`,
  gateway, four examples — pinned by per-package CI tests), the
  release-coherence gate (`scripts/version_check.py`, wired into
  `verify.sh`: workspace == manifests == lock == debian changelog ==
  README == CHANGELOG == the showcase scene mark), the `0.2.0`
  Debian entry with the updated census, docs raised to the milestone,
  and the all-in-one bundle `lion-display-v0.2.0.zip`.
  1,672 tests.
* **Phase 30 — the desktop matrix:** every display size, every
  graphics card, and the machines with none. GPU-class capability
  probing (`ldp-gpu::classify`): the EGL+GLES probe reads
  `glGetString`'s vendor/renderer/version into `GlIdentity` and
  classifies the GPU — pure table-driven logic, exhaustively
  unit-pinned, the honest `Unknown` default counting as hardware
  (never a silent degrade). The llvmpipe doctrine: `auto` refuses a
  CPU-rasterizer GL context (llvmpipe, swrast, SwiftShader — the
  no-graphics-card machine) because the specialized software backend
  is the faster CPU path, reason in the report line; `--renderer gl`
  still forces GL over any context; the startup line names the
  device. The sized output doctrine (`--resolution WxH`): the
  bring-up and every hotplug migration prefer a mode of exactly the
  forced size; a size nothing offers is the typed failure listing
  every size the panel offers (never a silent nearest-neighbor
  guess); a migration to a display without the size falls back to the
  unsized doctrine and the world keeps the preference. The desktop
  matrix benchmark rows (permanent `ldp-bench` rows): the Liquid
  desktop frame — wallpaper, four rounded-and-shadowed windows, one
  frosted dock bar — at 1920×1080 / 2560×1440 / 3440×1440 / 3840×2160,
  High tier and the no-GPU doctrine's tier, steady state and first
  frame. Measured (2 vCPU, 5-run medians): High steady 7.31 / 11.49 /
  15.11 / 24.67 ms across the four sizes (60 Hz holds at High through
  the 21:9 ultrawide), the doctrine's tier 5.96 / 7.95 / 10.96 /
  18.99 ms; the first 4K run caught the shadow memo thrashing its
  fixed 6 Mi-word budget (194 ms) — the budget now scales with the
  output (floored at the phone era's, so Phase 29 behavior is
  byte-exact) and 4K High dropped to 24.67 ms (7.9×), pinned by the
  flat-rebuild-counter oracle; the full bring-up's peak RSS 19 MiB.
  `docs/comparison.md`: twenty fields scored /100 against Wayland,
  X11, macOS WindowServer, and Windows DWM — every gap named with its
  roadmap line. 1,839 tests.
* **Phase 29 — the steady-state pass:** the low-end doctrine made
  real — the full Liquid scene (wallpaper, four rounded-and-shadowed
  cards, a frosted panel, full damage) at **8.70 ms per frame** on
  the software path (was 178.6 ms; 20.5×, byte-identical output,
  64.8 ms first frame). Shadows are memoized as imagery (built once
  per unique style, LRU-bounded), the frost is memoized on the
  backdrop's own words (rebuilt only when what sits beneath actually
  changed), the corner SDF runs only inside the arc bands (pixel
  centers are exactly half a pixel from every straight edge — 99.6%
  of a window's pixels never pay the `sqrt`), the box blur walks
  contiguous strips with an exact reciprocal mean, snapshots and
  material draws are word-level, and the copy paths cover the
  same-format family. Every equivalence is pinned by new oracle
  suites (the Phase 27 kernels restated as references, the
  steady-state transparency oracle). 1,816 tests.
* **Phase 28 — the positioning shell:** where the system puts
  windows, and the dock it keeps for itself. `ldp-shell::layout` —
  the pure placement engine: the output's shape picks the doctrine
  (portrait → phone: every app anchors at the usable origin above
  the dock, overflow runs *under* it — occluded, never destroyed;
  landscape → desktop: the classic cascade, caught at the usable
  edge; dialogs center), the dock's reservation carves the usable
  area, and every rule is arithmetic (no clocks, no scene types).
  Placement rides a root's *first attach* — the position applies
  with the very commit that maps the surface, so a window never
  lands at the origin and jumps — and live re-arrangements re-place
  every mapped root immediately (`set_position_now`, the R2 damage
  rule reading live positions). The system dock: the phone's home
  bar — a frosted bar the compositor itself owns (a haze plus a row
  of app pills, ARGB ink at a stable identity), rendered above every
  client layer with Phase 27's material machinery (zero growth of
  the render seam), reserving its thickness out of the usable area,
  rising from below the edge on a critically damped spring integrated
  at render cadence — bit-reproducible motion. `--dock auto|off`
  (CLI default auto; the library default stays off so every prior
  pixel oracle keeps byte-exact) and `--dock-thickness`; the honest
  third startup line ("phone layout, dock 84 px at the bottom").
  1,804 tests.
* **Phase 27 — the Liquid visual engine:** the macOS-class material
  language, on Linux, built for low-end devices. The system owns the
  materials: every served surface is dressed — antialiased rounded
  corners and a soft shadow for opaque windows, the frosted-glass
  backdrop (blur + desaturation + tint veil) behind translucent ones,
  the fullscreen wallpaper undressed. Four quality tiers
  (`--effects auto|high|medium|low|minimal`) resolve from the machine
  — a GL renderer serves the full 3-pass blur, a software renderer
  scales to the output's pixel budget, a tint veil keeps the identity
  where blur would stutter; the headless CI path stays plain. The
  spring engine adds the motion (critically damped, fixed substeps,
  bit-reproducible). All integer-deterministic, and styled frames are
  **byte-equal across the software and GL backends** — an automated
  gate. Also fixed: the GL stream's latent overlapping-damage
  double-composite. 1,769 tests.
* **Phase 26 — live output re-arrangement:** the serving compositor
  follows the display topology. A hotplug signal re-probes everything
  and the served pipeline moves: monitor swaps migrate mid-session
  (applied disable, objects released, a fresh scanout chain, the
  applied enable, the scheduler re-anchored at the new nominal, a
  full-scene re-render — the desktop survives with every client
  session intact); the last display going away is the honest dark
  state (pipeline disabled and released, buffer fences flushed, frame
  requests parked with `output_off`, the protocol still serving —
  commits commit, grabs answer `failed(no_output)`); the first
  re-plug relights and the waiting desktop paints again. Clients feel
  the transitions through their output objects: revoked
  (`capability_revoked`), re-bound, the fresh cascade. A mode
  renegotiation on the same connector migrates too — the pipeline
  comparison decides, not the status diff. 1,727 tests.
* **Phase 36 — the rootless door:** every X11 window becomes a
  first-class LDP window. The compositor serves the popup role end
  to end (`get_popup`'s anchor/gravity machine — mint, solve at
  attach, configure/ack, the live reposition move, dismiss, the
  parent-death sweep); `ldp-x11-bridge::rootless` exports every
  mapped root-child X window on its own surface (toplevel for the
  managed, popup anchored at the OR window's own screen rect for
  the override-redirect), the subtree composite painting each
  export's descendants in the export's own frame, per-window
  damage, one high-water pool; the X protocol's geometry stays the
  X clients' truth while the LDP display's placement is the
  screen's truth, input bridging them by construction; plus the
  quiescent keepalive (`connection.sync` per idle turn — the
  compositor parks cross-client events for the owner's next
  message) and three real bug fixes from the seal run's own hunt
  (the bridge's pointer `enter` coordinates — a latent v0.10.0
  arg-lift bug; the X face's spontaneous output parked until the
  foreign client's next request; the rootless pool's growth
  re-zeroing the mirror while only fresh windows re-committed).
  2,001 tests.
* **Phase 37 — one desktop, every arrangement:** the multi-monitor
  field's answer. `--outputs mirror` serves the clone doctrine —
  every display on the same desktop, the overlapping origins the
  protocol's own clone signal, each display cropping the desktop to
  its own mode (the per-pixel crop proof: a 720p display's every row
  equals the primary's first 1280 columns; two same-size displays'
  scanouts byte-equal word-for-word), a display joining mid-session
  joining the mirror, the dock rendering on every mirrored display,
  and the release gates spanning the mirror. `--scale F1,F2,…`
  serves one factor per output (the stretch rule: a hotplug newcomer
  inherits the doctrine's last entry). And the named-quirk table
  (`ldp-display::quirks`) ships the mechanism — every row carrying
  its symptom, its detection, and its operator escape; two real bug
  fixes from the seal run's own hunt (the renderers' origin-keyed
  canvas parking — the 720p display restored the 1080p canvas it
  had just parked, truncated into the wrong shape, caught by the
  crop proof's per-pixel equality; the hotplug newcomer resetting
  its scale to identity, losing the operator's `--scale`
  mid-session). 2,013 tests.
* **Phase 41 — the quirk ledger:** the VRR field's answer. The
  honest floor: `--vrr-floor N` (per-output CSV, the `--scale`
  grammar's mirror) clamps the advertised window into every consumer
  — the scheduler's widened deadline, the `output.vrr` event, the
  LFC cadence — while the device's own claim stands untouched (the
  override lives above the driver, where the giants' tables live);
  an unservable floor refuses boot naming the mistake. The LFC
  cadence: content slower than the window bridges on repeats locked
  to the content's own phase (`k = ceil(P/max)`, judder-free by
  construction — the giants' low-framerate-compensation
  arithmetic); the anti-flap latch holds the repeat regime through
  boundary-hugging flips. The sibling escape: `--vrr-uniform`
  collapses a mixed desktop to one fixed clock for the platforms
  whose cross-CRTC coupling flickers the fixed sibling (per-output
  VRR stays the default — the modern per-display doctrine). Six
  session proofs over the real socket (the clamp, the no-op, the
  passthrough, the refusal, the collapse with its contrast, the
  composition); the quirk table at five rows.
  2,084 tests.
* **Phase 40 — the deep material:** the visual-quality field's
  answer. The vibrant domain: `BackdropParams::saturation` extends
  to `0..=510` — below 255 the Phase 27 desaturation stands
  byte-identically, above it each channel extends *away* from its
  luma by its own distance scaled (integer-exact, rounded half away
  from zero) — the distance Windows' acrylic and macOS's vibrant
  materials keep over a plain blur; the named materials
  `vibrant_light` (the menu glass) and `vibrant_dark` (the
  control-center glass). The edge light: `LayerStyle::edge_light` —
  the luminous 1-px hairline traced inside the rounded silhouette
  (the coverage difference against the silhouette's own 1-px
  erosion, so the ring follows the corners on the same antialiased
  curve the ink clips with), drawn *after the ink* in both
  backends — software through the shared damage-clipped
  `apply_material`, GL as the stream's after-ink textured quad of
  the identical memoized words (byte-equal by construction), the
  ring served from `EdgeMemo` like the shadow's imagery. The
  material family: `Material` — Panel, Sheet, Menu, VibrantDark,
  Chrome — one named vocabulary the desktop speaks (a popup wears
  the menu glass, the dock wears chrome, the legacy pair verbatim,
  Minimal plain). The session proof: the menu glass end to end over
  the real wire — the interior is the vibrant frost hand-computed,
  the straight edges carry the hairline, the wallpaper untouched.
  2,063 tests.
* **Phase 39 — the fresh frame:** the input-latency field's answer.
  The position-state class (§10.4's table grows the row): the
  pointer's absolute `motion` and its `frame` terminator coalesce to
  the freshest value in the oldest pending slot (a discrete event is
  never pre-empted by a newer sample), `relative_motion` stays plain
  input (every delta delivers — full fidelity for raw-input
  consumers), and the discrete barrier (`CoalescingQueue::seal`)
  freezes the sample a click rode with — the served outbox becomes
  the class-aware emission path the doctrine described since Phase 7.
  The VRR-aware presentation clock: `FrameClock::arm_vrr` (the
  measured band is the panel's own window — the fast end the legacy
  band rejected as a duplicate) and the adaptive opportunity target
  (an adaptive surface's registration targets the panel's next
  refresh opportunity, not a nominal cell behind its own flip — the
  verdict at the honest landing, the deadlines pacing at the fast
  end while the client keeps up). The session proofs: the flood
  collapses sixteen batches to one motion with every delta, the
  click rides between its own sample and the fresher one, and every
  `presented` under `--vrr` carries the panel's actual 6.944 ms
  cadence while the default keeps the fixed grid. 2,050 tests.
* **Phase 38 — the honest peak:** the HDR field's answer. The panel
  negotiation (`ldp-hdr::negotiation`): the *effective* peak (the
  advertisement clamped by `--hdr-peak`), the negotiated canvas
  ceiling (the stack's brightest content clamped to the panel —
  dimmer content negotiates to itself, and the fold finally records
  what a v0.10.1-era comment only promised), and the per-surface
  mastering refinement (the declared HDR10 bounds meeting the
  ceiling). The luminance tail (`TonePolicy` in the renderer's
  color pipeline): every HDR layer's ink rides the BT.2390-
  structured knee from its declared mastering range onto the
  negotiated ceiling — the system's rolloff replaces the panel's
  hard clip, hue preserved, `Pass` keeping every pre-Phase-38 pixel
  oracle byte-identical, the GL v1 path refusing a tail-bearing
  layer typed (the honest boundary). `--hdr-peak N` is the
  bloated-peak quirk's escape — the advertisement, the canvas, and
  the ink all carrying the effective peak; the quirk table accrues
  its first row (`hdr-peak-bloat`). The session proofs pin the
  chain end to end: the same 10,000-nit code value renders as
  different honest ink on a 600-nit panel, a 400-nit panel, and
  under the operator's cap. 2,025 tests.
* **Phase 35 — the sleeping panel:** the machine that sleeps when
  nothing changes — panel self-refresh (the per-output machine:
  entry earned through consecutive quiet flip opportunities, exits
  named `damage`/`blank`/`backlight`/`unsupported`, the frozen
  timeline, the one-nominal rescan cost paid on every exit), the GPU
  clock governor (asymmetric hysteresis over the landed-flip load —
  up instant, down patient), the energy ledger (a documented
  parametric cost model and the CI-reproducible static-scene ratio
  the comparison table's power row carries), the serve-loop
  idle-ladder fix (the ladder never advanced on a truly idle real
  machine — no DRM events meant no ticks), `--no-psr` (the
  operator's escape for flickering panels), and the reports
  (`--selftest`'s power line, teardown's ledger verdict). 1,986
  tests.
* **Phase 34 — the hardware compositing path:** the display
  hardware's own planes join the render pipeline — `ldp-planes` (the
  26th crate) grades the layer stack for plane eligibility and solves
  the assignment (the split-point doctrine, zpos-monotone
  backtracking, a named demotion per composite layer); the import
  walk turns client buffers into kernel framebuffers
  (`drmPrimeFDToHandle` + `AddFB2WithModifiers` behind the
  `KmsBackend` seam); the frame loop takes one of three shapes —
  **zero-composite** (a fullscreen opaque client's own framebuffer
  scans out, the renderer emitting *no pass at all*), the underlay
  split (canvas on the primary, windows on overlays above), or the
  full composite — with retired overlays turned off every frame and
  the release gates riding the same flip; YUV and scaled GL uploads
  land byte-equal to the software path; dma-buf feedback tranches
  resolve the allocation advice; the display-model oracle keeps every
  pixel test meaningful across all three shapes
  (`docs/gpu-compositing.md`; `tests/planes_session.rs`). Alongside:
  Phase 33's evdev input path — the machine's real `/dev/input`
  devices through the seat router onto the wire. 1,944 tests.
* **Phase 25 — the real-KMS serve loop:** `lion-compositor --mode
  drm` serves for real — the `DisplayDriver` seam (one frame
  choreography over the mock and the real device), the mapped
  dumb-buffer delivery (MAP_DUMB + mmap, pitch-honoring writes, the
  equivalence suite byte-equal against the shadow oracle), the
  `serve_kms` poll loop (socket + DRM fd + signals), and honest
  teardown (disable commit, objects released, master dropped);
  `--probe` keeps the zero-risk TEST_ONLY rehearsal. 1,715 tests.
* **Phase 24 — hardware acceleration by default (the macOS
  doctrine):** GPU compositing through a real EGL+GLES 2.0 backend
  whenever the machine has a GL stack (`ldp-renderer::gles` — the
  `GlesApi` command seam, the `GlesRenderer`, the reference evaluator
  and recorder, the hardware-first selection policy; `ldp-gpu::gles` —
  the 48-entry dlopen'd FFI with per-command context migration),
  software fallback with the reason in the startup report, and
  `--renderer auto|gl|software` (default **auto**) on the compositor.
  The whole render pipeline — window-management redraws, animation
  frames, damage repaints, capture — flows through the selected
  backend, and the exit-criterion suite drives the **entire client
  session** through the GL seam demanding **byte-equality** with the
  software path. `--mode drm` grew a real-hardware scanout rehearsal:
  master takeover, dumb scanout buffers, framebuffer registration, and
  a TEST_ONLY atomic enable commit validated by the kernel — the
  always-on serve loop stays the next display milestone. 1,696 tests.


## Documentation

* [`docs/user-guide.md`](docs/user-guide.md) — running the compositor,
  the operator tools, the examples, and writing the first native
  client.
* [`docs/admin-guide.md`](docs/admin-guide.md) — installing the Debian
  packages, the systemd service, the security model for operations,
  troubleshooting.
* [`docs/maturity.md`](docs/maturity.md) — the honest per-subsystem
  scope matrix for v0.2.0 (what is CI-hardened, what is staged).
* [`docs/architecture.md`](docs/architecture.md) ·
  [`docs/protocol.md`](docs/protocol.md) ·
  [`docs/threat-model.md`](docs/threat-model.md) ·
  [`docs/roadmap.md`](docs/roadmap.md) ·
  [`docs/benchmarks.md`](docs/benchmarks.md) ·
  [`docs/comparison.md`](docs/comparison.md) — the honest field-by-field
  comparison against Wayland, X11, macOS WindowServer, and Windows DWM ·
  [`docs/comparison-windows-macos.md`](docs/comparison-windows-macos.md) —
  the architecture comparison at pipeline depth: LDP vs. the DWM/WDDM
  stack and the WindowServer/CoreAnimation stack, stage by stage
* API docs: `cargo doc --no-deps --open` (the verify gate builds them
  with warnings denied).

## Installation

From source: `./scripts/build-deb.sh` builds the Debian packages
(`dist/lion-compositor_0.8.0_amd64.deb`,
`dist/ldp-tools_0.8.0_amd64.deb`)
and proves they install cleanly in a fresh root — then
`dpkg -i dist/*.deb`. See [`packaging/README.md`](packaging/README.md).

## Building

Requires Rust 1.75+ (built and tested with 1.98).

```sh
cargo build          # build all workspace crates
cargo test           # run unit + integration tests (incl. the drift gate)
cargo clippy --all-targets -- -D warnings
cargo doc --no-deps --open
./scripts/verify.sh  # fmt + clippy + test + doc + spec lint + drift + version coherence, the CI gate
```

Protocol changes follow one path: edit `spec/*.toml`, then regenerate

```sh
cargo run -p ldpc -- gen   # updates crates/ldp-protocol/src/generated/ + docs/reference/
```

Generated code is committed; `cargo test` recompiles the spec and fails on
any drift between the committed tables and `spec/`.

`ldp-core` and `ldp-protocol` have **zero runtime dependencies**;
`ldp-transport` adds exactly one: `libc`, the canonical syscall-ABI
binding (hand-rolled `msghdr`/`cmsg` structs would mean *more* reviewed
`unsafe`, not less — all of it sits in the audited `sys` module).
`ldp-server` and `ldp-client` add no new runtime dependencies at all —
std plus the LDP layer crates below them, `#![forbid(unsafe_code)]`.
The client deliberately avoids even `rand`: reconnect jitter is a
seeded LCG, backoff is `std::thread::sleep` — the library every
application links stays on the LDP layer crates only.
`ldp-compositor` likewise adds none: pure scene/damage/scheduling
logic over `ldp-core`'s geometry, region algebra, and timing types —
no wire code, no I/O, and (decisively for the replay harness) no
clock reads. `ldp-renderer` adds none either: sampling, blending and
color math over `ldp-core` — it never reads a clock, which is what
keeps the golden frames byte-stable.

The tooling dependency (`toml`/`serde` in `ldpc`) never reaches a runtime
crate. Later crates keep the dependency tree minimal by policy (see
`CONTRIBUTING.md`); native libraries (libEGL, libdrm, libxkbcommon) are
loaded at runtime via `dlopen` so the core builds and runs on headless
CI without GPU drivers.

## Licensing

Dual-licensed under the MIT license and the Apache License 2.0
(`LICENSE-MIT`, `LICENSE-APACHE`). You may use LDP under either license.
Contributions are dual-licensed under the same terms unless stated otherwise.
