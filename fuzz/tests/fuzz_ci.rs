//! THE Phase 19 fuzz gate (`docs/roadmap.md`): every fuzzer runs
//! clean for the CI budget — no panics, no OOM, no leaks.
//!
//! One serialized test (the transport and dispatch targets assert
//! process-wide FD counts, so no other test in this binary may run
//! concurrently). A counting panic hook catches panics on *any*
//! thread — including the server's session threads — and the final
//! assertion demands zero.
//!
//! Budget: `LDP_FUZZ_SCALE` (default 1) multiplies every target's
//! iteration count linearly for soak runs; the defaults keep the
//! whole gate in the tens of seconds under the dev profile.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use ldp_fuzz::report::TargetReport;
use ldp_fuzz::{codec, dispatch, scale, transport, wayland, x11};

static PANICS: AtomicUsize = AtomicUsize::new(0);

/// The whole gate is one test: FD-count assertions are process-wide.
static GATE: Mutex<()> = Mutex::new(());

fn assert_sane(report: &TargetReport) {
    println!("{}", report.summary());
    assert!(
        report.sane(),
        "harness broken: {} (a healthy run exercises both accept and \
         reject paths)",
        report.summary()
    );
}

#[test]
fn fuzzers_run_clean_for_the_ci_budget() {
    let _guard = GATE.lock().expect("fuzz gate");

    // Count panics on every thread for the duration of the gate.
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|info| {
        PANICS.fetch_add(1, Ordering::SeqCst);
        eprintln!(
            "fuzz gate: PANIC on thread {:?}: {info}",
            std::thread::current().id()
        );
    }));

    let scale = scale();
    let results = [
        codec::run(0xC0DE_0001, 15_000 * scale),
        x11::run(0x0C0F_FEE0, 15_000 * scale),
        wayland::run(0x0A1B_2C30, 15_000 * scale),
        transport::run(0x0F5E_ED00, 1_500 * scale),
        dispatch::run(0x0D15_4700, 250 * scale),
    ];

    std::panic::set_hook(previous);

    for report in &results {
        assert_sane(report);
    }
    assert_eq!(
        PANICS.load(Ordering::SeqCst),
        0,
        "fuzzers must run clean: a panic escaped one of the targets"
    );

    // Seed determinism spot check: the in-memory targets must be
    // byte-for-byte reproducible (transport/dispatch involve OS
    // scheduling and real sockets, so they are exempt here and covered
    // by their own invariants instead).
    let a = codec::run(0xABCD_1234, 500);
    let b = codec::run(0xABCD_1234, 500);
    assert_eq!(a, b, "codec fuzzer must be seed-deterministic");
    let a = x11::run(0x9876_5432, 500);
    let b = x11::run(0x9876_5432, 500);
    assert_eq!(a, b, "x11 fuzzer must be seed-deterministic");
    let a = wayland::run(0x0F0F_0F0F, 500);
    let b = wayland::run(0x0F0F_0F0F, 500);
    assert_eq!(a, b, "wayland fuzzer must be seed-deterministic");

    println!(
        "fuzz gate: all five targets clean at scale {scale} \
         ({} panics)",
        PANICS.load(Ordering::SeqCst)
    );
}
