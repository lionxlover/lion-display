//! The scripted scenario: hello-ldp against the live Phase 10
//! compositor, brought up in-process (the testbench pattern — the
//! real accept loop on a background thread, the real transport, the
//! real pipeline).
//!
//! Exit-criterion shape: the example completes its full choreography
//! — connect, bootstrap, bind, pool + buffers + surface, one committed
//! frame, a presentation verdict — and the compositor actually
//! rendered the client's fill into scanout.

#![allow(dead_code)] // the bench keeps helpers the suite subsets vary on
#![allow(clippy::missing_panics_doc)] // test scaffolding: panics are the failure mode // the shared bench keeps helpers the suite subsets vary on

use std::sync::Arc;
use std::time::Duration;

use lion_compositor::server::{Compositor, CompositorConfig};
use lion_compositor::Shared;

/// A running in-process compositor (the Phase 10 testbench, kept to
/// the surface the examples need).
pub struct Testbench {
    /// The shared world (scanout + frame introspection).
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
            socket: format!("lion-ex-{tag}-{n}-{}", std::process::id()),
            ..CompositorConfig::default()
        };
        let compositor = Compositor::headless(config).expect("headless bring-up");
        let addr = compositor.addr.clone();
        let shared = Arc::clone(&compositor.shared);
        let accept = std::thread::Builder::new()
            .name(format!("accept-{tag}"))
            .spawn(move || {
                let mut compositor = compositor;
                // Runs until the test process ends; the listener parks
                // in accept between connections.
                let _ = compositor.serve_blocking();
            })
            .expect("accept thread");
        Testbench {
            shared,
            addr,
            _accept: accept,
        }
    }

    /// The compositor's current scanout (premultiplied ARGB words).
    pub fn scanout(&self) -> Vec<u32> {
        let world = self.shared.world.lock().expect("world lock");
        world.scanout_words().expect("the compositor is lit")
    }

    /// Frames rendered so far.
    pub fn frames(&self) -> u64 {
        let world = self.shared.world.lock().expect("world lock");
        world.frames
    }
}

/// A generous wall-clock budget for CI scheduling noise.
const WAIT: Duration = Duration::from_secs(10);

#[test]
fn hello_completes_and_the_frame_reaches_scanout() {
    let tb = Testbench::start("hello");

    // Before: a quiet desktop.
    let frames_before = tb.frames();

    let mut out = Vec::new();
    let code = hello_ldp::run(&tb.addr, &mut out).expect("hello runs");
    let text = String::from_utf8(out).expect("utf8");
    assert_eq!(code, 0, "hello output:\n{text}");

    // The choreography, in order, each stage named.
    assert!(
        text.contains("connected to ") && text.contains("global(s) advertised"),
        "{text}"
    );
    assert!(text.contains("ldp.core.compositor"), "{text}");
    assert!(text.contains("ldp.core.shm"), "{text}");
    assert!(text.contains("ldp.core.output"), "{text}");
    assert!(text.contains("surface"), "{text}");
    assert!(text.contains("frame 1 committed and live"), "{text}");
    assert!(text.contains("frame 1 presented at "), "{text}");
    assert!(text.contains("goodbye"), "{text}");

    // The compositor rendered at least one frame for the client…
    let deadline = std::time::Instant::now() + WAIT;
    while tb.frames() <= frames_before {
        assert!(deadline > std::time::Instant::now(), "no frame rendered");
        std::thread::sleep(Duration::from_millis(5));
    }
    // …and the client's fill (0x2A) is the pixel content: the surface
    // sits at (0, 0), 64x64 — XRGB8888 0x2A2A2A2A becomes premultiplied
    // ARGB 0xFF2A2A2A, and word 0 carries it.
    let scanout = tb.scanout();
    assert_eq!(scanout.len(), 1920 * 1080, "the mock mode is 1080p");
    assert_eq!(scanout[0], 0xFF2A_2A2A, "top-left carries the fill");
    assert_eq!(scanout[63], 0xFF2A_2A2A, "row 0, column 63");
    assert_eq!(scanout[63 * 1920], 0xFF2A_2A2A, "row 63, column 0");
    assert_eq!(scanout[63 * 1920 + 63], 0xFF2A_2A2A, "row 63, column 63");
    // …and only the 64x64 quad: the desktop behind stays black.
    assert_eq!(scanout[64], 0xFF00_0000, "column 64 is the desktop");
    assert_eq!(scanout[64 * 1920], 0xFF00_0000, "row 64 is the desktop");
}

#[test]
fn hello_reports_a_dead_socket_honestly() {
    // A name nothing listens on: connect must fail, the example must
    // say so and exit 1 — never hang, never fabricate success.
    let addr = ldp_transport::UnixAddr::abstract_name(b"lion-ex-hello-dead-socket")
        .expect("abstract name");
    let mut out = Vec::new();
    match hello_ldp::run(&addr, &mut out) {
        Ok(code) => panic!("dead socket must not succeed (code {code})"),
        Err(e) => assert!(!e.to_string().is_empty()),
    }
}

/// Phase 23's uniform version surface: `hello-ldp --version` exits 0 and
/// prints exactly `hello-ldp <workspace version>`.
#[test]
fn version_flag_reports_the_release() {
    let exe = env!("CARGO_BIN_EXE_hello-ldp");
    let out = std::process::Command::new(exe)
        .arg("--version")
        .output()
        .expect("spawn example");
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        text.trim(),
        format!("hello-ldp {}", env!("CARGO_PKG_VERSION"))
    );
}
