//! The KMS serve choreography — bring-up, delivery, teardown.
//!
//! Phase 25's state machine: the *same* request sequence a real
//! scanout service submits, expressed over [`KmsBackend`] so the mock
//! device proves it in CI and the libdrm backend runs it for real on
//! hardware. Three pieces:
//!
//! * [`select_pipeline`] — the output choice: a connected connector
//!   with modes (its preferred mode), the first CRTC, and the primary
//!   plane that can feed it. Every caller (the headless bring-up, the
//!   DRM serve loop, the probe) asks the same question once.
//! * [`enable`] / [`disable`] — the applied atomic commits that light
//!   up and extinguish a pipeline. `enable` is the Phase 24 rehearsal
//!   request minus `TEST_ONLY` — the same validated shape, submitted
//!   for real; `disable` is its mirror: plane off, CRTC off, connector
//!   unbound, `ALLOW_MODESET`, applied. A compositor that exits leaves
//!   the display dark, not wedged.
//! * [`write_frame_rows`] — the CPU delivery pass: a composed frame
//!   (premultiplied ARGB words) into a *pitch-carrying* scanout
//!   mapping. The kernel's dumb-buffer pitch may exceed `width * 4`;
//!   rows are copied independently so padding bytes stay untouched.
//!
//! The serve *loop* itself (render → flip → land → release) lives in
//! the compositor binary over [`crate::driver::DisplayDriver`] — this
//! module holds what both backends share: the choreography.

#![forbid(unsafe_code)]

use crate::atomic::AtomicRequest;
use crate::backend::KmsBackend;
use crate::commit::{CommitFlags, DstRect, SrcRect};
use crate::connector::ConnectorStatus;
use crate::error::Result;
use crate::ids::{ConnectorId, CrtcId, FbId, PlaneId};
use crate::mode::{Mode, ModeType};
use crate::plane::PlaneType;
use crate::timing::{self, SynthRequest, TimingFamily};

/// The pipeline a scanout service drives: one connector, one CRTC, one
/// primary plane, one mode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pipeline {
    /// The connected connector being driven.
    pub connector: ConnectorId,
    /// The CRTC scanning out.
    pub crtc: CrtcId,
    /// The primary plane feeding the CRTC.
    pub plane: PlaneId,
    /// The chosen (preferred when marked) mode.
    pub mode: Mode,
}

impl Pipeline {
    /// The mode's width in pixels.
    #[must_use]
    pub fn width(&self) -> u32 {
        u32::from(self.mode.hdisplay)
    }

    /// The mode's height in pixels.
    #[must_use]
    pub fn height(&self) -> u32 {
        u32::from(self.mode.vdisplay)
    }
}

/// Select the pipeline to serve: the first connected connector with
/// modes (its preferred mode, else the first), the first CRTC, and a
/// primary plane whose `possible_crtcs` covers that CRTC.
///
/// `None` is the honest "nothing to display" — no connected connector,
/// no CRTC, or no primary plane for it.
#[must_use]
pub fn select_pipeline(backend: &dyn KmsBackend) -> Option<Pipeline> {
    select_pipeline_sized(backend, None).ok()
}

/// An operator-requested output size (the `--resolution WxH` forcing
/// flag): only modes whose active area is exactly this size are
/// candidates.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ModeSize {
    /// Active width in pixels.
    pub width: u32,
    /// Active height in pixels.
    pub height: u32,
}

impl ModeSize {
    /// Parse the CLI spelling `WxH` (e.g. `1920x1080`).
    ///
    /// # Errors
    /// `Err(String)` for malformed text — the argv layer's error shape.
    pub fn parse(text: &str) -> Result<Self, String> {
        let (w, h) = text.split_once('x').ok_or_else(|| {
            format!("bad resolution '{text}' (want WIDTHxHEIGHT, e.g. 1920x1080)")
        })?;
        let width: u32 = w
            .parse()
            .map_err(|_| format!("bad width '{w}' in '{text}' (a whole number of pixels)"))?;
        let height: u32 = h
            .parse()
            .map_err(|_| format!("bad height '{h}' in '{text}' (a whole number of pixels)"))?;
        if width == 0 || height == 0 {
            return Err(format!(
                "degenerate resolution '{text}' (both axes must be nonzero)"
            ));
        }
        Ok(Self { width, height })
    }

    /// The `WxH` spelling (report lines).
    #[must_use]
    pub fn as_str(&self) -> String {
        format!("{}x{}", self.width, self.height)
    }
}

/// Why a sized selection could not produce a pipeline.
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum SelectError {
    /// Nothing to display: no connected connector with modes, no
    /// CRTC, or no primary plane for it — the same honesty as
    /// [`select_pipeline`]'s `None`.
    NoDisplay,
    /// The requested size is offered by no connected connector. Carries
    /// the wanted size and every distinct size that *is* offered
    /// (largest first) — the operator's honest menu, so the failure
    /// teaches the fix (`--resolution 1920x1080` etc.).
    NoModeFor {
        /// What the operator asked for.
        wanted: ModeSize,
        /// Every distinct offered size, largest area first.
        offered: Vec<(u32, u32)>,
    },
}

impl core::fmt::Display for SelectError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NoDisplay => f.write_str("no connected connector with modes on the device"),
            Self::NoModeFor { wanted, offered } => {
                let menu = offered
                    .iter()
                    .map(|(w, h)| format!("{w}x{h}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                // Phase 42: the honest menu now teaches the foundry's
                // escape too — a size no connector offers is a pour
                // away, not a dead end.
                write!(
                    f,
                    "no {}x{} mode on any connected connector (offered: {menu}; \
                     pour one with --synth {}x{}@HZ)",
                    wanted.width, wanted.height, wanted.width, wanted.height
                )
            }
        }
    }
}

/// Select the pipeline to serve, optionally forced to one output size.
///
/// The unsized path is [`select_pipeline`]'s doctrine verbatim (the
/// preferred mode, else the first). The sized path — `--resolution` —
/// filters each connected connector's modes to the exact active area
/// and picks the best of those (preferred flag first, then the higher
/// refresh), so `--resolution 1920x1080` on a panel offering 1080p60
/// and 1080p59.94 takes the 60 Hz marking. A wanted size no connector
/// offers is the typed [`SelectError::NoModeFor`] — never a silent
/// nearest-neighbor guess (the "different display sizes" doctrine:
/// the operator sees the true menu).
///
/// # Errors
/// [`SelectError`] — see its variants for the honesty contract.
pub fn select_pipeline_sized(
    backend: &dyn KmsBackend,
    wanted: Option<ModeSize>,
) -> Result<Pipeline, SelectError> {
    let top = backend.topology().map_err(|_| SelectError::NoDisplay)?;
    let connector = *top
        .connectors
        .iter()
        .find(|c| {
            backend.connector_info(**c).is_ok_and(|info| {
                info.status == ConnectorStatus::Connected && !info.modes.is_empty()
            })
        })
        .ok_or(SelectError::NoDisplay)?;
    let info = backend
        .connector_info(connector)
        .map_err(|_| SelectError::NoDisplay)?;
    let mode = pick_mode(&info.modes, wanted).ok_or_else(|| {
        // The honest menu: every distinct offered size, largest first.
        let mut offered: Vec<(u32, u32)> = info
            .modes
            .iter()
            .map(|m| (u32::from(m.hdisplay), u32::from(m.vdisplay)))
            .collect();
        offered.sort_unstable();
        offered.dedup();
        offered.sort_by_key(|&(w, h)| std::cmp::Reverse(u64::from(w) * u64::from(h)));
        SelectError::NoModeFor {
            wanted: wanted.unwrap_or(ModeSize {
                width: 0,
                height: 0,
            }),
            offered,
        }
    })?;
    let crtc = *top.crtcs.first().ok_or(SelectError::NoDisplay)?;
    let index = top.crtc_index(crtc).ok_or(SelectError::NoDisplay)?;
    let plane = *top
        .planes
        .iter()
        .find(|p| {
            backend
                .plane_info(**p)
                .is_ok_and(|info| info.kind == PlaneType::Primary && info.feeds_crtc_index(index))
        })
        .ok_or(SelectError::NoDisplay)?;
    Ok(Pipeline {
        connector,
        crtc,
        plane,
        mode,
    })
}

/// Select *every* pipeline the device can serve at once — the
/// multi-output doctrine (Phase 31): one pipeline per connected
/// connector with modes, in resource-list order, each on its own
/// unused CRTC with its own unused primary plane, until the device
/// runs out of CRTCs or planes (the honest cap — a third monitor on a
/// two-CRTC device simply has no pipeline to drive it, and the answer
/// says so by omission, never by failing the rest).
///
/// The first pipeline of the result is always exactly what
/// [`select_pipeline`] picks on the same device (the allocator walks
/// the same order with the same rules), so a single-output world built
/// over this function is byte-identical to the Phase 25 doctrine.
///
/// The sized doctrine rides per-connector: a connector that offers the
/// forced size serves it (best exact match — preferred flag, then
/// refresh); a connector that does not is *skipped*, not fatal — the
/// others still serve. The typed [`SelectError::NoModeFor`] fires only
/// when *no* connector offers the size, and its menu is the union of
/// every connected connector's offerings (largest area first).
///
/// `Ok(vec![])` never happens: either at least one pipeline serves, or
/// the error explains why nothing does.
///
/// # Errors
/// [`SelectError`] — see its variants for the honesty contract.
pub fn select_pipelines(
    backend: &dyn KmsBackend,
    wanted: Option<ModeSize>,
) -> Result<Vec<Pipeline>, SelectError> {
    let top = backend.topology().map_err(|_| SelectError::NoDisplay)?;
    // Connector snapshots in resource-list order; connected-with-modes
    // only (the same candidate predicate the single path uses).
    let infos: Vec<crate::connector::ConnectorInfo> = top
        .connectors
        .iter()
        .filter_map(|c| backend.connector_info(*c).ok())
        .filter(|info| info.status == ConnectorStatus::Connected && !info.modes.is_empty())
        .collect();
    if infos.is_empty() {
        return Err(SelectError::NoDisplay);
    }
    let mut used_crtcs: Vec<CrtcId> = Vec::new();
    let mut used_planes: Vec<PlaneId> = Vec::new();
    let mut pipelines: Vec<Pipeline> = Vec::new();
    let mut offered: Vec<(u32, u32)> = Vec::new();
    for info in &infos {
        let Some(mode) = pick_mode(&info.modes, wanted) else {
            // A sized miss on this connector: its sizes join the union
            // menu (the error's evidence), and the walk continues —
            // the other connectors may still serve.
            if wanted.is_some() {
                offered.extend(
                    info.modes
                        .iter()
                        .map(|m| (u32::from(m.hdisplay), u32::from(m.vdisplay))),
                );
            }
            continue;
        };
        // The allocation: the first unused CRTC that an unused primary
        // plane can feed — resource order, the single path's rule
        // applied per-CRTC.
        let allocation = top.crtcs.iter().find_map(|crtc| {
            if used_crtcs.contains(crtc) {
                return None;
            }
            let index = top.crtc_index(*crtc)?;
            let plane = top.planes.iter().find(|p| {
                !used_planes.contains(p)
                    && backend.plane_info(**p).is_ok_and(|info| {
                        info.kind == PlaneType::Primary && info.feeds_crtc_index(index)
                    })
            })?;
            Some((*crtc, *plane))
        });
        let Some((crtc, plane)) = allocation else {
            // The device is out of CRTCs or primary planes: the honest
            // cap — the connectors that follow stay unserved.
            break;
        };
        used_crtcs.push(crtc);
        used_planes.push(plane);
        pipelines.push(Pipeline {
            connector: info.id,
            crtc,
            plane,
            mode,
        });
    }
    if pipelines.is_empty() {
        // Nothing served. With a forced size, the honest menu is the
        // union over every connected connector that had modes to offer.
        if wanted.is_some() {
            offered.sort_unstable();
            offered.dedup();
            offered.sort_by_key(|&(w, h)| std::cmp::Reverse(u64::from(w) * u64::from(h)));
            if !offered.is_empty() {
                return Err(SelectError::NoModeFor {
                    wanted: wanted.unwrap_or(ModeSize {
                        width: 0,
                        height: 0,
                    }),
                    offered,
                });
            }
        }
        return Err(SelectError::NoDisplay);
    }
    Ok(pipelines)
}

/// The mode choice: unsized = the pinned doctrine (preferred else
/// first); sized = best of the exact-area matches (preferred flag,
/// then refresh).
fn pick_mode(modes: &[Mode], wanted: Option<ModeSize>) -> Option<Mode> {
    match wanted {
        None => modes
            .iter()
            .position(|m| m.kind.0 & ModeType::PREFERRED.0 != 0)
            .or(Some(0))
            .map(|i| modes[i].clone()),
        Some(size) => modes
            .iter()
            .filter(|m| u32::from(m.hdisplay) == size.width && u32::from(m.vdisplay) == size.height)
            .max_by(|a, b| {
                (a.is_preferred(), a.refresh_millihz())
                    .cmp(&(b.is_preferred(), b.refresh_millihz()))
            })
            .cloned(),
    }
}

/// Why a synthesis could not produce a pipeline (Phase 42 — the
/// foundry's own honest refusals, typed like [`SelectError`]).
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum SynthError {
    /// Nothing to pour onto: no connected connector with modes, no
    /// CRTC, or no primary plane — [`SelectError::NoDisplay`]'s
    /// honesty.
    NoDisplay,
    /// The ask itself is degenerate (a zero axis, an axis beyond the
    /// 16-bit mode wire, a family's own domain limit) — the CLI
    /// layer refuses these first; the typed arm is the API's own
    /// guard.
    Degenerate {
        /// What was asked for.
        wanted: ModeSize,
        /// The VESA family the pour was asked to follow (the error
        /// names it — the refusal is the family's own).
        family: TimingFamily,
    },
    /// The pour exceeds the connector's EDID-declared pixel-clock
    /// ceiling (the range-limits descriptor). Never a silent clamp —
    /// the panel's own declaration is the truth, and the refusal
    /// names it (the `pixel-clock-ceiling` quirk's doctrine).
    ClockCeiling {
        /// What was asked for.
        wanted: ModeSize,
        /// The pour's pixel clock, kilohertz.
        synthesized_khz: u32,
        /// The declared maximum, kilohertz.
        declared_khz: u32,
    },
}

impl core::fmt::Display for SynthError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NoDisplay => f.write_str("no connected connector with modes on the device"),
            Self::Degenerate { wanted, family } => write!(
                f,
                "degenerate synthesis {}x{} for {family} (the family's formula does not serve it)",
                wanted.width, wanted.height
            ),
            Self::ClockCeiling {
                wanted,
                synthesized_khz,
                declared_khz,
            } => write!(
                f,
                "the {}x{} pour clocks {} kHz over the panel's declared {} kHz ceiling \
                 (the EDID range limits — pick a smaller size or refresh)",
                wanted.width, wanted.height, synthesized_khz, declared_khz
            ),
        }
    }
}

/// Pour one synthesized pipeline (Phase 42 — the mode foundry's
/// single-output doctrine).
///
/// The pour is *always* a pour: `--synth` exists for timings the
/// firmware's list does not carry (a size, or a size at a refresh the
/// offered modes of that size lack) — an offered mode is
/// `--resolution`'s doctrine, not the foundry's. The connector's
/// EDID range limits gate the pour before it is ever proposed (a
/// clock above the declared ceiling is the typed [`SynthError::ClockCeiling`]
/// refusal — never a silent clamp); the KMS engine validates the
/// `USERDEF` mode against its own limits again at commit time (two
/// independent gates, both honest).
///
/// # Errors
/// [`SynthError`] — see its variants for the honesty contract.
pub fn select_pipeline_synth(
    backend: &dyn KmsBackend,
    request: SynthRequest,
) -> Result<Pipeline, SynthError> {
    let top = backend.topology().map_err(|_| SynthError::NoDisplay)?;
    let connector = *top
        .connectors
        .iter()
        .find(|c| {
            backend.connector_info(**c).is_ok_and(|info| {
                info.status == ConnectorStatus::Connected && !info.modes.is_empty()
            })
        })
        .ok_or(SynthError::NoDisplay)?;
    let info = backend
        .connector_info(connector)
        .map_err(|_| SynthError::NoDisplay)?;
    let mode = pour_for(&info, request)?;
    let crtc = *top.crtcs.first().ok_or(SynthError::NoDisplay)?;
    let index = top.crtc_index(crtc).ok_or(SynthError::NoDisplay)?;
    let plane = *top
        .planes
        .iter()
        .find(|p| {
            backend
                .plane_info(**p)
                .is_ok_and(|info| info.kind == PlaneType::Primary && info.feeds_crtc_index(index))
        })
        .ok_or(SynthError::NoDisplay)?;
    Ok(Pipeline {
        connector,
        crtc,
        plane,
        mode,
    })
}

/// Pour *every* pipeline the device can serve at once — the mode
/// foundry's multi-output doctrine (Phase 42): every connected
/// connector with modes gets its own pour (a desktop where every
/// display serves the synthesized size), each on its own unused CRTC
/// and primary plane under [`select_pipelines`]'s allocation rules.
///
/// A connector whose EDID-declared ceiling refuses the pour is
/// *skipped*, not fatal — the others still serve (the sized
/// doctrine's per-connector honesty); nothing serving at all
/// surfaces the first refusal (or [`SynthError::NoDisplay`] when
/// there was nothing to pour onto).
///
/// # Errors
/// [`SynthError`] — see its variants for the honesty contract.
pub fn select_pipelines_synth(
    backend: &dyn KmsBackend,
    request: SynthRequest,
) -> Result<Vec<Pipeline>, SynthError> {
    let top = backend.topology().map_err(|_| SynthError::NoDisplay)?;
    let infos: Vec<crate::connector::ConnectorInfo> = top
        .connectors
        .iter()
        .filter_map(|c| backend.connector_info(*c).ok())
        .filter(|info| info.status == ConnectorStatus::Connected && !info.modes.is_empty())
        .collect();
    if infos.is_empty() {
        return Err(SynthError::NoDisplay);
    }
    let mut used_crtcs: Vec<CrtcId> = Vec::new();
    let mut used_planes: Vec<PlaneId> = Vec::new();
    let mut pipelines: Vec<Pipeline> = Vec::new();
    let mut refusal: Option<SynthError> = None;
    for info in &infos {
        let mode = match pour_for(info, request) {
            Ok(mode) => mode,
            Err(e @ SynthError::ClockCeiling { .. }) => {
                // The honest skip: this connector's declared ceiling
                // refuses the pour — the first refusal is remembered
                // for the nothing-served case.
                refusal = refusal.or(Some(e));
                continue;
            }
            Err(e) => return Err(e),
        };
        let allocation = top.crtcs.iter().find_map(|crtc| {
            if used_crtcs.contains(crtc) {
                return None;
            }
            let index = top.crtc_index(*crtc)?;
            let plane = top.planes.iter().find(|p| {
                !used_planes.contains(p)
                    && backend.plane_info(**p).is_ok_and(|info| {
                        info.kind == PlaneType::Primary && info.feeds_crtc_index(index)
                    })
            })?;
            Some((*crtc, *plane))
        });
        let Some((crtc, plane)) = allocation else {
            break; // out of CRTCs or planes: the honest cap
        };
        used_crtcs.push(crtc);
        used_planes.push(plane);
        pipelines.push(Pipeline {
            connector: info.id,
            crtc,
            plane,
            mode,
        });
    }
    if pipelines.is_empty() {
        return Err(refusal.unwrap_or(SynthError::NoDisplay));
    }
    Ok(pipelines)
}

/// The per-connector pour, gated by the connector's own EDID range
/// limits (the panel's declared envelope — the first of the foundry's
/// two gates).
fn pour_for(
    info: &crate::connector::ConnectorInfo,
    request: SynthRequest,
) -> Result<Mode, SynthError> {
    let wanted = ModeSize {
        width: request.width,
        height: request.height,
    };
    let family = request.family;
    let mode = timing::pour(request).ok_or(SynthError::Degenerate { wanted, family })?;
    if let Some(declared) = info
        .identity()
        .and_then(|id| id.ranges.map(|r| r.max_clock_khz))
    {
        if mode.clock_khz > declared {
            return Err(SynthError::ClockCeiling {
                wanted,
                synthesized_khz: mode.clock_khz,
                declared_khz: declared,
            });
        }
    }
    Ok(mode)
}

/// The applied enable request: bind the connector, activate the CRTC
/// with the mode, scan out `fb` on the primary plane. The full
/// pipeline state in one commit — `ALLOW_MODESET` because bring-up
/// changes display state, `PAGE_FLIP_EVENT` so the timeline latches at
/// the first vblank.
///
/// # Panics
///
/// Never in-crate: the rects are mode-shaped by construction (the
/// builder's own validation fails first on degenerate geometry).
#[must_use]
pub fn enable_request(pipeline: &Pipeline, fb: FbId) -> AtomicRequest {
    let (w, h) = (pipeline.width(), pipeline.height());
    AtomicRequest::new()
        .flag(CommitFlags::ALLOW_MODESET)
        .flag(CommitFlags::PAGE_FLIP_EVENT)
        .connector_bind(pipeline.connector, pipeline.crtc)
        .crtc_active(pipeline.crtc, true)
        .crtc_mode(pipeline.crtc, &pipeline.mode)
        .plane_on(
            pipeline.plane,
            pipeline.crtc,
            fb,
            SrcRect::new(0, 0, w, h).expect("mode-sized source"),
            DstRect::new(0, 0, w, h).expect("mode-sized destination"),
        )
}

/// The applied disable request: release the plane, deactivate the CRTC,
/// unbind the connector — the teardown mirror of [`enable_request`],
/// leaving the display dark and the objects releasable.
#[must_use]
pub fn disable_request(pipeline: &Pipeline) -> AtomicRequest {
    AtomicRequest::new()
        .flag(CommitFlags::ALLOW_MODESET)
        .plane_off(pipeline.plane)
        .crtc_active(pipeline.crtc, false)
        .connector_unbind(pipeline.connector)
}

/// Submit the enable commit (applied — never `TEST_ONLY`).
///
/// # Errors
/// The backend's typed rejection (unknown objects, malformed shape, a
/// busy pipeline, modeset without master rights).
pub fn enable(backend: &mut dyn KmsBackend, pipeline: &Pipeline, fb: FbId) -> Result<()> {
    let request = enable_request(pipeline, fb);
    backend.commit(&request).map(|_| ())
}

/// Submit the disable commit (applied).
///
/// # Errors
/// The backend's typed rejection.
pub fn disable(backend: &mut dyn KmsBackend, pipeline: &Pipeline) -> Result<()> {
    let request = disable_request(pipeline);
    backend.commit(&request).map(|_| ())
}

/// Copy a composed frame into a pitch-carrying scanout mapping.
///
/// `frame` holds `width * height` premultiplied ARGB words (the
/// renderer's readout); `dst` is the mapping's bytes; `pitch` is the
/// row stride in bytes (the kernel's, at least `width * 4`). Rows copy
/// independently — padding between them stays as the kernel laid it
/// out. Words encode little-endian (the XRGB8888 byte layout on every
/// Linux target this crate runs on; the scanout format is a byte
/// order, not a pointer dereference).
///
/// # Panics
///
/// A frame, pitch, or mapping that underflows the geometry fails fast
/// here rather than scribbling out of bounds (callers pass mode-shaped
/// frames; the contract is asserted, not assumed).
pub fn write_frame_rows(dst: &mut [u8], pitch: usize, width: usize, height: usize, frame: &[u32]) {
    let row = width * 4;
    assert!(
        pitch >= row,
        "pitch {pitch} underflows the {width}-wide row ({row} bytes)"
    );
    assert!(
        frame.len() >= width * height,
        "frame of {} words underflows {width}x{height}",
        frame.len()
    );
    assert!(
        dst.len() >= pitch * height,
        "mapping of {} bytes underflows {height} rows at pitch {pitch}",
        dst.len()
    );
    for y in 0..height {
        let src_row = &frame[y * width..(y + 1) * width];
        let base = y * pitch;
        let dst_row = &mut dst[base..base + row];
        for (x, word) in src_row.iter().enumerate() {
            dst_row[x * 4..x * 4 + 4].copy_from_slice(&word.to_le_bytes());
        }
    }
}

/// Read a composed frame back out of a pitch-carrying scanout mapping
/// (the front-buffer oracle for mapped stores).
///
/// # Panics
///
/// The same fail-fast contract as [`write_frame_rows`].
#[must_use]
pub fn read_frame_rows(src: &[u8], pitch: usize, width: usize, height: usize) -> Vec<u32> {
    let row = width * 4;
    assert!(
        pitch >= row,
        "pitch {pitch} underflows the {width}-wide row"
    );
    assert!(
        src.len() >= pitch * height,
        "mapping of {} bytes underflows {height} rows at pitch {pitch}",
        src.len()
    );
    let mut out = Vec::with_capacity(width * height);
    for y in 0..height {
        let row_bytes = &src[y * pitch..y * pitch + row];
        out.extend(row_bytes.chunks_exact(4).map(|w| {
            u32::from(w[0])
                | (u32::from(w[1]) << 8)
                | (u32::from(w[2]) << 16)
                | (u32::from(w[3]) << 24)
        }));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pipeline() -> Pipeline {
        Pipeline {
            connector: ConnectorId::new(91).unwrap(),
            crtc: CrtcId::new(42).unwrap(),
            plane: PlaneId::new(50).unwrap(),
            mode: Mode::panel_1080p60(),
        }
    }

    #[test]
    fn enable_request_shape_is_the_rehearsal_minus_test_only() {
        let req = enable_request(&pipeline(), FbId::new(3001).unwrap());
        assert!(req.flags.allows_modeset());
        assert!(req.flags.wants_flip_event());
        assert!(!req.flags.is_test_only());
        // bind + active + mode + the 10-op plane enable.
        assert_eq!(req.ops.len(), 3 + 10);
        req.validate_shape().unwrap();
    }

    #[test]
    fn disable_request_releases_everything() {
        let req = disable_request(&pipeline());
        assert!(req.flags.allows_modeset());
        assert!(!req.flags.is_test_only());
        // plane off (2 ops) + crtc active + connector unbind.
        assert_eq!(req.ops.len(), 2 + 1 + 1);
        req.validate_shape().unwrap();
        // The plane's FB is 0 (released) and its CRTC 0 (unfed).
        assert!(req
            .ops
            .iter()
            .any(|o| o.name.as_str() == "FB_ID" && o.value == crate::props::PropValue::U64(0)));
    }

    #[test]
    fn row_copy_honors_padded_pitch() {
        let (w, h) = (4usize, 3usize);
        let frame: Vec<u32> = (0..12).map(|i| 0x0100_0000 + i as u32).collect();
        // Pitch with two bytes of per-row padding.
        let pitch = w * 4 + 2;
        let mut dst = vec![0xAAu8; pitch * h];
        write_frame_rows(&mut dst, pitch, w, h, &frame);
        for y in 0..h {
            let row = &dst[y * pitch..y * pitch + w * 4];
            let expect: Vec<u8> = frame[y * w..(y + 1) * w]
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect();
            assert_eq!(row, expect.as_slice(), "row {y} content");
            // Padding untouched.
            assert_eq!(&dst[y * pitch + w * 4..(y + 1) * pitch], &[0xAA, 0xAA]);
        }
        // Round-trip through the reader.
        assert_eq!(read_frame_rows(&dst, pitch, w, h), frame);
    }

    #[test]
    fn row_copy_with_exact_pitch() {
        let (w, h) = (3usize, 2usize);
        let frame: Vec<u32> = vec![0xFF00_1234, 0x80AA_BB77, 0x1122_3344, 0, 0, 0];
        let pitch = w * 4;
        let mut dst = vec![0u8; pitch * h];
        write_frame_rows(&mut dst, pitch, w, h, &frame);
        assert_eq!(read_frame_rows(&dst, pitch, w, h), frame[..w * h].to_vec());
    }

    #[test]
    fn underflowing_geometry_fails_fast() {
        let mut dst = vec![0u8; 8];
        let frame = vec![0u32; 2];
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            write_frame_rows(&mut dst, 4, 2, 1, &frame);
        }))
        .is_err());
    }

    #[test]
    fn mode_size_parses_and_rejects_honestly() {
        let s = ModeSize::parse("1920x1080").unwrap();
        assert_eq!((s.width, s.height), (1920, 1080));
        assert_eq!(s.as_str(), "1920x1080");
        assert!(ModeSize::parse("1080").is_err());
        assert!(ModeSize::parse("1920X1080").is_err());
        assert!(ModeSize::parse("axb").is_err());
        assert!(ModeSize::parse("0x1080").is_err());
        assert!(ModeSize::parse("1920x0").is_err());
        // The error teaches the spelling.
        assert!(ModeSize::parse("1080")
            .unwrap_err()
            .contains("WIDTHxHEIGHT"));
    }

    #[test]
    fn sized_selection_takes_the_best_exact_match() {
        use crate::mock::MockDevice;
        // A desktop panel: 4K60 preferred, plus 4K59.94, plus 1080p60.
        let mut device = MockDevice::laptop_dual();
        let top = device.topology().unwrap();
        let connector = top.connectors[0];
        let fourk = Mode::panel_4k60();
        let fourk_59 = {
            let mut m = Mode::panel_4k60();
            m.kind = ModeType::DRIVER;
            m
        };
        device.hotplug_connect(
            connector,
            vec![fourk, Mode::panel_1080p60(), fourk_59],
            vec![],
        );
        // Unsized: the doctrine verbatim (preferred = 4K60).
        let plain = select_pipeline_sized(&device, None).unwrap();
        assert_eq!((plain.width(), plain.height()), (3840, 2160));
        // Sized to the desktop classic: the 1080p mode.
        let sized = select_pipeline_sized(
            &device,
            Some(ModeSize {
                width: 1920,
                height: 1080,
            }),
        )
        .unwrap();
        assert_eq!((sized.width(), sized.height()), (1920, 1080));
        // Sized to 4K: preferred wins over the 59.94 sibling of the
        // same size.
        let fourk_sel = select_pipeline_sized(
            &device,
            Some(ModeSize {
                width: 3840,
                height: 2160,
            }),
        )
        .unwrap();
        assert_eq!(fourk_sel.mode.refresh_hz(), 60);
        assert!(fourk_sel.mode.is_preferred());
    }

    #[test]
    fn sized_miss_lists_the_offered_menu_largest_first() {
        use crate::mock::MockDevice;
        let mut device = MockDevice::laptop_dual();
        let top = device.topology().unwrap();
        let connector = top.connectors[0];
        device.hotplug_connect(
            connector,
            vec![Mode::panel_1080p60(), Mode::panel_4k60()],
            vec![],
        );
        let err = select_pipeline_sized(
            &device,
            Some(ModeSize {
                width: 2560,
                height: 1440,
            }),
        )
        .unwrap_err();
        // The Display spelling teaches the fix.
        let text = err.to_string();
        match err {
            SelectError::NoModeFor { wanted, offered } => {
                assert_eq!((wanted.width, wanted.height), (2560, 1440));
                assert_eq!(offered, vec![(3840, 2160), (1920, 1080)]);
            }
            other => panic!("expected NoModeFor, got {other:?}"),
        }
        assert!(text.contains("2560x1440"));
        assert!(text.contains("3840x2160, 1920x1080"));
    }

    #[test]
    fn sized_selection_without_display_is_no_display() {
        use crate::mock::MockDevice;
        // A machine whose every connector is gone: the honest
        // NoDisplay, whatever the operator asked for.
        let mut device = MockDevice::laptop_dual();
        let top = device.topology().unwrap();
        for c in top.connectors {
            device.hotplug_disconnect(c);
        }
        let err = select_pipeline_sized(
            &device,
            Some(ModeSize {
                width: 1920,
                height: 1080,
            }),
        )
        .unwrap_err();
        assert_eq!(err, SelectError::NoDisplay);
    }

    #[test]
    fn multi_selection_serves_every_connected_connector() {
        use crate::mock::MockDevice;
        // The preset's dual-screen machine: eDP-1 and HDMI-A-1 both
        // connected, two CRTCs, two primary planes — the allocator
        // serves both, one pipeline each, in resource order.
        let device = MockDevice::laptop_dual();
        let pipelines = select_pipelines(&device, None).unwrap();
        assert_eq!(pipelines.len(), 2);
        // Slot 0: eDP-1 on CRTC 42 driven by primary plane 50 — the
        // single doctrine verbatim.
        assert_eq!(pipelines[0].connector.raw(), 91);
        assert_eq!(pipelines[0].crtc.raw(), 42);
        assert_eq!(pipelines[0].plane.raw(), 50);
        assert_eq!((pipelines[0].width(), pipelines[0].height()), (1920, 1080));
        // Slot 1: HDMI-A-1 on the second CRTC with the second primary.
        assert_eq!(pipelines[1].connector.raw(), 93);
        assert_eq!(pipelines[1].crtc.raw(), 43);
        assert_eq!(pipelines[1].plane.raw(), 53);
        assert_eq!((pipelines[1].width(), pipelines[1].height()), (1920, 1080));
    }

    #[test]
    fn multi_selection_first_pipeline_is_the_single_doctrine() {
        use crate::mock::MockDevice;
        // The compatibility spine: the allocator's first answer is
        // exactly the single path's answer, so a single-output world
        // built over it never drifts from Phase 25.
        let device = MockDevice::laptop_dual();
        let single = select_pipeline(&device).unwrap();
        let multi = select_pipelines(&device, None).unwrap();
        assert_eq!(multi.first(), Some(&single));
    }

    #[test]
    fn multi_selection_is_capped_by_crtcs_honestly() {
        use crate::mock::MockDevice;
        // Three monitors, two CRTCs: the first two connectors serve
        // (resource order), the third is honestly unserved — the answer
        // says so by omission, never by failing the rest.
        let mut device = MockDevice::laptop_dual();
        let top = device.topology().unwrap();
        let dp = top.connectors[1]; // DP-1, disconnected in the preset
        device.hotplug_connect(dp, vec![Mode::panel_1080p60()], b"DPX".to_vec());
        let pipelines = select_pipelines(&device, None).unwrap();
        assert_eq!(pipelines.len(), 2, "the device has two CRTCs — the cap");
        assert_eq!(pipelines[0].connector.raw(), 91);
        // Resource order: DP-1 (92) serves before HDMI-A-1 (93) — the
        // honest cap leaves the third monitor unserved.
        assert_eq!(pipelines[1].connector.raw(), 92);
    }

    #[test]
    fn multi_sized_doctrine_serves_what_can_and_skips_the_rest() {
        use crate::mock::MockDevice;
        // HDMI offers 1280x720; eDP does not. The sized multi walk
        // serves HDMI at 720p and *skips* eDP (not fatal) — the
        // per-connector sized doctrine.
        let mut device = MockDevice::laptop_dual();
        let top = device.topology().unwrap();
        let edp = top.connectors[0];
        device.hotplug_connect(edp, vec![Mode::panel_1080p60()], vec![]);
        let pipelines = select_pipelines(
            &device,
            Some(ModeSize {
                width: 1280,
                height: 720,
            }),
        )
        .unwrap();
        assert_eq!(pipelines.len(), 1);
        assert_eq!(pipelines[0].connector.raw(), 93);
        assert_eq!((pipelines[0].width(), pipelines[0].height()), (1280, 720));
    }

    #[test]
    fn multi_sized_total_miss_offers_the_union_menu() {
        use crate::mock::MockDevice;
        // No connected connector offers the wanted size: the typed
        // NoModeFor with the *union* menu (both connectors' sizes,
        // largest first, deduplicated) — the operator's honest menu.
        let device = MockDevice::laptop_dual();
        let err = select_pipelines(
            &device,
            Some(ModeSize {
                width: 2560,
                height: 1440,
            }),
        )
        .unwrap_err();
        match err {
            SelectError::NoModeFor { wanted, offered } => {
                assert_eq!((wanted.width, wanted.height), (2560, 1440));
                // eDP offers 1920x1080 (twice); HDMI offers 1920x1080
                // and 1280x720 — the deduplicated union, largest first.
                assert_eq!(offered, vec![(1920, 1080), (1280, 720)]);
            }
            other => panic!("expected NoModeFor, got {other:?}"),
        }
    }

    #[test]
    fn multi_selection_without_display_is_no_display() {
        use crate::mock::MockDevice;
        let mut device = MockDevice::laptop_dual();
        let top = device.topology().unwrap();
        for c in top.connectors {
            device.hotplug_disconnect(c);
        }
        assert_eq!(
            select_pipelines(&device, None).unwrap_err(),
            SelectError::NoDisplay
        );
    }

    /// Phase 42 — the foundry pours: a 2560x1440@60 pour onto the
    /// preset's eDP panel (whose EDID declares a 600 MHz ceiling).
    /// The reduced-blanking arithmetic, hand-derived: htotal 2720
    /// (2560 + 160), vblank `ceil(0.0276 × 1440 / 0.9724)` = 41 lines
    /// → vtotal 1481 (the preset's own 144 Hz QHD fixture rides the
    /// same totals), clock `ceil(2720 × 1481 × 60000 / 10⁶)` =
    /// 241 700 kHz — under the ceiling, the pour serves.
    #[test]
    fn synth_pours_onto_the_first_connector() {
        use crate::mock::MockDevice;
        let device = MockDevice::laptop_dual();
        let pipeline = select_pipeline_synth(
            &device,
            SynthRequest {
                width: 2560,
                height: 1440,
                refresh_millihz: 60_000,
                family: TimingFamily::Rb,
            },
        )
        .unwrap();
        assert_eq!(pipeline.connector.raw(), 91); // eDP-1 first
        assert_eq!((pipeline.width(), pipeline.height()), (2560, 1440));
        assert_eq!(pipeline.mode.htotal, 2720);
        assert_eq!(pipeline.mode.vtotal, 1481);
        assert_eq!(pipeline.mode.clock_khz, 241_700);
        // The pour is a user-defined mode in the kernel's vocabulary.
        assert_eq!(
            pipeline.mode.kind.0 & ModeType::USERDEF.0,
            ModeType::USERDEF.0
        );
    }

    /// Phase 46 — the families pour through the same gate: an RB2
    /// 1080p60 ask (2000×1111, 133 320 kHz — the §3.4.3 economy)
    /// and a GTF one (2576×1118, 172 799 kHz) both serve the eDP
    /// panel under its 600 MHz ceiling, the family's own raster on
    /// the wire — the foundry's front door is the serve layer's own.
    #[test]
    fn synth_pours_every_family_through_the_gate() {
        use crate::mock::MockDevice;
        let device = MockDevice::laptop_dual();
        for (family, htotal, vtotal, clock) in [
            (TimingFamily::Rb2, 2000, 1111, 133_320),
            (TimingFamily::Cvt, 2576, 1120, 173_108),
            (TimingFamily::Gtf, 2576, 1118, 172_799),
        ] {
            let pipeline = select_pipeline_synth(
                &device,
                SynthRequest {
                    width: 1920,
                    height: 1080,
                    refresh_millihz: 60_000,
                    family,
                },
            )
            .unwrap();
            assert_eq!(pipeline.connector.raw(), 91, "eDP first for {family:?}");
            assert_eq!((pipeline.width(), pipeline.height()), (1920, 1080));
            assert_eq!(pipeline.mode.htotal, htotal, "{family:?}");
            assert_eq!(pipeline.mode.vtotal, vtotal, "{family:?}");
            assert_eq!(pipeline.mode.clock_khz, clock, "{family:?}");
            assert_eq!(
                pipeline.mode.kind.0 & ModeType::USERDEF.0,
                ModeType::USERDEF.0
            );
        }
    }

    /// The ceiling refusal: the preset's HDMI monitor declares a
    /// 340 MHz ceiling (the HDMI 1.4-era envelope) — a 4K60 pour
    /// (533 280 kHz) is refused with the typed error naming the
    /// declaration, never silently clamped. With eDP gone, HDMI is
    /// the first connector and the refusal surfaces.
    #[test]
    fn synth_refuses_above_the_declared_ceiling() {
        use crate::mock::MockDevice;
        let mut device = MockDevice::laptop_dual();
        let top = device.topology().unwrap();
        device.hotplug_disconnect(top.connectors[0]); // eDP gone
        let err = select_pipeline_synth(
            &device,
            SynthRequest {
                width: 3840,
                height: 2160,
                refresh_millihz: 60_000,
                family: TimingFamily::Rb,
            },
        )
        .unwrap_err();
        match err {
            SynthError::ClockCeiling {
                wanted,
                synthesized_khz,
                declared_khz,
            } => {
                assert_eq!((wanted.width, wanted.height), (3840, 2160));
                assert_eq!(synthesized_khz, 533_280);
                assert_eq!(declared_khz, 340_000);
            }
            other => panic!("expected ClockCeiling, got {other:?}"),
        }
        // The Display names the panel's own declaration.
        assert!(err.to_string().contains("340000 kHz"));
    }

    /// The multi doctrine: every connected connector gets its own
    /// pour — both preset displays serve the synthesized size, each
    /// on its own CRTC and primary plane.
    #[test]
    fn multi_synth_pours_per_connector() {
        use crate::mock::MockDevice;
        let device = MockDevice::laptop_dual();
        let pipelines = select_pipelines_synth(
            &device,
            SynthRequest {
                width: 2560,
                height: 1440,
                refresh_millihz: 60_000,
                family: TimingFamily::Rb,
            },
        )
        .unwrap();
        assert_eq!(pipelines.len(), 2);
        for p in &pipelines {
            assert_eq!((p.width(), p.height()), (2560, 1440));
            assert_eq!(p.mode.htotal, 2720);
            assert_eq!(p.mode.clock_khz, 241_700);
        }
        assert_eq!(pipelines[0].crtc.raw(), 42);
        assert_eq!(pipelines[1].crtc.raw(), 43);
    }

    /// The multi ceiling honesty: a connector whose declared ceiling
    /// refuses the pour is skipped — the others still serve; nothing
    /// serving surfaces the first refusal.
    #[test]
    fn multi_synth_ceiling_refusal_surfaces_when_nothing_serves() {
        use crate::mock::MockDevice;
        // HDMI refuses the 4K pour (340 MHz ceiling); eDP would serve
        // it — with eDP still connected both doctrines serve eDP.
        let device = MockDevice::laptop_dual();
        let pipelines = select_pipelines_synth(
            &device,
            SynthRequest {
                width: 3840,
                height: 2160,
                refresh_millihz: 60_000,
                family: TimingFamily::Rb,
            },
        )
        .unwrap();
        assert_eq!(pipelines.len(), 1); // eDP pours, HDMI skipped
        assert_eq!(pipelines[0].connector.raw(), 91);
        assert_eq!(pipelines[0].mode.clock_khz, 533_280);

        // Nothing serving: the first refusal is the honest answer.
        let mut device = MockDevice::laptop_dual();
        let top = device.topology().unwrap();
        device.hotplug_disconnect(top.connectors[0]); // eDP gone
        let err = select_pipelines_synth(
            &device,
            SynthRequest {
                width: 3840,
                height: 2160,
                refresh_millihz: 60_000,
                family: TimingFamily::Rb,
            },
        )
        .unwrap_err();
        assert!(matches!(err, SynthError::ClockCeiling { .. }));
    }

    /// The degenerate ask is typed, not truncated — and the honest
    /// menu now teaches the foundry's escape.
    #[test]
    fn degenerate_ask_and_the_menu_lesson() {
        use crate::mock::MockDevice;
        let device = MockDevice::laptop_dual();
        let err = select_pipeline_synth(
            &device,
            SynthRequest {
                width: 0,
                height: 1080,
                refresh_millihz: 60_000,
                family: TimingFamily::Rb,
            },
        )
        .unwrap_err();
        assert!(matches!(err, SynthError::Degenerate { .. }));

        // The sized-miss menu names the pour escape.
        let menu = SelectError::NoModeFor {
            wanted: ModeSize {
                width: 2560,
                height: 1440,
            },
            offered: vec![(1920, 1080)],
        }
        .to_string();
        assert!(menu.contains("--synth 2560x1440"));
    }
}
