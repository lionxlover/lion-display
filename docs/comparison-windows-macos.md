# LDP vs. Windows DWM and macOS WindowServer — the architecture comparison

*Phase 44 (v0.12.0), with Phase 45 (v0.13.0) closing three of the
named lines this document priced: the failure stage's supervisor,
the power stage's occlusion quiescing, and the input stage's
axis-metadata coalescing; Phase 46 (v0.14.0) completing the
mode foundry's timing families (every standard the VESA documents
define — the display-size field's custom-resolution depth, poured
as arithmetic); and Phase 47 (v0.15.0) closing the identity line
neither giant prices as architecture because both inherited it
implicitly: **the semantic scene** — surfaces that claim what they
*are* (role), what they may *expose* (security class — the
protected-content redaction at the compositor's own capture gate,
where DWM's SetWindowDisplayAffinity and WindowServer's
screen-capture exclusion live as per-window policy), and how the
machine spends its frame budget on them (the scene profile's
admission floor on the deadline walk), plus the composition
decision engine (every frame's path named with its cause) and the
compositor-owned window choreography (the springs every
well-designed app gets for free). Phase 48 (v0.16.0) closed the
transient-window line (the dialog arm with the modal gate, the
close fade), and Phase 49 (v0.17.0) closed the states line — the
frozen 17-request toplevel vocabulary served whole: the geometry
verbs as the real two-phase commit (where DWM's `ShowWindow` and
WindowServer's deminiaturize animate server-side without the
client's ack, LDP's maximize/fullscreen realize only at the
client's own committing buffer — no flicker race by construction),
and the visibility verbs with App-Nap frame parking (the minimize
DWM hides by clipping the window from the composition; LDP parks
the frame economy too — the occlusion line's own machinery, reused).
Phase 50 (v0.18.0) closed the operator's-hand line — the
interactive move/resize vocabulary the spec never grew: the
title-bar drag and the eight-edge grip, where DWM's
`WM_SYSCOMMAND`/`SC_MOVE` and WindowServer's title-bar drags are
server-side and opaque, LDP's `start_move`/`start_resize` carry
their cadence doctrines *on the record* (the move is server truth
at the input pump's cadence; the resize proposes at the pointer's
pace and realizes at the client's own commit — never tearing, the
grace window keeping pointer-paced supersession from punishing a
frame-cadence client), and dragging a maximized window demotes it
under the pointer's proportional grip exactly as the giants' title
drags do.
The matrix in
[`comparison.md`](comparison.md) scores the three systems field by
field — outcomes, medians, reasoned numbers.
This document goes one level deeper: it walks the display pipelines
themselves, stage by stage, from the moment a client's pixels exist to
the moment they light the panel, and asks how each system actually
works. The two contenders are the only display servers a majority of
humans have ever used: the **Desktop Window Manager** (Windows Vista
2006 → Windows 11, riding the WDDM kernel driver stack) and Apple's
**WindowServer** (the Quartz Compositor of Mac OS X 10.0, 2001, with a
lineage back to NeXTSTEP's Display PostScript, 1988). The policy is
the project's standing rule, unchanged: **honest above flattering** —
every claim about the giants is public, documented behavior; where
their internals are closed the text says "closed" and refuses to
guess; every claim about this repository is measured on its own rig;
and every gap carries the roadmap line that closes it. Nothing here
moves a single score in the matrix — the numbers there stand on their
own evidence. This is the reasoning under the reasoning.*

## The three stacks at a glance

| | **Windows (DWM + WDDM)** | **macOS (WindowServer)** | **lion-display (LDP)** |
|---|---|---|---|
| What composites | `dwm.exe` — a user-mode compositor per interactive logon session, speaking Direct3D | WindowServer — one system process owning every screen, every window, and all input routing | `lion-compositor` — one process over the LDP protocol, software or GLES backend |
| Kernel/driver half | WDDM: `dxgkrnl` (GPU scheduling), a kernel-mode driver and a user-mode driver per vendor, decades of vendor code | IOKit display drivers + the WindowServer contract; the compositor is the driver's only client | DRM/KMS through `ldp-display` (atomic commits, planes, VRR), mock-provable in CI |
| Client buffer path | Redirection surfaces (GDI) or shared DXGI swapchains (flip model, cross-process handles) | IOSurface window backing stores — shared cross-process buffers, drawn by CoreGraphics or Metal | Protocol buffers: shm pools and dma-bufs, imported through the honest walk (`drmPrimeFDToHandle` → `AddFB2WithModifiers`, cached refusals) |
| Change tracking | Win32 clip lists and `WM_PAINT` for GDI; property diffs on retained visual trees (DirectComposition / Windows.UI.Composition) | CoreAnimation transaction diffs — the app commits layer-tree changes, the render server holds the mirror tree | The exact damage algebra (R1–R4), proven against an independent per-pixel reference model by a randomized corpus |
| Input authority | The Raw Input Thread in `win32k.sys`; compositor uninvolved in routing | WindowServer is the input broker — every HID event crosses it (event taps included) | The seat: evdev → normalizer → acceleration → routing, with per-contact position-state coalescing at emission |
| Scanout | MPO hardware overlays + Direct Flip bypass; hardware flip queue | Closed; per-display composition through Metal, display-link paced | The plane-assignment solver: zero-composite frames, underlay splits, named demotions |
| Lineage | 2006 (Vista); GDI's 1985 model underneath | 2001 (Mac OS X); NeXT's 1988 DPS underneath | 2024–2026, this repository, pre-1.0 |

Two structural facts shape everything below. First, **Windows splits
the stack at the driver**: DWM is replaceable user-mode code above a
vendor-owned kernel driver model, which is why the graphics stack
could be rebuilt (Vista) without rewriting applications — and why
driver quality is a permanent third party in every Windows display
conversation. Second, **macOS concentrates the stack in one
process**: WindowServer owns the screen, the window list, the layer
mirrors, and the input routing, and every application is a client of
that one process. LDP's architecture is closer to the macOS shape
(one authoritative compositor process, clients as protocol peers)
implemented with the Linux substrate (DRM/KMS) underneath and one
decisive difference: **the contract is a published, machine-frozen
protocol, not a private IPC surface** — the freeze gate (`ldpc
snapshot --check`) breaks the build if the wire moves, where
Apple's and Microsoft's private interfaces move with every release
cycle at their sole discretion.

## Stage 1 — the client's pixels

**Windows.** A Win32 application's GDI drawing lands in a
*redirection surface* — a DWM-managed buffer the window believes is
its own screen memory (the compatibility fiction that let 1985-era
applications survive 2006's compositor). The fiction is expensive:
the window's drawing is a full copy class, and the pre-DWM world
(clip lists, `WM_PAINT` redraw regions, flicker) survives inside it
as the redraw protocol. Modern applications skip the fiction: a
Direct3D swapchain presented through the DXGI flip model shares
buffers with the compositor directly, and since Windows 8 the flip
model allows the application's own back buffer to become the
scanned-out front — zero copy at the limit. The two paths coexist,
which means the DWM carries two client-buffer doctrines forever.

**macOS.** There is no fiction: every window has a *backing store*
(an IOSurface, since 10.6), the application draws into it with
CoreGraphics or Metal, and WindowServer composites from it. The
backing store is window-server-managed memory the app maps — the
same sharing idea as dma-buf, a decade earlier. One famous behavior:
occluded windows' backing stores are compressed by the server, which
is why macOS desktop memory behaves better than its window count
suggests. CoreAnimation adds the second path: a retained layer tree
whose *property diffs* commit over Mach IPC, so an animation can run
entirely inside the server with no app involvement (the app is not
redrawn; its layer tree's future states were handed over).

**LDP.** The client owns its buffers and offers them: shm pools for
the software path, dma-bufs with explicit format modifiers for the
hardware path. The import walk is the honest core — each pool fd
walks `drmPrimeFDToHandle` and `AddFB2WithModifiers` at *use* time,
refusals are typed and cached (a bad buffer is refused once, named,
never retried into a stall), and the dma-buf feedback tranches tell
the client where to allocate next. The buffer is never copied for
the compositor's own reading unless the renderer's tier demands it.
The doctrine is the flip model's — the client's pixels are shared,
not submitted — but it is *symmetric*: every client, not only the
one that found the modern API.

## Stage 2 — the compositor's authority

**Windows.** DWM is a per-session user process with the authority to
composite the session's desktops. The security boundary is layered
underneath it: UAC's secure desktop is a separate desktop object the
logon process switches to (the user session's DWM does not compose
it), `UIAccess` grants accessibility automation a constrained path
across integrity levels, and the infamous `win32k` callback surface
sits beneath everything as a decades-long CVE ledger. DWM crash
recovery (Vista+): the session flashes and rebuilds — the
redirection surfaces persist, so windows survive their compositor.

**macOS.** WindowServer is *the* arbiter: no application can address
a display at all — screen, window registry, and the input stream all
route through the one process, and the sandbox, the ScreenCaptureKit
permission model, and the event-tap mediation all hang off that
concentration. The failure mode is correspondingly hard: a
WindowServer crash logs the user out. Application crashes, by
contrast, are perfectly isolated — the backing stores and layer
mirrors are server-owned, so the desktop's state outlives any client.

**LDP.** One compositor process holds the same concentration (no
client addresses the display; every buffer, every window, every
seat routes through it) with the authority modeled as *data*: 18
operations cross a permission matrix (the decision is ~0.23 ns,
re-measured this release), sessions hold constant-time-verified
tokens, and every grant and revocation lands in a hash-linked audit
chain the admin can walk. The X11 and Wayland bridges ride their own
scoped identities — a bridge process refuses to start without its
bridge-scope token. Crash discipline is tested, not asserted: the
crash corpus (`tests/src/crash.rs`) kills sessions mid-protocol and
asserts the teardown's FD and state honesty, and the server's own
suite kills it mid-dispatch. What LDP does not have is the giants'
certification apparatus — no driver-signing program, no vendor
test matrix; the honest substitute is the mock-KMS serve loop that
makes the kernel contract itself CI-provable.

## Stage 3 — knowing what changed

This is the stage where the three systems' ages show most.

**Windows** carries its 1985 layer openly: GDI invalidation
(`InvalidateRect`, clip lists, `WM_PAINT`) is *client-declared*,
rectangle-granular, and famous for both its economy and its lies
(applications that invalidate everything, applications that
invalidate nothing and draw stale). DirectComposition fixed the
model for the modern path — a retained visual tree where property
changes (position, opacity, transform, effects) are *diffed* and
only the diff composites — but the GDI path can never be retired,
so the DWM forever carries both doctrines. Its dirty accumulation is
closed; the observable behavior is that composition cost tracks
damage, not desktop area.

**macOS** never had the legacy: CoreAnimation's model is retained
from birth. The app commits a transaction (implicit, run-loop
flushed, or explicit); the render server's mirror tree applies the
diff; the *contents* of a layer are versioned (the IOSurface flips
to the new content), so "damage" on macOS is really "layer content
version changed" plus "layer properties changed" — a coarser,
happier granularity than rectangles. The cost is the layer-tree
abstraction itself: an application that wants per-pixel damage
control (a terminal, a browser) fights the model, and the famous
pathologies (a dozen transparent layers over video) come from the
model's generosity, not its stinginess.

**LDP** made the opposite bet: damage is *exact, algebraic, and
provable*. The four rule families (content, coverage, opaque flips,
restack pairs) compute the precise repaint set as a fold over the
surface tree, and the randomized corpus checks the algebra against
an independent per-pixel reference implementation — the claim
"damage is right" is machine-checked, not folklore. The v0.12
release makes the algebra's *quiet frame* cheap too: the pass
records skip unchanged surfaces (the mask clone is gone), the
suffix unions materialize only at opaque flips, and a 64-window
static desktop's damage pass measured **94,720 → 44,609 ns/op
(−52.9%)** on the release rig, with the one-moving-window load at
**126,906 → 77,199 ns/op (−39.2%)** — the A/B against the pristine
v0.11.0 engine on identical input. No giant publishes this number;
the discipline that produces it is the difference.

## Stage 4 — the composite itself

**Windows.** DWM composites through Direct3D into the primary
surface, historically one full-desktop pass per frame. The
optimizations are the interesting half: **Direct Flip** (Windows 8)
lets a fullscreen opaque swapchain bypass composition entirely —
the game's buffer scans out untouched; **MPO** (multi-plane
overlay, Windows 8.1+) moves the same idea into the windowed world
— hardware overlay planes composite a video or a game *below* the
DWM's own pass, the "independent flip" that saves the whole
composition pass; and the hardware flip queue moves flip scheduling
into the display hardware itself. The underlay shape — canvas on
the primary, windows on overlays above — is exactly the split
LDP's plane solver serves.

**macOS.** WindowServer composites per display through Metal (the
OpenGL era is behind it), pacing off the display link. The
compositor's effect vocabulary is the platform's signature — the
shadow cache, the per-window material blur, the spring animation
curve — and it is exactly here that the closed source hurts an
honest comparison: the *mechanisms* (a blur pass, a shadow atlas,
a texture cache) are inferable from public behavior and profiling,
but the *policies* (when a blur costs what, when a shadow
re-rasterizes) are not published. What is observable: heavy
translucency is the classic macOS composition cost, and the
window server's CPU/GPU usage tracks it.

**LDP.** The renderer contract carries two backends — the
specialized software path and GLES 2.0 — *byte-equal by
construction* (the GL stream draws the identical memoized ring
words; command-stream goldens pin exactly what the GL renderer
emits). Above it, the plane-assignment engine (`ldp-planes`)
maps the layer stack onto the CRTC's plane inventory: the
split-point doctrine, a zpos-monotone backtracking search, a named
demotion per composite layer (the no-silent-degrade rule applied
to offload), the **zero-composite frame** (a fullscreen opaque
client's own framebuffer on the panel, the renderer emitting no
pass at all), and the **underlay split** (canvas on the primary,
windows on overlays) — proven end-to-end through the real wire on
the mock's real plane inventory. The honest gap against the
giants: GLES 2.0 with no compute arm, no Vulkan, and none of the
per-vendor quirk decades — named lines the reserve phases hold.

## Stage 5 — presentation feedback and pacing

**Windows.** The DWM composition clock is a public API
(`DwmGetCompositionTimingInfo` — the QPC-anchored refresh grid),
DXGI present statistics report per-frame timing to the presenting
application, and waitable swapchain objects turn "the frame
landed" into a synchronization primitive. This is the most mature
presentation-feedback surface of the three; games and media
players build their pacing on it.

**macOS.** CVDisplayLink is the vsync callback surface; the layer
tree's media timing (begin/duration curves) paces animations
inside the server. What macOS does not offer is the frame-level
accounting Windows does — the display link tells you when the
display refreshes, not what happened to *your* frame.

**LDP.** The presentation clock is a protocol surface: every
surface targeting a flip gets a verdict *at the flip that carries
its content*, the adaptive opportunity target paces the client's
deadline at the panel's own probed window, and the LFC cadence
(k = ceil(P/max), judder-free by construction) bridges slow
content into the window. The phase-locked deadline grid is
deterministic in CI. The gap is the same one the input-latency
row carries: the giants' numbers ride measured hardware panels
with oscilloscopes; LDP's are mock-proven budgets with the
architecture in place — the real-panel latency lab is the named
roadmap line.

## Stage 6 — input

**Windows.** Input never touches the compositor: the Raw Input
Thread in `win32k.sys` owns device reading, legacy mouse-move
coalescing happens in the message queue (the doctrine a 1000 Hz
device parks sixteen samples and delivers one `WM_MOUSEMOVE`),
and `WM_INPUT` carries the uncoalesced raw stream to whoever
asks. The split is old, deliberate, and robust — the compositor
can crash without the keyboard dying.

**macOS.** WindowServer *is* the input path: the HID stack
delivers into it, it routes to the focused application, and event
taps (with accessibility consent) observe and transform the
stream. The concentration gives macOS its coherence (one place
implements gesture recognition, one place decides focus) and its
single point of failure.

**LDP.** The seat is a protocol-level subsystem: evdev reading,
the normalizer (button/key semantics), per-device pointer
acceleration (the smoothstep curve, property-pinned), grab and
focus policy, and — since v0.11.0, extended in v0.12 and again in
v0.13 — the **per-contact position-state coalescing** at the
emission path: a 120 Hz touchscreen parking sixteen two-finger
batches delivers TWO motions (each finger's own freshest sample
in its own slot, one shared terminator), a 240 Hz pen collapses
to one motion, one pressure, one tilt per frame, and — since
v0.13 — the contact's geometry axes ride the same doctrine
(`touch.shape`/`touch.orientation` are position state of their
own kinds: the freshest ellipse and angle per contact per display
frame, never a queue of stale ones). Discrete input seals only
its own contact's stream. This is the X11/Windows coalescing
doctrine with per-contact keys and per-axis kinds — a mechanism
neither giant documents publicly at this granularity, proven over
a real socket in CI.

## Stage 7 — power

**Windows.** The compositor's power story is mostly the driver's:
MPO saves composition passes, Modern Standby's connected idle
keeps the desktop light, and panel self-refresh lives in the
display driver's control — DWM's own contribution is the idle
frame skip. The levers are vendor-owned and closed.

**macOS.** WindowServer coordinates the display's sleep, App Nap
quiets invisible applications (a compositor-level decision — the
server knows what is occluded), and ProMotion drops the refresh
rate on static content (the panel's own half-rate and quarter-rate
modes). The occlusion knowledge is the structural advantage: the
process that composites is the process that decides who needs
to run.

**LDP.** The power path is measured, not vendor-owned: per-CRTC
panel self-refresh (the machine that earns entry through
consecutive quiet flips and names every exit — damage, blank,
backlight, unsupported — with the frozen timeline CI-proven: ten
seconds of a still desktop cross the device with zero vblanks),
the GPU clock governor (asymmetric hysteresis over landed-flip
load), the energy ledger (a documented parametric cost model
whose static-scene ratio is arithmetic the session tests
reproduce — the sleeping desktop draws ~13% of the
always-scanning counterfactual), the zero-composite frame, and —
since v0.13 — **occlusion quiescing**, the App-Nap doctrine the
damage pass's own visible map drives: a fully-occluded window's
frame requests park unanswered (no deadline is handed to a client
whose pixels cannot reach the panel), its live registration dies
with `surface_hidden`, and the reveal answers at the moment it
can present. The structural advantage the text attributes to
macOS's concentration — "the process that composites is the
process that decides who needs to run" — is exactly the
compositor's own knowledge here, and v0.13 is where it starts
paying. The honest gap is real eDP hardware: the property
vocabulary exists, the quirk rows accrue with panels.

## Stage 8 — HDR and VRR

**Windows.** HDR arrived as a mode (Windows 10 1703's Advanced
Color: HDR10 ST.2084 or FP16 scRGB, the whole desktop switching),
grew per-app activation, and Auto HDR remaps SDR games into it.
VRR is the WDDM 2.x FreeSync/G-Sync surface — the deepest
per-vendor quirk table in the industry, exactly the depth the
LDP quirk ledger's structure accrues toward. Windows' SDR-in-HDR
problem (the reference-white knob) is the same negotiation LDP
serves explicitly.

**macOS.** EDR — Extended Dynamic Range — is the most elegant
HDR doctrine of the three: HDR content rides *alongside* SDR at
a negotiated headroom without a display-mode switch, the compositor
blending in a domain wider than the panel's nominal peak. VRR is
ProMotion: genuinely good, laptop-and-tablet only, a fraction of
the desktop reach.

**LDP.** The honest peak: the pipeline is scene-linear end to end
(PQ/HLG tails, BT.2408 reference white for SDR), per-surface
mastering metadata rides the BT.2390 knee onto the *negotiated*
ceiling, the panel's effective peak is advertised and operator-
capped (the bloated-EDID quirk's `--hdr-peak`), and one truth
serves both directions. VRR carries the floor clamp, the LFC
latch, and the sibling escape, proven over the mock panel's own
48–144 Hz window. The DRM infoframe emission and ST 2094-40
dynamic metadata stay the named roadmap lines — the giants'
spent decades are the distance.

## Stage 9 — failure and recovery

**Windows.** DWM crashes are recoverable (the session rebuilds;
windows persist via their redirection surfaces); driver hangs are
the WDDM TDR story (timeout detection and recovery resets the
driver, the screen blinks, the desktop survives). The two-layer
failure model — compositor restartable, driver resettable — is
the design's quiet achievement.

**macOS.** An application crash is invisible (server-owned state
survives); a WindowServer crash logs the user out. There is no
compositor restart — the concentration that gives the process its
coherence gives it its blast radius.

**LDP.** The discipline is the tested one: the crash corpus kills
sessions mid-protocol and asserts honest teardown (no FD leaks, no
half-committed state), the server suite kills it mid-dispatch, and
the bridge suites prove foreign-protocol sessions die honestly
too. And since v0.13, the two-layer model itself is served:
`lion-supervisor` owns the session identity (the pinned abstract
socket) and the rebuild policy, treating the compositor as the
replaceable half — the Windows split at the driver boundary, here
at the process boundary. A `kill -9` compositor death (the harsh
death — no teardown, no last words) backs off, re-execs on the
same pin, and a reconnecting client presents again: the session
flashes and rebuilds. The clients own their buffers by protocol
doctrine, so the survivors of a compositor death are the clients
themselves — the DWM's redirection-surface story translated to a
protocol where the client was always the owner. Proven over real
processes in CI (`tests/rebuild_session.rs`), with the graceful
operator stop (SIGTERM forwarded, honest child teardown) and the
crash-loop bound the DWM never documented.

## Stage 10 — remote display

**Windows** remotes the *graphics pipeline*: RDP carries drawing
commands and bitmap diffs (modern RDP, the AVC444/H.264 path,
carries compressed frames), and the server's composition survives
the wire — a remote Windows session is a first-class Windows
session, which no other system matches at this maturity.

**macOS** remotes pixels: Screen Sharing (a VNC core with Apple's
additions, plus Virtual Display for a headless second monitor). It
is the oldest doctrine of the three and the least ambitious.

**LDP** remotes the protocol: the full LDP surface over loopback
TCP through two gateways, FD vocabulary included (memfd pools,
eventfd fences), with **pixel-exact scanout proven in CI** — the
remote session is not a projection, it is the same pixels. The
gaps are named: bearer tokens today, TLS and multi-user identity
on the roadmap line.

## The honest ledger

**What the giants have that this repository does not.** Decades of
per-vendor, per-panel quirk tables — NVIDIA's and AMD's range
lists, the EDID catalogs, the driver certification programs —
depth that cannot be simulated, only accrued (the quirk ledger's
structure and first rows are in; the decades are not). The
application ecosystems: every toolkit, every browser, every GPU
vendor targets their APIs; none targets LDP natively (the X11
bridge is the door, and it is open, but the room beyond belongs
to the giants). Compute-armed compositors — shader-heavy effects
at GPU speed on modern APIs (LDP's GLES 2.0 arm is honest about
being 2007's baseline with byte-equality as its discipline; the
Vulkan/compute arm is the named seam). Measured latency labs with
real panels and oscilloscopes. And the sheer mass of shipped
hardware — the mechanism set matches; the mileage cannot.

**What this repository has that the giants do not.** A readable,
auditable compositor: DWM's blend math and WindowServer's shadow
policies are closed to everyone outside their companies — here
every policy is source, every claim a test, every number a
reproducible measurement. **2,121 green tests** with byte-exact
pixel oracles across two renderer backends, command-stream goldens,
a per-pixel damage corpus, and fuzz corpora clean at a 20× soak
(the v0.12 soak found and fixed a latent harness panic the
default budget never reached — the discipline working as
designed). A machine-frozen protocol contract — the wire surface
cannot silently move, which neither giant can even promise
(their private interfaces are theirs to churn). The no-GPU path
as a first-class citizen (Windows requires a D3D class — WARP,
its CPU rasterizer, exists but is nobody's idea of a first-class
path; macOS requires an Apple GPU class; LDP's software backend
*holds 60 Hz at every desktop size* and refuses llvmpipe by
policy). Exact input coalescing at per-contact granularity, proven
over a real socket. And the honest documents themselves: a
comparison file that names its own arithmetic slips and corrects
them in place — a self-criticism discipline the giants' marketing
adjacent documentation does not attempt.

## The stage verdicts

| Stage | Windows | macOS | LDP | The reasoning, one line each |
|---|---|---|---|---|
| Client buffers | tie (two doctrines, forever) | tie (one doctrine, server-owned) | tie (one doctrine, symmetric, honest refusals) | All three reach zero-copy; only one documents the refusals. |
| Compositor authority | driver-split, layered | concentrated, coherent | concentrated + permissioned + audited | LDP prices the giants' certification apparatus as the missing half. |
| Change tracking | 1985 + retained-tree dual model | retained-tree only | exact algebra, corpus-proven | Only LDP's damage claim is machine-checked; only LDP's quiet frame is measured (−52.9%). |
| The composite | MPO/Direct Flip mature, closed policies | Metal, closed policies | plane solver, zero-composite, byte-equal, open | The mechanism set matches; the vendor decades and compute arm don't. |
| Presentation feedback | deepest public API | display-link only | protocol-surface verdicts, VRR-paced | Windows' lead is real; LDP's verdict fidelity is the differentiator. |
| Input | kernel-owned, robust, coalesced | server-routed, coherent | protocol-owned, per-contact and per-axis exact | Per-contact, per-axis coalescing is the newest mechanism of the three. |
| Power | vendor-owned levers | occlusion-informed + ProMotion | measured PSR/governor/ledger + occlusion quiescing | The structure is all here; real-panel rows accrue. |
| HDR/VRR | deepest quirk depth, mode-switch legacy | EDR elegance, narrow reach | honest peak + VRR ledger, no infoframe yet | EDR and the quirk decades are the honest remainders. |
| Failure recovery | two-layer, recoverable | hard blast radius | tested teardown + supervisor served | The DWM restart choreography is LDP's own since v0.13 — kill -9 proven over real processes. |
| Remote | pipeline remoting (RDP), mature | pixels (VNC-derived) | protocol remoting, pixel-exact | Three doctrines; only LDP's is provable pixel-exact in CI. |

Read the table exactly one way: **on architecture, the pre-1.0
codebase is already in the conversation** — every mechanism the
giants' pipelines are built from has an open, tested counterpart
here, usually with an exactness proof the closed systems cannot
offer. What it lacks is bought only with time: hardware mileage,
quirk-table depth, and the ecosystem. Those are the adoption-view
fields of the matrix, and the matrix prices them honestly.
