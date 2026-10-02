//! Display modes — the DRM `drmModeModeInfo` model, pure.
//!
//! A [`Mode`] is the full CRTC scanout timing: pixel clock in kHz, the
//! horizontal and vertical blanking choreography, interlace/doublescan
//! flags, and the preferred/driver type bits. Everything the frame
//! scheduler needs is derived *exactly* from these fields —
//! [`Mode::refresh_millihz`] and [`Mode::period`] are computed from the
//! pixel clock and totals, not from the kernel's pre-rounded `vrefresh`
//! field (59.94 Hz modes stay distinguishable from 60.000 Hz modes).
//!
//! The binary blob encoding is the kernel `drmModeModeInfo` wire layout:
//! little-endian, fixed 68-byte struct. Mode blobs are what the
//! `MODE_ID` CRTC property points at.

#![forbid(unsafe_code)]

use crate::error::{DisplayError, Result};
use ldp_core::time::RefreshInterval;

/// Mode flag bits (`DRM_MODE_FLAG_*`). Unknown bits survive round trips.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct ModeFlags(pub u32);

impl ModeFlags {
    /// Positive/negative hsync polarity.
    pub const PHSYNC: Self = Self(1 << 0);
    /// Negative hsync polarity.
    pub const NHSYNC: Self = Self(1 << 1);
    /// Positive vsync polarity.
    pub const PVSYNC: Self = Self(1 << 2);
    /// Negative vsync polarity.
    pub const NVSYNC: Self = Self(1 << 3);
    /// Interlaced mode.
    pub const INTERLACE: Self = Self(1 << 4);
    /// Double-scanned (mode doubled on scanout).
    pub const DBLSCAN: Self = Self(1 << 5);
    /// Composite sync (legacy).
    pub const CSYNC: Self = Self(1 << 6);
    /// Composite sync, positive polarity (legacy).
    pub const PCSYNC: Self = Self(1 << 7);
    /// Composite sync, negative polarity (legacy).
    pub const NCSYNC: Self = Self(1 << 8);
    /// Doubled pixel clock (DRM_MODE_FLAG_DBLCLK).
    pub const DBLCLOCK: Self = Self(1 << 12);
    /// Pixel clock halved (DRM_MODE_FLAG_CLKDIV2).
    pub const CLKDIV2: Self = Self(1 << 13);

    /// Whether the interlace flag is set.
    #[must_use]
    pub const fn is_interlaced(self) -> bool {
        self.0 & Self::INTERLACE.0 != 0
    }

    /// Whether double-scanning is on.
    #[must_use]
    pub const fn is_dblscan(self) -> bool {
        self.0 & Self::DBLSCAN.0 != 0
    }
}

impl core::ops::BitOr for ModeFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl core::ops::BitOrAssign for ModeFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

/// Mode type bits (`DRM_MODE_TYPE_*`).
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct ModeType(pub u32);

impl ModeType {
    /// A fixed/built-in mode (deprecated bit; preserved for round trips).
    pub const BUILTIN: Self = Self(1 << 0);
    /// The connector's preferred mode.
    pub const PREFERRED: Self = Self(1 << 3);
    /// A user-defined mode.
    pub const USERDEF: Self = Self(1 << 5);
    /// A driver-generated mode.
    pub const DRIVER: Self = Self(1 << 6);
}

impl core::ops::BitOr for ModeType {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl core::ops::BitOrAssign for ModeType {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

/// One complete scanout timing.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Mode {
    /// Pixel clock, kilohertz.
    pub clock_khz: u32,
    /// Horizontal: active, sync start, sync end, total (pixels), skew.
    pub hsync_start: u16,
    /// Horizontal sync end.
    pub hsync_end: u16,
    /// Horizontal total.
    pub htotal: u16,
    /// Horizontal skew (legacy).
    pub hskew: u16,
    /// Horizontal active width (pixels).
    pub hdisplay: u16,
    /// Vertical: active height (lines).
    pub vdisplay: u16,
    /// Vertical sync start.
    pub vsync_start: u16,
    /// Vertical sync end.
    pub vsync_end: u16,
    /// Vertical total.
    pub vtotal: u16,
    /// Vertical scan multiplier (legacy).
    pub vscan: u16,
    /// Flag bits.
    pub flags: ModeFlags,
    /// Type bits.
    pub kind: ModeType,
    /// Kernel-generated name, `hdisplay x vdisplay` (`i` suffixed when
    /// interlaced); at most 32 bytes, NUL padded.
    pub name: String,
}

impl Mode {
    /// Build a mode, deriving the kernel-style name.
    ///
    /// The parameter list is the `drmModeModeInfo` field order
    /// verbatim; introducing a builder here would only hide the ABI.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        clock_khz: u32,
        hdisplay: u16,
        hsync_start: u16,
        hsync_end: u16,
        htotal: u16,
        vdisplay: u16,
        vsync_start: u16,
        vsync_end: u16,
        vtotal: u16,
        flags: ModeFlags,
        kind: ModeType,
    ) -> Self {
        let suffix = if flags.is_interlaced() { "i" } else { "" };
        let name = format!("{hdisplay}x{vdisplay}{suffix}");
        Self {
            clock_khz,
            hdisplay,
            hsync_start,
            hsync_end,
            htotal,
            hskew: 0,
            vdisplay,
            vsync_start,
            vsync_end,
            vtotal,
            vscan: 1,
            flags,
            kind,
            name,
        }
    }

    /// Whether this mode is the connector's preferred one.
    #[must_use]
    pub const fn is_preferred(&self) -> bool {
        self.kind.0 & ModeType::PREFERRED.0 != 0
    }

    /// Refresh rate in millihertz, computed exactly from clock and totals:
    /// `clock_khz * 1_000_000 / (htotal * vtotal)` with DBLSCAN doubling
    /// and CLKDIV2 halving applied. 59.94 stays 59939/59940-distinguishable.
    #[must_use]
    pub fn refresh_millihz(&self) -> u64 {
        let mut numerator = u64::from(self.clock_khz) * 1_000_000;
        let mut denominator = u64::from(self.htotal) * u64::from(self.vtotal);
        if self.flags.is_dblscan() {
            denominator *= 2;
        }
        if self.flags.0 & ModeFlags::DBLCLOCK.0 != 0 {
            numerator *= 2;
        }
        if self.flags.0 & ModeFlags::CLKDIV2.0 != 0 {
            denominator *= 2;
        }
        numerator / denominator.max(1)
    }

    /// Refresh rate rounded to whole hertz (the kernel's `vrefresh`
    /// convention: `(htotal * vrefresh) / vtotal` rounding).
    #[must_use]
    pub fn refresh_hz(&self) -> u64 {
        (self.refresh_millihz() + 500) / 1000
    }

    /// Exact scanout period as a [`RefreshInterval`]; `None` only for a
    /// zero clock (rejected by validation).
    #[must_use]
    pub fn period(&self) -> Option<RefreshInterval> {
        RefreshInterval::from_millihz(
            u32::try_from(self.refresh_millihz().min(u32::MAX as u64)).ok()?,
        )
    }

    /// Field validity: positive dimensions, sane ordering, nonzero clock,
    /// totals covering the active area.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.clock_khz > 0
            && self.hdisplay > 0
            && self.vdisplay > 0
            && self.hsync_start >= self.hdisplay
            && self.hsync_end >= self.hsync_start
            && self.htotal >= self.hsync_end
            && self.vsync_start >= self.vdisplay
            && self.vsync_end >= self.vsync_start
            && self.vtotal >= self.vsync_end
            && self.vscan >= 1
            && self.name.len() < 32
    }

    /// Encode to the 68-byte `drmModeModeInfo` blob layout (little-endian).
    #[must_use]
    pub fn to_blob(&self) -> [u8; 68] {
        let mut b = [0u8; 68];
        b[0..4].copy_from_slice(&self.clock_khz.to_le_bytes());
        b[4..6].copy_from_slice(&self.hdisplay.to_le_bytes());
        b[6..8].copy_from_slice(&self.hsync_start.to_le_bytes());
        b[8..10].copy_from_slice(&self.hsync_end.to_le_bytes());
        b[10..12].copy_from_slice(&self.htotal.to_le_bytes());
        b[12..14].copy_from_slice(&self.hskew.to_le_bytes());
        b[14..16].copy_from_slice(&self.vdisplay.to_le_bytes());
        b[16..18].copy_from_slice(&self.vsync_start.to_le_bytes());
        b[18..20].copy_from_slice(&self.vsync_end.to_le_bytes());
        b[20..22].copy_from_slice(&self.vtotal.to_le_bytes());
        b[22..24].copy_from_slice(&self.vscan.to_le_bytes());
        // vrefresh (kernel-rounded) at offset 24.
        b[24..28].copy_from_slice(&(self.refresh_hz() as u32).to_le_bytes());
        b[28..32].copy_from_slice(&self.flags.0.to_le_bytes());
        b[32..36].copy_from_slice(&self.kind.0.to_le_bytes());
        let name = self.name.as_bytes();
        let n = name.len().min(31);
        b[36..36 + n].copy_from_slice(&name[..n]);
        b
    }

    /// Decode a `MODE_ID` blob payload.
    ///
    /// # Errors
    /// [`DisplayError::BadBlob`] for wrong length, undecodable name,
    /// or a field-invalid mode.
    pub fn from_blob(blob: &[u8]) -> Result<Self> {
        if blob.len() != 68 {
            return Err(DisplayError::BadBlob { what: "mode" });
        }
        let u16at = |o: usize| u16::from_le_bytes([blob[o], blob[o + 1]]);
        let u32at = |o: usize| u32::from_le_bytes([blob[o], blob[o + 1], blob[o + 2], blob[o + 3]]);
        let name_end = blob[36..].iter().position(|&c| c == 0).unwrap_or(32) + 36;
        let name = core::str::from_utf8(&blob[36..name_end])
            .map_err(|_| DisplayError::BadBlob { what: "mode" })?
            .to_owned();
        let mode = Self {
            clock_khz: u32at(0),
            hdisplay: u16at(4),
            hsync_start: u16at(6),
            hsync_end: u16at(8),
            htotal: u16at(10),
            hskew: u16at(12),
            vdisplay: u16at(14),
            vsync_start: u16at(16),
            vsync_end: u16at(18),
            vtotal: u16at(20),
            vscan: u16at(22),
            flags: ModeFlags(u32at(28)),
            kind: ModeType(u32at(32)),
            name,
        };
        if !mode.is_valid() {
            return Err(DisplayError::BadBlob { what: "mode" });
        }
        Ok(mode)
    }

    /// The standard 60 Hz panel timing (a common test fixture):
    /// 1920x1080, 148.5 MHz, hsync 2008-2052 total 2200, vsync 1084-1089
    /// total 1125.
    #[must_use]
    pub fn panel_1080p60() -> Self {
        Self::new(
            148_500,
            1920,
            2008,
            2052,
            2200,
            1080,
            1084,
            1089,
            1125,
            ModeFlags::NHSYNC | ModeFlags::NVSYNC,
            ModeType::PREFERRED | ModeType::DRIVER,
        )
    }

    /// The classic 59.94 Hz NTSC-rate timing (148.5 MHz / 1.001).
    #[must_use]
    pub fn panel_1080p_59_94() -> Self {
        Self::new(
            148_352,
            1920,
            2008,
            2052,
            2200,
            1080,
            1084,
            1089,
            1125,
            ModeFlags::NHSYNC | ModeFlags::NVSYNC,
            ModeType::DRIVER,
        )
    }

    /// A 144 Hz QHD timing (fixture for VRR windows). The 580,078 kHz
    /// clock over 2720x1481 totals lands at 143,999 mHz — exactly 144
    /// Hz under the kernel's rounded-`vrefresh` convention.
    #[must_use]
    pub fn panel_1440p144() -> Self {
        Self::new(
            580_078,
            2560,
            2608,
            2640,
            2720,
            1440,
            1443,
            1448,
            1481,
            ModeFlags::NHSYNC | ModeFlags::PVSYNC,
            ModeType::PREFERRED | ModeType::DRIVER,
        )
    }

    /// The standard 4K60 timing (the desktop-matrix fixture): 3840x2160
    /// at 594 MHz over 4400x2250 — exactly 60.000 Hz, the timing every
    /// UHD panel and GPU advertises.
    #[must_use]
    pub fn panel_4k60() -> Self {
        Self::new(
            594_000,
            3840,
            4016,
            4104,
            4400,
            2160,
            2168,
            2172,
            2250,
            ModeFlags::NHSYNC | ModeFlags::PVSYNC,
            ModeType::PREFERRED | ModeType::DRIVER,
        )
    }
}

impl core::fmt::Display for Mode {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "{} @ {}.{} Hz",
            self.name,
            self.refresh_millihz() / 1000,
            self.refresh_millihz() % 1000
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_math_is_exact() {
        let m = Mode::panel_1080p60();
        // 148500000 * 1000 / (2200*1125) = 60000 mHz exactly.
        assert_eq!(m.refresh_millihz(), 60_000);
        assert_eq!(m.refresh_hz(), 60);
        assert_eq!(m.period().unwrap().as_ns(), 16_666_666);

        let ntsc = Mode::panel_1080p_59_94();
        // 148352 kHz over 2200x1125: 148,352,000,000 / 2,475,000 mHz.
        assert_eq!(ntsc.refresh_millihz(), 59_940);
        assert_eq!(ntsc.refresh_hz(), 60);
        // 1e12 ns / 59940 mHz, truncated like the wire encoder.
        assert_eq!(ntsc.period().unwrap().as_ns(), 16_683_350);
    }

    #[test]
    fn names_follow_kernel_convention() {
        let m = Mode::panel_1080p60();
        assert_eq!(m.name, "1920x1080");
        let i = Mode::new(
            74_250,
            1920,
            2008,
            2052,
            2200,
            1080,
            1084,
            1089,
            1125,
            ModeFlags::INTERLACE,
            ModeType::DRIVER,
        );
        assert_eq!(i.name, "1920x1080i");
        assert!(i.flags.is_interlaced());
    }

    #[test]
    fn blob_round_trip_preserves_everything() {
        let m = Mode::panel_1440p144();
        let blob = m.to_blob();
        assert_eq!(blob.len(), 68);
        let back = Mode::from_blob(&blob).unwrap();
        assert_eq!(back, m);
        // Kernel vrefresh field is the rounded value.
        assert_eq!(
            u32::from_le_bytes([blob[24], blob[25], blob[26], blob[27]]),
            144
        );
    }

    #[test]
    fn bad_blobs_are_rejected() {
        assert!(Mode::from_blob(&[0u8; 67]).is_err());
        let mut blob = Mode::panel_1080p60().to_blob();
        blob[0..4].copy_from_slice(&0u32.to_le_bytes()); // zero clock
        assert!(Mode::from_blob(&blob).is_err());
        // hsync_start before hdisplay.
        let mut bad = Mode::panel_1080p60().to_blob();
        bad[6..8].copy_from_slice(&100u16.to_le_bytes());
        assert!(Mode::from_blob(&bad).is_err());
    }

    #[test]
    fn dblscan_refresh_is_derived() {
        let normal = Mode::panel_1080p60();
        let dbl = Mode {
            flags: ModeFlags::DBLSCAN,
            ..normal.clone()
        };
        assert_eq!(dbl.refresh_millihz(), 30_000);
    }
}
