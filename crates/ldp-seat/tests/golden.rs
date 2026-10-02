//! THE Phase 11 exit criterion: golden evdev traces → protocol events.
//!
//! Byte-literal device traces (the input side is golden by
//! construction) drive the full stack — stream decoder, framer,
//! normalizer, router — and the routed batches are asserted event by
//! event against hand-derived expectations, then re-encoded on the
//! wire and decoded back (the codec round-trip golden) with a
//! byte-stability check (encode twice → identical bytes).
//!
//! The acceleration curve is the identity profile here (the curve
//! itself is pinned by the property suite in `ldp-input`), so every
//! coordinate below is hand-computable arithmetic.

mod common;

use common::{
    binding, frame_trace, golden_router, golden_scene, mouse_spec, names, pump, surface,
    tablet_spec, touchpad_spec, touchscreen_spec, xkb_state,
};
use ldp_core::ids::ObjectId;
use ldp_core::limits::Limits;
use ldp_input::codes::{abs, btn, ev, key, rel};
use ldp_protocol::{decode, ValidationMode};
use ldp_seat::event::{AxisSource, KeymapFormat, PressState, SeatEvent, TabletToolType};
use ldp_seat::focus::{ClientBinding, SurfaceRef};
use ldp_seat::route::FINGER_WHEEL_RADIUS_MM;

use ldp_seat::route::RoutedEvent;

const T0: u64 = 1_000_000; // µs

/// The golden scene: A bottom (full), B top (right half).
fn scene_parts() -> (Vec<SurfaceRef>, Vec<ClientBinding>) {
    (
        vec![
            surface(2, 0x200, 100.0, 0.0, 100.0, 200.0),
            surface(1, 0x100, 0.0, 0.0, 200.0, 200.0),
        ],
        vec![
            binding(1, 1, 0x50, 0x300), // client 1 owns A
            binding(2, 2, 0x50, 0x400), // client 2 owns B
        ],
    )
}

#[test]
fn mouse_enter_motion_click() {
    let (surfaces, clients) = scene_parts();
    let scene = golden_scene(&surfaces, &clients);
    let mut pipeline = common::Pipeline::new(mouse_spec());
    let mut router = golden_router();

    // Frame 1: move to (5, 3) — over A.
    let out = pump(
        &mut pipeline,
        &mut router,
        &frame_trace(T0, &[(ev::REL, rel::X, 5), (ev::REL, rel::Y, 3)]),
        &scene,
    );
    let expected = vec![
        (
            1,
            0x300,
            SeatEvent::PointerEnter {
                surface: ObjectId::from_wire(0x100),
                x: 5.0,
                y: 3.0,
            },
        ),
        (
            1,
            0x300,
            SeatEvent::PointerRelativeMotion {
                dx_unaccel: 5.0,
                dy_unaccel: 3.0,
                dx_accel: 5.0,
                dy_accel: 3.0,
            },
        ),
        (1, 0x300, SeatEvent::PointerFrame),
    ];
    assert_routed(&out, &expected);

    // Frame 2: to (10, 6) — still A: plain motion.
    let out = pump(
        &mut pipeline,
        &mut router,
        &frame_trace(T0 + 8000, &[(ev::REL, rel::X, 5), (ev::REL, rel::Y, 3)]),
        &scene,
    );
    let expected = vec![
        (1, 0x300, SeatEvent::PointerMotion { x: 10.0, y: 6.0 }),
        (
            1,
            0x300,
            SeatEvent::PointerRelativeMotion {
                dx_unaccel: 5.0,
                dy_unaccel: 3.0,
                dx_accel: 5.0,
                dy_accel: 3.0,
            },
        ),
        (1, 0x300, SeatEvent::PointerFrame),
    ];
    assert_routed(&out, &expected);

    // Frame 3: click — implicit grab on A.
    let out = pump(
        &mut pipeline,
        &mut router,
        &frame_trace(T0 + 16_000, &[(ev::KEY, btn::LEFT as u16, 1)]),
        &scene,
    );
    assert_routed(
        &out,
        &[
            (
                1,
                0x300,
                SeatEvent::PointerButton {
                    button: btn::LEFT,
                    state: PressState::Pressed,
                },
            ),
            (1, 0x300, SeatEvent::PointerFrame),
        ],
    );

    // Frame 4: release — grab ends.
    let out = pump(
        &mut pipeline,
        &mut router,
        &frame_trace(T0 + 200_000, &[(ev::KEY, btn::LEFT as u16, 0)]),
        &scene,
    );
    assert_routed(
        &out,
        &[
            (
                1,
                0x300,
                SeatEvent::PointerButton {
                    button: btn::LEFT,
                    state: PressState::Released,
                },
            ),
            (1, 0x300, SeatEvent::PointerFrame),
        ],
    );
}

#[test]
fn mouse_crossing_emits_leave_and_enter() {
    let (surfaces, clients) = scene_parts();
    let scene = golden_scene(&surfaces, &clients);
    let mut pipeline = common::Pipeline::new(mouse_spec());
    let mut router = golden_router();

    // Into A at (95, 47).
    pump(
        &mut pipeline,
        &mut router,
        &frame_trace(T0, &[(ev::REL, rel::X, 95), (ev::REL, rel::Y, 47)]),
        &scene,
    );
    // Cross into B: (105, 47) — leave A, enter B (local 5, 47).
    let out = pump(
        &mut pipeline,
        &mut router,
        &frame_trace(T0 + 8000, &[(ev::REL, rel::X, 10)]),
        &scene,
    );
    assert_routed(
        &out,
        &[
            (
                1,
                0x300,
                SeatEvent::PointerLeave {
                    surface: ObjectId::from_wire(0x100),
                },
            ),
            (
                2,
                0x400,
                SeatEvent::PointerEnter {
                    surface: ObjectId::from_wire(0x200),
                    x: 5.0,
                    y: 47.0,
                },
            ),
            (
                2,
                0x400,
                SeatEvent::PointerRelativeMotion {
                    dx_unaccel: 10.0,
                    dy_unaccel: 0.0,
                    dx_accel: 10.0,
                    dy_accel: 0.0,
                },
            ),
            (2, 0x400, SeatEvent::PointerFrame),
        ],
    );
}

#[test]
fn mouse_wheel_sequence() {
    let (surfaces, clients) = scene_parts();
    let scene = golden_scene(&surfaces, &clients);
    let mut pipeline = common::Pipeline::new(mouse_spec());
    let mut router = golden_router();
    // Pointer at (0,0) — over A.
    let out = pump(
        &mut pipeline,
        &mut router,
        &frame_trace(T0, &[(ev::REL, rel::WHEEL, -1)]),
        &scene,
    );
    assert_routed(
        &out,
        &[
            (
                1,
                0x300,
                SeatEvent::PointerAxisSource {
                    source: AxisSource::Wheel,
                },
            ),
            (
                1,
                0x300,
                SeatEvent::PointerAxis {
                    axis: ldp_seat::event::Axis::Vertical,
                    value: -1.0,
                },
            ),
            (
                1,
                0x300,
                SeatEvent::PointerAxisDiscrete {
                    axis: ldp_seat::event::Axis::Vertical,
                    discrete: -1,
                },
            ),
            (1, 0x300, SeatEvent::PointerFrame),
        ],
    );
}

#[test]
fn keyboard_keys_and_modifiers() {
    let (surfaces, clients) = scene_parts();
    let scene = golden_scene(&surfaces, &clients);
    let mut pipeline = common::Pipeline::new(common::keyboard_spec());
    let mut router = golden_router();
    if let Some((state, fd)) = xkb_state() {
        let keymap_out = router.set_keymap(state, fd, &clients);
        // keymap + repeat_info to every client with a keyboard object.
        assert_eq!(keymap_out.len(), 4);
        assert!(matches!(
            keymap_out[0].event,
            SeatEvent::KeyboardKeymap {
                format: KeymapFormat::XkbV1
            }
        ));
        assert!(keymap_out[0].fd.is_some());
        assert_eq!(keymap_out[0].client, 1);
        assert_eq!(keymap_out[2].client, 2);
    }

    // Plain 'a' press: key event, no modifiers (the state did not change).
    let out = pump(
        &mut pipeline,
        &mut router,
        &frame_trace(T0, &[(ev::KEY, key::A as u16, 1)]),
        &scene,
    );
    assert_routed(
        &out,
        &[(
            1,
            0x301,
            SeatEvent::KeyboardKey {
                keycode: key::A,
                state: PressState::Pressed,
            },
        )],
    );

    // Shift down: modifiers follow (depressed = Shift, the mask the
    // library reports for the default map).
    let out = pump(
        &mut pipeline,
        &mut router,
        &frame_trace(T0 + 8000, &[(ev::KEY, key::LEFTSHIFT as u16, 1)]),
        &scene,
    );
    let expected_key = (
        1,
        0x301,
        SeatEvent::KeyboardKey {
            keycode: key::LEFTSHIFT,
            state: PressState::Pressed,
        },
    );
    if xkb_state().is_some() {
        assert_routed(
            &out,
            &[
                expected_key,
                (
                    1,
                    0x301,
                    SeatEvent::KeyboardModifiers {
                        depressed: 1,
                        latched: 0,
                        locked: 0,
                        group: 0,
                    },
                ),
            ],
        );
    } else {
        assert_routed(&out, &[expected_key]);
    }

    // 'a' with shift: no additional modifiers event.
    let out = pump(
        &mut pipeline,
        &mut router,
        &frame_trace(T0 + 16_000, &[(ev::KEY, key::A as u16, 1)]),
        &scene,
    );
    assert_routed(
        &out,
        &[(
            1,
            0x301,
            SeatEvent::KeyboardKey {
                keycode: key::A,
                state: PressState::Pressed,
            },
        )],
    );

    // Everything up: one device frame carries both releases; the
    // modifier summary follows the keys of its own frame.
    let out = pump(
        &mut pipeline,
        &mut router,
        &frame_trace(
            T0 + 24_000,
            &[
                (ev::KEY, key::A as u16, 0),
                (ev::KEY, key::LEFTSHIFT as u16, 0),
            ],
        ),
        &scene,
    );
    assert_eq!(names(&out), vec!["key", "key", "modifiers"]);
}

#[test]
fn touchscreen_contacts() {
    let (surfaces, clients) = scene_parts();
    let scene = golden_scene(&surfaces, &clients);
    let mut pipeline = common::Pipeline::new(touchscreen_spec());
    let mut router = golden_router();

    // One contact at (500, 500) → normalized 0.25 → output (50, 50)
    // → over A.
    let out = pump(
        &mut pipeline,
        &mut router,
        &frame_trace(
            T0,
            &[
                (ev::ABS, abs::MT_SLOT, 0),
                (ev::ABS, abs::MT_TRACKING_ID, 11),
                (ev::ABS, abs::MT_POSITION_X, 500),
                (ev::ABS, abs::MT_POSITION_Y, 500),
            ],
        ),
        &scene,
    );
    assert_routed(
        &out,
        &[
            (
                1,
                0x302,
                SeatEvent::TouchDown {
                    id: 11,
                    surface: ObjectId::from_wire(0x100),
                    x: 50.0,
                    y: 50.0,
                },
            ),
            (1, 0x302, SeatEvent::TouchFrame),
        ],
    );

    // Move to (1000, 1000) → normalized 0.5 → (100, 100): exact in f32.
    let out = pump(
        &mut pipeline,
        &mut router,
        &frame_trace(
            T0 + 8000,
            &[
                (ev::ABS, abs::MT_SLOT, 0),
                (ev::ABS, abs::MT_POSITION_X, 1000),
                (ev::ABS, abs::MT_POSITION_Y, 1000),
            ],
        ),
        &scene,
    );
    assert_routed(
        &out,
        &[
            (
                1,
                0x302,
                SeatEvent::TouchMotion {
                    id: 11,
                    x: 100.0,
                    y: 100.0,
                },
            ),
            (1, 0x302, SeatEvent::TouchFrame),
        ],
    );

    // Lift.
    let out = pump(
        &mut pipeline,
        &mut router,
        &frame_trace(
            T0 + 100_000,
            &[
                (ev::ABS, abs::MT_SLOT, 0),
                (ev::ABS, abs::MT_TRACKING_ID, -1),
            ],
        ),
        &scene,
    );
    assert_routed(
        &out,
        &[
            (1, 0x302, SeatEvent::TouchUp { id: 11 }),
            (1, 0x302, SeatEvent::TouchFrame),
        ],
    );
}

#[test]
fn touchpad_single_finger_moves_pointer() {
    let (surfaces, clients) = scene_parts();
    let scene = golden_scene(&surfaces, &clients);
    let mut pipeline = common::Pipeline::new(touchpad_spec());
    let mut router = golden_router();

    // Finger down at (20mm, 20mm): first frame establishes the
    // stroke, no motion.
    let out = pump(
        &mut pipeline,
        &mut router,
        &frame_trace(
            T0,
            &[
                (ev::ABS, abs::MT_SLOT, 0),
                (ev::ABS, abs::MT_TRACKING_ID, 11),
                (ev::ABS, abs::MT_POSITION_X, 800), // 20 mm at 40 units/mm
                (ev::ABS, abs::MT_POSITION_Y, 20),
            ],
        ),
        &scene,
    );
    assert!(out.is_empty(), "first frame only anchors: {out:?}");

    // Stroke moves +10mm X: pointer moves to (10, 0) — over A.
    let out = pump(
        &mut pipeline,
        &mut router,
        &frame_trace(
            T0 + 8000,
            &[
                (ev::ABS, abs::MT_SLOT, 0),
                (ev::ABS, abs::MT_POSITION_X, 1200),
            ],
        ),
        &scene,
    );
    assert_routed(
        &out,
        &[
            (
                1,
                0x300,
                SeatEvent::PointerEnter {
                    surface: ObjectId::from_wire(0x100),
                    x: 10.0,
                    y: 0.0,
                },
            ),
            (
                1,
                0x300,
                SeatEvent::PointerRelativeMotion {
                    dx_unaccel: 10.0,
                    dy_unaccel: 0.0,
                    dx_accel: 10.0,
                    dy_accel: 0.0,
                },
            ),
            (1, 0x300, SeatEvent::PointerFrame),
        ],
    );
}

#[test]
#[allow(clippy::too_many_lines)] // the corpus traces are the point
fn touchpad_two_finger_scroll() {
    let (surfaces, clients) = scene_parts();
    let scene = golden_scene(&surfaces, &clients);
    let mut pipeline = common::Pipeline::new(touchpad_spec());
    let mut router = golden_router();

    // Anchor the pointer focus with a single-finger stroke first
    // (scroll routes to the pointer focus).
    pump(
        &mut pipeline,
        &mut router,
        &frame_trace(
            T0,
            &[
                (ev::ABS, abs::MT_SLOT, 0),
                (ev::ABS, abs::MT_TRACKING_ID, 11),
                (ev::ABS, abs::MT_POSITION_X, 800),
                (ev::ABS, abs::MT_POSITION_Y, 20),
            ],
        ),
        &scene,
    );
    pump(
        &mut pipeline,
        &mut router,
        &frame_trace(
            T0 + 8000,
            &[
                (ev::ABS, abs::MT_SLOT, 0),
                (ev::ABS, abs::MT_POSITION_X, 900),
            ],
        ),
        &scene,
    );

    // Two fingers down at (20, 20)mm and (24, 20)mm.
    let out = pump(
        &mut pipeline,
        &mut router,
        &frame_trace(
            T0 + 20_000,
            &[
                (ev::ABS, abs::MT_SLOT, 0),
                (ev::ABS, abs::MT_TRACKING_ID, 11),
                (ev::ABS, abs::MT_POSITION_X, 800),
                (ev::ABS, abs::MT_POSITION_Y, 800),
                (ev::ABS, abs::MT_SLOT, 1),
                (ev::ABS, abs::MT_TRACKING_ID, 12),
                (ev::ABS, abs::MT_POSITION_X, 960),
                (ev::ABS, abs::MT_POSITION_Y, 800),
            ],
        ),
        &scene,
    );
    // Finger-count change during pending: nothing classified yet.
    assert!(out.is_empty(), "pending: {out:?}");

    // Both fingers move down 2 mm in parallel: the stroke classifies
    // as a scroll (begin only — deltas flow from the next frame).
    let out = pump(
        &mut pipeline,
        &mut router,
        &frame_trace(
            T0 + 30_000,
            &[
                (ev::ABS, abs::MT_SLOT, 0),
                (ev::ABS, abs::MT_POSITION_Y, 880),
                (ev::ABS, abs::MT_SLOT, 1),
                (ev::ABS, abs::MT_POSITION_Y, 880),
            ],
        ),
        &scene,
    );
    assert_routed(
        &out,
        &[
            (
                1,
                0x300,
                SeatEvent::PointerAxisSource {
                    source: AxisSource::Finger,
                },
            ),
            (1, 0x300, SeatEvent::PointerFrame),
        ],
    );

    // Another 2 mm: the update carries the delta in radians.
    let out = pump(
        &mut pipeline,
        &mut router,
        &frame_trace(
            T0 + 40_000,
            &[
                (ev::ABS, abs::MT_SLOT, 0),
                (ev::ABS, abs::MT_POSITION_Y, 960),
                (ev::ABS, abs::MT_SLOT, 1),
                (ev::ABS, abs::MT_POSITION_Y, 960),
            ],
        ),
        &scene,
    );
    let expected_value = -2.0f32 / FINGER_WHEEL_RADIUS_MM;
    assert_routed(
        &out,
        &[
            (
                1,
                0x300,
                SeatEvent::PointerAxis {
                    axis: ldp_seat::event::Axis::Vertical,
                    value: expected_value,
                },
            ),
            (1, 0x300, SeatEvent::PointerFrame),
        ],
    );

    // Lift one finger: scroll ends (axis_stop + frame).
    let out = pump(
        &mut pipeline,
        &mut router,
        &frame_trace(
            T0 + 100_000,
            &[
                (ev::ABS, abs::MT_SLOT, 0),
                (ev::ABS, abs::MT_TRACKING_ID, -1),
            ],
        ),
        &scene,
    );
    assert_eq!(names(&out), vec!["axis_stop", "axis_stop", "frame"]);
}

#[test]
#[allow(clippy::too_many_lines)] // the corpus traces are the point
fn touchpad_three_finger_swipe() {
    let (surfaces, clients) = scene_parts();
    let scene = golden_scene(&surfaces, &clients);
    let mut pipeline = common::Pipeline::new(touchpad_spec());
    let mut router = golden_router();

    // Pointer focus anchored on A.
    pump(
        &mut pipeline,
        &mut router,
        &frame_trace(
            T0,
            &[
                (ev::ABS, abs::MT_SLOT, 0),
                (ev::ABS, abs::MT_TRACKING_ID, 11),
                (ev::ABS, abs::MT_POSITION_X, 800),
                (ev::ABS, abs::MT_POSITION_Y, 20),
            ],
        ),
        &scene,
    );
    pump(
        &mut pipeline,
        &mut router,
        &frame_trace(
            T0 + 8000,
            &[
                (ev::ABS, abs::MT_SLOT, 0),
                (ev::ABS, abs::MT_POSITION_X, 900),
            ],
        ),
        &scene,
    );

    // Three fingers down.
    let out = pump(
        &mut pipeline,
        &mut router,
        &frame_trace(
            T0 + 20_000,
            &[
                (ev::ABS, abs::MT_SLOT, 0),
                (ev::ABS, abs::MT_TRACKING_ID, 11),
                (ev::ABS, abs::MT_POSITION_X, 800),
                (ev::ABS, abs::MT_POSITION_Y, 800),
                (ev::ABS, abs::MT_SLOT, 1),
                (ev::ABS, abs::MT_TRACKING_ID, 12),
                (ev::ABS, abs::MT_POSITION_X, 960),
                (ev::ABS, abs::MT_POSITION_Y, 800),
                (ev::ABS, abs::MT_SLOT, 2),
                (ev::ABS, abs::MT_TRACKING_ID, 13),
                (ev::ABS, abs::MT_POSITION_X, 1120),
                (ev::ABS, abs::MT_POSITION_Y, 800),
            ],
        ),
        &scene,
    );
    assert!(out.is_empty());

    // All three move +3mm X: swipe begins (fingers ≥ 3, parallel).
    let out = pump(
        &mut pipeline,
        &mut router,
        &frame_trace(
            T0 + 30_000,
            &[
                (ev::ABS, abs::MT_SLOT, 0),
                (ev::ABS, abs::MT_POSITION_X, 920),
                (ev::ABS, abs::MT_SLOT, 1),
                (ev::ABS, abs::MT_POSITION_X, 1080),
                (ev::ABS, abs::MT_SLOT, 2),
                (ev::ABS, abs::MT_POSITION_X, 1240),
            ],
        ),
        &scene,
    );
    assert_routed(
        &out,
        &[(1, 0x304, SeatEvent::GestureSwipeBegin { id: 1, fingers: 3 })],
    );

    // Continue: update with the centroid delta (3mm).
    let out = pump(
        &mut pipeline,
        &mut router,
        &frame_trace(
            T0 + 40_000,
            &[
                (ev::ABS, abs::MT_SLOT, 0),
                (ev::ABS, abs::MT_POSITION_X, 1040),
                (ev::ABS, abs::MT_SLOT, 1),
                (ev::ABS, abs::MT_POSITION_X, 1200),
                (ev::ABS, abs::MT_SLOT, 2),
                (ev::ABS, abs::MT_POSITION_X, 1360),
            ],
        ),
        &scene,
    );
    assert_routed(
        &out,
        &[(
            1,
            0x304,
            SeatEvent::GestureSwipeUpdate {
                id: 1,
                dx: 3.0,
                dy: 0.0,
            },
        )],
    );

    // All lift: clean end.
    let out = pump(
        &mut pipeline,
        &mut router,
        &frame_trace(
            T0 + 200_000,
            &[
                (ev::ABS, abs::MT_SLOT, 0),
                (ev::ABS, abs::MT_TRACKING_ID, -1),
                (ev::ABS, abs::MT_SLOT, 1),
                (ev::ABS, abs::MT_TRACKING_ID, -1),
                (ev::ABS, abs::MT_SLOT, 2),
                (ev::ABS, abs::MT_TRACKING_ID, -1),
            ],
        ),
        &scene,
    );
    assert_routed(
        &out,
        &[(
            1,
            0x304,
            SeatEvent::GestureSwipeEnd {
                id: 1,
                cancelled: false,
            },
        )],
    );
}

#[test]
#[allow(clippy::too_many_lines)] // the corpus traces are the point
fn tablet_proximity_contact_axes() {
    let (surfaces, clients) = scene_parts();
    let scene = golden_scene(&surfaces, &clients);
    let mut pipeline = common::Pipeline::new(tablet_spec());
    let mut router = golden_router();

    // Pen enters proximity: the tool event routes to the tablet
    // focus (the hit target at the resting position, (0,0) → A).
    let out = pump(
        &mut pipeline,
        &mut router,
        &frame_trace(T0, &[(ev::KEY, btn::TOOL_PEN as u16, 1)]),
        &scene,
    );
    assert_routed(
        &out,
        &[
            (
                1,
                0x303,
                SeatEvent::TabletTool {
                    id: 1,
                    tool_type: TabletToolType::Pen,
                },
            ),
            (1, 0x303, SeatEvent::TabletFrame),
        ],
    );

    // Pen contacts at (250, 750) → normalized (0.25, 0.75) → output
    // (50, 150) → over A: down + motion + pressure + tilt + frame.
    let out = pump(
        &mut pipeline,
        &mut router,
        &frame_trace(
            T0 + 8000,
            &[
                (ev::ABS, abs::X, 250),
                (ev::ABS, abs::Y, 750),
                (ev::ABS, abs::PRESSURE, 500),
                (ev::ABS, abs::TILT_X, 4500),
                (ev::ABS, abs::TILT_Y, -3000),
            ],
        ),
        &scene,
    );
    assert_routed(
        &out,
        &[
            (
                1,
                0x303,
                SeatEvent::TabletDown {
                    id: 1,
                    surface: ObjectId::from_wire(0x100),
                    x: 50.0,
                    y: 150.0,
                },
            ),
            (
                1,
                0x303,
                SeatEvent::TabletMotion {
                    id: 1,
                    x: 50.0,
                    y: 150.0,
                },
            ),
            (1, 0x303, SeatEvent::TabletPressure { id: 1, value: 0.5 }),
            (
                1,
                0x303,
                SeatEvent::TabletTilt {
                    id: 1,
                    x: 45.0_f32.to_radians(),
                    y: -30.0_f32.to_radians(),
                },
            ),
            (1, 0x303, SeatEvent::TabletFrame),
        ],
    );

    // Lift contact (pressure to zero): full axis state per frame —
    // the persistent tilt rides along — then leave proximity.
    let out = pump(
        &mut pipeline,
        &mut router,
        &frame_trace(T0 + 100_000, &[(ev::ABS, abs::PRESSURE, 0)]),
        &scene,
    );
    assert_routed(
        &out,
        &[
            (1, 0x303, SeatEvent::TabletUp { id: 1 }),
            (
                1,
                0x303,
                SeatEvent::TabletMotion {
                    id: 1,
                    x: 50.0,
                    y: 150.0,
                },
            ),
            (1, 0x303, SeatEvent::TabletPressure { id: 1, value: 0.0 }),
            (
                1,
                0x303,
                SeatEvent::TabletTilt {
                    id: 1,
                    x: 45.0_f32.to_radians(),
                    y: -30.0_f32.to_radians(),
                },
            ),
            (1, 0x303, SeatEvent::TabletFrame),
        ],
    );
    let out = pump(
        &mut pipeline,
        &mut router,
        &frame_trace(T0 + 200_000, &[(ev::KEY, btn::TOOL_PEN as u16, 0)]),
        &scene,
    );
    assert_routed(
        &out,
        &[
            (1, 0x303, SeatEvent::TabletToolDone { id: 1 }),
            (1, 0x303, SeatEvent::TabletFrame),
        ],
    );
}

#[test]
fn wire_round_trip_and_byte_stability() {
    let (surfaces, clients) = scene_parts();
    let scene = golden_scene(&surfaces, &clients);
    let mut pipeline = common::Pipeline::new(mouse_spec());
    let mut router = golden_router();

    // The full crossing sequence.
    let mut batch = Vec::new();
    batch.extend(pump(
        &mut pipeline,
        &mut router,
        &frame_trace(T0, &[(ev::REL, rel::X, 95), (ev::REL, rel::Y, 47)]),
        &scene,
    ));
    batch.extend(pump(
        &mut pipeline,
        &mut router,
        &frame_trace(T0 + 8000, &[(ev::REL, rel::X, 10)]),
        &scene,
    ));
    batch.extend(pump(
        &mut pipeline,
        &mut router,
        &frame_trace(
            T0 + 16_000,
            &[(ev::REL, rel::WHEEL, -1), (ev::KEY, btn::LEFT as u16, 1)],
        ),
        &scene,
    ));
    assert!(!batch.is_empty());

    // Encode → decode → identical typed values, and encode twice →
    // identical bytes (the stability half of the golden).
    let limits = Limits::default();
    for e in &batch {
        let msg = e.event.to_message(e.object);
        let bytes = msg.encode(&limits).expect("encode");
        assert_eq!(
            msg.required_fd_count(),
            0,
            "no ancillary descriptors in this corpus"
        );
        // Round trip: the wire form decodes back to the same message.
        let decoded = decode(&bytes, 0, &limits, ValidationMode::Tolerant).expect("decode");
        assert_eq!(decoded.object_id, e.object.as_u32());
        assert_eq!(decoded.opcode, msg.opcode);
        assert_eq!(decoded.args, msg.args);
    }
    // Byte stability: the whole batch, encoded twice, is identical.
    let encode_all = |b: &[RoutedEvent]| -> Vec<Vec<u8>> {
        b.iter()
            .map(|e| {
                e.event
                    .to_message(e.object)
                    .encode(&limits)
                    .expect("encode")
            })
            .collect()
    };
    let once = encode_all(&batch);
    let again = encode_all(&batch);
    assert_eq!(once, again);
    assert!(!once.is_empty());
    // Non-trivial corpus: the batch spans multiple interfaces.
    let interfaces: std::collections::BTreeSet<&str> =
        batch.iter().map(|e| e.event.interface()).collect();
    assert!(interfaces.contains("ldp.input.pointer"));
}

#[test]
fn kernel_autorepeat_is_dropped_not_routed() {
    let (surfaces, clients) = scene_parts();
    let scene = golden_scene(&surfaces, &clients);
    let mut pipeline = common::Pipeline::new(common::keyboard_spec());
    let mut router = golden_router();
    let out = pump(
        &mut pipeline,
        &mut router,
        &frame_trace(
            T0,
            &[
                (ev::KEY, key::A as u16, 1),
                (ev::KEY, key::A as u16, 2), // kernel autorepeat
                (ev::KEY, key::A as u16, 2),
            ],
        ),
        &scene,
    );
    assert_routed(
        &out,
        &[(
            1,
            0x301,
            SeatEvent::KeyboardKey {
                keycode: key::A,
                state: PressState::Pressed,
            },
        )],
    );
}

/// Assert a routed batch against expected `(client, object, event)`
/// triples.
fn assert_routed(out: &[RoutedEvent], expected: &[(u32, u32, SeatEvent)]) {
    assert_eq!(
        out.len(),
        expected.len(),
        "batch size\n got: {:?}\nwant: {:?}",
        names(out),
        expected
            .iter()
            .map(|(_, _, e)| e.name())
            .collect::<Vec<_>>()
    );
    for (i, (e, (client, object, event))) in out.iter().zip(expected).enumerate() {
        assert_eq!(e.client, *client, "client at {i}");
        assert_eq!(
            e.object.as_u32(),
            *object,
            "object at {i} ({})",
            e.event.name()
        );
        assert_eq!(&e.event, event, "event at {i}");
    }
}
