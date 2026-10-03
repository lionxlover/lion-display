# LDP vs. the field — the honest comparison

*Phase 45 (v0.13.0), the nap/claim/rebuild edition — the direct
successor of the quiet frame edition, closing four of the named
lines the earlier editions priced (occlusion quiescing, the
per-surface material request, the session-rebuild supervisor, the
axis-metadata coalescing), and now
two documents where one used to be: this matrix scores the
*outcomes*, and its new companion,
[`comparison-windows-macos.md`](comparison-windows-macos.md), walks
the *architectures* — Windows' DWM/WDDM stack and macOS'
WindowServer/CoreAnimation stack beside this one, stage by stage,
from the client's pixels to the lit panel, with an honest ledger of
what only time buys and what only this repository has. The
v0.12.0 release behind this edition is measured the same way the
matrix's numbers always are: the quiet 64-window damage pass
**−52.9%** and the one-moving-window load **−39.2%** against the
pristine v0.11.0 engine on identical input, the fuzz gate clean at
a 20× soak that found and fixed a latent harness panic the default
budget never reached, and 2,121 tests. The display-size row's own
named remainder ("the giants keep their 90s on decades of
EDID/timing quirk coverage — that depth cannot be simulated") is
answered with the timing machine itself: the VESA reduced-blanking
synthesis (any size, integers-only, the clock never under the ask),
the EDID timing parse (the preferred timing and the declared
envelope), the pour as a user-defined mode over the whole wire —
and the quirk trio that makes the diagnoses escapable. This
edition carries Phase 51 (v0.19.0): the focus and the view — the
states arm's follow-ons, part one (the frozen `activated` state
bit riding real proposals through every keyboard transition — the
key-window doctrine the desktop matrix always assumed now served
on the wire; the operator-side space switching `shell.
switch_workspace`, the taskbar's line the ledger named, with the
clamp-honest broadcast and the space-preferred focus), and the
gap-filling program's first release under the slowly-slowly
discipline: one gap per phase, one phase per version. This
document is the answer to
"how does this compare?" asked seriously: field by field, against
every display server that matters, with scores out of 100 and the
reasoning attached. The policy is the project's standing rule —
**honest above flattering**: every number cited is measured on this
repository's own
benchmark rig (2 vCPU, `docs/benchmarks.md`), every gap the giants
close is named with the roadmap line that closes it here, and nothing
is scored on promises.*

## The contenders

| | What it is | Lineage |
|---|---|---|
| **lion-display** | The LDP display server: 26 ldp crates, one protocol, two renderer backends, a hardware plane engine, an in-tree compatibility bridge (rootless: every X11 window a first-class window), a sleeping-panel power path, the honest-peak HDR negotiation, the fresh-frame latency path (position-state coalescing + the VRR-aware presentation clock), the deep material family (the vibrant frost, the edge light), the VRR quirk ledger (the honest floor, the LFC latch, the sibling escape), the mode foundry (VESA reduced-blanking synthesis, the EDID timing parse, the user-mode pour), the per-contact coalescer (the touch/tablet position-state doctrine), the living registry (dynamic globals — the withdrawal and re-advertisement the display set's own truth rides), the quiet-frame damage engine (the steady-state pass halved, A/B-measured), the occlusion quiescer (the App-Nap doctrine — parked frame requests for fully-occluded windows), the per-surface material request (`toplevel.set_material`), the session-rebuild supervisor (the DWM doctrine, kill -9 proven over real processes), the axis-metadata coalescer (shape/orientation as per-contact position state), the semantic scene (the identity claim triple `set_semantic_role`/`set_security_class`/`set_scene_profile`, the security-aware capture redaction, the per-surface budget floor, the composition decision engine, the compositor-owned transitions catalog), 2,241 green tests | This repository, v0.19.0 |
| **Wayland** | The protocol + its compositors (GNOME Mutter, KDE KWin, wlroots/sway, Weston) | 2008→, production since ~2014 |
| **X11 / Xorg** | The X server + DDX drivers + compositors (picom/xcompmgr era) | 1987→, Xorg 2004→ |
| **macOS WindowServer** | Apple's Quartz-based compositor (WindowServer + SkyLight/CoreAnimation) | 1989 (NeXT)→, macOS lineage |
| **Windows DWM** | The Desktop Window Manager (Vista 2006 → Windows 11) | 2006→, in-kernel + user-mode |

Scoring the *platform giants* is scoring decades of shipped hardware
support against a pre-1.0 codebase — the scores below reflect **what
each system delivers today**, and lion-display's pre-1.0 status is
priced in (see *Ecosystem*, *Deployment maturity*).

## The matrix

| Field | lion-display | Wayland | X11 | macOS | Windows | The reasoning |
|---|---|---|---|---|---|---|
| **Codebase & memory safety** | **95** | 75 | 45 | n/a | n/a | lion-display: Rust end-to-end, `unsafe` confined to four audited FFI modules, `forbid(unsafe_code)` everywhere else. Wayland compositors: mixed C (wlroots) / C++ (KWin) / Rust-curious (Mutter is C). X11: C, ~1.2 MLoC with CVE history. The giants' compositors are closed — scored n/a where unmeasurable, never guessed. |
| **Idle memory footprint** | **88** | 70 | 55 | 30 | 40 | Measured fresh by the deep-test audit (VmHWM, release build): **69 MiB peak RSS** for the v0.10.9 default dual-1080p headless bring-up with the desktop shell's dock rendered, **60 MiB with `--dock off`** — and v0.11.0's release re-measures **56 MiB (57.5 MB) on the same procedure, an A/B against the pristine v0.10.9 build on the same rig measuring 72.6 MiB: −22.6%**, the delivery path's per-frame full-canvas allocations gone (the frame delivery is a borrow, the display model's Vec is allocation-stable, the transport writer compacts its dead prefix instead of growing). The double-buffered scanout pair, the PSR shadow, and the shell's material caches still dominate; the code's own share stays single-digit MiB. This row previously carried **19 MiB**, the Phase-30 measurement — stale: it predates the double-buffered scanout, the PSR shadow, and the positioning shell, and the audit corrected it here rather than repeat it (95 → 85 then; 85 → 88 on the v0.11.0 A/B: Weston-class, still a third of Mutter's floor). Typical: Mutter 150–300 MiB, Weston ~40–80 MiB, Xorg 50–100 MiB + compositor. Apple/Microsoft numbers are not published; scored on the conservative side of public measurements. |
| **Low-end CPU performance** | **90** | 60 | 55 | n/a | n/a | The Phase 29/30 doctrine, measured: the full Liquid scene (rounded+shadowed windows, frosted glass) at **8.9 ms/frame steady at 1080×2340** (60 Hz-capable on a weak CPU), **7.31 ms at 1080p / 11.49 ms at 1440p / 15.11 ms at 21:9** desktop frames at the *High* tier, all on 2 vCPUs with byte-exact oracles. No other compositor publishes cold-start-to-60 Hz numbers for CPU-only paths; Wayland compositors generally require GL to be usable at all (llvmpipe fallback is famously slow). |
| **GPU compositing** | **94** | 90 | 50 | **95** | **95** | v0.10.1 lands the hardware compositing path whole: the **plane-assignment engine** (`ldp-planes`) maps the layer stack onto the CRTC's plane set — the split-point doctrine, zpos-monotone backtracking search, a named demotion per composite layer (the no-silent-degrade rule applied to offload); the **zero-composite frame** (a fullscreen opaque client's own framebuffer on the panel, the renderer emitting *no pass at all*) and the **underlay split** (canvas on the primary, windows on overlays above) are proven end-to-end through the real wire on the mock's real plane inventory; the **import walk** (pool fd → `drmPrimeFDToHandle` → `AddFB2WithModifiers`) registers client buffers as kernel framebuffers with cached honest refusals; YUV and scaled GL uploads land byte-equal to the software path; dma-buf **feedback tranches** resolve the allocation advice; the display model keeps every pixel oracle meaningful across all three frame shapes. Still GLES 2.0 with no Vulkan/compute arm and no per-vendor quirk decades — the named lines the reserve phases close. The giants keep their 95s on quirk-table depth; the mechanism set now matches theirs. |
| **No-GPU machines** | **95** | 40 | 70 | 20 | 20 | The doctrine pinned in Phase 30: `--renderer software` is a first-class backend (not a fallback of shame), `auto` **refuses llvmpipe** (a CPU GL stack is slower than the specialized word-blending backend — the reason carried in the startup line), and the tier auto-scales to hold 60 Hz at every desktop size. X11 without a GL stack was always fine (it never needed one); Wayland stacks range from "works on llvmpipe, slowly" to "requires hardware." Apple/Microsoft: the OS simply requires a GPU class. |
| **Visual quality ceiling** | **90** | 75 | 30 | **95** | 90 | The v0.10.7 change, and the materials distance is closed: Phase 27's Liquid engine served macOS-class materials — true rounded corners with fractional coverage, soft shadows as memoized imagery, 3-pass frosted glass, spring motion — **byte-equal across software and GL backends**, tiered so low-end devices keep them; v0.10.0 folded the HDR pipeline into the ceiling (scene-linear blending, PQ tail, BT.2408 reference white). v0.10.7 adds the depth the giants kept: **the vibrant domain** — the frost's saturation dial now extends *past* the backdrop's own chroma (`0..=510`, integer-exact, the acrylic/vibrant distance where Windows' acrylic and macOS's vibrant materials live), the **edge light** — the luminous 1-px hairline traced inside every rounded silhouette on the same antialiased coverage curve the corners ride (the stroke that makes glass read as glass), and the **material family** — one named vocabulary (panel, sheet, menu, vibrant-dark, chrome) the whole desktop speaks: menus and popups wear the menu glass (backdrop blur *everywhere* the giants mean it), the dock traces the chrome hairline, and every material resolves per tier so the weakest device keeps the identity. All of it **byte-equal across the two backends by construction** (the GL stream draws the identical memoized ring words) and pinned by hand-computed oracles plus an end-to-end session over the real wire. What keeps this a tie with Windows and not a lead over the field: WindowServer remains the reference ceiling — its materials take per-app requests through a mature API surface, and its design tuning rides decades of shipping polish; per-surface material requests over the shell protocol is the named roadmap line. Wayland compositors blur well on GL but vary wildly by compositor; X11's picom-era blur is a hack on a hack. |
| **Every display size** | **90** | 80 | 75 | 90 | 90 | Phase 30 benchmarked the desktop matrix (FHD → QHD → 21:9 ultrawide → 4K) with `--resolution WxH` forcing any offered mode and an honest menu on a miss. v0.10.0 made the fractional-scale doctrine true: the operator's `--scale F` advertises Q8.8 on every output, the positioning shell resolves layout on the *logical* canvas (a 1080p panel at 2x serves a 960×540 phone-density desktop), placement lands in physical pixels, and clients learn the rendering truth at bind and at `enter_output`. v0.10.9 answers the row's own named remainder — "decades of EDID/timing quirk coverage" — with **the mode foundry**: `--synth WxH@Hz` pours a VESA CVT reduced-blanking timing for *any* size (the arithmetic `xrandr --newmode` and Windows' custom-resolution form run on — integers-only, the 460 µs vertical-blanking minimum solved exactly, the pixel clock ceil-targeted so the realized refresh is never under the ask, the canonical `cvt -r 1920x1080` anchor's 2080×1111 totals reproduced bit-exactly), riding the wire as a **user-defined mode** the display engine validates at commit and the protocol face advertises (`xrandr --addmode`'s story, protocol-side — a bound client sees the pour among the firmware's modes, flagged current). The EDID parse grew its timing half: the detailed timing descriptors (the sink's *preferred* timing, marked like the kernel marks it) and the range-limits envelope — the declared pixel-clock ceiling gating every pour with the typed refusal naming it, never a silent clamp. The hotplug migration re-pours on the next display and falls back past a refusing ceiling honestly, and the bring-up's EDID audit diagnoses the connector's truth (a rotten block, a preferred-timing lie, a ceiling the list exceeds) with the quirk trio's escapes — eight rows tabled. What keeps this a tie with the giants rather than a lead: their quirk tables carry *decades* of per-panel EDID rows where the audit names the three classes a deterministic rig can prove; the GTF/CVT standard-blanking families for legacy analog sinks stay roadmap lines. 90 is "any size serves, synthesized per the VESA doctrine, both gates honest." |
| **Multi-monitor / topology** | **95** | 90 | 85 | **95** | **95** | The v0.10.4 change: **every arrangement a desk can ask for**. The v0.10.0 doctrine serves the extended desktop — every pipeline the allocator finds, left-to-right, per-output visibility, topology motion that survives mid-session, release gates that span the seam. v0.10.4 adds the rest of the vocabulary: **`--outputs mirror`** places every display on the *same* desktop (the overlapping origins are the protocol's own clone signal, the vocabulary X11 and the Wayland compositors serve), each display cropping the desktop to its own mode — proven *per-pixel* (a 720p display's every row equals the primary's first 1280 columns; two same-size displays' scanouts byte-equal word-for-word), with a display joining mid-session joining the mirror, never an extension. **Per-output scales** (`--scale 1,2`): one factor per output in output order, the shell resolving its logical canvas on the primary's own factor, a hotplug newcomer inheriting the doctrine's last entry (the stretch rule). And the **quirk table** — the mechanism the giants' decades ride: named rows, each with its symptom, its detection, and its operator escape (`psr-flicker` → `--no-psr`, `vrr-flicker` → serve without `--vrr`). What keeps this a tie with the giants rather than a lead: the quirk table's rows are five deep (v0.10.8's ledger accrues the VRR pair) where theirs are decades deep — the mechanism is here, the rows accrue with hardware. 95 is "every arrangement served and proven per-pixel; the quirk depth is the honest remainder." |
| **VRR (adaptive sync)** | **85** | **85** | 60 | 55 | 85 | The v0.10.8 change, and the last named *mechanism* distance closes: the quirk ledger. v0.10.0 armed it (every VRR-capable output's flips carry `VRR_ENABLED`, the ldp-vrr deadline policy widens the scheduler's window to the CRTC's range, the LFC ruling repeats the front buffer in-window); v0.10.6's presentation clock made the pacing honest (the panel-window measurement band, the adaptive opportunity target with the verdict at the content's own flip — a serving fidelity none of the field exposes at this exactness). v0.10.8 answers the row's own named remainder — "the multi-year driver quirk table, not the mechanism" — with the *structure* the decades fill: **the honest floor** (`--vrr-floor N`, per-output CSV, the `--scale` grammar's mirror): panels whose advertised range flickers at the bottom get the operator's truth clamped into every window consumer (the scheduler's widened deadline, the `output.vrr` advertisement clients pace against, the LFC cadence) while the device's own claim stands untouched — the same doctrine as `--hdr-peak`, and the same honesty: an unservable floor refuses boot naming the mistake; **the LFC cadence and latch**: content slower than the window bridges on repeats locked to the content's own phase (`k = ceil(P/max)`, judder-free by construction — the giants' low-framerate-compensation arithmetic), and the anti-flap latch keeps the repeat regime warm through boundary-hugging flips (the visible pumping the driver tables name `lfc-flap`); and **the sibling escape** (`--vrr-uniform`): a mixed desktop — one VRR panel, one fixed — collapses to one uniform fixed sync for the platforms whose cross-CRTC clock coupling flickers the fixed sibling (the pre-2020 driver doctrine, served as the escape while per-output VRR stays the default — the modern per-display behavior). All pinned end-to-end over the real socket against the mock panel's own advertised 48-144 Hz window: the floor's clamp on the wire event, the scheduler's narrowed widening, the passthrough per-output entry, the refusal, the collapse's disarming, and the two composed. What keeps this a tie with Wayland and Windows rather than a lead: the giants' quirk tables carry *decades* of per-vendor rows (NVIDIA's per-monitor range lists, AMD's EDID tables) where ours carries the mechanism and the first five rows — the ledger accrues with hardware; macOS's 55 reflects ProMotion's narrow, laptop-only reach. |
| **HDR** | **90** | 70 | 10 | **90** | 85 | The v0.10.5 change, and the field's mechanism set is now complete: **the honest peak**. The v0.10.0 pipeline advertised the panel's truth (`output.hdr_caps`), folded the desktop's stack into the mode controller's hysteresis, and blended every layer scene-linear with PQ encoding on the tail (SDR at the BT.2408 reference white) — but the advertisement and the ink never met: the canvas was a static description, and per-surface metadata fed nothing but the mode vote. v0.10.5 closes exactly the two remainders the last edition named. **Per-surface luminance metadata refinement**: every HDR layer's declared mastering bounds (`set_hdr_metadata`) now ride the BT.2390-structured knee onto the *negotiated* ceiling — the system's rolloff replaces the panel's hard clip, hue preserved, the identity segment below the knee byte-pinned. **Real-panel negotiation**: the panel's *effective* peak (the advertisement, `--hdr-peak`-capped for the bloated-EDID class every real HDR panel carries), the negotiated canvas ceiling (the stack's brightest content clamped to the panel — dimmer content rides at its own level), and the reply side (`hdr_caps` advertises what the render pass will actually deliver) — one truth, both directions, with the quirk table's first accrued row (`hdr-peak-bloat`) naming the escape. Proven in the scanout itself: the same 10,000-nit code value renders as different honest ink on a 600-nit panel, a 400-nit panel, and under the operator's cap. Beneath it the Phase 24 color spine: PQ/HLG transfers, BT.2020 primaries, per-output `ColorDescription`, ICC v2/v4 import, tone-map operators — all oracle-proven. What keeps this a tie with macOS rather than a lead: the DRM-side infoframe emission and ST 2094-40 dynamic metadata are named roadmap lines, and the giants carry real-panel measurement campaigns we simulate honestly. X11 scored 10 because it predates HDR entirely; Wayland's color-management protocol is still landing across compositors. |
| **Input latency & frame pacing** | **93** | 85 | 60 | **90** | 90 | v0.10.6 serves the giants' tuning, doctrine by doctrine, each pinned in CI. **Event coalescing** — the tuning X11's MotionNotify and Windows' message loop perfected over decades — is now the served emission path (§10.4's table grows the position-state row): a 1000 Hz device parking sixteen samples between display frames delivers ONE `pointer.motion` carrying the freshest coordinates at the oldest pending slot (a discrete event is never pre-empted by a newer sample), every `relative_motion` delta intact for raw-input consumers, one `frame` terminator — one wake per display frame, proven over the real socket; the discrete barrier seals the click's own sample (the X11 flush-before-button doctrine). **The VRR-aware presentation clock**: the measured band is the panel's own probed window (the LFC fast end honestly reported at 6.944 ms in CI — the legacy band rejected it as a duplicate), and an adaptive surface's verdict fires at the flip that carries its content, the deadlines pacing at the fast end while the client keeps up (the frame pacing follows the content — a feedback fidelity none of the field exposes at this exactness). The spine all along: the phase-locked deadline grid (deterministic in CI), the input→photon budget asserted under the flood (within one nominal period + costs, `last_input_photon_ns`), direct scanout candidacy (the honest subtraction). v0.11.0 closes the row's own named remainder — **the per-contact coalescer** (Phase 43): the doctrine extends from the pointer's single stream to every contact-carrying stream, per contact, per axis — a 120 Hz touchscreen parking sixteen two-finger batches between display frames delivers TWO motions (each finger's own freshest sample in its own slot, one shared terminator, the downs intact), a 240 Hz pen stream collapses to one motion, one pressure, one tilt per frame, and the discrete barrier seals a contact's own stream (a finger's down never freezes another finger's sample) — proven at all three tiers, the session proof over a real socket. What keeps this at 93 and not higher: the giants' 90s ride years of *measured hardware* latency tuning — unredirection verified on real panels with oscilloscopes — where ours is the architecture with mock-proven budgets; the real-panel latency lab stays the named roadmap line. |
| **Security model** | **90** | 65 | 20 | 85 | 80 | Every operation crosses a permission matrix (18 operations, decision ~0.23 ns — re-measured fresh for this edition), sessions carry tokens verified in constant time, the audit chain is hash-linked and verify-walked, and the threat model is a living document with every row tested. The v0.10.0 bridge carries the same doctrine: every foreign session rides its own LDP client identity and the bridge process refuses to run without a bridge-scope token. Wayland's model is "compositor decides" (good) but flat — one sandbox leak is the whole session; X11's is famously absent (any client can keylog the world). macOS/Windows sandbox + per-app privilege are strong but opaque. |
| **Remote display** | **90** | 45 | 75 | 60 | 65 | The remote relay is first-class: the full protocol over loopback TCP through two gateways with **pixel-exact scanout** proven in CI, FD vocabulary (memfd pools, eventfd fences) relayed, wrong-token fast-fail, dead-peer teardown, FD-leak lifecycle gates. X11's network transparency is the classic (and its doom — unauthenticated, unencrypted, per-message round trips). Wayland deliberately dropped network; each desktop ships its own separate VNC/RDP island. The gap priced in: bearer-token auth today, TLS and multi-user identity are roadmap lines. |
| **Protocol design** | **94** | 80 | 40 | n/a | n/a | One spec, one compiler (`ldpc`), generated-code drift gates, versioned negotiation proven to grow (capture joined after v1.0 — through the whole pipeline). Object model with capability revocation. v0.10.0 adds two things the field cannot match: the **frozen v1 API contract enforced by a gate** (`ldpc snapshot --check` — the frozen surface must match the compiled spec byte-for-byte; a moved name, opcode, argument, or version breaks the build until the change is consciously re-frozen), and the served clipboard data family (`ldp.data.data_device_manager`) — devices, sources, offers, selection, and byte-exact pipe transfers. v0.11.0 adds the **living registry** (Phase 43, the roadmap's own "dynamic globals" line): the advertisement set is no longer a static replay that goes stale — the dark state withdraws the output global (every live object of the interface revoked first, `interface_removed`, then the zero-argument `global_remove` barrier on every live registry object — the spec's own ordering sentence, served) and the relight re-advertises it with the schema's own version range; the mechanism rides a new default-no-op `Dispatcher::on_registry` seam, zero wire-surface change, the freeze gate still green. v0.19.0 takes the freeze a fourth conscious re-take (the Phase 45/47/50 doctrine now routine: `shell.switch_workspace` + `workspace_switched` appended after the frozen opcodes — 98 requests + 123 events — with the grace-parking state proposal carrying the frozen `activated` bit, the machine-level doctrine that a server-driven focus transition never punishes a client draining a drag's serials). The drift-fix that shipped with it: the client's `GlobalRemove` decoder now matches the frozen zero-arg shape it always should have. Wayland's protocol is good but XML+ handwritten bindings across compositors with documented divergence — and `wl_registry`'s removal semantics arrived piecemeal across compositors; X11's wire format is 1987's taxonomy. Apple/Microsoft protocols are private — not scored. |
| **API stability for apps** | 70 | 80 | **95** | 90 | 90 | The v1 API contract is frozen and machine-enforced — the freeze gate runs in `verify.sh`, so the wire surface cannot silently move. A 1987 X11 client still links (that's the 95, earned by 39 years); Wayland's 80 is shipped-everywhere churn-managed. lion-display's 70 is frozen-in-principle but ten months old — nobody's payroll depends on it yet, and only time moves that number. |
| **Ecosystem & compatibility** | **85** | **90** | **95** | 95 | 95 | The v0.10.3 change, and the biggest single movement in this table's history: **the rootless door**. An X11 application's windows are now windows — the bridge exports every top-level X window on its own LDP surface (the positioning shell places them, the Liquid materials dress them), override-redirect windows ride the popup role's anchor/gravity machine, input arrives with the two coordinate truths bridged by construction, and the whole thing is proven through a real Unix socket (three windows at fictional X geometries, pixel-exact, the damage isolated, the teardown honest). The popup role is a *native* protocol capability with its own session proof — any LDP client can anchor menus, not just the bridge. The rootful whole-screen mode remains as `--x11-rootful`. Wayland windows still map one-for-one through the Wayland face; the clipboard family still translates foreign selections. What keeps the field under the giants' 95s is the honest remainder: no browser, no toolkit, and no vendor driver targets LDP natively — the door is now a *gate* (X11 apps are first-class citizens, not a projection), but the room beyond it still belongs to the giants' decades. 85 is "the X11 world runs here as windows," which is the most a compatibility layer can honestly claim. |
| **Determinism & test discipline** | **98** | 70 | 50 | n/a | n/a | 2,121 tests (220 result suites, 0 failed — the count grown with the quiet-frame equivalence proofs and the mutator-edge regressions), all green on every commit and again from a fresh extraction; **byte-exact pixel oracles** across software/GL backends; **command-stream goldens** pinning exactly what the GL renderer emits; golden-image suites; 2,000-case fuzz corpora **clean at a 20× soak — the soak that found the transport mutator's one-byte `chunk_stream` panic and the empty-source splice sibling, both fixed with regression tests the same release**; spec↔code drift gates **and the API-freeze gate**; no-clock rendering (pure function of output × damage × layers); a mock KMS device that makes the *serve loop itself* CI-provable; the bridge's foreign sockets exercised end-to-end in CI. No display server in history ships this discipline — most can't (their state spaces are decades of accumulated hardware behavior). |
| **Portability** | 80 | **90** | 85 | 15 | 15 | Rust + `dlopen`-late-binding for EGL/libdrm: builds anywhere with a C toolchain, runs on any Linux from phones to servers, WASM-tested core crates. Wayland is portable across the free Unixes; X11 went everywhere including VMS. WindowServer/DWM are their platforms, period. |
| **Documentation** | **90** | 75 | 65 | 40 | 45 | Spec + compiler + user guide + admin guide + threat model + benchmark ledger + this file + the pipeline-depth architecture comparison (`comparison-windows-macos.md`), all versioned with the code and gated on drift. Wayland's docs are scattered but deep; X11's are archaeology; the giants' are marketing-adjacent and NDA-flavored. |
| **Power efficiency** | **90** | 80 | 45 | **90** | 85 | v0.10.2 lands the static-frame power path, and the number is *measured*, not asserted: **panel self-refresh** — the per-output machine that earns entry through consecutive quiet flip opportunities and names every exit (`damage`, `blank`, `backlight`, `unsupported`), the frozen timeline (ten seconds of a still desktop cross the device with zero vblanks — CI-proven), and the one-nominal rescan cost honestly paid on every exit; the **GPU clock governor** (asymmetric hysteresis over the landed-flip load — up instant, down patient, the DVFS seam); the **energy ledger** (a documented parametric cost model whose static-scene ratio is arithmetic the session tests reproduce — the sleeping desktop draws ~13% of the always-scanning counterfactual, prefix and wakes included); plus the idle ladder (with the production bug the audit caught: the ladder never advanced on a truly idle real machine — no DRM events meant no ticks), the zero-composite frame (v0.10.1 — a fullscreen client's own buffer on the panel, no GPU pass), and `--no-psr` as the honest escape for flickering panels. macOS's 90 rides the same features on real panels with a decade of quirk tables; ours is the full architecture with the property vocabulary for real eDP and the quirk table as the roadmap line. |
| **Future-proofing** | **85** | 70 | 30 | 75 | 75 | The seams are the argument: `Renderer` trait with two backends behind one selection policy (Vulkan slots in without touching the frame loop), `GlesApi` object-safe and injectable (the byte-equal oracle), `DisplayDriver` over mock and real KMS, transport as a trait, GPU-class probing that meets machines that don't exist yet as `Unknown`-but-hardware (never a silent degrade) — and now a frozen contract with versioned negotiation proven to grow. X11's future is maintenance mode; the giants' futures are closed questions. |

## The totals (unweighted, honest)

| | lion-display | Wayland | X11 | macOS | Windows |
|---|---|---|---|---|---|
| Sum over the 21 fields | **1,877** | 1,565 | 1,195 | 1,210* | 1,240* |

(\* The closed giants are scored on 17 of 21 fields — four fields are
unmeasurable on their codebases and are never guessed. The sums are a
straight addition of the matrix rows; the v0.9.0 edition of this file
carried an arithmetic slip in its stated totals that the TDR-003
report disclosed and this edition corrects in place — and the
v0.10.3 edition's stated adoption-view 77.1 divided the ecosystem
movement over eleven fields while the view averages nine; the
series below is recomputed straight from the rows.)

Read this exactly one way: **on the fields a display server's own
engineering controls** — safety, memory, low-end speed, the no-GPU
path, security, remote display, protocol design, determinism,
documentation, future-proofing — the v0.11.0 codebase averages
**91.5** and leads the field, because those fields are *this
project's whole thesis* (the deep-test audit edition re-ran the
measured spine fresh on the same rig the same day: every row within
±8% of the committed ledger — 7.12 ms at 1080p High, 15.15 ms at
the 21:9, 23.10 ms at 4K High — and the one claim that did not
reproduce, the 19 MiB memory floor, was corrected above, its −10
honestly paid in this average). On the fields that only time and
adoption can buy
(ecosystem, API stability, the decades of hardware quirk coverage
hiding inside every giant), v0.10.0 closed five of the named gaps —
the multi-output doctrine, the fractional scale, the HDR pipeline,
the frozen contract, the in-tree bridge — moving the adoption-view
average from 61.7 to 73.9; v0.10.1's plane engine carried it to
76.3, v0.10.2's sleeping panel to 78.0, v0.10.3's rootless door to
81.9, v0.10.4's every-arrangement desktop to 83.8, and v0.10.5's
honest peak to 85.4, and v0.10.6's fresh frame to 87.1 (the deep-test
audit recomputed this series straight from the rows) — the distance
to Windows' 90.0 fallen from 28.3 points to 2.9 across six releases,
without spending a point of the engineering margin. v0.10.7's +8
lands where the views keep it honest: the visual-quality field sits
*outside* both averages (it is neither engineering-controlled nor
adoption-bought — it is the materials axis, priced on its own row),
so the deep material lifts the field 82 → 90 and the straight total
1,856 → 1,864 without moving either view's arithmetic. v0.10.8's
quirk ledger moves the view again — VRR sits inside it, and the
field's own named remainder (the multi-year driver quirk table)
answered with the ledger's structure and first five rows lifts the
field 82 → 85 and the adoption view 87.1 → 87.4 (the distance to
Windows' 90.0 down to 2.6), the straight total 1,864 → 1,867.
v0.10.9's mode foundry closes the display-size row's own named
remainder — the "decades of EDID/timing quirk coverage" the field
kept its 88 for — with the synthesis machine, the timing parse,
and the pour over the whole wire: the field 88 → 90 (a tie with
macOS and Windows), the adoption view 87.4 → 87.6 (the distance
to Windows' 90.0 down to 2.4), the straight total 1,867 → 1,869.
v0.11.0's contact doctrine and living registry move three rows the
release genuinely earned: input latency 90 → 93 (the per-contact
coalescer, the row's own named remainder — now a lead over every
contender's 90), idle memory 85 → 88 (the A/B-measured −22.6% peak
RSS on the identical bring-up and client), protocol design 92 → 94
(the dynamic-globals line served) — the adoption view 87.6 → 88.1
(the distance to Windows' 90.0 down to 1.9), the straight total
1,869 → 1,877. v0.12.0 moves no row — it deepens the evidence the
rows already stand on: the determinism row's 20× soak (clean, with
the found-and-fixed harness panic as its proof the discipline
works), the low-end row's damage-pass A/B (the per-commit-batch tax
halved, −52.9% quiet / −39.2% moving), and the documentation row's
new companion (`comparison-windows-macos.md`, the architecture
answer at pipeline depth) — the totals stand at **1,877**, the
engineering average at **91.5**, the adoption view at **88.1**. A
weighted matrix that prizes "runs every app today" over "runs
correctly forever" scored us 50/100 on one line for three editions;
v0.10.3's rootless door raised it to 85 — the X11 world's applications
run here as first-class windows, which is the most a compatibility
strategy can honestly claim while no toolkit or browser targets LDP
natively. And on multi-monitor, the last field where a *mechanism*
gap (not a quirk-depth gap) kept the score down, v0.10.4 ties the
giants at 95.

## The measured spine

The numbers above stand on this repository's own rig (2 vCPU,
release builds, `ldp-bench`, `docs/benchmarks.md`):

| Scene | Steady state | 60 Hz verdict |
|---|---|---|
| 1080×2340 phone frame, High tier (Phase 29) | 8.86 ms | **holds 60 Hz on a weak CPU** |
| 1920×1080 desktop frame, High tier | 7.31 ms | **holds 60 Hz** (137 Hz-capable) |
| 1920×1080 desktop frame, no-GPU tier | 5.96 ms | **holds 60 Hz without any GPU** |
| 2560×1440 desktop frame, High tier | 11.49 ms | **holds 60 Hz** |
| 2560×1440 desktop frame, no-GPU tier | 7.95 ms | **holds 60 Hz without any GPU** |
| 3440×1440 ultrawide, High tier | 15.11 ms | **holds 60 Hz** (66 Hz-capable) |
| 3440×1440 ultrawide, no-GPU tier | 10.96 ms | **holds 60 Hz without any GPU** |
| 3840×2160 4K, High tier | 24.67 ms | the GPU's tier on CPU (the fix below took it from 194 ms); the GL backend serves it outright |
| 3840×2160 4K, no-GPU tier | 18.99 ms | 52 Hz at the full-damage worst case; a real desktop's localized damage sits far under budget |
| 4K XRGB word-copy composite (Phase 8 row) | 4.99 ms | the raw copy floor under everything |
| damage: quiet 64-window desktop pass (Phase 44 row) | **44,609 ns/op (−52.9% A/B** vs pristine v0.11.0's 94,720, 7-run medians) | the quiet-frame fast paths — a static desktop's commit-batch tax halved |
| damage: one moving window among 64 (Phase 44 row) | **77,199 ns/op (−39.2% A/B** vs 126,906) | the cursor-drag class — one mover's exact algebra, 63 spectators free |
| Full bring-up, peak RSS (fresh audit) | **69 MiB** v0.10.9 default boot / **56 MiB** v0.11.0, A/B-measured (−22.6%, same rig, same client, same procedure) | the memory floor, re-measured twice (the Phase-30 19 MiB row corrected once; v0.11.0's delivery-path economy measured against the pristine v0.10.9 build — see the idle-memory row) |

The 4K High row carries Phase 30's regression story: the first
full run measured **194 ms** — the shadow memo's fixed 6 Mi-word
budget thrashed under a 4K working set; the output-scaled budget
fixed it (**7.9×, same bytes**), and the flat-rebuild-counter oracle
keeps it fixed. v0.10.0's additions are correctness proofs rather
than new perf rows: the multi-output, scale, HDR, VRR, and idle
session tests; the bridge's end-to-end X11 session through a real
Unix socket; the command-stream goldens; and the freeze gate —
**2,121 tests**, every one green from a fresh extraction of the
release bundle.

*(Full-matrix medians are committed in `docs/benchmarks.md`; all
numbers are 5-run medians on the same 2-vCPU machine, release
builds.)*

## The named gaps (and their roadmap lines)

Every place a giant scores higher has a line in `docs/roadmap.md`.
The gaps v0.10.0 closed: several CRTCs for true multi-output (the
doctrine landed), fractional-scale negotiation beyond the fixed 1.0
(Q8.8 served), compositor-side HDR compositing and display peak
advertisement (the pipeline landed), DMA-BUF zero-copy upload (the
EGLImage arm), and the API freeze (the gate runs). v0.10.1 closed the
GPU-compositing set: the plane engine, direct scanout (the
zero-composite frame), the real import walk, YUV/scaled GL uploads,
and the feedback tranches (see `docs/gpu-compositing.md`). v0.10.2
closed the power set: per-CRTC PSR, the GPU clock governor, and the
energy ledger. v0.10.3 closed the rootless bridge split: per-window
X11 mapping (every top-level X window its own LDP surface,
override-redirect windows as popups, the two-truth coordinate
doctrine). v0.10.4 closed the multi-monitor arrangement set: mirror
mode, per-output scales, and the quirk-table mechanism. v0.10.5
closed the HDR negotiation set: per-surface luminance metadata
reaching the ink, the panel-peak negotiation vocabulary, and
`--hdr-peak` as the bloated-peak quirk's escape. v0.10.6 closed the
input-latency set: position-state coalescing in the served emission
path (the discrete barrier included) and the VRR-aware presentation
clock (the adaptive opportunity target — the pacing follows the
content). v0.11.0 closed the contact-doctrine and dynamic-globals
lines (the per-contact coalescer, the living registry). v0.13.0
closed four more: the per-surface material request
(`toplevel.set_material` — the NSVisualEffectView doctrine, the
freeze consciously re-taken), the occlusion quiescer (the App-Nap
doctrine: a fully-occluded window's frame requests park), the
session-rebuild supervisor (`lion-supervisor` — the DWM doctrine,
the failure-stage verdict's named line in
`comparison-windows-macos.md`, kill -9 proven over real processes),
and the axis-metadata coalescer (`touch.shape`/`touch.orientation`
as per-contact position state). v0.14.0 closed the foundry's own
named line whole: every timing family the VESA standards define —
CVT-RB2 (`:rb2`, the 80-pixel deep-color-era blank, 1-pixel
horizontal precision), the video-optimized 1000/1001 rate
(`:rb2v` — the 59.94 Hz class), CVT standard CRT blanking
(`:cvt`), and GTF (`:gtf`, the 1999 formula) — poured in exact
integer arithmetic, spec-faithful to the letter (the VESA CVT 1.2
and GTF 1.1 documents themselves fetched and followed formula by
formula). v0.15.0 closed the architecture's own identity line —
**the semantic scene**: surfaces claim what they *is* / may *expose* /
how the machine spends its frame budget on them
(`toplevel.set_semantic_role`/`set_security_class`/`set_scene_profile`,
appended additively with the freeze consciously re-taken); the
security-aware capture enforces the class ladder at the compositor's
own gate (protected/system surfaces redact out of every
client-visible frame while the display keeps showing them — the
capability model's `protected_surface` scope finally met by a
per-surface truth); the adaptive frame scheduler grew the per-surface
budget floor (gaming 1 ms, creative 4 ms — the creative-mode
doctrine the matrix's input-latency row has named since v0.10.6);
the composition decision engine names every frame's path with its
auditable cause; and the compositor-owned transitions catalog gives
every application the macOS-grammar window choreography
(`--transitions`, settling to byte-exact plain ink). Since v0.16.0
the frozen dialog surface is served too (the macOS sheet doctrine:
centered over the parent, the modal gate over the parent's whole
tree, the sheet-dies-with-window lifetime) and the choreography owns
the goodbye (the close fade over an owned snapshot at the window's
own stacking slot — both closed with zero wire surface moved). Since
v0.17.0 the toplevel states arm is served whole (the frozen
17-request vocabulary: the geometry verbs as the real two-phase
commit with the restore point; the visibility verbs with App-Nap
frame parking and byte-exact restoration; `workspace_count` at bind,
`set_workspace` with the clamped report — zero wire surface moved).
v0.21.0
closed the drawn chrome's first follow-on — the **server-side
caption drag** (and v0.22.0 the second: the **title glyph** —
Lion Sans, the compositor's own zero-dependency typeface with an
analytic-coverage rasterizer serving the band's text as its own
layer, pixel-true ellipsis truncation, the title change
repainting on the server's own claim — DWM's caption text and
WindowServer's title text answered with a face of our own, no
system font stack anywhere in the binary; and v0.23.0 the third:
the **Liquid chrome-material dressing** — the band wears the
chrome material, the dock's own, at the machine's effects tier:
the frost pane reading the backdrop beneath the veil, the chrome
hairline, the rounded frame, `Minimal` keeping the flat bar —
DWM's acrylic caption and WindowServer's vibrant title bar
answered with the system's own glass, zero wire surface moved; and
v0.24.0 the fourth: the **chrome-aware placement** — the placement
engine answers in frame space, every arm parking the *frame* (the
band and the border ring included) at the policy slot with the
content riding inside: a parked window's caption renders from row
0 of the display, the cascade steps *captions*, the physical
clamp holds the band's top edge at fractional scale factors, and
the chrome hit test went z-true (a higher window's band claims
above a lower window's ink — the press follows the visible ink) —
DWM's and WindowServer's frame-space placement answered with the
compositor's own geometry, zero wire surface moved; and v0.25.0 the
fifth: the **chrome ghost** — the close fade takes the whole window:
the ghost's capture freezes the dying band's shape truth and the
render path reads the same chrome cache the living desktop reads,
pushing the band, the strip, and the content at the ghost's own z
slot at the one close-spring opacity (the whole frame leaves as
one), the claims and the vacates grown by the frame, a Liquid
tier's frost still reading the canvas through the fade — DWM's
genie and WindowServer's zoom carrying the title bar with the
content, answered with the compositor's own choreography, zero
wire surface moved): the drawn title bar is a move grip, the server
minting the move drag itself at the pointer's first motion
through the same machinery `start_move` owns (one machinery, two
doors), the move server truth at the pump's cadence with zero
configures, the click doctrine, the close button's own grip never
converting, and the maximized-window demotion measured over the
*frame* — DWM's `SC_MOVE`-by-caption and WindowServer's
title-bar drag, answered server-side with the cadence on the
record; zero wire movement, pure server-side machinery. v0.20.0
closed the states arm's last follow-on whole — the SSD chrome pass
and the server-initiated `close` event it carries: the drawn title
bar, border ring, and close affordance painted as compositor-owned
ink around every server-decorated window (CPU ink on the dock's
model, the claims ledger, the chrome-aware maximize — the DWM
caption and the WindowServer title bar finally answered with drawn
chrome of our own, and the frozen `toplevel.close` given its first
sender: press arms, release fires, drag-away cancels — the caption
doctrine, zero wire movement). v0.19.0 closed two of the three
follow-ons (the `activated` bit —
the frozen flag riding real proposals through every focus
transition, the grace-parking state proposal never punishing a
drag-draining client; the operator-side space switching —
`shell.switch_workspace` with the clamp-honest `workspace_switched`
broadcast, the visibility sweep with dialogs following their
parents, the space-preferred focus, the hidden spaces' frame
parking — the taskbar's line the ledger named, served whole):
the dma-buf negotiation surface (client-negotiated pools over the
wire — the frozen `ldp.core.dmabuf` interface awaits its honest
`create` arm); the dock's dumb-buffer plane delivery; a dedicated
render thread (the single-mutex doctrine is load-bearing for the
byte-exactness oracles); a Vulkan renderer on the proven seam;
the real-panel end-to-end latency lab (the giants' measured-hardware
tuning); the quirk ledger's per-vendor rows (the mechanism and the
ledger's structure ship — v0.10.8 tables the floor, the latch, and
the sibling escape as the first five rows — the decades accrue with
hardware); the foundry's RBv3/OVT arms (CEA-861-H/I's successors
to the blanking story — Phase 46 poured every family the VESA
standards define; the CTA's own remain); TLS-class transport and
broker-era
identity for remote multi-user. The standing rule covers the honesty:
**no phase may contain stub code — missing functionality is
documented, never hidden.**
