//! The scripted scenarios: the in-process negotiation completes with
//! the payload intact, and the live mode reports the Phase 10
//! compositor's (absent) clipboard family honestly.

#![allow(dead_code)] // the bench keeps helpers the suite subsets vary on
#![allow(clippy::missing_panics_doc)] // test scaffolding: panics are the failure mode // the shared bench keeps helpers the suite subsets vary on

use std::sync::Arc;

use lion_compositor::server::{Compositor, CompositorConfig};
use lion_compositor::Shared;

/// A running in-process compositor (the Phase 10 testbench, kept to
/// the surface the examples need).
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
}

#[test]
fn the_negotiation_completes_with_the_payload_intact() {
    let args = clip_client::ClipArgs::default();
    let mut out = Vec::new();
    let code = clip_client::run(&args, &mut out).expect("negotiation runs");
    let text = String::from_utf8(out).expect("utf8");
    assert_eq!(code, 0, "clip output:\n{text}");

    // The choreography, in the protocol's own order.
    assert!(
        text.contains("two clients bound data devices on seat"),
        "{text}"
    );
    assert!(
        text.contains("set the selection — offers text/plain;charset=utf-8, image/png"),
        "{text}"
    );
    // The receiver's event cascade: data_offer, offer, offer,
    // selection (the manager routes exactly four).
    assert!(text.contains("data_device.data_offer"), "{text}");
    assert!(text.contains("data_device.selection"), "{text}");
    assert!(text.contains("sees the selection offer"), "{text}");
    assert!(text.contains("accepted text/plain;charset=utf-8"), "{text}");
    // The source learned the target (the routed data_source.target).
    assert!(text.contains("data_source.target"), "{text}");
    assert!(text.contains("transfer 0 admitted"), "{text}");
    assert!(text.contains("data_source.send"), "{text}");
    // The payload crossed the pipe, byte-exact.
    assert!(text.contains("the source served 23 byte(s)"), "{text}");
    assert!(
        text.contains("the receiver read 23 byte(s): \"the lion sleeps tonight\""),
        "{text}"
    );
}

#[test]
fn live_mode_reports_the_served_data_family() {
    let tb = Testbench::start("clip");
    let args = clip_client::ClipArgs {
        socket: Some(tb.addr.display_string()),
        live: true,
    };
    let mut out = Vec::new();
    let code = clip_client::run(&args, &mut out).expect("live probe runs");
    let text = String::from_utf8(out).expect("utf8");
    assert_eq!(code, 0, "clip output:\n{text}");

    // The served census (Phase 32): eight globals — the core trio,
    // the capture manager, the seat and shell service surfaces, the
    // data family's manager, and the built-in registry. The
    // clipboard is served: a full stack answers.
    assert!(text.contains("8 global(s) advertised"), "{text}");
    assert!(
        text.contains("ldp.data.data_device_manager is served"),
        "{text}"
    );
}

#[test]
fn live_mode_without_a_socket_names_the_convention() {
    // No --socket, no LDP_SOCKET (scrubbed for this test's fork): the
    // error must name both, and the exit code is a failure.
    let previous = std::env::var("LDP_SOCKET").ok();
    std::env::remove_var("LDP_SOCKET");
    let args = clip_client::ClipArgs {
        socket: None,
        live: true,
    };
    let mut out = Vec::new();
    let result = clip_client::run(&args, &mut out);
    if let Some(value) = previous {
        std::env::set_var("LDP_SOCKET", value);
    }
    let err = match result {
        Ok(code) => panic!("no socket must not succeed (code {code})"),
        Err(e) => e.to_string(),
    };
    assert!(err.contains("--socket"), "{err}");
    assert!(err.contains("LDP_SOCKET"), "{err}");
}

/// Phase 23's uniform version surface: `clip-client --version` exits 0 and
/// prints exactly `clip-client <workspace version>`.
#[test]
fn version_flag_reports_the_release() {
    let exe = env!("CARGO_BIN_EXE_clip-client");
    let out = std::process::Command::new(exe)
        .arg("--version")
        .output()
        .expect("spawn example");
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        text.trim(),
        format!("clip-client {}", env!("CARGO_PKG_VERSION"))
    );
}
