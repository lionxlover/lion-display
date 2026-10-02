//! The `lion-bridge` binary — argument parsing, the run, the exit
//! codes (0 the engine ran its course; 1 a reportable failure).

#![forbid(unsafe_code)]

use lion_bridge::{parse_args, usage, Engine};

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let code = match parse_args(&argv) {
        Ok(args) => run(args),
        Err(e) => handle_special(e),
    };
    std::process::exit(code);
}

fn run(args: lion_bridge::BridgeArgs) -> i32 {
    let hold = args.hold_ms;
    let faces = (args.wayland.len(), args.x11.len());
    match Engine::start(&args, Box::new(std::io::stdout())) {
        Ok(mut engine) => {
            println!(
                "lion-bridge {} — {} wayland face(s), {} x11 face(s)",
                env!("CARGO_PKG_VERSION"),
                faces.0,
                faces.1
            );
            match engine.run(hold) {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("lion-bridge: {e}");
                    1
                }
            }
        }
        Err(e) => {
            eprintln!("lion-bridge: {e}");
            1
        }
    }
}

fn handle_special(e: ldp_tools::error::ToolError) -> i32 {
    let text = e.to_string();
    if text.contains("__usage__") {
        println!("{}", usage());
        0
    } else if text.contains("__version__") {
        println!("lion-bridge {}", env!("CARGO_PKG_VERSION"));
        0
    } else {
        eprintln!("lion-bridge: {e}\n\n{}", usage());
        1
    }
}
