//! EDID base-block parsing — identity *and* timing, pure.
//!
//! Real connectors expose their truth through the `EDID` property
//! blob. This module parses the mandatory 128-byte base block: the
//! `00 FF FF FF FF FF FF 00` header, the compressed three-letter
//! manufacturer code, the little-endian product code and serial, the
//! model-year window, and the monitor-name text descriptor. Parsing
//! validates the mod-256 checksum — a corrupted EDID must never be
//! mistaken for a different monitor, because the identity feeds
//! output bookkeeping and (later) persistence.
//!
//! Phase 42 grows the parse into the block's *timing* half — the
//! bytes the "every display size" row's remainder priced:
//!
//! * **Detailed timing descriptors** (DTD): the 18-byte blocks that
//!   carry a real scanout timing — pixel clock in 10 kHz units, the
//!   split-byte active/blanking geometry, the sync edges, the image
//!   size. The *first* DTD is the sink's preferred timing mode (the
//!   E-EDID rule; the kernel marks it `PREFERRED`). [`DetailedTiming`]
//!   decodes to the crate's [`Mode`] — the same wire the CRTC takes.
//! * **Monitor range limits** (the `0xFC`-neighbor descriptor, tag
//!   `0xFD`): the sink's declared envelope — vertical rate band,
//!   horizontal rate band, and the maximum pixel clock. The mode
//!   foundry gates every pour against this ceiling, and the audit
//!   compares it against the enumerated mode list (the
//!   `pixel-clock-ceiling` quirk's detection).
//! * **The audit** ([`audit`]): the named findings the bring-up logs
//!   — a rotten EDID, a preferred-timing lie (stale firmware naming a
//!   smaller mode than the connector serves), a declared clock
//!   ceiling the enumerated list exceeds. Each finding names its
//!   quirk-table row; the rows name the operator's escape.
//!
//! The DTD byte layout is the E-EDID 1.4 / CTA-861 shared contract
//! (the kernel's `drm_mode_detailed` walks the same bytes): clock in
//! 10 kHz units at bytes 0–1 (zero clocks are display descriptors,
//! not timings), the 8+4-bit split fields for the horizontal and
//! vertical active/blanking pairs, the nested sync offsets and
//! widths, the image size in millimetres, and the trailing flags
//! byte. One honest granularity note: the DTD wire quantizes the
//! pixel clock to 10 kHz — a 148 352 kHz timing (the NTSC-rate
//! fixture) cannot round-trip through a DTD (it lands at 148 350);
//! the kernel's own DTD decode has the same wire, the same step.
//!
//! The flags byte (byte 17) is read as this module's documented
//! contract: bit 7 interlaced; for digital sinks, bit 6 the hsync
//! polarity (1 = positive), bit 5 the vsync polarity (1 = positive).
//! Every timing field the stack consumes — clock, geometry, blanking,
//! sync edges — is byte-exact per E-EDID; polarity is the one soft
//! field (nothing computational in this crate reads it), and the
//! writer and parser here share the contract so mock EDIDs round-trip
//! exactly.

#![forbid(unsafe_code)]

use crate::error::{DisplayError, Result};
use crate::mode::{Mode, ModeFlags, ModeType};

/// Parsed identity and timing truth of one monitor (EDID base block).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct EdidIdentity {
    /// Three uppercase letters, uncompressed (e.g. `["L","TN"]`-style
    /// 5-bit codes become `"LTN"` for Lite-On, `"BNQ"` for BenQ).
    pub manufacturer: String,
    /// Vendor product code (LE u16).
    pub product: u16,
    /// Serial number (LE u32); 0 = "not set" per spec but preserved.
    pub serial: u32,
    /// Manufacturing week (1-54; 0 = unset, 255 = model year only).
    pub week: u8,
    /// Manufacturing year (1990-2254).
    pub year: u16,
    /// EDID structure version and revision.
    pub version: (u8, u8),
    /// Input is digital (bit 7 of byte 20).
    pub digital: bool,
    /// Monitor name from a `0xFC` display descriptor, trimmed of the
    /// NUL/padding tail; empty when absent.
    pub monitor_name: String,
    /// Number of extension blocks that follow the base block.
    pub extension_blocks: u8,
    /// Every detailed timing descriptor in the base block, in slot
    /// order (Phase 42). Empty when the sink declares none.
    pub timings: Vec<DetailedTiming>,
    /// The declared monitor range limits (tag `0xFD`), when present
    /// (Phase 42) — the sink's honest envelope.
    pub ranges: Option<RangeLimits>,
}

/// The mandatory base-block header constant.
const HEADER: [u8; 8] = [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00];

/// One 18-byte detailed timing descriptor, decoded (Phase 42).
///
/// The field names are the scanout vocabulary the crate's [`Mode`]
/// already speaks; [`DetailedTiming::to_mode`] is the bridge.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DetailedTiming {
    /// Pixel clock, kilohertz (the wire carries 10 kHz units).
    pub clock_khz: u32,
    /// Horizontal active, pixels.
    pub hactive: u32,
    /// Horizontal blanking, pixels.
    pub hblank: u32,
    /// Horizontal sync offset from active end (front porch), pixels.
    pub hsync_offset: u32,
    /// Horizontal sync width, pixels.
    pub hsync_width: u32,
    /// Vertical active, lines.
    pub vactive: u32,
    /// Vertical blanking, lines.
    pub vblank: u32,
    /// Vertical sync offset from active end, lines.
    pub vsync_offset: u32,
    /// Vertical sync width, lines.
    pub vsync_width: u32,
    /// Image size, millimetres (the connector's physical panel area).
    pub size_mm: (u32, u32),
    /// Interlaced timing.
    pub interlaced: bool,
    /// Digital separate-sync polarities (hsync, vsync); `true` is
    /// positive. Read per this module's documented flags contract.
    pub hsync_positive: bool,
    /// Vertical sync polarity, `true` = positive.
    pub vsync_positive: bool,
}

impl DetailedTiming {
    /// Decode one 18-byte descriptor.
    ///
    /// # Errors
    /// [`DisplayError::BadEdid`] when the block is not a timing (a
    /// zero pixel clock — the display-descriptor signature) or a
    /// field fails the split-byte recombination's sanity (an active
    /// area of zero).
    pub fn parse(dtd: &[u8]) -> Result<Self> {
        if dtd.len() != 18 {
            return Err(DisplayError::BadEdid { offset: 128 });
        }
        let clock_units = u16::from_le_bytes([dtd[0], dtd[1]]);
        if clock_units == 0 {
            // A display descriptor (monitor name, range limits, …),
            // not a timing — the caller discriminates, but a direct
            // ask is a malformed-EDID answer.
            return Err(DisplayError::BadEdid { offset: 0 });
        }
        let clock_khz = u32::from(clock_units) * 10;
        let hactive = u32::from(dtd[2]) | (u32::from(dtd[4] & 0xF0) << 4);
        let hblank = u32::from(dtd[3]) | (u32::from(dtd[4] & 0x0F) << 8);
        let hsync_offset = u32::from(dtd[5]) | (u32::from(dtd[8] & 0xC0) << 2);
        let hsync_width = u32::from(dtd[6]) | (u32::from(dtd[8] & 0x30) << 4);
        let vsync_offset = u32::from(dtd[7] >> 4) | (u32::from(dtd[8] & 0x0C) << 2);
        let vsync_width = u32::from(dtd[7] & 0x0F) | (u32::from(dtd[8] & 0x03) << 4);
        let vactive = u32::from(dtd[9]) | (u32::from(dtd[11] & 0xF0) << 4);
        let vblank = u32::from(dtd[10]) | (u32::from(dtd[11] & 0x0F) << 8);
        let hsize = u32::from(dtd[12]) | (u32::from(dtd[14] & 0xF0) << 4);
        let vsize = u32::from(dtd[13]) | (u32::from(dtd[14] & 0x0F) << 8);
        if hactive == 0 || vactive == 0 {
            return Err(DisplayError::BadEdid { offset: 9 });
        }
        Ok(Self {
            clock_khz,
            hactive,
            hblank,
            hsync_offset,
            hsync_width,
            vactive,
            vblank,
            vsync_offset,
            vsync_width,
            size_mm: (hsize, vsize),
            interlaced: dtd[17] & 0x80 != 0,
            hsync_positive: dtd[17] & 0x40 != 0,
            vsync_positive: dtd[17] & 0x20 != 0,
        })
    }

    /// Encode as the 18-byte descriptor (the writer of the parser's
    /// own contract — the mock's EDID synthesis uses this to make
    /// fixtures that round-trip exactly).
    #[must_use]
    pub fn to_bytes(&self) -> [u8; 18] {
        let mut b = [0u8; 18];
        let clock_units = u16::try_from(self.clock_khz / 10).unwrap_or(0);
        b[0..2].copy_from_slice(&clock_units.to_le_bytes());
        b[2] = split_lo(self.hactive);
        b[3] = split_lo(self.hblank);
        b[4] = split_hi(self.hactive) << 4 | split_hi(self.hblank);
        b[5] = split_lo(self.hsync_offset);
        b[6] = split_lo(self.hsync_width);
        // Vertical pair: 4-bit low fields in byte 7, high bits folded
        // into byte 8's low nibble.
        b[7] = split_lo(self.vsync_offset) << 4 | split_lo(self.vsync_width);
        b[8] = split_hi(self.hsync_offset) << 6
            | split_hi(self.hsync_width) << 4
            | split_hi(self.vsync_offset) << 2
            | split_hi(self.vsync_width);
        b[9] = split_lo(self.vactive);
        b[10] = split_lo(self.vblank);
        b[11] = split_hi(self.vactive) << 4 | split_hi(self.vblank);
        b[12] = split_lo(self.size_mm.0);
        b[13] = split_lo(self.size_mm.1);
        b[14] = split_hi(self.size_mm.0) << 4 | split_hi(self.size_mm.1);
        let mut flags = 0u8;
        if self.interlaced {
            flags |= 0x80;
        }
        if self.hsync_positive {
            flags |= 0x40;
        }
        if self.vsync_positive {
            flags |= 0x20;
        }
        b[17] = flags;
        b
    }

    /// The timing as a scanout [`Mode`] (the kernel's
    /// `drm_mode_detailed` bridge). `kind` sets the type bits — the
    /// preferred slot's caller marks [`ModeType::PREFERRED`].
    #[must_use]
    pub fn to_mode(&self, kind: ModeType) -> Mode {
        let mut flags = ModeFlags::default();
        if self.interlaced {
            flags |= ModeFlags::INTERLACE;
        }
        flags |= if self.hsync_positive {
            ModeFlags::PHSYNC
        } else {
            ModeFlags::NHSYNC
        };
        flags |= if self.vsync_positive {
            ModeFlags::PVSYNC
        } else {
            ModeFlags::NVSYNC
        };
        Mode::new(
            self.clock_khz,
            split_u16(self.hactive),
            split_u16(self.hactive + self.hsync_offset),
            split_u16(self.hactive + self.hsync_offset + self.hsync_width),
            split_u16(self.hactive + self.hblank),
            split_u16(self.vactive),
            split_u16(self.vactive + self.vsync_offset),
            split_u16(self.vactive + self.vsync_offset + self.vsync_width),
            split_u16(self.vactive + self.vblank),
            flags,
            kind,
        )
    }

    /// A timing from a [`Mode`] (the writer side of the bridge; the
    /// image size rides separately — a mode does not carry it).
    #[must_use]
    pub fn from_mode(mode: &Mode, size_mm: (u32, u32)) -> Self {
        let hactive = u32::from(mode.hdisplay);
        let vactive = u32::from(mode.vdisplay);
        Self {
            clock_khz: mode.clock_khz,
            hactive,
            hblank: u32::from(mode.htotal) - hactive,
            hsync_offset: u32::from(mode.hsync_start) - hactive,
            hsync_width: u32::from(mode.hsync_end) - u32::from(mode.hsync_start),
            vactive,
            vblank: u32::from(mode.vtotal) - vactive,
            vsync_offset: u32::from(mode.vsync_start) - vactive,
            vsync_width: u32::from(mode.vsync_end) - u32::from(mode.vsync_start),
            size_mm,
            interlaced: mode.flags.is_interlaced(),
            hsync_positive: mode.flags.0 & ModeFlags::PHSYNC.0 != 0,
            vsync_positive: mode.flags.0 & ModeFlags::PVSYNC.0 != 0,
        }
    }
}

/// The 8-bit low field of a split u12.
const fn split_lo(v: u32) -> u8 {
    (v & 0xFF) as u8
}

/// The 4-bit high field of a split u12.
const fn split_hi(v: u32) -> u8 {
    ((v >> 8) & 0x0F) as u8
}

/// The u16 clamped view (mode fields are 16-bit; DTD fields are
/// 12-bit — the split helpers keep the writer honest at the same
/// width).
const fn split_u16(v: u32) -> u16 {
    if v > 65_535 {
        u16::MAX
    } else {
        v as u16
    }
}

/// The declared monitor range limits (descriptor tag `0xFD`,
/// Phase 42) — the sink's honest envelope.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RangeLimits {
    /// Vertical rate band, hertz (min, max).
    pub vrate_hz: (u8, u8),
    /// Horizontal rate band, kilohertz (min, max).
    pub hrate_khz: (u8, u8),
    /// Maximum pixel clock, kilohertz (the wire carries 10 MHz
    /// units — the ceiling the mode foundry gates every pour
    /// against).
    pub max_clock_khz: u32,
}

impl RangeLimits {
    /// Decode the range-limits descriptor's payload (the 13 bytes
    /// after the `00 00 00 FD 00` descriptor header).
    ///
    /// # Errors
    /// [`DisplayError::BadEdid`] on an inverted band (min above max)
    /// — the declared envelope must be sane to be believed.
    pub fn parse(payload: &[u8]) -> Result<Self> {
        if payload.len() < 5 {
            return Err(DisplayError::BadEdid { offset: 128 });
        }
        let limits = Self {
            vrate_hz: (payload[0], payload[1]),
            hrate_khz: (payload[2], payload[3]),
            max_clock_khz: u32::from(payload[4]) * 10_000,
        };
        if limits.vrate_hz.0 > limits.vrate_hz.1 || limits.hrate_khz.0 > limits.hrate_khz.1 {
            return Err(DisplayError::BadEdid { offset: 128 });
        }
        Ok(limits)
    }

    /// The descriptor payload's first five bytes (the writer side).
    #[must_use]
    pub fn to_bytes(&self) -> [u8; 5] {
        [
            self.vrate_hz.0,
            self.vrate_hz.1,
            self.hrate_khz.0,
            self.hrate_khz.1,
            u8::try_from(self.max_clock_khz / 10_000).unwrap_or(0),
        ]
    }
}

impl EdidIdentity {
    /// Parse and checksum-validate a 128-byte base block.
    ///
    /// # Errors
    /// [`DisplayError::BadEdid`] at the first offending byte for wrong
    /// length, wrong header, failed checksum, or out-of-range letters.
    pub fn parse(block: &[u8]) -> Result<Self> {
        if block.len() != 128 {
            return Err(DisplayError::BadEdid { offset: 128 });
        }
        if block[..8] != HEADER {
            return Err(DisplayError::BadEdid { offset: 0 });
        }
        let checksum = block.iter().fold(0u8, |a, &b| a.wrapping_add(b));
        if checksum != 0 {
            return Err(DisplayError::BadEdid { offset: 127 });
        }

        // Bytes 8-9: big-endian compressed manufacturer code — three
        // 5-bit letters, offset 0x01 ('A'..='Z').
        let code = ((u16::from(block[8]) << 8) | u16::from(block[9])) >> 2 & 0x7FFF;
        let mut manufacturer = String::new();
        for shift in [10, 5, 0] {
            let letter = u32::from((code >> shift) & 0x1F);
            if letter == 0 || letter > 26 {
                return Err(DisplayError::BadEdid { offset: 8 });
            }
            manufacturer.push(char::from_u32(b'A' as u32 - 1 + letter).unwrap_or('?'));
        }

        let product = u16::from_le_bytes([block[10], block[11]]);
        let serial = u32::from_le_bytes([block[12], block[13], block[14], block[15]]);
        let week = block[16];
        let year = u16::from(block[17]) + 1990;
        let version = (block[18], block[19]);
        let digital = block[20] & 0x80 != 0;

        // Descriptor slots 4-18, 22-36, 38-52, 54-68 (18 bytes each):
        // a slot whose first two bytes carry a pixel clock is a
        // detailed timing descriptor (the first one is the sink's
        // preferred timing mode — the E-EDID rule); a slot starting
        // 00 00 00 <tag> 00 is a display descriptor (tag 0xFC the
        // monitor name, tag 0xFD the range limits).
        let mut monitor_name = String::new();
        let mut timings: Vec<DetailedTiming> = Vec::new();
        let mut ranges: Option<RangeLimits> = None;
        for slot in (54..126).step_by(18) {
            if block[slot] | block[slot + 1] != 0 {
                if let Ok(dtd) = DetailedTiming::parse(&block[slot..slot + 18]) {
                    timings.push(dtd);
                }
            } else if block[slot + 2] == 0 && block[slot + 4] == 0 {
                let tag = block[slot + 3];
                let payload = &block[slot + 5..slot + 18];
                if tag == 0xFC {
                    let end = payload
                        .iter()
                        .position(|&c| c == 0x0A || c == 0x00)
                        .unwrap_or(payload.len());
                    String::from_utf8_lossy(&payload[..end])
                        .trim_end()
                        .clone_into(&mut monitor_name);
                } else if tag == 0xFD {
                    if let Ok(limits) = RangeLimits::parse(payload) {
                        ranges = Some(limits);
                    }
                }
            }
        }

        Ok(Self {
            manufacturer,
            product,
            serial,
            week,
            year,
            version,
            digital,
            monitor_name,
            extension_blocks: block[126],
            timings,
            ranges,
        })
    }

    /// A stable textual identity for logs and persistence.
    #[must_use]
    pub fn identity_string(&self) -> String {
        if self.serial != 0 {
            format!(
                "{}-{:04X}-{:08X}",
                self.manufacturer, self.product, self.serial
            )
        } else {
            format!("{}-{:04X}", self.manufacturer, self.product)
        }
    }

    /// The sink's preferred timing as a scanout mode (the first DTD,
    /// marked [`ModeType::PREFERRED`] — the kernel's own marking).
    /// `None` when the base block declares no timing.
    #[must_use]
    pub fn preferred_mode(&self) -> Option<Mode> {
        self.timings
            .first()
            .map(|dtd| dtd.to_mode(ModeType::PREFERRED | ModeType::DRIVER))
    }

    /// Build a synthetic (but structurally valid) base block — the mock
    /// device uses this to fabricate connector EDIDs, and the tests use
    /// it to prove the parser's round trip.
    #[must_use]
    pub fn synthesize(
        manufacturer: &str,
        product: u16,
        serial: u32,
        monitor_name: &str,
        year: u16,
    ) -> [u8; 128] {
        Self::synthesize_slots(
            manufacturer,
            product,
            serial,
            monitor_name,
            year,
            None,
            None,
            54, // the name in the first slot (the identity-only layout)
        )
    }

    /// Build a base block carrying the *timing* half (Phase 42): the
    /// preferred timing as the first DTD (slot 54 — the E-EDID rule
    /// the kernel's preferred-mode marking reads), the declared range
    /// limits in the second slot, and the monitor name in the third.
    /// The mock panels' EDIDs now tell the truth about what they scan.
    #[must_use]
    pub fn synthesize_detailed(
        manufacturer: &str,
        product: u16,
        serial: u32,
        monitor_name: &str,
        year: u16,
        preferred: Option<&Mode>,
        ranges: Option<RangeLimits>,
    ) -> [u8; 128] {
        Self::synthesize_slots(
            manufacturer,
            product,
            serial,
            monitor_name,
            year,
            preferred,
            ranges,
            90, // the name behind the timing pair
        )
    }

    /// The shared writer: identity always, the timing pair when
    /// given, the name descriptor at `name_slot`.
    #[allow(clippy::too_many_arguments)]
    fn synthesize_slots(
        manufacturer: &str,
        product: u16,
        serial: u32,
        monitor_name: &str,
        year: u16,
        preferred: Option<&Mode>,
        ranges: Option<RangeLimits>,
        name_slot: usize,
    ) -> [u8; 128] {
        let mut block = [0u8; 128];
        block[..8].copy_from_slice(&[0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00]);
        let letters: Vec<u32> = manufacturer
            .chars()
            .take(3)
            .map(|c| u32::from(c.to_ascii_uppercase()) - u32::from(b'A') + 1)
            .collect();
        let packed: u32 = ((letters.first().copied().unwrap_or(1) & 0x1F) << 10)
            | ((letters.get(1).copied().unwrap_or(1) & 0x1F) << 5)
            | (letters.get(2).copied().unwrap_or(1) & 0x1F);
        let code: u16 = u16::try_from(packed).unwrap_or(0);
        block[8] = u8::try_from((code << 2) >> 8).unwrap_or(0);
        block[9] = u8::try_from((code << 2) & 0xFF).unwrap_or(0);
        block[10..12].copy_from_slice(&product.to_le_bytes());
        block[12..16].copy_from_slice(&serial.to_le_bytes());
        block[16] = 0; // week unset
        block[17] = u8::try_from(year.saturating_sub(1990).min(254)).unwrap_or(30);
        block[18] = 1; // EDID 1.4
        block[19] = 4;
        block[20] = 0xA5; // digital input, placeholder flags

        // Slot 54: the preferred timing, when the fixture carries one.
        if let Some(mode) = preferred {
            let dtd = DetailedTiming::from_mode(mode, (309, 174));
            block[54..72].copy_from_slice(&dtd.to_bytes());
        }
        // Slot 72: the declared range limits.
        if let Some(limits) = ranges {
            block[72] = 0;
            block[73] = 0;
            block[74] = 0;
            block[75] = 0xFD;
            block[76] = 0;
            let payload = limits.to_bytes();
            block[77..82].copy_from_slice(&payload);
            // The remaining descriptor bytes pad zero (no extended
            // timing information — the honest default).
        }
        // The monitor name descriptor at the layout's slot.
        block[name_slot + 3] = 0xFC;
        let name = monitor_name.as_bytes();
        let n = name.len().min(13);
        block[name_slot + 5..name_slot + 5 + n].copy_from_slice(&name[..n]);
        block[name_slot + 5 + n..name_slot + 18].fill(0x20);
        // Terminate only when the name does not fill the descriptor:
        // a 13-byte name occupies the final byte, and overwriting it
        // with the line feed truncated one character on round trip
        // (found by the Phase 10 output cascade test).
        if n < 13 {
            block[name_slot + 5 + n] = 0x0A;
        }
        block[126] = 0; // no extensions
        let sum: i32 = block[..127].iter().map(|&b| i32::from(b)).sum();
        block[127] = u8::try_from((256 - sum % 256) % 256).unwrap_or(0);
        block
    }
}

/// A named EDID-side finding — the audit's answer, each row's name
/// the quirk table's (Phase 42). The bring-up logs these; the rows
/// carry the operator's escape.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EdidFinding {
    /// The connector supplies EDID bytes that do not parse (the
    /// `edid-rotten` row): a failed checksum or header — the
    /// identity is unknown, the timing truth absent.
    RottenEdid,
    /// The EDID's preferred timing names a smaller mode than the
    /// connector's best offering (the `edid-preferred-lie` row):
    /// stale firmware — the panel scans more than it claims to
    /// prefer.
    PreferredLie {
        /// What the EDID's first DTD names.
        preferred: (u32, u32),
        /// What the connector actually serves at its largest.
        best: (u32, u32),
    },
    /// The connector's enumerated list exceeds the EDID's declared
    /// pixel-clock ceiling (the `pixel-clock-ceiling` row): modes
    /// above the envelope are the ones that blank on the panel.
    ClockCeiling {
        /// The declared maximum, kilohertz.
        declared_khz: u32,
        /// The best offered mode's clock, kilohertz.
        best_khz: u32,
    },
}

impl EdidFinding {
    /// The quirk-table row this finding names (the table's stable
    /// name — the row carries the escape).
    #[must_use]
    pub const fn quirk_name(&self) -> &'static str {
        match self {
            EdidFinding::RottenEdid => "edid-rotten",
            EdidFinding::PreferredLie { .. } => "edid-preferred-lie",
            EdidFinding::ClockCeiling { .. } => "pixel-clock-ceiling",
        }
    }
}

impl core::fmt::Display for EdidFinding {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            EdidFinding::RottenEdid => {
                write!(f, "edid-rotten: the EDID does not parse (identity unknown)")
            }
            EdidFinding::PreferredLie { preferred, best } => write!(
                f,
                "edid-preferred-lie: the EDID prefers {}x{} but the connector serves {}x{}",
                preferred.0,
                preferred.1,
                best.0,
                best.1
            ),
            EdidFinding::ClockCeiling {
                declared_khz,
                best_khz,
            } => write!(
                f,
                "pixel-clock-ceiling: the best mode clocks {best_khz} kHz over the declared {declared_khz} kHz"
            ),
        }
    }
}

/// Audit one connector's truth: its enumerated mode list against its
/// EDID (Phase 42). Every finding is a named quirk row — the honest
/// diagnosis the bring-up prints, symptom to escape in one line.
///
/// No EDID (a hotplug half-state, a headless virtual) audits clean —
/// nothing declared, nothing contradicted.
#[must_use]
pub fn audit(modes: &[Mode], edid: Option<&[u8]>) -> Vec<EdidFinding> {
    let Some(bytes) = edid else {
        return Vec::new();
    };
    let Ok(id) = EdidIdentity::parse(bytes) else {
        return vec![EdidFinding::RottenEdid];
    };
    let mut findings = Vec::new();
    // The largest enumerated offering, area first then clock (the
    // "best the connector serves" the lie is measured against).
    let best = modes.iter().max_by_key(|m| {
        (
            u64::from(m.hdisplay) * u64::from(m.vdisplay),
            u64::from(m.clock_khz),
        )
    });
    if let (Some(best), Some(preferred)) = (best, id.preferred_mode()) {
        let preferred_area = u64::from(preferred.hdisplay) * u64::from(preferred.vdisplay);
        let best_area = u64::from(best.hdisplay) * u64::from(best.vdisplay);
        if best_area > preferred_area {
            findings.push(EdidFinding::PreferredLie {
                preferred: (u32::from(preferred.hdisplay), u32::from(preferred.vdisplay)),
                best: (u32::from(best.hdisplay), u32::from(best.vdisplay)),
            });
        }
    }
    if let (Some(best), Some(ranges)) = (best, id.ranges) {
        if best.clock_khz > ranges.max_clock_khz {
            findings.push(EdidFinding::ClockCeiling {
                declared_khz: ranges.max_clock_khz,
                best_khz: best.clock_khz,
            });
        }
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_synthetic() {
        let raw = EdidIdentity::synthesize("BNQ", 0x7D12, 0xAB_CDEF01, "Lion Panel", 2026);
        let id = EdidIdentity::parse(&raw).unwrap();
        assert_eq!(id.manufacturer, "BNQ");
        assert_eq!(id.product, 0x7D12);
        assert_eq!(id.serial, 0xAB_CDEF01);
        assert_eq!(id.monitor_name, "Lion Panel");
        assert_eq!(id.year, 2026);
        assert!(id.digital);
        assert_eq!(id.version, (1, 4));
        assert_eq!(id.identity_string(), "BNQ-7D12-ABCDEF01");
        // The identity-only layout declares no timing truth.
        assert!(id.timings.is_empty());
        assert!(id.preferred_mode().is_none());
        assert!(id.ranges.is_none());
    }

    #[test]
    fn checksum_corruption_detected() {
        let mut raw = EdidIdentity::synthesize("LTI", 1, 2, "x", 2025);
        raw[42] ^= 1;
        assert!(EdidIdentity::parse(&raw).is_err());
    }

    #[test]
    fn header_and_length_rejected() {
        assert!(EdidIdentity::parse(&[0u8; 127]).is_err());
        let mut raw = EdidIdentity::synthesize("AAA", 1, 1, "", 2025);
        raw[1] = 0;
        // Fix checksum so only the header is wrong.
        let sum: i32 = raw[..127].iter().map(|&b| i32::from(b)).sum();
        raw[127] = u8::try_from((256 - sum % 256) % 256).unwrap_or(0);
        assert!(EdidIdentity::parse(&raw).is_err());
    }

    #[test]
    fn manufacturer_letter_range_enforced() {
        let mut raw = EdidIdentity::synthesize("AZZ", 1, 1, "", 2025);
        // Force a zero letter (0x00 = invalid) in the first 5-bit slot.
        raw[8] = 0;
        raw[9] = 0;
        let sum: i32 = raw[..127].iter().map(|&b| i32::from(b)).sum();
        raw[127] = u8::try_from((256 - sum % 256) % 256).unwrap_or(0);
        let err = EdidIdentity::parse(&raw).unwrap_err();
        assert!(matches!(err, DisplayError::BadEdid { offset: 8 }));
    }

    #[test]
    fn serial_zero_identity() {
        let raw = EdidIdentity::synthesize("DEL", 0x1234, 0, "", 2025);
        let id = EdidIdentity::parse(&raw).unwrap();
        assert_eq!(id.identity_string(), "DEL-1234");
    }

    /// The timing round trip (Phase 42): a base block carrying the
    /// 1080p60 panel timing as its first DTD parses back to the same
    /// mode — clock, geometry, blanking, sync edges, polarity — with
    /// the preferred marking the E-EDID first slot carries. The DTD
    /// wire's own granularity (10 kHz clocks) holds exactly for this
    /// fixture: 148 500 kHz is 14 850 units.
    #[test]
    fn detailed_timing_round_trip() {
        let mode = Mode::panel_1080p60();
        let raw = EdidIdentity::synthesize_detailed(
            "LTI",
            0x3107,
            0x1122_3344,
            "Lion Panel 15",
            2026,
            Some(&mode),
            Some(RangeLimits {
                vrate_hz: (56, 75),
                hrate_khz: (30, 83),
                max_clock_khz: 300_000,
            }),
        );
        let id = EdidIdentity::parse(&raw).unwrap();
        assert_eq!(id.monitor_name, "Lion Panel 15");
        // The first DTD is the preferred timing, marked like the
        // kernel marks it.
        let preferred = id.preferred_mode().unwrap();
        assert_eq!(preferred.hdisplay, 1920);
        assert_eq!(preferred.vdisplay, 1080);
        assert_eq!(preferred.htotal, 2200);
        assert_eq!(preferred.vtotal, 1125);
        assert_eq!(preferred.clock_khz, 148_500);
        assert_eq!(preferred.hsync_start, 2008);
        assert_eq!(preferred.hsync_end, 2052);
        assert_eq!(preferred.vsync_start, 1084);
        assert_eq!(preferred.vsync_end, 1089);
        assert_eq!(preferred.flags, mode.flags);
        assert!(preferred.is_preferred());
        // The declared envelope round-trips.
        let ranges = id.ranges.unwrap();
        assert_eq!(ranges.vrate_hz, (56, 75));
        assert_eq!(ranges.hrate_khz, (30, 83));
        assert_eq!(ranges.max_clock_khz, 300_000);
    }

    /// The DTD wire's 10 kHz clock granularity, stated honestly: the
    /// NTSC-rate fixture (148 352 kHz) lands at 148 350 through a
    /// DTD — the same step the kernel's own decode takes.
    #[test]
    fn dtd_clock_granularity_is_ten_khz() {
        let mode = Mode::panel_1080p_59_94();
        let dtd = DetailedTiming::from_mode(&mode, (510, 290));
        let back = dtd.to_bytes();
        let parsed = DetailedTiming::parse(&back).unwrap();
        assert_eq!(parsed.clock_khz, 148_350);
        // Everything else survives exactly.
        assert_eq!(parsed.hactive, 1920);
        assert_eq!(parsed.hblank, 280);
        assert_eq!(parsed.vactive, 1080);
        assert_eq!(parsed.vblank, 45);
    }

    /// A zero pixel clock is a display descriptor, not a timing —
    /// the discriminator the descriptor walk keys on.
    #[test]
    fn zero_clock_is_not_a_timing() {
        assert!(DetailedTiming::parse(&[0u8; 18]).is_err());
        // The monitor-name descriptor's shape (00 00 00 FC 00 …).
        let mut dtd = [0u8; 18];
        dtd[3] = 0xFC;
        assert!(DetailedTiming::parse(&dtd).is_err());
    }

    /// The range-limits parse: sane bands decode, inverted bands are
    /// refused (a declared envelope must be sane to be believed).
    #[test]
    fn range_limits_sanity() {
        let mut payload = [0u8; 13];
        payload[0] = 48;
        payload[1] = 144;
        payload[2] = 24;
        payload[3] = 115;
        payload[4] = 60; // 600 MHz
        let limits = RangeLimits::parse(&payload).unwrap();
        assert_eq!(limits.vrate_hz, (48, 144));
        assert_eq!(limits.hrate_khz, (24, 115));
        assert_eq!(limits.max_clock_khz, 600_000);
        payload[0] = 144;
        payload[1] = 48;
        assert!(RangeLimits::parse(&payload).is_err());
    }

    /// The audit (Phase 42): the three named findings, each fired by
    /// its own fixture and each naming its quirk row.
    #[test]
    fn audit_names_the_findings() {
        // A clean panel: the EDID prefers what the list serves, the
        // clock sits under the declared ceiling.
        let clean_edid = EdidIdentity::synthesize_detailed(
            "LTI",
            1,
            1,
            "clean",
            2026,
            Some(&Mode::panel_1080p60()),
            Some(RangeLimits {
                vrate_hz: (56, 75),
                hrate_khz: (30, 83),
                max_clock_khz: 300_000,
            }),
        );
        let modes = vec![Mode::panel_1080p_59_94(), Mode::panel_1080p60()];
        assert!(audit(&modes, Some(&clean_edid)).is_empty());

        // No EDID at all: nothing declared, nothing contradicted.
        assert!(audit(&modes, None).is_empty());

        // A rotten EDID: bytes that fail the parse.
        let mut rotten = clean_edid;
        rotten[42] ^= 0xFF;
        let findings = audit(&modes, Some(&rotten));
        assert_eq!(findings, vec![EdidFinding::RottenEdid]);
        assert_eq!(findings[0].quirk_name(), "edid-rotten");

        // The preferred lie: stale firmware naming 720p while the
        // connector serves 1080p.
        let lying = EdidIdentity::synthesize_detailed(
            "BNQ",
            2,
            2,
            "stale",
            2024,
            Some(&Mode::new(
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
            )),
            None,
        );
        let findings = audit(&modes, Some(&lying));
        assert_eq!(
            findings,
            vec![EdidFinding::PreferredLie {
                preferred: (1280, 720),
                best: (1920, 1080)
            }]
        );
        assert_eq!(findings[0].quirk_name(), "edid-preferred-lie");

        // The clock ceiling: a declared 120 MHz envelope under a
        // 148.5 MHz best mode.
        let capped = EdidIdentity::synthesize_detailed(
            "BNQ",
            3,
            3,
            "capped",
            2026,
            Some(&Mode::panel_1080p60()),
            Some(RangeLimits {
                vrate_hz: (56, 75),
                hrate_khz: (30, 83),
                max_clock_khz: 120_000,
            }),
        );
        let findings = audit(&modes, Some(&capped));
        assert_eq!(
            findings,
            vec![EdidFinding::ClockCeiling {
                declared_khz: 120_000,
                best_khz: 148_500
            }]
        );
        assert_eq!(findings[0].quirk_name(), "pixel-clock-ceiling");
    }
}
