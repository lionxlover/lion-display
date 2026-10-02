//! THE Phase 19 stress gate (`docs/roadmap.md`): 32 clients, mixed
//! workloads, stable.
//!
//! The workload model — every client thread runs *sessions* against
//! the one in-process compositor:
//!
//! * **connect** (handshake), bind `shm` + `compositor`, build a real
//!   memfd pool and buffer,
//! * a run of 2–5 **frame cycles** (`frame → frame_target → attach →
//!   damage → commit → committed`), interleaved with sync round-trips
//!   and occasional buffer destroy/recreate churn,
//! * session end: **clean** (destroy chain, orderly disconnect) or
//!   **crash** (the connection drops abruptly — the reclamation path),
//! * then reconnect and repeat.
//!
//! Stability assertions after every gate run:
//!
//! * a full healthy canary session completes (the server is unwedged),
//! * the compositor rendered continuously (`frames` advanced),
//! * both session-end paths were exercised,
//! * the scene drained (no leaked routes/outboxes),
//! * the process FD table returned to its baseline (no leaks).
//!
//! Modes: compressed (default — bounded session count, CI seconds)
//! and **full** (`LDP_STRESS_FULL=1` — the 15-minute wall-clock gate;
//! the closure run's numbers are recorded in `docs/benchmarks.md`).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ldp_core::geometry::Rect;
use ldp_test::rng::SplitMix64;
use ldp_transport::count_open_fds;

use crate::harness::{Client, CompositorHandle};

/// The gate's client count (the roadmap's number).
pub const CLIENTS: usize = 32;

/// The full gate's duration.
pub const FULL_DURATION: Duration = Duration::from_secs(15 * 60);

/// One gate run's outcome.
#[derive(Clone, Copy, Debug)]
pub struct StressOutcome {
    /// `"compressed"` or `"full"`.
    pub mode: &'static str,
    /// Wall-clock seconds the gate ran.
    pub wall_secs: f64,
    /// Concurrent clients.
    pub clients: usize,
    /// Sessions completed (clean + crash).
    pub sessions: u64,
    /// Frame cycles committed.
    pub cycles: u64,
    /// Sync round-trips.
    pub syncs: u64,
    /// Sessions ended by abrupt disconnect.
    pub crashes: u64,
    /// Frames the compositor rendered during the gate.
    pub frames: u64,
}

impl StressOutcome {
    /// One-line summary for logs and reports.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "stress[{}]: {} clients, {} sessions ({} crashes), {} cycles, \
             {} syncs, {} frames, {:.1}s wall",
            self.mode,
            self.clients,
            self.sessions,
            self.crashes,
            self.cycles,
            self.syncs,
            self.frames,
            self.wall_secs
        )
    }
}

/// Shared run counters.
#[derive(Default)]
pub struct Counters {
    /// Sessions completed.
    pub sessions: AtomicU64,
    /// Frame cycles committed.
    pub cycles: AtomicU64,
    /// Sync round-trips.
    pub syncs: AtomicU64,
    /// Abrupt-disconnect session ends.
    pub crashes: AtomicU64,
}

/// Run the stress gate.
///
/// # Panics
///
/// On any stability violation: a healthy protocol step failing, the
/// canary not completing, frames not advancing, only one session-end
/// path exercised, the scene not draining, or the FD table leaking.
#[allow(clippy::too_many_lines)] // one straight-line harness loop
pub fn run_gate(full: bool) -> StressOutcome {
    let t0 = Instant::now();
    let handle = CompositorHandle::start("stress");
    let frames_start = handle.frames();
    // The FD baseline after the compositor's own resources exist.
    let fd_baseline = count_open_fds().expect("fd count");

    let deadline = if full { Some(t0 + FULL_DURATION) } else { None };
    // Compressed budget: sessions per client (crash reconnects make
    // the total higher); full mode runs until the wall clock says stop.
    let sessions_per_client: u64 = if full { u64::MAX } else { 8 };

    let counters = Arc::new(Counters::default());
    let addr = handle.addr.clone();
    let deadline = Arc::new(deadline);

    let mut threads = Vec::with_capacity(CLIENTS);
    for index in 0..CLIENTS {
        let addr = addr.clone();
        let counters = Arc::clone(&counters);
        let deadline = Arc::clone(&deadline);
        threads.push(
            std::thread::Builder::new()
                .name(format!("ldp-stress-{index}"))
                .spawn(move || {
                    let mut rng = SplitMix64::new(0x57E5_5000 + index as u64);
                    drive_client(
                        &addr,
                        &mut rng,
                        &counters,
                        sessions_per_client,
                        deadline.as_ref().as_ref(),
                    );
                })
                .expect("client thread"),
        );
    }
    for t in threads {
        t.join().expect("client thread must not panic");
    }

    // Settle: give reclamation a moment, then demand a fully drained
    // scene (no leaked routes or outboxes from any of the sessions).
    let drain_deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < drain_deadline {
        let drained = handle.world(|w| w.scene.routes.is_empty() && w.outboxes.is_empty());
        if drained {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        handle.world(|w| w.scene.routes.is_empty() && w.outboxes.is_empty()),
        "the scene did not drain after the stress run"
    );

    // The canary: a full healthy session on the abused server.
    Client::canary_session(&handle);

    let frames_end = handle.frames();
    let sessions = counters.sessions.load(Ordering::Relaxed);
    let crashes = counters.crashes.load(Ordering::Relaxed);
    assert!(sessions >= CLIENTS as u64, "sessions: {sessions}");
    assert!(
        crashes > 0 && crashes < sessions,
        "both session-end paths must be exercised (crashes {crashes} of {sessions})"
    );
    assert!(
        counters.cycles.load(Ordering::Relaxed) >= 64,
        "frame cycles: {}",
        counters.cycles.load(Ordering::Relaxed)
    );

    // FD hygiene: the table returns to its baseline and stays there
    // (reclamation lags the scene drain by a session-thread wakeup,
    // so poll to stability rather than demanding instant quiet).
    let fd_settled = || -> usize {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut last = count_open_fds().expect("fd count");
        while Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(150));
            let now = count_open_fds().expect("fd count");
            if now == last {
                return now;
            }
            last = now;
        }
        last
    };
    assert_eq!(
        fd_settled(),
        fd_baseline,
        "FD table changed across the stress run (leak)"
    );

    let outcome = StressOutcome {
        mode: if full { "full" } else { "compressed" },
        wall_secs: t0.elapsed().as_secs_f64(),
        clients: CLIENTS,
        sessions,
        cycles: counters.cycles.load(Ordering::Relaxed),
        syncs: counters.syncs.load(Ordering::Relaxed),
        crashes,
        frames: frames_end.saturating_sub(frames_start),
    };
    let floor = if full { 30_000 } else { 120 };
    assert!(
        outcome.frames >= floor,
        "the compositor rendered only {} frames ({})",
        outcome.frames,
        outcome.summary()
    );
    outcome
}

/// One client thread: sessions until the budget is spent.
fn drive_client(
    addr: &ldp_transport::UnixAddr,
    rng: &mut SplitMix64,
    counters: &Counters,
    sessions_budget: u64,
    deadline: Option<&Instant>,
) {
    let mut done = 0u64;
    while done < sessions_budget {
        if let Some(d) = deadline {
            if Instant::now() >= *d {
                return;
            }
        }
        let crash = rng.below(6) == 0;
        run_session(addr, rng, counters, crash, deadline);
        done += 1;
    }
}

/// One session: connect, cycles, then a clean or abrupt end.
fn run_session(
    addr: &ldp_transport::UnixAddr,
    rng: &mut SplitMix64,
    counters: &Counters,
    crash: bool,
    deadline: Option<&Instant>,
) {
    let mut client = Client::connect(addr);
    let shm = client.bind("ldp.core.shm");
    client.wait_until(|rec| rec.records.iter().filter(|r| r.event == "format").count() >= 2);
    let fill = rng.byte() | 0x20;
    let pool = client.create_pool(&shm, Client::QUAD_POOL_BYTES, fill);
    let mut buffer = client.create_buffer(&pool, 0, 2, 2, 8, 0x3432_5258);
    let compositor = client.bind("ldp.core.compositor");
    let surface = client.create_surface(&compositor);

    let cycles = 2 + rng.below(4);
    for cycle in 1..=cycles {
        if let Some(d) = deadline {
            if Instant::now() >= *d {
                break;
            }
        }
        let frame_id = cycle as u64;
        client.frame(&surface, frame_id);
        client.wait_until(|rec| rec.records.iter().any(|r| r.event == "frame_target"));
        client.attach(&surface, &buffer);
        client.damage(&surface, &[Rect::new(0, 0, 2, 2)]);
        let cookie = rng.next_u32();
        client.commit(&surface, cookie);
        client.wait_until(|rec| {
            rec.records.iter().any(|r| {
                r.event == "committed" && r.args[0] == ldp_core::wire::Value::Uint32(cookie)
            })
        });
        counters.cycles.fetch_add(1, Ordering::Relaxed);

        // Interleave sync round-trips (the queue-drain discipline).
        if rng.below(3) == 0 {
            client.sync();
            counters.syncs.fetch_add(1, Ordering::Relaxed);
        }
        // Buffer churn: destroy and recreate the delivery vehicle.
        if rng.below(4) == 0 && cycle < cycles {
            client.destroy(&buffer);
            buffer = client.create_buffer(&pool, 0, 2, 2, 8, 0x3432_5258);
        }
    }

    counters.sessions.fetch_add(1, Ordering::Relaxed);
    if crash {
        // Abrupt end: drop the connection with objects live — the
        // reclamation path (no destroy protocol, the socket just
        // closes).
        counters.crashes.fetch_add(1, Ordering::Relaxed);
        drop(client);
    } else {
        client.destroy(&buffer);
        client.destroy(&surface);
        client.wait_until(|rec| rec.records.iter().any(|r| r.event == "destroyed"));
        client.destroy(&pool);
    }
}

/// The serialized gate entry (FD and scene assertions are
/// process-wide).
#[cfg(test)]
mod tests {
    use super::*;

    // The shared cross-suite gate lives in the harness
    // (`harness::SUITE_GATE`): process-wide assertions serialize.

    #[test]
    fn the_stress_gate() {
        let _guard = crate::harness::SUITE_GATE.lock().expect("stress gate");
        let full = crate::full_stress_requested();
        let outcome = run_gate(full);
        println!("{}", outcome.summary());
    }
}
