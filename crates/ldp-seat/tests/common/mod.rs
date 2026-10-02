//! The shared input-pipeline testbench: evdev trace bytes to routed
//! protocol events in one call.
//!
//! Every integration suite (golden corpus, multi-seat isolation,
//! grabs) drives the *real* stack: [`StreamDecoder`] → [`Framer`] →
//! [`Normalizer`] → [`Router`]. The traces are byte literals — the
//! corpus input side is golden by construction.

#![allow(dead_code, clippy::module_name_repetitions)]

use std::os::fd::OwnedFd;

use ldp_core::geometry::PointF;
use ldp_core::ids::ObjectId;
use ldp_core::time::Mono;
use ldp_input::accel::AccelProfile;
use ldp_input::codes::{abs, btn, ev, key, rel, syn};
use ldp_input::device::{AbsInfo, ClassHint, DeviceClass, DeviceId, DeviceSpec};
use ldp_input::evdev::{Framer, RawEvent, StreamDecoder};
use ldp_input::normalizer::{InputEvent, Normalizer};
use ldp_input::xkb;
use ldp_seat::focus::{ClientBinding, InputRegion, Scene, SurfaceKey, SurfaceRef};
use ldp_seat::route::{RoutedEvent, Router, RouterConfig};
use ldp_seat::seat::{Seat, SeatManager};

/// The identity acceleration curve (plateau factor 1): the golden
/// corpus stays hand-computable; the curve itself is pinned by the
/// property suite in `ldp-input`.
pub fn flat_accel() -> AccelProfile {
    AccelProfile {
        threshold: 1.0,
        plateau_speed: 2.0,
        plateau_factor: 1.0,
    }
}

/// A standard USB mouse.
pub fn mouse_spec() -> DeviceSpec {
    DeviceSpec::new(
        "Golden Mouse",
        DeviceId {
            bustype: 0x03,
            vendor: 0x1,
            product: 0x2,
            version: 0x3,
        },
    )
    .with_bit(ev::REL, rel::X)
    .with_bit(ev::REL, rel::Y)
    .with_bit(ev::REL, rel::WHEEL)
    .with_bit(ev::KEY, btn::LEFT as u16)
    .with_bit(ev::KEY, btn::RIGHT as u16)
    .with_hint(ClassHint {
        mouse: true,
        ..ClassHint::default()
    })
}

/// A standard USB keyboard.
pub fn keyboard_spec() -> DeviceSpec {
    DeviceSpec::new(
        "Golden Keyboard",
        DeviceId {
            bustype: 0x03,
            vendor: 0x1,
            product: 0x2,
            version: 0x3,
        },
    )
    .with_bit(ev::KEY, key::A as u16)
    .with_bit(ev::KEY, key::LEFTSHIFT as u16)
    .with_bit(ev::KEY, key::CAPSLOCK as u16)
    .with_hint(ClassHint {
        keyboard: true,
        ..ClassHint::default()
    })
}

/// A multitouch touchscreen (2000×2000 sensor: the normalized
/// coordinates stay exact in f32 — golden floats are hand-checked).
pub fn touchscreen_spec() -> DeviceSpec {
    DeviceSpec::new(
        "Golden Touchscreen",
        DeviceId {
            bustype: 0x03,
            vendor: 0x1,
            product: 0x2,
            version: 0x3,
        },
    )
    .with_abs(abs::MT_POSITION_X, AbsInfo::range(2000))
    .with_abs(abs::MT_POSITION_Y, AbsInfo::range(2000))
    .with_abs(abs::MT_SLOT, AbsInfo::range(4))
    .with_abs(abs::MT_TRACKING_ID, AbsInfo::range(65535))
    .with_bit(ev::KEY, btn::TOUCH as u16)
    .with_hint(ClassHint {
        touchscreen: true,
        ..ClassHint::default()
    })
}

/// A multitouch touchpad (3200×2100 units, 40 units/mm on X).
pub fn touchpad_spec() -> DeviceSpec {
    let mut spec = DeviceSpec::new(
        "Golden Touchpad",
        DeviceId {
            bustype: 0x11,
            vendor: 0x1,
            product: 0x2,
            version: 0x3,
        },
    )
    .with_abs(abs::MT_POSITION_X, AbsInfo::range(3200))
    .with_abs(abs::MT_POSITION_Y, AbsInfo::range(2100))
    .with_abs(abs::MT_SLOT, AbsInfo::range(2))
    .with_abs(abs::MT_TRACKING_ID, AbsInfo::range(65535))
    .with_bit(ev::KEY, btn::TOOL_FINGER as u16)
    .with_bit(ev::KEY, btn::LEFT as u16)
    .with_hint(ClassHint {
        touchpad: true,
        ..ClassHint::default()
    });
    for code in [abs::MT_POSITION_X, abs::MT_POSITION_Y] {
        let mut a = *spec.abs.get(&code).unwrap();
        a.resolution = 40;
        spec.abs.insert(code, a);
    }
    spec
}

/// A pressure + tilt tablet.
pub fn tablet_spec() -> DeviceSpec {
    DeviceSpec::new(
        "Golden Tablet",
        DeviceId {
            bustype: 0x03,
            vendor: 0x1,
            product: 0x2,
            version: 0x3,
        },
    )
    .with_abs(abs::X, AbsInfo::range(1000))
    .with_abs(abs::Y, AbsInfo::range(1000))
    .with_abs(abs::PRESSURE, AbsInfo::range(1000))
    .with_abs(abs::TILT_X, AbsInfo::symmetric(9000))
    .with_abs(abs::TILT_Y, AbsInfo::symmetric(9000))
    .with_bit(ev::KEY, btn::TOOL_PEN as u16)
    .with_bit(ev::KEY, btn::STYLUS as u16)
    .with_bit(ev::KEY, btn::TOUCH as u16)
    .with_hint(ClassHint {
        tablet: true,
        ..ClassHint::default()
    })
}

/// The per-device pipeline: trace bytes in, normalized frames out.
pub struct Pipeline {
    decoder: StreamDecoder,
    framer: Framer,
    normalizer: Normalizer,
}

impl Pipeline {
    /// Build for a device.
    pub fn new(spec: DeviceSpec) -> Pipeline {
        Pipeline {
            decoder: StreamDecoder::new(),
            framer: Framer::new(),
            normalizer: Normalizer::new(spec),
        }
    }

    /// Feed trace bytes; returns `(frame time, events)` pairs.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<(Mono, Vec<InputEvent>)> {
        let events = self.decoder.feed(bytes);
        let frames = self.framer.feed_all(&events);
        frames
            .into_iter()
            .map(|f| {
                let time = f.time;
                let evs = self.normalizer.normalize_frame(&f);
                (time, evs)
            })
            .collect()
    }

    /// The device class (for the router's touchpad/touchscreen split).
    pub fn class(&self) -> DeviceClass {
        self.normalizer.spec().classify()
    }
}

/// Build an evdev trace from `(time_us, type, code, value)` records.
pub fn trace(records: &[(u64, u16, u16, i32)]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for (t, ty, code, value) in records {
        bytes.extend_from_slice(&RawEvent::new(*t, *ty, *code, *value).to_bytes());
    }
    bytes
}

/// A one-frame trace helper: appends the `SYN_REPORT` terminator.
pub fn frame_trace(time_us: u64, records: &[(u16, u16, i32)]) -> Vec<u8> {
    let mut with_time: Vec<(u64, u16, u16, i32)> = records
        .iter()
        .map(|(t, c, v)| (time_us, *t, *c, *v))
        .collect();
    with_time.push((time_us, ev::SYN, syn::REPORT, 0));
    trace(&with_time)
}

/// A surface in the golden scene.
pub fn surface(key: u64, object: u32, x: f32, y: f32, w: f32, h: f32) -> SurfaceRef {
    SurfaceRef {
        key: SurfaceKey::new(key),
        surface: ObjectId::from_wire(object),
        origin: PointF::new(x, y),
        size: (w, h),
        region: InputRegion::everywhere(),
    }
}

/// A fully-bound client.
pub fn binding(client: u32, surface: u64, seat: u32, base: u32) -> ClientBinding {
    ClientBinding {
        client,
        seat: ObjectId::from_wire(seat),
        surface: SurfaceKey::new(surface),
        pointer: Some(ObjectId::from_wire(base)),
        keyboard: Some(ObjectId::from_wire(base + 1)),
        touch: Some(ObjectId::from_wire(base + 2)),
        tablet: Some(ObjectId::from_wire(base + 3)),
        gestures: Some(ObjectId::from_wire(base + 4)),
    }
}

/// The standard golden scene: 200×200 bounds.
pub fn golden_scene<'a>(surfaces: &'a [SurfaceRef], clients: &'a [ClientBinding]) -> Scene<'a> {
    Scene {
        surfaces,
        clients,
        keyboard_focus: Some(SurfaceKey::new(1)),
        bounds: (200.0, 200.0),
    }
}

/// A router with the flat curve.
pub fn golden_router() -> Router {
    let cfg = RouterConfig {
        accel: flat_accel(),
        ..RouterConfig::default()
    };
    Router::new(cfg)
}

/// Feed trace bytes through the pipeline and the router.
pub fn pump(
    pipeline: &mut Pipeline,
    router: &mut Router,
    bytes: &[u8],
    scene: &Scene<'_>,
) -> Vec<RoutedEvent> {
    let mut out = Vec::new();
    for (time, events) in pipeline.feed(bytes) {
        out.extend(router.feed_frame(pipeline.class(), &events, time, scene));
    }
    out
}

/// A live xkb state (or `None` on machines without libxkbcommon).
pub fn xkb_state() -> Option<(xkb::State, OwnedFd)> {
    let lib = std::sync::Arc::new(xkb::sys::LibXkb::open().ok()?);
    let ctx = xkb::Context::new(lib).ok()?;
    let keymap = ctx.compile(&xkb::Rmlvo::default_selection()).ok()?;
    let state = keymap.state().ok()?;
    let fd = keymap.client_fd().ok()?;
    Some((state, fd))
}

/// The event names of a routed batch (compact assertion output).
pub fn names(events: &[RoutedEvent]) -> Vec<&'static str> {
    events.iter().map(|e| e.event.name()).collect()
}

/// Assign the golden devices to seats by tag.
pub fn golden_seat_manager() -> SeatManager {
    let mut m = SeatManager::new();
    for spec in [
        mouse_spec(),
        keyboard_spec(),
        touchscreen_spec(),
        touchpad_spec(),
        tablet_spec(),
    ] {
        m.assign(spec).expect("assign");
    }
    m
}

/// The capability mask of the golden device set.
pub fn golden_caps() -> u32 {
    Seat::compute_caps(&[
        mouse_spec(),
        keyboard_spec(),
        touchscreen_spec(),
        touchpad_spec(),
        tablet_spec(),
    ])
}
