//! `showcase` — argv adapter over [`showcase::run`].
//!
//! Exit codes: 0 every frame presented · 1 reportable failure
//! (connect, bootstrap, frame cycle) · 2 usage.

#![forbid(unsafe_code)]

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.iter().any(|a| a == "--help") {
        println!("{}", showcase::usage());
        return;
    }
    if argv.iter().any(|a| a == "--version" || a == "-V") {
        println!("showcase {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    let mut socket = None;
    let mut hold = None;
    let mut rest = Vec::new();
    let mut items = argv.iter();
    while let Some(item) = items.next() {
        if item == "--socket" {
            if let Some(name) = items.next() {
                socket = Some(name.clone());
            } else {
                eprintln!("showcase: --socket needs a value\n\n{}", showcase::usage());
                std::process::exit(2);
            }
        } else if let Some(name) = item.strip_prefix("--socket=") {
            socket = Some(name.to_owned());
        } else if item == "--hold" {
            if let Some(value) = items.next() {
                hold = value.parse::<f64>().ok();
                if hold.is_none() {
                    eprintln!(
                        "showcase: --hold needs a number (got '{value}')\n\n{}",
                        showcase::usage()
                    );
                    std::process::exit(2);
                }
            } else {
                eprintln!("showcase: --hold needs a value\n\n{}", showcase::usage());
                std::process::exit(2);
            }
        } else if let Some(value) = item.strip_prefix("--hold=") {
            hold = value.parse::<f64>().ok();
            if hold.is_none() {
                eprintln!(
                    "showcase: --hold needs a number (got '{value}')\n\n{}",
                    showcase::usage()
                );
                std::process::exit(2);
            }
        } else {
            rest.push(item.clone());
        }
    }
    if let Some(item) = rest.first() {
        eprintln!(
            "showcase: unknown argument '{item}'\n\n{}",
            showcase::usage()
        );
        std::process::exit(2);
    }
    let parsed = showcase::ShowcaseArgs { socket, hold };
    let addr = match showcase::resolve_socket(&parsed) {
        Ok(addr) => addr,
        Err(e) => {
            eprintln!("showcase: {e}\n\n{}", showcase::usage());
            std::process::exit(2);
        }
    };
    let code = match showcase::run_with(&addr, parsed.hold, &mut std::io::stdout()) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("showcase: {e}");
            1
        }
    };
    std::process::exit(code);
}
