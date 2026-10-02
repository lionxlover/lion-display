//! `ldp-profiler` — thin argv adapter over
//! [`ldp_tools::tools::profiler`].
//!
//! Exit codes: 0 measurement completed · 1 measurement failure
//! (connect, bootstrap, timeout, replay decode) · 2 usage.

#![forbid(unsafe_code)]

use ldp_tools::args::Args;
use ldp_tools::tools::profiler;

fn main() {
    let mut scanned = Args::from_env();
    if scanned.take_flag("--help") {
        println!("{}", profiler::usage());
        return;
    }
    if scanned.take_version_flag() {
        println!("ldp-profiler {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    match profiler::parse(scanned) {
        Ok(args) => {
            let code = match profiler::run(&args, &mut std::io::stdout()) {
                Ok(code) => code,
                Err(e) => {
                    eprintln!("ldp-profiler: {e}");
                    2
                }
            };
            std::process::exit(code);
        }
        Err(e) => {
            eprintln!("ldp-profiler: {e}\n\n{}", profiler::usage());
            std::process::exit(2);
        }
    }
}
