# tests

The Phase 19 cross-crate integration gates (`ldp-integration`) — every
suite drives the **real in-process compositor** (the Phase 10 vertical
slice) through the shared harness in `src/harness.rs`.

| Suite | What it proves |
|---|---|
| `stress` | THE stress gate: 32 concurrent clients, mixed workloads (frame cycles with real memfd pools, sync round-trips, buffer churn, crash-and-reconnect). After the run: a full canary session completes, frames advanced, both session-end paths exercised, the scene drained, the FD table back to baseline. Compressed by default; `LDP_STRESS_FULL=1` runs the 15-minute gate. |
| `crash` | The crash corpus: clients vanishing at every protocol cut point (after handshake / shm bind / pool / buffer / surface / attach / commit, and mid-frame at the raw transport level) plus a 24-round churn — each crash must drain and leave the server canary-healthy. |
| `hotplug` | The hotplug simulation: mock-KMS connector churn injected into the compositor's own device (connect/unplug/replug) while a client keeps committing and presenting, and the deterministic `Reprobe` topology diffs hold at the device level. |

Track record: the crash corpus surfaced a real leak —
`Scene::drop_client` did not clear the crashed client's pending buffer
releases, so the next flip parked a `release` event for a dead client,
re-creating its outbox queue post-mortem (and leaking the entry's
eventfd). Fixed in `binaries/lion-compositor/src/scene.rs`; the corpus
is the regression test.

Per-crate suites still live inside each crate (`crates/*/tests/`);
this package is the cross-crate layer only.
