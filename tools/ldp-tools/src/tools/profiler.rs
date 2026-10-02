//! `ldp-profiler` — frame-pipeline measurement, live and offline.
//!
//! ```text
//! ldp-profiler [--socket NAME] [--frames N] [--size WxH] [--timeout SECS]
//! ldp-profiler --replay FILE
//! ```
//!
//! The live mode drives a real client through the whole per-frame
//! choreography — `frame` registration, deadline reception, buffer
//! attach, damage, commit, presentation wait — `N` times over a
//! ping-pong surface, and reports:
//!
//! * **wall latency** (submit → `presented`, measured with
//!   [`std::time::Instant`]: what the client experience measures),
//! * **pipeline pacing** (the compositor's own presented timestamps,
//!   monotonic ns: what the scheduler actually did),
//! * **deadline hit-rate** (`presented.ts <= frame_target.target`: the
//!   deadline contract of the protocol),
//! * drops, with reasons.
//!
//! In headless mode the compositor's clock advances only at wake
//! points, so the pacing numbers are the simulated pipeline's — the
//! wall numbers are the real round-trip cost. Both are labeled.
//!
//! `--replay` computes the same statistics over a Phase 7 scheduler
//! recording — the offline, fully deterministic variant.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use ldp_compositor::replay::SchedInput;
use ldp_compositor::{Recording, SchedEvent};

use crate::args::{Args, UsageError};
use crate::session::ToolSession;
use crate::socket;

/// Parsed command line.
#[derive(Clone, Debug, Default)]
pub struct ProfilerArgs {
    /// `--socket NAME` / `LDP_SOCKET`.
    pub socket: Option<String>,
    /// `--frames N` (default 60).
    pub frames: Option<u32>,
    /// `--size WxH` (default 64x64).
    pub size: Option<(u32, u32)>,
    /// `--replay FILE`: offline mode over a scheduler recording.
    pub replay: Option<PathBuf>,
    /// `--timeout SECS` per frame wait (default 10).
    pub timeout: Option<Duration>,
}

/// Parse the command line.
///
/// # Errors
/// [`UsageError`] on unknown arguments or malformed values.
pub fn parse(mut argv: Args) -> Result<ProfilerArgs, UsageError> {
    let socket = argv.take_value("--socket")?;
    let mut frames = None;
    if let Some(count) = argv.take_value("--frames")? {
        frames = Some(
            count
                .parse()
                .map_err(|e| UsageError(format!("--frames needs a count: {e}")))?,
        );
    }
    let mut size = None;
    if let Some(text) = argv.take_value("--size")? {
        let parse_err = || UsageError("--size needs WxH (e.g. 128x96)".to_owned());
        let (w, h) = text.split_once('x').ok_or_else(parse_err)?;
        let w: u32 = w
            .parse()
            .map_err(|_| UsageError(format!("--size width '{w}' is not a number")))?;
        let h: u32 = h
            .parse()
            .map_err(|_| UsageError(format!("--size height '{h}' is not a number")))?;
        if w == 0 || h == 0 || w > 8192 || h > 8192 {
            return Err(UsageError("--size must be within 1..=8192".to_owned()));
        }
        size = Some((w, h));
    }
    let replay = argv.take_value("--replay")?.map(PathBuf::from);
    let mut timeout = None;
    if let Some(secs) = argv.take_value("--timeout")? {
        let secs: f64 = secs
            .parse()
            .map_err(|e| UsageError(format!("--timeout needs seconds: {e}")))?;
        if !(0.1..=600.0).contains(&secs) {
            return Err(UsageError(
                "--timeout must be within 0.1..=600 seconds".to_owned(),
            ));
        }
        timeout = Some(Duration::from_secs_f64(secs));
    }
    argv.require_empty()?;
    Ok(ProfilerArgs {
        socket,
        frames,
        size,
        replay,
        timeout,
    })
}

/// The usage text.
#[must_use]
pub fn usage() -> String {
    "ldp-profiler — frame-pipeline measurement\n\
     \n\
     USAGE:\n\
     \x20 ldp-profiler [--socket NAME] [--frames N] [--size WxH] [--timeout SECS]\n\
     \x20 ldp-profiler --replay FILE\n\
     \n\
     Live mode commits N frames through the full choreography (frame →\n\
     \x20 deadline → attach → damage → commit → presented) and reports wall\n\
     \x20 latency, pipeline pacing, and the deadline hit-rate. --replay runs\n\
     \x20 the same statistics over a scheduler recording (LDP7REC).\n\
     \x20 --version | -V  print the release version"
        .to_owned()
}

/// Run the tool; returns the process exit code.
///
/// # Errors
/// [`UsageError`] only; measurement failures print and return 1.
pub fn run(args: &ProfilerArgs, out: &mut dyn Write) -> Result<i32, UsageError> {
    let code = match &args.replay {
        Some(path) => replay_report(path, out) as i32,
        None => live_report(args, out) as i32,
    };
    Ok(code)
}

/// One frame's measurements.
#[derive(Clone, Copy, Debug)]
struct FrameMeasurement {
    /// Wall-clock submit→presented latency.
    wall_us: f64,
    /// The compositor's presentation timestamp (monotonic ns).
    presented_ns: u64,
    /// The frame-target deadline (monotonic ns).
    deadline_ns: u64,
}

/// Run the live measurement.
fn live_report(args: &ProfilerArgs, out: &mut dyn Write) -> bool {
    let addr = match socket::resolve(
        args.socket.as_deref(),
        std::env::var("LDP_SOCKET").ok().as_deref(),
    ) {
        Ok(addr) => addr,
        Err(e) => {
            let _ = writeln!(out, "ldp-profiler: {e}");
            return true;
        }
    };
    let frames = args.frames.unwrap_or(60);
    let (width, height) = args.size.unwrap_or((64, 64));
    let timeout = args.timeout.unwrap_or_else(|| Duration::from_secs(10));

    let mut session = match ToolSession::connect(&addr) {
        Ok(session) => session,
        Err(e) => {
            let _ = writeln!(
                out,
                "ldp-profiler: connect {} failed: {e}",
                addr.display_string()
            );
            return true;
        }
    };
    if let Err(e) = session.bootstrap() {
        let _ = writeln!(out, "ldp-profiler: bootstrap failed: {e}");
        return true;
    }
    if !session.has_global("ldp.core.shm") || !session.has_global("ldp.core.compositor") {
        let _ = writeln!(
            out,
            "ldp-profiler: server does not advertise the core surface interfaces"
        );
        return true;
    }
    let mut cycle = match session.setup_surface(width, height, 2, 0x40) {
        Ok(cycle) => cycle,
        Err(e) => {
            let _ = writeln!(out, "ldp-profiler: surface setup failed: {e}");
            return true;
        }
    };

    let _ = writeln!(
        out,
        "profiling: {} frame(s) of {}x{} XRGB8888 against {}",
        frames,
        width,
        height,
        addr.display_string()
    );
    let surface = cycle.surface.clone();
    let mut measurements: Vec<FrameMeasurement> = Vec::with_capacity(frames as usize);
    let mut failures = 0usize;
    for frame in 1..=u64::from(frames) {
        let submit = Instant::now();
        // The choreography: register, attach, damage, commit.
        if let Err(e) = session.commit_frame(
            &mut cycle,
            frame,
            &[ldp_core::geometry::Rect::new(0, 0, width, height)],
        ) {
            failures += 1;
            let _ = writeln!(out, "ldp-profiler: frame {frame} failed: {e}");
            continue;
        }
        let verdict = session.wait_presented(&surface, frame, timeout);
        let wall = submit.elapsed();
        match verdict {
            Ok(Some(sample)) => {
                let target = session.frame_target(&surface, frame);
                measurements.push(FrameMeasurement {
                    wall_us: wall.as_secs_f64() * 1e6,
                    presented_ns: sample.ts_ns,
                    deadline_ns: target.map_or(u64::MAX, |t| t.target_ns),
                });
            }
            verdict => {
                failures += 1;
                let _ = match verdict {
                    Ok(None) => writeln!(out, "ldp-profiler: frame {frame} dropped or timed out"),
                    Err(e) => writeln!(out, "ldp-profiler: frame {frame}: {e}"),
                    _ => unreachable!("covered above"),
                };
            }
        }
    }
    report_measurements(&measurements, failures, "live", out)
}

/// Report a measurement set (shared by live and replay paths).
fn report_measurements(
    measurements: &[FrameMeasurement],
    failures: usize,
    mode: &str,
    out: &mut dyn Write,
) -> bool {
    let presented = measurements.len();
    if presented == 0 {
        let _ = writeln!(out, "result: no frame presented ({failures} failure(s))");
        return true;
    }
    let mut wall: Vec<f64> = measurements.iter().map(|m| m.wall_us).collect();
    wall.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let pct = |p: f64| wall[(p * (wall.len() - 1) as f64).round() as usize];
    let mean = wall.iter().sum::<f64>() / wall.len() as f64;

    // Pipeline pacing from the compositor's own timestamps.
    let mut intervals: Vec<u64> = measurements
        .windows(2)
        .map(|pair| pair[1].presented_ns.saturating_sub(pair[0].presented_ns))
        .collect();
    let pacing_note = if intervals.is_empty() {
        "n/a (one frame)".to_owned()
    } else {
        intervals.sort_unstable();
        let min = intervals[0];
        let max = intervals[intervals.len() - 1];
        let mean = intervals.iter().sum::<u64>() / intervals.len() as u64;
        format!(
            "{} presentations, mean {} ns, spread {}-{} ns",
            intervals.len() + 1,
            mean,
            min,
            max
        )
    };
    let last_refresh = measurements.last().and_then(|_| intervals.last().copied());

    let hits = measurements
        .iter()
        .filter(|m| m.deadline_ns != u64::MAX && m.presented_ns <= m.deadline_ns)
        .count();
    let contracted = measurements
        .iter()
        .filter(|m| m.deadline_ns != u64::MAX)
        .count();
    let rate = if contracted == 0 {
        100.0
    } else {
        hits as f64 / contracted as f64 * 100.0
    };

    let _ = writeln!(out, "result ({mode}): {presented} frame(s) presented");
    let _ = writeln!(
        out,
        "  wall latency (submit→presented): min {:.1} µs, mean {:.1} µs, \
         p95 {:.1} µs, max {:.1} µs",
        wall[0],
        mean,
        pct(0.95),
        wall[wall.len() - 1]
    );
    let _ = writeln!(out, "  pipeline pacing (compositor clock): {pacing_note}");
    if let Some(refresh) = last_refresh {
        let _ = writeln!(out, "  observed frame interval: {refresh} ns");
    }
    let _ = writeln!(out, "  deadline hit-rate: {hits}/{contracted} ({rate:.1}%)");
    if failures > 0 {
        let _ = writeln!(out, "  failures: {failures}");
        return true;
    }
    false
}

/// Run the offline statistics over a scheduler recording.
fn replay_report(path: &std::path::Path, out: &mut dyn Write) -> bool {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) => {
            let _ = writeln!(out, "ldp-profiler: read {} failed: {e}", path.display());
            return true;
        }
    };
    let recording = match Recording::from_bytes(&bytes) {
        Ok(recording) => recording,
        Err(e) => {
            let _ = writeln!(
                out,
                "ldp-profiler: {} is not a valid recording: {e:?}",
                path.display()
            );
            return true;
        }
    };
    let events = match ldp_compositor::replay::replay(&recording) {
        Ok(events) => events,
        Err(e) => {
            let _ = writeln!(out, "ldp-profiler: replay failed: {e:?}");
            return true;
        }
    };
    let _ = SchedInput::Flip {
        ts: ldp_core::time::Mono::from_ns(0),
    }; // (formatting parity: inputs are listed by ldp-debug replay)
    let _ = writeln!(
        out,
        "replay: nominal {} ns/frame, {} input(s), {} output event(s)",
        recording.nominal_ns,
        recording.inputs.len(),
        events.len()
    );

    // Pair FrameTarget deadlines with Presented timestamps by frame id.
    let mut deadlines: BTreeMap<u64, u64> = BTreeMap::new();
    let mut presented: BTreeMap<u64, u64> = BTreeMap::new();
    let mut drops = 0usize;
    for event in &events {
        match event {
            SchedEvent::FrameTarget {
                frame, deadline, ..
            } => {
                deadlines.insert(*frame, deadline.deadline.as_ns());
            }
            SchedEvent::Presented { timing, .. } => {
                presented.insert(timing.frame, timing.presented_at.as_ns());
            }
            SchedEvent::FrameDropped { .. } => drops += 1,
        }
    }
    let mut measurements = Vec::with_capacity(presented.len());
    for (frame, ts) in &presented {
        let deadline = deadlines.get(frame).copied().unwrap_or(u64::MAX);
        // Offline wall latency is the pipeline's own submit→present span;
        // the recording carries arrival times per input.
        let presented_ns = *ts;
        let submitted = recording
            .inputs
            .iter()
            .find_map(|input| match input {
                // A commit is a candidate submission when it arrived no
                // later than the presentation (the first such input is
                // the submit; frame requests carry their own id).
                SchedInput::Commit { ts: commit_ts, .. } if commit_ts.as_ns() <= presented_ns => {
                    Some(commit_ts.as_ns())
                }
                SchedInput::FrameRequest { ts, frame: f, .. } if *f == *frame => Some(ts.as_ns()),
                _ => None,
            })
            .unwrap_or(presented_ns);
        let wall_us = (presented_ns.saturating_sub(submitted)) as f64 / 1000.0;
        measurements.push(FrameMeasurement {
            wall_us,
            presented_ns,
            deadline_ns: deadline,
        });
    }
    report_measurements(&measurements, drops, "replay", out)
}
