//! Lifecycle truths: validation failures are fatal with the right code,
//! pools grow, surface destruction repaints and releases, and multiple
//! clients stay isolated.

#![allow(clippy::too_many_lines)]

mod testbench;

use std::io::Write;

use ldp_client::ClientError;
use ldp_core::geometry::Rect;
use ldp_core::wire::Value;
use testbench::*;

/// Connect, bind shm, create a pool — the prelude of every case here.
fn shm_prelude(client: &mut TestClient, size: i64) -> ldp_client::Proxy {
    let shm = client.bind("ldp.core.shm");
    create_pool(client, &shm, memfd_pool(size as u64, 0x11), size)
}

#[test]
fn invalid_geometry_is_fatal_with_invalid_buffer() {
    let tb = Testbench::start("fatal");
    let mut client = TestClient::connect(&tb.addr);
    let pool = shm_prelude(&mut client, 4096);
    // Stride 4 cannot hold a 2-pixel row (needs 8).
    client
        .conn
        .create_object(
            &pool,
            "create_buffer",
            vec![
                Value::Int32(0),
                Value::Int32(2),
                Value::Int32(2),
                Value::Int32(4),
                Value::Uint32(0x3432_5258),
            ],
        )
        .expect("create_buffer sends");
    // The dispatcher's validation failure becomes the fatal
    // connection.error and the socket closes.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let outcome = client.conn.roundtrip(&mut client.events);
        match outcome {
            Err(ClientError::ServerError { code, .. }) => {
                assert_eq!(
                    code,
                    ldp_core::error::ErrorCode::InvalidBuffer,
                    "stride violations are invalid_buffer"
                );
                break;
            }
            Err(other) => panic!("expected the fatal server error, got {other:?}"),
            Ok(()) => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "the server never rejected the bad geometry"
                );
            }
        }
    }
    drop(client);
    let _ = tb;
}

#[test]
fn pool_resize_enables_larger_buffers() {
    let tb = Testbench::start("resize");
    let mut client = TestClient::connect(&tb.addr);
    let pool = shm_prelude(&mut client, 64);
    // A 16x16 buffer does not fit yet.
    client
        .conn
        .create_object(
            &pool,
            "create_buffer",
            vec![
                Value::Int32(0),
                Value::Int32(16),
                Value::Int32(16),
                Value::Int32(64),
                Value::Uint32(0x3432_5258),
            ],
        )
        .expect("oversized create_buffer sends");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match client.conn.roundtrip(&mut client.events) {
            Err(ClientError::ServerError { code, .. }) => {
                assert_eq!(code, ldp_core::error::ErrorCode::InvalidBuffer);
                break;
            }
            Err(other) => panic!("expected invalid_buffer, got {other:?}"),
            Ok(()) => assert!(std::time::Instant::now() < deadline, "no rejection"),
        }
    }
    drop(client);

    // A second client grows the same-shaped pool and succeeds. The
    // memfd carries a full 1024 bytes of fill; the pool *size* starts
    // at 64 and grows to 4096 (resize re-maps the same descriptor).
    let mut client = TestClient::connect(&tb.addr);
    let shm = client.bind("ldp.core.shm");
    let pool = create_pool(&mut client, &shm, memfd_pool_bytes(&vec![0x11; 1024]), 64);
    client
        .conn
        .send_request(&pool, "resize", vec![Value::Int64(4096)])
        .expect("resize");
    client.sync();
    let buffer = create_buffer(&mut client, &pool, 0, 16, 16, 64, 0x3432_5258);
    let compositor = client.bind("ldp.core.compositor");
    let surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("surface");
    attach(&mut client, &surface, &buffer);
    damage(&mut client, &surface, &[Rect::new(0, 0, 16, 16)]);
    commit(&mut client, &surface, 1);
    client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "committed" && r.args[0] == Value::Uint32(1))
    });
    // The grown buffer composites: row 15 survives the round trip.
    assert_eq!(tb.scanout()[15 * 1920], 0xFF11_1111);
}

#[test]
fn surface_destroy_repaints_the_desktop_and_releases() {
    let tb = Testbench::start("destroy");
    let mut client = TestClient::connect(&tb.addr);
    let shm = client.bind("ldp.core.shm");
    let pool = create_pool(&mut client, &shm, memfd_pool(16, 0x22), 16);
    let buffer = create_buffer(&mut client, &pool, 0, 2, 2, 8, 0x3432_5258);
    let compositor = client.bind("ldp.core.compositor");
    let surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("surface");
    client.bind("ldp.core.output");
    attach(&mut client, &surface, &buffer);
    damage(&mut client, &surface, &[Rect::new(0, 0, 2, 2)]);
    commit(&mut client, &surface, 1);
    client.wait_until(|c| c.records.iter().any(|r| r.event == "committed"));
    assert_eq!(tb.scanout()[0], 0xFF22_2222);

    destroy(&mut client, &surface);
    client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "destroyed" && r.args[0] == Value::Uint32(surface.id().as_u32()))
    });
    // The next wake repaints the removal: the desktop returns, and the
    // buffer's fence lands with the flip that stopped reading it.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if tb.scanout()[0] == 0xFF00_0000
            && client
                .events
                .records
                .iter()
                .any(|r| r.event == "release" && r.fence == Some(true))
        {
            break;
        }
        client.sync();
    }
    assert_eq!(tb.scanout()[0], 0xFF00_0000, "the desktop repainted");
    assert!(
        client
            .events
            .records
            .iter()
            .any(|r| r.event == "release" && r.target == buffer.id().as_u32()),
        "the attached buffer released on destroy"
    );
}

#[test]
fn two_clients_stay_isolated() {
    let tb = Testbench::start("iso");
    let mut a = TestClient::connect(&tb.addr);
    let mut b = TestClient::connect(&tb.addr);

    // Client A: red quad (surface stacked first, below).
    let shm_a = a.bind("ldp.core.shm");
    let red: Vec<u8> = [0x00FF_0000u32; 4]
        .iter()
        .flat_map(|w| w.to_le_bytes())
        .collect();
    let pool_a = create_pool(&mut a, &shm_a, memfd_pool_bytes(&red), 16);
    let buffer_a = create_buffer(&mut a, &pool_a, 0, 2, 2, 8, 0x3432_5258);
    let comp_a = a.bind("ldp.core.compositor");
    let surface_a = a
        .conn
        .create_object(&comp_a, "create_surface", vec![])
        .expect("surface a");
    attach(&mut a, &surface_a, &buffer_a);
    damage(&mut a, &surface_a, &[Rect::new(0, 0, 2, 2)]);
    commit(&mut a, &surface_a, 1);
    a.wait_until(|c| c.records.iter().any(|r| r.event == "committed"));
    assert_eq!(tb.scanout()[0], 0xFFFF_0000);

    // Client B: green quad stacked above A's (created later = front).
    let shm_b = b.bind("ldp.core.shm");
    let green: Vec<u8> = [0x0000_FF00u32; 4]
        .iter()
        .flat_map(|w| w.to_le_bytes())
        .collect();
    let pool_b = create_pool(&mut b, &shm_b, memfd_pool_bytes(&green), 16);
    let buffer_b = create_buffer(&mut b, &pool_b, 0, 2, 2, 8, 0x3432_5258);
    let comp_b = b.bind("ldp.core.compositor");
    let surface_b = b
        .conn
        .create_object(&comp_b, "create_surface", vec![])
        .expect("surface b");
    frame(&mut b, &surface_b, 1);
    attach(&mut b, &surface_b, &buffer_b);
    damage(&mut b, &surface_b, &[Rect::new(0, 0, 2, 2)]);
    commit(&mut b, &surface_b, 1);
    b.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "presented" && r.args[0] == Value::Uint64(1))
    });
    // B's presentation feedback targeted B's surface only; A (which
    // registered no frames) saw no presentation events at all. Object
    // ids are per-client namespaces, so the check is per-event-name.
    assert!(
        b.events
            .records
            .iter()
            .filter(|r| r.event == "presented")
            .all(|r| r.target == surface_b.id().as_u32()),
        "B's presentation feedback targets B's surface"
    );
    assert!(
        a.events.records.iter().all(|r| r.event != "presented"),
        "A never receives B's presentation feedback"
    );
    // B occludes A at the origin.
    assert_eq!(tb.scanout()[0], 0xFF00_FF00);

    // A disconnects: B's content and feedback survive. The teardown's
    // repaint damage consumes the next vblank (A's occlusion went away
    // — the desktop changed), so B waits for the scene to drain, lets
    // the world settle, then registers a fresh frame inside the new
    // window.
    drop(a);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if tb.world(|w| w.scene.routes.len() == 1) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(tb.world(|w| w.scene.routes.len()), 1, "A's surface drained");
    b.sync();
    frame(&mut b, &surface_b, 2);
    attach(&mut b, &surface_b, &buffer_b);
    damage(&mut b, &surface_b, &[Rect::new(0, 0, 2, 2)]);
    commit(&mut b, &surface_b, 2);
    b.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "presented" && r.args[0] == Value::Uint64(2))
    });
    assert_eq!(tb.scanout()[0], 0xFF00_FF00, "B is unaffected");
}

/// A memfd pool carrying explicit bytes.
fn memfd_pool_bytes(pixels: &[u8]) -> std::os::fd::OwnedFd {
    let fd = lion_compositor::sys::memfd("iso-bytes").expect("memfd");
    let mut file = std::fs::File::from(fd);
    file.write_all(pixels).expect("write");
    file.into()
}
