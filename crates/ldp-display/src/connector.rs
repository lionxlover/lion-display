//! Connector model — the plug on the outside of the machine.
//!
//! A [`ConnectorInfo`] snapshot answers: what kind of plug is it
//! ([`ConnectorType`] + per-type index), is something plugged in
//! ([`ConnectorStatus`]), how big is the panel in millimetres, what
//! subpixel geometry does it have, which encoders can drive it, which
//! modes does it offer, and what is the monitor's identity (`EDID` blob).
//!
//! Type names follow the kernel naming (`eDP-1`, `HDMI-A-1`, `DP-2`) so
//! logs match `modetest` output exactly.

#![forbid(unsafe_code)]

use crate::edid::EdidIdentity;
use crate::ids::{ConnectorId, CrtcId, EncoderId};
use crate::mode::Mode;

/// The physical connector kind (`DRM_MODE_CONNECTOR_*`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum ConnectorType {
    /// Analog VGA.
    Vga,
    /// DVI-I (digital + analog).
    DviI,
    /// DVI-D (digital only).
    DviD,
    /// DVI-A (analog only).
    DviA,
    /// Composite video.
    Composite,
    /// S-Video.
    SVideo,
    /// Laptop internal LVDS panel.
    Lvds,
    /// Component video.
    Component,
    /// DisplayPort.
    DisplayPort,
    /// HDMI type A.
    HdmiA,
    /// HDMI type B (dual-link).
    HdmiB,
    /// Embedded DisplayPort.
    EmbeddedDisplayPort,
    /// Virtual/software connector.
    Virtual,
    /// Display Serial Interface.
    Dsi,
    /// Display Parallel Interface.
    Dpi,
    /// USB display.
    Usb,
    /// Any newer kernel connector type this build does not name.
    Other(u32),
}

impl ConnectorType {
    /// Map the kernel constant; unknown codes become [`Self::Other`].
    #[must_use]
    pub const fn from_kernel(code: u32) -> Self {
        match code {
            1 => Self::Vga,
            2 => Self::DviI,
            3 => Self::DviD,
            4 => Self::DviA,
            5 => Self::Composite,
            6 => Self::SVideo,
            7 => Self::Lvds,
            8 => Self::Component,
            10 => Self::DisplayPort,
            11 => Self::HdmiA,
            12 => Self::HdmiB,
            14 => Self::EmbeddedDisplayPort,
            15 => Self::Virtual,
            16 => Self::Dsi,
            17 => Self::Dpi,
            20 => Self::Usb,
            other => Self::Other(other),
        }
    }

    /// The kernel constant.
    #[must_use]
    pub const fn kernel(self) -> u32 {
        match self {
            Self::Vga => 1,
            Self::DviI => 2,
            Self::DviD => 3,
            Self::DviA => 4,
            Self::Composite => 5,
            Self::SVideo => 6,
            Self::Lvds => 7,
            Self::Component => 8,
            Self::DisplayPort => 10,
            Self::HdmiA => 11,
            Self::HdmiB => 12,
            Self::EmbeddedDisplayPort => 14,
            Self::Virtual => 15,
            Self::Dsi => 16,
            Self::Dpi => 17,
            Self::Usb => 20,
            Self::Other(code) => code,
        }
    }

    /// The kernel prefix (`"HDMI-A"`, `"eDP"`, `"DP"`, ...).
    #[must_use]
    pub const fn prefix(self) -> &'static str {
        match self {
            Self::Vga => "VGA",
            Self::DviI => "DVI-I",
            Self::DviD => "DVI-D",
            Self::DviA => "DVI-A",
            Self::Composite => "Composite",
            Self::SVideo => "SVIDEO",
            Self::Lvds => "LVDS",
            Self::Component => "Component",
            Self::DisplayPort => "DP",
            Self::HdmiA => "HDMI-A",
            Self::HdmiB => "HDMI-B",
            Self::EmbeddedDisplayPort => "eDP",
            Self::Virtual => "Virtual",
            Self::Dsi => "DSI",
            Self::Dpi => "DPI",
            Self::Usb => "USB",
            // Unknown types print as UNKNOWN-n (kernel uses "UNKNOWN").
            Self::Other(_) => "UNKNOWN",
        }
    }

    /// Full connector name with its per-type index, kernel style
    /// (`"eDP-1"`, `"HDMI-A-1"`).
    #[must_use]
    pub fn full_name(self, index: u32) -> String {
        format!("{}-{}", self.prefix(), index)
    }
}

/// Cable/connection state (`DRM_MODE_CONNECTED_*`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum ConnectorStatus {
    /// A sink is attached and modes are available.
    Connected,
    /// Nothing is attached.
    Disconnected,
    /// The driver cannot tell (DPMS off, runtime PM, error).
    Unknown,
}

impl ConnectorStatus {
    /// Kernel constant.
    #[must_use]
    pub const fn kernel(self) -> u32 {
        match self {
            Self::Connected => 1,
            Self::Disconnected => 2,
            Self::Unknown => 3,
        }
    }

    /// From kernel constant.
    #[must_use]
    pub const fn from_kernel(code: u32) -> Self {
        match code {
            1 => Self::Connected,
            2 => Self::Disconnected,
            _ => Self::Unknown,
        }
    }
}

/// Panel subpixel arrangement (`DRM_MODE_SUBPIXEL_*`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum Subpixel {
    /// Unknown.
    Unknown,
    /// Horizontal RGB stripes.
    HorizontalRgb,
    /// Horizontal BGR stripes.
    HorizontalBgr,
    /// Vertical RGB stripes.
    VerticalRgb,
    /// Vertical BGR stripes.
    VerticalBgr,
    /// Not applicable (projector, virtual).
    None,
}

impl Subpixel {
    /// Kernel constant.
    #[must_use]
    pub const fn kernel(self) -> u32 {
        match self {
            Self::Unknown => 0,
            Self::HorizontalRgb => 1,
            Self::HorizontalBgr => 2,
            Self::VerticalRgb => 3,
            Self::VerticalBgr => 4,
            Self::None => 5,
        }
    }

    /// From kernel constant.
    #[must_use]
    pub const fn from_kernel(code: u32) -> Self {
        match code {
            1 => Self::HorizontalRgb,
            2 => Self::HorizontalBgr,
            3 => Self::VerticalRgb,
            4 => Self::VerticalBgr,
            5 => Self::None,
            _ => Self::Unknown,
        }
    }
}

/// One connector snapshot.
#[derive(Clone, Debug)]
pub struct ConnectorInfo {
    /// Object id.
    pub id: ConnectorId,
    /// Connector kind.
    pub kind: ConnectorType,
    /// Per-type index (the `-1` in `eDP-1`).
    pub type_index: u32,
    /// Connection state.
    pub status: ConnectorStatus,
    /// Panel physical width, millimetres (0 when unknown).
    pub mm_width: u32,
    /// Panel physical height, millimetres (0 when unknown).
    pub mm_height: u32,
    /// Subpixel arrangement.
    pub subpixel: Subpixel,
    /// Encoders able to drive this connector.
    pub encoders: Vec<EncoderId>,
    /// CRTC currently driving this connector, if any.
    pub current_crtc: Option<CrtcId>,
    /// Modes offered by the sink (empty when disconnected).
    pub modes: Vec<Mode>,
    /// EDID base block bytes when the sink supplies one.
    pub edid: Option<Vec<u8>>,
}

impl ConnectorInfo {
    /// Parsed monitor identity, when an EDID is present and valid.
    #[must_use]
    pub fn identity(&self) -> Option<EdidIdentity> {
        EdidIdentity::parse(self.edid.as_deref()?).ok()
    }

    /// Kernel-style full name (`eDP-1`).
    #[must_use]
    pub fn full_name(&self) -> String {
        self.kind.full_name(self.type_index)
    }

    /// The highest-refresh mode matching a preferred-mode search: the
    /// preferred flag wins; ties break toward the larger active area,
    /// then the higher refresh. `None` when no modes exist.
    #[must_use]
    pub fn best_mode(&self) -> Option<&Mode> {
        self.modes
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| {
                let ka = (
                    a.is_preferred(),
                    u64::from(a.hdisplay) * u64::from(a.vdisplay),
                    a.refresh_millihz(),
                );
                let kb = (
                    b.is_preferred(),
                    u64::from(b.hdisplay) * u64::from(b.vdisplay),
                    b.refresh_millihz(),
                );
                ka.cmp(&kb)
            })
            .map(|(_, m)| m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mode::ModeType;

    fn conn() -> ConnectorInfo {
        ConnectorInfo {
            id: ConnectorId::new(91).unwrap(),
            kind: ConnectorType::EmbeddedDisplayPort,
            type_index: 1,
            status: ConnectorStatus::Connected,
            mm_width: 290,
            mm_height: 190,
            subpixel: Subpixel::HorizontalRgb,
            encoders: vec![EncoderId::new(50).unwrap()],
            current_crtc: None,
            modes: vec![Mode::panel_1080p_59_94(), Mode::panel_1080p60()],
            edid: Some(EdidIdentity::synthesize("LTI", 7, 8, "Lion", 2026).to_vec()),
        }
    }

    #[test]
    fn naming_matches_kernel() {
        assert_eq!(ConnectorType::EmbeddedDisplayPort.full_name(1), "eDP-1");
        assert_eq!(ConnectorType::HdmiA.full_name(2), "HDMI-A-2");
        assert_eq!(ConnectorType::DisplayPort.full_name(3), "DP-3");
        assert_eq!(ConnectorType::Other(99).full_name(1), "UNKNOWN-1");
    }

    #[test]
    fn kernel_round_trips() {
        for code in [1u32, 2, 3, 4, 5, 6, 7, 8, 10, 11, 12, 14, 15, 16, 17, 20] {
            assert_eq!(ConnectorType::from_kernel(code).kernel(), code);
        }
        assert_eq!(
            ConnectorType::from_kernel(0xF00D),
            ConnectorType::Other(0xF00D)
        );
        for code in [1u32, 2, 3] {
            assert_eq!(ConnectorStatus::from_kernel(code).kernel(), code);
        }
    }

    #[test]
    fn identity_parses_from_snapshot() {
        let c = conn();
        let id = c.identity().unwrap();
        assert_eq!(id.manufacturer, "LTI");
        assert_eq!(id.monitor_name, "Lion");
        assert_eq!(c.full_name(), "eDP-1");
    }

    #[test]
    fn best_mode_prefers_flag_then_area_then_refresh() {
        let c = conn();
        // 1080p60 has the PREFERRED flag; 59.94 does not.
        assert_eq!(c.best_mode().unwrap().refresh_millihz(), 60_000);
        // Without flags, larger area wins.
        let mut c2 = conn();
        for m in &mut c2.modes {
            m.kind = ModeType::DRIVER;
        }
        assert_eq!(c2.best_mode().unwrap().refresh_millihz(), 60_000);
        // No modes -> None.
        c2.modes.clear();
        assert!(c2.best_mode().is_none());
    }
}
