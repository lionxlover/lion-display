//! Dev-profile smoke: both benchmark suites run to completion in
//! quick mode, produce sane shapes, and render both report forms.
//!
//! The real numbers come from release-profile `ldp-bench` runs
//! (committed to `docs/benchmarks.md`); this test only proves the
//! harness itself never wedges and every benchmark does real work
//! (each result has samples and a positive median).

use ldp_test::bench::BenchSuite;
use ldp_test::benchmarks::{performance_suite, security_suite};
use ldp_test::json::Json;

fn assert_sane(suite: &BenchSuite, min_results: usize) {
    assert!(
        suite.results.len() >= min_results,
        "{} produced {} results",
        suite.name,
        suite.results.len()
    );
    for r in &suite.results {
        assert!(!r.ns.is_empty(), "{}: no samples collected", r.name);
        assert!(
            r.median_run_ns() > 0.0,
            "{}: measured zero time — the benchmark is a no-op",
            r.name
        );
        assert!(
            r.median_run_ns().is_finite(),
            "{}: non-finite measurement",
            r.name
        );
    }
}

#[test]
fn performance_suite_smoke() {
    let suite = performance_suite(true);
    assert_sane(&suite, 12);
    let markdown = suite.to_markdown();
    assert!(markdown.contains("### performance"));
    assert!(markdown.contains("codec: encode registry.bind"));
    assert!(markdown.contains("transport: 64 KiB frame round-trip"));
    assert!(markdown.contains("scheduler: 60 Hz decision cadence"));
}

#[test]
fn security_suite_smoke() {
    let suite = security_suite(true);
    assert_sane(&suite, 6);
    let markdown = suite.to_markdown();
    assert!(markdown.contains("### security"));
    assert!(markdown.contains("token submit"));
    assert!(markdown.contains("audit chain verify"));
}

#[test]
fn full_report_json_smoke() {
    let perf = performance_suite(true);
    let security = security_suite(true);
    let json = Json::obj(vec![
        ("env", ldp_test::bench::env_json()),
        (
            "suites",
            Json::arr(vec![perf.to_json(), security.to_json()]),
        ),
    ])
    .to_pretty();
    assert!(json.contains("\"suite\": \"performance\""));
    assert!(json.contains("\"suite\": \"security\""));
    assert!(json.contains("\"median_ns_per_op\""));
}

/// Phase 23's uniform version surface: `ldp-bench --version` exits 0
/// and prints exactly `ldp-bench <workspace version>`.
#[test]
fn version_flag_reports_the_release() {
    let exe = env!("CARGO_BIN_EXE_ldp-bench");
    let out = std::process::Command::new(exe)
        .arg("--version")
        .output()
        .expect("spawn ldp-bench");
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        text.trim(),
        format!("ldp-bench {}", env!("CARGO_PKG_VERSION"))
    );
}
