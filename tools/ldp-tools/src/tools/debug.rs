//! `ldp-debug` — the live session tracer and scheduler-timeline replayer.
//!
//! ```text
//! ldp-debug trace [--socket NAME] [--duration SECS] [--bind IFACE]...
//!                 [--generate N] [--class CLASS]... [--filter TEXT]
//! ldp-debug replay <FILE>
//! ```
//!
//! `trace` connects as a real client, prints one line per dispatched
//! event (`[seq N] iface.op(args) [class]`, with the release-fence
//! state when a descriptor rides along), optionally binds extra
//! globals, optionally generates presentation traffic (a ping-pong
//! buffer surface committing `N` frames), and ends with a per-class /
//! per-interface summary. Filters narrow what is *printed*, never
//! what is *counted* — the summary is the honest census.
//!
//! `replay` decodes a Phase 7 scheduler recording
//! (`ldp_compositor::replay::Recording::to_bytes`, magic `LDP7REC`) —
//! the determinism-replay artifact the compositor's suites produce —
//! prints the input script and the output timeline, and reports the
//! deadline hit-rate.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use ldp_client::class::EventClass;
use ldp_compositor::replay::SchedInput;
use ldp_compositor::{Recording, SchedEvent};
use ldp_core::geometry::Rect;
use ldp_core::time::{FrameDropReason, PresentationMode};

use crate::args::{Args, UsageError};
use crate::session::{RecordedEvent, ToolSession};
use crate::socket;
use crate::value_fmt::{class_name, format_value};

/// Parsed command line.
#[derive(Clone, Debug)]
pub struct DebugArgs {
    /// `trace` or `replay`.
    pub command: DebugCommand,
}

/// The subcommand.
#[derive(Clone, Debug)]
pub enum DebugCommand {
    /// Live tracing.
    Trace(TraceArgs),
    /// Scheduler-recording replay.
    Replay(PathBuf),
}

/// Live-trace options.
#[derive(Clone, Debug, Default)]
pub struct TraceArgs {
    /// `--socket NAME` / `LDP_SOCKET`.
    pub socket: Option<String>,
    /// `--duration SECS` (default 3).
    pub duration: Duration,
    /// `--bind IFACE` (repeatable).
    pub binds: Vec<String>,
    /// `--generate N`: commit `N` presentation frames.
    pub generate: u32,
    /// `--class CLASS` print filter (repeatable).
    pub classes: Vec<String>,
    /// `--filter TEXT` substring print filter.
    pub filter: Option<String>,
}

/// Parse the command line.
///
/// # Errors
/// [`UsageError`] on unknown subcommands, arguments, or values.
pub fn parse(mut argv: Args) -> Result<DebugArgs, UsageError> {
    let Some(command) = argv.take_positional() else {
        return Err(UsageError(usage()));
    };
    let command = match command.as_str() {
        "trace" => DebugCommand::Trace(parse_trace(&mut argv)?),
        "replay" => {
            let file = argv
                .take_positional()
                .ok_or_else(|| UsageError("ldp-debug replay needs a FILE".to_owned()))?;
            argv.require_empty()?;
            DebugCommand::Replay(PathBuf::from(file))
        }
        "help" | "--help" | "-h" => return Err(UsageError(usage())),
        other => {
            return Err(UsageError(format!(
                "unknown command '{other}' (trace|replay)"
            )));
        }
    };
    Ok(DebugArgs { command })
}

/// Parse `trace` options.
fn parse_trace(rest: &mut Args) -> Result<TraceArgs, UsageError> {
    let mut args = TraceArgs {
        duration: Duration::from_secs(3),
        ..TraceArgs::default()
    };
    args.socket = rest.take_value("--socket")?;
    if let Some(secs) = rest.take_value("--duration")? {
        let secs: f64 = secs
            .parse()
            .map_err(|e| UsageError(format!("--duration needs seconds (e.g. 2.5): {e}")))?;
        if !(0.0..=600.0).contains(&secs) {
            return Err(UsageError(
                "--duration must be within 0..=600 seconds".to_owned(),
            ));
        }
        args.duration = Duration::from_secs_f64(secs);
    }
    args.binds = rest.take_values("--bind")?;
    if let Some(frames) = rest.take_value("--generate")? {
        args.generate = frames
            .parse()
            .map_err(|e| UsageError(format!("--generate needs a frame count: {e}")))?;
    }
    args.classes = rest.take_values("--class")?;
    for class in &args.classes {
        if parse_class(class).is_none() {
            return Err(UsageError(format!(
                "unknown class '{class}' (input|data|control|configuration|presentation)"
            )));
        }
    }
    args.filter = rest.take_value("--filter")?;
    rest.require_empty()?;
    Ok(args)
}

/// A class name to its lane.
fn parse_class(name: &str) -> Option<EventClass> {
    match name {
        "input" => Some(EventClass::Input),
        "data" => Some(EventClass::Data),
        "control" => Some(EventClass::Control),
        "configuration" => Some(EventClass::Configuration),
        "presentation" => Some(EventClass::Presentation),
        _ => None,
    }
}

/// The usage text.
#[must_use]
pub fn usage() -> String {
    "ldp-debug — live session tracer + scheduler replay\n\
     \n\
     USAGE:\n\
     \x20 ldp-debug trace [--socket NAME] [--duration SECS] [--bind IFACE]...\n\
     \x20                   [--generate N] [--class CLASS]... [--filter TEXT]\n\
     \x20 ldp-debug replay <FILE>\n\
     \n\
     trace   Connect and print every dispatched event, one line each,\n\
     \x20        with a per-class summary at the end. --generate commits N\n\
     \x20        presentation frames so presentation traffic is visible.\n\
     replay  Decode a scheduler recording (LDP7REC) and print the\n\
     \x20        timeline with its deadline hit-rate.\n\
     \x20 --version | -V  print the release version"
        .to_owned()
}

/// Run the tool; returns the process exit code.
///
/// # Errors
/// [`UsageError`] only; protocol and file failures print and return 1.
pub fn run(args: &DebugArgs, out: &mut dyn Write) -> Result<i32, UsageError> {
    let code = match &args.command {
        DebugCommand::Trace(trace) => trace_report(trace, out) as i32,
        DebugCommand::Replay(path) => replay_report(path, out) as i32,
    };
    Ok(code)
}

/// The per-class census a trace summary reports.
#[derive(Default)]
struct Census {
    total: usize,
    by_class: [usize; 5],
    by_interface: BTreeMap<String, usize>,
}

impl Census {
    fn record(&mut self, class: EventClass, interface: &str) {
        self.total += 1;
        self.by_class[class_index(class)] += 1;
        *self.by_interface.entry(interface.to_owned()).or_default() += 1;
    }
}

/// The lane's summary slot.
fn class_index(class: EventClass) -> usize {
    match class {
        EventClass::Input => 0,
        EventClass::Data => 1,
        EventClass::Control => 2,
        EventClass::Configuration => 3,
        EventClass::Presentation => 4,
    }
}

/// The class of a recorded event (the client's classifier).
fn record_class(record: &RecordedEvent) -> EventClass {
    ldp_client::class::class_of(&record.interface, &record.event)
}

/// Arguments of a recorded event, name=value pairs.
fn format_record_args(record: &RecordedEvent) -> String {
    let mut out = String::new();
    for (name, value) in record.arg_names.iter().zip(record.args.iter()) {
        if !out.is_empty() {
            out.push_str(", ");
        }
        let _ = write!(out, "{name}={}", format_value(value));
    }
    out
}

/// One tracer line for a recorded event (with fence marker).
fn trace_line(record: &RecordedEvent) -> String {
    let mut out = format!(
        "[seq {}] {}.{}({}) [{}]",
        record.seq,
        record.interface,
        record.event,
        format_record_args(record),
        class_name(record_class(record))
    );
    if let Some(signalled) = record.fence {
        out.push_str(if signalled {
            " fence:signalled"
        } else {
            " fence:pending"
        });
    }
    out
}

/// Whether a record passes the print filters.
fn passes(record: &RecordedEvent, trace: &TraceArgs) -> bool {
    if !trace.classes.is_empty() {
        let matched = trace
            .classes
            .iter()
            .any(|c| c == class_name(record_class(record)));
        if !matched {
            return false;
        }
    }
    if let Some(filter) = &trace.filter {
        let haystack = format!("{}.{}", record.interface, record.event);
        if !haystack.contains(filter.as_str()) {
            return false;
        }
    }
    true
}

/// Run a live trace.
fn trace_report(trace: &TraceArgs, out: &mut dyn Write) -> bool {
    let addr = match socket::resolve(
        trace.socket.as_deref(),
        std::env::var("LDP_SOCKET").ok().as_deref(),
    ) {
        Ok(addr) => addr,
        Err(e) => {
            let _ = writeln!(out, "ldp-debug: {e}");
            return true;
        }
    };
    let mut session = match ToolSession::connect(&addr) {
        Ok(session) => session,
        Err(e) => {
            let _ = writeln!(
                out,
                "ldp-debug: connect {} failed: {e}",
                addr.display_string()
            );
            return true;
        }
    };
    if let Err(e) = session.bootstrap() {
        let _ = writeln!(out, "ldp-debug: bootstrap failed: {e}");
        return true;
    }
    let _ = writeln!(
        out,
        "trace: {} ({} global(s) advertised)",
        addr.display_string(),
        session.globals().len()
    );

    // Optional extra binds (the registry lane is already drained).
    for iface in &trace.binds {
        if session.has_global(iface) {
            match session.bind(iface) {
                Ok(_) => {
                    let _ = writeln!(out, "trace: bound {iface}");
                }
                Err(e) => {
                    let _ = writeln!(out, "ldp-debug: bind {iface} failed: {e}");
                    return true;
                }
            }
        } else {
            let _ = writeln!(out, "trace: skip {iface} (not advertised)");
        }
    }

    let mut census = Census::default();
    let mut cursor = 0usize;

    // Optional generated presentation traffic.
    if trace.generate > 0 {
        let mut cycle = match session.setup_surface(64, 64, 2, 0x20) {
            Ok(cycle) => cycle,
            Err(e) => {
                let _ = writeln!(out, "ldp-debug: surface setup failed: {e}");
                return true;
            }
        };
        for frame in 1..=u64::from(trace.generate) {
            if let Err(e) = session.commit_frame(&mut cycle, frame, &[Rect::new(0, 0, 64, 64)]) {
                let _ = writeln!(out, "ldp-debug: generated frame {frame} failed: {e}");
                return true;
            }
        }
    }

    // The duration loop: round-trip, print the delta, census everything.
    let deadline = Instant::now() + trace.duration;
    loop {
        if let Err(e) = session.roundtrip() {
            let _ = writeln!(out, "trace: connection ended: {e}");
            break;
        }
        cursor = drain_and_print(&session, cursor, trace, &mut census, out);
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    // Summary: the honest census over *everything* dispatched.
    let _ = writeln!(
        out,
        "summary: {} event(s) in {:.1}s",
        census.total,
        trace.duration.as_secs_f64()
    );
    for (class, index) in [
        (EventClass::Input, 0usize),
        (EventClass::Data, 1),
        (EventClass::Control, 2),
        (EventClass::Configuration, 3),
        (EventClass::Presentation, 4),
    ] {
        let _ = writeln!(
            out,
            "  {:>13}: {}",
            class_name(class),
            census.by_class[index]
        );
    }
    for (interface, count) in &census.by_interface {
        let _ = writeln!(out, "  {interface:>25}: {count}");
    }
    false
}

/// Print and census the event delta since `cursor`.
fn drain_and_print(
    session: &ToolSession,
    cursor: usize,
    trace: &TraceArgs,
    census: &mut Census,
    out: &mut dyn Write,
) -> usize {
    let records = session.records();
    let start = cursor.min(records.len());
    for record in &records[start..] {
        census.record(record_class(record), &record.interface);
        if passes(record, trace) {
            let _ = writeln!(out, "{}", trace_line(record));
        }
    }
    records.len()
}

/// Run a scheduler-recording replay.
#[allow(clippy::too_many_lines)] // one linear timeline narrative
fn replay_report(path: &std::path::Path, out: &mut dyn Write) -> bool {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) => {
            let _ = writeln!(out, "ldp-debug: read {} failed: {e}", path.display());
            return true;
        }
    };
    let recording = match Recording::from_bytes(&bytes) {
        Ok(recording) => recording,
        Err(e) => {
            let _ = writeln!(
                out,
                "ldp-debug: {} is not a valid recording: {e:?}",
                path.display()
            );
            return true;
        }
    };
    let events = match ldp_compositor::replay::replay(&recording) {
        Ok(events) => events,
        Err(e) => {
            let _ = writeln!(out, "ldp-debug: replay failed: {e:?}");
            return true;
        }
    };
    let _ = writeln!(
        out,
        "recording: nominal {} ns/frame, {} input(s), {} output event(s)",
        recording.nominal_ns,
        recording.inputs.len(),
        events.len()
    );
    for input in &recording.inputs {
        let _ = writeln!(out, "in  {}", format_input(input));
    }

    let mut deadlines: BTreeMap<u64, u64> = BTreeMap::new();
    let mut presented_at: BTreeMap<u64, u64> = BTreeMap::new();
    let mut presented = 0usize;
    let mut dropped = 0usize;
    for event in &events {
        match event {
            SchedEvent::FrameTarget {
                surface,
                frame,
                deadline,
            } => {
                deadlines.insert(*frame, deadline.deadline.as_ns());
                let _ = writeln!(
                    out,
                    "out frame_target surface {surface} frame {frame}: \
                     deadline {} ns, vblank {} ns, refresh {} ns, mode {}",
                    deadline.deadline.as_ns(),
                    deadline.target_vblank.as_ns(),
                    deadline.refresh.as_ns(),
                    mode_name(deadline.mode)
                );
            }
            SchedEvent::Presented { surface, timing } => {
                presented += 1;
                presented_at.insert(timing.frame, timing.presented_at.as_ns());
                let _ = writeln!(
                    out,
                    "out presented surface {surface} frame {}: ts {} ns, \
                     refresh {} ns, flags{}{}{}{}",
                    timing.frame,
                    timing.presented_at.as_ns(),
                    timing.refresh.as_ns(),
                    if timing.flags.vblank { " vblank" } else { "" },
                    if timing.flags.scanout { " scanout" } else { "" },
                    if timing.flags.overlay { " overlay" } else { "" },
                    if timing.flags.torn { " torn" } else { "" }
                );
            }
            SchedEvent::FrameDropped {
                surface,
                frame,
                reason,
            } => {
                dropped += 1;
                let _ = writeln!(
                    out,
                    "out dropped surface {surface} frame {frame}: {}",
                    drop_reason_name(*reason)
                );
            }
        }
    }

    // Deadline hit-rate: pair each presented frame with its target.
    let hits = presented_at
        .iter()
        .filter(|(frame, _)| deadlines.contains_key(*frame))
        .filter(|(frame, ts)| **ts <= deadlines.get(*frame).copied().unwrap_or(u64::MAX))
        .count();
    let rate = if presented + dropped == 0 {
        100.0
    } else {
        hits as f64 / (presented + dropped) as f64 * 100.0
    };
    let _ = writeln!(
        out,
        "summary: {presented} presented, {dropped} dropped, \
         deadline hit-rate {rate:.1}%"
    );
    false
}

/// One input line of the replay script.
fn format_input(input: &SchedInput) -> String {
    match input {
        SchedInput::Flip { ts } => format!("flip @ {} ns", ts.as_ns()),
        SchedInput::FrameRequest { surface, frame, ts } => format!(
            "frame_request surface {surface} frame {frame} @ {} ns",
            ts.as_ns()
        ),
        SchedInput::Commit { surface, ts } => {
            format!("commit surface {surface} @ {} ns", ts.as_ns())
        }
        SchedInput::SetVisibility {
            surface,
            hidden,
            ts,
        } => {
            format!(
                "set_visibility surface {surface} hidden {hidden} @ {} ns",
                ts.as_ns()
            )
        }
        SchedInput::SetMode { surface, mode, ts } => {
            format!(
                "set_mode surface {surface} mode {} @ {} ns",
                mode_name(*mode),
                ts.as_ns()
            )
        }
        SchedInput::SetProfile {
            surface,
            profile,
            ts,
        } => {
            format!(
                "set_profile surface {surface} profile {} @ {} ns",
                profile_name(*profile),
                ts.as_ns()
            )
        }
        SchedInput::Park { ts } => format!("park @ {} ns", ts.as_ns()),
        SchedInput::Resume { ts } => format!("resume @ {} ns", ts.as_ns()),
    }
}

/// Scene-profile name (the Phase 47 replay vocabulary's doctrine
/// phrases).
fn profile_name(profile: ldp_compositor::SceneProfile) -> &'static str {
    match profile {
        ldp_compositor::SceneProfile::Desktop => "desktop",
        ldp_compositor::SceneProfile::Creative => "creative",
        ldp_compositor::SceneProfile::Gaming => "gaming",
    }
}

/// Presentation-mode name.
fn mode_name(mode: PresentationMode) -> &'static str {
    match mode {
        PresentationMode::Vsync => "vsync",
        PresentationMode::Adaptive => "adaptive",
        PresentationMode::Immediate => "immediate",
        _ => "(unknown)",
    }
}

/// Drop-reason name.
fn drop_reason_name(reason: FrameDropReason) -> &'static str {
    match reason {
        FrameDropReason::DeadlineMissed => "deadline-missed",
        FrameDropReason::SurfaceHidden => "surface-hidden",
        FrameDropReason::OutputOff => "output-off",
        FrameDropReason::Throttled => "throttled",
        FrameDropReason::Superseded => "superseded",
        _ => "(unknown)",
    }
}
