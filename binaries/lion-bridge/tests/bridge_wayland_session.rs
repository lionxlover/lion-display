//! EC: a real Wayland client's pixels travel a real Unix socket,
//! through the bridge process, into the compositor's scanout — the
//! ecosystem door's exit criterion.
//!
//! The testbench brings the compositor up on its abstract socket;
//! the engine (on its own thread) serves a Wayland display on a
//! second abstract socket; the scripted foreign client connects,
//! binds, maps a window with an shm pool, and commits — and the
//! compositor's scanout shows the foreign pixels. The whole stack
//! executes: wl wire, the dispatch state machine, the pool adoption
//! seam, the driver's export, the LDP framing, the server's scene
//! graph, the renderer, the (mock) display.

#![allow(clippy::too_many_lines)]

mod common;

use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;

use ldp_transport::sys::{recv_with_control, send_msg, ControlBuffer};
use ldp_wayland_bridge::protocol::{self, Interface};
use ldp_wayland_bridge::wire::{self, Value};

use lion_bridge::{parse_args, BridgeArgs, Engine};

/// One scripted foreign client over a real socket.
struct ForeignClient {
    stream: UnixStream,
    /// The pool descriptor this client shares (a memfd of pixels).
    pool_fd: std::os::fd::OwnedFd,
}

impl ForeignClient {
    /// Send one request (any fd arguments ride this batch).
    fn send(&mut self, iface: &Interface, object: u32, opcode: u32, args: Vec<Value>) {
        let schema = iface
            .requests
            .iter()
            .find(|m| m.opcode == opcode)
            .expect("request opcode exists");
        let msg = ldp_wayland_bridge::wire::Message {
            object_id: object,
            opcode,
            args,
        };
        let (bytes, _fds) = wire::encode(&msg, schema.args);
        let control = if schema.args.contains(&wire::Arg::Fd) {
            // The test's single pool descriptor rides the send.
            let mut c = ControlBuffer::with_fd_capacity(1);
            let used = c
                .encode_rights(&[self.pool_fd.as_raw_fd()])
                .expect("encode rights");
            Some((c, used))
        } else {
            None
        };
        match control {
            Some((c, used)) => {
                let prefix = c.encoded_prefix(used).to_vec();
                send_msg(self.stream.as_raw_fd(), &bytes, Some(&prefix)).expect("send+fd");
            }
            None => {
                send_msg(self.stream.as_raw_fd(), &bytes, None).expect("send");
            }
        }
    }

    /// Read whatever arrived (the nonblocking drain).
    fn drain(&mut self) -> Vec<u8> {
        let mut all = Vec::new();
        let mut buf = [0u8; 16 * 1024];
        loop {
            let mut control = ControlBuffer::with_fd_capacity(8);
            match recv_with_control(self.stream.as_raw_fd(), &mut buf, &mut control) {
                Ok(g) if g.bytes > 0 => all.extend_from_slice(&buf[..g.bytes]),
                _ => break,
            }
        }
        all
    }

    /// Drain until bytes arrive or the deadline passes (the engine
    /// pumps on its own turn cadence — the foreign side waits).
    fn await_bytes(&mut self, ms: u64) -> Vec<u8> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(ms);
        loop {
            let got = self.drain();
            if !got.is_empty() {
                return got;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the bridge never answered the foreign client"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

/// The full foreign lifecycle through the real stack.
#[test]
fn a_wayland_clients_pixels_reach_the_composits_scanout() {
    let tb = common::Testbench::start("bridge-wl");

    // The bridge token (8 honest words).
    let token_path = std::env::temp_dir().join(format!(
        "lion-bridge-token-{}-{}.txt",
        std::process::id(),
        line!()
    ));
    std::fs::write(&token_path, "1 2 3 4 5 6 7 8").expect("token file");

    let wayland_socket = format!("@lion-bridge-wl-{}-{}", std::process::id(), line!());
    let args = BridgeArgs {
        socket: Some(tb.addr.display_string().trim_start_matches('@').to_owned()),
        wayland: vec![wayland_socket.clone()],
        token: Some(token_path.display().to_string()),
        ..BridgeArgs::with_defaults()
    };

    // The engine thread: accept, pump, present.
    let engine = std::thread::Builder::new()
        .name("bridge-engine".to_owned())
        .spawn(move || {
            let mut engine = Engine::start(&args, Box::new(std::io::stdout())).expect("engine");
            let _ = engine.run(Some(8_000));
        })
        .expect("engine thread");

    // Connect the foreign client once the listener is up.
    let addr =
        ldp_transport::UnixAddr::abstract_name(wayland_socket.trim_start_matches('@').as_bytes())
            .expect("abstract addr");
    let mut deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let stream = loop {
        if let Ok(mut s) = ldp_transport::stream::TransportStream::connect(&addr) {
            let _ = s.set_nonblocking(true);
            use std::os::fd::{FromRawFd, IntoRawFd};
            // SAFETY: `into_fd` hands the live, owned descriptor over;
            // `into_raw_fd` releases it to us; wrapping it moves that
            // ownership into the stream (its drop closes it).
            break unsafe { UnixStream::from_raw_fd(IntoRawFd::into_raw_fd(s.into_fd())) };
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the bridge's wayland socket never accepted"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    };

    // The pool: 2x2 pixels of one distinctive color (0x22334455:
    // bytes B=55 G=44 R=33 A=22 — the X-family alpha is garbage by
    // definition, forced opaque on the composite).
    let pixels: Vec<u8> = [0x22334455u32; 4]
        .iter()
        .flat_map(|w| w.to_le_bytes())
        .collect();
    let pool_fd = {
        let fd = lion_bridge::sys::memfd(16).expect("memfd");
        let mut file = std::fs::File::from(fd);
        file.write_all(&pixels).expect("fill the pool");
        file.into()
    };

    let mut client = ForeignClient { stream, pool_fd };

    // ---- connect: the registry replay ----
    client.send(&protocol::WL_DISPLAY, 1, 1, vec![Value::NewId(2)]);
    let globals = client.await_bytes(5_000);
    assert!(!globals.is_empty(), "the registry replay arrived");

    // ---- the binds ----
    for (name, id) in [(1u32, 3u32), (2, 4), (3, 5), (4, 6)] {
        let iface = protocol::ALL_GLOBALS
            .get(usize::try_from(name).unwrap() - 1)
            .unwrap();
        client.send(
            &protocol::WL_REGISTRY,
            2,
            0,
            vec![
                Value::Uint(name),
                Value::String(iface.name.into()),
                Value::Uint(iface.version),
                Value::NewId(id),
            ],
        );
    }
    let _ = client.drain();

    // ---- map: surface + xdg + toplevel ----
    client.send(&protocol::WL_COMPOSITOR, 3, 0, vec![Value::NewId(9)]);
    client.send(
        &protocol::XDG_WM_BASE,
        6,
        2,
        vec![Value::NewId(10), Value::Object(9)],
    );
    client.send(&protocol::XDG_SURFACE, 10, 1, vec![Value::NewId(11)]);
    let configure = client.await_bytes(5_000);
    assert!(!configure.is_empty(), "the initial configure arrived");

    // ---- the pool (descriptor rides) + buffer + ack + commit ----
    client.send(
        &protocol::WL_SHM,
        4,
        0,
        vec![Value::Fd(0), Value::Int(16), Value::NewId(12)],
    );
    client.send(
        &protocol::WL_SHM_POOL,
        12,
        1,
        vec![
            Value::NewId(13),
            Value::Int(0),
            Value::Int(2),
            Value::Int(2),
            Value::Int(8),
            Value::Uint(1),
        ],
    );
    // The ack of configure serial 1, then the mapping commit.
    client.send(&protocol::XDG_SURFACE, 10, 4, vec![Value::Uint(1)]);
    client.send(
        &protocol::WL_SURFACE,
        9,
        1,
        vec![Value::Object(13), Value::Int(0), Value::Int(0)],
    );
    client.send(
        &protocol::WL_SURFACE,
        9,
        2,
        vec![Value::Int(0), Value::Int(0), Value::Int(2), Value::Int(2)],
    );
    client.send(&protocol::WL_SURFACE, 9, 6, vec![]);

    // ---- the exit criterion: the compositor's scanout ----
    // The engine pumps the foreign commit through the driver's export
    // into the LDP session; the compositor renders the window at the
    // origin (the library's legacy placement) and flips.
    deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let scanout = loop {
        let words = tb.scanout();
        // The XRGB word: B=55 G=44 R=33, alpha forced opaque.
        if words.first().copied() == Some(0xFF33_4455) {
            break words;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the foreign pixels never reached the scanout (first word: {:08x})",
            words.first().copied().unwrap_or(0)
        );
        std::thread::sleep(std::time::Duration::from_millis(25));
    };
    // The whole 2x2 window landed (the origin placement).
    assert_eq!(scanout[1], 0xFF33_4455, "the second foreign pixel");
    assert_eq!(scanout[1920], 0xFF33_4455, "the second row's foreign pixel");

    // The bridge stays honest: the compositor rendered a frame for
    // the foreign window.
    assert!(tb.frames() >= 1, "the foreign commit rendered");

    drop(client);
    let _ = engine.join();
    let _ = std::fs::remove_file(&token_path);
}

/// The parse of the same CLI the operator types.
#[test]
fn the_engine_rejects_a_faceless_invocation() {
    assert!(parse_args(&["--socket".to_owned(), "x".to_owned()]).is_err());
}
