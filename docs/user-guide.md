# LDP User Guide

Everything an LDP **client developer and example user** needs: building
the stack, running the reference compositor, pointing the operator
tools and example applications at it, and writing the first native
client. For installation via Debian packages, service management, and
the security model, read the companion
[`docs/admin-guide.md`](admin-guide.md) first. For what is and is not
production-hardened at v0.2.0, see [`docs/maturity.md`](maturity.md).

---

## 1. What you are holding

LDP — the Lion Display Protocol — is a display and input stack written
in safe Rust: a wire protocol with generational object identity, a
deadline-scheduled compositor core, capability-gated security, and
first-class color. The v0.5.0 milestone ships the 26-phase build
defined by [`docs/roadmap.md`](roadmap.md): 9 protocol modules, 34
interfaces, and 213 operations, served by the reference compositor on
the deterministic mock display **and on real DRM hardware**
(`--mode drm`: mapped dumb scanout, applied atomic modeset, page
flips, clean teardown — see §3.6), **compositing on the GPU whenever
the machine has a GL stack** (hardware by default, honest software
fallback; see §3.5), with the full operator toolchain and 1,715
passing tests behind it.

Two facts shape every usage pattern below. First, the reference
compositor at v0.2.0 is **headless-first**: it serves the complete
protocol surface on an in-memory mock display (1920x1080@60 by
default) whose clock only advances at protocol wake points, which
makes every served event stream deterministic and replayable. Second,
every client-side program in this repository — the eight operator
tools, `ldp-bench`, `ldpc`, the four examples, and your own
`ldp-client` programs — finds the server
through **one convention**: an abstract-namespace Unix socket name,
given by `--socket NAME` or the `LDP_SOCKET` environment variable.

## 2. Building from source

You need a Rust toolchain of 1.75 or newer (the project is developed
and gated on stable; `rustup` user-local installs are fine — no root
required). From the repository root:

```console
$ ./scripts/verify.sh
```

runs the full gate — `cargo fmt --check`, `cargo clippy --all-targets
--all-features -- -D warnings`, the complete test suite, `cargo doc`
with warnings denied, the spec linter, and the protocol-compiler drift
check. It is the same gate CI runs; if it is green locally, the tree
is releasable. For the optimized binaries that the Debian packaging
and the benchmark report use:

```console
$ cargo build --release -p lion-compositor -p ldp-tools -p ldp-test -p ldpc \
      -p hello-ldp -p pointer-paint -p clip-client
```

The release profile is `opt-level=3`, thin LTO, one codegen unit, and
`panic=abort` — a display server never unwinds across its unsafe
syscall seams. Binaries land in `target/release/`.

## 3. Running the compositor

```console
$ lion-compositor [OPTIONS]
  --mode <auto|headless|drm>  how to drive the display (default: auto)
  --socket <NAME>             abstract-namespace socket name
  --dump <DIR>                write frame_NNNN.ppm per rendered frame
  --selftest                  bring the pipeline up, then exit
  --quiet                     suppress informational output
  --help | --version
```

The three modes matter:

* **headless** serves the full protocol on the mock KMS device. This
  is the CI vehicle and the default workbench for client development.
* **drm** is the honest probe: it opens real DRM nodes and prints the
  discovered topology (connectors, CRTCs, planes). It does not yet
  drive pixels on real hardware — v0.2.0's real-machine scope is
  topology plus the EGL/DMA-BUF plumbing in the `ldp-display` and
  `ldp-gpu` crates; see the maturity matrix.
* **auto** probes DRM and falls back to headless when no device opens.
  On a desktop with a DRM stack it prints the topology and exits 0
  (probe-only scope); in a container or VM it serves headless.

Without `--socket`, the compositor listens on the abstract name
`lion-compositor-<pid>`. The name is printed on startup in
`@name` display form. `--selftest` performs the full headless
bring-up and tears it down — startup validation without serving, exit
code 0 on success. Usage errors exit 2.

`--dump DIR` writes every composited frame as `frame_NNNN.ppm` — the
same pixel stream the golden-image suites assert against, which makes
it the fastest way to see what the server actually rendered.

### 3.5 The renderer: hardware by default

The compositor's render pipeline — every window-management redraw,
every animation frame, every damage repaint, every capture — runs
through one renderer selection, made once at startup:

```console
$ lion-compositor --mode headless
renderer: gles (hardware: EGL + GLES 2.0 context up)     # a GL machine
renderer: software (GL unavailable: cannot load libEGL.so.1)  # no GL stack
```

`--renderer auto` (the default) composites on the GPU through the
EGL+GLES 2.0 backend whenever the machine has a GL stack and falls
back to the software reference otherwise, carrying the reason in the
report line — never a silent surprise. `--renderer gl` forces the GPU
path: a machine without GL fails startup with the typed reason
(operators who pin it want the failure, not a quiet CPU path).
`--renderer software` pins the reference backend. The equivalence
suites prove the two paths byte-identical through the whole client
session, so switching between them changes *where* pixels are
computed, never *which* pixels land.

## 3.6 DRM mode: serving the real display

On a machine with a DRM device, `--mode drm` is no longer a probe —
it is the real serve loop (Phase 25):

```console
$ lion-compositor --mode drm
scanout: 1920x1080 XRGB8888, pitch 7680, dual dumb buffers mapped
modeset: pipeline enabled (applied atomic commit)
renderer: gles (hardware: EGL + GLES 2.0 context up)
scanout serving: bring-up flip landed (Ctrl-C to exit)
lion-compositor (drm): listening on @lion-compositor-3124 — Ctrl-C to exit
```

Bring-up takes DRM-Master, allocates two kernel dumb buffers at the
chosen mode, registers them as framebuffers, maps both for CPU
composition (the pitch-honoring delivery the equivalence suites prove
byte-equal to the headless oracle), applies the atomic enable commit
for real, and latches the first flip as the scheduler's anchor. The
serve loop then polls two descriptors — the abstract-namespace socket
(your clients: the tools, the examples, `ldp-bench`, anything speaking
LDP) and the DRM fd (page-flip landings, topology events) — until
Ctrl-C or SIGTERM, which unwind through honest teardown: the display
left dark (disable commit), framebuffers released, dumb buffers
destroyed, master dropped.

`--probe` keeps the Phase 24 behavior — the whole bring-up validated
under `TEST_ONLY`, nothing applied, the screen never blinks — for
operators who want the zero-risk pre-flight. And `--mode drm` on a
machine *without* DRM nodes fails typed and loud (`no DRM device could
be opened`, exit 1); it never silently serves headless — use
`--mode auto` (the default) for that fallback.

The honest boundaries at v0.5.0: VRR is advertised on the mock
path but not yet detected on real nodes; the compositor takes over
whatever was on the screen when it exits (the disable commit leaves
the display dark — a session manager's respawn is the integration
point).

## 3.7 Hotplug: the display follows the topology

While the serve loop runs, the display topology is allowed to move —
cables pull, docks land, sinks re-negotiate their mode lists. The
compositor answers every hotplug signal with a full re-probe and one
of four transitions (Phase 26):

* **A monitor swap** — the served connector dies, another lives. The
  pipeline migrates mid-session: the operator sees

  ```console
  hotplug: migrated eDP-1 (1920x1080) -> HDMI-A-1 (1920x1080)
  ```

  and the desktop survives — the same clients, the same surfaces,
  presenting on the new display. Clients that bound the output object
  receive `connection.revoked` (`capability_revoked`) and re-bind for
  the fresh cascade (geometry, modes, identity) — that is the
  documented signal to re-layout.
* **The last display goes** —

  ```console
  hotplug: eDP-1 gone — dark (protocol serving; waiting for a display)
  ```

  the compositor goes *dark*: the pipeline is disabled and every
  kernel object released, but the protocol keeps serving. Commits
  still commit (the desktop waits for light); frame requests park
  (`frame_dropped` with `output_off` for the ones in flight, deferral
  for the new ones — no busy-looping); grabs answer
  `failed(no_output)`; buffers release their fences immediately
  (nothing is being read anymore).
* **The first display returns** —

  ```console
  hotplug: DP-1 lit — the desktop paints again
  ```

  the bring-up replays, deferred frame requests are answered on the
  new timeline, and the waiting desktop paints — without a single
  session lost across the whole cycle.
* **Topology noise** — a connector appearing while the served one
  lives changes nothing (`hotplug: topology changed; the served
  pipeline holds`): stability over preference.

A sink re-negotiating its mode list (same connector, different
modes) is a migration too — the pipeline comparison decides, not the
connector status. The one staged boundary: driving *several* outputs
at once (the multi-CRTC layout) is the next display milestone, and
the dynamic-global story (removing and re-advertising the output
global through the registry) is broker-era work — today the global
stays advertised and object revocation is the live-migration signal.

## 3.8 The visual language: `--effects`

Since Phase 27 the compositor dresses every surface it serves — the
*system* owns the materials (the macOS rule): rounded corners, a soft
shadow for opaque windows, and the frosted-glass backdrop behind
translucent ones. Clients draw content; the look arrives from the
compositor. Nothing in the protocol carries the styling, so every
existing client gains the language without changing a line.

The honest startup line names what you are getting:

```console
$ lion-compositor --mode drm
renderer: gles (hardware: EGL 1.4, GLES 2.0)
effects: high (3-pass blur frost, panel shadows, corners)
```

The tiers, and what `--effects auto` picks from the machine:

| Tier | Frost | Shadow | Corners | `auto` serves it when |
|---|---|---|---|---|
| `high` | 3-pass blur, desaturate, tint veil | soft, offset | 18 px | a GL renderer is active |
| `medium` | 2-pass blur | soft | 16 px | software renderer, output ≤ 2.5 MP |
| `low` | tint veil, no blur | hard 1-pass | 12 px | software renderer, larger outputs |
| `minimal` | none | none | none | the headless mock path (CI pixels) |

The doctrine behind the ladder is the low-end device: a phone GPU
serves the full language; a phone CPU scales the blur to the output's
pixel budget; the weakest hardware keeps the *identity* (the veil,
the corners, the layout) where blur would stutter. `--effects
high|medium|low|minimal` pins a tier explicitly — the startup line
always tells the truth about what is being served.

Determinism: the whole engine is integer math with named rounding
rules (the coverage kernel is the one float path, IEEE-exact
operations only), and styled frames are **byte-equal across the
software and GL backends** — an automated gate, not a hope. The
`pixels_effect` statistic in the render stats counts the effect work
per frame. Motion is part of the language too: the spring engine
(`ldp-compositor::spring`) gives clients the critically-damped macOS
feel — the showcase example's glass panel rides it through the normal
presentation loop.

The staged boundary: per-surface material *requests* (a client asking
for a specific material) arrive with the shell protocol's service
surface — the policy itself speaks the **material family** since
v0.10.7: one named vocabulary the whole desktop draws from.

### The material family (v0.10.7 — the deep material)

The glass stopped being one flavor. Three things joined the
language, all tiered exactly like the blur above and all
**byte-equal across the software and GL backends**:

* **The vibrant domain.** The frost's saturation dial used to only
  *desaturate* the backdrop towards its luma; it now extends *past*
  the backdrop's own chroma (`0..=510`, integer-exact — the
  distance Windows' acrylic and macOS's vibrant materials keep over
  a plain blur). The menu glass runs at 383 (the 1.5× read): a
  colorful wallpaper glows through a menu instead of graying out
  under it.
* **The edge light.** Every material can trace the luminous 1-px
  hairline inside its rounded silhouette — the light catch that
  separates glass from its backdrop. It is drawn *over* the
  surface's own ink, as memoized imagery (computed once per shape
  and reused while the window sits still, exactly like the
  shadows).
* **The family.** The system resolves surfaces through named
  materials: **panel** (an opaque window — corners and shadow),
  **sheet** (a translucent surface — the frosted glass), **menu**
  (a popup or menu — the vibrant frost plus the hairline: backdrop
  blur *everywhere* the giants mean it), **vibrant-dark** (the
  control-center glass), **chrome** (the dock — frost, no shadow,
  the gentler hairline). `--effects minimal` keeps every material
  plain: CI pixels and the library default stay byte-frozen.

For a client, nothing changes — the materials remain the system's
(the macOS rule), and the family is the compositor's own policy: a
`get_popup` surface is dressed in the menu glass automatically, the
dock traces its chrome edge at every tier above `minimal`.

## 3.9 The positioning shell: `--dock`

Since Phase 28 the system decides where windows sit, and keeps a
dock for itself at the screen edge — the *phone's home-screen
layer*. The output's shape picks the doctrine: a portrait output is
a **phone** (every app anchors at the usable origin, one stack above
the dock; an app taller than the usable area runs *under* the dock —
the overflow is occluded, never destroyed, the iOS keyboard
doctrine), a landscape output is a **desktop** (windows cascade
diagonally, each new one a step in, until the usable edge catches
the stack — a window never leaves the screen). Placement happens at
a surface's first attach: the position applies with the very commit
that maps the window, so an app *lands placed* — it never appears at
the origin and jumps. A monitor swap mid-session re-places every
mapped window onto the new screen's usable area in the migration
frame itself.

The **system dock** is the phone's home bar: a frosted bar the
compositor itself owns (not a client — the system draws it, the
system frosts it with the `--effects` tier's material language),
spanning the bottom edge, reserving its thickness out of the usable
area, with a row of app pills. It rides above every window — the
wallpaper runs *under* it — and its intro rises from below the edge
on a critically damped spring that settles within a quarter second
of the desktop's first frames.

The honest startup line names the resolved doctrine:

```console
$ lion-compositor --mode drm
renderer: gles (hardware: EGL 1.4, GLES 2.0)
effects: high (3-pass blur frost, panel shadows, corners)
shell: desktop layout, dock 84 px at the bottom
```

`--dock auto` (the CLI default) serves the shell; `--dock off`
reverts to the legacy behavior (windows keep their creation
positions, no chrome draws — the pixel-exact Phase 26/27 look, which
is also the *library* default so equivalence corpora keep their
oracles). `--dock-thickness <px>` tunes the reservation (the phone
home bar is 84). A portrait monitor gets the phone doctrine — the
classifier is shape, not a device probe — which is the correct
behavior for the portrait-first phone this serves.

Determinism: placement and the dock are arithmetic; the rise is the
spring engine integrated at render cadence from the display's own
timestamps, so the same frame sequence reproduces the same intro
bit-for-bit. The staged boundary: the shell *protocol* surface
(toplevel configure cycles, popup constraint solving against the
live usable area) is the next phase's work — today the compositor
places plain core surfaces, and the `ldp-shell` crate's machines
(the toplevel/popup/dialog vocabulary) are the library the service
will drive.

## 3.10 The desktop machine: GPU classes, `--resolution`, and no GPU at all

Since Phase 30 the compositor *meets the machine it boots on*. Three
operator-facing consequences:

**The renderer names the GPU.** The GL probe reads the context's
identity (`glGetString` — the vendor, the renderer, the version) and
classifies it: a discrete card (Radeon RX, GeForce, Arc), an
integrated GPU (Intel UHD/Iris, an APU's Radeon graphics), a virtual
GPU (virgl in a VM), a **software rasterizer** (llvmpipe and friends
— the machine has no graphics card, or its driver failed and Mesa
fell back), or `unknown` — which counts as hardware, because an
unrecognized GPU is a GPU, never a reason to silently degrade:

```console
$ lion-compositor --mode drm
renderer: gles (hardware: EGL + GLES 2.0 context up, discrete GPU: AMD Radeon RX 7900 XTX)
```

**The no-GPU doctrine.** On a machine whose only "GL" is a CPU
rasterizer, `auto` takes the *specialized* software backend instead —
it blends words directly, with no GL state machine, shader compiler,
or texture pipeline in the way, so it beats CPU-emulated GL on the
same silicon. The choice is honest and printed:

```console
$ lion-compositor --mode drm
renderer: software (GL is a CPU rasterizer: software rasterizer (no GPU): llvmpipe (LLVM 17.0.6, 256 bits); the specialized software backend is faster)
```

`--renderer gl` overrides the judgment (the operator is the final
authority, always); `--renderer software` pins the specialized
backend on any machine. The `--effects` tier then scales itself to
the output's pixel budget (a software renderer on a phone-sized
output serves Medium; on a desktop-sized output, Low — the blur
would not hold 60 Hz at 4K on a weak CPU), and the desktop matrix
benchmarks (`docs/benchmarks.md`) pin what each size actually costs:
**60 Hz holds at the High tier through the 21:9 ultrawide (7.31 /
11.49 / 15.11 ms at 1080p / 1440p / ultrawide), and under the
no-GPU doctrine's tier at every size (5.96 / 7.95 / 10.96 / 18.99 ms
up to and including 4K's full-damage worst case)** — measured on a
2-vCPU rig, so a real desktop CPU doubles the headroom, and the GL
backend serves 4K High outright.

**The sized output: `--resolution WxH`.** Force the output's mode to
an exact size — the desktop operator's tool. The compositor picks
the best mode of exactly that size the panel offers (the preferred
flag first, then the higher refresh), reports it on the scanout line,
and a size nothing offers fails startup *honestly*, listing every
size the panel does offer:

```console
$ lion-compositor --mode drm --resolution 2560x1440
scanout: 2560x1440 XRGB8888 (forced via --resolution 2560x1440), pitch 10240, dual dumb buffers mapped

$ lion-compositor --mode drm --resolution 1280x720
lion-compositor: DRM bring-up failed: resolution no 1280x720 mode on any connected connector (offered: 3840x2160, 1920x1080)
```

The preference rides every hotplug migration: a monitor swap prefers
the next display *at your size* when it offers one, falls back to
that display's preferred mode when it does not (the migration never
dies on your preference), and remembers the size for the display
after that.

**The poured output: `--synth WxH[@HZ][:family]` (v0.10.9; the
family grammar v0.14.0).** The mode
foundry: when the panel's own list does not carry the size you want
— or carries it only at refreshes you do not — the compositor
*pours* it, synthesizing the VESA timing the same
way `xrandr --newmode` and Windows' custom-resolution form do. The
refresh defaults to 60 (`--synth 1920x1080@144` serves 144 Hz on a
panel whose list offers only 60); the family names the standard —
`:rb` (the default, CVT reduced blanking), `:rb2` (the
deep-color-era 80-pixel blank, with 1-pixel width precision),
`:rb2v` (the video-optimized 59.94 Hz class), `:cvt` and `:gtf`
(the analog-era standards); the panel's own EDID-declared
clock ceiling gates the pour (an ask above it refuses startup
*naming the declaration* — never a silent clamp), and every client
that binds the output sees the poured mode in its advertised list,
flagged current. The pour rides hotplug migrations the same way
`--resolution` does. The two flags are mutually exclusive — the
offered list versus the pour, two different asks.

```console
$ lion-compositor --mode drm --synth 2560x1440
scanout: 2560x1440 XRGB8888 (pitch 10240), dual dumb buffers mapped
modes: eDP-1 pours 2560x1440 @ 60.0 Hz (CVT-RB 241700 kHz, userdef)

$ lion-compositor --mode drm --synth 7680x4320
lion-compositor: headless bring-up failed: synth: the 7680x4320 pour clocks 2089988 kHz over the panel's declared 600000 kHz ceiling
```

The honest comparison of all of this against Wayland, X11, macOS's
WindowServer, and Windows's DWM — twenty fields, scored, every gap
named with its roadmap line — is
[`docs/comparison.md`](comparison.md).

### 3.11 The power doctrine: the sleeping panel

The most expensive thing a display server does is *nothing visible*:
the display engine scans an unchanged framebuffer out of memory tens
of thousands of times per second, forever, so the panel can show a
frame that never changes. Since v0.10.2 the compositor closes that
account, and it does so **by default** — nothing to enable, no flag
to remember:

* **Panel self-refresh (PSR)** — when an output's content is static
  (nothing to render, nothing owed, no flip in flight, for two
  consecutive quiet power turns — roughly half a second at the serve
  loop's poll cadence), the compositor writes the connector's
  `panel self refresh` property: the panel holds its own GRAM copy,
  the pixel clock quiets, the memory bus idles, and the device goes
  *silent* — no vblanks, no flips, nothing. Any damage wakes it at
  exactly one refresh interval of rescan cost (the kernel's own
  page-flip semantics — the exit is never free, whichever vocabulary
  releases it); the idle ladder's dim rung wakes it for the
  backlight retrain and the hysteresis re-earns the sleep once the
  ramp settles; the blank kills the scan path outright. A panel
  whose driver refuses the property is simply never asked again (a
  power hint degrades, never dies).
* **The GPU clock governor** — the render clock follows the *landed
  flips* (the honest load: a static desktop loads zero, a 60 Hz
  animation loads one): one busy interval above two-thirds load
  steps the clock up immediately (under-clocking a heavy frame is
  visible, and the compositor never spends frame pacing), three
  quiet intervals below one-third walk it back down patiently.
* **The idle ladder** (`--idle MS`, still opt-in) — dim at the
  timeout, blank at twice it, suspend vocabulary beyond; activity
  lights everything back. v0.10.2 also fixed the production bug
  where the ladder only ticked when DRM events arrived — and a
  *truly* idle machine has none, so the ladder that detects
  inactivity never advanced when the machine was actually inactive.
* **The reports** — `--selftest` prints `power: psr on (the panel
  sleeps when static)`; a full session's teardown prints the energy
  ledger's verdict: total millijoules over the documented cost
  model, the **static-scene ratio** (the sleeping desktop draws
  ~13% of the always-scanning counterfactual in the model), the
  self-refresh seconds, the exits with their names, and the wakes.

If your panel's self-refresh flickers (a real quirk in the eDP
ecosystem — the reason the giants carry per-vendor quirk tables),
`--no-psr` is the honest escape: the panel keeps scanning the static
frame on the untouched grid, and the ledger prices the
counterfactual so you can see exactly what the escape costs.

### 3.12 The rootless door: X11 applications as windows

Since v0.10.3 the bridge's X11 face is **rootless by default**: an
X11 application's windows are windows — every top-level X window
rides its own LDP surface, the positioning shell places them (the
cascade, the usable area), the Liquid materials dress them, and an
override-redirect window (a menu, a tooltip) rides the popup role's
anchor/gravity machine exactly as a native client would. The
whole-screen projection of v0.10.0 remains available as the
`--x11-rootful` escape.

```console
$ lion-bridge --socket ldp-0 --x11 /tmp/.X11-unix/X9       --token /etc/lion/bridge.token
$ DISPLAY=:9 xterm          # a window among windows
```

Two truths coexist, by doctrine. The **X protocol's geometry** is
the X clients' truth — ConfigureNotify answers from the X window
tree, and the bridge never moves an X window (it is not a window
manager; X clients that position their windows see their own
coordinates). The **LDP display's placement** is the screen's
truth — the shell cascades the toplevels like any native window. A
click bridges them honestly: the pointer event arrives
surface-local, and surface-local plus the X window's own origin is
the X root coordinate the client sees — you hit the pixel you see,
the client sees the click at its own geometry.

The X root window itself is not exported (the XQuartz-rootless
doctrine: root-window content like wallpapers does not show — the
LDP desktop's own background does). Windows *inside* a top-level
(buttons, panes) paint into their window's export — the subtree
composite — exactly as the X protocol paints them. The quiescent
keepalive (`connection.sync`, a few 32-byte frames a second on an
idle link) polls the compositor's parked input events out; the
compositor parks cross-client events for the owner's next message,
and a bridge that only reacts would never see its keyboard.

Native clients gained the same capability: **the popup role is
served**. `shell.get_popup` mints the anchor/gravity machine —
placement solved at the surface's first attach (the buffer's size
is the solver's input; the menu *lands placed*, never
origin-then-jump), `popup.configure` carrying the parent-relative
proposal, `ack_configure`/`reposition` (the live move)/`dismiss`
answering the client vocabulary, and the parent's destruction
dismissing every child — a menu cannot outlive its window. A null
parent anchors to the output itself (the bar-menu doctrine).

### 3.13 One desktop, every arrangement

Since v0.10.4 the multi-output doctrine serves every way a desk can
put its displays on one desktop. The extended arrangement is the
default: every display carries its own portion of one left-to-right
desktop. **`--outputs mirror`** serves the clone arrangement — every
display shows the *same* desktop:

```console
$ lion-compositor --outputs mirror
outputs: mirrored (one desktop, every display a clone)
```

The overlapping origins are the protocol's own clone signal: a
client that binds two outputs and sees them at the same position
knows they show the same content (the same vocabulary X11 and the
Wayland compositors serve for clone mode — a client does not have
to guess). The desktop is the primary's bounds; each display crops
it to its own mode — a smaller display shows the top-left crop (a
720p display next to a 1080p primary shows the primary's first 1280
columns of every row, per-pixel), a larger one letterboxes in the
desktop's own background. The dock renders on every mirrored
display (the dock is the desktop's own chrome). A display joining
mid-session joins the *mirror* — never an extension.

**Per-output scales** serve the mixed-density desk: `--scale 1,2`
names one factor per output in output order (the eDP at 1x, the
HDMI at 2x); extras reuse the last factor, and a display joining
mid-session inherits it too (the stretch rule — the operator's list
never runs out). The positioning shell resolves its logical canvas
on the *primary's* own factor, so a 1x primary keeps the desktop's
geometry while the 2x display's clients render at their own
density.

And the **quirk ledger** names the display ecosystem's long tail:
every tabled quirk carries its symptom (what you see), its
detection (how to confirm it is the quirk and not a bug), and its
operator escape (the CLI route around it). The five rows shipped
today: `psr-flicker` (visible flicker on static content while panel
self-refresh is engaged — the escape is `--no-psr`), `vrr-flicker`
(brightness pumping while an adaptive-sync panel stretches its
clock — the escape is serving without `--vrr`), `hdr-peak-bloat`
(HDR highlights flattening late because the panel's EDID advertises
more peak than its backlight sustains — the escape is `--hdr-peak N`
with the measured sustained peak), `vrr-floor-flicker` (brightness
pumping *only on slow content*, while the clock stretches toward the
advertised minimum rate — the escape is `--vrr-floor N` with the
rate the panel honestly sustains, per-output like `--scale`:
`--vrr-floor 57,0` floors the eDP and passes the HDMI through), and
`vrr-sibling-flicker` (a fixed display flickering while its VRR
sibling stretches, on the platforms whose scanout clocks couple
across the seam — the escape is `--vrr-uniform`, one fixed desktop).
The selftest prints the whole table:

```console
$ lion-compositor --selftest
quirks: 5 quirk(s) tabled: psr-flicker (panel) — escape: --no-psr; \
vrr-flicker (sync) — escape: serve without --vrr; hdr-peak-bloat \
(advertising) — escape: --hdr-peak N (the measured sustained peak); \
vrr-floor-flicker (advertising) — escape: --vrr-floor N (the honest \
minimum rate, Hz); vrr-sibling-flicker (sync) — escape: --vrr-uniform \
(one fixed desktop across the seam)
```

### 3.14 The honest peak: the HDR panel negotiation

Since v0.10.5 the HDR pipeline negotiates. The v0.10.0 doctrine
advertised the panel's truth and blended scene-linear onto the PQ
canvas; the negotiation closes the loop — the advertisement, the
canvas, and every HDR layer's ink all carry the *effective* peak,
the value the render pass will actually deliver:

```console
$ lion-compositor --hdr --hdr-peak 400
hdr: hdr pq (effective peak 400 nits — operator-capped — the \
negotiation ceiling)
```

**The negotiated ceiling.** Each frame's stack summarizes its
brightest declared content (the maximum CLL over the mapped HDR
surfaces' `set_hdr_metadata`); the canvas's ceiling is that content
clamped to the panel's effective peak — dimmer content negotiates
to itself (a 400-nit movie on a 600-nit panel rides at 400, wasting
no code space above it), brighter content maps down. The PQ canvas
carries the ceiling as its luminance maximum — the honest record of
what the frame will hold.

**The per-surface refinement.** A surface that declares its
mastering bounds (`set_hdr_metadata`) has them reach the ink: the
layer's code values ride the BT.2390-structured knee from the
declared mastering range onto the negotiated ceiling — the
system's soft rolloff replaces the panel's own hard clip (the
system owns the materials, light included). Content below the knee
passes untouched; hue is preserved by construction. A surface with
no declared metadata gets the honest default: a clip at the
negotiated ceiling, never exceeding the panel, nothing else
claimed.

**The operator's honesty knob.** Real panels advertise more peak
than they sustain (the bloated-EDID class — highlights flatten late
because the tone mapping aims at a ceiling the backlight never
shows). `--hdr-peak N` (nits, with `--hdr`) states the measured
sustained truth; the advertisement (`output.hdr_caps`), the canvas
ceiling, and every HDR layer's tone policy all carry the effective
peak. This is the quirk table's first accrued row:

```console
$ lion-compositor --selftest
quirks: 3 quirk(s) tabled: psr-flicker (panel) — escape: --no-psr; \
vrr-flicker (sync) — escape: serve without --vrr; hdr-peak-bloat \
(advertising) — escape: --hdr-peak N (the measured sustained peak)
```

What a client should take from `hdr_caps` is exactly what the
compositor commits to: the advertised peak is the negotiated
ceiling's clamp — what you can rely on is what the render pass
will deliver.

### 3.15 The fresh frame: input latency and honest pacing

Since v0.10.6 the emission path serves the latency tuning the
mature display servers spent their decades on — as doctrine, not
as accident.

**Position-state coalescing.** The pointer's absolute `motion`
sample and its batch's `frame` terminator are *position state*:
when a newer sample parks before an older one has been delivered,
the newer value replaces the older *in its slot* — the delivery
order never moves. A 1000 Hz mouse moving between two of your
frames produces sixteen samples; your next wake reads ONE
`pointer.motion` carrying the freshest coordinates (whatever the
device's rate, you see the position your frame will render with),
ONE `pointer.frame`, and every `pointer.relative_motion` delta in
full — a raw-input consumer (a game's own camera filter) loses
nothing, while a position consumer parses one event instead of
forty-eight. Discrete events are never coalesced and never
reordered: a click, an enter, a leave each *seal* the position
state they rode with — the click's context is the position it
happened at, not a fresher one that moved on.

**The VRR-aware presentation clock.** Under `--vrr`, a surface
that opts into the window (`set_presentation_mode: adaptive`)
gets honest pacing: its registration targets the panel's next
refresh *opportunity* (the earliest in-window landing) instead of
a nominal grid cell, so the `presented` verdict fires at the flip
that actually carries its content — `presented_at` is the honest
landing instant and the event's `refresh` is the cadence the panel
actually ran (on the development mock's 48–144 Hz window, a
ready-at-every-flip client presents at the 6.944 ms fast end and
the feedback says so, where the pre-v0.10.6 clock reported the
16.667 ms nominal grid the panel was not running). Deadlines pace
at the fast end while you keep up: commit at readiness and the
frame pacing follows your content — which is the point of
adaptive sync. Without `--vrr` (or without the adaptive opt-in)
everything keeps the fixed nominal grid — the byte-exact doctrine
of the pre-VRR era. Since v0.10.8 the window you are told is the
window the system schedules inside: an operator's honest floor
(`--vrr-floor N`) clamps the `output.vrr` advertisement to the rate
the panel sustains, and the LFC machinery bridges content slower
than the floor on repeats locked to the content's own phase (no
judder, no boundary flapping) — pace against the event's bounds and
you are pacing against the truth.

**The budget that holds.** The input→photon rig (input arrival to
landed flip, `last_input_photon_ns`) is asserted in CI under the
flood: the freshest sample rides the next frame within one nominal
period plus submission costs — the coalesced mailbox changes the
delivery, never the render.

## 3.11 The window choreography: `--transitions`

Since v0.15.0 the compositor owns the window-open motion: a freshly
mapped toplevel **fades in on a spring** — the macOS motion grammar
(critically damped, ~300 ms to settle), integrated at the frame clock
so the curve is reproducible, never a timer loop. The motion is
server-driven: the frame-callback economy advances it while the
client draws, and the serve loop's poll cadence wakes it when every
client sleeps — the desktop's own choreography never depends on the
applications being well designed. Two server-side invariants hold:
**popups and ephemeral roles appear instantly** (a menu that fades in
has already failed its user — claim `toplevel.set_semantic_role` as
`tooltip` or `overlay` for the same instant behavior on a regular
window), and **the settled frame is byte-identical with the
never-animated one** (the fade ends at exactly full opacity — the
screenshot you take after the settle equals the one you would have
taken with the flag off).

Since v0.16.0 the compositor owns the **goodbye** too: a window
leaving the desktop — destroyed, or hidden by a detach commit —
**fades out over an owned snapshot of its last raster** (the client
is already gone; the fade rides the compositor's own copy, drawn at
the window's own stacking slot so a window closing *under* another
never draws above it). The settled desktop is the plain post-destroy
bytes — the same A/B byte-equality the open fade pins. The eligibility
rules mirror the open's (popups and tooltips dismiss instantly), and
the flag is the same one: `--transitions`.

The default is **off** — plain pixels from the first frame (the
byte-exactness doctrine; the CI equivalence corpora pin it). Turn it
on for the served desktop: `lion-compositor --transitions`. The
catalog names nine kinds (workspace change, app switch, fullscreen,
display connect/disconnect, lock/unlock among them); the geometry
drivers ride the render-thread roadmap line — the curves ship, the
slides arrive with it.

## 3.12 The dialog: `shell.get_dialog`

Since v0.16.0 the frozen dialog surface is served. A dialog is a
transient window attached to a toplevel — the "Save changes?" shape:

```text
surface = compositor.create_surface()
dialog  = shell.get_dialog(surface, parent_toplevel, modality)
#   modality: 1 (modal — gates the parent's input) or 2 (modeless)
# → dialog.configure(serial, 0, 0)   — the client's own size answer
dialog.ack_configure(serial)
surface.attach(buffer); surface.commit()   # the server centers it
```

The server centers the dialog over the parent's content area at the
first attach (clamped into the usable area — the position rides the
mapping commit, so the dialog never lands origin-then-jumps). The
`ack_configure` handshake is **strict**: the serial must be the live
proposal's (the dialog's size proposal is a contract, not the popup's
advisory placement — a stale serial is a protocol error). A surface
may take **at most one role** (toplevel, popup, dialog, or
subsurface): a second claim is refused with `invalid_state`.

**A modal dialog gates its parent's whole tree** while it is mapped:
the parent (and its popups) take no input — no pointer hits, no keys
(the parent's keyboard focus moves to the dialog at its mapping
commit). Modeless dialogs gate nothing. The gate lifts the moment the
dialog closes or dies.

**A dialog cannot outlive its window**: the parent's death closes
every dialog attached to it (`dialog.close` — destroy the dialog
object and surface when ready). The dialog's own surface dying closes
its machine the same way. `set_title` serves the bounded-string
budget (the same limit every string argument carries).

## 3.13 The states arm: maximize, fullscreen, minimize, workspaces

Since v0.17.0 every request the frozen toplevel interface declares is
served. The **geometry verbs** ride the two-phase commit:

```text
toplevel.maximize()
# → toplevel.configure(serial, states={maximized}, 1910, 986,
#                       insets=5/5/5/5, workspace=0, output=null)
toplevel.ack_configure(serial)
surface.attach(the_1910x986_buffer); surface.commit()
#   the position lands with the commit — the window fills the
#   workspace area (the usable area minus the insets your
#   decoration mode reserves; CSD reserves the 5-px system hit zone)
```

`fullscreen(output|null)` covers the **whole output** (zero insets,
the mode's size; `null` pins the current output — the proposal
carries your own bound output object). The client's ack plus the
committing buffer realize the placement; the **un-verbs**
(`unmaximize`, `unfullscreen`) propose the client's own size (0x0)
and the realizing commit returns the window to the restore point —
the position it held when server geometry first engaged.
Fullscreen precedes maximized: both flags may ride the states, but
the geometry is fullscreen's.

The **visibility verbs** apply immediately (the shell's own visual
decision, not a proposal to realize):

* `minimize()` — hidden from the space, kept alive: the window
  renders nowhere, takes no input, parks its frame requests (App
  Nap: a live `frame` registration dies with
  `frame_dropped(surface_hidden)`, and the parked request is
  answered the moment `unminimize()` returns it), and leaves its
  outputs (`leave_output` — `enter_output` on the way back). The
  desktop after the unminimize is byte-identical to the one before
  the minimize.
* `set_workspace(index)` — moves the window to the space, clamped
  into the count; `workspace_changed(actual)` reports where it
  landed, and the configure's `workspace` field carries it. The
  shell bind answers `workspace_count` (four by default;
  `--workspaces N` on the compositor opts another count in — `0`
  promotes to one).
* `set_sticky(true)` — the window shows on *every* space.

The **hints** (`set_title`, `set_app_id`, `set_min_size`,
`set_max_size`) land on the machine (the strings carry the
bounded-string budget — over-length is refused before the wire, an
embedded NUL is `invalid_string`; the size pair saturates: a min
above a max clamps to it). `ack_configure` is **strict** — the
serial must be the live proposal's; a superseding proposal kills the
previous serial, so stale acks are refused with `invalid_state`.
The mint's own handshake (the initial configure, serial 1) is a
proposal like any other: acking it is accepted.

## 3.14 The operator's hand: interactive move and resize

Since v0.18.0 a client can hand the geometry to the compositor —
the title-bar drag and the edge grip, the two interactions every
desktop serves. Ask under the seat's freshness serial (the one
serial you have seen most recently from the seat — today that is
the handshake configure's; press-issued serials are a named
roadmap line):

```text
# after the press, with the serial the seat last issued:
toplevel.start_move(seat, serial)
#   the window follows the pointer from here — no configure will
#   ever arrive for the move (position is server truth). The drag
#   ends at the button's release; the geometry stands.

toplevel.start_resize(seat, serial, edges=bottom_right)
#   → toplevel.configure(serial, states={resizing}, w, h, ...)
toplevel.ack_configure(serial)
surface.attach(the_wxh_buffer); surface.commit()
#   the size AND the position realize together with this commit —
#   a left/top grip moves the origin at the same moment (the
#   opposite corner anchors). The release brings one final
#   configure with the resizing state cleared — ack it too.
```

The eight edges are the `resize_edge` enum (top, bottom, left,
right, and the four corners). Your min/max size hints clamp every
proposal — the drag never proposes a size your own grammar
forbids. A **move** on a maximized or fullscreen window
**demotes** it: the states release, the floating size restores
under the pointer's proportional grip, and one configure carries
the restored size — ack and commit it; the drag carries on in the
meantime (the shrink lands with your commit, never tearing). A
**resize** on a geometry-stated window is refused
(`invalid_state` — the states own the size; demote first). The
one softening of the strict ack lives here: during a live resize,
the last 8 superseded drag serials stay acknowledgeable — ack the
configure you actually *saw* even if the pointer has already moved
on; the fresher proposal stays live for your next ack. (A
pointer-paced flood also collapses in your event queue: you read
the freshest live configure per wake, the final one always
delivers.)


## 3.15 The drawn chrome: server decorations and the close button

Since v0.20.0 the chrome your `get_toplevel(surface, decoration=
server)` reserved is drawn by the compositor itself — the title
bar, the border ring, and the close button. Nothing changes in
your protocol flow: the insets your configure carries are the
same reservation they always were, your content simply never
underdraws the band. Two events can now reach you that could not
before:

```text
# the operator clicked the drawn close button:
toplevel.close()
#   no arguments. Destroy the toplevel when you are ready —
#   save your work, flush your state, then destroy. The server
#   never force-kills: ignoring the event keeps your window.
```

The close click follows the caption doctrine: the operator's
*press-then-release inside the button* is the ask (a press that
drags away cancels — the click that left never happened). Your
pointer never sees either half: the band is compositor-owned
pixels, its presses are consumed before routing — the only thing
that crosses the wire is the ask itself. Client-decorated windows
(`decoration=client`) see none of this: your buffer's top rows
are your own title bar, your presses route to you, and no close
button is drawn for you. Fullscreen covers the chrome entirely
(zero insets — the close affordance goes with the band).

If you draw your own title bar but still want to react to a
server-side close ask (session policy, the future window menu),
the `close` event arrives the same way — the event is toplevel
vocabulary, not SSD vocabulary: any toplevel can receive it.

## 3.18 The Liquid band: the title bar's glass

Since v0.23.0 the drawn title bar wears the system's own glass: at
any effects tier above `minimal` (§3.8), the band is a *frosted
pane* — the chrome material, the same glass the dock wears —
sampling what sits beneath it, blurring it, and veiling it in the
system's light tint, with the chrome hairline catching the light on
the frame's top edge and the window's corners rounded like every
glass pane. Nothing changes for your client:

```text
# the whole story is the server's (frozen protocol, unchanged):
#   --effects low|medium|high  → the band is glass
#   --effects minimal          → the band is the flat bar (v0.22 bytes)
```

The behaviors your users will see:

* **The glass reads the desktop**: a red window sliding under the
  band warms it, a blue one cools it, the blur mixing the boundary
  between them — the title bar is genuinely backdrop-dependent, the
  same frost the menu and sheet materials serve.
* **The readability floor**: the band's own veil anchors the
  lightness — over any backdrop the bar stays light and the dark
  title ink keeps its contrast; the title text, the border ring,
  and the close capsule are as crisp as ever.
* **The content rides above the pane**: your window's buffer draws
  over the glass — opaque content is pixel-identical to the
  undressed render (only the band, the ring, and the frame's
  corners carry the material).
* **`minimal` is honest**: the flat bar every Phase 52-54 oracle
  pinned, byte-identical — the quality budget degrades the look,
  never the truth.

## 3.19 The chrome-aware placement: the caption is always on-screen

Since v0.24.0 the placement engine answers in *frame* space: a
server-decorated window parks with its **frame** — the band and the
border ring included — at the policy slot, the content riding
inside. Your users will never see a title bar clipped off the top
of the screen, wherever the system places the window:

```text
# the client's whole story (frozen protocol, unchanged):
#   nothing — the placement is the server's own geometry
```

The behaviors your users will see:

* **The parked window's band is visible**: a window the system
  places at the usable origin (the first cascade slots, the phone's
  app stack) shows its caption from row 0 of the display — the
  title readable, the close button reachable, the drag grip live.
* **The cascade steps captions**: each new window's *title bar*
  takes the diagonal step — the stack reads like every desktop a
  user has ever used (DWM and WindowServer both answer this way).
* **Plain and client-decorated windows never moved**: a surface
  without the drawn chrome keeps the exact pre-v0.24.0 placement
  (its content is its whole window — nothing to grow), proven by
  the identity sweep in the layout's own tests.
* **Migrations keep the caption visible**: a display hotplug
  re-places the stack with the frame inside the new usable area; a
  fullscreen window (zero insets) migrates as plain geometry.
* **Presses follow the visible ink**: a band that sits over another
  window's content claims the press for its own window (focusing
  it, gripping it) — never for the hidden ink beneath it.

## 3.17 The title glyph: the drawn text in the band

Since v0.22.0 the drawn title bar carries its text: the string
your client set with `set_title` renders as real glyphs — Lion
Sans, the compositor's own typeface — over the band Phase 52
painted. Nothing changes for your client beyond one honest
expectation: the title is now *visible*.

```text
# the client's whole story (frozen protocol, unchanged):
toplevel.set_title("Image Viewer")   # any time, before or after mapping
# ... the band renders: [ Image Viewer              ( × ) ]
```

The behaviors your users will see:

* **The truncation**: a title wider than the band's budget (the
  frame minus the close button's territory) ends in an ellipsis —
  `Image Viewer — Deep...` never spills onto the close affordance,
  never wraps, never scrolls. The decision is pixel-true (the
  measured widths, never a character count).
* **The change**: `set_title` at any time repaints the strip on
  the next frame — no commit, no damage, nothing else required of
  the client (the repaint is the server's own claim).
* **The honest boxes**: the typeface covers ASCII and the
  ellipsis; anything else draws the notdef box (a visible hollow
  rectangle — truthful, never garbage); control characters draw
  blanks.
* **CSD windows** draw no server title (the buffer's top is the
  client's own chrome, as ever); **fullscreen** covers the band
  and the title with it.

## 3.16 The caption drag: moving windows by their title bar

Since v0.21.0 the drawn title bar is a *move grip*: the operator
presses anywhere on the band (outside the close button), moves,
and the window follows — the server mints the move drag itself,
no request ever crossing the wire. Nothing changes for your
client:

```text
# the operator's narrative — the whole story is server-side:
#   press on the band     → armed (consumed, the window focuses)
#   first motion          → the server mints the move drag
#   every motion batch    → the window moves (server truth)
#   release               → the drag ends, the geometry stands
# your client sees: nothing. No button, no motion while the
#   grip is held, no configure — position is server truth.
```

The doctrines your client can rely on: a *press without motion
never drags* (a click on the band is the Phase 52 behavior —
focus only), the move never proposes a configure (the position
applies server-side at the input pump's cadence; only a
*maximized* window dragged out by its caption sees one — the
demotion's floating-size restore, acked and committed at your
own cadence, never tearing), and the close button's own grip
never becomes a move (its press-drag-release is the cancel, not
a drag). Client-decorated windows see none of this either way:
your buffer's top is your own title bar, your presses route to
you, and moving your window remains `start_move` — the
interactive-drag request your own title bar serves (§3.14's
drag vocabulary, the same machinery two doors away).

## 3.20 The chrome ghost: the whole window leaves as one

Since v0.25.0 the close fade takes the whole window: when a
server-decorated window leaves the desktop, its band and title
ride the same fade as its content — one frame, one opacity, one
spring. Before this release the choreography told half a story:
the content faded under the Phase 48 ghost while the drawn band
left with the route, one abrupt frame. Now:

```text
# the operator's narrative (the close fade, --transitions on):
#   the window dies          → the ghost begins: band, strip, content
#   every frame              → the whole frame dims together
#   the settle               → the plain post-destroy desktop
# A/B: the settled frame equals the same destroy with
#   transitions off, byte for byte — the fade is pure choreography.
```

What your client can rely on: the fade is server-side (nothing
crosses the wire — the destroy or the unmapping commit you
already send is the whole story), the eligibility is the Phase 48
doctrine verbatim (popups and ephemeral roles dismiss instantly,
a hidden window leaves no ghost, and with transitions off — the
library default — the destroy is plain, the band leaving with the
route as it always has), and the settled desktop is the
never-animated bytes (the A/B oracle, byte for byte). At a Liquid
tier the fading band keeps its glass until the settle: the frost
still reads the desktop behind it — a window leaving in glass,
not in pieces.

Client-decorated windows are untouched: your buffer's top is
your own title bar, and your close fade is the content ghost
alone, exactly as Phase 48 served it.

## 4. Sockets and discovery

LDP transports run over abstract-namespace AF_UNIX sockets (no
filesystem path, no stale socket files to clean up). Every
client-side binary resolves the server the same way:

1. the `--socket NAME` (or `--socket=NAME`) flag, else
2. the `LDP_SOCKET` environment variable, else
3. a usage error telling you to provide one.

The name is the abstract-namespace name without the leading NUL; tools
accept it with or without an `@` prefix. The compositor's startup line
prints the canonical `@name` form to copy-paste:

```console
$ lion-compositor --mode headless --socket workbench
lion-compositor (headless): listening on @workbench — Ctrl-C to exit

$ LDP_SOCKET=workbench ldp-info --live
```

Session identity rides on `SO_PEERCRED`: the server reads the client's
kernel-attested uid/pid at accept time — there is no anonymous access
to attribute or audit.

## 5. The operator tools

Eight operator tools ship in the `ldp-tools` binary package (and in
`target/release/` after a source build); `ldp-bench` and `ldpc` ride
along in the same package, and the `ldp-remote-gateway` relay (§9)
completes the set. All of them report usage problems as exit code 2,
reportable failures as 1, success as 0 — and every binary in this
repository, tools and examples alike, answers `--version` / `-V`
with ` <name> <release version>` on stdout, exit 0 (the uniform
convention `scripts/version_check.py` keeps coherent).

### ldp-info — protocol summary, live session, hardware probe

```console
$ ldp-info                       # the compiled protocol at a glance
$ ldp-info --live                # connect: server/seat/output reports
$ ldp-info --display             # DRM/KMS topology + GPU render nodes
$ ldp-info --live --socket NAME # explicit socket
```

Offline it prints the registry census (modules, interfaces, globals,
opcode counts) straight from the compiled schema tables — useful as a
sanity check that the binary you deployed carries the protocol you
expect. `--live` connects and reports what the running session
actually serves. `--display` is the hardware probe.

### ldp-debug — live tracer and scheduler replay

```console
$ ldp-debug trace --socket NAME [--duration SECS] [--bind IFACE]...
                   [--generate N] [--class CLASS]... [--filter TEXT]
$ ldp-debug replay <FILE>
```

`trace` connects and prints every dispatched event, one line each,
with a per-class summary at the end; `--generate N` commits N
presentation frames so presentation traffic is visible in the trace.
`replay` decodes a scheduler recording (the `LDP7REC` format written
by the profiler) and prints the timeline with its deadline hit-rate.

### ldp-validate — spec ↔ served-schema cross-check

```console
$ ldp-validate [--spec-dir DIR]
$ ldp-validate --live [--socket NAME]
```

Offline it lints a spec directory the same way `scripts/verify.sh`
does. `--live` is the drift tripwire: it cross-checks the schema a
running server serves against the spec TOML you point it at, so a
server built from a different protocol generation is caught before it
confuses clients.

### ldp-profiler — latency and deadline hit-rate

```console
$ ldp-profiler [--socket NAME] [--frames N] [--size WxH] [--timeout SECS]
$ ldp-profiler --replay FILE
```

Live mode commits N frames through the full choreography — frame
request, deadline, attach, damage, commit, presentation verdict — and
reports wall latency, pipeline pacing, and the deadline hit-rate.
`--replay` runs the same statistics over a recording. This is the tool
that produced the numbers in [`docs/benchmarks.md`](benchmarks.md).

### ldp-audit — audit-log verifier

```console
$ ldp-audit <FILE> [--records] [--expect-head HEX]
$ ldp-audit --live [--socket NAME]
```

Verifies a JSONL audit log: structural integrity (no deleted or
duplicated records) then the full SHA-256 hash-chain walk (reordering
is caught by the chain, truncation by `--expect-head`, the operator
checkpoint). `--records` prints one line per record.
`--live` inspects what a running server offers.

### ldp-input-debug — evdev dump analysis

```console
$ ldp-input-debug <FILE> [--events] [--name NAME]
$ ldp-input-debug --live [--socket NAME]
```

Reads a binary evdev dump (24-byte `input_event` records), infers the
device class (mouse/keyboard/touch/tablet), and reports the spec the
server would derive. `--events` prints every raw event. `--live`
inspects the running server's input surface.

### ldp-grab — output frame capture (Phase 22)

```console
$ ldp-grab [--socket NAME] [--out FILE] [--format png|ppm]
$ ldp-grab [--socket NAME] --count N [--interval SECS] [--out BASE]
```

Captures what the compositor is presenting right now: binds the
`ldp.capture.capture_manager` global, sends `grab`, and receives the
read-only frame snapshot (premultiplied ARGB8888, `width × height`).
The frame is un-premultiplied to straight RGB (the dump convention) and
written as a **PNG** (default; deterministic output, diffable in CI) or
a raw **PPM** (`--format ppm`; byte-exact against the scanout). With
`--count N`, grabs N frames `--interval` seconds apart, naming the
files `BASE-<k>.<ext>`. Works unchanged over the remote transport —
dial the edge gateway's socket and the frame arrives through the
relay's whole-file snapshot vocabulary, pixel-exact against the
compositor's own scanout.

### ldp-bench and ldpc

`ldp-bench` runs the release benchmark suites (`--json PATH` and
`--markdown PATH` for machine- and report-readable output). `ldpc` is
the protocol compiler: `ldpc gen` regenerates the Rust schema tables
and reference docs from `spec/*.toml`, `ldpc gen --check` is the drift
gate, `ldpc validate` lints a spec directory.

## 6. The example applications

Three examples ship under `/usr/lib/lion-display/examples/` (deb
install) or `target/release/` (source build). Each is a complete,
scripted LDP client built on `ldp-client`:

* **hello-ldp** (`hello-ldp [--socket NAME]`) — connect, commit one
  frame, read the verdict: the whole presentation contract (buffer,
  damage, commit, `presented` event with the pixel-exact scanout
  assertion) in one process. Read its `src/lib.rs` first; it is the
  canonical "smallest real client".
* **pointer-paint** (`pointer-paint [--socket NAME]`) — an evdev trace
  through the real input normalizer into a pixel-exact stroke on a
  client surface: the input→event→render pipeline in miniature.
* **clip-client** (`clip-client [--live [--socket NAME]]`) — with no
  flags, the full in-process clipboard offer/accept/receive negotiation
  (two synthetic clients through the real manager, a real pipe, EOF
  discipline); `--live` probes the running server's clipboard.

## 7. Writing a native client

The client library is `ldp-client` (crates/ldp-client). The shape of a
session:

1. `Connection::connect(addr)` — dial the abstract socket and complete
   the hello/welcome handshake; the server's advertised versions and
   capabilities come back on the connection object.
2. Bind the globals you need (registry, seats, outputs, shm), each
   returning a typed proxy object. Proxy handles are generation-safe:
   if the server destroys an object out from under you, later use is a
   typed `stale_object` protocol error, never a misdelivered call.
3. Drive your surface: create a buffer pool (memfd-backed), attach a
   buffer, accumulate damage, commit against the server-pushed
   presentation deadline (`frame_target` events carry the target
   timestamp, refresh interval, and latency budget — you schedule
   *against* the deadline; you never ask "may I draw?").
3a. *Since v0.15.0, the semantic claims* (optional, on the toplevel):
   `set_semantic_role` tells the server what the window *is* — a
   `dialog` joins the choreography, a `tooltip`/`overlay` appears
   instantly, a `lock` surface is never capturable (the security
   floor applies whatever the class claims);
   `set_security_class(protected)` keeps the window on the user's
   display but **out of every screenshot and capture**
   (`normal`/`private` capture; `private` classifies for the
   screen-share negotiation to come); `set_scene_profile(gaming)`
   floors the deadline walk's admission at 1 ms — the tightest
   makeable pacing for latency-critical windows (`creative` at 4 ms
   for editors; `desktop` is the operator's own policy). The claims
   are policy, never render orders: the server keeps the quality
   budget and its own invariants.
4. Read events through the **class lanes**: input-class events are
   drained with priority over presentation-class floods, so a client
   animating at 240 Hz cannot starve its own input handling. This is a
   property-tested guarantee, not a best-effort promise.
5. `sync()` for a round-trip barrier, and the disconnect/reconnect
   helpers when you want clean teardown semantics.

The rustdoc (`cargo doc -p ldp-client --open`) documents every type;
the three examples are the living documentation of the patterns. The
wire contract itself is specified in `spec/*.toml` and rendered into
[`docs/reference/`](reference/) by `ldpc`.

## 8. The deterministic headless doctrine

The mock display's clock advances only at protocol wake points, so a
fixed client script produces a byte-identical server event stream.
This is why the conformance suites can assert golden schedules, why
the fuzz corpus is replayable by seed, and why `--dump` frames are
comparable across runs. Practical consequence for you: timing you
observe against the headless server reflects protocol logic, not wall
clock; measure wall-clock behavior with `ldp-profiler` against a
release build.

## 9. Remote display: the gateway pair

`ldp-remote-gateway` carries whole LDP sessions across hosts (Phase
21). An **edge** near the client accepts ordinary local `AF_UNIX`
connections — clients are completely unaware they are remote; the
`--socket` you pass them is the edge's name — and bridges each one
over authenticated TCP to a **hub** near the compositor, which dials
the compositor's socket as a local client.

```console
# On the compositor host:
$ ldp-remote-gateway hub --listen 0.0.0.0:6400 --compositor ldp-0     --token-file /etc/lion/remote.token
# On the client host:
$ ldp-remote-gateway edge --listen ldp-remote --connect 10.0.0.2:6400     --token-file /etc/lion/remote.token
$ LDP_SOCKET=ldp-remote showcase        # any LDP client, unmodified
```

The token file holds 32 raw bytes or 64 hex characters; keep it mode
`600`. `LDP_REMOTE_TOKEN` (hex) is the environment alternative. Both
sides negotiate caps at the `HELLO` handshake: `--max-envelope MiB`
(default 64 — a 4K XRGB frame is ~33) and `--pool-cap MiB` (default
256, total pooled shm per session). Keepalive defaults to a 10 s ping
with a 20 s dead-peer deadline; `--keepalive SECS` tunes both, and
`--no-keepalive` disables the monitor.

What crosses the wire: messages byte-identical, shm pool contents at
`create_pool` and the attached buffer's window before every commit
(the write-then-commit contract holds — the end-to-end suite proves
scanout **pixel-exact** through both gateways), pool growth, eventfd
release fences as counter snapshots, keymap/ICC files whole, and
clipboard-class pipes as streamed chunks. What refuses the session
with a diagnostic instead: GPU descriptors (`dmabuf` buffers,
explicit-sync fences) — those are Vulkan-era work. Scope to know
before deploying: the hub attributes every session to the gateway's
own local account (single-user remote display), and the token is a
bearer secret, not TLS — use private links.

## 10. Environment variables

| Variable | Effect |
|---|---|
| `LDP_SOCKET` | Default abstract socket name for every client-side binary (flag wins). |
| `LDP_STRESS_FULL` | Set to 1 to run the full 15-minute stress gate instead of the compressed CI form (`cargo test -p ldp-integration`). |
| `LDP_FUZZ_SCALE` | Soak multiplier for the deterministic fuzz targets (`fuzz/`). |
| `LDP_CORPUS_DEBUG` / `LDP_CORPUS_TRACE` | Debug/trace output from the corpus generators. |
| `LDP_REMOTE_TOKEN` | The remote gateways' bearer token (64 hex chars; `--token-file` wins). |

## 11. Benchmarks and profiling

The committed release numbers — codec encode/decode in the hundreds of
nanoseconds, a 1 KiB transport round-trip at ~1.1 µs, 64 KiB frames at
~8.4 GiB/s, the 60 Hz scheduler decision at ~62 ns, the constant-time
token walk at ~55 ns — live in [`docs/benchmarks.md`](benchmarks.md)
with methodology and the full stress-gate report. Reproduce with:

```console
$ cargo run -p ldp-test --release --bin ldp-bench -- --markdown report.md
```

## 12. Troubleshooting

* **"no socket name: pass --socket NAME or set LDP_SOCKET"** — you ran
  a tool with neither the flag nor the variable. The compositor's
  startup line prints the `@name` to use.
* **Connection refused / `ECONNREFUSED`** — no server listens on that
  abstract name. Check the compositor is running and the name matches
  exactly; remember the default name embeds the server PID, so pin one
  with `--socket` in scripted setups.
* **Tool exits 1 right after connect** — the server closed the session
  with a protocol error; the tool's stderr carries the typed error
  (stale object, bad argument, permission denial). `ldp-debug trace`
  against the same socket shows the server-side event stream.
* **Compositor exits 0 immediately on a desktop** — that is `--mode
  auto` finding a DRM device and completing its probe-only scope. Use
  `--mode headless` explicitly when you want the protocol service.
* **`--dump` directory stays empty** — frames are written per
  *composited* frame; a server with no client commits composites
  nothing. Run `hello-ldp` against it first.
* **Remote client fails instantly** — three suspects, in order: the
  hub is unreachable from the edge (the edge fast-fails the client
  like a dead compositor socket); the tokens disagree (same failure
  shape — check both files byte-for-byte); or the caps are too small
  for the client's frames (raise `--max-envelope`; a 4K frame needs
  ~33 MiB).
* **Remote session drops after ~20 s of idleness** — that is the
  keepalive deadline doing its job on a silent peer; if the peer is
  healthy and merely quiet, either answer pings (the client library
  already does) or raise `--keepalive`.
