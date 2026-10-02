//! The output global: one display output's protocol face.
//!
//! [`OutputGlobal`] freezes everything the `ldp.core.output` cascade
//! needs — connector identity and EDID, the mode list, the VRR window,
//! the layout position — and produces the bind-time event sequence as
//! plain `(event, args)` pairs the dispatcher streams after `bound`.
//! The same model answers the renderer's [`OutputDesc`] and the
//! scheduler's nominal refresh, so the protocol description and the
//! pipeline geometry cannot drift apart.
//!
//! Wire values (from `spec/core.toml`): `output_caps` bits
//! vrr=0/hdr=1/atomic=2/overlay_planes=3/cursor=4; `mode_flags` bits
//! preferred=0/current=1/interlaced=2; `output_subpixel`,
//! `color_primaries`, `transfer_function`, `color_range`, `transform`
//! enums as their spec tables; `vrr_caps` seamless=0/fixed_rate=1.

#![forbid(unsafe_code)]

use ldp_core::bitset::Bitset128;
use ldp_core::buffer::FourCC;
use ldp_core::color::ColorDescription;
use ldp_core::geometry::Rect;
use ldp_core::time::RefreshInterval;
use ldp_core::wire::Value;
use ldp_display::backend::KmsBackend;
use ldp_display::ids::{ConnectorId, CrtcId};
use ldp_display::mode::{Mode, ModeFlags, ModeType};
use ldp_display::serve::Pipeline;
use ldp_display::{ConnectorStatus, ConnectorType, MockDevice, Subpixel};
use ldp_renderer::OutputDesc;

/// A bitset from its low wire word (bits 0..=31).
fn low_bits(v: u32) -> Bitset128 {
    Bitset128::from_words([v, 0, 0, 0])
}

/// One served output.
#[derive(Clone, Debug)]
pub struct OutputGlobal {
    /// The connector this output mirrors.
    pub connector: ConnectorInfo,
    /// The CRTC driving it.
    pub crtc: CrtcId,
    /// VRR window in nanoseconds `(min, max)` when adaptive sync is
    /// supported.
    pub vrr: Option<(u64, u64)>,
    /// Index into `connector.modes` of the active mode.
    pub mode_index: usize,
    /// Position in the global logical layout.
    pub layout: (i32, i32),
    /// Kernel-style connector name (`eDP-1`).
    pub name: String,
    /// The output's scale factor (Phase 31, `--scale F`): advertised
    /// over the cascade (`output.scale`, Q8.8) and used by the
    /// positioning shell's logical canvas. Identity by default.
    pub scale: ldp_core::scale::ScaleFactor,
    /// The panel's HDR capabilities when the operator forces the HDR
    /// pipeline (`--hdr`, Phase 31): PQ support and the peak
    /// luminance — advertised over the cascade (`output.hdr_caps`)
    /// and used by the output-mode policy (the peak negotiation's
    /// ceiling). `None` (the default) is the honest SDR panel.
    pub hdr: Option<ldp_hdr::policy::OutputCaps>,
    /// Overlay planes exist on this pipeline.
    pub overlay_planes: bool,
    /// A hardware cursor plane exists.
    pub cursor_plane: bool,
    /// The EDID audit's named findings (Phase 42): the connector's
    /// enumerated mode list against its EDID — a rotten block, a
    /// preferred-timing lie, a declared clock ceiling the list
    /// exceeds. Each finding names its quirk-table row; the bring-up
    /// prints them (the honest diagnosis, symptom to escape in one
    /// line). Empty is the clean panel (or no EDID at all).
    pub edid_findings: Vec<ldp_display::edid::EdidFinding>,
}

/// Minimal connector snapshot (fields the output model needs).
///
/// This mirrors `ldp_display::connector::ConnectorInfo` without the
/// fields irrelevant to the protocol face, keeping the model
/// constructible from any backend.
#[derive(Clone, Debug)]
pub struct ConnectorInfo {
    /// Object id.
    pub id: ConnectorId,
    /// Connection state.
    pub status: ConnectorStatus,
    /// Panel physical width, millimetres (0 when unknown).
    pub mm_width: u32,
    /// Panel physical height, millimetres.
    pub mm_height: u32,
    /// Subpixel arrangement.
    pub subpixel: Subpixel,
    /// Modes offered by the sink.
    pub modes: Vec<Mode>,
    /// EDID identity, when the sink supplies a parseable one.
    pub identity: Option<ldp_display::EdidIdentity>,
}

impl OutputGlobal {
    /// Build from the mock device's first connected connector bound to
    /// its CRTC, preferring the VRR-capable CRTC.
    ///
    /// # Errors
    /// `None` when no connected connector with modes exists — a
    /// headless bring-up failure, not a protocol state.
    #[must_use]
    pub fn from_mock(device: &MockDevice, layout: (i32, i32)) -> Option<OutputGlobal> {
        let top = device.topology().ok()?;
        let connector = top
            .connectors
            .iter()
            .map(|c| device.connector_info(*c))
            .filter_map(Result::ok)
            .find(|c| {
                c.status == ConnectorStatus::Connected && !c.modes.is_empty() && c.edid.is_some()
            })?;
        let crtc = *top.crtcs.first()?;
        let vrr = device.vrr_window(crtc);
        Some(Self::from_connector(device, connector, crtc, vrr, layout))
    }

    /// Build over any backend, from an already-selected pipeline
    /// (Phase 25's real path): the connector snapshot frozen for the
    /// protocol face, VRR reported as unsupported until the property
    /// walk that detects it ships (the honest boundary — see
    /// `docs/roadmap.md`).
    ///
    /// # Errors
    /// `None` when the connector's snapshot cannot be read — the
    /// honest bring-up failure.
    #[must_use]
    pub fn from_backend(
        backend: &dyn KmsBackend,
        pipeline: &Pipeline,
        layout: (i32, i32),
    ) -> Option<OutputGlobal> {
        let connector = backend.connector_info(pipeline.connector).ok()?;
        Some(Self::from_connector(
            backend,
            connector,
            pipeline.crtc,
            None, // VRR detection on real nodes: a documented future item
            layout,
        ))
    }

    /// The shared constructor: freeze the connector snapshot, prefer
    /// the marked mode, name the output kernel-style.
    #[must_use]
    fn from_connector(
        backend: &dyn KmsBackend,
        connector: ldp_display::ConnectorInfo,
        crtc: CrtcId,
        vrr: Option<(u64, u64)>,
        layout: (i32, i32),
    ) -> OutputGlobal {
        let identity = connector.identity();
        // Phase 42: the EDID audit runs once, at the freeze — the
        // connector's enumerated list against its declared timing
        // truth (the raw snapshot still carries the EDID bytes here).
        let edid_findings = ldp_display::edid::audit(&connector.modes, connector.edid.as_deref());
        let kind = connector.kind;
        let type_index = connector.type_index;
        let connector = ConnectorInfo {
            id: connector.id,
            status: connector.status,
            mm_width: connector.mm_width,
            mm_height: connector.mm_height,
            subpixel: connector.subpixel,
            modes: connector.modes,
            identity,
        };
        // Preferred mode when the sink marks one, else the first.
        let mode_index = connector
            .modes
            .iter()
            .position(|m| m.kind.0 & ModeType::PREFERRED.0 != 0)
            .unwrap_or(0);
        let name = format!(
            "{}-{}",
            connector_kind_name(kind),
            type_index_of_parts(backend, connector.id, type_index)
        );
        OutputGlobal {
            connector,
            crtc,
            vrr,
            mode_index,
            layout,
            name,
            scale: ldp_core::scale::ScaleFactor::IDENTITY,
            hdr: None,
            overlay_planes: true,
            cursor_plane: true,
            edid_findings,
        }
    }

    /// The active mode.
    #[must_use]
    pub fn mode(&self) -> &Mode {
        &self.connector.modes[self.mode_index]
    }

    /// Re-point the active mode at the operator's forced size
    /// (`--resolution WxH`, Phase 30): the best exact-area match in
    /// the frozen list — preferred flag first, then the higher
    /// refresh, the same ordering the sized pipeline selection uses,
    /// so the protocol face and the driven CRTC agree.
    ///
    /// `false` is the honest miss (no mode of that size on this
    /// connector — the caller fails loudly or falls back to the
    /// unsized doctrine); `true` re-points [`Self::mode`] and every
    /// protocol consequence (`current_mode`, geometry, the renderer's
    /// output description) with it.
    #[must_use]
    pub fn select_size(&mut self, size: ldp_display::serve::ModeSize) -> bool {
        let Some(index) = self
            .connector
            .modes
            .iter()
            .enumerate()
            .filter(|(_, m)| {
                u32::from(m.hdisplay) == size.width && u32::from(m.vdisplay) == size.height
            })
            .max_by(|(_, a), (_, b)| {
                (a.is_preferred(), a.refresh_millihz())
                    .cmp(&(b.is_preferred(), b.refresh_millihz()))
            })
            .map(|(i, _)| i)
        else {
            return false;
        };
        self.mode_index = index;
        true
    }

    /// Adopt the mode foundry's pour (Phase 42): the user-defined
    /// mode *joins the protocol face's advertised list* — the
    /// `xrandr --newmode --addmode` story, protocol-side — and the
    /// active mode points at it. The mock's engine layer keeps its
    /// own envelope gate at commit time (the second of the foundry's
    /// two gates); the advertised list is the *protocol's* truth: a
    /// client binding this output sees the poured mode among its
    /// modes.
    ///
    /// The KMS list itself never learns the pour: the mode stays a
    /// `USERDEF` mode on the wire, validated by the engine, never
    /// laundered into a sink-offered timing.
    pub fn adopt_mode(&mut self, mode: &Mode) {
        if let Some(index) = self.connector.modes.iter().position(|m| m == mode) {
            self.mode_index = index;
            return;
        }
        self.connector.modes.push(mode.clone());
        self.mode_index = self.connector.modes.len() - 1;
    }

    /// Whether the active mode is the foundry's pour (a `USERDEF`
    /// timing — Phase 42): the report lines key on it.
    #[must_use]
    pub fn serves_poured_mode(&self) -> bool {
        self.mode().kind.0 & ModeType::USERDEF.0 != 0
    }

    /// The nominal refresh interval of the active mode.
    ///
    /// # Panics
    ///
    /// Never: the fallback rate is positive by construction.
    #[must_use]
    pub fn refresh(&self) -> RefreshInterval {
        self.mode().period().unwrap_or_else(|| {
            RefreshInterval::from_millihz(60_000).expect("60 Hz is a positive rate")
        })
    }

    /// The renderer's output description (XRGB8888 sRGB scanout), at
    /// the output's layout origin — the renderer's framebuffer
    /// identity in a multi-output desktop (each output keeps its own
    /// canonical frame; the primary at the origin is the Phase 25
    /// doctrine, byte-identical).
    ///
    /// # Panics
    ///
    /// Never: the active mode passed the pipeline's own validation.
    #[must_use]
    pub fn output_desc(&self) -> OutputDesc {
        self.output_desc_sdr()
    }

    /// The renderer's output description under the *HDR mode* (Phase
    /// 31): an HDR-capable panel in `HdrPq` composites onto a PQ
    /// canvas (the renderer's cross-description pipeline blends every
    /// layer scene-linear and encodes PQ on the tail — SDR content
    /// rides at the BT.2408 reference white, HDR content at its
    /// negotiated peak). SDR (and every non-HDR panel) keeps the sRGB
    /// description — the Phase 25 bytes.
    ///
    /// Phase 38: the canvas carries the *negotiated* ceiling — the
    /// stack's brightest content clamped to the panel's effective
    /// peak (`negotiated_nits`) — as its `luminance_max`. The
    /// description is the honest record of what the canvas will hold;
    /// the renderer's per-layer tail does the actual rolloff work.
    /// `None` (no negotiation yet — the HDR stack is empty) keeps the
    /// static `pq_hdr()` description, the Phase 31 bytes.
    ///
    /// # Panics
    ///
    /// Never in-crate: the constructor's geometry is the live mode's,
    /// always nonzero (the same guarantee `output_desc` carries).
    #[must_use]
    pub fn output_desc_hdr(
        &self,
        hdr_mode: ldp_hdr::policy::OutputMode,
        negotiated_nits: Option<u32>,
    ) -> OutputDesc {
        if hdr_mode == ldp_hdr::policy::OutputMode::HdrPq && self.hdr.is_some() {
            let color = match negotiated_nits {
                Some(peak) if (1..=10_000).contains(&peak) => {
                    let mut c = ColorDescription::pq_hdr();
                    c.luminance_max = ldp_core::color::Luminance::from_nits(peak);
                    c
                }
                _ => ColorDescription::pq_hdr(),
            };
            let mode = self.mode();
            return OutputDesc::with_origin(
                u32::from(mode.hdisplay),
                u32::from(mode.vdisplay),
                FourCC::XRGB8888,
                color,
                self.layout,
            )
            .expect("the active mode is a valid output geometry");
        }
        self.output_desc_sdr()
    }

    /// The sRGB SDR description (the default).
    #[must_use]
    fn output_desc_sdr(&self) -> OutputDesc {
        let mode = self.mode();
        OutputDesc::with_origin(
            u32::from(mode.hdisplay),
            u32::from(mode.vdisplay),
            FourCC::XRGB8888,
            ColorDescription::srgb_sdr(),
            self.layout,
        )
        .expect("the active mode is a valid output geometry")
    }

    /// The output's logical size (physical / scale) — the canvas the
    /// positioning shell's doctrine resolves on.
    #[must_use]
    pub fn logical_size(&self) -> (u32, u32) {
        let mode = self.mode();
        (
            self.scale.unscale_px(u32::from(mode.hdisplay)),
            self.scale.unscale_px(u32::from(mode.vdisplay)),
        )
    }

    /// Output bounds rooted at the layout position.
    #[must_use]
    pub fn bounds(&self) -> Rect {
        let mode = self.mode();
        Rect::new(
            self.layout.0,
            self.layout.1,
            u32::from(mode.hdisplay),
            u32::from(mode.vdisplay),
        )
    }

    /// The bind-time cascade: `(event name, args)` in emission order.
    ///
    /// # Panics
    ///
    /// Never through the public API: the geometry and identity data
    /// were validated when the output was built.
    #[must_use]
    pub fn cascade(&self) -> Vec<(&'static str, Vec<Value>)> {
        let mut out = Vec::with_capacity(12);
        out.push(("capabilities", self.capabilities_args()));
        out.push(("geometry", self.geometry_args()));
        for (i, m) in self.connector.modes.iter().enumerate() {
            out.push(("mode", self.mode_args(m, i)));
        }
        out.push((
            "current_mode",
            vec![Value::Uint32(
                u32::try_from(self.mode_index).unwrap_or(0) + 1,
            )],
        ));
        // The fractional truth (Phase 31): the real factor, Q8.8 —
        // 384 reads back as 1.5x. Identity keeps the Phase 25 bytes.
        out.push(("scale", vec![Value::Uint32(self.scale.to_q8())]));
        out.push(("transform", vec![Value::Enum(1)])); // normal
        out.push(("color", Self::color_args()));
        // The HDR truth (Phase 31): `--hdr` advertises the panel's
        // capabilities — static metadata (this cascade), PQ, wide
        // gamut, and the peak (mastering max and supported CLL, the
        // negotiation's ceiling). `None` keeps the Phase 25 empties.
        let hdr_args = match &self.hdr {
            Some(caps) => {
                let mut bits = 0u32;
                bits |= 1 << 0; // static_metadata: this event exists
                if caps.pq {
                    bits |= 1 << 1;
                }
                if caps.hlg {
                    bits |= 1 << 2;
                }
                if caps.wide_gamut {
                    bits |= 1 << 3;
                }
                bits |= 1 << 4; // tone_mapping: the BT.2390 operator
                let peak = caps.max_luminance.as_wire_units();
                vec![
                    Value::Bitset(low_bits(bits)),
                    Value::Uint32(peak),
                    Value::Uint32(peak),
                ]
            }
            None => vec![
                Value::Bitset(Bitset128::EMPTY),
                Value::Uint32(0),
                Value::Uint32(0),
            ],
        };
        out.push(("hdr_caps", hdr_args));
        out.push(("vrr", self.vrr_args()));
        out.push(("name", vec![Value::String(self.name.clone().into())]));
        out
    }

    /// `capabilities` arguments: vrr/atomic/overlay/cursor bits.
    fn capabilities_args(&self) -> Vec<Value> {
        let mut caps = 0u32;
        if self.vrr.is_some() {
            caps |= 1 << 0; // vrr
        }
        caps |= 1 << 2; // atomic
        if self.overlay_planes {
            caps |= 1 << 3;
        }
        if self.cursor_plane {
            caps |= 1 << 4;
        }
        vec![Value::Bitset(low_bits(caps))]
    }

    /// `geometry` arguments: layout, physical size, subpixel, identity.
    fn geometry_args(&self) -> Vec<Value> {
        let (make, model, serial) = self.identity_strings();
        vec![
            Value::Int32(self.layout.0),
            Value::Int32(self.layout.1),
            Value::Int32(i32::try_from(self.connector.mm_width).unwrap_or(0)),
            Value::Int32(i32::try_from(self.connector.mm_height).unwrap_or(0)),
            Value::Enum(subpixel_wire(self.connector.subpixel)),
            Value::String(make.into()),
            Value::String(model.into()),
            Value::String(serial.into()),
        ]
    }

    /// `mode` arguments for mode list index `i`.
    fn mode_args(&self, m: &Mode, i: usize) -> Vec<Value> {
        let mut flags = 0u32;
        if m.kind.0 & ModeType::PREFERRED.0 != 0 {
            flags |= 1 << 0;
        }
        if i == self.mode_index {
            flags |= 1 << 1;
        }
        if m.flags.0 & ModeFlags::INTERLACE.0 != 0 {
            flags |= 1 << 2;
        }
        vec![
            Value::Uint32(u32::try_from(i).unwrap_or(0) + 1),
            Value::Bitset(low_bits(flags)),
            Value::Uint32(u32::from(m.hdisplay)),
            Value::Uint32(u32::from(m.vdisplay)),
            Value::Uint32(u32::try_from(m.refresh_millihz().min(u64::from(u32::MAX))).unwrap_or(0)),
        ]
    }

    /// `color` arguments: the sRGB SDR pipeline description.
    fn color_args() -> Vec<Value> {
        let color = ColorDescription::srgb_sdr();
        vec![
            Value::Enum(color.primaries.to_wire()),
            Value::Enum(color.transfer.to_wire()),
            Value::Enum(color.range.to_wire()),
            Value::Uint32(color.luminance_min.as_wire_units()),
            Value::Uint32(color.luminance_max.as_wire_units()),
            Value::Uint32(color.reference_white.as_wire_units()),
        ]
    }

    /// `vrr` arguments: the adaptive-sync window in millihertz.
    fn vrr_args(&self) -> Vec<Value> {
        match self.vrr {
            Some((min_ns, max_ns)) => vec![
                Value::Uint32(u32::try_from(1_000_000_000_000u64 / max_ns).unwrap_or(0)),
                Value::Uint32(u32::try_from(1_000_000_000_000u64 / min_ns).unwrap_or(0)),
                Value::Bitset(low_bits(1)), // seamless
            ],
            None => vec![
                Value::Uint32(0),
                Value::Uint32(0),
                Value::Bitset(Bitset128::EMPTY),
            ],
        }
    }

    /// `(make, model, serial)` strings from EDID identity.
    fn identity_strings(&self) -> (String, String, String) {
        match &self.connector.identity {
            Some(id) => (
                id.manufacturer.clone(),
                id.monitor_name.clone(),
                format!("{}", id.serial),
            ),
            None => (String::new(), String::new(), String::new()),
        }
    }
}

/// Subpixel layout wire value.
fn subpixel_wire(s: Subpixel) -> u32 {
    match s {
        Subpixel::None => 2,
        Subpixel::HorizontalRgb => 3,
        Subpixel::HorizontalBgr => 4,
        Subpixel::VerticalRgb => 5,
        Subpixel::VerticalBgr => 6,
        _ => 1, // Unknown and future layouts
    }
}

/// Connector kind display name (`eDP`, `DP`, `HDMI-A`).
fn connector_kind_name(kind: ConnectorType) -> &'static str {
    match kind {
        ConnectorType::Vga => "VGA",
        ConnectorType::DviI => "DVI-I",
        ConnectorType::DviD => "DVI-D",
        ConnectorType::DviA => "DVI-A",
        ConnectorType::Composite => "Composite",
        ConnectorType::SVideo => "SVIDEO",
        ConnectorType::Lvds => "LVDS",
        ConnectorType::Component => "Component",
        ConnectorType::DisplayPort => "DP",
        ConnectorType::HdmiA => "HDMI-A",
        ConnectorType::HdmiB => "HDMI-B",
        ConnectorType::EmbeddedDisplayPort => "eDP",
        ConnectorType::Virtual => "Virtual",
        ConnectorType::Dsi => "DSI",
        ConnectorType::Usb => "USB",
        _ => "Unknown",
    }
}

/// The connector's kernel-style index (its snapshot carries it; the
/// fallback re-walks the topology for same-kind ordering).
fn type_index_of_parts(backend: &dyn KmsBackend, id: ConnectorId, snapshot_index: u32) -> u32 {
    if snapshot_index > 0 {
        return snapshot_index;
    }
    // Snapshot index zero is unknown here: count same-kind connectors
    // up to ours — the kernel's naming rule.
    let Ok(top) = backend.topology() else {
        return 1;
    };
    let mut index = 1;
    for c in &top.connectors {
        if *c == id {
            return index;
        }
        if let Ok(info) = backend.connector_info(*c) {
            if info.kind == connector_kind_of_backend(backend, id) {
                index += 1;
            }
        }
    }
    index
}

fn connector_kind_of_backend(backend: &dyn KmsBackend, id: ConnectorId) -> ConnectorType {
    backend
        .connector_info(id)
        .map_or(ConnectorType::Other(0), |c| c.kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_output_cascade_is_complete_and_ordered() {
        let dev = MockDevice::laptop_dual();
        let out = OutputGlobal::from_mock(&dev, (0, 0)).expect("eDP-1 connected");
        assert_eq!(out.name, "eDP-1");
        assert_eq!(out.connector.id.raw(), 91);
        assert_eq!(out.crtc.raw(), 42);
        let cascade = out.cascade();
        let names: Vec<&str> = cascade.iter().map(|(n, _)| *n).collect();
        assert_eq!(
            names[..3],
            ["capabilities", "geometry", "mode"],
            "capabilities, geometry, then modes"
        );
        assert_eq!(names.last().copied(), Some("name"));
        // Two modes on the panel (1080p60 preferred + 59.94).
        let mode_events = cascade.iter().filter(|(n, _)| *n == "mode").count();
        assert_eq!(mode_events, 2);
        // Preferred first, carrying the preferred + current bits.
        let (_, mode_args) = cascade.iter().find(|(n, _)| *n == "mode").unwrap();
        let Value::Bitset(b) = &mode_args[1] else {
            panic!("flags")
        };
        assert_eq!(b.to_words()[0], 0b0011, "preferred | current");
        let Value::Uint32(w) = mode_args[2] else {
            panic!("w")
        };
        assert_eq!(w, 1920);
        // Geometry identity from EDID.
        let (_, geo) = cascade.iter().find(|(n, _)| *n == "geometry").unwrap();
        let Value::String(make) = &geo[5] else {
            panic!("make")
        };
        assert_eq!(&**make, "LTI");
        let Value::String(model) = &geo[6] else {
            panic!("model")
        };
        assert_eq!(&**model, "Lion Panel 15");
        let Value::Int32(mmw) = geo[2] else {
            panic!("mmw")
        };
        assert_eq!(mmw, 309);
        // VRR window: 48..144 Hz.
        let (_, vrr) = cascade.iter().find(|(n, _)| *n == "vrr").unwrap();
        let Value::Uint32(min) = vrr[0] else {
            panic!("min")
        };
        let Value::Uint32(max) = vrr[1] else {
            panic!("max")
        };
        assert_eq!((min, max), (48_000, 144_000));
    }

    #[test]
    fn output_geometry_matches_renderer_desc() {
        let dev = MockDevice::laptop_dual();
        let out = OutputGlobal::from_mock(&dev, (0, 0)).unwrap();
        let desc = out.output_desc();
        assert_eq!((desc.width, desc.height), (1920, 1080));
        assert_eq!(out.bounds(), Rect::new(0, 0, 1920, 1080));
        // 60 Hz exact: 148.5 MHz over 2200x1125.
        assert_eq!(out.refresh().as_ns(), 16_666_666);
    }
}
