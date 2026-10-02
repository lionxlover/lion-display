# Deny-by-default policy

LDP is the component of LionOS that owns pixels, input, clipboard and screen
metadata for every application. Security reports are treated with maximum
priority.

## Reporting a vulnerability

* **Do not open public issues for security bugs.**
* Report privately to **security@lionos.example** (replace with the address of
  the LionOS security team; if you are reading a downstream mirror, report to
  the party distributing your build).
* Include: affected crate and version, threat model assumptions, reproduction
  steps, and impact assessment. A proof-of-concept is welcome but not required.
* Target response times: acknowledgement within 72 hours, assessment within
  14 days. Coordinated disclosure with a 90-day embargo by default.

## Scope

In scope: all crates in this repository, the protocol design
(`spec/`, `docs/`), and the toolchain (`ldpc`).

Out of scope: downstream distributions, the Linux kernel, GPU driver bugs,
and issues in applications running atop LDP.

## Security design references

* `docs/threat-model.md` — adversary model, permission matrix, attack surface.
* `docs/architecture.md` §11–§14 — capability enforcement and audit architecture.
* Layered policy: signed **manifest baseline** at launch plus **runtime user
  escalation** (broker prompts) for the most sensitive operations; every
  decision is written to the tamper-evident audit log.

## Hardening commitments

1. The core protocol crates are `#![forbid(unsafe_code)]`.
2. `unsafe` is permitted only in isolated, reviewed, documented blocks
   (transport syscalls, GPU/driver interfaces) — grep for `// SAFETY:`.
3. Wire parsing validates length, alignment, FD count, UTF-8, and enum ranges
   before any allocation proportional to untrusted sizes.
4. Every permission check defaults to *deny*; audit records are appended
   synchronously for grant, denial, and revocation events.
5. Fuzzing (`fuzz/`, Phase 19) covers codec, transport framing, and protocol
   dispatch from untrusted bytes.
