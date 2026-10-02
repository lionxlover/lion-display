//! The Phase 22 capture surface, end to end: bind `capture_manager`,
//! `grab`, and receive a frame whose snapshot is **pixel-exact** against
//! the compositor's own scanout oracle — plus repeat grabs (fresh
//! descriptors, no state), and the empty-scene baseline.

mod testbench;

use ldp_core::geometry::Rect;
use ldp_core::wire::Value;
use testbench::*;

/// Opaque red / green / blue / white quad as XRGB words (2x2).
fn quad() -> Vec<u8> {
    let mut v = Vec::new();
    for word in [0x00FF_0000u32, 0x0000_FF00, 0x0000_00FF, 0x00FF_FFFF] {
        v.extend_from_slice(&word.to_le_bytes());
    }
    v
}

fn pool_bytes(pixels: &[u8]) -> std::os::fd::OwnedFd {
    use std::io::Write as _;
    let fd = lion_compositor::sys::memfd("capture-pixels").expect("memfd");
    let mut file = std::fs::File::from(fd);
    file.write_all(pixels).expect("write pixels");
    file.into()
}

/// The process's open-descriptor count (the FD-hygiene oracle).
fn fd_count() -> usize {
    let dir = format!("/proc/{}/fd", std::process::id());
    match std::fs::read_dir(&dir) {
        Ok(entries) => entries.count(),
        Err(_) => 0,
    }
}

/// Bind the capture manager and grab once; returns (w, h, words).
fn grab(client: &mut TestClient) -> (u32, u32, Vec<u32>) {
    let capture = client.bind("ldp.capture.capture_manager");
    client
        .conn
        .send_request(&capture, "grab", vec![])
        .expect("send grab");
    client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "frame" && r.snapshot.is_some())
    });
    let frame = client.last("frame");
    // The frame event's schema order: (fd, width, height).
    assert_eq!(frame.args.len(), 3, "frame args: {:?}", frame.args);
    let (Value::Uint32(width), Value::Uint32(height)) = (&frame.args[1], &frame.args[2]) else {
        panic!("frame dims are not uint32: {:?}", frame.args)
    };
    let bytes = frame.snapshot.clone().expect("snapshot bytes");
    assert_eq!(
        bytes.len(),
        *width as usize * *height as usize * 4,
        "snapshot length must be w*h*4"
    );
    let words: Vec<u32> = bytes
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    (*width, *height, words)
}

#[test]
fn grabbed_frame_is_pixel_exact_against_the_scanout() {
    let tb = Testbench::start("grab-exact");
    let mut client = TestClient::connect(&tb.addr);
    // Draw the quad at (0,0).
    let shm = client.bind("ldp.core.shm");
    let pool = create_pool(&mut client, &shm, pool_bytes(&quad()), 16);
    let buffer = create_buffer(&mut client, &pool, 0, 2, 2, 8, 0x3432_5258);
    let compositor = client.bind("ldp.core.compositor");
    let surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("surface");
    attach(&mut client, &surface, &buffer);
    damage(&mut client, &surface, &[Rect::new(0, 0, 2, 2)]);
    commit(&mut client, &surface, 1);
    client.wait_until(|c| c.records.iter().any(|r| r.event == "committed"));

    let (w, h, words) = grab(&mut client);
    // The mock laptop's mode drives the expected geometry.
    let scanout = tb.scanout();
    assert_eq!(w as usize * h as usize, scanout.len());
    assert_eq!(words, scanout, "the snapshot must equal the scanout oracle");
    // And the quad actually landed in it.
    assert_eq!(words[0], 0xFFFF_0000, "opaque red at (0,0)");
    assert_eq!(words[1], 0xFF00_FF00);
}

#[test]
fn repeated_grabs_stay_exact_and_independent() {
    let tb = Testbench::start("grab-repeat");
    let mut client = TestClient::connect(&tb.addr);
    let (w1, h1, first) = grab(&mut client);
    let (w2, h2, second) = grab(&mut client);
    assert_eq!((w1, h1), (w2, h2));
    assert_eq!(first, second, "an idle scene grabs identically");
    assert_eq!(first, tb.scanout());
}

#[test]
fn empty_scene_grabs_the_cleared_background() {
    let tb = Testbench::start("grab-empty");
    let mut client = TestClient::connect(&tb.addr);
    let (_, _, words) = grab(&mut client);
    // The scanout chain starts cleared to opaque black.
    assert!(
        words.iter().all(|&w| w == 0xFF00_0000),
        "cleared scanout expected, got {:?}..",
        &words[..4]
    );
    assert_eq!(words, tb.scanout());
}

#[test]
fn capture_survives_many_sessions_without_leaking_descriptors() {
    // FD hygiene: three full session lifecycles of grab-heavy clients
    // must leave the compositor's FD table at its baseline (the Phase
    // 21 stability doctrine applied to the new surface).
    let baseline = fd_count();
    let tb = Testbench::start("grab-fd");
    for _ in 0..3 {
        let mut client = TestClient::connect(&tb.addr);
        for _ in 0..4 {
            let _ = grab(&mut client);
        }
        // Disconnect (dropped) — the session end reclaims everything.
        drop(client);
        // Let the accept loop reap the session.
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    // Poll for eventual consistency of the FD count.
    let mut now = 0usize;
    for _ in 0..50 {
        now = fd_count();
        if now <= baseline + 8 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(
        now <= baseline + 8,
        "FD count {now} vs baseline {baseline}: leaked descriptors"
    );
}
