//! Presentation-model truths over the wire: vblank-grid timestamps,
//! frame pairing, late-commit drops, superseded registrations, and the
//! buffer release fence lifecycle.

#![allow(clippy::too_many_lines)]

mod testbench;

use ldp_core::geometry::Rect;
use ldp_core::wire::Value;
use testbench::*;

/// One connected, fully-driven client: shm, pool, 2x2 buffer, surface,
/// bound output.
struct Driven {
    client: TestClient,
    #[allow(dead_code)]
    shm: ldp_client::Proxy,
    #[allow(dead_code)]
    pool: ldp_client::Proxy,
    buffer: ldp_client::Proxy,
    surface: ldp_client::Proxy,
    #[allow(dead_code)]
    compositor: ldp_client::Proxy,
    #[allow(dead_code)]
    output: ldp_client::Proxy,
}

fn driven(tag: &str, fill: u8) -> (Testbench, Driven) {
    let tb = Testbench::start(tag);
    let mut client = TestClient::connect(&tb.addr);
    let shm = client.bind("ldp.core.shm");
    let pool = create_pool(&mut client, &shm, memfd_pool(16, fill), 16);
    let buffer = create_buffer(&mut client, &pool, 0, 2, 2, 8, 0x3432_5258);
    let compositor = client.bind("ldp.core.compositor");
    let surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("surface");
    let output = client.bind("ldp.core.output");
    (
        tb,
        Driven {
            client,
            shm,
            pool,
            buffer,
            surface,
            compositor,
            output,
        },
    )
}

/// The ts argument of an event record.
fn ts_of(record: &Recorded) -> u64 {
    match &record.args[1] {
        Value::Ts(t) => *t,
        other => panic!("expected ts, got {other:?}"),
    }
}

/// Draw one frame: register interest, attach, damage, commit.
fn present_frame(d: &mut Driven, frame_id: u64, fill: u8) {
    frame(&mut d.client, &d.surface, frame_id);
    // Fresh pool content through a second pool/buffer pair is the
    // honest way to change pixels (buffers are immutable); for timing
    // tests the same buffer with fresh damage suffices.
    attach(&mut d.client, &d.surface, &d.buffer);
    damage(&mut d.client, &d.surface, &[Rect::new(0, 0, 2, 2)]);
    commit(
        &mut d.client,
        &d.surface,
        u32::try_from(frame_id).unwrap_or(0),
    );
    let _ = fill;
    d.client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "presented" && r.args[0] == Value::Uint64(frame_id))
    });
}

#[test]
fn frame_chain_presents_on_the_vblank_grid() {
    let (_tb, mut d) = driven("grid", 0x40);
    for k in [2u64, 3, 4] {
        present_frame(&mut d, k, 0x40 + k as u8);
        let presented = d
            .client
            .events_of("presented")
            .into_iter()
            .find(|r| r.args[0] == Value::Uint64(k))
            .expect("presented arrives");
        assert_eq!(
            ts_of(presented),
            16_666_666 * k,
            "frame {k} presents exactly on the grid"
        );
    }
}

#[test]
fn late_commit_drops_the_registration() {
    let (_tb, mut d) = driven("late", 0x50);
    // A second surface on the same client drives virtual time forward.
    let surface_b = d
        .client
        .conn
        .create_object(&d.compositor, "create_surface", vec![])
        .expect("surface b");
    // Register interest on A (targets vblank 2), then advance the clock
    // past A's deadline by presenting B's first frame.
    frame(&mut d.client, &d.surface, 1);
    d.client
        .wait_until(|c| c.records.iter().any(|r| r.event == "frame_target"));
    frame(&mut d.client, &surface_b, 100);
    attach(&mut d.client, &surface_b, &d.buffer);
    damage(&mut d.client, &surface_b, &[Rect::new(0, 0, 2, 2)]);
    commit(&mut d.client, &surface_b, 1);
    d.client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "presented" && r.args[0] == Value::Uint64(100))
    });

    // A's content now arrives after its expiry: the registration dies
    // with deadline_missed while the content itself goes live.
    attach(&mut d.client, &d.surface, &d.buffer);
    damage(&mut d.client, &d.surface, &[Rect::new(0, 0, 2, 2)]);
    commit(&mut d.client, &d.surface, 7);
    d.client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "frame_dropped" && r.args[0] == Value::Uint64(1))
    });
    let dropped = d
        .client
        .events_of("frame_dropped")
        .into_iter()
        .find(|r| r.args[0] == Value::Uint64(1))
        .expect("drop event");
    match &dropped.args[1] {
        Value::Enum(reason) => assert_eq!(*reason, 1, "deadline_missed"),
        other => panic!("reason {other:?}"),
    }
}

#[test]
fn re_registration_supersedes_the_old_frame() {
    let (_tb, mut d) = driven("supersede", 0x60);
    frame(&mut d.client, &d.surface, 1);
    d.client
        .wait_until(|c| c.records.iter().any(|r| r.event == "frame_target"));
    frame(&mut d.client, &d.surface, 2);
    d.client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "frame_target" && r.args[0] == Value::Uint64(2))
    });
    d.client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "frame_dropped" && r.args[0] == Value::Uint64(1))
    });
    let dropped = d
        .client
        .events_of("frame_dropped")
        .into_iter()
        .find(|r| r.args[0] == Value::Uint64(1))
        .expect("superseded drop");
    match &dropped.args[1] {
        Value::Enum(reason) => assert_eq!(*reason, 5, "superseded"),
        other => panic!("reason {other:?}"),
    }
}

#[test]
fn superseded_buffer_releases_with_a_signalled_fence() {
    let (_tb, mut d) = driven("release", 0x70);
    // A second buffer over the same pool.
    let buffer2 = create_buffer(&mut d.client, &d.pool, 0, 2, 2, 8, 0x3432_5241);

    present_frame(&mut d, 1, 0x70);
    // No release yet: buffer 1 is still the composited content.
    assert!(
        d.client
            .events_of("release")
            .into_iter()
            .all(|r| r.target != d.buffer.id().as_u32()),
        "no premature release"
    );

    // Swap to buffer 2 and present: buffer 1's fence lands with the
    // superseding flip.
    frame(&mut d.client, &d.surface, 2);
    attach(&mut d.client, &d.surface, &buffer2);
    damage(&mut d.client, &d.surface, &[Rect::new(0, 0, 2, 2)]);
    commit(&mut d.client, &d.surface, 2);
    d.client.wait_until(|c| {
        c.records.iter().any(|r| {
            r.event == "release" && r.target == d.buffer.id().as_u32() && r.fence == Some(true)
        })
    });
    // Exactly one release, on the right object.
    assert_eq!(
        d.client
            .events_of("release")
            .into_iter()
            .filter(|r| r.target == d.buffer.id().as_u32())
            .count(),
        1
    );
    // Buffer 2 is still referenced: no release for it.
    assert!(
        d.client
            .events_of("release")
            .into_iter()
            .all(|r| r.target != buffer2.id().as_u32()),
        "the live buffer is not released"
    );
}
