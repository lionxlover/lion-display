//! Scale and steady-state: the phase-4 exit criterion "1k clients × 1k
//! objects steady-state without growth".
//!
//! Coverage strategy (calibrated to keep CI minutes sane — the literal
//! 1M-operation product also runs here, sized to finish in seconds):
//!
//! * `objects_1k_generation_cycle` — one client holds 1000 objects,
//!   destroys all 1000, rebinds 1000 (the generation table at work),
//! * `clients_1k_sequential` — 1000 distinct connection lifecycles with
//!   FD-baseline checks every 200,
//! * `steady_state_without_growth` — waves of concurrent clients, each
//!   holding `OBJECTS` objects: the literal clients × objects product,
//!   with the server's live-session count and the process FD count
//!   asserted flat across waves.
//!
//! The suite is serialized (process-wide FD counts) and uses the null
//! audit sink and null dispatcher: this measures the session core, not
//! observability overhead.

#![cfg(target_os = "linux")]

mod common;

use std::sync::Mutex;
use std::time::{Duration, Instant};

use common::{MockClient, TestServer};
use ldp_core::bitset::Bitset128;

use ldp_server::{GlobalAdvert, ServerConfig};

static SUITE: Mutex<()> = Mutex::new(());

/// Objects per client in the steady-state product test.
const OBJECTS: u32 = 1000;
/// Concurrent clients per wave.
const PER_WAVE: usize = 50;
/// Waves: total distinct clients = WAVES × PER_WAVE = 1000.
const WAVES: usize = 20;

fn scale_config() -> ServerConfig {
    ServerConfig {
        globals: vec![
            GlobalAdvert::new("ldp.core.output"),
            GlobalAdvert::new("ldp.core.compositor"),
        ],
        max_clients: PER_WAVE as u32 + 8,
        ..ServerConfig::default()
    }
}

/// A client with a registry at object 2; drains the replay (the
/// implicit registry + output + compositor = 3 globals).
fn boot(server: &TestServer) -> MockClient {
    let mut c = MockClient::connect(&server.addr);
    c.hello(1, Bitset128::EMPTY);
    c.expect_welcome();
    c.get_registry(1, 2);
    for _ in 0..3 {
        let _ = c.expect_global();
    }
    c
}

#[test]
fn objects_1k_generation_cycle() {
    let _guard = SUITE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let server = TestServer::start("scale-objects", scale_config());
    let mut c = boot(&server);

    // Round 1: 1000 binds at IDs 3..=1002 (2 is the registry itself).
    for id in 3..=1002u32 {
        c.bind(2, "ldp.core.output", 1, id);
        c.expect_bound();
    }
    c.sync(1);

    // Destroy all 1000 (leaving the registry itself).
    for id in 3..=1002u32 {
        c.destroy(id, id);
    }

    // Round 2: rebind the same IDs — the generation table advanced under
    // every slot; the wire IDs resolve to the new objects.
    for id in 3..=1002u32 {
        c.bind(2, "ldp.core.output", 1, id);
        c.expect_bound();
    }
    c.sync(2);
    c.ping(3);
}

#[test]
fn clients_1k_sequential() {
    let _guard = SUITE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let server = TestServer::start("scale-clients", scale_config());
    let fd_baseline = ldp_transport::count_open_fds().unwrap();
    let started = Instant::now();

    for i in 0..1000u32 {
        let mut c = boot(&server);
        c.bind(2, "ldp.core.output", 1, 9);
        c.expect_bound();
        c.destroy(9, i);
        c.sync(i);
        drop(c);
        if (i + 1) % 200 == 0 {
            assert!(
                server.wait_live_zero(Duration::from_secs(5)),
                "sessions must not accumulate (after client {})",
                i + 1
            );
            let fds = ldp_transport::count_open_fds().unwrap();
            assert_eq!(
                fds,
                fd_baseline,
                "fd count must stay flat (after client {})",
                i + 1
            );
        }
    }
    assert!(
        server.wait_live_zero(Duration::from_secs(5)),
        "all sessions reclaimed at the end"
    );
    let fds = ldp_transport::count_open_fds().unwrap();
    assert_eq!(fds, fd_baseline, "fd count flat after 1000 clients");
    eprintln!(
        "1000 sequential client lifecycles in {:.2}s",
        started.elapsed().as_secs_f32()
    );
}

#[test]
fn steady_state_without_growth() {
    let _guard = SUITE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let server = TestServer::start("scale-steady", scale_config());
    let fd_baseline = ldp_transport::count_open_fds().unwrap();
    let started = Instant::now();
    let total_ops = WAVES * PER_WAVE * OBJECTS as usize;

    for wave in 0..WAVES {
        let mut joins = Vec::with_capacity(PER_WAVE);
        let addr = server.addr.clone();
        for client_index in 0..PER_WAVE {
            let addr = addr.clone();
            let cookie = (wave * PER_WAVE + client_index) as u32;
            joins.push(std::thread::spawn(move || {
                let mut c = boot_at(&addr);
                // OBJECTS binds per client: the literal product.
                for k in 0..OBJECTS {
                    let id = 3 + k;
                    c.bind(2, "ldp.core.output", 1, id);
                    c.expect_bound();
                }
                // Hold the objects briefly, then hand them back and go.
                c.sync(cookie);
                for k in 0..OBJECTS {
                    let id = 3 + k;
                    c.destroy(id, cookie);
                }
                c.ping(cookie);
            }));
        }
        for j in joins {
            j.join().expect("client thread must not panic");
        }
        // The wave's sessions all end; nothing may linger.
        assert!(
            server.wait_live_zero(Duration::from_secs(10)),
            "wave {wave} left sessions behind"
        );
        let fds = ldp_transport::count_open_fds().unwrap();
        assert_eq!(
            fds, fd_baseline,
            "fd count must be flat across waves (wave {wave})"
        );
    }

    eprintln!(
        "{} bind/destroy round-trips across {} clients in {:.2}s",
        total_ops,
        WAVES * PER_WAVE,
        started.elapsed().as_secs_f32()
    );
    assert_eq!(WAVES * PER_WAVE, 1000, "the product is 1k clients");
    assert_eq!(OBJECTS, 1000, "…× 1k objects each");
}

fn boot_at(addr: &ldp_transport::UnixAddr) -> MockClient {
    let mut c = MockClient::connect(addr);
    c.hello(1, Bitset128::EMPTY);
    c.expect_welcome();
    c.get_registry(1, 2);
    for _ in 0..3 {
        let _ = c.expect_global();
    }
    c
}
