//! The Phase 19 hotplug simulation: connector topology churn under a
//! live compositor.
//!
//! Two levels:
//!
//! * **the device level** — the mock KMS device's hotplug injection
//!   ([`MockDevice::hotplug_connect`] /
//!   [`MockDevice::hotplug_disconnect`]) and the deterministic
//!   topology diff ([`Reprobe::diff`]): additions, removals, and
//!   status changes come out in the documented order.
//! * **the compositor level** — the same injections into *the
//!   compositor's own device* while the frame pipeline runs: a client
//!   keeps committing frames and receiving presentation verdicts
//!   across every churn step, and the compositor keeps rendering
//!   (frames advance monotonically).
//!
//! The active connector (the one feeding the compositor's CRTC) is
//! left driving throughout — churn targets the idle connectors, the
//! topology surface a real compositor sees when monitors come and go.

use std::time::{Duration, Instant};

use ldp_core::geometry::Rect;
use ldp_display::backend::KmsBackend;
use ldp_display::connector::ConnectorStatus;
use ldp_display::hotplug::Reprobe;
use ldp_display::ids::ConnectorId;
use ldp_display::mock::MockDevice;

use crate::harness::{Client, CompositorHandle};

/// The connected-status snapshot in resource-list order.
fn snapshot(device: &mut MockDevice) -> Vec<(ConnectorId, ConnectorStatus)> {
    let topology = device.topology().expect("topology");
    topology
        .connectors
        .iter()
        .map(|id| {
            let status = device.connector_info(*id).expect("connector info").status;
            (*id, status)
        })
        .collect()
}

/// One commit cycle on a live client surface (returns after the
/// presentation verdict).
fn cycle(client: &mut Client, surface: &ldp_client::Proxy, n: u64) {
    let cookie = 0x3000 + u32::try_from(n).expect("cycle count");
    client.frame(surface, n);
    client.wait_until(|rec| rec.records.iter().any(|r| r.event == "frame_target"));
    client.damage(surface, &[Rect::new(0, 0, 2, 2)]);
    client.commit(surface, cookie);
    client.wait_until(|rec| {
        rec.records
            .iter()
            .any(|r| r.event == "committed" && r.args[0] == ldp_core::wire::Value::Uint32(cookie))
    });

    client.wait_until(|rec| rec.records.iter().any(|r| r.event == "presented"));
}

/// The device-level corpus: inject, diff, verify.
///
/// # Panics
///
/// On any diff disagreement with the injected topology.
pub fn run_device_level() {
    let mut device = MockDevice::laptop_dual();
    let before = snapshot(&mut device);

    // Find an idle (disconnected) connector and a spare connected one.
    let idle = before
        .iter()
        .find(|(_, s)| *s == ConnectorStatus::Disconnected)
        .map(|(id, _)| *id)
        .expect("the preset has a disconnected connector");
    let spare = before
        .iter()
        .find(|(_, s)| *s == ConnectorStatus::Connected)
        .map(|(id, _)| *id)
        .expect("the preset has a connected connector");

    // Hotplug a monitor onto the idle connector: status change (and
    // the queued HotplugEvent the udev path would deliver).
    let modes = device
        .connector_info(spare)
        .expect("spare info")
        .modes
        .clone();
    let edid = vec![0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00];
    device.hotplug_connect(idle, modes, edid);
    let after = snapshot(&mut device);
    let diff = Reprobe::diff(&before, &after);
    assert_eq!(
        diff.removed,
        Vec::new(),
        "a connect must not remove anything"
    );
    assert!(
        diff.changed.contains(&(idle, ConnectorStatus::Connected)),
        "the connect must flip the idle connector's status: {diff:?}"
    );

    // Unplug it again: back to the original topology.
    device.hotplug_disconnect(idle);
    let again = snapshot(&mut device);
    assert_eq!(again, before, "connect-then-disconnect restores topology");

    // Unplug the spare connected connector: a status change the other
    // way (the diff orders removals, then additions, then changes).
    device.hotplug_disconnect(spare);
    let unplugged = snapshot(&mut device);
    let diff = Reprobe::diff(&before, &unplugged);
    assert!(
        diff.changed
            .contains(&(spare, ConnectorStatus::Disconnected)),
        "the disconnect must flip the spare's status: {diff:?}"
    );
    assert_eq!(diff.added, Vec::new(), "a disconnect must not add anything");
}

/// The compositor-level simulation: churn while the pipeline serves a
/// client.
///
/// # Panics
///
/// If the client's session stops completing across a churn step, the
/// compositor stops rendering, or the scene does not drain.
pub fn run_compositor_level() {
    let handle = CompositorHandle::start("hotplug");

    // The client whose session must survive the churn.
    let mut client = Client::connect(&handle.addr);
    let shm = client.bind("ldp.core.shm");
    client.wait_until(|rec| rec.records.iter().filter(|r| r.event == "format").count() >= 2);
    let pool = client.create_pool(&shm, Client::QUAD_POOL_BYTES, 0x70);
    let buffer = client.create_buffer(&pool, 0, 2, 2, 8, 0x3432_5258);
    let compositor = client.bind("ldp.core.compositor");
    let surface = client.create_surface(&compositor);

    // The churn targets: the idle connector and a connected one that
    // is NOT feeding the compositor's CRTC.
    let active_crtc = handle.world(|w| w.crtc().expect("lit"));
    let (idle, spare) = handle.world_mut(|w| {
        let device = w.device.as_mock_mut().expect("the mock driver");
        let before = snapshot(device);
        let idle = before
            .iter()
            .find(|(_, s)| *s == ConnectorStatus::Disconnected)
            .map(|(id, _)| *id)
            .expect("idle connector");
        let spare = before
            .iter()
            .find(|(id, s)| {
                *s == ConnectorStatus::Connected
                    && device.connector_binding(*id) != Some(active_crtc)
            })
            .map(|(id, _)| *id)
            .expect("spare non-active connector");
        (idle, spare)
    });

    let (modes, edid) = handle.world_mut(|w| {
        let device = w.device.as_mock_mut().expect("the mock driver");
        let info = device.connector_info(spare).expect("spare info");
        (
            info.modes.clone(),
            vec![0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00],
        )
    });

    let frames_before = handle.frames();

    // Commit cycles across churn: connect the idle monitor, cycle,
    // disconnect it, cycle, unplug and replug the spare, cycle.
    handle.world_mut(|w| {
        w.device
            .as_mock_mut()
            .expect("the mock driver")
            .hotplug_connect(idle, modes.clone(), edid.clone());
    });
    cycle(&mut client, &surface, 1);
    handle.world_mut(|w| {
        w.device
            .as_mock_mut()
            .expect("the mock driver")
            .hotplug_disconnect(idle);
    });
    cycle(&mut client, &surface, 2);
    handle.world_mut(|w| {
        w.device
            .as_mock_mut()
            .expect("the mock driver")
            .hotplug_disconnect(spare);
    });
    cycle(&mut client, &surface, 3);
    handle.world_mut(|w| {
        w.device
            .as_mock_mut()
            .expect("the mock driver")
            .hotplug_connect(spare, modes, edid);
    });
    cycle(&mut client, &surface, 4);

    // The pipeline kept rendering throughout.
    std::thread::sleep(Duration::from_millis(50));
    let frames_after = handle.frames();
    assert!(
        frames_after > frames_before,
        "the compositor must keep rendering across hotplug churn \
         ({frames_before} -> {frames_after})"
    );

    // Clean teardown, then the canary on the post-churn server.
    client.destroy(&buffer);
    client.destroy(&surface);
    client.wait_until(|rec| rec.records.iter().any(|r| r.event == "destroyed"));
    client.destroy(&pool);
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if handle.world(|w| w.scene.routes.is_empty() && w.outboxes.is_empty()) {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    Client::canary_session(&handle);
}

#[cfg(test)]
mod tests {
    use super::*;

    // The shared cross-suite gate lives in the harness
    // (`harness::SUITE_GATE`): process-wide assertions serialize.

    #[test]
    fn the_hotplug_simulation() {
        let _guard = crate::harness::SUITE_GATE.lock().expect("hotplug gate");
        run_device_level();
        run_compositor_level();
    }
}
