# LDP Benchmarks

The Phase 19 benchmark report (`docs/roadmap.md`): performance and
security micro-benchmarks from `ldp-bench` (run with
`cargo run -p ldp-test --release --bin ldp-bench`), the full
15-minute stress gate, and the fuzz CI budget. Machine-readable form:
`ldp-bench --json PATH` (schema: `env` + `suites[].results[]` with
`median/mean/p95/min/max_ns_per_op`).

Committed numbers: release build, 2 CPUs, 2026-09-29 (Phase 30
re-baseline — the Liquid desktop-matrix rows are this phase's exit
criterion: every desktop size, the High tier and the tier the no-GPU
doctrine picks, steady state and first frame); Phase 29 re-baselined
the phone-frame rows on the same day; Phase 21 additions and the
`Region::subtract` improvement re-measured on 2026-09-28 and marked;
the damage-engine rows joined at Phase 44 (2026-10-01, measured on
the v0.12.0 engine with the pristine-v0.11.0 A/B committed in the
Phase 44 section).

Produced by `ldp-bench` (Phase 19, `docs/roadmap.md`). Build profile: **release**, parallelism: 2 CPU(s), mode: full.

### performance

| codec: encode registry.bind | ns/op | 109.521 | 109.722 | 117.100 | 104.797 | 117.100 | 7 |
| codec: encode surface.damage (8 rects) | ns/op | 405.586 | 410.060 | 437.826 | 405.086 | 437.826 | 7 |
| codec: decode surface.damage (8 rects) | ns/op | 261.072 | 263.255 | 269.969 | 260.793 | 269.969 | 7 |
| codec: decode + strict signature check (registry.bind) | ns/op | 357.877 | 351.945 | 368.439 | 338.295 | 368.439 | 7 |
| transport: 1 KiB frame round-trip (sendmsg + recvmsg) | ns/op | 1060.359 | 1063.786 | 1094.711 | 1037.938 | 1094.711 | 7 |
| transport: 64 KiB frame round-trip | MiB/s | 8261.667 | 8136.475 | 7638.381 | 7638.381 | 8283.544 | 7 |
| geometry: Region.add (256 seeded rects) | ns/op | 3.375 | 3.728 | 5.223 | 3.336 | 5.223 | 7 |
| geometry: Region.subtract (64 cutters) | ns/op | 1622.109 | 1646.350 | 1757.188 | 1589.188 | 1757.188 | 7 |
| damage: quiet 64-window desktop pass (no changes) | ns/op | 44609 | 44606 | 45076 | 43986 | 45076 | 7 |
| damage: one moving window among 64 (1 px/frame) | ns/op | 77199 | 77454 | 78750 | 76938 | 78750 | 7 |
| scheduler: 60 Hz decision cadence (frame_request + commit + drain + flip) | ns/op | 61.655 | 64.109 | 68.250 | 61.593 | 68.250 | 5 |
| color: sRGB transfer decode (1 Mi samples) | ns/op | 7.806 | 7.951 | 8.485 | 7.673 | 8.485 | 5 |
| color: PQ transfer encode (1 Mi samples) | ns/op | 17.449 | 17.578 | 18.482 | 17.240 | 18.482 | 5 |
| color: matrix apply3 (1 Mi RGB triples, BT.709 -> BT.2020) | ns/op | 3.716 | 3.717 | 3.726 | 3.712 | 3.726 | 5 |
| clipboard: 16 MiB stream through a real pipe (4 KiB pages) | MiB/s | 1711.394 | 1696.139 | 1662.682 | 1662.682 | 1715.367 | 3 |
| renderer: 3840x2160 opaque composite (full damage) | ns/op | 4990005.000 | 6857960.800 | 14608660.000 | 4803463.000 | 14608660.000 | 5 |
| renderer: 3840x2160 ARGB8888 composite (full damage, opaque pixels) | ns/op | 7425329.000 | 7294023.200 | 7502251.000 | 7023136.000 | 7502251.000 | 5 |
| renderer: 1920x1080 composite (12 windows, 32 damage rects) | ns/op | 464659.000 | 452979.600 | 478964.000 | 426930.000 | 478964.000 | 5 |
| renderer: 1080x2340 Liquid phone frame (wallpaper + 4 rounded cards + frosted panel, steady state) | ns/op | 8861690.000 | 8886765.800 | 9070046.000 | 8750409.000 | 9070046.000 | 5 |
| renderer: 1080x2340 Liquid phone frame (same scene, first frame) | ns/op | 63973382.000 | 64024934.000 | 64807076.000 | 63464122.000 | 64807076.000 | 5 |
| renderer: 1920x1080 Liquid desktop frame (wallpaper + 4 rounded windows + frosted dock, steady state, High tier) | ns/op | 7313920.000 | 7336324.200 | 7589659.000 | 6924314.000 | 7589659.000 | 5 |
| renderer: 1920x1080 Liquid desktop frame (same scene, first frame, High tier) | ns/op | 51702761.000 | 52153168.200 | 53992988.000 | 51440584.000 | 53992988.000 | 5 |
| renderer: 1920x1080 Liquid desktop frame (wallpaper + 4 rounded windows + frosted dock, steady state, the no-GPU doctrine's tier) | ns/op | 5956691.000 | 5898281.200 | 6022513.000 | 5735372.000 | 6022513.000 | 5 |
| renderer: 2560x1440 Liquid desktop frame (wallpaper + 4 rounded windows + frosted dock, steady state, High tier) | ns/op | 11492168.000 | 11523712.000 | 12046003.000 | 11115099.000 | 12046003.000 | 5 |
| renderer: 2560x1440 Liquid desktop frame (wallpaper + 4 rounded windows + frosted dock, steady state, the no-GPU doctrine's tier) | ns/op | 7946161.000 | 7923641.200 | 8061548.000 | 7702546.000 | 8061548.000 | 5 |
| renderer: 3440x1440 Liquid desktop frame (wallpaper + 4 rounded windows + frosted dock, steady state, High tier) | ns/op | 15114438.000 | 15331657.000 | 16191206.000 | 14414371.000 | 16191206.000 | 5 |
| renderer: 3440x1440 Liquid desktop frame (wallpaper + 4 rounded windows + frosted dock, steady state, the no-GPU doctrine's tier) | ns/op | 10957648.000 | 12266351.200 | 15587693.000 | 10913720.000 | 15587693.000 | 5 |
| renderer: 3840x2160 Liquid desktop frame (wallpaper + 4 rounded windows + frosted dock, steady state, High tier) | ns/op | 24674387.000 | 24826010.000 | 26220196.000 | 23604166.000 | 26220196.000 | 5 |
| renderer: 3840x2160 Liquid desktop frame (same scene, first frame, High tier) | ns/op | 221865499.000 | 222178781.400 | 224754434.000 | 218996246.000 | 224754434.000 | 5 |
| renderer: 3840x2160 Liquid desktop frame (wallpaper + 4 rounded windows + frosted dock, steady state, the no-GPU doctrine's tier) | ns/op | 18989667.000 | 19037706.800 | 19368421.000 | 18642937.000 | 19368421.000 | 5 |
| remote: envelope codec round-trip (1 KiB DATA) | ns/op | 96.404 | 96.357 | 96.494 | 96.156 | 96.494 | 7 |
| remote: pool-update codec round-trip (64 KiB window, in-memory) | MiB/s | 5760247.961 | 5762971.371 | 5749153.189 | 5749153.189 | 5776996.103 | 7 |

### security

| security: token submit — valid (64-entry table, constant-time walk) | ns/op | 48.707 | 50.114 | 58.482 | 48.686 | 58.482 | 7 |
| security: token submit — forged (64-entry table) | ns/op | 47.010 | 47.940 | 55.258 | 45.193 | 55.258 | 7 |
| security: token submit — cross-app (real token, wrong app) | ns/op | 49.221 | 49.243 | 49.338 | 49.219 | 49.338 | 7 |
| security: audit chain append (1024 events, fresh chain) | ns/op | 770.138 | 769.604 | 799.166 | 737.029 | 799.166 | 5 |
| security: audit chain verify (4096 records, full hash walk) | ns/op | 553.271 | 558.269 | 573.248 | 548.256 | 573.248 | 5 |
| security: permission matrix decide (all 18 operations) | ns/op | 0.228 | 0.229 | 0.233 | 0.227 | 0.233 | 7 |

## The Phase 44 changes

The quiet frame release (v0.12.0) — the damage engine's steady-state
economy, measured the honest way: **an A/B against the pristine
v0.11.0 engine, identical input, identical rig, full-size release
runs (7-run medians)**. The two rows above are new to the ledger
this phase (the workloads every desktop runs between its moving
frames; the harness builds a 64-window cascading opaque tree, primes
one pass so every surface carries its "old" side, then measures the
steady passes).

* **The quiet 64-window desktop pass** (a commit batch arrives,
  nothing moved — the animating desktop's every-other-frame load):
  **94,720 → 44,609 ns/op (−52.9%)**. The pristine engine paid one
deep region clone per surface per frame for the suffix-union table
  (O(n²) rect copies for a desktop whose opaque flips number zero),
  four region allocations per static window for the R2 coverage
  algebra, two subtracts and two intersects per window for the R3
  flip algebra, a three-allocation round-trip to ask "is this
  window fully visible," and one mask clone per window per frame
  for the pass records. The quiet-frame engine pays none of it:
  the below-unions materialize only at flip nodes, the coverage
  algebra runs only for surfaces whose bounds changed, the flip
  algebra only for surfaces whose footprint changed, the
  fully-visible check is a one-rect comparison, and the pass
  records skip quiescent unchanged surfaces (the guard's
  damage-consumption and occlusion-relatch terms are load-bearing —
  pinned by three regression tests, with the per-pixel corpus
  oracle standing over every equivalence).
* **The one-moving-window load** (one window slides 1 px per frame
  among 64 — the cursor-drag class: one surface's coverage rule
  fires, everything else stays quiet): **126,906 → 77,199 ns/op
  (−39.2%)**. The mover's own algebra still runs at full exactness;
  the 63 spectators stop paying for it.
* **The renderer rows are unchanged within run-to-run noise, by
  doctrine**: the full suite re-run fresh shows every Liquid
  desktop-matrix and composite row within noise of the committed
  ledger (1080p High steady 7.59 ms vs 7.31 committed, 4K High
  steady 24.79 vs 24.67) — the optimization lives in the damage
  pass the renderer's own rows do not measure. The committed
  ledger's rows stand.
* **The fuzz soak is the phase's second number**: the gate at
  `LDP_FUZZ_SCALE=20` — 300,000 codec iterations, 300,000 X11,
  300,000 Wayland, 30,000 transport, 5,000 dispatch connections —
  runs clean (83 s, zero panics, FD counts asserted), the soak that
  found the one-byte `chunk_stream` panic now behind it.

## The Phase 43 changes

The contact doctrine and the living registry release (v0.11.0) — the
hardening and delivery-economy wave measured the honest way: **an A/B
against the pristine v0.10.9 build, identical rig, identical
procedure, identical client** (the release-build compositor driven
headless by the showcase example, `VmHWM` read at quiescence).

* **Peak RSS, the A/B**: the default dual-1080p headless bring-up
  with the dock rendered measures **72.6 MiB (74.3 MB) on pristine
  v0.10.9 and 56.2 MiB (57.5 MB) on v0.11.0 — a −22.6% reduction**
  (three runs each, spread under 1%: 74,384/74,300/74,256 kB vs
  58,024/57,284/57,344 kB). The dock-off variant lands the same
  shape (73.7 → 57.1 MB). The mechanism is the delivery path's
  allocation economy: the per-frame full-canvas `to_vec()` is gone
  (the delivery writes the renderer's borrowed readout), the
  display model reuses the output slot's own `Vec` (capacity
  persists across frames; the planes blend in place), and the
  transport writer compacts its unsent front instead of draining
  per chunk.
* **The renderer rows are unchanged within run-to-run noise, by
  doctrine**: `ldp-bench` re-run fresh on both builds shows every
  Liquid desktop-matrix and composite row within ±5% either way
  (1080p High steady 7.09 → 7.16 ms, 4K High steady 24.92 → 24.13
  ms, 4K first frame 203.95 → 203.21 ms) — the byte-exactness
  contract holds, and the optimization lives in the serve path the
  microbenchmarks do not model. The committed ledger's rows stand.
* **The DoS closures are the un-benchmarked win**: the GL path's
  disjoint-rect merge went from restart-per-merge (O(n³) intersect
  tests — a legal 4096-rect overlapping damage list parked
  `begin_frame` for minutes) to a single union-find pass; the
  accumulated damage region folds to its bounding box past twice
  the per-request rect cap; the subsurface depth cap refuses the
  stack-overflow chain at creation. These have no honest steady
  state to time — their benchmark is "the frame that used to never
  arrive".

## The Phase 30 changes

The desktop matrix (`docs/roadmap.md`, Phase 30): every display size,
every tier, and the machines with no GPU — the compositor meets the
machine it boots on, measured.

* **The new desktop-matrix rows** (the Liquid desktop frame: a plain
  wallpaper, four rounded-and-shadowed windows in the quadrants, one
  frosted dock bar, full damage every frame — proportional geometry
  per size): the **High tier** steady state measures **7.31 ms at
  1920x1080** (137 Hz-capable), **11.49 ms at 2560x1440**,
  **15.11 ms at 3440x1440** (the 21:9 ultrawide), **24.67 ms at
  3840x2160**; the **no-GPU doctrine's tier** (Medium at 2.1 MP, Low
  above) measures **5.96 / 7.95 / 10.96 / 18.99 ms** across the same
  sizes. The verdict on this 2-vCPU rig: 60 Hz holds at the High
  tier through the ultrawide, and under the doctrine's tier at every
  size up to and including the full-damage 4K worst case (52 Hz —
  with a real desktop's localized damage far under budget, and the
  GL backend serving 4K High outright).
* **The 4K thrash fix the matrix caught**: the first full run
  measured the 4K High steady state at **194.4 ms** — the shadow
  memo's fixed 6 Mi-word budget sat *under* a 4K desktop's working
  set (five materials, ~6.5 Mi words), so the LRU evicted and
  rebuilt every material every frame. The budget now scales with the
  output (`begin_frame` pays twice its pixels, floored at the phone
  6 Mi): the steady state dropped **194.4 -> 24.67 ms median (7.9x,
  same bytes)**, proven by the new flat-counter oracle
  (`four_k_desktop_steady_state_does_not_thrash_the_shadow_memo`).
  A phone-sized output keeps the floor and its byte-exact Phase 29
  behavior — the budget only ever grows.
* The first-frame rows at the matrix's two ends: **51.7 ms at
  1080p**, **221.9 ms at 4K** — the cold material generation a
  modeset or a window map pays once.
* Every other row is unchanged within run noise on the same machine
  (the phone frame's 8.86 ms vs Phase 29's 8.70 ms is the same
  code inside run-to-run variance).

## The Phase 29 changes

The steady-state pass (`docs/roadmap.md`, Phase 29): the low-end
doctrine made real — the full Liquid scene at 60 Hz on a weak CPU,
byte-identical to Phase 28's output (the styled golden suites, the
GL/software equivalence corpus, and the new steady-state
transparency oracle all pin the bytes).

* **`renderer: 1080x2340 Liquid phone frame (… steady state)`**
  measured **178,615,908 → 8,696,141 ns median** (20.5×, same
  machine, before/after): the Phase 28 path regenerated every
  shadow and frost material every frame — four 3-pass box blurs and
  five full-surface SDF walks per frame at 60 fps. The steady state
  now serves the shadow from the material memo (built once per
  unique style), the frost from its memo (rebuilt only when the
  backdrop's own words changed — the steady state skips the blur
  entirely), and pays one word-level snapshot + comparison for the
  frost.
* **`renderer: 1080x2340 Liquid phone frame (… first frame)`**
  measured **178,019,530 → 64,827,190 ns median** (2.75×): the
  one-time material builds — the box blur's edge-free interior
  loops, 16-column strip walk (contiguous row slices instead of a
  cache line per row per column), and exact 48-bit reciprocal mean
  (proven exhaustively equal to the division for the sanitized
  window domain) — plus the corner-arc SDF bands (99.6% of a
  window's pixels never run the `sqrt`).
* Every other row of the table is unchanged within run noise on the
  same machine (the plain compositing paths were already
  memory-bound; the copy-path broadening and the direct-mapping
  blend specialization land inside the existing rows' variance).
* The two new rows are permanent suite shapes: the steady-state row
  is the phone's 60 Hz budget line (8.70 ms median against a
  16.67 ms frame), and the first-frame row is the window-map /
  modeset cost.

## The Phase 22 changes

* **`renderer: 3840x2160 ARGB8888 composite (full damage, opaque
  pixels)`** measured **51,259,400 → 6,842,010 ns median** (7.4x,
  same machine, before/after): same-format 1:1 fully-opaque ARGB/ABGR
  layers — the shape of a real window interior — now take the
  `CopyPremul` path (64-pixel chunks: fully-opaque chunks word-copy,
  mixed chunks blend per pixel; the copy is bit-identical to the blend
  by construction, proven by the golden suite).
* **`renderer: 1920x1080 composite (12 windows, 32 damage rects)`**
  measured **524,734 → 464,718 ns median** (~12%): the per-frame
  damage **row index** builds each row's merged x-intervals once per
  frame (retained buffers, no steady-state allocation) and serves
  every layer-row in O(intervals) instead of rescanning every damage
  rectangle — the desktop shape, where the old path's O(rects x rows
  x layers) dominated.
* The committed `renderer: 3840x2160 opaque composite` row is the
  same code path as before (unchanged within run noise: 5.12 → 4.87
  ms median on this machine's quieter runs); the copy path was already
  memory-bandwidth-bound.

## The Phase 21 changes

* `Region::subtract` (64 cutters) measured **373.9 ns → 189.0 ns
  median** on the same machine before/after the Phase 21 rewrite
  (retained scratch buffers swapped per cutter instead of a fresh
  allocation each, plus a disjoint-rectangle fast path). The committed
  Phase 19 number (3495 ns) is the original code on the original CI
  box; the table row above is unchanged for it, and the improvement
  is stated in same-machine before/after terms instead.
* The `renderer:` row is the Phase 8 exit criterion (4K single-surface
  composition under 8 ms) as a permanently tracked number — measured
  median 5.1 ms on this machine, with the p95 spike reflecting
  scheduler noise on a shared CI box, not allocator behavior.
* The `remote:` rows measure the Phase 21 relay's CPU share: the
  per-envelope codec cost on both gateway CPUs, and the in-memory
  encode+parse throughput of one 64 KiB commit window (the socket's
  share is the transport suite's domain; end-to-end correctness is
  `crates/ldp-remote/tests/remote_loopback.rs`).

## Methodology

* Every benchmark drives the real crate surface — no mocks, no harness shortcuts: the actual codec, framing writer/reader over a real socketpair, region algebra, the 60 Hz frame scheduler, the color pipeline, the clipboard pipe, the constant-time token walk, the hash-chained audit log, and the permission matrix.
* Inputs are deterministic (seeded generators), so runs are comparable across machines to the extent hardware allows.
* Timed runs follow one warmup pass; the table reports median / mean / p95 / min / max over the timed runs.
* `ns/op` medians are the headline for latency shapes; `MiB/s` for bandwidth shapes. Dev-profile numbers are indicative only — the committed report is a release run.

## The stress gate (full run)

`LDP_STRESS_FULL=1 cargo test -p ldp-integration --lib
-- --test-threads=1 the_stress_gate` — the roadmap's stress gate:
32 concurrent clients, mixed workloads (frame cycles with real memfd
pools, sync round-trips, buffer churn, crash-and-reconnect), 15
minutes of wall clock.

| metric | value |
|---|---|
| clients | 32 |
| wall clock | 900.6 s |
| sessions completed | 30,056 |
| sessions ended by abrupt disconnect (crash + reclamation) | 4,909 |
| frame cycles committed | 104,918 |
| sync round-trips | 34,893 |
| frames composited | 129,819 |

Stability verdicts, all asserted by the gate: a full healthy canary
session completes on the abused server, both session-end paths
exercised, the scene fully drains (no leaked routes or outboxes), and
the process FD table returns to its baseline. The compressed mode
(the default in CI) runs the same workload model for ~3 s.

## The fuzz CI budget

`cargo test -p ldp-fuzz --test fuzz_ci` — all five deterministic
targets run clean under the default budget (no panics, no OOM, no
leaks; both accept and reject paths exercised on every target;
seed-determinism proven for the in-memory targets).
`LDP_FUZZ_SCALE=<n>` multiplies iteration counts for soak runs.

| target | iterations | accepted | rejected |
|---|---|---|---|
| codec | 15,000 | 20,031 | 9,969 |
| x11 | 15,000 | 34,303 | 12,200 |
| wayland | 15,000 | 24,528 | 2,756 |
| transport (real socketpairs) | 1,500 | 2,731 | 1,500 |
| dispatch (real server connections) | 250 | 168 | 277 |
