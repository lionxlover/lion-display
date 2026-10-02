//! `ldp-grab` — thin argv adapter over [`ldp_tools::tools::grab`].
//!
//! Exit codes: 0 success · 1 reportable failure (connect, bootstrap,
//! capture, write) · 2 usage.

#![forbid(unsafe_code)]

use ldp_tools::args::Args;
use ldp_tools::tools::grab;

fn main() {
    let mut scanned = Args::from_env();
    if scanned.take_flag("--help") {
        println!("{}", grab::usage());
        return;
    }
    if scanned.take_version_flag() {
        println!("ldp-grab {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    match grab::parse(scanned) {
        Ok(args) => {
            let code = match grab::run(&args, &mut std::io::stdout()) {
                Ok(code) => code,
                Err(e) => {
                    eprintln!("ldp-grab: {e}");
                    2
                }
            };
            std::process::exit(code);
        }
        Err(e) => {
            eprintln!("ldp-grab: {e}\n\n{}", grab::usage());
            std::process::exit(2);
        }
    }
}
