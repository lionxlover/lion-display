//! `ldpc` command-line interface.
//!
//! ```text
//! ldpc check    [--spec-dir DIR]
//! ldpc gen      [--spec-dir DIR] [--rust-out DIR] [--docs-out DIR] [--check]
//! ldpc snapshot [--spec-dir DIR] [--out FILE] [--check]
//! ```
//!
//! Exit codes: 0 success · 1 validation or drift failure · 2 usage error.
//! Defaults assume the repository root as working directory.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "Usage:
  ldpc check    [--spec-dir DIR]
  ldpc gen      [--spec-dir DIR] [--rust-out DIR] [--docs-out DIR] [--check]
  ldpc snapshot [--spec-dir DIR] [--out FILE] [--check]

check    Parse and validate the spec set; print a summary.
gen      Generate Rust schema files, the introspection blob, and the
         reference docs. --check compares with the files on disk instead of
         writing (CI drift gate: exit 1 with a report on any difference).
snapshot Freeze the client-facing API surface (every module, interface,
         operation, opcode, since, reply, and argument) into
         spec/surface-v1.json — the stability contract's evidence.
         --check compares with the file on disk instead of writing
         (the freeze gate: exit 1 naming every difference — a frozen
         name, opcode, argument, or version that moved breaks the
         build until the change is consciously re-frozen).

Defaults: --spec-dir spec
          --rust-out crates/ldp-protocol/src/generated
          --docs-out docs/reference
          --out spec/surface-v1.json

--version | -V  print the release version and exit.";

struct GenOpts {
    spec_dir: PathBuf,
    rust_out: PathBuf,
    docs_out: PathBuf,
    check_only: bool,
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(command) = args.next() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let rest: Vec<String> = args.collect();
    match command.as_str() {
        "check" => cmd_check(&rest),
        "gen" => cmd_gen(&rest),
        "snapshot" => cmd_snapshot(&rest),
        "help" | "--help" | "-h" => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        "--version" | "-V" => {
            println!("ldpc {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        other => {
            eprintln!("unknown command '{other}'\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn flag_value(rest: &[String], name: &str) -> Result<Option<String>, String> {
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        if a == name {
            return match it.next() {
                Some(v) => Ok(Some(v.clone())),
                None => Err(format!("{name} needs a value")),
            };
        }
        if let Some(v) = a.strip_prefix(&format!("{name}=")) {
            return Ok(Some(v.to_string()));
        }
    }
    Ok(None)
}

fn spec_dir(rest: &[String]) -> Result<PathBuf, String> {
    Ok(PathBuf::from(
        flag_value(rest, "--spec-dir")?.unwrap_or_else(|| "spec".to_string()),
    ))
}

fn cmd_check(rest: &[String]) -> ExitCode {
    let dir = match spec_dir(rest) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: {e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let set = match ldpc::compile_dir(&dir) {
        Ok(s) => s,
        Err(diags) => {
            report(&diags);
            return ExitCode::FAILURE;
        }
    };
    let c = set.counts();
    println!(
        "spec OK: {} module(s), {} interface(s), {} request(s), {} event(s), {} enum(s), {} bitset(s)",
        c.modules, c.interfaces, c.requests, c.events, c.enums, c.bitsets
    );
    ExitCode::SUCCESS
}

fn cmd_gen(rest: &[String]) -> ExitCode {
    let opts = match parse_gen_opts(rest) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("error: {e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let set = match ldpc::compile_dir(&opts.spec_dir) {
        Ok(s) => s,
        Err(diags) => {
            report(&diags);
            return ExitCode::FAILURE;
        }
    };
    let c = set.counts();
    println!(
        "compiled {} module(s): {} interface(s), {} request(s), {} event(s), {} enum(s), {} bitset(s)",
        c.modules, c.interfaces, c.requests, c.events, c.enums, c.bitsets
    );

    let rust_sources = set.rust_files();
    let blob = set.blob();
    let md_docs = set.markdown_files();

    let mut outputs: BTreeMap<PathBuf, Vec<u8>> = BTreeMap::new();
    for (name, text) in &rust_sources {
        outputs.insert(opts.rust_out.join(name), text.clone().into_bytes());
    }
    outputs.insert(opts.rust_out.join("blob.bin"), blob);
    for (name, text) in &md_docs {
        outputs.insert(opts.docs_out.join(name), text.clone().into_bytes());
    }

    if opts.check_only {
        return drift_report(&outputs);
    }
    write_outputs(&outputs)
}

fn cmd_snapshot(rest: &[String]) -> ExitCode {
    let dir = match spec_dir(rest) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: {e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let out_path = match flag_value(rest, "--out") {
        Ok(Some(v)) => PathBuf::from(v),
        Ok(None) => PathBuf::from("spec/surface-v1.json".to_string()),
        Err(e) => {
            eprintln!("error: {e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let check_only = rest.iter().any(|a| a == "--check");
    for a in rest {
        if a.starts_with("--")
            && !matches!(a.as_str(), "--check")
            && !a.starts_with("--spec-dir")
            && !a.starts_with("--out")
        {
            eprintln!("error: unknown option '{a}'\n{USAGE}");
            return ExitCode::from(2);
        }
    }
    let set = match ldpc::compile_dir(&dir) {
        Ok(s) => s,
        Err(diags) => {
            report(&diags);
            return ExitCode::FAILURE;
        }
    };
    let surface = set.render_surface();
    if check_only {
        return surface_drift_report(&out_path, surface.as_bytes());
    }
    match std::fs::write(&out_path, &surface) {
        Ok(()) => {
            println!("surface frozen: {}", out_path.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: cannot write {}: {e}", out_path.display());
            ExitCode::FAILURE
        }
    }
}

/// The freeze gate's report: byte-compare the frozen surface with the
/// compiled one, naming the first divergence lines.
fn surface_drift_report(path: &std::path::Path, want: &[u8]) -> ExitCode {
    let Some(have) = std::fs::read(path).ok() else {
        println!(
            "SURFACE DRIFT: {} missing (run `cargo run -p ldpc -- snapshot`)",
            path.display()
        );
        return ExitCode::FAILURE;
    };
    if have == want {
        println!("surface frozen: no drift (the v1 contract holds)");
        return ExitCode::SUCCESS;
    }
    println!(
        "SURFACE DRIFT: {} differs from the compiled surface",
        path.display()
    );
    let have_lines: Vec<&str> = std::str::from_utf8(&have).unwrap_or("").lines().collect();
    let want_lines: Vec<&str> = std::str::from_utf8(want).unwrap_or("").lines().collect();
    for (i, (a, b)) in have_lines.iter().zip(want_lines.iter()).enumerate() {
        if a != b {
            println!("  line {}:\n    frozen: {}\n    live:   {}", i + 1, a, b);
            break;
        }
    }
    println!(
        "  frozen {} line(s), live {} line(s); re-freeze consciously: `cargo run -p ldpc -- snapshot`",
        have_lines.len(),
        want_lines.len()
    );
    ExitCode::FAILURE
}

fn parse_gen_opts(rest: &[String]) -> Result<GenOpts, String> {
    let spec_path =
        PathBuf::from(flag_value(rest, "--spec-dir")?.unwrap_or_else(|| "spec".to_string()));
    let rust_out = PathBuf::from(
        flag_value(rest, "--rust-out")?
            .unwrap_or_else(|| "crates/ldp-protocol/src/generated".to_string()),
    );
    let docs_out = PathBuf::from(
        flag_value(rest, "--docs-out")?.unwrap_or_else(|| "docs/reference".to_string()),
    );
    let check_only = rest.iter().any(|a| a == "--check");
    for a in rest {
        if a.starts_with("--")
            && !matches!(a.as_str(), "--check")
            && !a.starts_with("--spec-dir")
            && !a.starts_with("--rust-out")
            && !a.starts_with("--docs-out")
        {
            return Err(format!("unknown option '{a}'"));
        }
    }
    Ok(GenOpts {
        spec_dir: spec_path,
        rust_out,
        docs_out,
        check_only,
    })
}

fn drift_report(outputs: &BTreeMap<PathBuf, Vec<u8>>) -> ExitCode {
    let mut drift = 0;
    for (path, want) in outputs {
        match std::fs::read(path) {
            Ok(have) if &have == want => {}
            Ok(_) => {
                println!("DRIFT: {} differs from ldpc output", path.display());
                drift += 1;
            }
            Err(_) => {
                println!("DRIFT: {} missing", path.display());
                drift += 1;
            }
        }
    }
    if drift == 0 {
        println!("generated outputs match ldpc: no drift");
        ExitCode::SUCCESS
    } else {
        println!("{drift} file(s) drifted; run `cargo run -p ldpc -- gen`");
        ExitCode::FAILURE
    }
}

fn write_outputs(outputs: &BTreeMap<PathBuf, Vec<u8>>) -> ExitCode {
    let mut written = 0;
    for (path, bytes) in outputs {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    eprintln!("error: cannot create {}: {e}", parent.display());
                    return ExitCode::FAILURE;
                }
            }
        }
        match std::fs::read(path) {
            Ok(existing) if &existing == bytes => continue,
            _ => {}
        }
        if let Err(e) = std::fs::write(path, bytes) {
            eprintln!("error: cannot write {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
        written += 1;
    }
    println!("wrote {written} file(s) (unchanged files skipped)");
    ExitCode::SUCCESS
}

fn report(diags: &[ldpc::Diagnostic]) {
    println!("spec validation FAILED ({} problem(s)):", diags.len());
    for d in diags {
        println!("  - {d}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn flag_parsing() {
        let rest = vec!["--spec-dir".to_string(), "x".to_string()];
        assert_eq!(
            flag_value(&rest, "--spec-dir").unwrap().as_deref(),
            Some("x")
        );
        // A lookup miss is not an error: unknown-option detection happens in
        // `parse_gen_opts`, which knows the whole flag vocabulary.
        assert_eq!(flag_value(&rest, "--check").unwrap(), None);
        assert_eq!(flag_value(&rest, "--missing").unwrap(), None);
        let eq = vec!["--spec-dir=y".to_string()];
        assert_eq!(flag_value(&eq, "--spec-dir").unwrap().as_deref(), Some("y"));
        let dangling = vec!["--spec-dir".to_string()];
        assert!(flag_value(&dangling, "--spec-dir").is_err());
    }

    #[test]
    fn gen_opts_defaults() {
        let o = parse_gen_opts(&[]).unwrap();
        assert_eq!(o.spec_dir, Path::new("spec"));
        assert_eq!(o.rust_out, Path::new("crates/ldp-protocol/src/generated"));
        assert_eq!(o.docs_out, Path::new("docs/reference"));
        assert!(!o.check_only);
        assert!(parse_gen_opts(&["--wat".to_string()]).is_err());
        assert!(parse_gen_opts(&["--check".to_string()]).unwrap().check_only);
    }
}
