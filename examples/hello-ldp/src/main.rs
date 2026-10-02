//! `hello-ldp` — argv adapter over [`hello_ldp::run`].
//!
//! Exit codes: 0 the frame presented · 1 reportable failure (connect,
//! bootstrap, frame cycle) · 2 usage.

#![forbid(unsafe_code)]

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.iter().any(|a| a == "--help") {
        println!("{}", hello_ldp::usage());
        return;
    }
    if argv.iter().any(|a| a == "--version" || a == "-V") {
        println!("hello-ldp {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    let mut socket = None;
    let mut rest = Vec::new();
    let mut items = argv.iter();
    while let Some(item) = items.next() {
        if item == "--socket" {
            if let Some(name) = items.next() {
                socket = Some(name.clone());
            } else {
                eprintln!(
                    "hello-ldp: --socket needs a value\n\n{}",
                    hello_ldp::usage()
                );
                std::process::exit(2);
            }
        } else if let Some(name) = item.strip_prefix("--socket=") {
            socket = Some(name.to_owned());
        } else {
            rest.push(item.clone());
        }
    }
    if let Some(item) = rest.first() {
        eprintln!(
            "hello-ldp: unknown argument '{item}'\n\n{}",
            hello_ldp::usage()
        );
        std::process::exit(2);
    }
    let parsed = hello_ldp::HelloArgs { socket };
    let addr = match hello_ldp::resolve_socket(&parsed) {
        Ok(addr) => addr,
        Err(e) => {
            eprintln!("hello-ldp: {e}\n\n{}", hello_ldp::usage());
            std::process::exit(2);
        }
    };
    let code = match hello_ldp::run(&addr, &mut std::io::stdout()) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("hello-ldp: {e}");
            1
        }
    };
    std::process::exit(code);
}
