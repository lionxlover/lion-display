//! `ldp-debug` — thin argv adapter over [`ldp_tools::tools::debug`].
//!
//! Exit codes: 0 success · 1 reportable failure (connect, bootstrap,
//! replay decode) · 2 usage.

#![forbid(unsafe_code)]

use ldp_tools::args::Args;
use ldp_tools::tools::debug;

fn main() {
    let mut scanned = Args::from_env();
    if scanned.take_flag("--help") {
        println!("{}", debug::usage());
        return;
    }
    if scanned.take_version_flag() {
        println!("ldp-debug {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    match debug::parse(scanned) {
        Ok(args) => {
            let code = match debug::run(&args, &mut std::io::stdout()) {
                Ok(code) => code,
                Err(e) => {
                    eprintln!("ldp-debug: {e}");
                    2
                }
            };
            std::process::exit(code);
        }
        Err(e) => {
            eprintln!("ldp-debug: {e}\n\n{}", debug::usage());
            std::process::exit(2);
        }
    }
}
