//! The scripted scenario: pointer-paint against the live Phase 10
//! compositor. The evdev trace runs through the real normalizer, the
//! stroke lands pixel-exact in scanout, and the final frame's
//! presentation verdict comes back.

#![allow(dead_code)] // the bench keeps helpers the suite subsets vary on
#![allow(clippy::missing_panics_doc)] // test scaffolding: panics are the failure mode // the shared bench keeps helpers the suite subsets vary on

use std::sync::Arc;

use ldp_input::codes::{ev, rel, syn};
use ldp_input::evdev::RawEvent;

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
    /// Panics on a dark world — this example's scenario never goes
    /// dark (the dark state is the compositor suite's business).
    pub fn scanout(&self) -> Vec<u32> {
        let world = self.shared.world.lock().expect("world lock");
        world.scanout_words().expect("the compositor is lit")
    }
}

/// The scanout word at canvas (x, y) — the surface sits at (0, 0) and
/// the compositor's mode is 1920×1080.
fn word_at(scanout: &[u32], x: u32, y: u32) -> u32 {
    scanout[y as usize * 1920 + x as usize]
}

#[test]
fn the_stroke_lands_pixel_exact() {
    let tb = Testbench::start("paint");

    let mut out = Vec::new();
    let (code, stroke) =
        pointer_paint::run(&tb.addr, &pointer_paint::demo_trace(), &mut out).expect("paint runs");
    let text = String::from_utf8(out).expect("utf8");
    assert_eq!(code, 0, "paint output:\n{text}");

    // The choreography named itself.
    assert!(text.contains("connected to "), "{text}");
    assert!(text.contains("canvas"), "{text}");
    assert!(text.contains("frame 16 → cursor (64, 64)"), "{text}");
    assert!(text.contains("presented at "), "{text}");
    assert!(text.contains("16 stroke pixel(s)"), "{text}");

    // The demo trace walks a closed 16-px square from (64, 64).
    assert_eq!(stroke.len(), 16, "the trace paints 16 points");
    assert_eq!(stroke[0], pointer_paint::StrokePoint { x: 68, y: 64 });
    assert_eq!(stroke[3], pointer_paint::StrokePoint { x: 80, y: 64 });
    assert_eq!(stroke[7], pointer_paint::StrokePoint { x: 80, y: 80 });
    assert_eq!(stroke[11], pointer_paint::StrokePoint { x: 64, y: 80 });
    assert_eq!(stroke[15], pointer_paint::StrokePoint { x: 64, y: 64 });

    // Wait for the final frame to reach scanout, then assert pixels:
    // every stroke point is white, the canvas interior is background.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let scanout = tb.scanout();
        let painted = stroke
            .iter()
            .all(|p| word_at(&scanout, p.x, p.y) == 0xFFFF_FFFF);
        if painted || deadline <= std::time::Instant::now() {
            for point in &stroke {
                assert_eq!(
                    word_at(&scanout, point.x, point.y),
                    0xFFFF_FFFF,
                    "stroke pixel ({}, {}) — output:\n{text}",
                    point.x,
                    point.y
                );
            }
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let scanout = tb.scanout();
    // The interior of the square (never touched) carries the fill.
    assert_eq!(word_at(&scanout, 72, 72), 0xFF11_1111, "canvas interior");
    // Off-canvas is the desktop.
    assert_eq!(
        word_at(&scanout, 128, 0),
        0xFF00_0000,
        "past the canvas edge"
    );
}

#[test]
fn a_two_packet_trace_paints_its_points_and_nothing_else() {
    let tb = Testbench::start("paint2");
    // Two packets: (3, -1) then (2, 2) — from (64, 64) the stroke
    // visits (67, 63) and (69, 65).
    let trace = vec![
        RawEvent::new(100, ev::REL, rel::X, 3),
        RawEvent::new(100, ev::REL, rel::Y, -1),
        RawEvent::new(100, ev::SYN, syn::REPORT, 0),
        RawEvent::new(200, ev::REL, rel::X, 2),
        RawEvent::new(200, ev::REL, rel::Y, 2),
        RawEvent::new(200, ev::SYN, syn::REPORT, 0),
    ];
    let mut out = Vec::new();
    let (code, stroke) = pointer_paint::run(&tb.addr, &trace, &mut out).expect("paint runs");
    assert_eq!(code, 0);
    assert_eq!(
        stroke,
        vec![
            pointer_paint::StrokePoint { x: 67, y: 63 },
            pointer_paint::StrokePoint { x: 69, y: 65 },
        ]
    );

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let scanout = tb.scanout();
        if word_at(&scanout, 67, 63) == 0xFFFF_FFFF && word_at(&scanout, 69, 65) == 0xFFFF_FFFF
            || deadline <= std::time::Instant::now()
        {
            assert_eq!(word_at(&scanout, 67, 63), 0xFFFF_FFFF);
            assert_eq!(word_at(&scanout, 69, 65), 0xFFFF_FFFF);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    // Nothing else: the previous positions stay background.
    let scanout = tb.scanout();
    assert_eq!(word_at(&scanout, 64, 64), 0xFF11_1111, "start untouched");
    assert_eq!(word_at(&scanout, 68, 64), 0xFF11_1111, "interim untouched");
}

/// Phase 23's uniform version surface: `pointer-paint --version` exits 0 and
/// prints exactly `pointer-paint <workspace version>`.
#[test]
fn version_flag_reports_the_release() {
    let exe = env!("CARGO_BIN_EXE_pointer-paint");
    let out = std::process::Command::new(exe)
        .arg("--version")
        .output()
        .expect("spawn example");
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        text.trim(),
        format!("pointer-paint {}", env!("CARGO_PKG_VERSION"))
    );
}
