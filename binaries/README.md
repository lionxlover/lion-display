# binaries

Compositor and bridge executables.

* `lion-compositor` (Phase 10) — the vertical-slice example compositor:
  full LDP service over the mock KMS device in headless mode (the CI
  vehicle), an honest DRM probe mode (`ldp-gpu` node discovery +
  `ldp-display` backend report; KMS pixel delivery on real nodes
  arrives with the GPU phases), and `auto` (probe, fall back). See its
  crate docs for the headless time doctrine and the CLI (`--help`).
* `lion-supervisor` (Phase 45) — the session-rebuild supervisor: the
  DWM crash-recovery doctrine over LDP. Pins the session's abstract
  socket, spawns the compositor, and on a crash backs off and
  re-execs on the same pin (a fresh `LDP_SESSION_EPOCH`) — the
  session flashes and rebuilds, a reconnecting client presents again.
  The operator's SIGTERM is the graceful session end (forwarded,
  honored); `--max-restarts` bounds the crash loop; `--pid-file`
  opens the operator/test window. See `--help`.
* `ldp-wayland-bridge`, `ldp-x11-bridge` — Phase 17.
