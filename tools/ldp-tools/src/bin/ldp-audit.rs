//! `ldp-audit` — thin argv adapter over [`ldp_tools::tools::audit`].
//!
//! Exit codes: 0 success · 1 reportable failure (connect, probe) ·
//! 2 usage.

#![forbid(unsafe_code)]

use ldp_tools::args::Args;
use ldp_tools::tools::audit;

fn main() {
    let mut scanned = Args::from_env();
    if scanned.take_flag("--help") {
        println!("{}", audit::usage());
        return;
    }
    if scanned.take_version_flag() {
        println!("ldp-audit {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    match audit::parse(scanned) {
        Ok(args) => {
            let code = match audit::run(&args, &mut std::io::stdout()) {
                Ok(code) => code,
                Err(e) => {
                    eprintln!("ldp-audit: {e}");
                    2
                }
            };
            std::process::exit(code);
        }
        Err(e) => {
            eprintln!("ldp-audit: {e}\n\n{}", audit::usage());
            std::process::exit(2);
        }
    }
}
