//! # pointer-paint — an input-driven LDP client
//!
//! The hello-ldp choreography plus the input half: an evdev-style
//! pointer trace runs through the real [`ldp_input`] normalizer (the
//! Phase 11 pipeline, library-side), every motion lands as a painted
//! pixel in the client's shm surface, every frame goes through the
//! real commit cycle, and the compositor presents each one. When the
//! trace ends, the scanout carries the exact stroke.
//!
//! This is the smallest demonstration of the two-way contract: input
//! events flow *in* (normalized, not raw), presentation feedback flows
//! *back* (deadline-driven, not fire-and-forget).
//!
//! ```text
//! pointer-paint [--socket NAME] [--script FILE]
//! ```
//!
//! `--script FILE` reads a raw evdev dump (24-byte `input_event`
//! records, the format `ldp-input-debug` analyzes); without it the
//! built-in demonstration trace runs — a small closed shape.

#![forbid(unsafe_code)]

use std::io::Write;

use ldp_core::geometry::Rect;
use ldp_input::codes::{btn, ev, rel, syn};
use ldp_input::device::{DeviceId, DeviceSpec};
use ldp_input::evdev::{Framer, RawEvent, StreamDecoder};
use ldp_input::normalizer::{InputEvent, Normalizer};
use ldp_tools::error::{Result, ToolError};
use ldp_tools::session::ToolSession;
use ldp_transport::UnixAddr;
/// The canvas: 128×128 XRGB8888 at (0, 0).
const CANVAS: u32 = 128;

/// The background fill (dark gray).
const BACKGROUND: u8 = 0x11;

/// The stroke color (XRGB8888 white).
const STROKE: u32 = 0xFFFF_FFFF;

/// Parsed command line.
#[derive(Clone, Debug, Default)]
pub struct PaintArgs {
    /// `--socket NAME` / `LDP_SOCKET`.
    pub socket: Option<String>,
    /// `--script FILE`: a raw evdev dump.
    pub script: Option<String>,
}

/// The usage text.
#[must_use]
pub fn usage() -> String {
    "pointer-paint — an input-driven client\n\
     \n\
     USAGE:\n\
     \x20 pointer-paint [--socket NAME] [--script FILE]\n\
     \n\
     OPTIONS:\n\
     \x20 --socket NAME   abstract socket name (or LDP_SOCKET)\n\
     \x20 --script FILE   raw evdev dump (24-byte input_event records);\n\
     \x20                 default: the built-in demonstration trace\n\
     \x20 --help | --version  this text"
        .to_owned()
}

/// The built-in trace: a closed square corner walk starting at (32, 32)
/// — right 16, down 16, left 16, up 16 — with the button held, in
/// eight packets (the kernel batches a motion pair per report).
#[must_use]
pub fn demo_trace() -> Vec<RawEvent> {
    let mut out = Vec::new();
    let mut t = 1_000;
    for (dx, dy) in [
        (4, 0),
        (4, 0),
        (4, 0),
        (4, 0),
        (0, 4),
        (0, 4),
        (0, 4),
        (0, 4),
        (-4, 0),
        (-4, 0),
        (-4, 0),
        (-4, 0),
        (0, -4),
        (0, -4),
        (0, -4),
        (0, -4),
    ] {
        if dx != 0 {
            out.push(RawEvent::new(t, ev::REL, rel::X, dx));
        }
        if dy != 0 {
            out.push(RawEvent::new(t, ev::REL, rel::Y, dy));
        }
        out.push(RawEvent::new(t, ev::SYN, syn::REPORT, 0));
        t += 1_000;
    }
    out
}

/// Read a raw evdev dump (the `ldp-input-debug` file format).
///
/// # Errors
/// [`ToolError::Io`] on read failure; a partial trailing record is
/// left over per the framing doctrine (the trace is what decoded).
pub fn read_trace(path: &std::path::Path) -> Result<Vec<RawEvent>> {
    let bytes = std::fs::read(path)?;
    let mut decoder = StreamDecoder::new();
    Ok(decoder.feed(&bytes))
}

/// One painted stroke point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StrokePoint {
    /// X on the canvas.
    pub x: u32,
    /// Y on the canvas.
    pub y: u32,
}

/// Run the example against `addr` with `trace`; returns the exit code
/// and the stroke it painted (the scenario asserts the pixels).
///
/// # Errors
/// [`ToolError`] on connection, bootstrap, or frame-cycle failures.
pub fn run(
    addr: &UnixAddr,
    trace: &[RawEvent],
    out: &mut dyn Write,
) -> Result<(i32, Vec<StrokePoint>)> {
    let mut session = ToolSession::connect(addr)?;
    session.bootstrap()?;
    let _ = writeln!(
        out,
        "paint: connected to {} — {} global(s)",
        addr.display_string(),
        session.globals().len()
    );

    // The input half: the real normalizer over a mouse-shaped spec
    // (relative axes plus the button block).
    let spec = DeviceSpec::new("pointer-paint", DeviceId::default())
        .with_bit(ev::REL, rel::X)
        .with_bit(ev::REL, rel::Y)
        .with_bit(ev::KEY, u16::try_from(btn::LEFT).unwrap_or(0x110));
    let mut normalizer = Normalizer::new(spec);
    let mut framer = Framer::new();

    let mut cycle = session.setup_surface(CANVAS, CANVAS, 2, BACKGROUND)?;
    let _ = writeln!(
        out,
        "paint: {CANVAS}x{CANVAS} canvas, surface {}",
        cycle.surface.id().as_u32()
    );

    // The stroke: start centered, follow the normalized motion.
    let mut cursor = (CANVAS / 2, CANVAS / 2);
    let mut stroke: Vec<StrokePoint> = Vec::new();
    let stride = ldp_tools::session::FrameCycle::stride(CANVAS);
    let mut frame_id: u64 = 0;

    for event in trace {
        let Some(frame) = framer.feed(*event) else {
            continue;
        };
        let inputs = normalizer.normalize_frame(&frame);
        let mut moved = false;
        for input in inputs {
            if let InputEvent::PointerMotion { dx, dy } = input {
                let x = i64::from(cursor.0) + i64::from(dx);
                let y = i64::from(cursor.1) + i64::from(dy);
                // The canvas clamps at its edges (the pointer can
                // wander off-canvas; the paint simply stops).
                if x >= 0
                    && y >= 0
                    && (x as u64) < u64::from(CANVAS)
                    && (y as u64) < u64::from(CANVAS)
                {
                    cursor = (x as u32, y as u32);
                    stroke.push(StrokePoint {
                        x: cursor.0,
                        y: cursor.1,
                    });
                    moved = true;
                }
            }
        }
        if !moved {
            continue;
        }
        // Paint every stroke pixel so far into *every* buffer's region
        // of the pool (round-robin attach means either one can go
        // live next) and commit the whole canvas (damage accounting is
        // the compositor's; correctness here is the pixels).
        frame_id += 1;
        let _ = writeln!(
            out,
            "paint: frame {frame_id} → cursor ({}, {})",
            cursor.0, cursor.1
        );
        let buffer_bytes = u64::from(stride) * u64::from(CANVAS);
        let mut bytes = vec![BACKGROUND; usize::try_from(cycle.pool.size).unwrap_or(0)];
        for buffer in 0..cycle.buffers.len() as u64 {
            for point in &stroke {
                let offset = buffer * buffer_bytes
                    + u64::from(point.y) * u64::from(stride)
                    + u64::from(point.x) * 4;
                let at = usize::try_from(offset).unwrap_or(0);
                if at + 4 <= bytes.len() {
                    bytes[at..at + 4].copy_from_slice(&STROKE.to_le_bytes());
                }
            }
        }
        cycle.pool.write_at(0, &bytes)?;
        session.commit_frame(&mut cycle, frame_id, &[Rect::new(0, 0, CANVAS, CANVAS)])?;
    }

    // The final verdict: the last frame must reach scanout.
    match session.wait_presented(&cycle.surface, frame_id, ldp_tools::session::WAIT) {
        Ok(Some(sample)) => {
            let _ = writeln!(
                out,
                "paint: frame {frame_id} presented at {} ns — {} stroke pixel(s)",
                sample.ts_ns,
                stroke.len()
            );
        }
        Ok(None) => {
            let _ = writeln!(out, "paint: final frame dropped (no verdict)");
        }
        Err(e) => return Err(e),
    }
    Ok((0, stroke))
}

/// Resolve the socket the same way the tools do.
///
/// # Errors
/// A usage-shaped [`ToolError`] when neither source names a socket.
pub fn resolve_socket(args: &PaintArgs) -> Result<UnixAddr, ToolError> {
    let env = std::env::var("LDP_SOCKET").ok();
    let name = args.socket.as_deref().or(env.as_deref()).map(str::to_owned);
    let Some(name) = name else {
        return Err(ToolError::Logic(
            "no socket name: pass --socket NAME or set LDP_SOCKET".to_owned(),
        ));
    };
    let name = name.strip_prefix('@').unwrap_or(&name).to_owned();
    if name.is_empty() {
        return Err(ToolError::Logic("socket name is empty".to_owned()));
    }
    UnixAddr::abstract_name(name.as_bytes()).map_err(|e| ToolError::Logic(format!("{e}")))
}
