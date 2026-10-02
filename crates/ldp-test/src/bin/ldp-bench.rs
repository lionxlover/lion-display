//! `ldp-bench` — the Phase 19 benchmark runner.
//!
//! Runs the performance and security suites
//! (`ldp_test::benchmarks`) and renders them three ways:
//!
//! * human-readable tables to stdout,
//! * `--json PATH` — the machine-readable report (see
//!   `docs/benchmarks.md` for the schema),
//! * `--markdown PATH` — the report as committed documentation
//!   (the file behind `docs/benchmarks.md`).
//!
//! Doctrine (mirrors `crates/ldp-renderer/tests/perf_4k.rs`): numbers
//! are release-profile numbers; dev-profile runs are labeled as such
//! and only useful as smoke tests. `--quick` shrinks corpora for
//! dev-profile smoke runs.
//!
//! Exit codes: 0 success, 2 usage.

use std::fmt::Write as _;
use std::io::Write as _;

fn main() {
    let mut json_path: Option<String> = None;
    let mut markdown_path: Option<String> = None;
    let mut quick = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--json" => {
                json_path = args.next().or_else(|| usage("--json needs a path"));
            }
            "--markdown" => {
                markdown_path = args.next().or_else(|| usage("--markdown needs a path"));
            }
            "--quick" => quick = true,
            "--help" | "-h" => {
                println!(
                    "ldp-bench — the LDP benchmark harness\n\n\
                     Usage: ldp-bench [--json PATH] [--markdown PATH] [--quick]\n\n\
                     --json PATH      write the machine-readable JSON report\n\
                     --markdown PATH  write the markdown report (docs/benchmarks.md shape)\n\
                     --quick          shrink corpora and run counts (dev-profile smoke)\n\
                     --version        print the release version"
                );
                return;
            }
            "--version" | "-V" => {
                println!("ldp-bench {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            other => {
                usage(&format!("unknown argument '{other}'"));
            }
        }
    }

    let profile = if cfg!(debug_assertions) {
        "dev"
    } else {
        "release"
    };
    eprintln!("ldp-bench: profile={profile} quick={quick}");

    let perf = ldp_test::benchmarks::performance_suite(quick);
    eprintln!(
        "performance suite: {} benchmarks, {} results",
        perf.results.len(),
        perf.results.len()
    );
    let security = ldp_test::benchmarks::security_suite(quick);
    eprintln!(
        "security suite: {} benchmarks, {} results",
        security.results.len(),
        security.results.len()
    );

    // Markdown report (stdout or file).
    let markdown = render_markdown(&perf, &security, profile, quick);
    match markdown_path {
        Some(path) => {
            std::fs::write(&path, markdown).unwrap_or_else(|e| {
                eprintln!("ldp-bench: cannot write {path}: {e}");
                std::process::exit(1);
            });
            eprintln!("ldp-bench: markdown report written to {path}");
        }
        None => print!("{markdown}"),
    }

    // JSON report.
    if let Some(path) = json_path {
        let json = ldp_test::json::Json::obj(vec![
            ("env", ldp_test::bench::env_json()),
            ("quick", ldp_test::json::Json::Bool(quick)),
            (
                "suites",
                ldp_test::json::Json::arr(vec![perf.to_json(), security.to_json()]),
            ),
        ])
        .to_pretty();
        std::fs::write(&path, json).unwrap_or_else(|e| {
            eprintln!("ldp-bench: cannot write {path}: {e}");
            std::process::exit(1);
        });
        eprintln!("ldp-bench: JSON report written to {path}");
    }
}

/// The full markdown report: header, environment, both suites, and
/// the methodology notes that make the numbers interpretable.
fn render_markdown(
    perf: &ldp_test::bench::BenchSuite,
    security: &ldp_test::bench::BenchSuite,
    profile: &str,
    quick: bool,
) -> String {
    let cpus = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    let mut out = String::new();
    out.push_str("# LDP Benchmarks\n\n");
    let _ = writeln!(
        out,
        "Produced by `ldp-bench` (Phase 19, `docs/roadmap.md`). Build profile: \
         **{profile}**, parallelism: {cpus} CPU(s), mode: {}.",
        if quick { "quick (smoke)" } else { "full" }
    );
    out.push('\n');
    out.push_str(&perf.to_markdown());
    out.push('\n');
    out.push_str(&security.to_markdown());
    out.push_str(
        "\n## Methodology\n\n\
         * Every benchmark drives the real crate surface — no mocks, no \
         harness shortcuts: the actual codec, framing writer/reader over a \
         real socketpair, region algebra, the 60 Hz frame scheduler, the \
         color pipeline, the clipboard pipe, the constant-time token walk, \
         the hash-chained audit log, and the permission matrix.\n\
         * Inputs are deterministic (seeded generators), so runs are \
         comparable across machines to the extent hardware allows.\n\
         * Timed runs follow one warmup pass; the table reports median / \
         mean / p95 / min / max over the timed runs.\n\
         * `ns/op` medians are the headline for latency shapes; `MiB/s` \
         for bandwidth shapes. Dev-profile numbers are indicative only — \
         the committed report is a release run.\n",
    );
    let _ = std::io::stdout().flush();
    out
}

fn usage(message: &str) -> ! {
    eprintln!("ldp-bench: {message}");
    eprintln!("usage: ldp-bench [--json PATH] [--markdown PATH] [--quick]");
    std::process::exit(2);
}
