//! The mock KMS device — a faithful, deterministic DRM in userspace.
//!
//! [`MockDevice`] implements [`crate::backend::KmsBackend`] over an
//! in-memory topology:
//! connectors with real EDID blobs and mode lists, CRTCs with VRR
//! windows and [`CrtcTimeline`]s, planes with `IN_FORMATS` capability
//! blobs, a global property registry (DRM properties are shared objects,
//! so `CRTC_ID` has one id across connectors *and* planes), blob and
//! framebuffer stores, and an injected clock.
//!
//! The atomic commit path lives in `validate`; this module owns
//! state and queries. Mock id namespaces: properties from 1000, blobs
//! from 2000, framebuffers from 3000, fence tokens from 1 — chosen far
//! apart so misrouted ids fail loudly in tests.
//!
//! Determinism: no clock reads, no hash-map iteration in output paths —
//! event order is timeline order, queries return resource-list order.

#![forbid(unsafe_code)]

use std::collections::HashMap;

use crate::atomic::AtomicRequest;
use crate::backend::{Blob, CommitOutcome};
use crate::connector::{ConnectorInfo, ConnectorStatus};
use crate::error::Result;
use crate::events::DeviceEvent;
use crate::fb::FbSpec;
use crate::ids::{AnyId, ConnectorId, CrtcId, FbId, PlaneId, PropId};
use crate::mode::Mode;
use crate::plane::PlaneInfo;
use crate::props::{prop, PropertyInfo};
use ldp_core::buffer::Modifier;
use ldp_core::time::Mono;

use super::timeline::CrtcTimeline;
use super::validate;

/// One connector's mutable state.
pub(crate) struct MockConnector {
    /// The immutable-ish snapshot (mutated by hotplug only).
    pub info: ConnectorInfo,
    /// The display engine's declared synthesis envelope (Phase 42):
    /// the maximum pixel clock a *user-defined* mode may carry
    /// through this connector's commit validation — the KMS layer's
    /// own gate under the mode foundry's doctrine (the `USERDEF`
    /// mode a pour mints is validated here, exactly as a real
    /// driver's atomic check validates user modes against its own
    /// limits — the second of the foundry's two gates; the EDID
    /// range limits are the first). `None`: this engine accepts no
    /// user-defined modes at all.
    pub synth_max_clock_khz: Option<u32>,
}

/// One CRTC's mutable state.
pub(crate) struct MockCrtc {
    /// Object id.
    pub id: CrtcId,
    /// The VRR window when the hardware supports adaptive sync.
    pub vrr_capable: Option<(u64, u64)>,
    /// ACTIVE prop value.
    pub active: bool,
    /// MODE_ID wire value.
    pub mode_blob: u64,
    /// VRR_ENABLED wire value.
    pub vrr_enabled: bool,
    /// Live timeline while active.
    pub timeline: Option<CrtcTimeline>,
}

/// One plane's mutable scanout state (the property values), as the
/// device models them. Public for test introspection through
/// [`crate::mock::MockDevice::plane_state`].
#[derive(Clone, Copy, Debug)]
pub struct PlaneState {
    /// CRTC_ID wire value.
    pub crtc: u64,
    /// FB_ID wire value.
    pub fb: u64,
    /// SRC_X 16.16.
    pub src_x: u64,
    /// SRC_Y 16.16.
    pub src_y: u64,
    /// SRC_W 16.16.
    pub src_w: u64,
    /// SRC_H 16.16.
    pub src_h: u64,
    /// CRTC_X.
    pub crtc_x: i64,
    /// CRTC_Y.
    pub crtc_y: i64,
    /// CRTC_W.
    pub crtc_w: u64,
    /// CRTC_H.
    pub crtc_h: u64,
    /// IN_FENCE_FD.
    pub in_fence: i64,
    /// ZPOS.
    pub zpos: u64,
}

impl Default for PlaneState {
    fn default() -> Self {
        Self {
            crtc: 0,
            fb: 0,
            src_x: 0,
            src_y: 0,
            src_w: 0,
            src_h: 0,
            crtc_x: 0,
            crtc_y: 0,
            crtc_w: 0,
            crtc_h: 0,
            in_fence: -1,
            zpos: 0,
        }
    }
}

/// One plane.
pub(crate) struct MockPlane {
    /// Static capabilities.
    pub info: PlaneInfo,
    /// Mutable scanout state.
    pub state: PlaneState,
}

/// The mock DRM device.
pub struct MockDevice {
    pub(crate) now: Mono,
    pub(crate) connectors: Vec<MockConnector>,
    pub(crate) crtcs: Vec<MockCrtc>,
    pub(crate) planes: Vec<MockPlane>,
    /// Global property registry (name → definition).
    pub(crate) prop_defs: Vec<PropertyInfo>,
    /// Per-object catalogs.
    pub(crate) catalogs: HashMap<AnyId, Vec<PropId>>,
    /// Per-object current values.
    pub(crate) values: HashMap<(AnyId, PropId), u64>,
    /// Blob store.
    pub(crate) blobs: Vec<Blob>,
    /// Framebuffer store: (id, spec, modifier).
    pub(crate) fbs: Vec<(FbId, FbSpec, Option<Modifier>)>,
    /// Imported GEM handles by originating fd (the PRIME object-identity
    /// mirror — see [`crate::backend::KmsBackend::import_gem`]).
    pub(crate) gems: HashMap<i32, u32>,
    /// The next GEM handle the mock mints.
    pub(crate) next_gem: u32,
    /// In-fence fds (mock namespace) → ready time.
    pub(crate) fences: HashMap<i64, Mono>,
    /// Out-fence tokens → (ready time, signaled).
    pub(crate) out_fences: HashMap<u64, (Mono, bool)>,
    /// Events waiting for the next drain (hotplug).
    pub(crate) queued: Vec<DeviceEvent>,
    pub(crate) next_prop: u32,
    pub(crate) next_blob: u32,
    pub(crate) next_fb: u32,
    pub(crate) next_token: u64,
}

impl MockDevice {
    /// The reference preset: a laptop panel + DP + HDMI, two CRTCs
    /// (CRTC #0 VRR-capable 48-144 Hz), primary/overlay/cursor planes.
    /// Construction details live in `preset`.
    #[must_use]
    pub fn laptop_dual() -> Self {
        super::preset::build()
    }

    /// The injected clock.
    #[must_use]
    pub fn now(&self) -> Mono {
        self.now
    }

    /// Advance the clock by `ns`, returning every event that became due
    /// (timeline order: CRTCs in resource order, events in time order
    /// within each).
    pub fn advance_ns(&mut self, ns: u64) -> Vec<DeviceEvent> {
        self.advance_to(self.now.saturating_add_ns(ns))
    }

    /// Advance the clock to `when` (monotonic; earlier times are a no-op).
    pub fn advance_to(&mut self, when: Mono) -> Vec<DeviceEvent> {
        let mut out = std::mem::take(&mut self.queued);
        if when > self.now {
            self.now = when;
        }
        let frozen = self.psr_frozen_crtcs();
        for crtc in &mut self.crtcs {
            // The sleeping panel (Phase 35): a CRTC whose connector is
            // in self-refresh does not advance — no vblanks, no flip
            // landings, no fences. The panel holds its GRAM; the
            // display engine is dark. Advancing time past a frozen
            // timeline is free (that is the point), and the first
            // event after the release is one full nominal later.
            if frozen.contains(&crtc.id) {
                continue;
            }
            if let Some(timeline) = crtc.timeline.as_mut() {
                out.extend(timeline.advance_to(self.now, crtc.id));
            }
        }
        // Signal out-fence tokens whose flips completed.
        let mut ready: Vec<u64> = Vec::new();
        for (token, (at, signaled)) in &self.out_fences {
            if !*signaled && *at <= self.now {
                ready.push(*token);
            }
        }
        for token in ready {
            if let Some(entry) = self.out_fences.get_mut(&token) {
                entry.1 = true;
            }
        }
        out
    }

    /// Register a mock in-fence fd that becomes ready at `ready`.
    pub fn add_in_fence(&mut self, fd: i64, ready: Mono) {
        self.fences.insert(fd, ready);
    }

    /// Whether an out-fence token has signaled.
    #[must_use]
    pub fn out_fence_ready(&self, token: u64) -> bool {
        self.out_fences.get(&token).is_some_and(|(_, s)| *s)
    }

    /// Simulate a monitor being plugged into a connector.
    pub fn hotplug_connect(&mut self, connector: ConnectorId, modes: Vec<Mode>, edid: Vec<u8>) {
        if let Some(conn) = self.connectors.iter_mut().find(|c| c.info.id == connector) {
            conn.info.status = ConnectorStatus::Connected;
            conn.info.modes = modes;
            conn.info.edid = Some(edid);
            self.queued
                .push(DeviceEvent::Hotplug(crate::events::HotplugEvent {
                    connector,
                }));
        }
    }

    /// Simulate a monitor being unplugged.
    pub fn hotplug_disconnect(&mut self, connector: ConnectorId) {
        if let Some(conn) = self.connectors.iter_mut().find(|c| c.info.id == connector) {
            conn.info.status = ConnectorStatus::Disconnected;
            conn.info.modes.clear();
            conn.info.edid = None;
            self.queued
                .push(DeviceEvent::Hotplug(crate::events::HotplugEvent {
                    connector,
                }));
        }
    }

    /// Which CRTC a connector is bound to (test helper).
    #[must_use]
    pub fn connector_binding(&self, connector: ConnectorId) -> Option<CrtcId> {
        let obj = AnyId::Connector(connector);
        let prop = self
            .prop_defs
            .iter()
            .find(|p| p.name.as_str() == prop::CRTC_ID)?;
        let value = self.values.get(&(obj, prop.id)).copied().unwrap_or(0);
        CrtcId::new(u32::try_from(value).ok()?)
    }

    /// A CRTC's current mode (test helper).
    #[must_use]
    pub fn crtc_mode_now(&self, crtc: CrtcId) -> Option<Mode> {
        let c = self.crtcs.iter().find(|c| c.id == crtc)?;
        if !c.active || c.mode_blob == 0 {
            return None;
        }
        let blob = self
            .blobs
            .iter()
            .find(|b| b.id.raw() as u64 == c.mode_blob)?;
        Mode::from_blob(&blob.data).ok()
    }

    /// A plane's live state (test helper).
    #[must_use]
    pub fn plane_state(&self, plane: PlaneId) -> Option<PlaneState> {
        self.planes
            .iter()
            .find(|p| p.info.id == plane)
            .map(|p| p.state)
    }

    /// The out-fence completion time for a token (test helper).
    #[must_use]
    pub fn out_fence_time(&self, token: u64) -> Option<Mono> {
        self.out_fences.get(&token).map(|(at, _)| *at)
    }

    /// Whether panel self-refresh is engaged on a connector (test
    /// introspection; the property-walk equivalent reads
    /// `PANEL_SELF_REFRESH`). The Phase 35 sleeping-panel state.
    #[must_use]
    pub fn psr_enabled(&self, connector: ConnectorId) -> bool {
        let obj = AnyId::Connector(connector);
        self.prop_defs
            .iter()
            .find(|p| p.name.as_str() == prop::PANEL_SELF_REFRESH)
            .and_then(|p| self.values.get(&(obj, p.id)).copied())
            .unwrap_or(0)
            != 0
    }

    /// The CRTCs whose driven connector is in self-refresh — the
    /// frozen set. A CRTC qualifies when any connector bound to it
    /// carries the PSR property on (a panel that went to sleep takes
    /// its scan pipeline with it).
    pub(super) fn psr_frozen_crtcs(&self) -> Vec<CrtcId> {
        let Some(crtc_prop) = self
            .prop_defs
            .iter()
            .find(|p| p.name.as_str() == prop::CRTC_ID)
            .map(|p| p.id)
        else {
            return Vec::new();
        };
        let Some(psr_prop) = self
            .prop_defs
            .iter()
            .find(|p| p.name.as_str() == prop::PANEL_SELF_REFRESH)
            .map(|p| p.id)
        else {
            return Vec::new();
        };
        let mut frozen = Vec::new();
        for conn in &self.connectors {
            let obj = AnyId::Connector(conn.info.id);
            let bound = self.values.get(&(obj, crtc_prop)).copied().unwrap_or(0);
            let psr = self.values.get(&(obj, psr_prop)).copied().unwrap_or(0);
            if psr != 0 && bound != 0 {
                if let Ok(id) = u32::try_from(bound) {
                    if let Some(crtc) = CrtcId::new(id) {
                        frozen.push(crtc);
                    }
                }
            }
        }
        frozen
    }

    /// Whether VRR is currently enabled on a CRTC (test
    /// introspection; the property-walk equivalent reads
    /// `VRR_ENABLED`).
    #[must_use]
    pub fn vrr_enabled(&self, crtc: CrtcId) -> bool {
        self.crtcs
            .iter()
            .find(|c| c.id == crtc)
            .is_some_and(|c| c.vrr_enabled)
    }

    /// A CRTC's adaptive-sync window in nanoseconds `(min, max)` —
    /// the `vrr_capable` fixture value (test introspection; embedders
    /// like the Phase-10 compositor read it to fill `output.vrr`).
    #[must_use]
    pub fn vrr_window(&self, crtc: CrtcId) -> Option<(u64, u64)> {
        self.crtcs
            .iter()
            .find(|c| c.id == crtc)
            .and_then(|c| c.vrr_capable)
    }

    /// Flips completed on a CRTC so far (test introspection).
    #[must_use]
    pub fn flip_count(&self, crtc: CrtcId) -> u64 {
        self.crtcs
            .iter()
            .find(|c| c.id == crtc)
            .and_then(|c| c.timeline.as_ref())
            .map_or(0, CrtcTimeline::flip_seq)
    }

    /// The CRTC's nominal frame period in nanoseconds, from the active
    /// timeline (test introspection).
    #[must_use]
    pub fn frame_period_ns(&self, crtc: CrtcId) -> Option<u64> {
        self.crtcs
            .iter()
            .find(|c| c.id == crtc)
            .and_then(|c| c.timeline.as_ref())
            .map(CrtcTimeline::nominal)
    }

    /// The earliest time any CRTC will emit its next event (flip,
    /// fence release, or idle vblank); `None` when no timeline is
    /// live. Step tests advance exactly to this point.
    #[must_use]
    pub fn next_event_at(&self) -> Option<Mono> {
        let frozen = self.psr_frozen_crtcs();
        self.crtcs
            .iter()
            .filter(|c| !frozen.contains(&c.id))
            .filter_map(|c| c.timeline.as_ref().map(CrtcTimeline::next_event_at))
            .min()
    }

    /// Number of vblanks emitted for a CRTC (test helper).
    #[must_use]
    pub fn vblank_count(&self, crtc: CrtcId) -> u64 {
        self.crtcs
            .iter()
            .find(|c| c.id == crtc)
            .and_then(|c| c.timeline.as_ref())
            .map_or(0, CrtcTimeline::vblank_seq)
    }

    /// The pending-flip count on a CRTC (test helper).
    #[must_use]
    pub fn pending_flips(&self, crtc: CrtcId) -> usize {
        self.crtcs
            .iter()
            .find(|c| c.id == crtc)
            .and_then(|c| c.timeline.as_ref())
            .map_or(0, CrtcTimeline::pending_count)
    }

    /// Commit entry point (validation + application in [`validate`]).
    pub(crate) fn commit_request(&mut self, request: &AtomicRequest) -> Result<CommitOutcome> {
        validate::commit(self, request)
    }
}

impl core::fmt::Debug for MockDevice {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("MockDevice")
            .field("now", &self.now)
            .field(
                "connectors",
                &self
                    .connectors
                    .iter()
                    .map(|c| c.info.id.raw())
                    .collect::<Vec<_>>(),
            )
            .field(
                "crtcs",
                &self.crtcs.iter().map(|c| c.id.raw()).collect::<Vec<_>>(),
            )
            .field(
                "planes",
                &self
                    .planes
                    .iter()
                    .map(|p| p.info.id.raw())
                    .collect::<Vec<_>>(),
            )
            .finish_non_exhaustive()
    }
}
