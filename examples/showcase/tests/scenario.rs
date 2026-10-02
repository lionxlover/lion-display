//! The scripted scenario: showcase against the live Phase 10
//! compositor, brought up in-process (the testbench pattern — the
//! real accept loop on a background thread, the real transport, the
//! real pipeline).
//!
//! Exit-criterion shape: the example completes its full choreography
//! — connect, bootstrap, a 960×540 surface, four committed animated
//! frames each with a presentation verdict — and the compositor's
//! scanout carries the client-authored pixels *exactly*: every pixel
//! of the final frame matches the deterministic render, and the
//! desktop behind the window stays black.

#![allow(dead_code)] // the bench keeps helpers the suite subsets vary on
#![allow(clippy::missing_panics_doc)] // test scaffolding: panics are the failure mode

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
fn showcase_completes_and_scanout_is_pixel_exact() {
    let tb = Testbench::start("showcase");

    let frames_before = tb.frames();

    let mut out = Vec::new();
    let code = showcase::run(&tb.addr, &mut out).expect("showcase runs");
    let text = String::from_utf8(out).expect("utf8");
    assert_eq!(code, 0, "showcase output:\n{text}");

    // The choreography, in order, each stage named.
    assert!(
        text.contains("connected to ") && text.contains("global(s) advertised"),
        "{text}"
    );
    assert!(
        text.contains("960x540 surface") && text.contains("buffer(s)"),
        "{text}"
    );
    for id in 1..=showcase::FRAMES {
        let committed = format!("frame {id} committed and live");
        let presented = format!("frame {id} presented at ");
        assert!(text.contains(&committed), "{text}");
        assert!(text.contains(&presented), "{text}");
    }
    assert!(
        text.contains(&format!(
            "{} frame(s) presented — goodbye",
            showcase::FRAMES
        )),
        "{text}"
    );

    // The compositor rendered the client's frames…
    let deadline = std::time::Instant::now() + WAIT;
    while tb.frames() <= frames_before {
        assert!(deadline > std::time::Instant::now(), "no frame rendered");
        std::thread::sleep(Duration::from_millis(5));
    }

    // …and the scanout is pixel-exact: every window pixel equals the
    // deterministic final render (XRGB8888 little-endian word
    // 0xFFrrggbb → premultiplied ARGB at full alpha), and the desktop
    // behind stays black.
    let scanout = tb.scanout();
    assert_eq!(scanout.len(), 1920 * 1080, "the mock mode is 1080p");
    let rgb = showcase::render_frame(showcase::FRAMES - 1);
    let panel = showcase::render_panel(showcase::FRAMES - 1);
    let w = showcase::W as usize;
    let h = showcase::H as usize;
    let stride = 1920usize;
    let mul255 = |v: u32, a: u32| (v * a + 127) / 255;
    let over = |s: u32, d: u32| -> u32 {
        let sa = s >> 24;
        let inv = 255 - sa;
        let chan = |sc: u32, dc: u32| (sc + mul255(dc, inv)).min(255);
        0xFF00_0000u32
            | chan(s >> 16 & 0xFF, d >> 16 & 0xFF) << 16
            | chan(s >> 8 & 0xFF, d >> 8 & 0xFF) << 8
            | chan(s & 0xFF, d & 0xFF)
    };
    for y in 0..h {
        for x in 0..w {
            let src = (y * w + x) * 3;
            let mut expected: u32 = 0xFF00_0000
                | (u32::from(rgb[src]) << 16)
                | (u32::from(rgb[src + 1]) << 8)
                | u32::from(rgb[src + 2]);
            // The glass panel rides the top rows — at the testbench's
            // default Minimal tier it blends plainly (no system
            // material): haze and pills over the wallpaper.
            if y < showcase::PANEL_H as usize {
                expected = over(panel[y * w + x], expected);
            }
            assert_eq!(
                scanout[y * stride + x],
                expected,
                "pixel mismatch at ({x}, {y})"
            );
        }
    }
    // The desktop behind the window: black, untouched.
    assert_eq!(scanout[w], 0xFF00_0000, "column after the window");
    assert_eq!(scanout[h * stride], 0xFF00_0000, "row under the window");
    assert_eq!(scanout[1919 + 1079 * stride], 0xFF00_0000, "far corner");
}

#[test]
fn showcase_reports_a_dead_socket_honestly() {
    // A name nothing listens on: connect must fail, the example must
    // say so and exit 1 — never hang, never fabricate success.
    let addr = ldp_transport::UnixAddr::abstract_name(b"lion-ex-showcase-dead-socket")
        .expect("abstract name");
    let mut out = Vec::new();
    match showcase::run(&addr, &mut out) {
        Ok(code) => panic!("dead socket must not succeed (code {code})"),
        Err(e) => assert!(!e.to_string().is_empty()),
    }
}

/// Phase 23's uniform version surface: `showcase --version` exits 0 and
/// prints exactly `showcase <workspace version>`.
#[test]
fn version_flag_reports_the_release() {
    let exe = env!("CARGO_BIN_EXE_showcase");
    let out = std::process::Command::new(exe)
        .arg("--version")
        .output()
        .expect("spawn example");
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        text.trim(),
        format!("showcase {}", env!("CARGO_PKG_VERSION"))
    );
}
