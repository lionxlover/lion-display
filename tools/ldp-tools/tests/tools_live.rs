//! The Phase 18 exit criterion: **every tool runs against the Phase 10
//! compositor in CI** — the real compiled binaries (`CARGO_BIN_EXE_*`)
//! over the real abstract socket of an in-process compositor (the
//! testbench pattern: the accept loop on a background thread).
//!
//! Each tool gets its own compositor (fresh state, no cross-talk) and
//! must exit 0 with the expected report lines; the honest-degradation
//! probes (`ldp-audit --live`, `ldp-input-debug --live`) additionally
//! assert the exact NOT-served wording for the interfaces the Phase 10
//! vertical slice does not carry.

#![allow(dead_code)] // the bench keeps helpers the suite subsets vary on
#![allow(clippy::missing_panics_doc)] // test scaffolding: panics are the failure mode

use std::sync::Arc;

use lion_compositor::server::{Compositor, CompositorConfig};
use lion_compositor::Shared;

/// A running in-process compositor.
pub struct Testbench {
    /// The shared world.
    pub shared: Arc<Shared>,
    /// The listening abstract address.
    pub addr: ldp_transport::UnixAddr,
    _accept: std::thread::JoinHandle<()>,
}

static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl Testbench {
    /// Bring up the compositor and its accept loop.
    pub fn start(tag: &str) -> Testbench {
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let config = CompositorConfig {
            socket: format!("lion-tools-it-{tag}-{n}-{}", std::process::id()),
            ..CompositorConfig::default()
        };
        let compositor = Compositor::headless(config).expect("headless bring-up");
        let addr = compositor.addr.clone();
        let shared = Arc::clone(&compositor.shared);
        let accept = std::thread::Builder::new()
            .name(format!("accept-{tag}"))
            .spawn(move || {
                let mut compositor = compositor;
                let _ = compositor.serve_blocking();
            })
            .expect("accept thread");
        Testbench {
            shared,
            addr,
            _accept: accept,
        }
    }

    /// The socket name as the tools take it (the `@`-sigiled display
    /// form — exactly what a user would pass on the command line).
    pub fn socket_flag(&self) -> String {
        self.addr.display_string()
    }
}

/// Resolve one of this package's compiled binaries.
///
/// `env!()` — the compile-time form — is the canonical cargo contract
/// for `CARGO_BIN_EXE_*`: the values are frozen into the test binary
/// at build time, where the runtime environment is not guaranteed to
/// carry them through every harness. The seven binaries of this
/// package are a closed set, known here at compile time.
fn tool_exe(bin: &str) -> &'static str {
    match bin {
        "ldp-info" => env!("CARGO_BIN_EXE_ldp-info"),
        "ldp-debug" => env!("CARGO_BIN_EXE_ldp-debug"),
        "ldp-validate" => env!("CARGO_BIN_EXE_ldp-validate"),
        "ldp-profiler" => env!("CARGO_BIN_EXE_ldp-profiler"),
        "ldp-audit" => env!("CARGO_BIN_EXE_ldp-audit"),
        "ldp-input-debug" => env!("CARGO_BIN_EXE_ldp-input-debug"),
        "ldp-grab" => env!("CARGO_BIN_EXE_ldp-grab"),
        other => panic!("unknown ldp-tools binary {other:?}"),
    }
}

/// Run one of the compiled binaries; returns (exit code, combined
/// stdout+stderr text).
fn run_tool(bin: &str, args: &[&str]) -> (i32, String) {
    let exe = tool_exe(bin);
    let output = std::process::Command::new(exe)
        .args(args)
        .env_remove("LDP_SOCKET")
        .output()
        .expect("spawn tool");
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    (output.status.code().unwrap_or(-1), text)
}

#[test]
fn ldp_info_reports_the_live_session() {
    let tb = Testbench::start("info");
    let (code, text) = run_tool("ldp-info", &["--live", "--socket", &tb.socket_flag()]);
    assert_eq!(code, 0, "ldp-info output:\n{text}");
    assert!(text.contains("session: "), "{text}");
    assert!(text.contains("welcome: protocol release"), "{text}");
    assert!(text.contains("limits: "), "{text}");
    assert!(text.contains("advertised globals:"), "{text}");
    assert!(text.contains("ldp.core.compositor"), "{text}");
    assert!(text.contains("ldp.core.shm"), "{text}");
    assert!(text.contains("ldp.core.output"), "{text}");
    // The output block: the mock KMS device's cascade decodes.
    assert!(text.contains("output:"), "{text}");
    assert!(text.contains("modes: "), "{text}");
    // The served schema matches the compiled registry (the empty-name
    // introspection streams every interface — 34 since capture joined).
    assert!(
        text.contains("introspection: 34 schema payload(s), matches the compiled registry"),
        "{text}"
    );
}

#[test]
fn ldp_debug_traces_live_events() {
    let tb = Testbench::start("debug");
    let (code, text) = run_tool(
        "ldp-debug",
        &[
            "trace",
            "--socket",
            &tb.socket_flag(),
            "--duration",
            "1",
            "--generate",
            "4",
        ],
    );
    assert_eq!(code, 0, "ldp-debug output:\n{text}");
    assert!(text.contains("trace: "), "{text}");
    // The generated presentation frames show up as dispatched events.
    assert!(text.contains("[seq "), "{text}");
    // The per-class census ran to the duration's end.
    assert!(text.contains("summary:"), "{text}");
}

#[test]
fn ldp_validate_live_matches_the_compiled_schema() {
    let tb = Testbench::start("validate");
    let (code, text) = run_tool(
        "ldp-validate",
        &[
            "--live",
            "--socket",
            &tb.socket_flag(),
            // The spec directory relative to this crate's root (the
            // binary's CWD under cargo test).
            "--spec-dir",
            "../../spec",
        ],
    );
    assert_eq!(code, 0, "ldp-validate output:\n{text}");
    assert!(text.contains("spec OK: "), "{text}");
    assert!(
        text.contains("live: served schema matches the compiled registry"),
        "{text}"
    );
}

#[test]
fn ldp_profiler_measures_the_live_pipeline() {
    let tb = Testbench::start("profiler");
    let (code, text) = run_tool(
        "ldp-profiler",
        &[
            "--socket",
            &tb.socket_flag(),
            "--frames",
            "8",
            "--timeout",
            "10",
        ],
    );
    assert_eq!(code, 0, "ldp-profiler output:\n{text}");
    assert!(text.contains("frame(s) presented"), "{text}");
    // The deadline hit-rate line closed the report.
    assert!(text.contains("hit-rate"), "{text}");
}

#[test]
fn ldp_audit_live_reports_the_missing_broker() {
    let tb = Testbench::start("audit");
    let (code, text) = run_tool("ldp-audit", &["--live", "--socket", &tb.socket_flag()]);
    assert_eq!(code, 0, "ldp-audit output:\n{text}");
    assert!(text.contains("live: "), "{text}");
    // The current census: eight globals (core trio, capture manager,
    // the Phase 31 seat and shell surfaces, the Phase 32 data family
    // manager, the built-in registry).
    assert!(text.contains("8 global(s) advertised"), "{text}");
    assert!(
        text.contains("ldp.security.security is NOT served"),
        "{text}"
    );
}

#[test]
fn ldp_input_debug_live_reports_the_served_seat() {
    let tb = Testbench::start("input");
    let (code, text) = run_tool(
        "ldp-input-debug",
        &["--live", "--socket", &tb.socket_flag()],
    );
    assert_eq!(code, 0, "ldp-input-debug output:\n{text}");
    assert!(text.contains("live: "), "{text}");
    // Phase 31 wired the seat service surface into the served
    // globals — the live path now meets the input path it analyzes.
    assert!(text.contains("ldp.input.seat is served"), "{text}");
}

#[test]
fn a_dead_socket_is_reported_not_hung_on() {
    // Every live tool must fail fast and say so — never hang waiting
    // for a socket nothing listens on.
    for (bin, args) in [
        (
            "ldp-info",
            vec!["--live", "--socket", "@lion-tools-it-dead"],
        ),
        (
            "ldp-debug",
            vec![
                "trace",
                "--socket",
                "@lion-tools-it-dead",
                "--duration",
                "1",
            ],
        ),
        (
            "ldp-validate",
            vec![
                "--live",
                "--socket",
                "@lion-tools-it-dead",
                "--spec-dir",
                "../../spec",
            ],
        ),
        (
            "ldp-profiler",
            vec![
                "--socket",
                "@lion-tools-it-dead",
                "--frames",
                "1",
                "--timeout",
                "1",
            ],
        ),
        (
            "ldp-audit",
            vec!["--live", "--socket", "@lion-tools-it-dead"],
        ),
        (
            "ldp-input-debug",
            vec!["--live", "--socket", "@lion-tools-it-dead"],
        ),
    ] {
        let (code, text) = run_tool(bin, &args);
        assert_ne!(
            code, 0,
            "{bin} on a dead socket must fail (output:\n{text})"
        );
        assert!(
            text.contains("connect") || text.contains("failed"),
            "{bin} must name the failure (output:\n{text})"
        );
    }
}

// ---- the Phase 22 capture tool ---------------------------------------

/// `ldp-grab` captures the live compositor's frame: PNG out, exit 0, and
/// the file is a structurally valid PNG of the mock mode's geometry. The
/// PPM form is byte-verifiable: its pixels must equal the compositor's
/// scanout (un-premultiplied to straight RGB).
#[test]
fn ldp_grab_captures_the_live_frame() {
    let tb = Testbench::start("grab");
    let dir = std::env::temp_dir().join(format!("ldp-grab-it-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let png_path = dir.join("shot.png");
    let (code, text) = run_tool(
        "ldp-grab",
        &[
            "--socket",
            &tb.socket_flag(),
            "--out",
            png_path.to_str().unwrap(),
        ],
    );
    assert_eq!(code, 0, "ldp-grab output:\n{text}");
    assert!(
        text.contains("grabbed 1920x1080"),
        "the report line names the geometry: {text}"
    );
    let png = std::fs::read(&png_path).expect("the PNG exists");
    // Signature + IHDR + IEND: a valid PNG whose dims are the mode's.
    assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
    assert_eq!(&png[12..16], b"IHDR");
    let w = u32::from_be_bytes([png[16], png[17], png[18], png[19]]);
    let h = u32::from_be_bytes([png[20], png[21], png[22], png[23]]);
    assert_eq!((w, h), (1920, 1080));
    assert_eq!(&png[png.len() - 8..png.len() - 4], b"IEND");
    assert!(
        png.len() > 8_000,
        "1080p PNG should be non-trivial: {}",
        png.len()
    );

    // PPM: byte-exact against the scanout oracle.
    let ppm_path = dir.join("shot.ppm");
    let (code, text) = run_tool(
        "ldp-grab",
        &[
            "--socket",
            &tb.socket_flag(),
            "--out",
            ppm_path.to_str().unwrap(),
            "--format",
            "ppm",
        ],
    );
    assert_eq!(code, 0, "ldp-grab ppm output:\n{text}");
    let ppm = std::fs::read(&ppm_path).expect("the PPM exists");
    let header = "P6\n1920 1080\n255\n";
    assert_eq!(&ppm[..header.len()], header.as_bytes());
    // The empty scene scans out opaque black: straight RGB zeros.
    assert!(ppm[header.len()..].iter().all(|&b| b == 0));
    let _ = std::fs::remove_dir_all(&dir);
}

/// `--count 2` with an interval grabs two distinct files.
#[test]
fn ldp_grab_counts_multiple_frames() {
    let tb = Testbench::start("grab-multi");
    let dir = std::env::temp_dir().join(format!("ldp-grab-multi-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let (code, text) = run_tool(
        "ldp-grab",
        &[
            "--socket",
            &tb.socket_flag(),
            "--out",
            dir.join("seq").to_str().unwrap(),
            "--count",
            "2",
        ],
    );
    assert_eq!(code, 0, "ldp-grab multi output:\n{text}");
    assert!(dir.join("seq-1.png").is_file(), "first frame file");
    assert!(dir.join("seq-2.png").is_file(), "second frame file");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Usage errors exit 2 with the usage text; a bad format is rejected.
#[test]
fn ldp_grab_rejects_bad_arguments() {
    let (code, text) = run_tool("ldp-grab", &["--format", "jpeg"]);
    assert_eq!(code, 2, "{text}");
    assert!(text.contains("unknown --format 'jpeg'"), "{text}");
    assert!(text.contains("ldp-grab"), "{text}");
}

/// Phase 23's uniform version surface: every tool exits 0 and prints
/// exactly `<tool> <workspace version>` for both `--version` and `-V`.
#[test]
fn every_tool_reports_its_version() {
    let version = env!("CARGO_PKG_VERSION");
    for bin in [
        "ldp-info",
        "ldp-debug",
        "ldp-validate",
        "ldp-profiler",
        "ldp-audit",
        "ldp-input-debug",
        "ldp-grab",
    ] {
        for flag in ["--version", "-V"] {
            let (code, text) = run_tool(bin, &[flag]);
            assert_eq!(code, 0, "{bin} {flag}:\n{text}");
            assert_eq!(
                text.trim(),
                format!("{bin} {version}"),
                "{bin} {flag} must print exactly '<tool> <version>'"
            );
        }
    }
}
