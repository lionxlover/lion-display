//! `lion-supervisor` — the session-rebuild supervisor (Phase 45, the
//! DWM doctrine served).
//!
//! Windows' quiet achievement in failure handling is the two-layer
//! model: DWM is a *restartable* compositor — when it dies the session
//! flashes and rebuilds, the windows' backing stores survive their
//! compositor, and the user keeps working. macOS has no counterpart (a
//! WindowServer crash logs the user out) — the concentration that
//! gives that process its coherence gives it its blast radius.
//!
//! LDP's answer is this supervisor: a small, std-only process that
//! owns the **session identity** (the pinned abstract socket name) and
//! the **rebuild policy**, and treats `lion-compositor` as the
//! replaceable half — exactly the split Windows made at the driver
//! boundary, here at the process boundary. The clients own their
//! buffers by protocol doctrine (the flip-model symmetry), so a
//! rebuilt compositor plus a reconnecting client *is* the rebuilt
//! session: the DWM story with the LDP twist — every layer of the
//! recovery is open, tested code.
//!
//! The contract:
//!
//! * **Crash → rebuild.** The compositor exiting non-zero (or by a
//!   signal) is a crash: the supervisor backs off, re-execs the same
//!   binary with the same pinned socket name, and increments the
//!   session epoch (passed as `LDP_SESSION_EPOCH` — the child's world
//!   may report it; tools may read it).
//! * **Clean exit → session over.** Exit code 0 is the session ending
//!   (the operator's `--mode drm` teardown, a `--selftest`) — the
//!   supervisor exits 0 with it, no rebuild.
//! * **Operator stop → graceful stop.** SIGTERM/SIGINT to the
//!   supervisor is forwarded to the child (which tears its pipelines
//!   down honestly); the child's exit after an operator-requested
//!   stop is a stop, never a crash — the supervisor exits 0.
//! * **Crash-loop bound.** `--max-restarts N` caps the rebuilds (0 =
//!   unlimited, the DWM doctrine; a crash-looping compositor is
//!   restarted forever until the operator intervenes).
//! * **The pid file.** `--pid-file PATH` writes each child's pid —
//!   the test and operator window into the child process.
//!
//! The supervised child is spawned with the supervisor's stdio (the
//! child's own `--quiet` discipline applies) and the pass-through
//! arguments verbatim (everything after `--`). The supervisor refuses
//! a pass-through `--socket`: the pinned name is the session identity,
//! and it is the supervisor's alone to own.

#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use lion_compositor::sys;

/// The poll cadence of the watch loop (the wake-point doctrine's
/// coarse cousin: 10 ms of real time between child checks).
const WATCH_POLL_MS: u64 = 10;
/// How long the graceful stop waits for the child's own teardown
/// before escalating to SIGKILL.
const GRACEFUL_STOP_SECS: u64 = 5;

/// The supervisor's configuration (the argv's mirror).
#[derive(Debug)]
struct Supervisor {
    /// The pinned abstract socket name — the session identity.
    socket: String,
    /// The compositor binary (a PATH lookup by default).
    compositor: String,
    /// The rebuild backoff.
    backoff_ms: u64,
    /// The rebuild cap (0 = unlimited, the DWM doctrine).
    max_restarts: u32,
    /// Where each child's pid is written (the test/operator window).
    pid_file: Option<PathBuf>,
    /// The child's arguments, verbatim (everything after `--`).
    pass_through: Vec<String>,
}

impl Supervisor {
    /// Parse argv; `Err(text)` is the usage error (exit 2 with text).
    fn parse() -> Result<Self, String> {
        let mut supervisor = Supervisor {
            socket: "lion-desktop".to_owned(),
            compositor: "lion-compositor".to_owned(),
            backoff_ms: 250,
            max_restarts: 0,
            pid_file: None,
            pass_through: Vec::new(),
        };
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--socket" => {
                    supervisor.socket = args.next().ok_or("--socket needs a value")?;
                }
                "--compositor" => {
                    supervisor.compositor = args.next().ok_or("--compositor needs a path")?;
                }
                "--backoff-ms" => {
                    let value = args.next().ok_or("--backoff-ms needs a value")?;
                    supervisor.backoff_ms = value.parse().map_err(|_| {
                        format!("bad backoff '{value}' (a whole number of milliseconds)")
                    })?;
                }
                "--max-restarts" => {
                    let value = args.next().ok_or("--max-restarts needs a value")?;
                    supervisor.max_restarts = value.parse().map_err(|_| {
                        format!("bad restart cap '{value}' (a whole number, 0 = unlimited)")
                    })?;
                }
                "--pid-file" => {
                    supervisor.pid_file =
                        Some(PathBuf::from(args.next().ok_or("--pid-file needs a path")?));
                }
                "--" => {
                    supervisor.pass_through = args.collect();
                    // The pinned name is the supervisor's alone: a
                    // pass-through `--socket` would fight the pin.
                    if supervisor.pass_through.iter().any(|a| a == "--socket") {
                        return Err("the pass-through args carry --socket — the pinned name is \
                             the supervisor's (drop it; the child receives it from here)"
                            .to_owned());
                    }
                    break;
                }
                "--help" | "-h" => {
                    print_help();
                    std::process::exit(0);
                }
                "--version" | "-V" => {
                    println!("lion-supervisor {}", env!("CARGO_PKG_VERSION"));
                    std::process::exit(0);
                }
                other => {
                    return Err(format!("unknown argument '{other}' (try --help)"));
                }
            }
        }
        Ok(supervisor)
    }

    /// The rebuild loop — the whole supervisor.
    fn run(&self) -> ExitCode {
        // The operator's stop signal (SIGTERM/SIGINT → the atomic flag
        // the sys module installs; the same discipline the compositor
        // itself serves under).
        if let Err(e) = sys::install_shutdown_handlers() {
            eprintln!("lion-supervisor: signal handlers: {e}");
            return ExitCode::FAILURE;
        }
        let mut epoch: u64 = 0;
        let mut rebuilds: u32 = 0;
        loop {
            let mut child = self.spawn(epoch);
            let pid = child.id();
            self.write_pid(pid);
            eprintln!(
                "lion-supervisor: session up (socket {}, epoch {epoch}, pid {pid})",
                self.socket
            );
            // The watch loop: poll the child, honor the operator's stop.
            let crash: Option<std::process::ExitStatus> = loop {
                if sys::shutdown_requested() {
                    // The operator asked: forward the stop, take the
                    // child's exit as a stop whatever its story (the
                    // headless accept loop has no SIGTERM path of its
                    // own — the *supervisor* is the session's signal
                    // authority).
                    stop_child(&mut child);
                    self.remove_pid_file();
                    return ExitCode::SUCCESS;
                }
                match child.try_wait() {
                    Ok(Some(status)) => {
                        if status.success() {
                            // The session ended cleanly (an operator
                            // teardown, a selftest): over, not a crash.
                            self.remove_pid_file();
                            eprintln!(
                                "lion-supervisor: session over (socket {}, exit 0)",
                                self.socket
                            );
                            return ExitCode::SUCCESS;
                        }
                        break Some(status);
                    }
                    Ok(None) => {
                        std::thread::sleep(Duration::from_millis(WATCH_POLL_MS));
                    }
                    Err(e) => {
                        eprintln!("lion-supervisor: wait: {e}");
                        self.remove_pid_file();
                        return ExitCode::FAILURE;
                    }
                }
            };
            // The crash: count, bound, back off, rebuild.
            rebuilds = rebuilds.saturating_add(1);
            if self.max_restarts != 0 && rebuilds > self.max_restarts {
                eprintln!(
                    "lion-supervisor: rebuild cap reached ({}) — giving up (socket {})",
                    self.max_restarts, self.socket
                );
                self.remove_pid_file();
                return ExitCode::FAILURE;
            }
            let why = crash.map_or_else(|| "unknown".to_owned(), |status| status.to_string());
            eprintln!(
                "lion-supervisor: rebuild {rebuilds}: the compositor died ({why}) — \
                 re-binding {} in {} ms",
                self.socket, self.backoff_ms
            );
            let deadline = Instant::now() + Duration::from_millis(self.backoff_ms);
            while Instant::now() < deadline {
                if sys::shutdown_requested() {
                    self.remove_pid_file();
                    return ExitCode::SUCCESS;
                }
                std::thread::sleep(Duration::from_millis(WATCH_POLL_MS.min(50)));
            }
            epoch = epoch.saturating_add(1);
        }
    }

    /// Spawn one compositor generation: the pinned socket, the
    /// pass-through args, the epoch env, the inherited stdio.
    fn spawn(&self, epoch: u64) -> std::process::Child {
        Command::new(&self.compositor)
            .arg("--socket")
            .arg(&self.socket)
            .args(&self.pass_through)
            .env("LDP_SESSION_EPOCH", epoch.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap_or_else(|e| {
                eprintln!("lion-supervisor: cannot exec '{}' — {e}", self.compositor);
                std::process::exit(1);
            })
    }

    /// Write the child's pid (the test/operator window).
    fn write_pid(&self, pid: u32) {
        if let Some(path) = &self.pid_file {
            // A failed pid-file write is reported, never fatal: the
            // session's truth is the socket, not the file.
            if let Err(e) = std::fs::write(path, format!("{pid}\n")) {
                eprintln!("lion-supervisor: pid file: {e}");
            }
        }
    }

    /// Remove the pid file at session end.
    fn remove_pid_file(&self) {
        if let Some(path) = &self.pid_file {
            // The file may already be gone (the directory cleaned
            // under us): removal is best-effort.
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Stop the child gracefully: SIGTERM, wait out the graceful window,
/// then SIGKILL. The child's own teardown discipline (DRM master
/// dropped, buffers destroyed, display dark) is given the time it
/// needs; the SIGKILL is the last resort, named as one.
///
/// The stop flag is latched (one atomic store, no counting — the
/// signal-safe doctrine), so the graceful window waits out the full
/// duration rather than re-reading it: a second operator signal
/// during the window is the *same* stop, not an escalation.
fn stop_child(child: &mut std::process::Child) {
    // The signal itself lives in the audited syscall layer (`sys`),
    // the crate's one home for `unsafe` — the same discipline every
    // other syscall here serves under.
    if let Err(e) = sys::terminate_child(child.id()) {
        eprintln!("lion-supervisor: forwarding the stop: {e}");
    }
    let deadline = Instant::now() + Duration::from_secs(GRACEFUL_STOP_SECS);
    while Instant::now() < deadline {
        match child.try_wait() {
            // Reaped (the graceful exit) or unwaitable (the wait itself
            // failed): either way the stop is over.
            Ok(Some(_)) | Err(_) => return,
            Ok(None) => {
                std::thread::sleep(Duration::from_millis(WATCH_POLL_MS));
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn print_help() {
    println!(
        "lion-supervisor — the session-rebuild supervisor: the DWM doctrine over LDP\n\
         \n\
         USAGE:\n\
         \x20 lion-supervisor [OPTIONS] [-- <lion-compositor args>]\n\
         \n\
         The supervisor pins the session's abstract socket, spawns the\n\
         \x20 compositor, and rebuilds it on a crash — the same socket, a new\n\
         \x20 epoch (LDP_SESSION_EPOCH). Clients reconnect and re-create their\n\
         \x20 windows: the session flashes and rebuilds, the desktop survives.\n\
         \n\
         OPTIONS:\n\
         \x20 --socket <NAME>         the pinned abstract socket (default: lion-desktop)\n\
         \x20 --compositor <PATH>     the compositor binary (default: lion-compositor on PATH)\n\
         \x20 --backoff-ms <MS>       the rebuild delay (default: 250)\n\
         \x20 --max-restarts <N>      the rebuild cap; 0 = unlimited, the DWM doctrine (default: 0)\n\
         \x20 --pid-file <PATH>       each child's pid, for tests and operators\n\
         \x20 --help | -h             this text\n\
         \x20 --version | -V          the release\n\
         \n\
         Everything after `--` passes to the compositor verbatim\n\
         \x20 (e.g. `-- --mode drm --renderer gl`). A pass-through `--socket`\n\
         \x20 is refused: the pinned name is the supervisor's alone."
    );
}

fn main() -> ExitCode {
    match Supervisor::parse() {
        Ok(supervisor) => supervisor.run(),
        Err(text) => {
            eprintln!("lion-supervisor: {text}\n\ntry --help");
            ExitCode::from(2)
        }
    }
}
