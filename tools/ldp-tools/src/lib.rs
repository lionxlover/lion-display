//! # ldp-tools — the operator toolchain of the LDP display stack
//!
//! Phase 18 of the roadmap: the shared library behind the six operator
//! binaries and the example applications.
//!
//! ```text
//! ldp-info        protocol + live-session + display/GPU introspection
//! ldp-debug       live event tracer + scheduler-timeline replay
//! ldp-validate    spec-set compilation + live schema cross-check
//! ldp-profiler    frame-pipeline measurement (live + replay)
//! ldp-audit       hash-chained audit-log reader / verifier
//! ldp-input-debug evdev dump analysis (raw → frames → normalized)
//! ```
//!
//! Every tool's logic lives here as a library function (`tools::*::run`)
//! so the integration suites exercise the same code the binaries run;
//! the `src/bin/*.rs` adapters are argv parsing and exit codes only.
//!
//! ## Doctrine
//!
//! * **Honest degradation.** The reference `lion-compositor` (the Phase 10
//!   vertical slice) advertises exactly four globals — `ldp.core.registry`,
//!   `ldp.core.compositor`,
//!   `ldp.core.shm`, `ldp.core.output`. A tool that needs an interface the
//!   server does not advertise says so and exits cleanly; it never binds a
//!   non-advertised global (that is a fatal protocol error by design) and
//!   never fabricates the missing functionality.
//! * **Live first, offline always.** Tools whose core function is offline
//!   (spec validation, evdev analysis, audit verification) additionally
//!   implement a `--live` mode that connects, inspects the registry, and
//!   reports what the running server actually offers — the Phase 18 exit
//!   criterion is "every tool runs against the Phase 10 compositor".
//! * **Determinism over decoration.** Output is plain text on a
//!   [`std::io::Write`]; timestamps print in the compositor's monotonic
//!   nanoseconds; nothing reads a wall clock unless it is measuring wall
//!   latency and says so.
//! * **The sys seam.** Every `unsafe` in this crate lives in [`sys`] with a
//!   `SAFETY` comment (the `ldp-transport` precedent); all other modules are
//!   `#![forbid(unsafe_code)]`, so the crate root does not forbid.
//!
//! ## Socket discovery convention
//!
//! `lion-compositor` listens on an abstract-namespace Unix socket whose
//! name is `--socket NAME` on the server side. Tools resolve the same
//! name in this order: the `--socket NAME` flag, then the `LDP_SOCKET`
//! environment variable; with neither, live mode is a usage error that
//! names the convention (there is no default name: the compositor's
//! own default embeds its pid). See [`socket::resolve`].

pub mod args;
pub mod audit_log;
pub mod error;
pub mod session;
pub mod socket;
pub mod sys;
pub mod tools;
pub mod value_fmt;
