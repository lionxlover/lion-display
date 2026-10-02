//! `ldp-info` — thin argv adapter over [`ldp_tools::tools::info`].
//!
//! Exit codes: 0 success · 1 reportable failure (connect, bootstrap,
//! probe) · 2 usage.

#![forbid(unsafe_code)]

use ldp_tools::args::Args;
use ldp_tools::tools::info;

fn main() {
    let mut scanned = Args::from_env();
    if scanned.take_flag("--help") {
        println!("{}", info::usage());
        return;
    }
    if scanned.take_version_flag() {
        println!("ldp-info {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    match info::parse(scanned) {
        Ok(args) => {
            let code = match info::run(&args, &mut std::io::stdout()) {
                Ok(code) => code,
                Err(e) => {
                    eprintln!("ldp-info: {e}");
                    2
                }
            };
            std::process::exit(code);
        }
        Err(e) => {
            eprintln!("ldp-info: {e}\n\n{}", info::usage());
            std::process::exit(2);
        }
    }
}
