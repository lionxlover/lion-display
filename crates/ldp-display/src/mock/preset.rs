//! The reference mock topology builder.
//!
//! [`build`] assembles the laptop-dual preset:
//!
//! * `eDP-1` (id 91): connected internal panel, 1920×1080@60 preferred
//!   + 59.94, 309×174 mm, RGB subpixel, EDID `LTI/0x3107`.
//! * `DP-1` (id 92): disconnected.
//! * `HDMI-A-1` (id 93): connected external monitor, 1080p60 + 720p60,
//!   EDID `BNQ/0x7D12`.
//! * CRTCs 42 (VRR-capable 48–144 Hz) and 43 (fixed sync).
//! * Planes: primaries 50/53, overlays 51/52/54, ARGB cursor 55 — all
//!   bound to their CRTC's `possible_crtcs` bit, with `IN_FORMATS`
//!   capability blobs.
//!
//! Object ids are fixed constants so golden tests address them directly.

#![forbid(unsafe_code)]

use std::collections::HashMap;

use crate::connector::{ConnectorStatus, ConnectorType, Subpixel};
use crate::edid::{EdidIdentity, RangeLimits};
use crate::ids::{ConnectorId, CrtcId, EncoderId, PlaneId};
use crate::mode::{Mode, ModeFlags, ModeType};
use crate::plane::{FormatModifier, InFormats, PlaneType};

use super::device::{MockConnector, MockCrtc, MockDevice, MockPlane, PlaneState};
use ldp_core::buffer::{FourCC, Modifier};
use ldp_core::time::Mono;

/// Build the preset (see the module docs for the topology).
pub(super) fn build() -> MockDevice {
    let mut dev = MockDevice::empty();
    add_connectors(&mut dev);
    add_crtcs(&mut dev);
    add_planes(&mut dev);
    dev.build_catalogs();
    dev
}

/// The three connectors: the internal panel, an empty DP, an external
/// HDMI monitor.
///
/// Phase 42 grows the preset's EDIDs into the *timing* truth: the
/// panel's block carries its 1080p60 preferred timing and a declared
/// 600 MHz range ceiling (a modern DP-class envelope); the HDMI
/// monitor's carries a **stale 720p preferred timing** — the
/// `edid-preferred-lie` fixture, firmware that never caught up with
/// the panel's 1080p capability — and the 340 MHz ceiling of the
/// HDMI 1.4 era. The synthesis envelopes mirror the ceilings: the
/// eDP engine pours up to 600 MHz, the HDMI engine up to 340 MHz
/// (a 4K60 foundry pour serves on the panel and is refused on the
/// monitor — the honest ceiling doctrine, both gates).
fn add_connectors(dev: &mut MockDevice) {
    let panel_edid = EdidIdentity::synthesize_detailed(
        "LTI",
        0x3107,
        0x1122_3344,
        "Lion Panel 15",
        2026,
        Some(&Mode::panel_1080p60()),
        Some(RangeLimits {
            vrate_hz: (56, 75),
            hrate_khz: (30, 83),
            max_clock_khz: 600_000,
        }),
    );
    add_connector(
        dev,
        ConnectorType::EmbeddedDisplayPort,
        1,
        ConnectorStatus::Connected,
        309,
        174,
        Subpixel::HorizontalRgb,
        vec![Mode::panel_1080p60(), Mode::panel_1080p_59_94()],
        panel_edid.to_vec(),
        Some(600_000),
    );
    add_connector(
        dev,
        ConnectorType::DisplayPort,
        1,
        ConnectorStatus::Disconnected,
        0,
        0,
        Subpixel::Unknown,
        vec![],
        Vec::new(),
        None,
    );
    // The stale-firmware fixture: the EDID prefers 720p while the
    // connector enumerates 1080p — the audit's `edid-preferred-lie`
    // fires on the default dual-screen boot (the honest diagnosis,
    // printed at bring-up).
    let hdmi_720p = Mode::new(
        74_250,
        1280,
        1390,
        1430,
        1650,
        720,
        725,
        730,
        750,
        ModeFlags::PVSYNC | ModeFlags::NHSYNC,
        ModeType::DRIVER,
    );
    let hdmi_edid = EdidIdentity::synthesize_detailed(
        "BNQ",
        0x7D12,
        0x5566_7788,
        "Lion External",
        2025,
        Some(&hdmi_720p),
        Some(RangeLimits {
            vrate_hz: (56, 75),
            hrate_khz: (15, 83),
            max_clock_khz: 340_000,
        }),
    );
    add_connector(
        dev,
        ConnectorType::HdmiA,
        1,
        ConnectorStatus::Connected,
        510,
        290,
        Subpixel::HorizontalRgb,
        vec![Mode::panel_1080p60(), hdmi_720p],
        hdmi_edid.to_vec(),
        Some(340_000),
    );
}

/// The two CRTCs: #42 VRR-capable 48-144 Hz, #43 fixed sync.
fn add_crtcs(dev: &mut MockDevice) {
    // VRR window 48-144 Hz on CRTC 42 (the panel's adaptive-sync range).
    let vrr = (1_000_000_000 / 144, 1_000_000_000 / 48);
    dev.crtcs.push(MockCrtc {
        id: CrtcId::new(42).unwrap(),
        vrr_capable: Some(vrr),
        active: false,
        mode_blob: 0,
        vrr_enabled: false,
        timeline: None,
    });
    dev.crtcs.push(MockCrtc {
        id: CrtcId::new(43).unwrap(),
        vrr_capable: None,
        active: false,
        mode_blob: 0,
        vrr_enabled: false,
        timeline: None,
    });
}

/// The six planes: primaries, overlays, and the ARGB cursor.
fn add_planes(dev: &mut MockDevice) {
    let rgb_caps = vec![
        FormatModifier {
            format: FourCC::XRGB8888,
            modifier: Modifier::LINEAR,
        },
        FormatModifier {
            format: FourCC::XRGB8888,
            modifier: Modifier::INTEL_X,
        },
        FormatModifier {
            format: FourCC::ARGB8888,
            modifier: Modifier::LINEAR,
        },
        FormatModifier {
            format: FourCC::NV12,
            modifier: Modifier::LINEAR,
        },
    ];
    add_plane(
        dev,
        PlaneId::new(50).unwrap(),
        PlaneType::Primary,
        0,
        &rgb_caps,
        0,
    );
    add_plane(
        dev,
        PlaneId::new(51).unwrap(),
        PlaneType::Overlay,
        0,
        &rgb_caps,
        1,
    );
    add_plane(
        dev,
        PlaneId::new(52).unwrap(),
        PlaneType::Overlay,
        0,
        &rgb_caps,
        2,
    );
    add_plane(
        dev,
        PlaneId::new(53).unwrap(),
        PlaneType::Primary,
        1,
        &[
            FormatModifier {
                format: FourCC::XRGB8888,
                modifier: Modifier::LINEAR,
            },
            FormatModifier {
                format: FourCC::ARGB8888,
                modifier: Modifier::LINEAR,
            },
        ],
        0,
    );
    add_plane(
        dev,
        PlaneId::new(54).unwrap(),
        PlaneType::Overlay,
        1,
        &[FormatModifier {
            format: FourCC::XRGB8888,
            modifier: Modifier::LINEAR,
        }],
        1,
    );
    add_plane(
        dev,
        PlaneId::new(55).unwrap(),
        PlaneType::Cursor,
        0,
        &[FormatModifier {
            format: FourCC::ARGB8888,
            modifier: Modifier::LINEAR,
        }],
        0,
    );
}

impl MockDevice {
    /// An empty device with id allocators ready.
    pub(super) fn empty() -> Self {
        Self {
            now: Mono::ZERO,
            connectors: Vec::new(),
            crtcs: Vec::new(),
            planes: Vec::new(),
            prop_defs: Vec::new(),
            catalogs: HashMap::new(),
            values: HashMap::new(),
            blobs: Vec::new(),
            fbs: Vec::new(),
            gems: HashMap::new(),
            next_gem: 5000,
            fences: HashMap::new(),
            out_fences: HashMap::new(),
            queued: Vec::new(),
            next_prop: 1000,
            next_blob: 2000,
            next_fb: 3000,
            next_token: 1,
        }
    }
}

/// The ten parameters are the connector snapshot's own fields plus
/// the synthesis envelope; the builder is the kernel's
/// `drmModeGetConnector` mirror.
#[allow(clippy::too_many_arguments)]
fn add_connector(
    dev: &mut MockDevice,
    kind: ConnectorType,
    type_index: u32,
    status: ConnectorStatus,
    mm_w: u32,
    mm_h: u32,
    subpixel: Subpixel,
    modes: Vec<Mode>,
    edid: Vec<u8>,
    synth_max_clock_khz: Option<u32>,
) {
    let id = ConnectorId::new(90 + u32::try_from(dev.connectors.len()).unwrap() + 1).unwrap();
    dev.connectors.push(MockConnector {
        info: crate::connector::ConnectorInfo {
            id,
            kind,
            type_index,
            status,
            mm_width: mm_w,
            mm_height: mm_h,
            subpixel,
            encoders: vec![EncoderId::new(50 + type_index).unwrap()],
            current_crtc: None,
            modes,
            edid: if edid.is_empty() { None } else { Some(edid) },
        },
        synth_max_clock_khz,
    });
}

fn add_plane(
    dev: &mut MockDevice,
    id: PlaneId,
    kind: PlaneType,
    crtc_index: u32,
    caps: &[FormatModifier],
    default_zpos: u64,
) {
    let in_formats = Some(InFormats::parse(&InFormats::encode(caps)).unwrap());
    let mut formats: Vec<FourCC> = caps.iter().map(|c| c.format).collect();
    formats.sort_by_key(|f| f.code());
    formats.dedup();
    let mut info = crate::plane::PlaneInfo {
        id,
        kind,
        possible_crtcs: crate::ids::CrtcMask::covering(2),
        current_crtc: None,
        formats,
        in_formats,
    };
    // Only the plane's own CRTC index is possible.
    info.possible_crtcs = crate::ids::CrtcMask::from_raw(1 << crtc_index);
    dev.planes.push(MockPlane {
        info,
        state: PlaneState {
            zpos: default_zpos,
            ..PlaneState::default()
        },
    });
}
