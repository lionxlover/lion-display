//! `ldp-grab` — output frame capture: screenshots from a live compositor.
//!
//! ```text
//! ldp-grab [--socket NAME] [--out FILE] [--format png|ppm] [--count N]
//!          [--interval SECS]
//! ```
//!
//! Binds the Phase 22 `ldp.capture.capture_manager` global, sends `grab`,
//! and receives the read-once frame snapshot: premultiplied ARGB8888
//! words, `width × height`. The frame is un-premultiplied to straight RGB
//! (the dump convention — composited over opaque black) and written as a
//! PNG (the `ldp-png` encoder, deterministic output) or a raw PPM (P6).
//!
//! `--count N` grabs N frames `--interval` seconds apart (screencast-ish
//! capture without the streaming protocol: honest snapshots, no teardown
//! contract), naming files `out-<k>.<ext>`. Works unchanged over the
//! remote transport — dial the edge gateway's socket and the frame ships
//! through the relay's whole-file vocabulary.

#![forbid(unsafe_code)]

use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use ldp_png::{ColorType, PngEncoder, PngError};

use crate::args::{Args, UsageError};
use crate::session::ToolSession;
use crate::socket;

/// The capture interface this tool drives.
const CAPTURE: &str = "ldp.capture.capture_manager";

/// Parsed command line.
#[derive(Clone, Debug)]
pub struct GrabArgs {
    /// `--socket NAME` / `LDP_SOCKET`.
    pub socket: Option<String>,
    /// `--out FILE` (default `ldp-grab`).
    pub out: PathBuf,
    /// `--format png|ppm` (default png).
    pub png: bool,
    /// `--count N` frames (default 1).
    pub count: u32,
    /// `--interval SECS` between frames (default 0).
    pub interval: f64,
}

/// Parse the command line.
///
/// # Errors
/// [`UsageError`] on unknown arguments or malformed values.
pub fn parse(mut argv: Args) -> Result<GrabArgs, UsageError> {
    let socket = argv.take_value("--socket")?;
    let out: PathBuf = match argv.take_value("--out")? {
        Some(p) => PathBuf::from(p),
        None => PathBuf::from("ldp-grab"),
    };
    let mut png = true;
    if let Some(fmt) = argv.take_value("--format")? {
        match fmt.as_str() {
            "png" => png = true,
            "ppm" => png = false,
            other => return Err(UsageError(format!("unknown --format '{other}' (png|ppm)"))),
        }
    }
    let count = argv
        .take_value("--count")?
        .map(|v| {
            v.parse::<u32>()
                .map_err(|_| UsageError(format!("--count '{v}' is not a number")))
        })
        .transpose()?
        .unwrap_or(1);
    let interval = argv
        .take_value("--interval")?
        .map(|v| {
            v.parse::<f64>()
                .map_err(|_| UsageError(format!("--interval '{v}' is not a number")))
        })
        .transpose()?
        .unwrap_or(0.0);
    if count == 0 {
        return Err(UsageError("--count must be at least 1".into()));
    }
    argv.require_empty()?;
    Ok(GrabArgs {
        socket,
        out,
        png,
        count,
        interval,
    })
}

/// The usage text.
#[must_use]
pub fn usage() -> String {
    "ldp-grab — capture output frames from a live compositor\n\
     \n\
     USAGE:\n\
     \x20 ldp-grab [--socket NAME] [--out FILE] [--format png|ppm]\n\
     \x20            [--count N] [--interval SECS]\n\
     \n\
     OPTIONS:\n\
     \x20 --socket NAME     abstract socket name (or LDP_SOCKET)\n\
     \x20 --out FILE        output path (default: ldp-grab[.png|.ppm])\n\
     \x20 --format FMT      png (default) or ppm (raw P6)\n\
     \x20 --count N         grab N frames (default 1; files get -<k> suffixes)\n\
     \x20 --interval SECS   seconds between frames (default 0)\n\
     \x20 --help | --version  this text\n\
     \n\
     Requires the server to advertise ldp.capture.capture_manager\n\
     (the Phase 22 capture surface). Over the remote transport, dial\n\
     \x20 the edge gateway's socket: frames relay as whole-file snapshots."
        .to_owned()
}

/// One captured frame.
struct Frame {
    width: u32,
    height: u32,
    /// Premultiplied ARGB words, little-endian bytes as shipped.
    words: Vec<u32>,
}

/// Run the tool; returns the process exit code.
///
/// # Errors
/// [`UsageError`] only; protocol failures print and return 1.
pub fn run(args: &GrabArgs, out: &mut dyn Write) -> Result<i32, UsageError> {
    let addr = socket::resolve(
        args.socket.as_deref(),
        std::env::var("LDP_SOCKET").ok().as_deref(),
    )?;
    let mut session = ToolSession::connect(&addr)
        .map_err(|e| UsageError(format!("connect {}: {e}", addr.display_string())))?;
    session
        .bootstrap()
        .map_err(|e| UsageError(format!("bootstrap: {e}")))?;
    if !session.has_global(CAPTURE) {
        let _ = writeln!(
            out,
            "ldp-grab: this server does not advertise {CAPTURE}; nothing to capture"
        );
        return Ok(1);
    }
    let capture = session
        .bind(CAPTURE)
        .map_err(|e| UsageError(format!("bind {CAPTURE}: {e}")))?;
    let interval = if args.interval > 0.0 {
        Some(Duration::from_secs_f64(args.interval))
    } else {
        None
    };
    let mut failures = 0u32;
    for k in 0..args.count {
        if k > 0 {
            if let Some(wait) = interval {
                std::thread::sleep(wait);
            }
        }
        match grab_one(&mut session, &capture) {
            Ok(frame) => {
                // File name: the stem (extension stripped only when it
                // already names the output format), a per-frame suffix
                // for multi-grab, and the format's extension.
                let ext = if args.png { "png" } else { "ppm" };
                let stem = args
                    .out
                    .file_name()
                    .map_or_else(String::new, |f| f.to_string_lossy().into_owned());
                let stem = if stem.ends_with(&format!(".{ext}")) {
                    stem[..stem.len() - ext.len() - 1].to_owned()
                } else {
                    stem
                };
                let suffix = if args.count > 1 {
                    format!("-{}", k + 1)
                } else {
                    String::new()
                };
                let path = args.out.with_file_name(format!("{stem}{suffix}.{ext}"));
                match write_frame(&path, &frame, args.png) {
                    Ok(()) => {
                        let _ = writeln!(
                            out,
                            "grabbed {}x{} -> {}",
                            frame.width,
                            frame.height,
                            path.display()
                        );
                    }
                    Err(e) => {
                        let _ = writeln!(out, "ldp-grab: write {}: {e}", path.display());
                        failures += 1;
                    }
                }
            }
            Err(e) => {
                let _ = writeln!(out, "ldp-grab: grab {k} failed: {e}");
                failures += 1;
            }
        }
    }
    Ok(i32::from(failures > 0))
}

/// One grab round: send `grab`, wait for the frame (or `failed`) event.
fn grab_one(session: &mut ToolSession, capture: &ldp_client::Proxy) -> std::io::Result<Frame> {
    session
        .connection()
        .send_request(capture, "grab", Vec::new())
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    let got = session
        .wait_until(
            |records| {
                records.iter().any(|r| {
                    r.event == "frame" && r.snapshot.is_some()
                        || (r.interface == "ldp.capture.capture_manager" && r.event == "failed")
                })
            },
            crate::session::WAIT,
        )
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    if !got {
        return Err(std::io::Error::other("timed out waiting for the frame"));
    }
    if let Some(f) = session.last_event("frame") {
        if f.interface == CAPTURE {
            let width = f.u32_arg("width").unwrap_or(0);
            let height = f.u32_arg("height").unwrap_or(0);
            let bytes = f.snapshot.clone().unwrap_or_default();
            if bytes.len() != width as usize * height as usize * 4 {
                return Err(std::io::Error::other(format!(
                    "snapshot length {} != {}x{}x4",
                    bytes.len(),
                    width,
                    height
                )));
            }
            let words: Vec<u32> = bytes
                .chunks_exact(4)
                .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect();
            return Ok(Frame {
                width,
                height,
                words,
            });
        }
    }
    if session
        .events_of("failed")
        .iter()
        .any(|r| r.interface == CAPTURE)
    {
        return Err(std::io::Error::other(
            "the server could not capture (out_of_memory)",
        ));
    }
    Err(std::io::Error::other("no frame event arrived"))
}

/// Un-premultiply and write one frame.
fn write_frame(path: &std::path::Path, frame: &Frame, png: bool) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut rgb = Vec::with_capacity(frame.words.len() * 3);
    for word in &frame.words {
        let a = (word >> 24) & 0xFF;
        let r = (word >> 16) & 0xFF;
        let g = (word >> 8) & 0xFF;
        let b = word & 0xFF;
        let (r, g, b) = if a == 0 {
            (0, 0, 0)
        } else if a == 255 {
            (r, g, b)
        } else {
            (
                ((r * 255 + a / 2) / a).min(255),
                ((g * 255 + a / 2) / a).min(255),
                ((b * 255 + a / 2) / a).min(255),
            )
        };
        rgb.extend_from_slice(&[r as u8, g as u8, b as u8]);
    }
    if png {
        let mut encoder = PngEncoder::new();
        let file = encoder
            .encode(frame.width, frame.height, ColorType::Rgb8, &rgb)
            .map_err(png_io)?;
        std::fs::write(path, file)
    } else {
        let mut file = std::fs::File::create(path)?;
        write!(file, "P6\n{} {}\n255\n", frame.width, frame.height)?;
        file.write_all(&rgb)
    }
}

fn png_io(e: PngError) -> std::io::Error {
    std::io::Error::other(e.to_string())
}
