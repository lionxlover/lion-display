# LDP Maturity Matrix — v0.25.0

The honest per-subsystem statement of what the 30-phase build
delivered, what hardens it, and what is deliberately staged for later.
It exists because the roadmap's standing rule — *missing functionality
is always documented, never hidden* — needs one place where a reader
can find both halves. Nothing on this page is aspiration: every
maturity claim cites the gate, suite, or committed measurement that
proves it, and every boundary states plainly what v0.9.0 does not do.

## How to read the levels

* **CI-hardened.** The property is enforced by an automated gate that
  runs on every `./scripts/verify.sh` (and CI) invocation. The primary
  evidence is the suite itself; regressions fail the build.
* **Integration-proven.** The property is exercised end-to-end against
  the real in-process stack — a real socket, real FDs, the real
  compositor — inside the integration suites, not against mocks of the
  components under test.
* **Mock-verified (hardware-adjacent).** The full logic is implemented
  and golden-tested against the deterministic mock device (mock KMS,
  simulated vblank timeline, synthetic EDID/udev). Real-hardware
  exercise is pending — the code path is honest about this at runtime
  (probe, report, or graceful dlopen degradation), never silent.
* **Staged.** Not in v0.4.0. Tracked in the roadmap's post-v0.4.0
  section; present here so the boundary is explicit.

## Cross-cutting gates (always on)

| Gate | What it enforces | Where |
|---|---|---|
| `cargo fmt --check` | one formatting canon | `scripts/verify.sh` |
| `cargo clippy --all-targets --all-features -- -D warnings` | pedantic lints, zero findings, every crate | `scripts/verify.sh` |
| 2,327 tests | unit + integration + conformance + property suites across all 37 workspace members | `cargo test` |
| `RUSTDOCFLAGS=-D warnings cargo doc` (incl. private items) | API docs are complete; intra-doc links resolve | `scripts/verify.sh` |
| `scripts/spec_lint.py` | 9 spec modules structurally valid | `scripts/verify.sh` |
| `ldpc gen --check` | committed generated code matches the spec TOML — no drift | `scripts/verify.sh` |
| `scripts/version_check.py` | release coherence: one version across the manifests, lock, debian changelog, README, CHANGELOG, and the scene mark | `scripts/verify.sh` |
| Architecture lint | zero Wayland/X11 vocabulary in any core crate (bridges only) | `ldp-wayland-bridge/tests/architecture.rs` |
| `#![forbid(unsafe_code)]` | server/compositor/client/tool paths contain no unsafe; syscall seams are isolated in audited transport/display/gpu/session crates | crate roots |
| Zero-dependency core | `ldp-core` has no dependencies at all | `ldp-core/Cargo.toml` |

## The matrix

| Phase | Landed | Maturity | Primary evidence |
|---|---|---|---|
| 57 | The chrome ghost (v0.25.0): the drawn chrome's fifth follow-on — a dying server-decorated window's band rides the close fade (the frozen shape truth, the whole frame leaving as one at the one close-spring opacity, the claims and the vacates grown by the frame, the Liquid frost outliving its window) | CI-hardened | `chrome_ghost_session.rs` (the band fading with the content — the frozen shape pinned, the monotone decay, the settled desktop equal to the never-animated destroy byte for byte; the title strip riding the fade — the darkest stroke pixel's contrast against the band through the fade's meaningful span; the z fidelity — the window destroyed under another keeping its band below the survivor, the survivor's opaque content byte-exact through the whole fade; the plain/CSD controls ghosting content-only; the transitions-off control; the Liquid ghost carrying the `liquid` bit with the dressed band dimming; the unmap arm riding the chrome) |
| 56 | The chrome-aware placement (v0.24.0): the drawn chrome's fourth follow-on — the placement answers in frame space (the *frame* takes the policy slot, the content rides inside; the pixel-true physical clamp; the migration arm on the applied insets; the z-true chrome hit) | CI-hardened | `placement_session.rs` (the parked window's band on-screen from row 0 with the affordance reachable; the frame cascade with each caption from its own row; the plain/CSD zero-drift interleaved with the chrome-aware stack; the oversized frame's corner anchor; the 2x panel's doubled shift; the hotplug migration re-anchoring the frame with the claims ledger repainting the chrome on the new panel) + `ldp-shell` layout's chrome unit tests (the frame-slot answers, the zero-inset identity sweep across every policy and slot, the chrome clamp) + `shell.rs`'s chrome-arm unit tests (the frame cascade, the phone park, the 2x and 1.25x scales, the frame re-placement) + the z-true routing proven live by the caption suite's band press over the second filler's content |
| 55 | The Liquid band (v0.23.0): the drawn chrome's third follow-on — the band wears the chrome material (the dock's own glass: the frost pane, the veil, the hairline, the rounded frame) + the frost memo's budget doctrine | CI-hardened | `liquid_session.rs` (the glass oracle at the Medium tier — the frost, the veil, and the hairline every word pinned against the world's own material builder, the corner cut to the backdrop; the backdrop read — the red\|blue split underlay with the blur's boundary mix exact; the underlay change flowing through the glass; the Minimal freeze — the flat bar byte-identical; the content riding above the pane; the capsule opaque over the glass; the strip still reading; the readability floor) + `shell.rs`'s dressed-ink unit test (the veil's word, the ring/capsule/glyph unchanged, the shape key's material variant) + `ldp-renderer`'s memo suites (the budget holding a desktop of panes, the LRU and the honest over-budget path, the entry cap, the `frost_rebuilds` thrash oracle — five panes, one build, flat forever) |
| 54 | The title glyph (v0.22.0): the band's own text — Lion Sans, the compositor's own typeface + the analytic-coverage rasterizer + the strip layer | CI-hardened | `title_session.rs` (the strip's pixel oracle importing the renderer's own over-rule — every word of the drawn title pinned, the truncation ending in the ellipsis with the never-reach-the-button proof, the title change repainting with no client commit, the empty title's byte-identical scanout, the CSD/fullscreen territory, the sharing proof — one base raster, distinct strips) + `ldp-font`'s unit suites (the rasterizer's exact math — the grid-aligned square, the fractional half-cover, the right triangle's diagonal, the circle's πr², the hole subtraction, the bearing contract, determinism; the table's orientation doctrine — every outer counter-clockwise, every hole paired; the layout's pixel-true truncation boundaries, the notdef/blank doctrines, the ink-box bounds, the scale sweep) |
| 53 | The caption drag (v0.21.0): the drawn title bar as a move grip (the server-minted drag) | CI-hardened | `caption_session.rs` (the caption drag at the pump's cadence — the press armed and consumed, the conversion at the first motion, the exact pointer travel, the pixel oracle of the band following and the desktop behind, the zero-configure proof; the click doctrine — no motion, no drag; the maximized-window demotion over the frame — the proportional anchor, the restored band still under the hand, the ack+commit realize; the close affordance's grip never converting; the death sweep) |
| 52 | The drawn chrome (v0.20.0): the SSD pass (the drawn band + the frozen `toplevel.close` first sender) | CI-hardened | `chrome_session.rs` (the close narrative — the press consumed and focusing, the release firing the frozen event, the client answer, the role death; the drag-away cancel and the re-arm; the ignoring client keeps its window and its desktop; the band press with no delivery and no ask; the CSD territory; the fullscreen cover; the pixel oracle — the band/ring/capsule/glyph words, the content hole, the minimize hiding the chrome with the ink and the unminimize restoring the exact bytes; the maximized frame fill with the content inset) + `shell.rs` painter unit tests (the band/ring/button/hole ink words, the cross glyph, the scaled affordance, the degenerate frames, the shape keys) + `ssd.rs` close-button geometry tests (the title-band placement at five scales, the degenerate saturation) |
| 51 | The focus and the view (v0.19.0): the `activated` state bit riding real proposals + the operator-side space switching | CI-hardened | `focus_session.rs` (the press narrative between two windows with both proposals and the machine agreement; the dying holder's promotion; the view-switch narrative — the ink flip, the sticky doctrine, the hidden space's frame parking, the space-preferred focus both directions, the parked frame answering on return; the clamp and the broadcast to a second binder, the no-op silence); `toplevel.rs`'s state-proposal grace test (the parked drag serial still acknowledgeable); the dialog gate's return grown its keyboard truth; the drag narratives grown the press's activation proposal |
| 43 | The per-contact coalescer + the living registry (dynamic globals) + the hardening/delivery-economy wave (v0.11.0) | CI-hardened | `coalesce.rs`'s per-contact key suite (independent collapse, per-contact seals); `outbox.rs`'s four Phase-43 doctrine proofs; `latency_session.rs`'s two-finger flood over the real socket (2 motions, 1 frame, each contact's own freshest); `hotplug_rearrange.rs`'s `the_registry_tracks_the_display_set_honestly` (revoke-before-barrier ordering, withdrawn bind, relight re-advert, fresh cascade, second dark round); `tree_semantics.rs`'s depth-cap pair; the A/B peak-RSS audit (72.6 → 56.2 MiB, same rig and procedure) |
| 44 | The quiet frame + the fuzz-soak hardening + the architecture comparison (v0.12.0) | CI-hardened | `damage_rules.rs`'s three fast-path equivalence proofs (damage consumed exactly once, the occlusion-only relatch, the partially-occluded scanout exclusion); `mutate.rs`'s two mutator-edge regressions (the one-byte `chunk_stream` source, the empty-source splice); the A/B damage benchmark (quiet 64-window pass 94,720 → 44,609 ns/op, one-moving-window 126,906 → 77,199, pristine-v0.11.0 engine on identical input, 7-run medians); the fuzz gate clean at 20× soak scale; `docs/comparison-windows-macos.md`'s ten-stage pipeline walk with its honest ledger |
| 1 | `ldp-core` (IDs/generations, errors, wire values, geometry+damage algebra, timing, color/HDR types, buffers, caps, tokens, limits) | CI-hardened | `invariants.rs`; zero deps, zero unsafe |
| 2 | `ldpc` + `ldp-protocol` (TOML→tables, codec, validation pipeline) | CI-hardened | `roundtrip_all.rs`, `malformed_corpus.rs`, `drift.rs`; drift gate in verify |
| 3 | `ldp-transport` (AF_UNIX, SCM_RIGHTS, framing, backpressure, SO_PEERCRED) | Integration-proven | `end_to_end.rs` (10k FD round-trips), `hygiene.rs` (FD-bomb, slow peer, FD-count asserted), `pressure.rs` |
| 4 | `ldp-server` (sessions, generational store, dispatch, ceilings, reclamation) | Integration-proven | `scale.rs` (1k×1k steady state), `crash.rs` (kill -9 corpus), `conformance.rs` |
| 5 | `ldp-client` (connection, proxies, class lanes, sync, reconnect) | Integration-proven | `full_session.rs`, `class_lanes.rs` (input-never-starved property) |
| 6 | `ldp-compositor` core (surface tree, atomic commits, damage+occlusion) | CI-hardened | `corpus.rs` (randomized graphs vs reference model), `occlusion.rs`, `snapshot_invariants.rs` |
| 7 | deadline frame scheduler + presentation events | CI-hardened | `scheduler_golden.rs`, `scheduler_property.rs` (hit-rate under jitter), `scheduler_replay.rs`, `coalesce*.rs` |
| 8 | `ldp-renderer` software backend + `Renderer` contract | CI-hardened | `golden_blend/transform/color/formats.rs` (pixel-exact), `perf_4k.rs` (<8 ms release, 2-core), `scanout.rs` |
| 9 | `ldp-display` (DRM/KMS atomic via dlopen) + `ldp-gpu` (EGL, DMA-BUF, sync) | Mock-verified | `atomic_golden.rs`, `vrr_timeline.rs`, `real_layer.rs`; `egl_bootstrap.rs`, `bit_comparable.rs`, `node_discovery.rs`, `sync_model.rs` — and since v0.10.4 the named-quirk table (`quirks.rs`: every row with its symptom, its detection, and its operator escape — the mechanism the giants' quirk decades ride) |
| 10 | `lion-compositor` vertical slice | Integration-proven | `full_session.rs`, `presentation.rs`, `render_pixels.rs`, `lifecycle.rs` — since v0.10.6 the fresh-frame proofs: `latency_session.rs` (the flood's 16:1 position-state collapse with the delta stream intact, the discrete barrier's sealed click, the VRR fast-end refresh honestly reported at 6.944 ms, the fixed-grid contrast, the input→photon budget under the flood) — and since v0.10.7 the material-depth proof: `material_session.rs` (the menu glass end to end over the real wire — the vibrant frost's hand-computed interior, the hairline at the straight edges, the wallpaper untouched, the steady state's bytes) — and since v0.15.0 the semantic-scene proofs: `semantic_session.rs` (the A/B/A capture-redaction byte-equality, the lock role's floor, the scene profile's two-truths oracle, the role carry) and `transitions_session.rs` (the window-open fade's first-frame start, the settle-to-exact-bytes oracle, the instant tooltip, the off-by-default identity) — and since v0.16.0 the knock and the goodbye: `dialog_session.rs` (the dialog mint/ack/centering pixel oracle, the second-role refusal, the stale-ack refusal, the modal gate's full narrative — the focus handoff over the real wire, the gate's return — the parent-death close, the modeless control) and `ghost_session.rs` (the close fade's destroy and unmap arms with the A/B byte oracle, the off-by-default identity, the z fidelity under a covering window, the popup/tooltip instant-dismiss) — and since v0.17.0 the states arm: `states_session.rs` (the handshake under the seat serial with the CSD insets and the accepted ack; the maximize narrative — the cascade restore point, the derived size, the position realize, the fill's pixel oracle; the unmaximize restore; the fullscreen cover with the output pin; the precedence; the stale-ack refusal; the minimize narrative — the App-Nap frame park, the leave/enter truth, the A/B/A byte oracle; the workspace move/clamp/report; the sticky doctrine; the hints with the error codes and the saturation) — and since v0.18.0 the operator's hand: `drag_session.rs` (the move narrative — the exact pointer travel, the pixel oracle, the zero-configure proof, the keep band over the wire; the resize narrative — the `resizing` state riding live proposals, the two-phase realize with the top-left anchored, the final proposal clearing the state; the left-edge resize moving the origin at the realize; the graceful ack of a superseded drag serial; the maximized-window demotion under the pointer's proportional grip with the pixel truths; the typed refusals — stale serial, geometry-stated resize, out-of-domain edges; the death sweep) — and since v0.19.0 the focus and the view: `focus_session.rs` (the press's activation proposal and its clear between two windows, the machine agreement; the dying holder's promotion through the one seam; the view switch — the ink flip, the sticky doctrine, the frame parking with the parked request answering on the return, the space-preferred focus both directions with the sticky window never stealing the keys, the clamp and the second binder's broadcast, the no-op silence) — and since v0.20.0 the drawn chrome: `chrome_session.rs` (the frozen `toplevel.close` finally sent over the real socket — the press armed and consumed, the release firing inside the affordance, the cancel outside it; the pixel oracle of the drawn band, ring, capsule, and glyph; the claims ledger's byte-exact hide and restore) — and since v0.21.0 the caption drag: `caption_session.rs` (the drawn title bar as a move grip over the real socket — the conversion at the first motion, the zero-configure move, the demotion over the frame with the hand still gripping the restored band, the close grip never converting, the death sweep) — and since v0.22.0 the title glyph: `title_session.rs` (the drawn strip's exact-word oracle over the band — the coverage-tinted glyphs, the pixel-true ellipsis, the no-commit title change, the empty title's identity, the CSD/fullscreen gates, the strip/raster sharing split) — and since v0.23.0 the Liquid band: `liquid_session.rs` (the chrome material on the drawn band over the real socket at the Medium tier — the frost pane and the veil and the hairline every word pinned against the world's own `frost_material`, the red\|blue split underlay proving the glass genuinely reads its backdrop with the blur's boundary mix exact, the underlay change flowing through, the Minimal freeze, the content above the pane, the capsule, the strip, the readability floor) — and since v0.24.0 the chrome-aware placement: `placement_session.rs` (the parked window's band on-screen from row 0 of the display — the gap's own refutation; the frame cascade; the plain/CSD zero-drift; the oversized frame's corner anchor; the 2x panel's doubled shift; the hotplug migration re-anchoring the frame with the claims ledger repainting the chrome on the new panel) — and since v0.25.0 the chrome ghost: `chrome_ghost_session.rs` (the close fade taking the whole window — the dying band's frozen shape pinned, the band and the title strip riding the content's fade at the one opacity, the z fidelity under a covering survivor, the plain/CSD content-only ghosts, the transitions-off control, the Liquid ghost, the unmap arm) |
| 11 | `ldp-input` + `ldp-seat` (evdev, accel, gestures, xkb, seats, grabs) | CI-hardened | golden evdev traces (`golden.rs`), accel property tests, `multi_seat.rs`, `grabs.rs` |
| 12 | `ldp-shell` (toplevel/popup/dialog machines, SSD, spaces) | CI-hardened | `conformance.rs` (from `spec/shell.toml`), `configure_fuzz.rs`, `ssd_dpi.rs`, `popup_geometry.rs`, `spaces_stack.rs` — and since v0.10.3 the popup role is *served* by the compositor: `popup_session.rs` (solve at attach, ack, the live reposition move, dismiss, the parent-death sweep) |
| 13 | `ldp-clipboard` (offers, MIME, streaming, primary, DnD) | Integration-proven | `transfer_fuzz.rs`, `stream_100mb.rs` (real pipe, under limits), `dnd_fsm.rs`, `permission.rs` |
| 14 | `ldp-color` + `ldp-hdr` (transfers, matrices, gamut, BT.2390, ICC v2/v4) | CI-hardened | `kavt.rs` (known-answer vectors), `icc_roundtrip.rs`, `pipeline_props.rs`, `renderer_consistency.rs`, `properties.rs` — and since v0.10.5 the negotiation module (`negotiation.rs`: the effective panel peak, the negotiated canvas ceiling, the per-surface mastering refinement) |
| 15 | `ldp-vrr` (policy engine, refresh windows, tearing opt-in — and since v0.10.8 the quirk ledger) | CI-hardened | `policy_table.rs`, `golden_timelines.rs`, `tearing_isolation.rs`, `panel_integration.rs` — and since v0.10.8: `quirk.rs`'s floor cross-product (the clamp's bounds, both rejections, the CSV grammar), `refresh.rs`'s LFC cadence table and the anti-flap latch state machine, and in the compositor `vrr_floor_session.rs` (the floor's clamp on the wire event and the scheduler's widening, the passthrough entry, the refusal, the uniform collapse with its contrast, the composition — six proofs over the real socket against the mock panel's own advertised window) |
| 16 | `ldp-security`, `ldp-accessibility`, `ldp-power`, `ldp-session` | CI-hardened | `matrix.rs` (18 ops × deny/prompt/grant, exhaustive), `tokens.rs` (forgery/replay corpus), `chain.rs`, `bus_ordering.rs` (5k events), `reanchor.rs` (suspend/resume vs real FrameClock), `dbus_codec.rs` (golden bytes), `logind_flow.rs`, `lock_vt.rs` |
| 17 | `ldp-x11-bridge` + `ldp-wayland-bridge` (real protocol subsets) | Integration-proven | `xeyes.rs`/`xclock.rs` (byte-for-byte lifecycles, Xvfb-free), `weston_terminal.rs`, `bigreq_shm.rs`, `length_bomb.rs`, `serials.rs`, `popup.rs`, `driver_token.rs`, `architecture.rs` — and since v0.10.3 the rootless X11 export: `bridge_x11_rootless_session.rs` (three X windows as first-class LDP surfaces through a real socket, the popup-anchored OR window, the coordinate-fiction input proof, per-window damage isolation, the unmap teardown) with `--x11-rootful` keeping the v0.10.0 whole-screen regression |
| 18 | `ldp-tools` (6 binaries) + 3 examples | Integration-proven | `tools_live.rs` (compiled binaries over the real socket, 7 scenarios), examples' `scenario.rs` suites |
| 19 | `ldp-test`, `fuzz/`, `tests/`, `ldp-bench`, `docs/benchmarks.md` | CI-hardened | `fuzz_ci.rs` (5 targets, panic-hook counting, FD stability), `stress.rs` (15-min full gate: 32 clients, 30,056 sessions, 129,819 frames, FD baseline restored), committed benchmark report |
| 20 | Debian packaging, systemd unit, guides, this matrix | CI-hardened | `scripts/build-deb.sh --verify` (fresh-root dpkg install, installed-binary smoke tests, `systemd-analyze --root` verification); `scripts/verify.sh` |
| 21 | `ldp-remote` + `ldp-remote-gateway` (TCP relay, FD vocabulary, auth, keepalive) | Integration-proven | `remote_loopback.rs` (showcase pixel-exact over the wire, concurrent clients, token fast-fail, length bomb, keepalive teardown, FD stability) |
| 22 | `ldp.capture` protocol + `ldp-png` + `ldp-grab` + renderer perf (CopyPremul, row index) | Integration-proven | `capture.rs` (grabs pixel-exact vs scanout, FD hygiene), `remote_loopback.rs` (capture over the wire pixel-exact), `tools_live.rs` (ldp-grab live), ldp-png round-trip with an independent decoder, `golden_blend.rs` (fast-path bit-exactness) |
| 23 | Release v0.2.0: the version surface + release coherence + the milestone seal (docs, debian, bundle) | CI-hardened | `every_tool_reports_its_version` + the per-package `version_flag_reports_the_release` tests, `scripts/version_check.py` in verify, `scripts/build-deb.sh --verify`, fresh-extraction verify of the bundle |
| 24 | Hardware rendering by default (`ldp-renderer::gles` + `ldp-gpu::gles` + `--renderer`) + the DRM scanout rehearsal | CI-hardened (rendering) / mock-verified + rehearsal (display) | `gles_equivalence.rs` (byte-equal randomized corpus, damage persistence), `gles_stream.rs` (command-stream goldens), `hardware_renderer.rs` (the full session byte-equal under the GL seam, forced-gl typed failure), `gles_hardware` honest degradation, the dumb-ioctl ABI pin, the TEST_ONLY rehearsal on real DRM |
| 25 | The real-KMS serve loop (`ldp-display::driver` + `serve` + mapped dumb scanout + `serve_kms` + teardown) | Mock-verified + byte-equal delivery (the mmap path on CI) / real-path FFI pinned, hardware-run | `serve_loop.rs` (applied enable/disable state, pipeline selection, padded-pitch round-trip through a real mmap), `kms_serve.rs` (shadow-vs-mapped byte-equality over full sessions, teardown's applied disable, hotplug service without clock motion, honest nodeless failures), the MAP_DUMB ABI pin, `real_layer.rs` honest headless |
| 26 | Live output re-arrangement (`lion-compositor::rearrange` + the dark state + scheduler re-anchoring + revoked output objects) | Mock-verified (the same choreography both drivers execute; CI proves the mock half end-to-end through real client sessions) | `hotplug_rearrange.rs` (the swap: migration's applied state + revoked + re-bind + pixel-exact scanout at the new geometry; dark: honest device state, sessions surviving, fences flushed, deferred requests, `failed(no_output)` grabs; relight: deferred answers + the desktop painting again; stability, re-anchoring, renegotiation, bind-while-dark), the scheduler `reanchor` unit suite, `topology_statuses` — and since v0.10.4 the arrangement survives the re-arrangement: `mirror_session.rs` (the clone doctrine — every display at the origin, the per-pixel crop for mixed modes, the newcomer joining the mirror, the hotplug newcomer inheriting the scale doctrine) |
| 30 | The desktop matrix (`ldp-gpu::classify` + the glGetString FFI + the class-aware renderer policy + `--resolution` + `select_pipeline_sized` + `OutputGlobal::select_size` + the desktop benchmark rows + `docs/comparison.md`) | CI-hardened (the class-aware policy and the sized selection are pure logic with every cell pinned; the library defaults keep every prior pixel oracle byte-exact) | `classify.rs`'s exhaustive table (every known renderer string, the software-wins-over-everything rule, the honest Unknown default, the summary truncation), the compositor renderer suite (Auto+llvmpipe refused with the reason, forced-GL-over-llvmpipe honored, unknown/virtual stay hardware), `serve.rs`'s sized-selection suite (the best exact match, the offered-menu failure largest-first, the no-display arm), `resolution_session.rs` (the forced size that is offered serves it — the world, the flagged current mode, the scanout pixels; the honest miss fails with the menu; the migration honors the forced size and falls back honestly without dying), the committed desktop-matrix rows in `benchmarks.md` (1080p High 7.7 ms, 1440p High 12.3 ms, 60 Hz at every size under the no-GPU doctrine) |
| 29 | The steady-state pass (`ldp-renderer::effects`' `MaterialCache`/`FrostMemo` + `StyleState` + the corner bands + the rebuilt blur + the word-level styled paths + the GL memo wiring) | CI-hardened (byte-identity through memoization is an automated gate) | `effects`' Phase 29 oracle suite (the Phase 27 blur/shadow/frost/fold kernels restated verbatim as references and cross-checked over randomized shapes incl. degenerate capsule radii; the exhaustive reciprocal-division proof; the band-soundness sweep; the merged-band single-visit property; both memos' hit/miss semantics), `software::tests::styled_steady_state_is_byte_equal_to_a_cold_render` (warm renderer == cold render, bytes and stats), `gles_styled.rs` and `styled_session.rs` re-running as the byte-equality gates, `benchmarks.md`'s committed phone-frame rows (178.6 → 8.70 ms steady state, same machine) |
| 28 | The positioning shell (`ldp-shell::layout` + `lion-compositor::shell` + the attach-time placement, the migration re-layout, the system dock, `--dock`) | CI-hardened (placement and dock pixels are pinned against hand-derived reference math; the legacy default is a byte-exact regression gate) | `layout.rs`'s unit suite (the carve on every edge, the cascade's step and catch, the clamp, the degenerate outputs), `damage_rules.rs`'s immediate-move rule (`set_position_now` damages old and new cells without a commit), the shell module suite (the ink's haze/pill geometry, the rise's monotone settle and vacate rule), `shell_session.rs` (the desktop cascade pixel-exact with the frost/haze/pill hand values and the mid-rise dynamic; the phone re-anchor on a portrait swap — the doctrine re-resolving mid-session; the legacy regression gate; the plain Minimal dock; the report lines; the whole session byte-equal under the injected reference GL backend) |
| 27 | The Liquid visual engine (`ldp-renderer::style` + `effects` + the styled software/GL paths + `ldp-compositor::spring` + `--effects` and the per-surface policy) | CI-hardened (byte-equality across backends is an automated gate) | `golden_effects.rs` (hand-derived oracles: the corner coverage constants, the hard shadow strip, the veil-over-backdrop frost, damage clipping, the oversize-radius clamp), `gles_styled.rs` (the randomized styled corpus: software vs reference-GL byte-equal over full/partial/multi-rect damage with second frames), `styled_session.rs` (the whole protocol session: the frosted dock pixel-exact against in-file reference math, the plain-path regression gate at Minimal, the tier resolution/report, the session byte-equal under the injected GL backend), the spring physics suite, `style`/`effects` unit suites, the showcase glass-panel session |

## Deliberate scope boundaries

These are the lines between v0.2.0 and the future, stated the way the
code states them (the runtime is honest at each boundary):

1. **Real-hardware pixel delivery.** `ldp-display` implements the full
   atomic-modesetting machinery (object model, commit pipeline, prop
   negotiation, VRR properties, hotplug diffing) and proves it against
   the mock KMS device with golden commit streams. Since Phase 24 the
   shipped `lion-compositor` **rehearses** the whole real-hardware
   bring-up on DRM machines (master takeover, dumb scanout buffers,
   framebuffer registration, a `TEST_ONLY` atomic enable commit the
   kernel validates) and reports each step; the always-on serve loop
   (wake-point event integration, flip landing, presentation pushes
   from a frame thread) is Phase 25. Maturity of the *crate*:
   mock-verified; of the *rehearsal*: real-hardware validated,
   non-destructive; of the *serve path*: staged.
2. **GPU rendering — delivered by default (Phase 24).** The GL
   backend (`ldp-renderer::gles` over `ldp-gpu::gles`'s dlopen'd
   EGL+GLES 2.0 context) composites whenever the machine has a GL
   stack; the selection is hardware-first with the reason carried, and
   the compositor's whole render pipeline flows through it — proven
   byte-equal to the software reference over randomized corpora and
   through the entire client session (the reference evaluator is the
   oracle; real-GPU blending may differ by ±1 LSB in float). The GL
   v1 subset is the 32-bit RGB family, 1:1, `Transform::Normal`;
   YUV uploads, scaled sampling, DMA-BUF zero-copy, and a dedicated
   render thread are the next GL milestones.
3. **Bridges run in-process, exercised by tests — not as system
   services.** Both bridges are library crates; their wire codecs,
   drivers, and conformance suites are complete (xeyes/xclock run
   byte-for-byte; weston-terminal-class clients map, type, resize),
   and `binaries/lion-bridge` ships as the served face — the
   rootless X11 export since v0.10.3 (every top-level window its
   own LDP surface) — but nothing yet hosts them as always-on
   translation daemons with their own socket activation and
   supervision. Staged.
4. **Security broker is a library interface.** The capability matrix,
   token minting/validation, and hash-chained audit are complete and
   property-tested; the daemonized escalation UX (a system prompter
   with its own policy UI) is an integration point LDP defines but
   v0.1.0 does not ship. Staged.
5. **Session management talks the logind protocol, not a live logind.**
   The from-spec D-Bus codec is golden-tested, the session/VT/lock
   state machines are conformance-tested, suspend/resume re-anchoring
   is proven against the real FrameClock — but CI never speaks to a
   real `logind` instance. Mock-verified on the D-Bus boundary.
6. **Remote transport is single-user and token-authenticated.**
   Phase 21 delivered the full relay (`ldp-remote`): whole sessions,
   per-commit pool updates, fences, snapshots, and streaming pipes
   cross the network with pixel-exact delivery (integration-proven).
   The remaining lines are deliberate: the hub attributes sessions to
   the gateway's local account (`SO_PEERCRED` cannot cross a network —
   multi-user remote identity is broker-era work); authentication is a
   bearer token, not TLS (private links until a transport-security
   phase); GPU descriptors (`dmabuf`, explicit-sync fences) are refused
   rather than proxied (Vulkan-era work).
7. **No Vulkan, no VR/AR.** Explicitly post-v0.2.0 roadmap items
   (`docs/roadmap.md`, milestone table), restated here so the absence
   is discoverable.

## Beyond v0.9.0

The roadmap's milestone table names the next directions — the
multi-output layout (several CRTCs, several served pipelines at
once), per-surface material requests over the shell protocol (the
`ldp.shell` service surface's remaining half: toplevel configure
cycles — the popup role and its constraint solving are served since
v0.10.3), YUV/scaled GL
uploads, DMA-BUF zero-copy, a dedicated render thread (the dock's
intro today advances only when client frames flow — self-scheduled
chrome animation is that thread's first job), a Vulkan renderer
(the GPU-class seam it slots into is now proven: the selection
policy already classifies machines and refuses CPU-rasterized GL),
VR/AR exploration, multi-user remote identity (broker-era work),
TLS-class transport security, and dynamic globals (registry
`global_remove` + re-advertise, so the output global itself can come
and go) — and the architecture's
"future directions" section
([`docs/architecture.md`](architecture.md) §24) holds the design space.
The v0.9.0 codebase was shaped for them: the `Renderer` contract
carries two backends behind one selection policy, the serve loop runs
one choreography over both display drivers *and follows the topology*
(the re-arrangement transitions are the seam a multi-output layout
grows on), the transport is a trait seam, the remote relay is in
place with its FD vocabulary, and the protocol's version/capability
negotiation is wired end-to-end (and proven to grow twice: capture
joined after v1.0, and `capture_error::no_output` joined in Phase 26,
both through the full pipeline).
