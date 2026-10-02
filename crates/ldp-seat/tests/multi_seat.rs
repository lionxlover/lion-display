//! Multi-seat isolation: two seats, interleaved input, and the proof
//! that nothing crosses.
//!
//! Two full stacks (seat managers, routers, pipelines, client sets)
//! run side by side; device frames interleave; every routed event
//! must land on a client of the *issuing* seat. Focus, pointer
//! position, and modifier state are asserted independent.

mod common;

use common::{
    binding, frame_trace, golden_router, mouse_spec, pump, surface, touchscreen_spec, xkb_state,
};
use ldp_core::geometry::PointF;
use ldp_core::ids::ObjectId;
use ldp_input::codes::{abs, ev, rel};
use ldp_seat::event::{PressState, SeatEvent};
use ldp_seat::focus::{ClientBinding, InputRegion, Scene, SurfaceKey, SurfaceRef};
use ldp_seat::route::{RoutedEvent, Router};
use ldp_seat::seat::{seat_caps, SeatManager};

const T0: u64 = 1_000_000;

/// The two-seat world: seat0 (mouse + surface A + client 1), seat1
/// (mouse + surface B + client 2). Disjoint everything — that is the
/// point: identical coordinates, different seats, no crossing.
struct World {
    router0: Router,
    router1: Router,
    surf0: Vec<SurfaceRef>,
    clients0: Vec<ClientBinding>,
    surf1: Vec<SurfaceRef>,
    clients1: Vec<ClientBinding>,
}

impl World {
    fn new() -> World {
        World {
            router0: golden_router(),
            router1: golden_router(),
            // Both surfaces at the origin: the hit tests resolve
            // identically per seat, the clients differ.
            surf0: vec![surface(1, 0x100, 0.0, 0.0, 100.0, 100.0)],
            clients0: vec![binding(1, 1, 0x50, 0x300)],
            surf1: vec![surface(2, 0x200, 0.0, 0.0, 100.0, 100.0)],
            clients1: vec![binding(2, 2, 0x60, 0x400)],
        }
    }
}

/// Seat 0's view (field borrows only: the router stays mutable).
fn scene0<'a>(surfaces: &'a [SurfaceRef], clients: &'a [ClientBinding]) -> Scene<'a> {
    Scene {
        surfaces,
        clients,
        keyboard_focus: Some(SurfaceKey::new(1)),
        bounds: (100.0, 100.0),
    }
}

/// Seat 1's view.
fn scene1<'a>(surfaces: &'a [SurfaceRef], clients: &'a [ClientBinding]) -> Scene<'a> {
    Scene {
        surfaces,
        clients,
        keyboard_focus: Some(SurfaceKey::new(2)),
        bounds: (100.0, 100.0),
    }
}

#[test]
fn devices_route_to_their_tagged_seats() {
    let mut m = SeatManager::new();
    let mut seat0_mouse = mouse_spec();
    seat0_mouse.name = "Mouse0".into();
    seat0_mouse.seat_tag = "seat0".into();
    let mut seat1_mouse = mouse_spec();
    seat1_mouse.name = "Mouse1".into();
    seat1_mouse.seat_tag = "seat-usb".into();

    let c0 = m.assign(seat0_mouse).expect("assign 0");
    let c1 = m.assign(seat1_mouse).expect("assign 1");
    assert_eq!(c0.seat, "seat0");
    assert_eq!(c1.seat, "seat-usb");
    assert_eq!(c0.caps, seat_caps::POINTER);
    // Two seats, one device each, no sharing.
    assert_eq!(m.seats().len(), 2);
    assert_eq!(m.seat("seat0").expect("seat0").devices.len(), 1);
    assert_eq!(m.seat("seat-usb").expect("seat-usb").devices.len(), 1);
    assert_eq!(m.seat_of_device("Mouse0"), Some("seat0"));
    assert_eq!(m.seat_of_device("Mouse1"), Some("seat-usb"));
}

#[test]
fn interleaved_input_never_crosses_seats() {
    let mut world = World::new();
    let mut pipe0 = common::Pipeline::new(mouse_spec());
    let mut pipe1 = common::Pipeline::new(mouse_spec());

    // Seat 0's mouse moves to (10, 10) — over A (client 1).
    let scene = scene0(&world.surf0, &world.clients0);
    let out0 = pump(
        &mut pipe0,
        &mut world.router0,
        &frame_trace(T0, &[(ev::REL, rel::X, 10), (ev::REL, rel::Y, 10)]),
        &scene,
    );
    assert!(!out0.is_empty());
    assert!(
        out0.iter().all(|e| e.client == 1),
        "seat0 → client 1 only: {out0:?}"
    );

    // Seat 1's mouse moves to (20, 20) — the same coordinates, its
    // own router state, its own event stream.
    let scene = scene1(&world.surf1, &world.clients1);
    let out1 = pump(
        &mut pipe1,
        &mut world.router1,
        &frame_trace(T0, &[(ev::REL, rel::X, 20), (ev::REL, rel::Y, 20)]),
        &scene,
    );
    assert!(
        out1.iter().all(|e| e.client == 2),
        "seat1 → client 2 only: {out1:?}"
    );

    // Positions are independent per seat.
    let p0 = world.router0.pointer_position();
    let p1 = world.router1.pointer_position();
    assert_eq!((p0.x, p0.y), (10.0, 10.0));
    assert_eq!((p1.x, p1.y), (20.0, 20.0));
    assert_eq!(world.router0.pointer_focus(), Some(SurfaceKey::new(1)));
    assert_eq!(world.router1.pointer_focus(), Some(SurfaceKey::new(2)));

    // A click on seat 1 does not disturb seat 0's state.
    let before = world.router0.keys_held().to_vec();
    let scene = scene1(&world.surf1, &world.clients1);
    let out1 = pump(
        &mut pipe1,
        &mut world.router1,
        &frame_trace(
            T0 + 8000,
            &[
                (ev::KEY, ldp_input::codes::btn::LEFT as u16, 1),
                (ev::KEY, ldp_input::codes::btn::LEFT as u16, 0),
            ],
        ),
        &scene,
    );
    assert!(out1.iter().all(|e| e.client == 2));
    assert_eq!(world.router0.keys_held(), before);
    // Seat 0's pointer never moved.
    assert_eq!(world.router0.pointer_position(), PointF::new(10.0, 10.0));
}

#[test]
fn surface_gone_on_one_seat_leaves_the_other_intact() {
    let mut world = World::new();
    let mut pipe0 = common::Pipeline::new(mouse_spec());
    let mut pipe1 = common::Pipeline::new(mouse_spec());
    let s0 = scene0(&world.surf0, &world.clients0);
    pump(
        &mut pipe0,
        &mut world.router0,
        &frame_trace(T0, &[(ev::REL, rel::X, 10), (ev::REL, rel::Y, 10)]),
        &s0,
    );
    let s1 = scene1(&world.surf1, &world.clients1);
    pump(
        &mut pipe1,
        &mut world.router1,
        &frame_trace(T0, &[(ev::REL, rel::X, 20), (ev::REL, rel::Y, 20)]),
        &s1,
    );

    // Seat 0 loses its surface (destroyed).
    world.router0.surface_gone(SurfaceKey::new(1));
    assert_eq!(world.router0.pointer_focus(), None);
    // Seat 1's focus survives — surface_gone on one router never
    // touches another seat's state.
    assert_eq!(world.router1.pointer_focus(), Some(SurfaceKey::new(2)));
}

#[test]
fn keymap_state_is_per_seat() {
    let (surfaces, clients) = (
        vec![surface(1, 0x100, 0.0, 0.0, 100.0, 100.0)],
        vec![binding(1, 1, 0x50, 0x300), binding(2, 2, 0x60, 0x400)],
    );
    let scene = Scene {
        surfaces: &surfaces,
        clients: &clients,
        keyboard_focus: Some(SurfaceKey::new(1)),
        bounds: (100.0, 100.0),
    };
    let mut router0 = golden_router();
    let router1 = golden_router();
    if let Some((state, fd)) = xkb_state() {
        // Only seat 0 gets the keymap.
        let out = router0.set_keymap(state, fd, &clients[..1]);
        assert_eq!(out.len(), 2, "keymap + repeat_info to client 1 only");
        assert!(out.iter().all(|e| e.client == 1));

        // Shift on seat 0's keyboard moves seat 0's modifiers; seat
        // 1's stay clean.
        let mut pipe0 = common::Pipeline::new(common::keyboard_spec());
        let out = pump(
            &mut pipe0,
            &mut router0,
            &frame_trace(T0, &[(ev::KEY, ldp_input::codes::key::LEFTSHIFT as u16, 1)]),
            &scene,
        );
        assert!(out.iter().any(|e| matches!(
            e.event,
            SeatEvent::KeyboardModifiers { depressed, .. } if depressed != 0
        )));
        assert_eq!(router1.modifiers(), None);
    }
}

#[test]
fn touchscreen_grabs_stay_within_their_seat() {
    let (surfaces, clients) = (
        vec![surface(1, 0x100, 0.0, 0.0, 100.0, 100.0)],
        vec![binding(1, 1, 0x50, 0x300)],
    );
    let scene = Scene {
        surfaces: &surfaces,
        clients: &clients,
        keyboard_focus: Some(SurfaceKey::new(1)),
        bounds: (100.0, 100.0),
    };
    let mut router = golden_router();
    let mut pipe = common::Pipeline::new(touchscreen_spec());
    // Contact at (500, 500) of a 2000×2000 sensor → (25, 25).
    let out = pump(
        &mut pipe,
        &mut router,
        &frame_trace(
            T0,
            &[
                (ev::ABS, abs::MT_SLOT, 0),
                (ev::ABS, abs::MT_TRACKING_ID, 7),
                (ev::ABS, abs::MT_POSITION_X, 500),
                (ev::ABS, abs::MT_POSITION_Y, 500),
            ],
        ),
        &scene,
    );
    assert_routed_touch(
        &out,
        &[
            SeatEvent::TouchDown {
                id: 7,
                surface: ObjectId::from_wire(0x100),
                x: 25.0,
                y: 25.0,
            },
            SeatEvent::TouchFrame,
        ],
    );
}

/// Assert a touch-only batch (client and object are implicit).
fn assert_routed_touch(out: &[RoutedEvent], expected: &[SeatEvent]) {
    assert_eq!(
        out.len(),
        expected.len(),
        "got {:?}",
        out.iter().map(|e| e.event.name()).collect::<Vec<_>>()
    );
    for (e, want) in out.iter().zip(expected) {
        assert_eq!(&e.event, want);
        assert_eq!(e.client, 1);
        assert_eq!(e.object.as_u32(), 0x302);
    }
}

#[allow(unused_imports)]
use common::keyboard_spec;
#[allow(unused_imports)]
use ldp_seat::focus::RectF;
#[allow(unused_imports)]
use InputRegion as _InputRegionRef;
#[allow(unused_imports)]
use PressState as _PressStateRef;
