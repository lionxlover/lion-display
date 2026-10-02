//! The remote relay's end-to-end loopback gates: real clients, both
//! gateways, real TCP, the real Phase 10 compositor — in-process.
//!
//! The flagship is the showcase scenario over the wire: an unmodified
//! client library session (introspection, globals replay, a 960×540
//! surface, ping-pong pools, four committed animated frames, each with
//! a presentation verdict and a release fence) carried through
//! edge → TCP → hub → compositor, with the compositor's scanout
//! proving **pixel-exact** delivery — the write-then-commit shm
//! contract, preserved across a network.
//!
//! The supporting gates: multi-client isolation, the token's
//! fast-fail, the length-bomb cap, keepalive teardown of a silent
//! peer, and FD-count stability across whole session lifecycles.
//!
//! The whole suite serializes on one gate: FD counts are asserted
//! process-wide (the `tests/` precedent).

#![allow(dead_code)] // the bench keeps helpers the suite subsets vary on
#![allow(clippy::missing_panics_doc)] // test scaffolding: panics are the failure mode

use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use ldp_client::EventHandler;
use ldp_remote::link::{Keepalive, AUTH_TOKEN_BYTES};
use ldp_remote::wire::{Envelope, EnvelopeKind, RemoteConfig, MAGIC, WIRE_VERSION};
use ldp_remote::{EdgeConfig, EdgeGateway, HubConfig, HubGateway};
use ldp_transport::UnixAddr;
use lion_compositor::server::{Compositor, CompositorConfig};
use lion_compositor::Shared;

/// The one cross-test gate (FD assertions are process-wide).
static GATE: Mutex<()> = Mutex::new(());

/// A running in-process compositor (the testbench pattern).
struct Bench {
    shared: Arc<Shared>,
    addr: UnixAddr,
}

static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn bench(tag: &str) -> Bench {
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let config = CompositorConfig {
        socket: format!("lion-rmt-{tag}-{n}-{}", std::process::id()),
        ..CompositorConfig::default()
    };
    let compositor = Compositor::headless(config).expect("headless bring-up");
    let addr = compositor.addr.clone();
    let shared = Arc::clone(&compositor.shared);
    std::thread::Builder::new()
        .name(format!("accept-{tag}"))
        .spawn(move || {
            let mut compositor = compositor;
            let _ = compositor.serve_blocking();
        })
        .expect("accept thread");
    Bench { shared, addr }
}

impl Bench {
    /// The compositor's current scanout (premultiplied ARGB words).
    fn scanout(&self) -> Vec<u32> {
        let world = self.shared.world.lock().expect("world lock");
        world.scanout_words().expect("the compositor is lit")
    }

    /// Frames rendered so far.
    fn frames(&self) -> u64 {
        let world = self.shared.world.lock().expect("world lock");
        world.frames
    }
}

/// One gateway pair around one compositor: hub on an ephemeral TCP
/// port, edge on a fresh abstract socket.
struct Remote {
    bench: Bench,
    hub: HubGateway,
    edge: EdgeGateway,
}

fn remote(tag: &str, token: [u8; AUTH_TOKEN_BYTES]) -> Remote {
    let bench = bench(tag);
    let hub = HubGateway::start(HubConfig {
        listen: String::from("127.0.0.1:0"),
        compositor: bench.addr.clone(),
        token,
        config: RemoteConfig::default(),
        keepalive: Keepalive::default(),
        ..HubConfig::default()
    })
    .expect("hub starts");
    let edge = EdgeGateway::start(EdgeConfig {
        listen: UnixAddr::abstract_name(format!("ldp-rmt-{tag}-{}", std::process::id()).as_bytes())
            .expect("abstract name"),
        remote: format!("{}", hub.local_addr()),
        token,
        config: RemoteConfig::default(),
        keepalive: Keepalive::default(),
        ..EdgeConfig::default()
    })
    .expect("edge starts");
    Remote { bench, hub, edge }
}

fn token(fill: u8) -> [u8; AUTH_TOKEN_BYTES] {
    [fill; AUTH_TOKEN_BYTES]
}

/// A generous wall-clock budget for CI scheduling noise.
const WAIT: Duration = Duration::from_secs(10);

/// The flagship: the showcase scenario, unmodified, over the wire —
/// and the scanout is pixel-exact.
#[test]
fn showcase_over_the_wire_is_pixel_exact() {
    let _gate = GATE.lock().expect("suite gate");
    let remote = remote("showcase", token(0xA7));

    let frames_before = remote.bench.frames();

    let mut out = Vec::new();
    let code = showcase::run(remote.edge.addr(), &mut out).expect("showcase runs");
    let text = String::from_utf8(out).expect("utf8");
    assert_eq!(code, 0, "showcase output:\n{text}");
    assert!(
        text.contains("connected to ") && text.contains("global(s) advertised"),
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
    while remote.bench.frames() <= frames_before {
        assert!(deadline > std::time::Instant::now(), "no frame rendered");
        std::thread::sleep(Duration::from_millis(5));
    }

    // …and the scanout is pixel-exact: every window pixel equals the
    // deterministic final render (XRGB8888 LE word 0xFFrrggbb →
    // premultiplied ARGB at full alpha), and the desktop behind the
    // window stays black.
    let scanout = remote.bench.scanout();
    assert_eq!(scanout.len(), 1920 * 1080, "the mock mode is 1080p");
    let rgb = showcase::render_frame(showcase::FRAMES - 1);
    let panel = showcase::render_panel(showcase::FRAMES - 1);
    let w = showcase::W as usize;
    let h = showcase::H as usize;
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
            // The glass panel blends plainly over the top rows at the
            // harness's default Minimal tier.
            if y < showcase::PANEL_H as usize {
                expected = over(panel[y * w + x], expected);
            }
            assert_eq!(
                scanout[y * 1920 + x],
                expected,
                "pixel ({x},{y}) diverged over the wire"
            );
        }
    }
    for y in h..1080 {
        for x in w..1920 {
            assert_eq!(
                scanout[y * 1920 + x],
                0xFF00_0000,
                "the desktop behind the window must stay black at ({x},{y})"
            );
        }
    }

    // Both gateways drain the session cleanly.
    assert!(
        remote.hub.wait_quiet(WAIT),
        "hub session did not drain: {} active",
        remote.hub.active_sessions()
    );
    assert!(
        remote.edge.wait_quiet(WAIT),
        "edge session did not drain: {} active",
        remote.edge.active_sessions()
    );
}

/// Two remote clients at once: each gets its own session, its own
/// presentation verdicts, and both complete the full choreography.
#[test]
fn two_concurrent_remote_clients_both_present() {
    let _gate = GATE.lock().expect("suite gate");
    let remote = remote("duo", token(0x5C));

    let addr = remote.edge.addr().clone();
    let first = std::thread::spawn(move || {
        let mut out = Vec::new();
        let code = showcase::run(&addr, &mut out).expect("first client runs");
        (code, String::from_utf8(out).expect("utf8"))
    });
    let addr = remote.edge.addr().clone();
    let second = std::thread::spawn(move || {
        let mut out = Vec::new();
        let code = showcase::run(&addr, &mut out).expect("second client runs");
        (code, String::from_utf8(out).expect("utf8"))
    });

    for (code, text) in [first.join().expect("first"), second.join().expect("second")] {
        assert_eq!(code, 0, "{text}");
        assert!(text.contains("4 frame(s) presented — goodbye"), "{text}");
    }
    assert!(remote.hub.wait_quiet(WAIT), "hub sessions drained");
    assert!(remote.edge.wait_quiet(WAIT), "edge sessions drained");
}

/// A wrong token refuses the session: the client's dial fast-fails
/// exactly like dialing a dead compositor socket, and neither gateway
/// leaks a session.
#[test]
fn wrong_token_fast_fails_the_client() {
    let _gate = GATE.lock().expect("suite gate");
    let remote = remote("auth", token(0x11));

    // A second edge with the WRONG token against the same hub.
    let imposter = EdgeGateway::start(EdgeConfig {
        listen: UnixAddr::abstract_name(
            format!("ldp-rmt-auth-bad-{}", std::process::id()).as_bytes(),
        )
        .expect("abstract name"),
        remote: format!("{}", remote.hub.local_addr()),
        token: token(0x22),
        config: RemoteConfig::default(),
        keepalive: Keepalive::default(),
        ..EdgeConfig::default()
    })
    .expect("imposter edge starts");

    let mut out = Vec::new();
    // The client dials the imposter edge; the hub refuses the token;
    // the edge drops the client. The expected outcome is a failed
    // session (a disconnect-flavored error), never success.
    match showcase::run(imposter.addr(), &mut out) {
        Ok(code) => panic!("the session must not succeed, exit code {code}"),
        Err(e) => {
            let text = String::from_utf8_lossy(&out);
            assert!(
                text.contains("connected") || text.is_empty(),
                "the client should fail at or before the handshake: {text} ({e})"
            );
        }
    }

    assert!(imposter.wait_quiet(WAIT), "imposter drained");
    assert_eq!(remote.hub.active_sessions(), 0, "no hub session leaked");
    assert_eq!(remote.edge.active_sessions(), 0);
}

/// A hostile length claim costs the hub exactly one header read: no
/// session, no allocation, connection dropped.
#[test]
fn length_bomb_is_rejected_without_a_session() {
    let _gate = GATE.lock().expect("suite gate");
    let remote = remote("bomb", token(0x99));

    let mut stream = std::net::TcpStream::connect(remote.hub.local_addr()).expect("dial the hub");
    // A syntactically valid HELLO header claiming ~2 GiB of body.
    let mut header = [0u8; 16];
    header[0..4].copy_from_slice(&MAGIC.to_le_bytes());
    header[4] = WIRE_VERSION;
    header[5] = EnvelopeKind::Hello as u8;
    header[8..12].copy_from_slice(&2_000_000_000u32.to_le_bytes());
    use std::io::Write as _;
    stream.write_all(&header).expect("send the bomb");
    // Any follow-up byte would be body; the hub must have hung up.
    std::thread::sleep(Duration::from_millis(200));
    let mut scratch = [0u8; 16];
    use std::io::Read as _;
    let read = stream.read(&mut scratch);
    assert!(
        matches!(read, Err(ref e) if e.kind() == std::io::ErrorKind::UnexpectedEof)
            || matches!(read, Ok(0)),
        "the hub must drop the bomb, got {read:?}"
    );

    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(remote.hub.active_sessions(), 0, "no session spawned");
}

/// A peer that says HELLO and then goes silent is torn down by the
/// keepalive monitor — the session joins, the descriptors close.
#[test]
fn silent_peer_is_torn_down_by_keepalive() {
    let _gate = GATE.lock().expect("suite gate");
    let bench = bench("silent");
    // An aggressive policy for the test: ping after 300 ms, dead after
    // 900 ms.
    let hub = HubGateway::start(HubConfig {
        listen: String::from("127.0.0.1:0"),
        compositor: bench.addr.clone(),
        token: token(0x33),
        config: RemoteConfig::default(),
        keepalive: Keepalive {
            idle_ping: Duration::from_millis(300),
            pong_deadline: Duration::from_millis(900),
        },
        ..HubConfig::default()
    })
    .expect("hub starts");

    // Dial, authenticate, then go silent while holding the socket.
    let mut stream = std::net::TcpStream::connect(hub.local_addr()).expect("dial");
    let hello = Envelope::new(EnvelopeKind::Hello, {
        let mut body = Vec::with_capacity(AUTH_TOKEN_BYTES + 12);
        body.extend_from_slice(&token(0x33));
        body.extend_from_slice(&RemoteConfig::default().max_envelope.to_le_bytes());
        body.extend_from_slice(&RemoteConfig::default().pool_total_cap.to_le_bytes());
        body
    });
    hello
        .write_to(&mut stream, RemoteConfig::default().envelope_cap())
        .expect("hello");
    // Consume the HELLO_ACK so the handshake completes, then wait for
    // the session to be counted (the hub registers it right after the
    // handshake returns — observable, eventually consistent).
    let _ack = ldp_remote::wire::read_envelope(&mut stream, RemoteConfig::default().envelope_cap())
        .expect("hello ack");
    let appeared = std::time::Instant::now() + Duration::from_secs(2);
    while hub.active_sessions() == 0 {
        assert!(
            std::time::Instant::now() < appeared,
            "the authenticated session was never counted"
        );
        std::thread::sleep(Duration::from_millis(5));
    }

    // Now: silence. The monitor must tear it down within its deadline
    // (plus scheduling slack).
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while hub.active_sessions() > 0 {
        assert!(
            deadline > std::time::Instant::now(),
            "the silent peer was never torn down"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Whole session lifecycles leave the process's descriptor count
/// exactly where it started.
#[test]
fn sessions_leave_no_fd_leaks() {
    let _gate = GATE.lock().expect("suite gate");
    let remote = remote("leak", token(0xD4));

    // Baseline after everything is listening.
    let before = ldp_transport::count_open_fds().expect("count fds");

    for round in 0..3 {
        let mut out = Vec::new();
        let code = showcase::run(remote.edge.addr(), &mut out).expect("client runs");
        assert_eq!(code, 0, "round {round} failed");
        assert!(remote.hub.wait_quiet(WAIT), "hub drained (round {round})");
        assert!(remote.edge.wait_quiet(WAIT), "edge drained (round {round})");
    }

    // Compositor-side reclamation is asynchronous with the gateways
    // going quiet; poll to the baseline with a deadline.
    let deadline = std::time::Instant::now() + WAIT;
    loop {
        let now = ldp_transport::count_open_fds().expect("count fds");
        if now == before {
            break;
        }
        assert!(
            deadline > std::time::Instant::now(),
            "fd count did not return to baseline: before {before}, after {now}"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// The Phase 22 capture surface over the wire: an unmodified client
/// binds `capture_manager` through the edge, grabs, and the frame that
/// comes back through both gateways is **pixel-exact** against the
/// compositor's own scanout — the whole-file snapshot vocabulary
/// carrying a frame the way it carries keymaps.
#[test]
fn capture_over_the_wire_is_pixel_exact() {
    let _gate = GATE.lock().expect("suite gate");
    let remote = remote("capture", token(0x3E));

    // A client through the edge (exactly as it would dial a compositor).
    // The whole session is scoped: dropping the connection is the
    // disconnect, after which both gateways must drain.
    let frame = {
        let mut conn = ldp_client::Connection::connect(remote.edge.addr()).expect("connect");
        let registry = conn.registry().expect("registry");
        let mut collector = Frames::default();
        // Replay globals and find the capture manager.
        let mut saw_capture = false;
        let deadline = std::time::Instant::now() + WAIT;
        while !saw_capture {
            assert!(deadline > std::time::Instant::now(), "no globals replayed");
            conn.roundtrip(&mut collector).expect("roundtrip");
            saw_capture = collector.records.iter().any(|r| {
                r.event == "global" && global_interface(&r.args) == "ldp.capture.capture_manager"
            });
        }
        let _ = registry;
        let capture = conn.bind("ldp.capture.capture_manager").expect("bind");
        conn.send_request(&capture, "grab", vec![]).expect("grab");
        let deadline = std::time::Instant::now() + WAIT;
        while collector.frame.is_none() {
            assert!(deadline > std::time::Instant::now(), "no frame arrived");
            conn.roundtrip(&mut collector).expect("roundtrip");
        }
        collector.frame.clone().expect("frame recorded")
    };

    // The frame event: (fd, width, height) — the snapshot re-materialized
    // by the edge as a fresh memfd, read whole at dispatch.
    let (bytes, width, height) = frame;
    assert_eq!(bytes.len(), width as usize * height as usize * 4, "w*h*4");
    let words: Vec<u32> = bytes
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    assert_eq!(words.len(), 1920 * 1080, "the mock mode is 1080p");

    // Pixel-exact against the compositor's own oracle.
    let scanout = remote.bench.scanout();
    assert_eq!(words, scanout, "the remote grab must equal the scanout");

    // The gateways drain the session cleanly.
    assert!(remote.hub.wait_quiet(WAIT), "hub session did not drain");
    assert!(remote.edge.wait_quiet(WAIT), "edge session did not drain");
}

/// The collecting handler for the capture test: globals replay plus the
/// one frame event (snapshot read at dispatch, read-once).
#[derive(Default)]
struct Frames {
    records: Vec<Record>,
    frame: Option<(Vec<u8>, u32, u32)>,
}

/// One recorded event (name + string args for globals replay).
struct Record {
    event: String,
    args: Vec<ldp_core::wire::Value>,
}

/// The `interface` string argument of a `global` event (the registry
/// replay's shape: interface, version_min, version_max).
fn global_interface(args: &[ldp_core::wire::Value]) -> &str {
    args.iter()
        .find_map(|a| match a {
            ldp_core::wire::Value::String(s) => Some(s.as_ref()),
            _ => None,
        })
        .unwrap_or("")
}

impl EventHandler for Frames {
    fn on_event(&mut self, event: &ldp_client::queue::Event) -> ldp_core::error::Result<()> {
        if event.interface == "ldp.capture.capture_manager" && event.op.name == "frame" {
            // Read the snapshot immediately (read-once): the descriptor
            // closes when the queue drops the event.
            let bytes = event.fds.raw_at(0).map(|raw| {
                use std::os::fd::FromRawFd as _;
                // SAFETY: dup(2) of the descriptor the event carried; the
                // duplicate is adopted by File and closed on drop.
                let dup = unsafe { libc::dup(raw) };
                assert!(dup != -1, "dup the frame descriptor");
                // SAFETY: the dup result is fresh and unowned.
                let fd = unsafe { std::os::fd::OwnedFd::from_raw_fd(dup) };
                use std::io::Read as _;
                let mut v = Vec::new();
                std::fs::File::from(fd)
                    .read_to_end(&mut v)
                    .expect("read frame");
                v
            });
            let dims: Vec<u32> = event
                .args
                .iter()
                .filter_map(|a| match a {
                    ldp_core::wire::Value::Uint32(v) => Some(*v),
                    _ => None,
                })
                .collect();
            if let (Some(bytes), Some((&w, &h))) = (bytes, dims.first().zip(dims.get(1))) {
                self.frame = Some((bytes, w, h));
            }
        } else {
            self.records.push(Record {
                event: event.op.name.to_owned(),
                args: event.args.clone(),
            });
        }
        Ok(())
    }
}

/// Phase 23's uniform version surface: the gateway binary exits 0 and
/// prints exactly `ldp-remote-gateway <workspace version>`.
#[test]
fn version_flag_reports_the_release() {
    let exe = env!("CARGO_BIN_EXE_ldp-remote-gateway");
    let out = std::process::Command::new(exe)
        .arg("--version")
        .output()
        .expect("spawn gateway");
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        text.trim(),
        format!("ldp-remote-gateway {}", env!("CARGO_PKG_VERSION"))
    );
}
