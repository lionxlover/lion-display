//! `ldp-input-debug` — thin argv adapter over
//! [`ldp_tools::tools::input_debug`].
//!
//! Exit codes: 0 analysis completed · 1 analysis failure (read,
//! empty-with-partial) · 2 usage.

#![forbid(unsafe_code)]

use ldp_tools::args::Args;
use ldp_tools::tools::input_debug;

fn main() {
    let mut scanned = Args::from_env();
    if scanned.take_flag("--help") {
        println!("{}", input_debug::usage());
        return;
    }
    if scanned.take_version_flag() {
        println!("ldp-input-debug {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    match input_debug::parse(scanned) {
        Ok(args) => {
            let code = match input_debug::run(&args, &mut std::io::stdout()) {
                Ok(code) => code,
                Err(e) => {
                    eprintln!("ldp-input-debug: {e}");
                    2
                }
            };
            std::process::exit(code);
        }
        Err(e) => {
            eprintln!("ldp-input-debug: {e}\n\n{}", input_debug::usage());
            std::process::exit(2);
        }
    }
}
