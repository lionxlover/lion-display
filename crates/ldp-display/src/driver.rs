//! The display driver seam — the serve loop's device contract.
//!
//! [`KmsBackend`] answers *state* (topology, properties, commits,
//! events); a serve loop also needs *time* and *waiting*: the mock
//! device advances an injected clock at wake points, the real device
//! hands out kernel timestamps and lands its events on the DRM fd.
//! [`DisplayDriver`] is that union — the wake-point integration the
//! Phase 24 rehearsal named as Phase 25's core.
//!
//! * [`MockDevice`] (the CI vehicle): [`DisplayDriver::wait_events`]
//!   advances the deterministic clock exactly to the next due event —
//!   the headless time doctrine, unchanged; a zero timeout drains only
//!   what is already due (hotplug mail), never moving the clock.
//! * [`DrmDriver`] (the real device): `poll(2)` on the DRM fd for at
//!   most the timeout, then `try_events` — the kernel's page-flip
//!   events arrive on the descriptor the compositor already owns. A
//!   `None` timeout waits for the next event (the pending flip's
//!   landing); `Some(Duration::ZERO)` polls without blocking.
//!
//! Both drivers' `now()` share the `Mono` domain with event
//! timestamps, so scheduler math stays backend-independent.

#![forbid(unsafe_code)]

use core::time::Duration;

use crate::backend::KmsBackend;
use crate::drm::sys;
use crate::drm::DrmBackend;
use crate::error::Result;
use crate::events::DeviceEvent;
use crate::MockDevice;
use ldp_core::time::Mono;

/// A device the serve loop can drive: a KMS backend plus its time and
/// wait surface. Object-safe; `Send` because the world crosses session
/// threads under its mutex.
pub trait DisplayDriver: KmsBackend + Send {
    /// The device's current time (mock: the injected clock; real: the
    /// kernel's CLOCK_MONOTONIC — the same domain as flip events).
    fn now(&self) -> Mono;

    /// Wait for device events, at most `timeout` (`None` = until the
    /// next event). Never spins: the mock advances its clock exactly
    /// to the next due event; the real driver blocks in `poll(2)`.
    /// Returns empty when nothing became due within the timeout.
    ///
    /// # Errors
    /// [`crate::error::DisplayError::System`] when the event read or
    /// the poll itself fails.
    fn wait_events(&mut self, timeout: Option<Duration>) -> Result<Vec<DeviceEvent>>;

    /// The real-device view, when this driver is a [`DrmDriver`] — the
    /// serve loop polls its fd; teardown borrows its backend. `None`
    /// on the mock.
    #[must_use]
    fn as_drm(&self) -> Option<&DrmDriver> {
        None
    }

    /// The mutable real-device view.
    fn as_drm_mut(&mut self) -> Option<&mut DrmDriver> {
        None
    }

    /// The mock-device view, when this driver is a [`MockDevice`] —
    /// the test suites' introspection (applied-state assertions,
    /// hotplug injection). `None` on the real driver.
    #[must_use]
    fn as_mock(&self) -> Option<&MockDevice> {
        None
    }

    /// The mutable mock-device view.
    fn as_mock_mut(&mut self) -> Option<&mut MockDevice> {
        None
    }
}

impl DisplayDriver for MockDevice {
    fn now(&self) -> Mono {
        MockDevice::now(self)
    }

    fn wait_events(&mut self, timeout: Option<Duration>) -> Result<Vec<DeviceEvent>> {
        // The headless doctrine: the clock advances only at wake
        // points — exactly to the next due event, never past it, and
        // never in idle drift. A zero timeout means "what is already
        // due": draining the queued mail (hotplug) without moving the
        // clock at all.
        match timeout {
            Some(Duration::ZERO) => Ok(self.advance_to(self.now())),
            _ => match self.next_event_at() {
                Some(at) => Ok(self.advance_to(at)),
                None => Ok(Vec::new()),
            },
        }
    }

    fn as_mock(&self) -> Option<&MockDevice> {
        Some(self)
    }

    fn as_mock_mut(&mut self) -> Option<&mut MockDevice> {
        Some(self)
    }
}

/// The real device behind the serve loop: a [`DrmBackend`] plus its
/// poll-based wait. Constructed after bring-up (master, dumb buffers,
/// framebuffers, mappings are DrmBackend methods the caller used
/// directly); everything after that flows through the driver.
pub struct DrmDriver {
    backend: DrmBackend,
}

impl DrmDriver {
    /// Wrap a brought-up backend.
    #[must_use]
    pub fn new(backend: DrmBackend) -> Self {
        Self { backend }
    }

    /// The wrapped backend (teardown's dumb-buffer/master vocabulary).
    #[must_use]
    pub fn backend(&self) -> &DrmBackend {
        &self.backend
    }

    /// The wrapped backend, mutably.
    #[must_use]
    pub fn backend_mut(&mut self) -> &mut DrmBackend {
        &mut self.backend
    }

    /// The DRM descriptor (the compositor's accept loop polls it
    /// alongside the listening socket).
    #[must_use]
    pub fn fd(&self) -> i32 {
        self.backend.fd()
    }
}

impl KmsBackend for DrmDriver {
    fn version(&self) -> Result<crate::backend::DeviceVersion> {
        self.backend.version()
    }
    fn topology(&self) -> Result<crate::backend::Topology> {
        self.backend.topology()
    }
    fn connector_info(
        &self,
        connector: crate::ids::ConnectorId,
    ) -> Result<crate::connector::ConnectorInfo> {
        self.backend.connector_info(connector)
    }
    fn crtc_info(&self, crtc: crate::ids::CrtcId) -> Result<crate::backend::CrtcInfo> {
        self.backend.crtc_info(crtc)
    }
    fn plane_info(&self, plane: crate::ids::PlaneId) -> Result<crate::plane::PlaneInfo> {
        self.backend.plane_info(plane)
    }
    fn property(&self, prop: crate::ids::PropId) -> Result<crate::props::PropertyInfo> {
        self.backend.property(prop)
    }
    fn object_properties(&self, obj: crate::ids::AnyId) -> Result<crate::props::ObjectProperties> {
        self.backend.object_properties(obj)
    }
    fn blob(&self, blob: crate::ids::BlobId) -> Result<Vec<u8>> {
        self.backend.blob(blob)
    }
    fn create_blob(&mut self, data: &[u8]) -> Result<crate::backend::Blob> {
        self.backend.create_blob(data)
    }
    fn destroy_blob(&mut self, blob: crate::ids::BlobId) -> Result<()> {
        self.backend.destroy_blob(blob)
    }
    fn add_fb(
        &mut self,
        spec: &crate::fb::FbSpec,
        modifier: Option<ldp_core::buffer::Modifier>,
    ) -> Result<crate::ids::FbId> {
        self.backend.add_fb(spec, modifier)
    }

    fn import_gem(&mut self, fd: i32) -> Result<u32> {
        self.backend.import_gem(fd)
    }
    fn rm_fb(&mut self, fb: crate::ids::FbId) -> Result<()> {
        self.backend.rm_fb(fb)
    }
    fn commit(
        &mut self,
        request: &crate::atomic::AtomicRequest,
    ) -> Result<crate::backend::CommitOutcome> {
        self.backend.commit(request)
    }
    fn try_events(&mut self) -> Result<Vec<DeviceEvent>> {
        self.backend.try_events()
    }
}

impl DisplayDriver for DrmDriver {
    fn now(&self) -> Mono {
        // The kernel's flip timestamps are CLOCK_MONOTONIC; the
        // scheduler reasons in the same domain. A failure (kernel
        // contract violation) surfaces as the zero instant — the
        // scheduler degrades to ordering-only, never crashes.
        sys::monotonic_now().unwrap_or(Mono::ZERO)
    }

    fn wait_events(&mut self, timeout: Option<Duration>) -> Result<Vec<DeviceEvent>> {
        let ms = match timeout {
            // Round up so a sub-millisecond timeout still gets its
            // poll turn; None waits for the event itself.
            Some(t) => i32::try_from(t.as_millis().max(1)).unwrap_or(i32::MAX),
            None => -1,
        };
        if sys::poll_readable(self.backend.fd(), ms)? {
            self.try_events()
        } else {
            Ok(Vec::new())
        }
    }

    fn as_drm(&self) -> Option<&DrmDriver> {
        Some(self)
    }

    fn as_drm_mut(&mut self) -> Option<&mut DrmDriver> {
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_wait_advances_exactly_one_event_quanta() {
        let mut dev = MockDevice::laptop_dual();
        // Bring the CRTC's timeline up: register a framebuffer, enable
        // through the serve choreography (one applied commit), then wait.
        let pipeline = crate::serve::select_pipeline(&dev).expect("eDP-1 present");
        let (w, h) = (pipeline.width(), pipeline.height());
        let fb = dev
            .add_fb(
                &crate::fb::FbSpec::single(w, h, ldp_core::buffer::FourCC::XRGB8888, 1, w * 4, 0),
                Some(ldp_core::buffer::Modifier::LINEAR),
            )
            .expect("FB registers");
        crate::serve::enable(&mut dev, &pipeline, fb).expect("enable applies");
        // A blocking wait lands the enable flip at the first vblank.
        let events = dev.wait_events(None).expect("wait");
        assert!(
            events.iter().any(|e| matches!(e, DeviceEvent::PageFlip(_))),
            "the bring-up flip lands: {events:?}"
        );
        // A zero-timeout wait right after: nothing new is due (the
        // clock has not moved), so nothing arrives.
        let quiet = dev.wait_events(Some(Duration::ZERO)).expect("drain");
        assert!(quiet.is_empty(), "nothing due at the standing clock");
    }
}
