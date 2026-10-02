//! # lion-bridge — the LionOS compatibility door
//!
//! Wayland and X11 applications run on `lion-display` through this
//! bridge process: a Wayland *display* (`--wayland PATH`) whose
//! clients become LDP windows one-for-one, and an X11 *display*
//! (`--x11 PATH`) whose whole screen becomes one rootful LDP window.
//! The pure protocol machinery lives in the `ldp-wayland-bridge` and
//! `ldp-x11-bridge` crates; this process owns what those crates
//! refused to: the sockets, the pool and keymap descriptors, the
//! MIT-SHM SysV mappings, the event clock, and the single-threaded
//! poll engine.
//!
//! The security doctrine carries: every foreign session rides its own
//! LDP client identity, and the process refuses to run without a
//! bridge-scope token (`--token FILE`, hex, 8 words — the Phase 16
//! vocabulary; the compositor's broker-era enforcement is the named
//! roadmap line, the bridge's gate runs today).
//!
//! The engine is one thread, one poll turn over every descriptor
//! (each foreign socket, each LDP link, the listeners): readiness
//! pumps that side; nothing blocks; a quarter-second cap keeps
//! shutdown and the idle ladder honest.
//!
//! ```text
//! lion-bridge --socket NAME (--wayland PATH | --x11 PATH)...
//!            [--token FILE] [--app-id ID] [--keymap PATH]
//!            [--screen WxH] [--hold MS]
//! ```

// The crate root cannot forbid `unsafe` (the `sys` module is the
// audited exception, the ldp-transport precedent); every other
// module carries `#![forbid(unsafe_code)]` itself.

#[path = "sys.rs"]
pub mod sys;

pub mod ldp;
pub mod wayland;
pub mod x11;

use std::io::Write;

use ldp_tools::error::{Result, ToolError};
use ldp_transport::UnixAddr;
use ldp_wayland_bridge::driver::TokenCheck as WlTokenCheck;
use ldp_x11_bridge::driver::TokenCheck as XTokenCheck;
use ldp_x11_bridge::setup::ScreenParams;

use wayland::{Keymap, WlSession};

/// Parsed command line.
#[derive(Clone, Debug, Default)]
pub struct BridgeArgs {
    /// The LDP server's abstract socket (`--socket NAME` /
    /// `LDP_SOCKET`).
    pub socket: Option<String>,
    /// Wayland display socket paths (`--wayland PATH`, repeatable).
    pub wayland: Vec<String>,
    /// X11 display socket paths (`--x11 PATH`, repeatable).
    pub x11: Vec<String>,
    /// The bridge token file (`--token FILE`).
    pub token: Option<String>,
    /// The bridge's app id (`--app-id ID`).
    pub app_id: String,
    /// An explicit keymap (`--keymap PATH`).
    pub keymap: Option<String>,
    /// The X11 screen size (`--screen WxH`, default 1024x768).
    pub screen: (u32, u32),
    /// The run duration in ms (`--hold MS`, the operator's smoke
    /// test; absent = until the process is stopped).
    pub hold_ms: Option<u64>,
    /// The rootful X11 escape (`--x11-rootful`): the whole X screen
    /// as one window. The default is **rootless** — every top-level
    /// X window rides its own LDP surface, override-redirect windows
    /// anchor as popups (the v0.10.3 doctrine).
    pub x11_rootful: bool,
}

impl BridgeArgs {
    /// The defaults-carrying shape (`app_id` and `screen` included).
    #[must_use]
    pub fn with_defaults() -> BridgeArgs {
        BridgeArgs {
            app_id: "lion-bridge".to_owned(),
            screen: (1024, 768),
            x11_rootful: false,
            ..BridgeArgs::default()
        }
    }
}

/// The usage text.
#[must_use]
pub fn usage() -> String {
    "lion-bridge — the Wayland/X11 compatibility door for lion-display\n\
     \n\
     USAGE:\n\
     \x20 lion-bridge --socket NAME (--wayland PATH | --x11 PATH)...\n\
     \x20             [--token FILE] [--app-id ID] [--keymap PATH]\n\
     \x20             [--screen WxH] [--hold MS] [--help | --version]\n\
     \n\
     FACES:\n\
     \x20 --wayland PATH   serve a Wayland display at PATH (a leading @\n\
     \x20                   names an abstract socket; repeatable)\n\
     \x20 --x11 PATH       serve an X11 display at PATH (repeatable)\n\
     \n\
     OPTIONS:\n\
     \x20 --socket NAME    the compositor's abstract socket (or LDP_SOCKET)\n\
     \x20 --token FILE     the bridge-scope token (8 hex words); required\n\
     \x20 --app-id ID      the bridge's identity (default lion-bridge)\n\
     \x20 --keymap PATH    the xkb v1 keymap to serve (else the distro\n\
     \x20                   default, else the honest empty keymap)\n\
     \x20 --screen WxH     the X11 root screen (default 1024x768)\n\
     \x20 --hold MS        run for MS then exit (smoke tests)\n\
     \x20 --x11-rootful    serve the whole X screen as one window (the rootless default\n\
     \x20                  gives every X window its own LDP surface)\n\
     \x20 --help | --version  this text"
        .to_owned()
}

/// The operator-supplied token, verified by the drivers' gates.
#[derive(Clone, Copy)]
struct FileToken {
    words: [u32; 8],
}

impl FileToken {
    /// Read and parse the token file (one 64-hex-char run, or 8
    /// whitespace-separated hex/decimal words).
    ///
    /// # Errors
    ///
    /// [`ToolError`] naming the malformed shape.
    fn load(path: &str) -> Result<FileToken> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| ToolError::Logic(format!("token file {path}: {e}")))?;
        let cleaned: String = text.chars().filter(|c| !c.is_whitespace()).collect();
        let words: Vec<u32> = if cleaned.len() == 64 {
            (0..8)
                .map(|i| u32::from_str_radix(&cleaned[i * 8..(i + 1) * 8], 16))
                .collect::<std::result::Result<Vec<u32>, _>>()
                .map_err(|_| ToolError::Logic("the token file is not 8 hex words".to_owned()))?
        } else {
            text.split_whitespace()
                .map(|w| {
                    u32::from_str_radix(w.trim_start_matches("0x"), 16)
                        .or_else(|_| w.parse::<u32>())
                })
                .collect::<std::result::Result<Vec<u32>, _>>()
                .map_err(|_| ToolError::Logic("the token file is not 8 words".to_owned()))?
        };
        let arr: [u32; 8] = words.try_into().map_err(|_| {
            ToolError::Logic("the token file must carry exactly 8 words".to_owned())
        })?;
        Ok(FileToken { words: arr })
    }

    /// The constant-time comparison the Phase 16 discipline pins.
    fn matches(&self, token: [u32; 8]) -> bool {
        let mut diff = 0u32;
        for (a, b) in self.words.iter().zip(token.iter()) {
            diff |= a ^ b;
        }
        diff == 0
    }
}

impl WlTokenCheck for FileToken {
    fn check(&mut self, _app_id: &str, token: [u32; 8]) -> bool {
        self.matches(token)
    }
}

impl XTokenCheck for FileToken {
    fn check(&mut self, _app_id: &str, token: [u32; 8]) -> bool {
        self.matches(token)
    }
}

/// Resolve the LDP socket (`--socket` or `LDP_SOCKET`).
///
/// # Errors
///
/// [`ToolError`] when neither source names a socket.
pub fn resolve_socket(args: &BridgeArgs) -> Result<UnixAddr> {
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

/// Parse `WxH`.
fn parse_size(s: &str) -> Option<(u32, u32)> {
    let (w, h) = s.split_once('x')?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

/// Parse the command line (argv without the program name).
///
/// # Errors
///
/// [`ToolError`] naming the malformed flag (the special values
/// `__usage__`/`__version__` request the texts).
pub fn parse_args(argv: &[String]) -> Result<BridgeArgs> {
    let mut args = BridgeArgs::with_defaults();
    let mut i = 0;
    while i < argv.len() {
        let flag = argv[i].clone();
        let value = |i: &mut usize| -> Result<String> {
            *i += 1;
            argv.get(*i)
                .cloned()
                .ok_or_else(|| ToolError::Logic(format!("{flag} needs a value")))
        };
        match flag.as_str() {
            "--socket" => args.socket = Some(value(&mut i)?),
            "--wayland" => args.wayland.push(value(&mut i)?),
            "--x11" => args.x11.push(value(&mut i)?),
            "--x11-rootful" => args.x11_rootful = true,
            "--token" => args.token = Some(value(&mut i)?),
            "--app-id" => args.app_id = value(&mut i)?,
            "--keymap" => args.keymap = Some(value(&mut i)?),
            "--hold" => {
                let v = value(&mut i)?;
                args.hold_ms = Some(
                    v.parse()
                        .map_err(|_| ToolError::Logic("--hold needs milliseconds".to_owned()))?,
                );
            }
            "--screen" => {
                let v = value(&mut i)?;
                args.screen = parse_size(&v)
                    .ok_or_else(|| ToolError::Logic("--screen needs WxH".to_owned()))?;
            }
            "--help" => return Err(ToolError::Logic("__usage__".to_owned())),
            "--version" => return Err(ToolError::Logic("__version__".to_owned())),
            other => return Err(ToolError::Logic(format!("unknown flag {other}"))),
        }
        i += 1;
    }
    if args.wayland.is_empty() && args.x11.is_empty() {
        return Err(ToolError::Logic(
            "no face: pass --wayland PATH and/or --x11 PATH".to_owned(),
        ));
    }
    Ok(args)
}

/// The running engine: every face, one poll turn at a time.
pub struct Engine {
    wayland_listeners: Vec<(std::os::unix::net::UnixListener, String)>,
    wayland_sessions: Vec<WlSession>,
    x11: Vec<x11::X11Face>,
    addr: UnixAddr,
    token: [u32; 8],
    app_id: String,
    keymap: Keymap,
    out: Box<dyn Write>,
}

impl Engine {
    /// Bring up every requested face.
    ///
    /// # Errors
    ///
    /// [`ToolError`] when a socket cannot be bound, the token is
    /// malformed, or an LDP connection refuses.
    pub fn start(args: &BridgeArgs, out: Box<dyn Write>) -> Result<Engine> {
        let addr = resolve_socket(args)?;
        let Some(token_path) = &args.token else {
            return Err(ToolError::Logic(
                "no token: the bridge refuses to run without its bridge-scope token (--token FILE)"
                    .to_owned(),
            ));
        };
        let gate = FileToken::load(token_path)?;
        let keymap = Keymap::load(args.keymap.as_deref())
            .map_err(|e| ToolError::Logic(format!("keymap: {e}")))?;
        let screen = ScreenParams {
            width: u16::try_from(args.screen.0.min(8192)).unwrap_or(1024),
            height: u16::try_from(args.screen.1.min(8192)).unwrap_or(768),
            ..ScreenParams::default()
        };
        let mut wayland_listeners = Vec::new();
        for path in &args.wayland {
            let listener = bind_wayland(path)
                .map_err(|e| ToolError::Logic(format!("wayland socket {path}: {e}")))?;
            wayland_listeners.push((listener, path.clone()));
        }
        let mut x11 = Vec::new();
        for path in &args.x11 {
            let mut face_gate = gate;
            let face = x11::X11Face::open(
                path,
                screen,
                &addr,
                gate.words,
                &args.app_id,
                &mut face_gate,
                // The rootless default: every top-level X window
                // rides its own LDP surface. `--x11-rootful` is the
                // operator's escape (the whole-screen window).
                !args.x11_rootful,
            )
            .map_err(|e| ToolError::Logic(format!("x11 face {path}: {e:?}")))?;
            x11.push(face);
        }
        Ok(Engine {
            wayland_listeners,
            wayland_sessions: Vec::new(),
            x11,
            addr,
            token: gate.words,
            app_id: args.app_id.clone(),
            keymap,
            out,
        })
    }

    /// The engine loop: one poll turn at a time until `hold_ms`
    /// elapses (when given).
    ///
    /// # Errors
    ///
    /// [`ToolError`] when an LDP link dies fatally (the whole engine
    /// stops — the operator restarts it; a foreign client's failure
    /// only closes that client).
    pub fn run(&mut self, hold_ms: Option<u64>) -> Result<()> {
        let started = std::time::Instant::now();
        loop {
            if let Some(hold) = hold_ms {
                if u64::try_from(started.elapsed().as_millis()).unwrap_or(0) >= hold {
                    break;
                }
            }
            self.turn()?;
        }
        let _ = writeln!(self.out, "bridge: shutting down");
        Ok(())
    }

    /// One poll turn.
    fn turn(&mut self) -> Result<()> {
        // The interest set: every listener, every session pair, every
        // X11 client row.
        let mut set: Vec<sys::Interest> = Vec::new();
        for (listener, _) in &self.wayland_listeners {
            set.push(sys::Interest {
                fd: std::os::fd::AsRawFd::as_raw_fd(listener),
                read: true,
                write: false,
            });
        }
        for face in &self.x11 {
            set.push(face.listener_interest());
            set.push(face.ldp_interest());
            set.extend(face.client_interests());
        }
        for s in &self.wayland_sessions {
            set.push(s.interest());
            set.push(s.ldp_interest());
        }
        let ready = sys::poll(&set, 250).map_err(|e| ToolError::Logic(format!("poll: {e}")))?;
        let row_of = |fd: i32| {
            ready
                .iter()
                .find(|r| r.fd == fd)
                .copied()
                .unwrap_or(sys::Ready::default())
        };
        // Accept on the Wayland listeners (the accepted streams are
        // collected first — the sessions take the mutable borrow).
        let mut incoming: Vec<(std::os::unix::net::UnixStream, String)> = Vec::new();
        for (listener, path) in &self.wayland_listeners {
            let row = row_of(std::os::fd::AsRawFd::as_raw_fd(listener));
            if row.readable {
                while let Ok((stream, _)) = listener.accept() {
                    incoming.push((stream, path.clone()));
                }
            }
        }
        for (stream, path) in incoming {
            let mut gate = FileToken { words: self.token };
            let app_id = self.app_id.clone();
            match WlSession::accept(
                stream,
                &self.addr,
                self.token,
                &app_id,
                &self.keymap,
                &mut gate,
            ) {
                Ok(session) => {
                    let _ = writeln!(
                        self.out,
                        "bridge: wayland client on {path} (LDP session up)"
                    );
                    self.wayland_sessions.push(session);
                }
                Err(end) => {
                    let _ = writeln!(
                        self.out,
                        "bridge: wayland client on {path} refused: {end:?}"
                    );
                }
            }
        }
        // Pump the Wayland sessions; collect the dead.
        let mut dead: Vec<usize> = Vec::new();
        for (i, s) in self.wayland_sessions.iter_mut().enumerate() {
            let wl_row = row_of(s.interest().fd);
            let ldp_row = row_of(s.ldp_interest().fd);
            match s.pump(&wl_row, &ldp_row) {
                Ok(()) => {}
                Err(end) => {
                    let _ = writeln!(self.out, "bridge: wayland session ended: {end:?}");
                    dead.push(i);
                }
            }
        }
        for i in dead.iter().rev() {
            self.wayland_sessions.remove(*i);
        }
        // Pump the X11 faces.
        for face in &mut self.x11 {
            let listener_row = row_of(face.listener_interest().fd);
            let link_row = row_of(face.ldp_interest().fd);
            face.turn(listener_row.readable, &link_row)
                .map_err(|e| ToolError::Logic(format!("x11 face: {e:?}")))?;
        }
        Ok(())
    }
}

/// Bind a Wayland display socket (a leading `@` = abstract).
fn bind_wayland(path: &str) -> std::io::Result<std::os::unix::net::UnixListener> {
    if let Some(abstract_name) = path.strip_prefix('@') {
        sys::bind_abstract(abstract_name.as_bytes())
    } else {
        if let Some(dir) = std::path::Path::new(path).parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::remove_file(path);
        std::os::unix::net::UnixListener::bind(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn parse_rejects_faceless_runs() {
        assert!(parse_args(&args(&["--socket", "x"])).is_err());
    }

    #[test]
    fn parse_reads_every_flag() {
        let a = parse_args(&args(&[
            "--socket",
            "@lion",
            "--wayland",
            "/run/ldp/wayland-0",
            "--x11",
            "/tmp/.X11-unix/X9",
            "--token",
            "/etc/ldp/bridge.token",
            "--app-id",
            "lion-wl-bridge",
            "--screen",
            "1024x768",
            "--hold",
            "250",
        ]))
        .expect("parses");
        assert_eq!(a.socket.as_deref(), Some("@lion"));
        assert_eq!(a.wayland, ["/run/ldp/wayland-0"]);
        assert_eq!(a.x11, ["/tmp/.X11-unix/X9"]);
        assert_eq!(a.app_id, "lion-wl-bridge");
        assert_eq!(a.screen, (1024, 768));
        assert_eq!(a.hold_ms, Some(250));
    }

    #[test]
    fn the_token_file_parses_both_shapes() {
        let dir = std::env::temp_dir().join(format!("lion-bridge-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let p = dir.join("hex.token");
        std::fs::write(
            &p,
            "0000000100000002000000030000000400000005000000060000000700000008\n",
        )
        .expect("write");
        let t = FileToken::load(p.to_str().unwrap()).expect("parses");
        assert_eq!(t.words[0], 1);
        assert_eq!(t.words[7], 8);
        let p2 = dir.join("words.token");
        std::fs::write(&p2, "1 2 3 4 5 6 7 8").expect("write");
        let t2 = FileToken::load(p2.to_str().unwrap()).expect("parses");
        assert_eq!(t2.words, [1, 2, 3, 4, 5, 6, 7, 8]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_token_gate_compares_every_word() {
        let mut gate = FileToken { words: [9u32; 8] };
        assert!(WlTokenCheck::check(&mut gate, "", [9u32; 8]));
        for i in 0..8 {
            let mut forged = [9u32; 8];
            forged[i] ^= 1;
            assert!(
                !WlTokenCheck::check(&mut gate, "", forged),
                "word {i} participates"
            );
        }
    }

    #[test]
    fn the_wayland_abstract_bind_listens() {
        let name = format!("lion-bridge-it-{}", std::process::id());
        let listener = bind_wayland(&format!("@{name}")).expect("abstract bind");
        let addr = UnixAddr::abstract_name(name.as_bytes()).expect("addr");
        // The transport's own connect speaks the abstract form.
        let _client =
            ldp_transport::stream::TransportStream::connect(&addr).expect("a client connects");
        let (stream, _) = listener.accept().expect("the accept lands");
        drop(stream);
    }
}
