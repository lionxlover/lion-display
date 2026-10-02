# LDP Administrator Guide

Deployment and operation of the LDP stack: installing the Debian
packages, running the reference compositor as a systemd service, the
security model as it affects operations (sockets, credentials,
capabilities, audit), resource behavior, and the honest scope of the
v0.2.0 milestone. Client development workflows live in the companion
[`docs/user-guide.md`](user-guide.md); the scope matrix in
[`docs/maturity.md`](maturity.md) is the reference for what is
hardened versus staged.

---

## 1. Installation

### From the release packages

Two binary packages built from the `lion-display` source package:

```console
# dpkg -i lion-compositor_0.2.0_amd64.deb ldp-tools_0.2.0_amd64.deb
```

or from a local repository with `apt install lion-compositor
ldp-tools`. What lands where (usr-merge layout throughout):

| Path | Package | Content |
|---|---|---|
| `/usr/bin/lion-compositor` | lion-compositor | the reference server |
| `/usr/lib/systemd/system/lion-compositor.service` | lion-compositor | the service unit |
| `/usr/share/doc/lion-compositor/` | lion-compositor | copyright, changelog, admin guide, maturity matrix |
| `/usr/bin/ldp-{info,debug,validate,profiler,audit,input-debug,grab,remote-gateway}` | ldp-tools | the eight operator tools |
| `/usr/bin/{ldp-bench,ldpc}` | ldp-tools | benchmark runner, protocol compiler |
| `/usr/lib/lion-display/examples/` | ldp-tools | hello-ldp, pointer-paint, clip-client |
| `/usr/share/doc/ldp-tools/` | ldp-tools | copyright, changelog, user guide |

`libdrm2`, `libegl1`, and `libxkbcommon0` are `Recommends`, not hard
dependencies: the protocol service itself needs none of them — they
are dlopen'd only when the DRM probe, EGL bootstrap, or xkb keymap
path actually runs, and every one of those paths degrades honestly
(reporting what is missing) instead of failing opaquely.

Verify an installation at any time with `dpkg -V lion-compositor
ldp-tools` (the packages ship md5sums) and the per-binary smoke tests:

```console
$ lion-compositor --selftest
renderer: software (GL unavailable: cannot load libEGL.so.1)
effects: medium (2-pass blur frost, soft shadows, corners)
shell: desktop layout, dock 84 px at the bottom
power: psr on (the panel sleeps when static)
outputs: extended (one desktop, outputs left-to-right)
quirks: 8 quirk(s) tabled: psr-flicker (panel) — escape: --no-psr; …
vrr: off (the fixed nominal grid)
hdr: off (the honest SDR panel)
modes: HDMI-A-1: edid-preferred-lie: the EDID prefers 1280x720 but the connector serves 1920x1080
selftest: pipeline up (eDP-1 1920x1080@60, 2 FBs, timeline live)
```

(The first line is the renderer selection's honest report: `gles
(hardware: …, discrete GPU: <device>)` on machines with a real GL
stack — those machines serve the full `high` Liquid tier, and since
v0.9.0 the line *names the GPU* (the class and the device, from
`glGetString`); on a machine whose only GL is a CPU rasterizer
(llvmpipe — no graphics card, or the driver failed), `auto` takes the
specialized software backend with the reason carried, because it is
the faster CPU path. The second line is the resolved effects tier
(Phase 27): `--effects auto` scales the visual language to the
machine (see the user guide's §3.8); `--effects minimal` pins the
plain path. The third line is the positioning shell (Phase 28):
`--dock auto` (the CLI default) serves the layout doctrine and the
system dock — see the user guide's §3.9; `--dock off` reverts to the
legacy pixel-exact behavior; `--dock-thickness <px>` tunes the
reservation. `--renderer auto` is the
default — hardware first, software fallback with the reason carried;
`--renderer gl` makes a missing GL stack a hard startup failure.
`--resolution WxH` (v0.9.0) forces the output's mode to an exact
size — the desktop operator's tool; a size the panel does not offer
is a hard startup failure listing every size it does offer. The
desktop matrix — what every display size actually costs, on the GPU
and without one — is in `docs/benchmarks.md`, and the honest
field-by-field comparison against Wayland, X11, macOS, and Windows
is `docs/comparison.md`.)

### Building packages from source

```console
$ git clone https://github.com/lionos/lion-display && cd lion-display
$ ./scripts/build-deb.sh            # build + package + fresh-root verify
```

The script drives the full pipeline in three stages — the release
build (`--build`), `dpkg-buildpackage -us -uc -b` (`--package`), and
the fresh-root verification (`--verify`). The verification stage is
the part worth knowing about as an administrator: it installs both
`.deb` files with real `dpkg` against a pristine root inside a user
namespace, asserts the installed status, exercises every installed
binary (`--selftest`, `--help` walks), and runs `systemd-analyze
--root=... verify` against the installed unit — the same
"installs cleanly in a fresh container" gate the release itself had
to pass. The packaging is deliberately debhelper-free
(`Rules-Requires-Root: no`), so it builds anywhere `dpkg-dev`, cargo,
and a Rust 1.75+ toolchain exist, entirely unprivileged.

## 2. Running as a systemd service

```console
# systemctl enable --now lion-compositor
$ systemctl status lion-compositor
```

The unit runs `/usr/bin/lion-compositor --mode auto --quiet`. What
that means on your machine:

* **No DRM stack** (headless host, container, VM without drivers): the
  service serves the complete LDP protocol on the deterministic mock
  display, indefinitely — the reference vehicle for clients, tools,
  and CI.
* **DRM device present**: since v0.4.0 the service enters the **real
  serve loop** — it takes DRM-Master, drives the connected output
  (mapped dumb scanout, applied atomic modeset), serves LDP clients
  on the abstract socket, and exits through clean teardown when
  systemd stops it (SIGTERM → disable commit → display dark). Since
  v0.5.0 the serving pipeline **follows the topology**: monitor swaps
  migrate mid-session (one report line per transition in the
  journal — `hotplug: migrated …`), the last display going away
  leaves the service dark but serving (LDP clients stay connected;
  the first re-plug paints the desktop again), so a cable pull is no
  longer a restart event. On
  multi-seat or console-occupied hosts where another master owns the
  device, bring-up fails typed and `Restart=on-failure` retries with
  backoff — check `journalctl -u lion-compositor` for the honest
  reason line. Operators who only want the zero-risk validation can
  run `lion-compositor --probe` once by hand (TEST_ONLY, the screen
  never blinks).

Override flags with a drop-in rather than editing the unit (updates
replace the unit file; drop-ins survive):

```console
# systemctl edit lion-compositor
```

```ini
[Service]
ExecStart=
ExecStart=/usr/bin/lion-compositor --mode headless --socket prod-ldp
```

If you override with `--dump DIR`, relax `ProtectSystem=strict` (or
add a `ReadWritePaths=`) in the same drop-in, since the default unit
grants no filesystem writes: the protocol service needs none — its
abstract-namespace socket lives in the kernel, not on disk, and the
mock display is in-memory.

The unit is hardened by default: `NoNewPrivileges`, strict
filesystem protection, kernel-tunable/module/cgroup/log/clock/hostname
protection, namespace and SUID/SGID restrictions,
`MemoryDenyWriteExecute`, an empty capability bounding set, and a
`@system-service` syscall filter whose coverage of the stack's needs
(memfd, socketpair, sendmsg/recvmsg with SCM_RIGHTS, eventfd, epoll)
was verified syscall-by-syscall against the systemd filter
definitions. The device policy stays permissive so the DRM probe can
open `/dev/dri/*` when present; the headless path never touches
devices.

Logs go to the journal: `journalctl -u lion-compositor -f`. Usage
errors exit 2; serve failures exit 1; a clean probe exit is 0.

## 3. The security model an operator should know

LDP's security architecture is deny-by-default with three layers, and
each layer has an operational surface:

1. **Kernel-attested identity.** Every client connection is
   credentialed by `SO_PEERCRED` at accept time — the uid/pid are read
   from the kernel, not asserted by the client. There are no
   unattributed sessions on the wire, and the audit chain records the
   attribution.
2. **Manifest baseline + capability tokens.** Sensitive operations
   (screenshots, input injection, global shortcuts, display
   reconfiguration, clipboard across applications) are gated behind
   unforgeable 256-bit tokens minted by the broker against an
   application manifest's declared scopes. Token validation is
2a. **Per-surface security classes (v0.15.0).** The capture gate also
   serves a per-window ladder: a client that marks its window
   `toplevel.set_security_class(protected)` (or claims the `lock`
   role, whose floor applies whatever the class says) keeps the
   content on the physical display but **out of every
   client-visible capture frame** — the rectangle paints black in
   `capture_manager.grab` results and `ldp-grab` output alike. This
   is the operator-facing HDCP-class doctrine: the screenshot policy
   of a kiosk, a banking window, or a lock screen, enforced at the
   compositor because the compositor is what composes the visible.
   `Private`-class windows capture locally (the screen-share
   negotiation that consults the class rides the broker-era line).
   constant-time (~55 ns) and application-bound: a valid token
   presented by the wrong application fails. The full permission
   matrix — 18 operations — is exhaustively tested across
   deny/prompt/grant outcomes.
3. **The audit chain.** Every grant and denial lands in a hash-chained
   JSONL log: SHA-256 over (previous hash, record), window-anchored.
   `ldp-audit FILE` verifies structure (deletions and duplications are
   structural failures) and the chain itself (reordering is caught by
   the hash walk); `--expect-head HEX` detects truncated tails from an
   operator checkpoint. Chain verification costs ~0.5 µs/record — an
   evening's log verifies in milliseconds.

Abstract-namespace sockets carry no filesystem permissions — access
control is the credential + token model above, not path bits. If you
need to compartmentalize which local users may connect at all, run
separate compositor instances (each with its own `--socket` name) per
trust boundary; session identity and audit remain per-instance.

**Resource ceilings** are enforced server-side, not advisory:
per-session object counts, buffer sizes, message framing limits, and
FD counts are all bounded, and the stress gate (32 clients, 30,056
sessions, 4,909 abrupt disconnects reclaimed, 129,819 frames over 15
minutes) demonstrates the server returns to its FD baseline after
crash floods. A `kill -9` mid-message leaves a healthy server — crash
reclamation is a tested property, not a hope.

## 4. Performance expectations

The committed release numbers (2-core CI hardware, release profile)
live in [`docs/benchmarks.md`](benchmarks.md); the headline rows an
operator cares about: a 1 KiB protocol round-trip at ~1.1 µs, bulk
64 KiB frames at ~8.4 GiB/s, the 60 Hz scheduling decision at ~62 ns,
audit append at ~0.75 µs/event. The 4K single-surface composition
budget (< 8 ms on 2 cores, release build) is the Phase 8 exit
criterion the software renderer was built to meet.

For your own hardware, the honest measurement is always:

```console
$ ldp-bench --markdown /tmp/report.md
```

against the installed (release, stripped) binaries.

### Power expectations (v0.10.2)

The static-frame power path is on by default: a still desktop puts
the panel into self-refresh (zero device events while it sleeps),
the GPU clock decays to the floor, and the idle ladder (if
`--idle MS` is configured) dims and blanks on top. The teardown's
`power:` line is the machine's own account — the energy ledger's
model (documented constants, not a measurement of your specific
hardware), with the static-scene ratio reading ~0.13 for a desktop
that sleeps and ~1.0 for one that cannot. On panels whose
self-refresh flickers, `--no-psr` restores the always-scanning
behavior (and the ledger will show you what that costs).

### X11 compatibility expectations (v0.10.3)

The bridge's X11 face is rootless by default: every top-level X
window is its own LDP window (placed by the shell, dressed by the
materials), override-redirect windows ride the popup role, and each
window commits only its own damage. Expect per-window costs in the
same class as the rootful screen: the export composite is the same
blit discipline the rootful driver runs, distributed per window.
Input events to a quiescent X application arrive over the
`connection.sync` keepalive (a few tiny frames per second — the
compositor parks cross-client events for the owner's next message).
If an operator needs the v0.10.0 whole-screen projection (a kiosk
displaying a complete X desktop as one window), `--x11-rootful`
restores it; the root window's content (wallpapers) is not exported
in rootless mode — the LDP desktop's own background shows instead.

### Display arrangement expectations (v0.10.4)

The extended desktop is the default (`--outputs multi`): every
display carries its own portion. `--outputs mirror` serves the clone
arrangement — every display shows the same desktop, each cropping
to its own mode. A mirrored desktop costs one render per distinct
display *geometry* (same-size mirrors share the renderer's canvas —
identical content, one canvas), and every display flips at its own
refresh: a buffer both displays scan out releases after both flip
past it. Per-output scales (`--scale 1,2`) advertise one factor per
output; the shell's geometry resolves on the primary's own factor,
and a display joining mid-session inherits the doctrine's last
entry (the stretch rule).

### The quirk ledger (v0.10.4; the VRR rows v0.10.8; the timing trio v0.10.9)

The display ecosystem's long tail is quirks — behaviors real panels
and display engines exhibit that the clean KMS model does not name.
The giants carry decades of per-vendor quirk tables; this project
ships the *mechanism* with the rows it can honestly claim — every
row named, with its symptom, its detection, and its operator
escape. A quirk is only tabled when the system can route around it
deterministically: a quirk with no escape is a bug report, not a
table row.

| Quirk | Class | Symptom | Detection | Escape | Cost |
|---|---|---|---|---|---|
| `psr-flicker` | panel | visible flicker on static content while panel self-refresh is engaged | the flicker stops within one refresh of `--no-psr` (a static framebuffer never flickers — it is the self-refresh transition) | `--no-psr` | the display engine keeps scanning a static framebuffer — the sleep-tier power savings are traded away |
| `vrr-flicker` | sync | brightness pumping while an adaptive-sync panel stretches its scanout clock inside the VRR window | the pumping stops with the fixed nominal grid (serve without `--vrr`) — it is the panel's backlight response to the stretched clock, not the content | serve without `--vrr` | the commit window stays at the fixed nominal — late commits cannot borrow the panel's stretch |
| `hdr-peak-bloat` | advertising | HDR highlights flatten or dim late — the tone mapping aims at a peak the panel's backlight never actually shows | the panel's EDID peak exceeds what a meter reads on a full-white field (the advertisement is the bloated side; the measurement is the sustained truth) | `--hdr-peak N` (the measured sustained peak) | highlights above the measured peak compress one stop earlier — honestly, instead of the panel's own late clip |
| `vrr-floor-flicker` | advertising | brightness pumping on slow content — the panel flickers only while the scanout clock stretches toward its advertised minimum refresh rate | the pumping stops with the fixed nominal grid (serve without `--vrr`) but *not* with faster content — it is the deepest stretch band, not the VRR mechanism; a floor above the band (`--vrr-floor 57` on a panel advertising 48, say) removes it | `--vrr-floor N` (the honest minimum rate, Hz) | content slower than the floor rides the LFC repeats instead of the removed stretch band — the cadence is phase-aligned, the deep stretch is gone |
| `vrr-sibling-flicker` | sync | a fixed-sync display flickers while its VRR sibling stretches — visible only on a mixed desktop (one adaptive-sync output, one fixed) | the flicker stops when the whole desktop serves fixed (without `--vrr`) — the outputs' scanout clocks are coupled on this platform; the panel itself is healthy | `--vrr-uniform` (one fixed desktop across the seam) | the VRR output pays the fixed grid — the late-commit stretch and the adaptive opportunity both lapse until the desktop is uncoupled |
| `edid-rotten` | timing | the monitor reports no identity — the startup line names the connector, not the panel; persistence keys cannot bind to the monitor | the bring-up audit prints `edid-rotten` (the EDID bytes fail the checksum or header parse) while the connector still enumerates modes — the block is corrupt, the panel is not | `--synth WxH` (pour the known truth) or `--resolution WxH` | the pour is a user-defined mode — the display engine validates it at commit, and the panel's own preferred timing stays unknown until the cable or firmware is fixed |
| `edid-preferred-lie` | timing | the desktop comes up smaller than the panel — the monitor's EDID prefers 720p-era timings while the connector enumerates the full-size modes | the bring-up audit prints `edid-preferred-lie` (the EDID's first detailed timing names a smaller active area than the connector's largest offered mode) — stale firmware, not a driver bug | `--resolution WxH` (force the real size) or `--synth WxH` (pour it when even the list is wrong) | the operator asserts what the firmware should have — a wrong guess serves a wrong size, honestly refused only when the connector does not offer it |
| `pixel-clock-ceiling` | timing | the highest mode blanks the panel — the mode enumerates, the commit validates, the display stays dark or drops to black mid-scan | the bring-up audit prints `pixel-clock-ceiling` (the connector's best mode clocks above the EDID range-limits descriptor's declared maximum) — the panel's own envelope is the truth | `--resolution WxH` with a mode under the declared ceiling, or `--synth WxH@Hz` (the foundry refuses pours above the ceiling, naming it) | the desktop stays inside the panel's honest envelope — the above-ceiling modes the list tempts with are traded away |

`lion-compositor --selftest` prints the table's report line; the
rows accrue with hardware (the mechanism — `ldp-display`'s `quirks`
module — is the stable part; a row is a public commitment, its
name and escape never move). The `hdr-peak-bloat` row was the first
accrued row (v0.10.5); v0.10.8's quirk ledger accrues the VRR pair;
v0.10.9's mode foundry accrues the timing trio — eight rows deep
now, each row's detection the bring-up audit's own named finding.

**The VRR quirk diagnosis, worked** (the operator's path through the
two new rows): watch *when* the artifact appears. Brightness pumping
that tracks *slow* content and vanishes with faster content is the
floor class — the deepest stretch band flickers; raise the floor
(`--vrr --vrr-floor 57`) and the panel never schedules into the band
again (the selftest's `vrr:` line prints the clamp's audit trail,
per output). Flicker on a *different display* than the one running
VRR — a fixed sibling flickering while its VRR neighbor stretches —
is the coupling class; collapse the desktop (`--vrr --vrr-uniform`)
and both settle on one fixed clock. The floor clamps only what the
*system* schedules — the scheduler's widened deadline, the
`output.vrr` advertisement, the LFC cadence — while the device's own
advertised window stands untouched (the override lives above the
driver, exactly where the giants' tables live). The floor's grammar
is `--scale`'s: a lone rate blankets every output; a comma list
names one rate per output in output order (`57,0` — the eDP
floored, the HDMI passthrough), extras reusing the last entry. An
unservable floor — above the mode's own refresh rate — refuses
boot, naming the mistake; content slower than the floor rides the
LFC repeats (locked to the content's own phase, the boundary flap
latched away) instead of the flickering stretch.

### The mode foundry (v0.10.9)

The custom-resolution machine — what `xrandr --newmode` and
Windows' custom-resolution form run on — served as arithmetic:

```console
$ lion-compositor --synth 2560x1440
…
modes: eDP-1 pours 2560x1440 @ 60.0 Hz (CVT-RB 241700 kHz, userdef)
selftest: pipeline up (eDP-1 2560x1440@60, 2 FBs, timeline live)
```

`--synth WxH[@HZ][:family]` pours a VESA timing for *any* size
(the refresh defaults to 60, the family to `rb` — CVT reduced
blanking v1) — the size the panel's own list does not carry, or
the size at a refresh its modes of that size lack (`--synth
1920x1080@144` on a 60 Hz panel serves 144). **The family names
the standard (v0.14.0 — the foundry completes)**: `:rb2` (CVT
reduced blanking v2 — the 80-pixel deep-color-era blank and
**1-pixel horizontal precision**, so `--synth 1366x768:rb2` serves
the odd width exactly), `:rb2v` (RB2 with the 1000/1001
video-optimized rate — `--synth 1920x1080@60:rb2v` serves the
59.94 Hz class), `:cvt` (standard CRT blanking — the analog-era
duty-cycle arithmetic), and `:gtf` (the 1999 generalized timing
formula). The arithmetic is integers-only, the pixel clock is
ceil-targeted so the realized refresh is *never under* the ask
(the VESA tool's 0.25 MHz grid under-serves; this foundry lands
1080p RB60 at 60.000087 Hz — the sub-millihertz wire residual,
stated), and the canonical anchors are pinned in the test suite
(`cvt -r 1920 1080 60`'s 2080×1111 totals and the four-family
1080p60 rasters, bit-exact).

**The two gates, both honest** — this is the part to know before
pouring on real hardware. The connector's EDID range limits gate
the pour *before* it is proposed: a clock above the panel's
declared maximum is the typed refusal naming the declaration (the
`pixel-clock-ceiling` row's doctrine — never a silent clamp), and
an unservable pour refuses boot rather than serving a lie. The
display engine validates the pour *again* at commit time, exactly
as a real driver's atomic check validates user modes — the pour
rides the wire as a **user-defined mode** (the kernel's own
vocabulary), never laundered into a sink-offered timing. A bound
client sees the pour in the output's advertised mode list, flagged
current (`xrandr --addmode`'s story, protocol-side).

**The pour rides the topology**: a hotplug swap re-pours on the
next display (each connector under its own ceiling); a connector
whose declared ceiling refuses falls back to the unsized doctrine —
the migration never dies on the operator's preference. `--synth`
and `--resolution` are mutually exclusive (the pour vs the offered
list — two different asks; the CLI refuses the pair).

**The audit line** (`modes:` in the selftest, `edid:` at DRM
bring-up): the bring-up diagnoses every connector's truth — a
rotten EDID, a preferred-timing lie (stale firmware naming a
smaller mode than the connector serves), a declared clock ceiling
the enumerated list exceeds. Every finding names its quirk row
(the table above); the rows name the escape. The worked path:
a desktop that comes up smaller than the panel → read the `modes:`
line → `edid-preferred-lie` → force the real size
(`--resolution 1920x1080`) or pour it (`--synth 1920x1080`) — the
diagnosis teaches the fix, one line at a time.

### The HDR negotiation (v0.10.5)

The HDR pipeline negotiates, and the operator is part of it. The
panel's *effective* peak — what the render pass will actually
deliver — is the advertisement clamped by `--hdr-peak N` (with
`--hdr`): a panel whose EDID claims 600 nits but sustains 450 gets
`--hdr-peak 450`, and from then on the advertisement
(`output.hdr_caps`), the PQ canvas's negotiated ceiling, and every
HDR layer's tone mapping all carry 450. Clients reading `hdr_caps`
see what they can rely on — the negotiation's reply side.

The canvas ceiling itself negotiates per frame: the stack's
brightest declared content (the maximum CLL over the mapped HDR
surfaces' `set_hdr_metadata`) clamped to the effective peak —
dimmer content rides at its own level, brighter content maps down.
Per-surface mastering metadata reaches the ink through the
BT.2390-structured knee (the system's soft rolloff replacing the
panel's hard clip); surfaces without metadata get the honest clip
at the ceiling. The selftest's HDR line names the served doctrine:

```console
$ lion-compositor --selftest
hdr: hdr pq (effective peak 600 nits — the negotiation ceiling)
```

The DRM-side infoframe emission (the HDMI HDR Static Metadata
InfoFrame carrying the negotiated ceiling to the physical panel) is
a named roadmap line; the negotiation's computation is complete and
proven today.

### The fresh frame: latency expectations (v0.10.6)

The emission path coalesces position state by doctrine — no
operator knob, because there is nothing to tune: a 1000 Hz pointer
between two display frames parks sixteen samples and the
frame-cadenced client reads one `motion` (the freshest coordinates,
every `relative_motion` delta intact, the discrete events sealed in
order). The expectation this sets: input-driven clients should
process pointer state at *their* frame cadence, not the device's —
a client that drains per frame sees one wake per frame whatever
the mouse's polling rate, and the input→photon budget (arrival to
landed flip) stays within one nominal period plus submission costs,
asserted in CI under the flood.

Under `--vrr` the presentation feedback is the honest pacing
oracle: a client on the adaptive presentation mode that commits at
readiness presents at the panel's fast end (on the development
mock's 48–144 Hz window, 6.944 ms per frame), and its `presented`
events carry exactly that cadence — `refresh` is what the panel
ran, `presented_at` is the flip that carried the content. The
operator reading a latency complaint should ask which of the two
doctrines is in play: the fixed grid (the default — 16.667 ms
verdicts, byte-exact since the early phases) or the window (the
adaptive opt-in — pacing follows the content). What remains
honestly unserved is the real-panel measurement campaign: the
budgets are mock-proven and CI-pinned, the oscilloscope lab is the
named roadmap line.

### The deep material: visual expectations (v0.10.7)

The material family serves by role — popups and menus wear the
vibrant menu glass, the dock wears chrome with the hairline, opaque
windows keep the panel material — and the whole language scales with
the `--effects` tier exactly as before: a `low`-tier machine keeps
the identity without the blur (the veil, the corners, and now the
hairline too — the ring costs perimeter work, not area), `minimal`
keeps CI-plain pixels. The expectations this sets for an operator
reading a visual complaint: the *frost saturation* is not a
mis-calibration — menus intentionally boost the backdrop's chroma
past itself (the acrylic look; a colorful wallpaper glows through a
menu), while sheets keep the Phase 27 desaturating frost; the
*hairline* at a surface's edge is the material's own light stroke,
not a rendering artifact; and none of it costs steady-state memory —
the ring is memoized imagery like the shadows (one build per shape
while a window sits still, the `pixels_effect` statistic carrying
the per-frame cost honestly). Everything is byte-equal across the
software and GL backends by construction, so a visual difference
between `--renderer software` and the GL path is a bug, not a
tuning question.

### The states arm: window-state expectations (v0.17.0)

The toplevel states vocabulary is served whole, and every piece of
it carries operator-visible behavior worth knowing when reading a
desktop complaint. `--workspaces N` (default 4; `0` promotes to 1)
sets the spaces count the shell bind reports (`workspace_count`)
and the clamp `set_workspace` rounds into — a client asking for
space 99 on a 4-space desktop lands on space 3, by design, and the
`workspace_changed` event tells it so honestly. A **minimized**
window (or one moved to a space the seat is not viewing) is not
occluded — it is *gone from the desktop*: it renders nothing, takes
no input, parks its frame requests (App Nap — the frame economy
sleeps), and its client sees `leave_output`; when it returns the
desktop is byte-identical to before it left. Maximize and fullscreen
are client-acknowledged two-phase proposals, not server jumps: the
window only moves when the client commits a buffer, so a
never-acking client never sees its window moved (a hung app's
window stays exactly where it was — the honest reading of "the
window ignored my maximize" is that the app is not servicing its
protocol). The `--transitions` choreography composes with all of
it: the window-open fade fires for a window *returning* from a
minimize exactly as for one mapping the first time, and a hidden
window's destroy leaves no close-fade ghost (it had no ink on the
canvas to fade).

### The operator's hand: drag expectations (v0.18.0)

The interactive move/resize vocabulary (`toplevel.start_move` /
`start_resize`) is served, and its cadence doctrines are worth
knowing when a desktop complaint names a "laggy drag". A **move**
is server truth at the input pump's cadence: the window follows
the pointer batch-for-batch (the damage engine repainting both
ends), the client sees *no* configures, and the keep band holds
48 logical px of any dragged window reachable inside the work
area — an operator cannot lose a window off-screen, whatever the
client. A **resize** proposes at the pointer's cadence but
realizes at the *client's* cadence: the window only takes its new
size when the app acks and commits a matching buffer, so a slow
app drags its resize visibly behind the pointer (the honest
reading of "the resize lags" is that the app is not servicing its
protocol — the same doctrine as the maximize verb's). The resize
never tears: the position and the committed buffer land together.
Dragging a maximized window by its title demotes it under the
pointer's grip (the restore size proposes, the window detaches,
the shrink lands with the app's commit); resizing a
geometry-stated window is refused by design. A grip dies with its
window, its app, or a superseding interaction — no zombie drags
hold the desktop's geometry.

## 5. Troubleshooting

* **Service starts then exits 0 immediately** — `--mode auto` found a
  DRM device and completed its probe-only scope. That is correct
  v0.2.0 behavior on real hardware; pin `--mode headless` via a
  drop-in if you wanted the protocol service.
* **`journalctl` shows `selftest failed: ...`** — the headless
  bring-up could not complete (memory exhaustion is the realistic
  cause on very small systems; the mock display allocates two
  1920x1080 framebuffers plus pools).
* **Clients get `ECONNREFUSED`** — no listener on that abstract name:
  the service is not running, or it is running with a different
  `--socket`. The startup log line prints the canonical `@name`.
* **Tools report permission denials on sensitive operations** — that
  is the capability model working: the operation needs a brokered
  token bound to the application manifest. Check the audit log with
  `ldp-audit` for the recorded denial.
* **Suspected protocol drift between server and tools** —
  `ldp-validate --live` cross-checks the served schema against
  `spec/`; mismatches mean the binaries come from different protocol
  generations. The Debian packages are version-locked as one release,
  so this points at a mixed installation.
* **Audit verification fails with `NonContiguous`** — the log has a
  deleted or duplicated record (structural tampering or a partial
  write). A reordering failure from the hash walk is unambiguous
  tampering: the chain does not hash to the recorded heads.

## 6. Upgrading and scope

v0.7.0 is the seventh milestone: the bootstrap of v0.1.0, the growth
phases (the remote relay, capture with `ldp-grab` and the PNG
encoder, the renderer performance tier), the hardware doctrine,
**the real-KMS serve loop**, **live output re-arrangement**, **the
Liquid visual engine** (the macOS-class material language with
low-end quality tiers — `--effects`), and **the positioning shell**
(the layout doctrine from the output's shape, placement at first
attach, the frosted system dock — `--dock`) —
`--mode drm` drives real hardware end to end with clean teardown,
and the served pipeline follows the topology (swaps migrate
mid-session, the last display gone leaves the compositor dark but
serving, the first re-plug paints the desktop again) — all hardened
on the deterministic display and proven byte-equal on the delivery
path; the multi-output layout, multi-user remote identity, and the
daemonized broker/escalation UX are staged next.
Read
[`docs/maturity.md`](maturity.md) before planning anything
production-facing on top of this release — it states, per subsystem,
exactly what is CI-hardened, what is integration-proven, and what is
deliberately future work. Uninstall is `dpkg -r ldp-tools
lion-compositor`; no conffiles, no maintainer scripts, no residual
state (the service writes nothing to disk).
