//! `clip-client` — argv adapter over [`clip_client::run`].
//!
//! Exit codes: 0 the report/negotiation completed · 1 reportable
//! failure · 2 usage.

#![forbid(unsafe_code)]

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.iter().any(|a| a == "--help") {
        println!("{}", clip_client::usage());
        return;
    }
    if argv.iter().any(|a| a == "--version" || a == "-V") {
        println!("clip-client {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    let mut socket = None;
    let mut rest = Vec::new();
    let mut live = false;
    let mut items = argv.iter();
    while let Some(item) = items.next() {
        match item.as_str() {
            "--live" => live = true,
            "--socket" => {
                if let Some(name) = items.next() {
                    socket = Some(name.clone());
                } else {
                    eprintln!(
                        "clip-client: --socket needs a value\n\n{}",
                        clip_client::usage()
                    );
                    std::process::exit(2);
                }
            }
            other => {
                if let Some(name) = other.strip_prefix("--socket=") {
                    socket = Some(name.to_owned());
                } else {
                    rest.push(other.to_owned());
                }
            }
        }
    }
    if let Some(item) = rest.first() {
        eprintln!(
            "clip-client: unknown argument '{item}'\n\n{}",
            clip_client::usage()
        );
        std::process::exit(2);
    }
    let parsed = clip_client::ClipArgs { socket, live };
    let code = match clip_client::run(&parsed, &mut std::io::stdout()) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("clip-client: {e}");
            1
        }
    };
    std::process::exit(code);
}
