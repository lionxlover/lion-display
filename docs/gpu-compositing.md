# The hardware compositing path

*Phase 34 (v0.10.1). This document is the GPU-compositing field's
architecture: the whole road from a client's buffer to the panel —
which pixels the GPU never touches, why, and what the alternatives
(the giants' MPO stacks) do differently. The engine it describes is
`ldp-planes` (the decision layer) plus the compositor's
[`plane_session`](../binaries/lion-compositor/src/plane_session.rs)
(the walk, the commit, the display model), all CI-proven over the
mock KMS device's real plane inventory.*

## The question a compositor must answer

Every frame, the display hardware offers a small number of **planes**
— scanout slots a CRTC blends in zpos order with no GPU involvement
at all. The compositor's job is to answer: *which subset of the layer
stack can the hardware blend on its own?* Windows calls the answer
MPO (multi-plane overlay) and its DWM composites exactly this shape;
macOS's WindowServer composes the same doctrine over its render
server. Until v0.10.1, LDP answered only the narrowest version of the
question ("could this single fullscreen layer ride the primary
plane?" — the renderer's pre-filter). The plane engine answers the
real one.

## The three frame shapes

```text
Shape 1 — ZERO-COMPOSITE (the ceiling):
  zpos 1: [ overlay 51 ]   ← top window (fb-backed, 1:1, unstyled)
  zpos 0: [ primary 50 ]   ← the fullscreen opaque client's OWN buffer
  The renderer emits NO pass: no upload, no blend, no copy. The
  client's pixels scan out untouched — zero GPU work for the frame.

Shape 2 — SPLIT (the underlay):
  zpos 1: [ overlay 51 ]   ← the offloaded suffix rides hardware
  zpos 0: [ primary 50 ]   ← the composited CANVAS: the prefix the
                              renderer blended (damage-scoped)
  The GPU composites only the layers under the split point; the
                              planes above arrive free.

Shape 3 — FULL COMPOSITE (the honest fallback):
  zpos 0: [ primary 50 ]   ← the canvas carries everything
  Every layer failed eligibility (or the budget): the Phase 25/31
  bytes verbatim.
```

The **split-point doctrine** is the correctness rule that makes shapes
2 and 3 safe: a composited layer blends *into the canvas on the
primary plane*, so everything **under** it must render there too —
there is no GL surface sandwiched between hardware planes. The
offloaded set is therefore always a **suffix** of the stack (the top
layers) and the composite set a **prefix**. `Assigner::solve` finds
the deepest admissible split, fits the suffix to the overlay budget,
and searches plane assignments with depth-first backtracking over the
per-layer capability sets — the search that fixes the greedy failure
mode (an early layer monopolizing the only plane, or the only zpos
slot, a later layer can use).

## The eligibility table

A layer may ride a plane when it is:

| Rule | Why | The demotion (named, per layer) |
|---|---|---|
| **GEM-backed** — the import walk registered a kernel framebuffer | Only GEM memory scans out; a CPU window has no handle | `NoFb` |
| **Untransformed** | Plane rotation caps are a v2 line | `Transformed` |
| **1:1** | Planes do not scale in v1 (the sampler's continuous-nearest rule belongs to the GL arm) | `Scaled` |
| **Color-exact** — the output's own description | No per-plane color management in v1 | `ColorMismatch` |
| **In bounds** | Partially-offscreen planes are a future line | `OutOfBounds` |
| **Opacity 1.0** | No standard plane-alpha property exists | `Translucent` |
| **Unstyled** | Rounded corners and shadows exist only in the render path — a plane would scan a rounded window square | `Styled` |
| **Backdrop-free** | Frost samples the composite beneath it: its truth *is* the blend | `NeedsBackdrop` |
| **(format, modifier) supported** | The `IN_FORMATS` walk, per plane | `FormatUnsupported` |

The bottom layer of a zero-composite stack additionally must **cover
the output opaquely** (`BottomNotCovering` names the miss). Cursor
planes are reserved for the pointer sprite and never assigned.
Overlay planes below the primary's zpos are carried but ignored
(below-canvas stacking is a future line). Every composite layer
carries exactly one named reason in the plan's **demotion ledger** —
the no-silent-degrade rule applied to hardware offload.

Per-pixel alpha is *fine* on a plane (the hardware's
`PIXEL_BLEND_MODE = Pre-multiplied` blends it); it is surface-level
opacity that demotes.

## The import walk (buffer → framebuffer)

```text
client pool fd ──drmPrimeFDToHandle──▶ GEM handle
                                       │
           buffer geometry (w, h, format, planes)
                                       │
                                       ▼
                              drmModeAddFB2WithModifiers ──▶ FbId
```

The walk runs **once per buffer object** (the ledger caches refusals
as honestly as successes — a memfd on real hardware refuses cleanly
and the layer composites, the report line says so). The mock accepts
any live descriptor, which is exactly what makes the full
orchestration — solve, multi-plane commit, flip, release gates —
CI-provable with real client buffers over the real wire. On real
hardware the memfd pools today's clients send take the CPU path until
the dma-buf negotiation surface ships (the named roadmap line); the
mechanism, the solver, and the commit choreography are already the
shipped, tested ones.

## The commit choreography

One atomic request per frame carries the whole plane state: every
assignment **on** with its stacking zpos, the canvas (or the bottom
layer's own framebuffer) on the primary, and — the correctness rule
the retire path enforces — every overlay that left the plan **off**
(a stale overlay would hover above the canvas forever otherwise; the
mock's modeset detector was taught the kernel's real rule that a
plane *disable* is a plain page-flip-class commit). The flip lands,
the scheduler observes it, presentation feedback flows, and the
release gates for superseded buffers pass exactly as they always did
— a buffer a plane read is released when the next flip replaces it,
the same per-output gate arithmetic.

## The display model (the pixel oracle)

The panel blends planes in hardware; a test harness reading only the
primary's framebuffer would be blind to the overlays. The frame loop
therefore maintains the **display model** — the composed truth: the
canvas with every plane-assigned layer blended over it in zpos order,
through the renderer's own canonical sampler and blend kernel (the
word-copy fast path for opaque alpha-free layers keeps a fullscreen
compose in the milliseconds even in debug builds). For zero-composite
frames the base may be stale — harmless by construction: the
primary-role layer covers opaquely, its blend replaces every word.
`World::scanout_words()` serves the display model (the scanout chain
before the first frame), so every existing pixel oracle — the
session suites, the capture path, the equivalence corpora — keeps its
meaning across all three frame shapes.

## YUV and scaled uploads (the GL arm's other half)

The GL renderer composites **every format the software path
composites** since v0.10.1: the 32-bit RGB family keeps the fast
word-swizzle arm, and YUV-family / RGB888 / RGB565 layers decode
through the software path's own sampler (`sample_premul_u8` — the
identical fetch, matrix, and quantize), so the decoded upload is
byte-equal to what the CPU renderer would blend, by construction.
Scaled placements upload the **source** pixels and let the draw scale
them: the GPU's NEAREST sampler and the reference evaluator's
continuous-nearest rule are the same math the software path's
`Mapping::Scaled` runs — pixel centers map through the inverse scale
and floor. (Fullscreen NV12 video still rides the primary plane
outright on drivers that take 4:2:0 — the zero-composite shape above,
no decode at all.)

## The allocation advice (dma-buf feedback)

`ldp-gpu`'s feedback tranches intersect what the GL device imports
with what the planes scan out: **tranche 0** (scanout-preferred — a
buffer allocated here is zero-copy end to end, plane-eligible by
default, with compressible modifiers preferred over linear), then
**tranche 1** (GL-only — the EGLImage arm, still no CPU payload, but
never offload). An empty tranche 0 is honest: no scanout-capable
allocation exists on that machine. The protocol surface that carries
tranches to clients is the named roadmap line; the policy it will
serve is shipped and tested.

## What remains (the honest ledger)

* **Vulkan renderer + compute** — the GLES 2.0 path stays the oracle
  doctrine; the Vulkan seam exists (the `Renderer` trait) and is the
  next named line (the reserve phases).
* **Per-vendor quirk tables** — the giants' 15 years of scheduler
  integration and driver quirks; the framework's seams
  (`PlaneInventory`, the demotion ledger) are where a quirk table
  plugs in.
* **Plane scaling/rotation, below-primary overlays, per-plane color
  management** — v2 lines, each named in the demotion table above.
* **The dock's plane delivery** — the system dock's ink is
  CPU-generated; delivering it to a plane needs the dumb-buffer
  staging path. Until then a visible dock pins the frame to the
  composite arm (the demotion ledger says `NoFb`) — the same honest
  subtraction the frost pays.
* **The dma-buf protocol surface** — client-negotiated dma-buf pools
  (the feedback tranches' wire form) so real-hardware clients get the
  mock's orchestration.

The zero-composite frame — the client's own framebuffer on the panel,
zero GPU passes, proven in CI (`binaries/lion-compositor/tests/
planes_session.rs`) — is the ceiling this document exists for.
