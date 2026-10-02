//! `ldp-input-debug` — evdev dump analysis: raw → frames → normalized.
//!
//! ```text
//! ldp-input-debug <FILE> [--events] [--name NAME] [--live [--socket NAME]]
//! ```
//!
//! The offline half walks the exact pipeline `ldp-input` runs on a
//! live device, over a captured dump of 24-byte kernel `input_event`
//! records:
//!
//! 1. **raw** — [`StreamDecoder`] (partial-record tolerant) with a
//!    census per event type and the observed time span;
//! 2. **frames** — the [`Framer`] grouping by `SYN_REPORT`, flagging
//!    `SYN_DROPPED` and protocol-A separators;
//! 3. **normalized** — the [`Normalizer`] over a device spec
//!    *inferred from the observed stream* (every observed `(type,
//!    code)` declared as a capability; ABS ranges from the observed
//!    minimum/maximum) — the inference is printed, because a dump
//!    carries no `EVIOCG*` probe data and pretending otherwise would
//!    be fabrication.
//!
//! The `--live` half follows the Phase 18 doctrine for offline-core
//! tools: connect and report what the running server offers — the
//! `ldp.input.seat` global is the input path this tool's analysis
//! feeds, so its absence is reported honestly (the Phase 10 vertical
//! slice does not serve it).

#![forbid(unsafe_code)]

use std::fmt::Write as _;
use std::io::Write;
use std::path::PathBuf;

use ldp_input::codes::{abs, btn, ev, key, rel, syn};
use ldp_input::device::{AbsInfo, ClassHint, DeviceId, DeviceSpec};
use ldp_input::evdev::{Framer, RawEvent, StreamDecoder};
use ldp_input::normalizer::{Axis, InputEvent, Normalizer};

use crate::args::{Args, UsageError};
use crate::session::ToolSession;
use crate::socket;

/// The seat interface this tool's analysis feeds.
const SEAT_GLOBAL: &str = "ldp.input.seat";

/// Parsed command line.
#[derive(Clone, Debug, Default)]
pub struct InputDebugArgs {
    /// The evdev dump (24-byte records) to analyze.
    pub file: Option<PathBuf>,
    /// `--events`: print every raw event as one line.
    pub events: bool,
    /// `--name NAME`: the synthesized device name (default "dump").
    pub name: Option<String>,
    /// `--live`: inspect a running server instead.
    pub live: bool,
    /// `--socket NAME` / `LDP_SOCKET`.
    pub socket: Option<String>,
}

/// Parse the command line.
///
/// # Errors
/// [`UsageError`] on unknown arguments or a file given with `--live`.
pub fn parse(mut argv: Args) -> Result<InputDebugArgs, UsageError> {
    let events = argv.take_flag("--events");
    let mut name = None;
    if let Some(text) = argv.take_value("--name")? {
        if text.is_empty() {
            return Err(UsageError("--name wants a non-empty value".to_owned()));
        }
        name = Some(text);
    }
    let socket = argv.take_value("--socket")?;
    let live = argv.take_flag("--live");
    let file = argv.take_positional().map(PathBuf::from);
    argv.require_empty()?;
    if live && file.is_some() {
        return Err(UsageError(
            "a dump file cannot be combined with --live".to_owned(),
        ));
    }
    if !live && file.is_none() {
        return Err(UsageError(
            "give an evdev dump file (or --live to inspect a running server)".to_owned(),
        ));
    }
    Ok(InputDebugArgs {
        file,
        events,
        name,
        live,
        socket,
    })
}

/// The usage text.
#[must_use]
pub fn usage() -> String {
    "ldp-input-debug — evdev dump analysis (raw → frames → normalized)\n\
     \n\
     USAGE:\n\
     \x20 ldp-input-debug <FILE> [--events] [--name NAME]\n\
     \x20 ldp-input-debug --live [--socket NAME]\n\
     \n\
     OPTIONS:\n\
     \x20 <FILE>        the evdev dump (binary 24-byte input_event records)\n\
     \x20 --events      print every raw event, one line each\n\
     \x20 --name NAME   device name for the inferred spec (default: dump)\n\
     \x20 --live        inspect what a running server offers\n\
     \x20 --socket NAME abstract socket name (or LDP_SOCKET)\n\
     \x20 --help | --version  this text"
        .to_owned()
}

/// Run the tool; returns the process exit code.
///
/// # Errors
/// [`UsageError`] only; analysis failures print and return 1.
///
/// # Panics
/// When `--live` is absent but no file was given — [`parse`] rejects
/// that combination first, so the invariant holds by construction.
#[allow(clippy::too_many_lines)] // the three analysis stages, one narrative
pub fn run(args: &InputDebugArgs, out: &mut dyn Write) -> Result<i32, UsageError> {
    if args.live {
        return Ok(live_report(args, out));
    }
    let path = args
        .file
        .as_ref()
        .expect("parse guarantees a file without --live");
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) => {
            let _ = writeln!(out, "ldp-input-debug: read {}: {e}", path.display());
            return Ok(1);
        }
    };
    // ---- stage 1: raw ------------------------------------------------
    let mut decoder = StreamDecoder::new();
    let raw_events = decoder.feed(&bytes);
    let partial = decoder.buffered();
    let _ = writeln!(
        out,
        "raw: {} record(s) decoded from {} byte(s) in {}",
        raw_events.len(),
        bytes.len(),
        path.display()
    );
    if partial > 0 {
        let _ = writeln!(
            out,
            "raw: {partial} trailing byte(s) do not form a complete record \
             (left over, per the framing doctrine)"
        );
    }
    if raw_events.is_empty() {
        let _ = writeln!(out, "raw: nothing to analyze");
        // A partial tail is informational (the framing doctrine: the
        // leftover bytes ride along, they are not a failure) — exit 0.
        return Ok(0);
    }
    let span_us = raw_events
        .last()
        .map(|last| last.time_us.saturating_sub(raw_events[0].time_us));
    let _ = writeln!(
        out,
        "raw: span {} us (first ts {}, last ts {})",
        span_us.unwrap_or(0),
        raw_events[0].time_us,
        raw_events.last().map_or(0, |e| e.time_us)
    );
    for (ev_type, count) in census(&raw_events) {
        let _ = writeln!(
            out,
            "raw:   {} (0x{ev_type:02x}): {count}",
            ev_type_name(ev_type)
        );
    }
    if args.events {
        for event in &raw_events {
            let _ = writeln!(out, "  {}", format_raw(event));
        }
    }

    // ---- stage 2: frames ----------------------------------------------
    let mut framer = Framer::new();
    let mut formed = Vec::new();
    for event in &raw_events {
        if let Some(frame) = framer.feed(*event) {
            formed.push(frame);
        }
    }
    let pending = framer.pending().len();
    let dropped = formed.iter().filter(|f| f.dropped).count();
    let protocol_a = formed.iter().filter(|f| f.protocol_a).count();
    let _ = writeln!(out, "frames: {} formed by SYN_REPORT", formed.len());
    if pending > 0 {
        let _ = writeln!(
            out,
            "frames: {pending} event(s) after the last SYN_REPORT — a \
             truncated capture, flushed as an unterminated frame is NOT \
             (the kernel never ends a packet without SYN_REPORT)"
        );
    }
    if dropped > 0 {
        let _ = writeln!(
            out,
            "frames: {dropped} frame(s) flagged SYN_DROPPED — consumers \
             must resynchronize (tracking restarts, motion history resets)"
        );
    }
    if protocol_a > 0 {
        let _ = writeln!(
            out,
            "frames: {protocol_a} frame(s) carried SYN_MT_REPORT — \
             protocol-A (slotless) multitouch, which the pipeline marks \
             unsupported"
        );
    }

    // ---- stage 3: normalized -------------------------------------------
    let spec = infer_spec(args.name.as_deref().unwrap_or("dump"), &raw_events);
    let _ = writeln!(
        out,
        "device: inferred from the observed stream — name {:?}, {} \
         capability bit(s), {} ABS axis/axes",
        spec.name,
        spec.bits.len(),
        spec.abs.len()
    );
    for (code, info) in &spec.abs {
        let _ = writeln!(
            out,
            "device:   {} range {}..={}",
            abs_name(*code),
            info.min,
            info.max
        );
    }
    let _ = writeln!(
        out,
        "device: class hints mouse={} touchpad={} touchscreen={} \
         tablet={} keyboard={}",
        spec.hint.mouse,
        spec.hint.touchpad,
        spec.hint.touchscreen,
        spec.hint.tablet,
        spec.hint.keyboard
    );
    let mut normalizer = Normalizer::new(spec);
    let mut normalized_count = 0usize;
    let mut other = 0usize;
    for frame in &formed {
        for event in normalizer.normalize_frame(frame) {
            normalized_count += 1;
            if matches!(event, InputEvent::Other { .. }) {
                other += 1;
            }
            let _ = writeln!(out, "  {}", format_normalized(&event));
        }
    }
    let _ = writeln!(
        out,
        "normalized: {normalized_count} event(s) ({other} outside the consumed \
         vocabulary, surfaced not dropped)"
    );
    Ok(0)
}

/// The `--live` registry report.
fn live_report(args: &InputDebugArgs, out: &mut dyn Write) -> i32 {
    let addr = match socket::resolve(
        args.socket.as_deref(),
        std::env::var("LDP_SOCKET").ok().as_deref(),
    ) {
        Ok(addr) => addr,
        Err(e) => {
            let _ = writeln!(out, "ldp-input-debug: {e}");
            return 1;
        }
    };
    let mut session = match ToolSession::connect(&addr) {
        Ok(session) => session,
        Err(e) => {
            let _ = writeln!(
                out,
                "ldp-input-debug: connect {} failed: {e}",
                addr.display_string()
            );
            return 1;
        }
    };
    if let Err(e) = session.bootstrap() {
        let _ = writeln!(out, "ldp-input-debug: bootstrap failed: {e}");
        return 1;
    }
    let _ = writeln!(
        out,
        "live: {} ({} global(s) advertised)",
        addr.display_string(),
        session.globals().len()
    );
    for global in session.globals() {
        let _ = writeln!(
            out,
            "  {} v{}-v{}",
            global.interface, global.version_min, global.version_max
        );
    }
    if session.has_global(SEAT_GLOBAL) {
        let _ = writeln!(
            out,
            "live: {SEAT_GLOBAL} is served — this server runs the input \
             path; capture a device dump with the evdev backend and \
             analyze it here"
        );
    } else {
        let _ = writeln!(
            out,
            "live: {SEAT_GLOBAL} is NOT served — this server runs no \
             input path (the Phase 10 vertical slice composites and \
             presents; input lands with the seat integration); the \
             offline half of this tool applies to any evdev dump"
        );
    }
    0
}

/// Per-type census, ordered by type then first appearance.
fn census(events: &[RawEvent]) -> Vec<(u16, usize)> {
    let mut seen: Vec<(u16, usize)> = Vec::new();
    for event in events {
        if let Some(entry) = seen.iter_mut().find(|(t, _)| *t == event.ev_type) {
            entry.1 += 1;
        } else {
            seen.push((event.ev_type, 1));
        }
    }
    seen.sort_by_key(|(t, _)| *t);
    seen
}

/// Infer a device spec from the observed stream.
fn infer_spec(name: &str, events: &[RawEvent]) -> DeviceSpec {
    let mut spec = DeviceSpec::new(name, DeviceId::default());
    let mut abs_min: std::collections::BTreeMap<u16, i32> = std::collections::BTreeMap::new();
    let mut abs_max: std::collections::BTreeMap<u16, i32> = std::collections::BTreeMap::new();
    let mut has_rel = false;
    let mut has_keys = false;
    let mut has_touch = false;
    let mut has_pressure_tool = false;
    for event in events {
        if event.ev_type == ev::SYN {
            continue;
        }
        spec = spec.with_bit(event.ev_type, event.code);
        if event.ev_type == ev::REL {
            has_rel |= event.code == rel::X || event.code == rel::Y;
        }
        if event.ev_type == ev::KEY {
            let code = u32::from(event.code);
            if code < btn::LEFT {
                has_keys = true;
            }
            if (btn::TOOL_PEN..=btn::TOOL_LENS).contains(&code) {
                has_pressure_tool = true;
            }
            if code == btn::TOUCH {
                has_touch = true;
            }
        }
        if event.ev_type == ev::ABS {
            let code = event.code;
            let value = event.value;
            let min = abs_min.entry(code).or_insert(value);
            if value < *min {
                *min = value;
            }
            let max = abs_max.entry(code).or_insert(value);
            if value > *max {
                *max = value;
            }
            if code == abs::MT_POSITION_X
                || code == abs::MT_POSITION_Y
                || code == abs::MT_TRACKING_ID
                || code == abs::MT_SLOT
            {
                has_touch = true;
            }
            if code == abs::PRESSURE || code == abs::TILT_X || code == abs::TILT_Y {
                has_pressure_tool = true;
            }
        }
    }
    for code in abs_min.keys().copied().collect::<Vec<_>>() {
        let min = abs_min[&code];
        let max = abs_max[&code].max(min);
        let info = if min < 0 {
            // Observed negatives: a signed axis (tilt-style); widen to
            // the symmetric hull so the normalizer's 0..1 mapping is
            // honest about the observed range.
            let reach = (-min).max(max);
            AbsInfo::symmetric(reach)
        } else {
            AbsInfo::range(max.max(1))
        };
        spec = spec.with_abs(code, info);
    }
    let mut hint = ClassHint {
        keyboard: has_keys && !has_rel && !has_touch,
        mouse: has_rel,
        ..ClassHint::default()
    };
    if has_touch && !has_pressure_tool {
        hint.touchscreen = true;
    }
    if has_pressure_tool {
        hint.tablet = true;
    }
    spec.with_hint(hint)
}

/// One raw event as one line.
fn format_raw(event: &RawEvent) -> String {
    format!(
        "ts {:>12}  {} {} = {}",
        event.time_us,
        ev_type_name(event.ev_type),
        code_name(event.ev_type, event.code),
        event.value
    )
}

/// One normalized event as one line.
fn format_normalized(event: &InputEvent) -> String {
    match event {
        InputEvent::PointerMotion { dx, dy } => {
            format!("motion dx={dx:+} dy={dy:+}")
        }
        InputEvent::Wheel(wheel) => format!(
            "wheel {} discrete={:+} hi_res={:+.1}",
            axis_name(wheel.axis),
            wheel.discrete,
            wheel.hi_res
        ),
        InputEvent::Button { button, pressed } => format!(
            "button {} {}",
            btn_name(*button),
            if *pressed { "pressed" } else { "released" }
        ),
        InputEvent::Key { keycode, pressed } => format!(
            "key {} {}",
            key_name(*keycode),
            if *pressed { "pressed" } else { "released" }
        ),
        InputEvent::Touch(touch) => {
            let mut line = format!(
                "touch n={} began={:?} ended={:?}",
                touch.points.len(),
                touch.began,
                touch.ended
            );
            if touch.dropped {
                line.push_str(" DROPPED");
            }
            for point in &touch.points {
                let pressure = point
                    .pressure
                    .map_or_else(|| "-".to_owned(), |p| format!("{p:.3}"));
                let _ = write!(
                    line,
                    " [id {} x={:.3} y={:.3} p={}]",
                    point.id, point.x, point.y, pressure
                );
            }
            line
        }
        InputEvent::Tablet(tablet) => format!(
            "tablet tool={} contact={} x={:.3} y={:.3} p={} dist={} tilt=({},{})",
            tablet
                .tool
                .map_or_else(|| "-".to_owned(), |tool| tablet_tool_name(tool).to_owned()),
            tablet.contact,
            tablet.x,
            tablet.y,
            tablet
                .pressure
                .map_or_else(|| "-".to_owned(), |p| format!("{p:.3}")),
            tablet
                .distance
                .map_or_else(|| "-".to_owned(), |d| format!("{d:.3}")),
            tablet
                .tilt_x
                .map_or_else(|| "-".to_owned(), |t| format!("{t:.3}")),
            tablet
                .tilt_y
                .map_or_else(|| "-".to_owned(), |t| format!("{t:.3}")),
        ),
        InputEvent::Other {
            ev_type,
            code,
            value,
        } => format!("other {} {code} = {value}", ev_type_name(*ev_type)),
    }
}

/// EV_* type names (the documented subset, hex otherwise).
fn ev_type_name(ev_type: u16) -> String {
    match ev_type {
        ev::SYN => "EV_SYN".to_owned(),
        ev::KEY => "EV_KEY".to_owned(),
        ev::REL => "EV_REL".to_owned(),
        ev::ABS => "EV_ABS".to_owned(),
        ev::MSC => "EV_MSC".to_owned(),
        ev::LED => "EV_LED".to_owned(),
        ev::REP => "EV_REP".to_owned(),
        _ => format!("EV_0x{ev_type:02x}"),
    }
}

/// Code names per type (the pinned subset, plain numbers otherwise).
fn code_name(ev_type: u16, code: u16) -> String {
    match ev_type {
        ev::SYN => match code {
            syn::REPORT => "SYN_REPORT".to_owned(),
            syn::MT_REPORT => "SYN_MT_REPORT".to_owned(),
            syn::DROPPED => "SYN_DROPPED".to_owned(),
            _ => format!("SYN_{code}"),
        },
        ev::REL => rel_name(code),
        ev::KEY => {
            if u32::from(code) >= btn::LEFT {
                btn_name(u32::from(code))
            } else {
                key_name(u32::from(code))
            }
        }
        ev::ABS => abs_name(code),
        _ => code.to_string(),
    }
}

/// REL_* names.
fn rel_name(code: u16) -> String {
    match code {
        rel::X => "REL_X".to_owned(),
        rel::Y => "REL_Y".to_owned(),
        rel::HWHEEL => "REL_HWHEEL".to_owned(),
        rel::WHEEL => "REL_WHEEL".to_owned(),
        rel::HWHEEL_HI_RES => "REL_HWHEEL_HI_RES".to_owned(),
        rel::WHEEL_HI_RES => "REL_WHEEL_HI_RES".to_owned(),
        _ => format!("REL_{code}"),
    }
}

/// ABS_* names.
fn abs_name(code: u16) -> String {
    match code {
        abs::X => "ABS_X".to_owned(),
        abs::Y => "ABS_Y".to_owned(),
        abs::PRESSURE => "ABS_PRESSURE".to_owned(),
        abs::DISTANCE => "ABS_DISTANCE".to_owned(),
        abs::TILT_X => "ABS_TILT_X".to_owned(),
        abs::TILT_Y => "ABS_TILT_Y".to_owned(),
        abs::TOOL_WIDTH => "ABS_TOOL_WIDTH".to_owned(),
        abs::MT_SLOT => "ABS_MT_SLOT".to_owned(),
        abs::MT_TOUCH_MAJOR => "ABS_MT_TOUCH_MAJOR".to_owned(),
        abs::MT_TOUCH_MINOR => "ABS_MT_TOUCH_MINOR".to_owned(),
        abs::MT_ORIENTATION => "ABS_MT_ORIENTATION".to_owned(),
        abs::MT_POSITION_X => "ABS_MT_POSITION_X".to_owned(),
        abs::MT_POSITION_Y => "ABS_MT_POSITION_Y".to_owned(),
        abs::MT_TOOL_TYPE => "ABS_MT_TOOL_TYPE".to_owned(),
        abs::MT_TRACKING_ID => "ABS_MT_TRACKING_ID".to_owned(),
        abs::MT_PRESSURE => "ABS_MT_PRESSURE".to_owned(),
        _ => format!("ABS_{code}"),
    }
}

/// KEY_* names (the pinned subset, plain codes otherwise).
fn key_name(keycode: u32) -> String {
    match keycode {
        key::ESC => "KEY_ESC".to_owned(),
        key::ZERO => "KEY_0".to_owned(),
        key::LEFTCTRL => "KEY_LEFTCTRL".to_owned(),
        key::LEFTSHIFT => "KEY_LEFTSHIFT".to_owned(),
        key::LEFTALT => "KEY_LEFTALT".to_owned(),
        key::SPACE => "KEY_SPACE".to_owned(),
        key::CAPSLOCK => "KEY_CAPSLOCK".to_owned(),
        key::RIGHTCTRL => "KEY_RIGHTCTRL".to_owned(),
        key::RIGHTALT => "KEY_RIGHTALT".to_owned(),
        key::LEFTMETA => "KEY_LEFTMETA".to_owned(),
        key::RIGHTMETA => "KEY_RIGHTMETA".to_owned(),
        _ if (key::ROW_1..=key::ZERO).contains(&keycode) => {
            format!("KEY_{}", keycode - key::ROW_1 + 1)
        }
        _ if (key::A..=key::A + 8).contains(&keycode) => {
            format!(
                "KEY_{}",
                char::from_u32(u32::from(b'A') + keycode - key::A).unwrap_or('?')
            )
        }
        _ => format!("KEY_{keycode}"),
    }
}

/// BTN_* names (the pinned subset, plain codes otherwise).
fn btn_name(code: u32) -> String {
    match code {
        btn::LEFT => "BTN_LEFT".to_owned(),
        btn::RIGHT => "BTN_RIGHT".to_owned(),
        btn::MIDDLE => "BTN_MIDDLE".to_owned(),
        btn::SIDE => "BTN_SIDE".to_owned(),
        btn::EXTRA => "BTN_EXTRA".to_owned(),
        btn::TOOL_PEN => "BTN_TOOL_PEN".to_owned(),
        btn::TOOL_RUBBER => "BTN_TOOL_RUBBER".to_owned(),
        btn::TOOL_BRUSH => "BTN_TOOL_BRUSH".to_owned(),
        btn::TOOL_PENCIL => "BTN_TOOL_PENCIL".to_owned(),
        btn::TOOL_AIRBRUSH => "BTN_TOOL_AIRBRUSH".to_owned(),
        btn::TOOL_FINGER => "BTN_TOOL_FINGER".to_owned(),
        btn::TOOL_MOUSE => "BTN_TOOL_MOUSE".to_owned(),
        btn::TOOL_LENS => "BTN_TOOL_LENS".to_owned(),
        btn::TOUCH => "BTN_TOUCH".to_owned(),
        btn::STYLUS => "BTN_STYLUS".to_owned(),
        btn::STYLUS2 => "BTN_STYLUS2".to_owned(),
        _ => format!("BTN_{code}"),
    }
}

/// Wheel axis names.
const fn axis_name(axis: Axis) -> &'static str {
    match axis {
        Axis::Vertical => "vertical",
        Axis::Horizontal => "horizontal",
    }
}

/// Tablet tool names.
const fn tablet_tool_name(tool: ldp_input::normalizer::TabletTool) -> &'static str {
    use ldp_input::normalizer::TabletTool;
    match tool {
        TabletTool::Pen => "pen",
        TabletTool::Eraser => "eraser",
        TabletTool::Brush => "brush",
        TabletTool::Pencil => "pencil",
        TabletTool::Airbrush => "airbrush",
        TabletTool::Mouse => "mouse",
        TabletTool::Lens => "lens",
        TabletTool::Finger => "finger",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ldp_input::evdev::RawEvent as Ev;

    fn dump(events: &[Ev]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for event in events {
            bytes.extend_from_slice(&event.to_bytes());
        }
        bytes
    }

    /// A minimal mouse-ish trace: motion, wheel, click, in two frames.
    fn mouse_trace() -> Vec<Ev> {
        vec![
            Ev::new(1000, ev::REL, rel::X, 3),
            Ev::new(1000, ev::REL, rel::Y, -1),
            Ev::new(1000, ev::SYN, syn::REPORT, 0),
            Ev::new(2000, ev::KEY, u16::try_from(btn::LEFT).unwrap(), 1),
            Ev::new(2000, ev::REL, rel::WHEEL, 1),
            Ev::new(2000, ev::SYN, syn::REPORT, 0),
            Ev::new(3000, ev::KEY, u16::try_from(btn::LEFT).unwrap(), 0),
            Ev::new(3000, ev::SYN, syn::REPORT, 0),
        ]
    }

    fn run_on(bytes: &[u8]) -> String {
        // Unique per call: the parallel test threads would otherwise
        // race on one shared path (`line!()` here is constant).
        static RUN_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = RUN_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("ldp-tools-inputdbg-{}-{n}", std::process::id()));
        std::fs::write(&path, bytes).unwrap();
        let mut out = Vec::new();
        let code = run(
            &parse(Args::from_argv([path.display().to_string()])).unwrap(),
            &mut out,
        )
        .unwrap();
        let _ = std::fs::remove_file(&path);
        let text = String::from_utf8(out).unwrap();
        assert_eq!(code, 0, "tool output:\n{text}");
        text
    }

    #[test]
    fn parse_accepts_and_rejects() {
        assert!(parse(Args::from_argv(["d.bin", "--events"])).is_ok());
        assert!(parse(Args::from_argv(["d.bin", "--name", "mouse"])).is_ok());
        assert!(parse(Args::from_argv([""; 0])).is_err());
        assert!(parse(Args::from_argv(["d.bin", "--live"])).is_err());
        assert!(parse(Args::from_argv(["--live"])).is_ok());
        assert!(parse(Args::from_argv(["d.bin", "--name", ""])).is_err());
    }

    #[test]
    fn mouse_dump_walks_all_three_stages() {
        let text = run_on(&dump(&mouse_trace()));
        assert!(text.contains("raw: 8 record(s)"), "{text}");
        assert!(text.contains("EV_REL (0x02): 3"), "{text}");
        assert!(text.contains("EV_KEY (0x01): 2"), "{text}");
        assert!(text.contains("frames: 3 formed"), "{text}");
        assert!(text.contains("motion dx=+3 dy=-1"), "{text}");
        assert!(text.contains("wheel vertical discrete=+1"), "{text}");
        assert!(text.contains("button BTN_LEFT pressed"), "{text}");
        assert!(text.contains("button BTN_LEFT released"), "{text}");
        assert!(text.contains("device: class hints mouse=true"), "{text}");
    }

    #[test]
    fn partial_tail_is_reported_not_decoded() {
        let mut bytes = dump(&mouse_trace());
        bytes.extend_from_slice(&[0u8; 10]);
        let text = run_on(&bytes);
        assert!(text.contains("10 trailing byte(s)"), "{text}");
        assert!(text.contains("raw: 8 record(s)"), "{text}");
    }

    #[test]
    fn empty_dump_reports_nothing() {
        let text = run_on(&[]);
        assert!(text.contains("raw: 0 record(s)"), "{text}");
        assert!(text.contains("nothing to analyze"), "{text}");
    }

    #[test]
    fn garbage_only_is_a_partial_record() {
        let text = run_on(&[0xABu8; 7]);
        assert!(text.contains("nothing to analyze"), "{text}");
        assert!(text.contains("7 trailing byte(s)"), "{text}");
    }

    #[test]
    fn dropped_and_protocol_a_flags_surface() {
        let events = vec![
            Ev::new(10, ev::REL, rel::X, 1),
            Ev::new(10, ev::SYN, syn::DROPPED, 0),
            Ev::new(10, ev::SYN, syn::REPORT, 0),
            Ev::new(20, ev::ABS, abs::MT_POSITION_X, 100),
            Ev::new(20, ev::SYN, syn::MT_REPORT, 0),
            Ev::new(20, ev::SYN, syn::REPORT, 0),
        ];
        let text = run_on(&dump(&events));
        assert!(text.contains("1 frame(s) flagged SYN_DROPPED"), "{text}");
        assert!(text.contains("1 frame(s) carried SYN_MT_REPORT"), "{text}");
    }

    #[test]
    fn tablet_dump_normalizes_pressure_and_tilt() {
        // Tilt carries the kernel's hundredths-of-degree scale: -4500
        // is -45 degrees, i.e. -pi/4 radians in the normalized event.
        let events = vec![
            Ev::new(10, ev::KEY, u16::try_from(btn::TOOL_PEN).unwrap(), 1),
            Ev::new(10, ev::ABS, abs::X, 500),
            Ev::new(10, ev::ABS, abs::Y, 250),
            Ev::new(10, ev::ABS, abs::PRESSURE, 512),
            Ev::new(10, ev::ABS, abs::TILT_X, -4500),
            Ev::new(10, ev::SYN, syn::REPORT, 0),
        ];
        let text = run_on(&dump(&events));
        assert!(text.contains("tablet tool=pen"), "{text}");
        assert!(text.contains("tablet=true"), "{text}");
        // Observed negatives widen to the symmetric hull (the inferred
        // spec's documented rule), so the printed range is ±4500.
        assert!(text.contains("ABS_TILT_X range -4500..=4500"), "{text}");
        assert!(text.contains("tilt=(-0.785,-)"), "{text}");
    }

    #[test]
    fn multitouch_dump_reports_contacts() {
        let events = vec![
            Ev::new(10, ev::ABS, abs::MT_SLOT, 0),
            Ev::new(10, ev::ABS, abs::MT_TRACKING_ID, 100),
            Ev::new(10, ev::ABS, abs::MT_POSITION_X, 400),
            Ev::new(10, ev::ABS, abs::MT_POSITION_Y, 300),
            Ev::new(10, ev::ABS, abs::MT_PRESSURE, 32),
            Ev::new(10, ev::KEY, u16::try_from(btn::TOUCH).unwrap(), 1),
            Ev::new(10, ev::SYN, syn::REPORT, 0),
            Ev::new(20, ev::ABS, abs::MT_SLOT, 0),
            Ev::new(20, ev::ABS, abs::MT_TRACKING_ID, -1),
            Ev::new(20, ev::KEY, u16::try_from(btn::TOUCH).unwrap(), 0),
            Ev::new(20, ev::SYN, syn::REPORT, 0),
        ];
        let text = run_on(&dump(&events));
        assert!(text.contains("touch n=1 began=[100]"), "{text}");
        assert!(text.contains("ended=[100]"), "{text}");
        assert!(text.contains("touchscreen=true"), "{text}");
    }

    #[test]
    fn keyboard_dump_reports_keys() {
        let events = vec![
            Ev::new(10, ev::KEY, u16::try_from(key::A).unwrap(), 1),
            Ev::new(10, ev::SYN, syn::REPORT, 0),
            Ev::new(20, ev::KEY, u16::try_from(key::A).unwrap(), 0),
            Ev::new(20, ev::SYN, syn::REPORT, 0),
        ];
        let text = run_on(&dump(&events));
        assert!(text.contains("key KEY_A pressed"), "{text}");
        assert!(text.contains("key KEY_A released"), "{text}");
        assert!(text.contains("keyboard=true"), "{text}");
    }

    #[test]
    fn events_listing_names_raw_codes() {
        let path =
            std::env::temp_dir().join(format!("ldp-tools-inputdbg-list-{}", std::process::id()));
        std::fs::write(&path, dump(&mouse_trace())).unwrap();
        let mut out = Vec::new();
        run(
            &parse(Args::from_argv([
                path.display().to_string(),
                "--events".to_owned(),
            ]))
            .unwrap(),
            &mut out,
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("EV_REL REL_X = 3"), "{text}");
        assert!(text.contains("EV_KEY BTN_LEFT = 1"), "{text}");
        assert!(text.contains("EV_SYN SYN_REPORT = 0"), "{text}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn unknown_codes_print_as_numbers_not_panic() {
        let events = vec![
            Ev::new(10, 0x1f, 0x1234, 5),
            Ev::new(10, ev::SYN, syn::REPORT, 0),
        ];
        let text = run_on(&dump(&events));
        assert!(text.contains("EV_0x1f"), "{text}");
        assert!(text.contains("other EV_0x1f 4660 = 5"), "{text}");
    }

    #[test]
    fn name_tables_cover_the_pinned_subset() {
        assert_eq!(ev_type_name(ev::MSC), "EV_MSC");
        assert_eq!(rel_name(rel::HWHEEL_HI_RES), "REL_HWHEEL_HI_RES");
        assert_eq!(abs_name(abs::MT_TRACKING_ID), "ABS_MT_TRACKING_ID");
        assert_eq!(btn_name(btn::STYLUS), "BTN_STYLUS");
        assert_eq!(key_name(key::ROW_1), "KEY_1");
        assert_eq!(key_name(key::A), "KEY_A");
        assert_eq!(key_name(key::A + 7), "KEY_H");
        assert_eq!(key_name(9999), "KEY_9999");
        assert_eq!(
            code_name(ev::KEY, u16::try_from(btn::MIDDLE).unwrap()),
            "BTN_MIDDLE"
        );
    }

    #[test]
    fn inferred_spec_declares_only_observed_bits() {
        let spec = infer_spec("t", &mouse_trace());
        assert!(spec.has(ev::REL, rel::X));
        assert!(spec.has(ev::REL, rel::Y));
        assert!(spec.has(ev::REL, rel::WHEEL));
        assert!(spec.has(ev::KEY, u16::try_from(btn::LEFT).unwrap()));
        assert!(!spec.has(ev::REL, rel::HWHEEL));
        assert!(spec.abs.is_empty());
        assert!(spec.hint.mouse);
        assert!(!spec.hint.keyboard);
    }
}
