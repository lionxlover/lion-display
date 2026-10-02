# LDP Threat Model

Status: **Phase 1 baseline.** Scope: the LDP protocol, the reference
compositor, the bridges, and the tooling. Out of scope: the Linux kernel,
GPU drivers, and applications themselves (they are *adversaries* or *assets*
from LDP's point of view).

---

## 1. Assets

1. **Pixels** — window contents of every application (passwords, messages,
   medical records…). Confidentiality requirement: no application can read
   another's pixels without a traceable grant.
2. **Input** — the global keystroke stream (includes passwords typed into
   every window) and pointer/touch/stylus streams.
3. **Clipboard / primary selection** — cross-application data.
4. **Screen metadata** — window titles, app-ids, layout, output inventory
   (reveals user activity patterns).
5. **Availability** — the compositor must keep rendering; a malicious or
   crashed client must not wedge the session.
6. **Integrity of the audit trail** — security decisions must be
   reconstructible and tamper-evident.

## 2. Adversaries

| Adversary | Capability | Motivation |
|---|---|---|
| Malicious app | full LDP client, user uid, arbitrary messages, malicious buffers/fences, crashes itself at will | data theft, keylogging, DoS |
| Compromised app | same, plus a stolen-but-expired token replay attempt | persistence |
| Compromised bridge | same as app + holds a legitimate `bridge` scope token | foreign-client abuse |
| Local non-graphical process | can connect to the socket path, try to impersonate | privilege probing |
| Remote attacker | none directly (no network surface in v1) | via other services |

Trust anchors: the kernel (FD passing, SO_PEERCRED, DMA-BUF isolation),
logind (session/device ownership), the session broker (manifests, prompts,
token minting) — the broker is the only component allowed to make grant
decisions, and it runs as its own uid with a private socket.

## 3. Permission matrix (layered policy)

Legend: **M** = manifest scope required · **E** = runtime escalation prompt
(either satisfies) · **F** = free (no grant) · **—** = impossible by protocol.

| Operation | Requirement | Notes |
|---|---|---|
| render own windows | F | the basic contract |
| read own buffers back | F | |
| keyboard/pointer events for focused window | F | focus-gated delivery |
| global input grab | M (`input_grab`) | popup menus use transient grabs, free |
| clipboard write (focused) | F | selection bound to seat focus |
| clipboard read | M (`clipboard_read`) | paste is content exfiltration vector |
| primary selection read | M (`clipboard_read`) | same scope |
| screenshot (single window) | M+E (`screenshot`) | window-targeted |
| screenshot (screen) / recording | M+E (`screenshot`/`screen_record`) | prompt re-shown periodically |
| synthetic input injection | M+E (`input_inject`) | audited per batch |
| global shortcuts | M (`global_shortcut`) | |
| display reconfiguration | M (`configure_display`) | settings daemon |
| workspace management (foreign windows) | M (`manage_workspaces`) | shell/switcher |
| a11y event stream subscription | M (`a11y_control`) | screen readers |
| reading other clients' window titles | — | not exposed on the wire at all |
| mapping other clients' buffers | — | per-connection namespaces |
| reading audit log | E (`audit_read`) | administrators |

Deny-by-default: anything not listed is denied with `unauthorized` and an
audit record. Tokens are 256-bit random, scope-bounded, expiring, revocable,
compared in constant time, and never logged in raw form.

## 4. Attack surface & mitigations

### 4.1 Wire parsing (untrusted bytes + FDs)

* Lengths are validated before allocation (bounded work per malformed
  message — see `protocol.md` §9 validation pipeline).
* Tagged arguments allow schema-independent structural validation; signature
  checks then run before semantic dispatch.
* FD count cross-checked between header and SCM_RIGHTS; FD bombs (many FDs in
  a small message) are closed immediately on rejection; per-client FD ceiling.
* ID confusion is impossible: per-connection namespaces + generation check
  (`stale_object`), interface type check (`invalid_interface`).
* Unicode: strings must be valid UTF-8 (no NUL-within tricks; length excludes
  NUL); MIME/app-id charset validated per semantics.

### 4.2 DoS via resource exhaustion

Per-client ceilings: objects, buffer bytes, FDs, event queue bytes,
message size, region rects. Backpressure coalescing never propagates one
client's stall to others (per-connection queues; no shared locks across
dispatch). Render-time damage is clipped to output bounds; a client sending
huge damage gets its damage clipped, not the compositor's render time blown.

### 4.3 GPU abuse

Fences: every buffer handoff is fenced; a client that never signals an
acquire fence stalls only its own surface (fence wait is per-surface,
timeouts escalate to `frame_dropped` + surface de-scheduling, never a
compositor hang). Buffers are size- and stride-validated on attach; modifier
support is checked against the device before import.

### 4.4 Input leakage

Focus-gated delivery: keyboard events go to the focused surface only; global
grabs require scope; injection requires prompt. Keymaps are shared
server→client (read-only FD), never client→server.

### 4.5 Screen capture abuse

Capture paths exist only behind `screenshot`/`screen_record` tokens with
mandatory prompts; every capture emits an audit record including window
target, client id, and timestamp. Indicator policy (recording dot) is a
Phase 16 hardening item — listed here so it is tracked, not hidden.

### 4.6 Bridge abuse

Bridges hold wide but *logged* authority. Their child windows are marked with
the bridge's app-id so users can attribute prompts to the responsible app.
Bridge tokens expire and are re-minted per login.

### 4.7 Crash robustness (availability)

The server never executes client code; dispatch is table-driven; client
death is detected by socket HUP + pidfd, resources reclaimed in bounded
time (object store sweep), grabs released, clipboard offers cancelled.
Server crash recovery: the session broker restarts the compositor under
systemd; clients detect the closed socket and reconnect (the protocol is
stateless across connections by design).

## 5. Audit log

Append-only JSONL at `$XDG_STATE_HOME/ldp/audit.jsonl`: one record per
grant/denial/revocation/capture/injection event, hash-chained
(`prev = sha256(prev || record)`), written synchronously by the broker.
`ldp-audit` verifies the chain. Records contain client identity
(uid, pid at grant time, app-id, sandbox flavor), scope, decision, and
justification (manifest hash or prompt id) — never raw token bytes.

## 6. Residual risks (tracked, honest)

* Indicator-of-capture UX (screen recording dot) — Phase 16.
* Multi-GPU cross-device DMA-BUF attack classes depend on driver behavior;
  LDP mitigates with explicit fences and test-imports, but driver bugs are
  out of scope.
* The bridges' authority is wide; sandboxing of bridge children is a
  packaging-level control (Phase 20).
* Prompt fatigue: escalation UX quality is a product risk, not a protocol
  risk; the broker enforces per-app granularity to blunt it.
