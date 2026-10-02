//! `ldp-validate` — thin argv adapter over
//! [`ldp_tools::tools::validate`].
//!
//! Exit codes: 0 all checks pass · 1 validation failure · 2 usage.

#![forbid(unsafe_code)]

use ldp_tools::args::Args;
use ldp_tools::tools::validate;

fn main() {
    let mut scanned = Args::from_env();
    if scanned.take_flag("--help") {
        println!("{}", validate::usage());
        return;
    }
    if scanned.take_version_flag() {
        println!("ldp-validate {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    match validate::parse(scanned) {
        Ok(args) => {
            let code = match validate::run(&args, &mut std::io::stdout()) {
                Ok(code) => code,
                Err(e) => {
                    eprintln!("ldp-validate: {e}");
                    2
                }
            };
            std::process::exit(code);
        }
        Err(e) => {
            eprintln!("ldp-validate: {e}\n\n{}", validate::usage());
            std::process::exit(2);
        }
    }
}
