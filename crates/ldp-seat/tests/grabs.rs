//! Grab semantics through the router: implicit grabs while buttons
//! hold, explicit popup grabs, and dismissal when surfaces die.

mod common;

use common::{binding, frame_trace, golden_router, mouse_spec, names, pump, surface};
use ldp_input::codes::{btn, ev, rel};
use ldp_seat::event::{PressState, SeatEvent};
use ldp_seat::focus::{ClientBinding, Scene, SurfaceKey, SurfaceRef};
use ldp_seat::grab::GrabKind;

const T0: u64 = 1_000_000;

fn scene_parts() -> (Vec<SurfaceRef>, Vec<ClientBinding>) {
    (
        // B topmost over A's right half.
        vec![
            surface(2, 0x200, 100.0, 0.0, 100.0, 200.0),
            surface(1, 0x100, 0.0, 0.0, 200.0, 200.0),
        ],
        vec![binding(1, 1, 0x50, 0x300), binding(2, 2, 0x50, 0x400)],
    )
}

fn scene<'a>(surfaces: &'a [SurfaceRef], clients: &'a [ClientBinding]) -> Scene<'a> {
    Scene {
        surfaces,
        clients,
        keyboard_focus: Some(SurfaceKey::new(1)),
        bounds: (200.0, 200.0),
    }
}

#[test]
fn implicit_grab_pins_pointer_to_the_press_surface() {
    let (surfaces, clients) = scene_parts();
    let sc = scene(&surfaces, &clients);
    let mut pipeline = common::Pipeline::new(mouse_spec());
    let mut router = golden_router();

    // Into A at (95, 50).
    pump(
        &mut pipeline,
        &mut router,
        &frame_trace(T0, &[(ev::REL, rel::X, 95), (ev::REL, rel::Y, 50)]),
        &sc,
    );
    assert_eq!(router.pointer_focus(), Some(SurfaceKey::new(1)));

    // Press: the implicit grab starts on A.
    pump(
        &mut pipeline,
        &mut router,
        &frame_trace(T0 + 8000, &[(ev::KEY, btn::LEFT as u16, 1)]),
        &sc,
    );
    assert_eq!(
        router.pointer_grab(),
        Some((SurfaceKey::new(1), GrabKind::Implicit))
    );

    // Drag across the boundary into B: while the button holds, every
    // event stays with A — the grab overrides the hit test.
    let out = pump(
        &mut pipeline,
        &mut router,
        &frame_trace(T0 + 16_000, &[(ev::REL, rel::X, 20)]),
        &sc,
    );
    let a_events: Vec<_> = out.iter().filter(|e| e.client == 1).collect();
    assert!(!a_events.is_empty(), "client 1 keeps the drag: {out:?}");
    assert!(
        out.iter().all(|e| e.client == 1),
        "no crossing during grab: {out:?}"
    );
    // Position crossed into B's territory (115, 50).
    assert!((router.pointer_position().x - 115.0).abs() < 1e-5);

    // Release: the grab ends.
    pump(
        &mut pipeline,
        &mut router,
        &frame_trace(T0 + 200_000, &[(ev::KEY, btn::LEFT as u16, 0)]),
        &sc,
    );
    assert_eq!(router.pointer_grab(), None);
    // The next motion re-evaluates focus: now over B.
    let out = pump(
        &mut pipeline,
        &mut router,
        &frame_trace(T0 + 300_000, &[(ev::REL, rel::X, 10)]),
        &sc,
    );
    assert!(
        out.iter().any(|e| e.client == 2),
        "focus follows after release: {out:?}"
    );
}

#[test]
fn multi_button_grab_spans_all_releases() {
    let (surfaces, clients) = scene_parts();
    let sc = scene(&surfaces, &clients);
    let mut pipeline = common::Pipeline::new(mouse_spec());
    let mut router = golden_router();
    pump(
        &mut pipeline,
        &mut router,
        &frame_trace(T0, &[(ev::REL, rel::X, 50), (ev::REL, rel::Y, 50)]),
        &sc,
    );

    // Left + right down: both on A.
    pump(
        &mut pipeline,
        &mut router,
        &frame_trace(
            T0 + 8000,
            &[
                (ev::KEY, btn::LEFT as u16, 1),
                (ev::KEY, btn::RIGHT as u16, 1),
            ],
        ),
        &sc,
    );
    assert!(router.pointer_grab().is_some());

    // Release left only: grab survives.
    pump(
        &mut pipeline,
        &mut router,
        &frame_trace(T0 + 16_000, &[(ev::KEY, btn::LEFT as u16, 0)]),
        &sc,
    );
    assert!(router.pointer_grab().is_some());

    // Release right: grab ends.
    pump(
        &mut pipeline,
        &mut router,
        &frame_trace(T0 + 24_000, &[(ev::KEY, btn::RIGHT as u16, 0)]),
        &sc,
    );
    assert_eq!(router.pointer_grab(), None);
}

#[test]
fn wheel_follows_the_grab() {
    let (surfaces, clients) = scene_parts();
    let sc = scene(&surfaces, &clients);
    let mut pipeline = common::Pipeline::new(mouse_spec());
    let mut router = golden_router();
    // Into A, press, drag into B territory, scroll: the wheel feeds
    // the grabber (A), not the surface under the pointer.
    pump(
        &mut pipeline,
        &mut router,
        &frame_trace(T0, &[(ev::REL, rel::X, 95), (ev::REL, rel::Y, 50)]),
        &sc,
    );
    pump(
        &mut pipeline,
        &mut router,
        &frame_trace(
            T0 + 8000,
            &[(ev::KEY, btn::LEFT as u16, 1), (ev::REL, rel::X, 20)],
        ),
        &sc,
    );
    let out = pump(
        &mut pipeline,
        &mut router,
        &frame_trace(T0 + 16_000, &[(ev::REL, rel::WHEEL, 1)]),
        &sc,
    );
    assert!(out
        .iter()
        .any(|e| matches!(e.event, SeatEvent::PointerAxisDiscrete { discrete: 1, .. })));
    assert!(
        out.iter().all(|e| e.client == 1),
        "wheel feeds the grabber: {out:?}"
    );
}

#[test]
fn surface_gone_releases_the_grab_and_focus() {
    let (surfaces, clients) = scene_parts();
    let sc = scene(&surfaces, &clients);
    let mut pipeline = common::Pipeline::new(mouse_spec());
    let mut router = golden_router();
    pump(
        &mut pipeline,
        &mut router,
        &frame_trace(T0, &[(ev::REL, rel::X, 50), (ev::REL, rel::Y, 50)]),
        &sc,
    );
    pump(
        &mut pipeline,
        &mut router,
        &frame_trace(T0 + 8000, &[(ev::KEY, btn::LEFT as u16, 1)]),
        &sc,
    );
    assert!(router.pointer_grab().is_some());

    // The grabbed surface is destroyed mid-grab: everything it held
    // is dismissed — no zombie grab. The compositor rebuilds its
    // scene without the surface (only B remains).
    router.surface_gone(SurfaceKey::new(1));
    assert_eq!(router.pointer_grab(), None);
    assert_eq!(router.pointer_focus(), None);
    let after = vec![surfaces[0].clone()];
    let sc_after = scene(&after, &clients[1..]);
    // The orphan release has no grab to end and no focus to receive
    // it: the pointer is over B's territory but never entered it.
    let out = pump(
        &mut pipeline,
        &mut router,
        &frame_trace(T0 + 200_000, &[(ev::KEY, btn::LEFT as u16, 0)]),
        &sc_after,
    );
    assert!(out.is_empty(), "orphan release is silent: {out:?}");
}

#[test]
fn press_outside_any_surface_is_silent_but_stateful() {
    let (_, clients) = scene_parts();
    let mut pipeline = common::Pipeline::new(mouse_spec());
    let mut router = golden_router();
    // A covers the output but accepts input only in its top-left
    // 50×50 region.
    let mut a = surface(1, 0x100, 0.0, 0.0, 200.0, 200.0);
    a.region = ldp_seat::focus::InputRegion {
        rects: vec![ldp_seat::focus::RectF::new(0.0, 0.0, 50.0, 50.0)],
    };
    let surfaces = vec![a];
    let sc = scene(&surfaces, &clients);
    // Move into the region, press, drag out of it: the grab holds.
    pump(
        &mut pipeline,
        &mut router,
        &frame_trace(T0, &[(ev::REL, rel::X, 25), (ev::REL, rel::Y, 25)]),
        &sc,
    );
    let enter = pump(
        &mut pipeline,
        &mut router,
        &frame_trace(T0 + 8000, &[(ev::KEY, btn::LEFT as u16, 1)]),
        &sc,
    );
    assert_eq!(names(&enter), vec!["button", "frame"]);
    let out = pump(
        &mut pipeline,
        &mut router,
        &frame_trace(T0 + 16_000, &[(ev::REL, rel::X, 80), (ev::REL, rel::Y, 80)]),
        &sc,
    );
    // Outside the region but grabbed: events keep flowing to A.
    assert!(out.iter().all(|e| e.client == 1));
    assert!(out
        .iter()
        .any(|e| matches!(e.event, SeatEvent::PointerMotion { .. })));
}

#[test]
fn two_presses_two_releases_are_symmetric() {
    let (surfaces, clients) = scene_parts();
    let sc = scene(&surfaces, &clients);
    let mut pipeline = common::Pipeline::new(mouse_spec());
    let mut router = golden_router();
    pump(
        &mut pipeline,
        &mut router,
        &frame_trace(T0, &[(ev::REL, rel::X, 10), (ev::REL, rel::Y, 10)]),
        &sc,
    );
    // Press, release, press, release: each press grabs, each release
    // frees; the second cycle behaves exactly like the first.
    for t in [T0 + 8000, T0 + 100_000] {
        let press = pump(
            &mut pipeline,
            &mut router,
            &frame_trace(t, &[(ev::KEY, btn::LEFT as u16, 1)]),
            &sc,
        );
        assert_eq!(names(&press), vec!["button", "frame"], "press at {t}");
        assert!(router.pointer_grab().is_some());
        let release = pump(
            &mut pipeline,
            &mut router,
            &frame_trace(t + 50_000, &[(ev::KEY, btn::LEFT as u16, 0)]),
            &sc,
        );
        assert_eq!(names(&release), vec!["button", "frame"]);
        assert!(router.pointer_grab().is_none());
    }
}

#[allow(unused_imports)]
use PressState as _PressStateRef;
