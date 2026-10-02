//! The Phase 19 crash corpus: clients vanishing at every protocol cut
//! point, the server reclaiming and staying healthy.
//!
//! Each corpus entry drives a client to a specific stage, then drops
//! the connection *abruptly* (objects live, no destroy protocol, the
//! socket just closes). After every crash the suite demands:
//!
//! * the scene drains (routes and outboxes released) within a
//!   bounded wait,
//! * a full healthy canary session completes on the abused server.
//!
//! The cut points cover the whole session lifecycle — plus a raw
//! transport-level shape (a partial frame in flight when the peer
//! vanishes) and a churn run that crashes repeatedly at
//! randomly-chosen points.

use std::time::{Duration, Instant};

use ldp_core::geometry::Rect;
use ldp_test::rng::SplitMix64;
use ldp_transport::{count_open_fds, TransportStream};

use crate::harness::{Client, CompositorHandle};

/// One crash corpus entry: drive to the stage, vanish.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CutPoint {
    /// Gone right after the handshake.
    AfterHandshake,
    /// Gone after binding shm (formats observed).
    AfterShmBind,
    /// Gone with a pool live.
    AfterPool,
    /// Gone with a buffer live.
    AfterBuffer,
    /// Gone with a surface live.
    AfterSurface,
    /// Gone with a buffer attached.
    AfterAttach,
    /// Gone right after a commit (presentation in flight).
    AfterCommit,
    /// Gone with a partial frame in flight (raw transport level).
    MidFrame,
}

impl CutPoint {
    /// Every corpus entry, lifecycle order.
    pub const ALL: [CutPoint; 8] = [
        CutPoint::AfterHandshake,
        CutPoint::AfterShmBind,
        CutPoint::AfterPool,
        CutPoint::AfterBuffer,
        CutPoint::AfterSurface,
        CutPoint::AfterAttach,
        CutPoint::AfterCommit,
        CutPoint::MidFrame,
    ];

    /// Corpus label.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            CutPoint::AfterHandshake => "after-handshake",
            CutPoint::AfterShmBind => "after-shm-bind",
            CutPoint::AfterPool => "after-pool",
            CutPoint::AfterBuffer => "after-buffer",
            CutPoint::AfterSurface => "after-surface",
            CutPoint::AfterAttach => "after-attach",
            CutPoint::AfterCommit => "after-commit",
            CutPoint::MidFrame => "mid-frame",
        }
    }
}

/// Drive one client to the cut point, then drop it abruptly.
fn crash_at(handle: &CompositorHandle, cut: CutPoint) {
    if cut == CutPoint::MidFrame {
        // Raw transport: connect, write a partial frame (less than
        // one envelope), vanish.
        let mut raw = TransportStream::connect(&handle.addr).expect("raw connect");
        let partial = [0x08u8, 0, 0, 0, 0, 0, 0, 0, 1]; // 1-word header, cut short
        raw.send_chunk(&partial, None).expect("partial write");
        drop(raw);
        return;
    }
    let mut client = Client::connect(&handle.addr);
    let shm = client.bind("ldp.core.shm");
    if cut == CutPoint::AfterHandshake {
        drop(client);
        return;
    }
    client.wait_until(|rec| rec.records.iter().filter(|r| r.event == "format").count() >= 2);
    if cut == CutPoint::AfterShmBind {
        drop(client);
        return;
    }
    let pool = client.create_pool(&shm, Client::QUAD_POOL_BYTES, 0x50);
    if cut == CutPoint::AfterPool {
        drop(client);
        return;
    }
    let buffer = client.create_buffer(&pool, 0, 2, 2, 8, 0x3432_5258);
    if cut == CutPoint::AfterBuffer {
        drop(client);
        return;
    }
    let compositor = client.bind("ldp.core.compositor");
    let surface = client.create_surface(&compositor);
    if cut == CutPoint::AfterSurface {
        drop(client);
        return;
    }
    client.frame(&surface, 1);
    client.wait_until(|rec| rec.records.iter().any(|r| r.event == "frame_target"));
    client.attach(&surface, &buffer);
    if cut == CutPoint::AfterAttach {
        drop(client);
        return;
    }
    client.damage(&surface, &[Rect::new(0, 0, 2, 2)]);
    client.commit(&surface, 0xDEAD);
    // AfterCommit: presentation feedback is in flight — vanish now.
    drop(client);
}

/// Demand the scene fully drains (bounded wait).
fn await_drained(handle: &CompositorHandle, label: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if handle.world(|w| w.scene.routes.is_empty() && w.outboxes.is_empty()) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let (routes, boxes) = handle.world(|w| (w.scene.routes.len(), w.outboxes.len()));
    panic!(
        "the scene did not drain after a crash at '{label}': \
         {routes} route(s), {boxes} outbox queue(s)"
    );
}

/// Run the full corpus against one compositor.
///
/// # Panics
///
/// On any reclamation failure: the scene not draining, the canary not
/// completing, or an FD leak across the corpus.
pub fn run_corpus() {
    let handle = CompositorHandle::start("crash");
    let fd_baseline = count_open_fds().expect("fd count");

    // The systematic walk: every cut point, drain + canary each time.
    for cut in CutPoint::ALL {
        println!("corpus: crash at {}", cut.label());
        crash_at(&handle, cut);
        await_drained(&handle, cut.label());
        Client::canary_session(&handle);
        await_drained(&handle, cut.label());
    }

    // The churn run: repeated crashes at seeded-random cut points.
    let mut rng = SplitMix64::new(0xC4A5_5000);
    for i in 0..24 {
        let cut = *rng.pick(&CutPoint::ALL);
        crash_at(&handle, cut);
        await_drained(&handle, &format!("churn-{i}-{}", cut.label()));
        if rng.below(4) == 0 {
            Client::canary_session(&handle);
        }
    }
    // Final proof: healthy session + drained scene + stable FDs.
    Client::canary_session(&handle);
    await_drained(&handle, "final");
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(
        count_open_fds().expect("fd count"),
        fd_baseline,
        "FD table changed across the crash corpus (leak)"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    // The shared cross-suite gate lives in the harness
    // (`harness::SUITE_GATE`): process-wide assertions serialize.

    #[test]
    fn the_crash_corpus() {
        let _guard = crate::harness::SUITE_GATE.lock().expect("crash corpus");
        run_corpus();
    }
}
