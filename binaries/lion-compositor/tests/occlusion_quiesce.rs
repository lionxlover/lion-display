//! Phase 45's exit criterion (the occlusion quiescing half): the
//! App-Nap doctrine served end to end through the real protocol — a
//! fully-occluded window's live frame dies with `surface_hidden`, its
//! later `frame` requests park unanswered (no deadline is handed to a
//! client whose pixels cannot reach the panel), and the reveal both
//! answers the parked request and restores the full presentation
//! cycle — over the real socket, with the deterministic render.
//!
//! The macOS story this answers: "App Nap quiets invisible
//! applications (a compositor-level decision — the server knows what
//! is occluded)". Here the knowledge is the damage pass's own visible
//! map, the decision is change-driven at the pass, and the quiescing
//! is the scheduler's contract — every claim in this file is an
//! event the wire actually carried.

mod testbench;

use ldp_client::Proxy;
use ldp_core::geometry::Rect;
use ldp_core::wire::Value;
use testbench::*;

use lion_compositor::server::CompositorConfig;

/// The XRGB8888 fourcc ("XR24") — opaque ink.
const XR24: u32 = 0x3432_5258;
/// The mock output: 1920x1080@60.
const OUT_W: u32 = 1920;
const OUT_H: u32 = 1080;

/// `frame_drop_reason.surface_hidden` (the spec's enum value).
const SURFACE_HIDDEN: u64 = 2;

/// A memfd carrying `w*h` copies of one XRGB word.
fn solid_pool(w: u32, h: u32, word: u32) -> std::os::fd::OwnedFd {
    let pixels: Vec<u8> = std::iter::repeat(word.to_le_bytes())
        .take((w * h) as usize)
        .flatten()
        .collect();
    let fd = lion_compositor::sys::memfd("quiesce-pool").expect("memfd");
    {
        use std::os::unix::fs::FileExt as _;
        let file = std::fs::File::from(fd.try_clone().expect("clone"));
        file.write_at(&pixels, 0).expect("write pool");
    }
    fd
}

/// The frame-drop reason of one recorded `frame_dropped` event.
fn drop_reason(record: &Recorded) -> u64 {
    match &record.args[1] {
        Value::Enum(reason) => u64::from(*reason),
        other => panic!("frame_dropped reason is not an enum: {other:?}"),
    }
}

/// The frame id of one recorded `frame_target` / `frame_dropped`.
fn frame_id(record: &Recorded) -> u64 {
    match &record.args[0] {
        Value::Uint64(frame) => *frame,
        other => panic!("frame id is not a uint64: {other:?}"),
    }
}

/// `surface.attach(null)` — the protocol unmap.
fn unmap(client: &mut TestClient, surface: &Proxy) {
    client
        .conn
        .send_request(surface, "attach", vec![Value::Object(None)])
        .expect("null attach");
}

/// `surface.set_opaque_region(rects)` — the occlusion contract: a
/// client's declared opaque footprint is what the damage algebra
/// subtracts (and what the plane solver can scan out).
fn set_opaque(client: &mut TestClient, surface: &Proxy, rect: Rect) {
    client
        .conn
        .send_request(
            surface,
            "set_opaque_region",
            vec![Value::Array {
                element: ldp_core::wire::ArgType::Rect,
                items: vec![ldp_core::wire::Primitive::Rect(rect)].into(),
            }],
        )
        .expect("set_opaque_region");
}

/// One window brought up and presenting: the Phase 10 choreography
/// (frame, attach, damage, commit) and the verdict waited on — the
/// frame registration is what makes the presentation observable.
/// `opaque = true` additionally declares the window's footprint
/// opaque (the occluder's contract).
fn present_window(
    client: &mut TestClient,
    shm: &Proxy,
    compositor: &Proxy,
    w: u32,
    h: u32,
    word: u32,
    opaque: bool,
) -> Proxy {
    let pool = create_pool(client, shm, solid_pool(w, h, word), (w * h * 4) as i64);
    let buffer = create_buffer(client, &pool, 0, w as i32, h as i32, (w * 4) as i32, XR24);
    let surface = client
        .conn
        .create_object(compositor, "create_surface", vec![])
        .expect("create_surface");
    if opaque {
        set_opaque(client, &surface, Rect::new(0, 0, w, h));
    }
    frame(client, &surface, 1);
    attach(client, &surface, &buffer);
    damage(client, &surface, &[Rect::new(0, 0, w, h)]);
    commit(client, &surface, 1);
    client.wait_until(|c| c.records.iter().any(|r| r.event == "presented"));
    surface
}

/// THE exit criterion: occlusion quiesces, reveal restores.
#[test]
fn the_occluded_window_sleeps_until_revealed() {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tb = Testbench::start_with(
        "quiesce",
        CompositorConfig {
            socket: format!("lion-quiesce-{n}-{}", std::process::id()),
            ..CompositorConfig::default()
        },
    );

    // ---- client A: the window that will be occluded ----------------
    // The first root placed — cascade step zero, the desktop origin.
    let mut a = TestClient::connect(&tb.addr);
    let shm_a = a.bind("ldp.core.shm");
    let compositor_a = a.bind("ldp.core.compositor");
    a.bind("ldp.core.output");
    let word = 0x0030_4050u32; // (R80, G64, B48) — the recognizable ink
    let a_surface = present_window(&mut a, &shm_a, &compositor_a, 200, 100, word, false);

    // Sanity: A is visible and its frame cycle is alive — a frame
    // request draws a deadline.
    frame(&mut a, &a_surface, 2);
    a.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "frame_target" && frame_id(r) == 2)
    });

    // ---- client B: the fullscreen opaque occluder ------------------
    let mut b = TestClient::connect(&tb.addr);
    let shm_b = b.bind("ldp.core.shm");
    let compositor_b = b.bind("ldp.core.compositor");
    b.bind("ldp.core.output");
    let b_surface = present_window(
        &mut b,
        &shm_b,
        &compositor_b,
        OUT_W,
        OUT_H,
        0x0020_2020, // dark grey, opaque
        true,
    );

    // The occlusion verdict, both halves:
    // * A left its only output (`leave_output` — the visibility the
    //   damage pass resolved);
    // * A's live registration for frame 2 died with `surface_hidden`
    //   (the App-Nap termination: the frame it was holding can never
    //   present while occluded).
    a.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "leave_output" && r.target == a_surface.id().as_u32())
    });
    a.wait_until(|c| {
        c.records.iter().any(|r| {
            r.event == "frame_dropped" && frame_id(r) == 2 && drop_reason(r) == SURFACE_HIDDEN
        })
    });

    // The quiesced request parks: A asks for frame 3, B keeps the
    // timeline alive with another presentation — and no deadline for
    // frame 3 ever arrives. (A bounded number of syncs: the mock
    // clock advances only at wake points, so every delivery the
    // protocol owes A would land inside these.)
    frame(&mut a, &a_surface, 3);
    frame(&mut b, &b_surface, 2);
    damage(&mut b, &b_surface, &[Rect::new(0, 0, 64, 64)]);
    commit(&mut b, &b_surface, 2);
    b.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "presented" && r.target == b_surface.id().as_u32())
    });
    for _ in 0..4 {
        a.sync();
    }
    assert!(
        !a.events
            .records
            .iter()
            .any(|r| r.event == "frame_target" && frame_id(r) == 3),
        "the occluded surface must not receive a frame_target (collected: {:#?})",
        a.events.records
    );

    // ---- the reveal: B unmaps, the desktop shows A again -----------
    let frames_before_reveal = tb.frames();
    unmap(&mut b, &b_surface);
    commit(&mut b, &b_surface, 3);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while tb.frames() <= frames_before_reveal {
        assert!(
            deadline > std::time::Instant::now(),
            "the unmap never repainted"
        );
        a.sync();
    }

    // The unhide answered the parked request: frame 3's deadline
    // arrives NOW — the moment A can present again, not a flip later.
    a.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "frame_target" && frame_id(r) == 3)
    });

    // And the full cycle restores: A re-enters its output, requests a
    // fresh frame, and runs the damage/commit cycle. Two rounds: the
    // first commit's flip re-lights A's ink, and — because A's
    // deadline miss during the occluded window (frame 3's answer
    // riding the very flip that expired it — the mock's wake-point
    // delivery, zero real time where a real client has the whole
    // budget) fed the escalation ladder — frame 4's target sits one
    // vblank deep, so the *verdict* fires at the second round's flip:
    // the ladder handing the client a deadline it can actually meet,
    // the verdict at the first flip at-or-after the target, exactly
    // the contract.
    a.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "enter_output" && r.target == a_surface.id().as_u32())
    });
    frame(&mut a, &a_surface, 4);
    damage(&mut a, &a_surface, &[Rect::new(0, 0, 200, 100)]);
    commit(&mut a, &a_surface, 4);
    damage(&mut a, &a_surface, &[Rect::new(0, 0, 200, 100)]);
    commit(&mut a, &a_surface, 5);
    a.wait_until(|c| {
        c.records.iter().any(|r| {
            r.event == "presented" && r.target == a_surface.id().as_u32() && frame_id(r) == 4
        })
    });

    // The pixel truth: the panel shows A's ink again (the reveal
    // repainted the desktop from live state — no client round-trip
    // was needed for the reveal itself).
    let scanout = tb.scanout();
    let px = scanout[0];
    assert_eq!(px & 0x00FF_FFFF, word & 0x00FF_FFFF, "A's ink returned");
}
