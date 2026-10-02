# fuzz

The Phase 19 deterministic fuzzers — five targets, one gate, zero
external frameworks.

| Target | Drives | Invariants beyond "never panics" |
|---|---|---|
| `codec` | the `ldp-protocol` wire codec | every spec-synthesized valid message round-trips exactly and passes strict signature validation; mutants decode in strict and tolerant modes against varying declared FD-table sizes |
| `transport` | the `ldp-transport` framing reader over **real socketpairs** | fragmented delivery, declared FD-count lies, real `SCM_RIGHTS` batches, half-delivered crash shapes; the process FD table is byte-stable across the whole run |
| `dispatch` | a real `ldp-server` `Server` (null dispatcher) | fuzzed bursts (mutated requests, aligned garbage, forged target ids, event-opcode smuggling) — sessions die with structured verdicts, are fully reclaimed, FD baseline restored |
| `x11` | the `ldp-x11-bridge` request framing | short + BIG-REQUESTS framing, both endiannesses; consumed sizes always match payload + header |
| `wayland` | the `ldp-wayland-bridge` wire codec against the pinned tables | valid round-trips; incomplete bounds always exceed the buffered bytes and make strict progress when honored |

Everything is seed-addressed (`ldp-test::rng::SplitMix64` +
`ldp-test::mutate`): a finding names a seed and replays exactly.

The CI gate (`tests/fuzz_ci.rs`) runs all five targets under one
serialized test — a counting panic hook covers every thread including
the server's session threads — and asserts both accept and reject
paths were exercised on every target. `LDP_FUZZ_SCALE=<n>` multiplies
all iteration counts for soak runs.

Track record: the gate's very first run surfaced a real length-bomb
DoS in the Wayland bridge (an unbounded input buffer waiting for a
~2 GiB message that could never complete) — fixed with the
one-large-frame ceiling and regression-tested in
`crates/ldp-wayland-bridge/tests/length_bomb.rs`.
