//! The benchmark harness behind `ldp-bench`.
//!
//! Doctrine — mirrors the Phase 8 perf gate
//! (`crates/ldp-renderer/tests/perf_4k.rs`):
//!
//! * **release-first**: numbers from dev builds are noise; the harness
//!   labels its output with the build profile and the committed report
//!   is produced by a release run,
//! * **median over repeated runs**: warmup first (page faults,
//!   allocator settling, icache), then a fixed run count, then
//!   order statistics — medians resist scheduler spikes,
//! * **self-describing units**: every result carries its unit
//!   (`ns/op`, `ops/s`, `MiB/s`) so a report row can never be
//!   misread,
//! * **machine-readable + human-readable**: the same result set
//!   renders to JSON (for trend tooling) and markdown (for
//!   `docs/benchmarks.md`).
//!
//! The harness measures *wall time of real work* — it never asserts
//! thresholds itself (that is the EC tests' job); it reports.

use std::fmt::Write as _;
use std::time::Instant;

use crate::json::Json;

/// One benchmark's measurements.
#[derive(Clone, Debug)]
pub struct BenchResult {
    /// Fully qualified name (`"codec: encode surface.commit (2 rects)"`).
    pub name: String,
    /// Operations per measured run (throughput = ops × runs / time).
    pub ops_per_run: u64,
    /// Bytes of payload per measured run (0 when not byte-shaped).
    pub bytes_per_run: u64,
    /// Sorted per-run durations in ns.
    pub ns: Vec<u64>,
}

/// A collected suite of results plus environment metadata.
#[derive(Clone, Debug)]
pub struct BenchSuite {
    /// Suite name (`"performance"` / `"security"`).
    pub name: String,
    /// Results in execution order.
    pub results: Vec<BenchResult>,
}

impl BenchResult {
    /// Median ns per operation.
    #[must_use]
    pub fn median_ns_per_op(&self) -> f64 {
        self.median_run_ns() / self.ops_per_run as f64
    }

    /// Mean ns per operation.
    #[must_use]
    pub fn mean_ns_per_op(&self) -> f64 {
        if self.ns.is_empty() {
            return f64::NAN;
        }
        let total: f64 = self.ns.iter().map(|n| *n as f64).sum();
        total / (self.ops_per_run as f64 * self.ns.len() as f64)
    }

    /// 95th percentile ns per operation (nearest-rank over runs).
    #[must_use]
    pub fn p95_ns_per_op(&self) -> f64 {
        if self.ns.is_empty() {
            return f64::NAN;
        }
        let idx = ((self.ns.len() as f64 * 0.95).ceil() as usize).clamp(1, self.ns.len()) - 1;
        self.ns[idx] as f64 / self.ops_per_run as f64
    }

    /// Minimum ns per operation.
    #[must_use]
    pub fn min_ns_per_op(&self) -> f64 {
        self.ns
            .first()
            .map_or(f64::NAN, |n| *n as f64 / self.ops_per_run as f64)
    }

    /// Maximum ns per operation.
    #[must_use]
    pub fn max_ns_per_op(&self) -> f64 {
        self.ns
            .last()
            .map_or(f64::NAN, |n| *n as f64 / self.ops_per_run as f64)
    }

    /// Median run duration in ns.
    #[must_use]
    pub fn median_run_ns(&self) -> f64 {
        match self.ns.len() {
            0 => f64::NAN,
            n => self.ns[n / 2] as f64,
        }
    }

    /// Operations per second (from the median run).
    #[must_use]
    pub fn ops_per_sec(&self) -> f64 {
        let ns = self.median_ns_per_op();
        if ns > 0.0 {
            1e9 / ns
        } else {
            f64::NAN
        }
    }

    /// Bytes per second (from the median run); `None` when not
    /// byte-shaped.
    #[must_use]
    pub fn bytes_per_sec(&self) -> Option<f64> {
        if self.bytes_per_run == 0 {
            return None;
        }
        Some(self.bytes_per_run as f64 * self.ops_per_sec())
    }

    /// The headline unit label for reports.
    #[must_use]
    pub fn unit_label(&self) -> &'static str {
        if self.bytes_per_run > 0 {
            "MiB/s"
        } else {
            "ns/op"
        }
    }

    /// The headline value for reports (in the unit's natural scale).
    #[must_use]
    pub fn headline(&self) -> f64 {
        if self.bytes_per_run > 0 {
            self.bytes_per_sec()
                .map_or(f64::NAN, |b| b / (1024.0 * 1024.0))
        } else {
            self.median_ns_per_op()
        }
    }

    /// JSON form (one object; see `docs/benchmarks.md` for the schema).
    #[must_use]
    pub fn to_json(&self) -> Json {
        let mut fields = vec![
            ("name", Json::str(self.name.clone())),
            ("ops_per_run", Json::int(self.ops_per_run as i64)),
            ("bytes_per_run", Json::int(self.bytes_per_run as i64)),
            ("runs", Json::int(self.ns.len() as i64)),
            ("median_ns_per_op", Json::num(self.median_ns_per_op())),
            ("mean_ns_per_op", Json::num(self.mean_ns_per_op())),
            ("p95_ns_per_op", Json::num(self.p95_ns_per_op())),
            ("min_ns_per_op", Json::num(self.min_ns_per_op())),
            ("max_ns_per_op", Json::num(self.max_ns_per_op())),
        ];
        if let Some(b) = self.bytes_per_sec() {
            fields.push(("bytes_per_sec", Json::num(b)));
        }
        Json::obj(fields)
    }
}

impl BenchSuite {
    /// An empty suite.
    #[must_use]
    pub fn new(name: &str) -> BenchSuite {
        BenchSuite {
            name: name.into(),
            results: Vec::new(),
        }
    }

    /// Append a result.
    pub fn add(&mut self, result: BenchResult) {
        self.results.push(result);
    }

    /// JSON form of the whole suite.
    #[must_use]
    pub fn to_json(&self) -> Json {
        Json::obj(vec![
            ("suite", Json::str(self.name.clone())),
            (
                "results",
                Json::arr(self.results.iter().map(BenchResult::to_json).collect()),
            ),
        ])
    }

    /// Markdown table of the whole suite (headline stats).
    ///
    /// Latency-shaped rows report `ns/op` in every column;
    /// byte-shaped rows report `MiB/s` in every column — a row never
    /// mixes units.
    #[must_use]
    pub fn to_markdown(&self) -> String {
        let mut out = format!("### {}\n\n", self.name);
        out.push_str("| benchmark | unit | median | mean | p95 | min | max | runs |\n");
        out.push_str("|---|---|---|---|---|---|---|---|\n");
        for r in &self.results {
            let fmt = |v: f64| {
                if v >= 1000.0 {
                    format!("{v:.0}")
                } else {
                    format!("{v:.3}")
                }
            };
            let cols: [String; 5] = if r.bytes_per_run > 0 {
                let mibs = |ns_per_op: f64| {
                    if ns_per_op > 0.0 && ns_per_op.is_finite() {
                        r.bytes_per_run as f64 * 1e9 / ns_per_op / (1024.0 * 1024.0)
                    } else {
                        f64::NAN
                    }
                };
                [
                    fmt(mibs(r.median_ns_per_op())),
                    fmt(mibs(r.mean_ns_per_op())),
                    fmt(mibs(r.p95_ns_per_op())),
                    fmt(mibs(r.min_ns_per_op())),
                    fmt(mibs(r.max_ns_per_op())),
                ]
            } else {
                [
                    fmt(r.median_ns_per_op()),
                    fmt(r.mean_ns_per_op()),
                    fmt(r.p95_ns_per_op()),
                    fmt(r.min_ns_per_op()),
                    fmt(r.max_ns_per_op()),
                ]
            };
            let _ = writeln!(
                out,
                "| {} | {} | {} | {} | {} | {} | {} | {} |",
                r.name,
                r.unit_label(),
                cols[0],
                cols[1],
                cols[2],
                cols[3],
                cols[4],
                r.ns.len()
            );
        }
        out
    }
}

/// Environment metadata (build profile, parallelism, timestamp).
#[must_use]
pub fn env_json() -> Json {
    let profile = if cfg!(debug_assertions) {
        "dev"
    } else {
        "release"
    };
    let cpus = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    Json::obj(vec![
        ("profile", Json::str(profile)),
        ("cpus", Json::int(cpus as i64)),
        ("unix_seconds", Json::int(now as i64)),
    ])
}

/// Measure `f` (one run) after `warmup` untimed runs, `runs` timed
/// runs, `ops_per_run` operations per run.
///
/// `f` must be idempotent-ish: it may be called `warmup + runs` times
/// and its state persists between calls (that is the point — steady
/// state is what gets measured).
pub fn bench<F>(name: &str, warmup: usize, runs: usize, ops_per_run: u64, mut f: F) -> BenchResult
where
    F: FnMut(),
{
    let mut ns = Vec::with_capacity(runs);
    for _ in 0..warmup {
        f();
    }
    for _ in 0..runs {
        let t0 = Instant::now();
        f();
        ns.push(t0.elapsed().as_nanos() as u64);
    }
    ns.sort_unstable();
    BenchResult {
        name: name.into(),
        ops_per_run: ops_per_run.max(1),
        bytes_per_run: 0,
        ns,
    }
}

/// [`bench_bytes`] — like [`bench`](fn@bench) but with a byte-shaped
/// payload (reports MiB/s too).
pub fn bench_bytes<F>(
    name: &str,
    warmup: usize,
    runs: usize,
    ops_per_run: u64,
    bytes_per_run: u64,
    mut f: F,
) -> BenchResult
where
    F: FnMut(),
{
    let mut ns = Vec::with_capacity(runs);
    for _ in 0..warmup {
        f();
    }
    for _ in 0..runs {
        let t0 = Instant::now();
        f();
        ns.push(t0.elapsed().as_nanos() as u64);
    }
    ns.sort_unstable();
    BenchResult {
        name: name.into(),
        ops_per_run: ops_per_run.max(1),
        bytes_per_run,
        ns,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_over_a_synthetic_distribution() {
        let r = BenchResult {
            name: "synthetic".into(),
            ops_per_run: 2,
            bytes_per_run: 0,
            ns: vec![10, 20, 30, 40, 50, 60, 70, 80, 90, 100],
        };
        // Median run 50+60 -> 55? No: even length takes ns[5] = 60
        // (upper median, documented by construction).
        assert_eq!(r.median_run_ns(), 60.0);
        assert_eq!(r.median_ns_per_op(), 30.0);
        assert_eq!(r.mean_ns_per_op(), 27.5);
        // Nearest-rank p95 of 10 samples: ceil(9.5)=10th -> index 9.
        assert_eq!(r.p95_ns_per_op(), 50.0);
        assert_eq!(r.min_ns_per_op(), 5.0);
        assert_eq!(r.max_ns_per_op(), 50.0);
        assert_eq!(r.ops_per_sec(), 1e9 / 30.0);
        assert!(r.bytes_per_sec().is_none());
        assert_eq!(r.unit_label(), "ns/op");
    }

    #[test]
    fn byte_shaped_results_report_bandwidth() {
        let r = BenchResult {
            name: "pipe".into(),
            ops_per_run: 1,
            bytes_per_run: 1024 * 1024,
            ns: vec![1_000_000],
        };
        assert_eq!(r.bytes_per_sec(), Some(1024.0 * 1024.0 * 1000.0));
        assert_eq!(r.unit_label(), "MiB/s");
        assert_eq!(r.headline(), 1000.0);
    }

    #[test]
    fn bench_measures_real_work() {
        // A run that touches memory must take measurable time.
        let sink = std::sync::Arc::new(std::sync::Mutex::new(vec![0u64; 1024]));
        let sink2 = std::sync::Arc::clone(&sink);
        let r = bench("memset-ish", 1, 5, 1024, move || {
            let mut v = sink2.lock().unwrap();
            for x in v.iter_mut() {
                *x = x.wrapping_add(1);
            }
        });
        assert_eq!(r.ns.len(), 5);
        assert!(r.median_run_ns() > 0.0);
    }

    #[test]
    fn json_and_markdown_render() {
        let mut suite = BenchSuite::new("test-suite");
        suite.add(BenchResult {
            name: "one".into(),
            ops_per_run: 1,
            bytes_per_run: 0,
            ns: vec![100, 200, 300],
        });
        suite.add(BenchResult {
            name: "two".into(),
            ops_per_run: 4,
            bytes_per_run: 4096,
            ns: vec![1000],
        });
        let j = suite.to_json().to_pretty();
        assert!(j.contains("\"suite\": \"test-suite\""));
        assert!(j.contains("\"median_ns_per_op\""));
        assert!(j.contains("\"bytes_per_sec\""));
        let md = suite.to_markdown();
        assert!(md.contains("### test-suite"));
        assert!(md.contains("| one | ns/op |"));
        assert!(md.contains("| two | MiB/s |"));
        let env = env_json().to_pretty();
        assert!(env.contains("\"profile\""));
    }
}
