//! # hello-ldp — the smallest complete LDP client
//!
//! One file's worth of protocol choreography, start to finish:
//! connect and handshake, replay the globals, create a surface with a
//! ping-pong shm buffer pair, commit one frame, and wait for the
//! compositor's presentation verdict. When this prints
//! `hello: frame 1 presented`, every layer of the stack — client
//! library, wire codec, transport, server dispatch, scene graph,
//! scheduler, renderer, (headless) scanout — has executed for the
//! first client-authored pixel in the process.
//!
//! The logic lives here as [`run`] so the scripted scenario in
//! `tests/scenario.rs` exercises the same path the binary runs.
//!
//! ```text
//! hello-ldp [--socket NAME]
//! ```
//!
//! Socket discovery follows the toolchain convention: `--socket NAME`
//! (a leading `@` is accepted) or `LDP_SOCKET`; with neither, the
//! error names the convention.

#![forbid(unsafe_code)]

use std::io::Write;

use ldp_core::geometry::Rect;
use ldp_tools::error::{Result, ToolError};
use ldp_tools::session::ToolSession;
use ldp_transport::UnixAddr;

/// Parsed command line.
#[derive(Clone, Debug, Default)]
pub struct HelloArgs {
    /// `--socket NAME` / `LDP_SOCKET`.
    pub socket: Option<String>,
}

/// The usage text.
#[must_use]
pub fn usage() -> String {
    "hello-ldp — connect, commit one frame, read the verdict\n\
     \n\
     USAGE:\n\
     \x20 hello-ldp [--socket NAME]\n\
     \n\
     OPTIONS:\n\
     \x20 --socket NAME   abstract socket name (or LDP_SOCKET)\n\
     \x20 --help | --version  this text"
        .to_owned()
}

/// Run the example against `addr`; returns the process exit code.
///
/// # Errors
/// [`ToolError`] on connection, bootstrap, or frame-cycle failures —
/// the binary prints it and exits 1.
pub fn run(addr: &UnixAddr, out: &mut dyn Write) -> Result<i32, ToolError> {
    let mut session = ToolSession::connect(addr)?;
    session.bootstrap()?;
    let _ = writeln!(
        out,
        "hello: connected to {} — {} global(s) advertised",
        addr.display_string(),
        session.globals().len()
    );
    for global in session.globals() {
        let _ = writeln!(
            out,
            "hello:   {} v{}-v{}",
            global.interface, global.version_min, global.version_max
        );
    }

    // A 64x64 window at (0, 0) with two buffers — the minimal legal
    // frame cycle (re-attach needs a spare while the first is in
    // scanout).
    let mut cycle = session.setup_surface(64, 64, 2, 0x2A)?;
    let _ = writeln!(
        out,
        "hello: surface {} ({}x{}, {} buffer(s))",
        cycle.surface.id().as_u32(),
        64,
        64,
        cycle.buffers.len()
    );

    session.commit_frame(&mut cycle, 1, &[Rect::new(0, 0, 64, 64)])?;
    let _ = writeln!(out, "hello: frame 1 committed and live");

    match session.wait_presented(&cycle.surface, 1, ldp_tools::session::WAIT) {
        Ok(Some(sample)) => {
            let _ = writeln!(
                out,
                "hello: frame 1 presented at {} ns (refresh {} ns)",
                sample.ts_ns, sample.refresh_ns
            );
        }
        Ok(None) => {
            let _ = writeln!(out, "hello: frame 1 was dropped (no presentation verdict)");
        }
        Err(e) => return Err(e),
    }
    let _ = writeln!(out, "hello: goodbye");
    Ok(0)
}

/// Resolve the socket the same way the tools do.
///
/// # Errors
/// A usage-shaped [`ToolError`] when neither source names a socket.
pub fn resolve_socket(args: &HelloArgs) -> Result<UnixAddr, ToolError> {
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
