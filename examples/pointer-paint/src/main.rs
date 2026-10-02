//! `pointer-paint` — argv adapter over [`pointer_paint::run`].
//!
//! Exit codes: 0 the stroke presented · 1 reportable failure · 2 usage.

#![forbid(unsafe_code)]

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.iter().any(|a| a == "--help") {
        println!("{}", pointer_paint::usage());
        return;
    }
    if argv.iter().any(|a| a == "--version" || a == "-V") {
        println!("pointer-paint {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    let mut socket = None;
    let mut script = None;
    let mut rest = Vec::new();
    let mut items = argv.iter();
    while let Some(item) = items.next() {
        match item.as_str() {
            "--socket" => {
                if let Some(name) = items.next() {
                    socket = Some(name.clone());
                } else {
                    eprintln!(
                        "pointer-paint: --socket needs a value\n\n{}",
                        pointer_paint::usage()
                    );
                    std::process::exit(2);
                }
            }
            "--script" => {
                if let Some(path) = items.next() {
                    script = Some(path.clone());
                } else {
                    eprintln!(
                        "pointer-paint: --script needs a value\n\n{}",
                        pointer_paint::usage()
                    );
                    std::process::exit(2);
                }
            }
            other => {
                if let Some(name) = other.strip_prefix("--socket=") {
                    socket = Some(name.to_owned());
                } else if let Some(path) = other.strip_prefix("--script=") {
                    script = Some(path.to_owned());
                } else {
                    rest.push(other.to_owned());
                }
            }
        }
    }
    if let Some(item) = rest.first() {
        eprintln!(
            "pointer-paint: unknown argument '{item}'\n\n{}",
            pointer_paint::usage()
        );
        std::process::exit(2);
    }
    let parsed = pointer_paint::PaintArgs { socket, script };
    let addr = match pointer_paint::resolve_socket(&parsed) {
        Ok(addr) => addr,
        Err(e) => {
            eprintln!("pointer-paint: {e}\n\n{}", pointer_paint::usage());
            std::process::exit(2);
        }
    };
    let trace = match &parsed.script {
        Some(path) => match pointer_paint::read_trace(std::path::Path::new(path)) {
            Ok(trace) => trace,
            Err(e) => {
                eprintln!("pointer-paint: {e}");
                std::process::exit(1);
            }
        },
        None => pointer_paint::demo_trace(),
    };
    if trace.is_empty() {
        eprintln!("pointer-paint: the trace decoded to zero events");
        std::process::exit(1);
    }
    let code = match pointer_paint::run(&addr, &trace, &mut std::io::stdout()) {
        Ok((code, _)) => code,
        Err(e) => {
            eprintln!("pointer-paint: {e}");
            1
        }
    };
    std::process::exit(code);
}
