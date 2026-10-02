//! `ldp-remote-gateway` — argv adapter over the relay engine.
//!
//! Two modes, one per gateway half:
//!
//! ```text
//! ldp-remote-gateway hub   --listen 0.0.0.0:6400 --compositor ldp-compositor --token-file T
//! ldp-remote-gateway edge  --listen ldp-remote --connect 10.0.0.2:6400 --token-file T
//! ```
//!
//! Exit codes: 0 clean shutdown (SIGINT) · 1 startup failure · 2 usage.

#![forbid(unsafe_code)]

use std::time::Duration;

use ldp_core::error::Result;
use ldp_remote::link::{Keepalive, AUTH_TOKEN_BYTES};
use ldp_remote::wire::RemoteConfig;
use ldp_remote::{EdgeConfig, EdgeGateway, HubConfig, HubGateway};
use ldp_transport::UnixAddr;

fn usage() -> String {
    String::from(
        "ldp-remote-gateway — relay LDP sessions across hosts (Phase 21)\n\n\
         USAGE:\n\
         \x20 ldp-remote-gateway hub  --listen ADDR --compositor NAME [--token-file PATH]\n\
         \x20                              [--max-envelope MiB] [--pool-cap MiB]\n\
         \x20                              [--keepalive SECS | --no-keepalive]\n\
         \x20 ldp-remote-gateway edge --listen NAME --connect ADDR [--token-file PATH]\n\
         \x20                              [--max-envelope MiB] [--pool-cap MiB]\n\
         \x20                              [--keepalive SECS | --no-keepalive]\n\n\
         MODES:\n\
         \x20 hub   Accept authenticated TCP sessions near the compositor; bridge\n\
         \x20       each one to the compositor's AF_UNIX socket.\n\
         \x20 edge  Accept ordinary local AF_UNIX clients; bridge each one to the\n\
         \x20       remote hub over authenticated TCP.\n\n\
         The 32-byte bearer token comes from --token-file (one file, raw bytes;\n\
         64 hex chars also accepted) or LDP_REMOTE_TOKEN (hex). Default: 32 zero\n\
         bytes — fine for a private loopback, never for a real network.\n\n\
         --version | -V prints the release version and exits.\n",
    )
}

struct Args {
    mode: Mode,
    listen: Option<String>,
    connect: Option<String>,
    compositor: Option<String>,
    token_file: Option<String>,
    max_envelope_mib: u32,
    pool_cap_mib: u64,
    keepalive: Option<Keepalive>,
}

enum Mode {
    Hub,
    Edge,
}

fn parse(argv: &[String]) -> Result<Args> {
    let mut args = Args {
        mode: Mode::Hub,
        listen: None,
        connect: None,
        compositor: None,
        token_file: None,
        max_envelope_mib: 64,
        pool_cap_mib: 256,
        keepalive: Some(Keepalive::default()),
    };
    let mut items = argv.iter();
    if let Some(first) = items.next() {
        args.mode = match first.as_str() {
            "hub" => Mode::Hub,
            "edge" => Mode::Edge,
            other => {
                return Err(ldp_core::error::LdpError::Logic {
                    what: "unknown mode",
                })
                .map_err(|e| {
                    let _ = other;
                    e
                });
            }
        };
    }
    while let Some(item) = items.next() {
        match item.as_str() {
            "--listen" => args.listen = items.next().cloned(),
            "--connect" | "--compositor" => {
                if item == "--compositor" {
                    args.compositor = items.next().cloned();
                } else {
                    args.connect = items.next().cloned();
                }
            }
            "--token-file" => args.token_file = items.next().cloned(),
            "--max-envelope" => {
                args.max_envelope_mib = items.next().and_then(|v| v.parse().ok()).unwrap_or(64);
            }
            "--pool-cap" => {
                args.pool_cap_mib = items.next().and_then(|v| v.parse().ok()).unwrap_or(256);
            }
            "--keepalive" => {
                let secs: u64 = items.next().and_then(|v| v.parse().ok()).unwrap_or(10);
                args.keepalive = Some(Keepalive {
                    idle_ping: Duration::from_secs(secs),
                    pong_deadline: Duration::from_secs(secs * 2),
                });
            }
            "--no-keepalive" => {
                args.keepalive = Some(Keepalive {
                    idle_ping: Duration::ZERO,
                    pong_deadline: Duration::from_secs(1),
                });
            }
            "--help" | "-h" => {
                print!("{}", usage());
                std::process::exit(0);
            }
            other => {
                eprintln!(
                    "ldp-remote-gateway: unknown argument '{other}'\n\n{}",
                    usage()
                );
                std::process::exit(2);
            }
        }
    }
    Ok(args)
}

fn load_token(path: Option<&str>) -> Result<[u8; AUTH_TOKEN_BYTES]> {
    let hex_or_raw = |text: &str| -> Option<[u8; AUTH_TOKEN_BYTES]> {
        if text.len() == AUTH_TOKEN_BYTES * 2 {
            let mut out = [0u8; AUTH_TOKEN_BYTES];
            for (i, byte) in out.iter_mut().enumerate() {
                *byte = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).ok()?;
            }
            Some(out)
        } else {
            None
        }
    };
    let token = match path {
        Some(path) => {
            let bytes = std::fs::read(path).map_err(|e| {
                ldp_core::error::LdpError::Io(std::sync::Arc::new(std::io::Error::other(format!(
                    "reading token file '{path}': {e}"
                ))))
            })?;
            if bytes.len() == AUTH_TOKEN_BYTES {
                let mut out = [0u8; AUTH_TOKEN_BYTES];
                out.copy_from_slice(&bytes);
                out
            } else if let Some(token) = String::from_utf8(bytes.clone())
                .ok()
                .and_then(|s| hex_or_raw(s.trim()))
            {
                token
            } else {
                return Err(ldp_core::error::LdpError::Logic {
                    what: "token file must hold 32 raw bytes or 64 hex chars",
                });
            }
        }
        None => match std::env::var("LDP_REMOTE_TOKEN") {
            Ok(hex) => hex_or_raw(hex.trim()).ok_or(ldp_core::error::LdpError::Logic {
                what: "LDP_REMOTE_TOKEN must be 64 hex chars",
            })?,
            Err(_) => [0u8; AUTH_TOKEN_BYTES],
        },
    };
    Ok(token)
}

fn main() -> std::process::ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.iter().any(|a| a == "--help" || a == "-h") {
        print!("{}", usage());
        return std::process::ExitCode::SUCCESS;
    }
    if argv.iter().any(|a| a == "--version" || a == "-V") {
        println!("ldp-remote-gateway {}", env!("CARGO_PKG_VERSION"));
        return std::process::ExitCode::SUCCESS;
    }
    let args = match parse(&argv) {
        Ok(args) => args,
        Err(_) => {
            eprint!("{}", usage());
            return std::process::ExitCode::from(2);
        }
    };
    let token = match load_token(args.token_file.as_deref()) {
        Ok(token) => token,
        Err(e) => {
            eprintln!("ldp-remote-gateway: {e}");
            return std::process::ExitCode::from(1);
        }
    };
    let config = RemoteConfig {
        max_envelope: args.max_envelope_mib * 1024 * 1024,
        pool_total_cap: args.pool_cap_mib * 1024 * 1024,
        ..RemoteConfig::default()
    };
    let keepalive = args.keepalive.unwrap_or(Keepalive {
        idle_ping: Duration::ZERO,
        pong_deadline: Duration::from_secs(1),
    });

    let serve = match args.mode {
        Mode::Hub => {
            let listen = args
                .listen
                .clone()
                .unwrap_or_else(|| String::from("127.0.0.1:6400"));
            let compositor = args
                .compositor
                .clone()
                .unwrap_or_else(|| String::from("ldp-compositor"));
            let hub_config = HubConfig {
                listen,
                compositor: UnixAddr::abstract_name(compositor.as_bytes())
                    .unwrap_or_else(|_| UnixAddr::abstract_name(b"ldp-compositor").expect("name")),
                token,
                config,
                keepalive,
                ..HubConfig::default()
            };
            match HubGateway::start(hub_config) {
                Ok(gateway) => {
                    eprintln!(
                        "ldp-remote-gateway: hub listening on {}",
                        gateway.local_addr()
                    );
                    Ok(())
                }
                Err(e) => Err(e),
            }
        }
        Mode::Edge => {
            let listen = args
                .listen
                .clone()
                .unwrap_or_else(|| String::from("ldp-remote"));
            let connect = args
                .connect
                .clone()
                .unwrap_or_else(|| String::from("127.0.0.1:6400"));
            let edge_config = EdgeConfig {
                listen: UnixAddr::abstract_name(listen.as_bytes())
                    .unwrap_or_else(|_| UnixAddr::abstract_name(b"ldp-remote").expect("name")),
                remote: connect,
                token,
                config,
                keepalive,
                ..EdgeConfig::default()
            };
            match EdgeGateway::start(edge_config) {
                Ok(gateway) => {
                    eprintln!(
                        "ldp-remote-gateway: edge listening on {}",
                        gateway.addr().display_string()
                    );
                    Ok(())
                }
                Err(e) => Err(e),
            }
        }
    };
    match serve {
        Ok(()) => {
            // Serve until killed; the accept loops run on background
            // threads and own their sessions.
            loop {
                std::thread::sleep(Duration::from_secs(3600));
            }
        }
        Err(e) => {
            eprintln!("ldp-remote-gateway: {e}");
            std::process::ExitCode::from(1)
        }
    }
}
