# packaging

Debian packaging and the systemd unit for the LDP stack.

## Layout

* `debian/` (repository root) — the Debian source package control
  files: `control` (source `lion-display`; binary packages
  `lion-compositor` + `ldp-tools`), `changelog`, `copyright`, `rules`,
  `source/format` (native 3.0). The rules are deliberately
  debhelper-free — plain POSIX make driving cargo + `dpkg-gencontrol` +
  `dpkg-deb` — with `Rules-Requires-Root: no`, so the build runs
  unprivileged anywhere `dpkg-dev` and a Rust 1.75+ toolchain exist.
* `systemd/lion-compositor.service` — the shipped unit (installed to
  `/usr/lib/systemd/system/`, usr-merge layout). Hardened by default;
  see the unit's comments for the override pattern.

## Building and verifying

```console
$ ./scripts/build-deb.sh            # release build + package + verify
```

Stages can be run separately (`--build`, `--package`, `--verify`) —
useful when the release build's LTO exceeds a caller's tool timeout.
Artifacts land in `dist/`:

* `lion-compositor_0.7.0_amd64.deb`
* `ldp-tools_0.7.0_amd64.deb`
* `.changes` / `.buildinfo` from `dpkg-buildpackage`

The `--verify` stage is the Phase 20 exit criterion made executable:
both `.deb` files install with **real dpkg** against a pristine root
inside a user namespace (`unshare -rm`, the container-safe equivalent
of a fresh machine), installed status is asserted, every installed
binary is smoke-tested (`--selftest`, `--help` walks), and the systemd
unit passes `systemd-analyze --root=... verify` — which also checks
the unit's `Documentation=` targets exist in the installed root.
